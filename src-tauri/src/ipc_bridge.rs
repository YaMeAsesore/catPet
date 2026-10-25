//! IPC: comandos Tauri y eventos tipados hacia el frontend.
//!
//! Decisión: una sola “boca” de comandos sobre el router → la UI, MPRIS/SMTC y
//! los atajos globales hablan con la misma interfaz sin saber qué motor suena.
//!
//! Canales de eventos (todos con payload tipado):
//! - `playback://state`   → PlaybackStateEvent { engine, state: playing|paused|stopped, gen, ts_ms }
//! - `playback://track`   → TrackChangedEvent { engine, track: Option<TrackAttribution>, gen, ts_ms }
//! - `playback://position`→ { position_ms, duration_ms }
//! - `playback://volume` → { percent }
//! - `playback://queue`  → QueueEvent { current, length, rev }
//! - `spotify://account`  → SpotifyAccountInfo (al conectar/desconectar/cambiar gate)
//!
//! Compatibilidad: se mantiene `player://state` (string) y `queue://changed`
//! para clientes antiguos; la UI actual usa `playback://queue` (con `rev` para
//! no recargar toda la cola cuando solo cambia el cursor).
//!
//! Convención de argumentos (bug real 2026-10-11): Tauri v2 espera las claves en
//! **camelCase** por defecto (`ArgumentCase::Camel` en tauri-macros, salvo
//! `rename_all` explícito). Un param Rust `base_url` se invoca como `baseUrl`;
//! enviar snake_case falla con "missing required key". Ver `src/lib/tauri.ts`.

use crate::batch;
use crate::librespot_engine::{self, AudioBackend, SpotifyAccountInfo, SpotifyConfig};
use crate::playback_router::{self, EngineKind, UnifiedStatus};
use crate::ytmusic;
use crate::{AppState, db};
use tauri::{AppHandle, Emitter, Manager, State};

// ---------- Spotify ----------

/// Conecta el motor librespot. Verifica Premium ANTES de inicializar nada.
/// `pkce_token`: token de la sesión OAuth (Web API / Web Playback SDK).
#[tauri::command]
pub async fn spotify_connect(
    app: AppHandle,
    state: State<'_, AppState>,
    device_name: Option<String>,
    backend: Option<String>,
    pkce_token: Option<String>,
) -> Result<SpotifyAccountInfo, String> {
    let cfg = SpotifyConfig {
        device_name: device_name
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Música".to_string()),
        backend: parse_backend(backend.as_deref()),
        ..Default::default()
    };
    let shared = state.spotify.clone();
    let handle =
        librespot_engine::connect(&app, cfg, pkce_token.as_deref(), shared.clone()).await?;
    let info = librespot_engine::account_info(Some(&handle));
    *shared.lock().map_err(|_| "spotify lock".to_string())? = Some(handle);
    // Al conectar, el router pasa a modo Spotify si hay cola de Spotify pendiente.
    app.emit("spotify://account", &info).ok();
    app.emit("playback://state", playback_router::PlaybackStateEvent {
        engine: EngineKind::Spotify,
        state: "stopped",
        gen: 0,
        ts_ms: playback_router::now_ms(),
    })
    .ok();
    Ok(info)
}

/// Desconecta el dispositivo Connect pero MANTIENE las credenciales (reconexión
/// silenciosa en el próximo arranque).
#[tauri::command]
pub fn spotify_disconnect(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let handle = state
        .spotify
        .lock()
        .map_err(|_| "spotify lock".to_string())?
        .take();
    if let Some(h) = handle {
        h.shutdown();
    }
    let info = librespot_engine::account_info(None);
    app.emit("spotify://account", &info).ok();
    // Si el router estaba en Spotify, vuelve a libmpv para no dejar comandos huérfanos.
    {
        let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
        router.set_engine(&app, EngineKind::Libmpv)?;
    }
    Ok(())
}

/// Cierra sesión: desconecta y borra credenciales del keyring.
#[tauri::command]
pub fn spotify_logout(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    librespot_engine::clear_stored();
    spotify_disconnect(app, state)
}

/// Estado de la cuenta/motor Spotify para la UI (sin material de auth).
#[tauri::command]
pub fn spotify_account(state: State<'_, AppState>) -> Result<SpotifyAccountInfo, String> {
    let g = state.spotify.lock().map_err(|_| "spotify lock".to_string())?;
    Ok(librespot_engine::account_info(g.as_ref()))
}

/// Reproduce una URI de Spotify (`spotify:track:…`) por el motor librespot.
#[tauri::command]
pub async fn spotify_load(
    app: AppHandle,
    state: State<'_, AppState>,
    uri: String,
    start_playing: Option<bool>,
) -> Result<(), String> {
    // Clonamos la Session (Arc) y soltamos el lock antes del await: el guard de
    // std::sync::Mutex no es Send y el comando Tauri debe serlo.
    let session = {
        let g = state.spotify.lock().map_err(|_| "spotify lock".to_string())?;
        let h = g.as_ref().ok_or("Spotify no conectado")?;
        h.load_uri(&uri, start_playing.unwrap_or(true), 0)?;
        h.session.clone()
    };
    // Atribución visible (artista/álbum/carátula) vía librespot-metadata.
    let track = librespot_engine::current_attribution(&session, &uri)
        .await
        .unwrap_or_else(|| librespot_engine::TrackAttribution {
            uri: uri.clone(),
            title: "Spotify".into(),
            artist: String::new(),
            album: String::new(),
            art_url: String::new(),
            duration_ms: 0,
        });
    {
        let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
        router.set_engine(&app, EngineKind::Spotify)?;
        router.set_track_attribution(track.clone());
        playback_router::emit_track(&app, &router);
        playback_router::emit_state(&app, &router);
    }
    Ok(())
}

// ---------- Control de reproducción unificado ----------

/// Motor activo: "libmpv" | "spotify".
#[tauri::command]
pub fn playback_set_engine(
    app: AppHandle,
    state: State<'_, AppState>,
    engine: String,
) -> Result<UnifiedStatus, String> {
    let kind = match engine.as_str() {
        "libmpv" => EngineKind::Libmpv,
        "spotify" | "librespot" => EngineKind::Spotify,
        other => return Err(format!("motor desconocido: {other}")),
    };
    let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
    router.set_engine(&app, kind)?;
    Ok(router.status())
}

#[tauri::command]
pub fn playback_status(state: State<'_, AppState>) -> Result<UnifiedStatus, String> {
    let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
    Ok(router.status())
}

#[tauri::command]
pub async fn player_toggle(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    // Si nunca se reprodujo nada pero hay cola, arranca el actual (red:
    // fuera del runtime async).
    let need_play = {
        let router = state.router.lock().map_err(|_| "router lock".to_string())?;
        let never_started = router.current_track().is_none() && !router.spotify_ready();
        drop(router);
        let has_queue = state
            .queue
            .lock()
            .map_err(|_| "queue lock".to_string())?
            .current_track()
            .is_some();
        never_started && has_queue
    };
    if need_play {
        crate::play_current_user_blocking(app.clone(), "toggle").await?;
        return Ok(true);
    }
    let playing = {
        let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
        router.toggle()?
    };
    {
        let router = state.router.lock().map_err(|_| "router lock".to_string())?;
        playback_router::emit_state(&app, &router);
        crate::integrations::sync_playback_state(&app, router.paused_state());
    }
    Ok(playing)
}

/// Play explícito (MPRIS lo usa).
#[tauri::command]
pub fn player_play(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
    router.play()?;
    playback_router::emit_state(&app, &router);
    crate::integrations::sync_playback_state(&app, router.paused_state());
    Ok(())
}

/// Pause explícito (MPRIS lo usa).
#[tauri::command]
pub fn player_pause(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
    router.pause()?;
    playback_router::emit_state(&app, &router);
    crate::integrations::sync_playback_state(&app, router.paused_state());
    Ok(())
}

#[tauri::command]
pub fn player_seek(state: State<'_, AppState>, secs: f64) -> Result<(), String> {
    // La UI agrupa el arrastre (máx. ~5 seeks/s): mantener el comando síncrono
    // garantiza que se apliquen EN ORDEN y el último valor gane siempre.
    state
        .router
        .lock()
        .map_err(|_| "router lock".to_string())?
        .seek_secs(secs)
}

#[tauri::command]
pub fn player_volume(app: AppHandle, state: State<'_, AppState>, volume: i64) -> Result<(), String> {
    let vol = volume.clamp(0, 100) as u8;
    {
        let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
        router.set_volume(vol)?;
    }
    #[derive(serde::Serialize, Clone)]
    struct V {
        percent: u8,
    }
    app.emit("playback://volume", V { percent: vol }).ok();
    crate::persist_session_debounced(&app);
    Ok(())
}

/// Estado unificado incluyendo atribución del track actual.
#[tauri::command]
pub async fn player_status(app: AppHandle) -> Result<crate::player::PlayerStatus, String> {
    // La UI lo llama cada 500 ms; leer propiedades de libmpv no debe competir
    // con el pintado en el hilo principal.
    tokio::task::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or("sin estado".to_string())?;
        crate::timed("player_status", || {
        // Compatibilidad: la UI vieja lee PlayerStatus de mpv. Con Spotify activo
        // el progreso lo alimentan los eventos (ver playback_status para el completo).
        let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
        let st = match router.engine() {
            EngineKind::Libmpv => {
                // Refresca el latch de fin de pista para que el tick lo vea.
                router.refresh_eof();
                let mut live = router.mpv_status();
                live.eof = router.eof_seen();
                live
            }
            EngineKind::Spotify => crate::player::PlayerStatus {
                paused: router.paused_state(),
                pos_secs: (router.position_state() as f64) / 1000.0,
                dur_secs: (router.duration_state() as f64) / 1000.0,
                volume: router.volume() as i64,
                path: router.current_track().map(|t| t.uri),
                // Spirc avanza solo por su cola; el eof local no aplica aquí.
                eof: false,
                confirmed: true,
                error: None,
            },
        };
        Ok(st)
        })
    })
    .await
    .map_err(|e| format!("tarea de estado: {e}"))?
}

/// Reintenta la pista ACTUAL con resolución fresca (estrategia nueva, no la
/// misma URL): lo llama el tick una sola vez ante error sin confirmación.
/// Nunca relanza mientras la carga avanza (solo si no hay confirmación).
/// Devuelve true si reintentó; false si ya se agotó (pausa + UI coherente).
/// Async: la re-resolución (red) corre en el pool de bloqueo.
#[tauri::command]
pub async fn player_retry_current(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    // Misma intención (mismo gen, sin bumpear): el reintento no desordena.
    // Si el usuario ya cambió de pista, no se resucita la vieja.
    let (uri, gen) = {
        let router = state.router.lock().map_err(|_| "router lock".to_string())?;
        if router.engine() != EngineKind::Libmpv {
            return Ok(false);
        }
        // Nunca relanzar loadfile mientras la carga avanza (el watchdog con
        // progreso ya extiende el plazo; un loadfile encima emitiría end-file
        // de la carga anterior y cascaba saltos).
        if router.is_loading() {
            return Ok(true);
        }
        let uri = match router.current_track() {
            Some(t) => t.uri,
            None => return Ok(false),
        };
        let gen = match router.gen_for(&uri) {
            Some(g) => g,
            None => playback_router::request_gen("reintento", uri.trim_start_matches("ytmusic://")),
        };
        (uri, gen)
    };
    let allowed = {
        state.router.lock().map_err(|_| "router lock".to_string())?.retry_allowed(&uri)
    };
    if !allowed {
        {
            let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
            if let Err(e) = router.pause() {
                eprintln!("[musica] pausa tras reintento agotado: {e}");
            }
            playback_router::emit_state(&app, &router);
        }
        crate::integrations::sync_playback(&app);
        return Ok(false);
    }
    // Resolución fresca con otra estrategia (no repetir la misma URL) +
    // carga, todo bloqueante en el pool de bloqueo. Mismo gen (sin bumpear).
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || {
        // Si la intención ya cambió (el usuario saltó), no resucitar la vieja.
        if let Some(st) = app2.try_state::<crate::AppState>() {
            let still = st.router.lock().map(|r| r.current_track().is_some_and(|t| t.uri == uri)).unwrap_or(false);
            if !still {
                eprintln!("[gen] gen={gen} origen=reintento resultado=descartado-intencion-cambiada");
                return Ok(false);
            }
        }
        // Re-resuelve con cliente web (el normal usa android primero),
        // con IPv4 forzado. play_current reutiliza esa caché (sin 2ª red).
        if uri.starts_with("ytmusic://") {
            crate::ytmusic::invalidate_cache(&uri);
            match crate::ytmusic::resolve_audio_retry(&uri, Some(gen)) {
                Ok(_) => {}
                Err(e) if e == crate::ytmusic::STALE => return Ok(true),
                Err(e) => return Err(e),
            }
        }
        match app2.try_state::<crate::AppState>() {
            Some(st) => crate::play_current(&app2, &st, "reintento", playback_router::GenMode::Reuse(gen)).map(|_| true),
            None => Err("sin estado".into()),
        }
    })
    .await
    .map_err(|e| format!("tarea de reintento: {e}"))?
}

/// Auto-avance al terminar una pista (lo llama el tick de la UI al ver `eof`).
/// Atómico en backend a propósito: evaluar el latch, consumirlo y avanzar en el
/// mismo comando evita que dos ticks encadenen saltos (carrera imposible desde
/// el frontend). Solo motor Libmpv (Spirc se auto-gestiona). La última canción
/// se detiene con pausa (no envuelve: el siguiente manual sigue circular).
/// Si una pista falla (video no disponible, región, red), se SALTA a la
/// siguiente en vez de quedarse detenido (máx 6 intentos, cortafuegos a los 3
/// fallos seguidos); la última falla deja pausa + error visible. Devuelve qué
/// pasó para que la UI marque las fallidas (gris + motivo) y avise una sola vez.
#[tauri::command]
pub async fn player_track_ended(app: AppHandle, state: State<'_, AppState>) -> Result<AdvanceReport, String> {
    // Latch rápido (sin red): si no hay fin pendiente, se vuelve al instante.
    // R4: solo el eof de la pista actual avanza; si el usuario ya cambió, el
    // latch es viejo y se descarta (el clic no se pierde ni se salta).
    let has_eof = {
        let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
        if router.engine() != EngineKind::Libmpv {
            return Ok(AdvanceReport::idle());
        }
        // Motor muerto (ventana cerrada): recrearlo aquí mismo y seguir.
        if let Err(e) = router.ensure_mpv_alive() {
            eprintln!("[musica] no se pudo recrear mpv: {e}");
        }
        router.refresh_eof();
        router.take_eof_if_current()
    };
    if !has_eof {
        return Ok(AdvanceReport::idle());
    }
    // El avance resuelve red: fuera del runtime async.
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || advance_after_eof(&app2))
        .await
        .map_err(|e| format!("tarea de auto-avance: {e}"))?
}

/// Bucle de avance tras un fin consumido (bloqueante: resuelve red).
/// Salta fallos (máx 6, cortafuegos a los 3 fallos seguidos). Las marcadas en
/// caché negativa se saltan al instante (sin yt-dlp) y NO cuentan para el
/// cortafuegos: ese contador es solo para fallos reales de red o motor.
fn advance_after_eof(app: &AppHandle) -> Result<AdvanceReport, String> {
    let state = app
        .try_state::<crate::AppState>()
        .ok_or("sin estado".to_string())?;
    // R3: UN gen para todo el salto (no uno por pista saltada). El controlador
    // lo asigna una vez; cada candidata reutiliza el mismo.
    let ended_id = state
        .router
        .lock()
        .map(|r| r.current_track().map(|t| t.uri).unwrap_or_default())
        .unwrap_or_default();
    let ended_short = ended_id.strip_prefix("ytmusic://").unwrap_or(ended_id.as_str()).to_string();
    let gen = playback_router::request_gen("eof", &ended_short);
    let mut failed: Vec<FailedTrack> = vec![];
    let mut firewall = false;
    // Las muertas conocidas no consumen el presupuesto de 6 intentos reales:
    // una lista con muchas seguidas sigue saltando hasta una reproducible.
    let mut intentos = 0;
    loop {
        if intentos >= 6 {
            break;
        }
        let next_path = {
            let mut q = state.queue.lock().map_err(|_| "queue lock".to_string())?;
            match q.advance_if_not_last() {
                Some(t) => t.path.clone(),
                None => break,
            }
        };
        // Muerta conocida: salto instantáneo, sin red ni cortafuegos.
        let t_neg = std::time::Instant::now();
        if let Some(motivo) = crate::ytmusic::neg_get(&next_path) {
            eprintln!(
                "[musica] auto-avance salta pista muerta (neg): {motivo} en {}ms",
                t_neg.elapsed().as_millis()
            );
            failed.push(FailedTrack { path: next_path, reason: short_reason(&motivo) });
            continue;
        }
        intentos += 1;
        match crate::play_current(app, &state, "eof", playback_router::GenMode::Reuse(gen)) {
            Ok(()) => {
                if let Ok(mut r) = state.router.lock() {
                    r.note_success();
                }
                return Ok(AdvanceReport {
                    advanced: true,
                    failed,
                    stopped: false,
                    error: None,
                })
            }
            Err(e) => {
                // Throttle de YouTube: no quemar más intentos; pausa + aviso.
                if crate::ytmusic::looks_like_throttle(&e) {
                    {
                        let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
                        if let Err(pe) = router.pause() {
                            eprintln!("[musica] pausa tras throttle: {pe}");
                        }
                        playback_router::emit_state(&app, &router);
                    }
                    crate::integrations::sync_playback(&app);
                    return Ok(AdvanceReport {
                        advanced: false,
                        failed,
                        stopped: true,
                        error: Some(
                            "YouTube limitó las peticiones (429): espera unos minutos antes de seguir".into(),
                        ),
                    });
                }
                eprintln!("[musica] auto-avance salta pista fallida: {e}");
                failed.push(FailedTrack { path: next_path, reason: short_reason(&e) });
                let n = state
                    .router
                    .lock()
                    .map(|mut r| r.note_failure())
                    .unwrap_or(0);
                if n >= 3 {
                    firewall = true;
                    break;
                }
            }
        }
    }
    // Sin siguiente (o todo falló): detenerse dejando la UI coherente.
    // El aviso final solo aparece si la pista donde paramos fue la que falló.
    {
        let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
        if let Err(pe) = router.pause() {
            eprintln!("[musica] pausa tras auto-avance: {pe}");
        }
        playback_router::emit_state(&app, &router);
    }
    crate::integrations::sync_playback(&app);
    if firewall {
        return Ok(AdvanceReport {
            advanced: false,
            failed,
            stopped: true,
            error: Some("Se detuvo tras 3 fallos seguidos".into()),
        });
    }
    let error = (|| {
        let q = state.queue.lock().ok()?;
        let c = q.current?;
        let t = q.items.get(c)?;
        failed.iter().find(|f| f.path == t.path)
    })()
    .map(|f| format!("No se pudo reproducir: {}", f.reason));
    Ok(AdvanceReport { advanced: false, failed, stopped: true, error })
}

/// Motivo corto para el tooltip de pista no disponible.
fn short_reason(msg: &str) -> String {
    const MAX: usize = 140;
    let m = msg.trim();
    if m.len() <= MAX {
        return m.to_string();
    }
    format!("{}…", &m[..MAX])
}

#[derive(serde::Serialize, Clone)]
pub struct AdvanceReport {
    pub advanced: bool,
    /// Pistas que fallaron en el camino (para marcarlas en gris).
    pub failed: Vec<FailedTrack>,
    pub stopped: bool,
    /// Un único aviso final (si aplica).
    pub error: Option<String>,
}

impl AdvanceReport {
    fn idle() -> Self {
        Self { advanced: false, failed: vec![], stopped: false, error: None }
    }
}

#[derive(serde::Serialize, Clone)]
pub struct FailedTrack {
    pub path: String,
    pub reason: String,
}

/// Ajustes finos de shuffle/repeat (solo tienen sentido con Spotify activo).
#[tauri::command]
pub fn playback_shuffle(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    state
        .router
        .lock()
        .map_err(|_| "router lock".to_string())?
        .shuffle(on);
    Ok(())
}

#[tauri::command]
pub fn playback_repeat(state: State<'_, AppState>, mode: String) -> Result<(), String> {
    let router = state.router.lock().map_err(|_| "router lock".to_string())?;
    match mode.as_str() {
        "off" => {
            router.repeat_ctx(false);
            router.repeat_track(false);
        }
        "context" => router.repeat_ctx(true),
        "track" => router.repeat_track(true),
        other => return Err(format!("modo de repeat desconocido: {other}")),
    }
    Ok(())
}

// ---------- Cola unificada (eventos) ----------

/// Reproduce el índice de la cola por el motor que corresponda al path.
/// Async: la resolución de red corre en el pool de bloqueo.
#[tauri::command]
pub async fn player_play_index(app: AppHandle, state: State<'_, AppState>, index: usize) -> Result<(), String> {
    {
        let mut q = state.queue.lock().map_err(|_| "queue lock".to_string())?;
        if index >= q.items.len() {
            return Err("índice fuera de cola".into());
        }
        q.current = Some(index);
    }
    crate::play_current_user_blocking(app.clone(), "clic").await
}

/// Estado de la política de restricciones para la UI (presupuesto de saltos…).
#[tauri::command]
pub fn spotify_policy_status(
    state: State<'_, AppState>,
) -> Result<crate::spotify_policy::BudgetSnapshot, String> {
    let policy = {
        let g = state.spotify.lock().map_err(|_| "spotify lock".to_string())?;
        let h = g.as_ref().ok_or("Spotify no conectado")?;
        h.policy.clone()
    };
    let mut p = policy.lock().map_err(|_| "policy lock".to_string())?;
    Ok(p.snapshot())
}

// ---------- YouTube Music (yt-dlp, solo streaming) ----------

/// ¿Está yt-dlp disponible? (puerta para el mensaje de instalación).
#[tauri::command]
pub async fn youtube_probe() -> ytmusic::Probe {
    // Detección fresca (refleja una instalación hecha con la app abierta) pero
    // fuera del hilo de la UI: lanza 3 procesos (`--version`).
    tokio::task::spawn_blocking(ytmusic::probe_fresh)
        .await
        .unwrap_or_else(|_| ytmusic::probe())
}

/// Importa una página de una playlist/video (carga progresiva).
#[tauri::command]
pub async fn youtube_import(
    url: String,
    start: Option<u32>,
    end: Option<u32>,
) -> Result<YtImport, String> {
    let start = start.unwrap_or(1).max(1);
    let end = end.unwrap_or(100).clamp(start, start + 499);
    let pl = ytmusic::import_page(&url, start, end).await?;
    Ok(YtImport {
        title: pl.title,
        tracks: pl.tracks.into_iter().map(|t| t.to_track("YouTube")).collect(),
    })
}

#[derive(serde::Serialize, Clone)]
pub struct YtImport {
    pub title: String,
    pub tracks: Vec<db::Track>,
}

/// Estado de la sesión (lee el cookies.txt; persiste entre arranques).
#[tauri::command]
pub fn youtube_account() -> ytmusic::YtAccount {
    ytmusic::account()
}

/// L2: verifica la sesión ahora (botón "Verificar sesión"). Petición barata
/// (LL, un ítem); respeta el backoff de 429.
#[tauri::command]
pub async fn youtube_verify_session() -> ytmusic::SessionCheck {
    ytmusic::verify_session().await
}

/// Conecta exportando cookies del navegador elegido. Sin contraseñas.
#[tauri::command]
pub async fn youtube_connect(
    browser: String,
    profile: Option<String>,
) -> Result<ytmusic::YtAccount, String> {
    ytmusic::connect_browser(browser.trim(), profile.as_deref()).await?;
    Ok(ytmusic::account())
}

/// L3b: login integrado de Google en ventana con datos aislados (sin pedir ni
/// guardar la contraseña). Espera SAPISID+LOGIN_INFO, valida el candidato con
/// LL y solo entonces sustituye el jar. La UI sigue el progreso en
/// `youtube://login`.
#[tauri::command]
pub async fn youtube_google_login(app: AppHandle) -> Result<(), String> {
    crate::yt_login::start(&app)
}

/// L4: borra SOLO los datos aislados de la ventana de login integrado (se
/// ofrece al "Cerrar sesión"; nada se borra sin el clic del usuario).
#[tauri::command]
pub async fn youtube_clear_login_data(app: AppHandle) -> Result<(), String> {
    crate::yt_login::clear_data(&app).await
}

/// Navegadores con base de cookies encontrada (para el selector).
#[tauri::command]
pub fn youtube_browsers() -> Vec<ytmusic::BrowserProfile> {
    ytmusic::detect_browsers()
}

/// Respaldo: importa un cookies.txt exportado del navegador.
#[tauri::command]
pub async fn youtube_import_cookies(path: String) -> Result<ytmusic::YtAccount, String> {
    ytmusic::import_cookies_file(&path).await?;
    Ok(ytmusic::account())
}

/// Cierra sesión: borra cookies, metadatos y caché.
#[tauri::command]
pub fn youtube_disconnect(app: AppHandle) -> Result<(), String> {
    ytmusic::disconnect();
    app.emit("youtube://account", false).ok();
    Ok(())
}

/// "Mis playlists": Liked + canal propio (requiere sesión).
#[tauri::command]
pub async fn youtube_my_playlists() -> Result<Vec<ytmusic::YtMine>, String> {
    ytmusic::my_playlists().await
}

/// Configura el canal propio para listar sus playlists ("" lo quita).
#[tauri::command]
pub fn youtube_set_channel(url: String) -> Result<(), String> {
    ytmusic::set_channel(&url)
}

/// ¿Qué paths de YouTube ya están resueltos? (indicador "listo" en filas).
#[tauri::command]
pub async fn youtube_cached(paths: Vec<String>) -> Vec<bool> {
    // Puede llegar una lista de 10 000 ids al abrir/revelar una playlist.
    tokio::task::spawn_blocking(move || ytmusic::cached_paths(&paths))
        .await
        .unwrap_or_default()
}

/// ¿Qué paths están marcados no disponibles (caché negativa)? Motivo o null
/// por pista, para pintarlas en gris sin intentar nada.
#[tauri::command]
pub async fn youtube_dead(paths: Vec<String>) -> Vec<Option<String>> {
    tokio::task::spawn_blocking(move || ytmusic::neg_motives(&paths))
        .await
        .unwrap_or_default()
}

/// S4.2b: segundos que quedan del backoff de límite de YouTube (0 = libre).
/// «Emparejar todas» lo sondea para pausar con cuenta atrás sin lanzar yt-dlp.
#[tauri::command]
pub fn youtube_throttle_secs() -> u32 {
    ytmusic::throttle_remaining_secs()
}

/// Precalienta las primeras pistas de una lista recién mostrada (primer clic
/// casi instantáneo + muertas descubiertas por adelantado). Fire-and-forget:
/// respeta "la última gana" y el cupo de 2 yt-dlp.
#[tauri::command]
pub fn youtube_warm(paths: Vec<String>) {
    let all: Vec<String> = paths.into_iter().filter(|p| p.starts_with("ytmusic://")).collect();
    if !all.is_empty() {
        // current_idx=None: la función ordenará por orden de lista (primera = actual).
        ytmusic::warm_next(&all, ytmusic::current_gen(), None);
    }
}

/// Abre una playlist: lista plana barata + primer lote de filas INMEDIATO
/// (sin resolver audio). El precalentado corre en segundo plano.
#[tauri::command]
pub async fn youtube_batch_open(
    state: State<'_, AppState>,
    url: String,
) -> Result<batch::Snapshot, String> {
    // La ruta de la BD habilita la caché persistente de metadatos (reabrir
    // una playlist reciente no vuelve a pedirla a YouTube).
    let db_path = state.db_path.clone();
    batch::open_with_cache(url, Some(db_path)).await
}

/// Instantánea del lote (la UI la polla máx. ~2 veces por segundo mientras
/// precalienta las primeras pistas).
#[tauri::command]
pub async fn youtube_batch_progress() -> batch::Snapshot {
    tokio::task::spawn_blocking(batch::progress)
        .await
        .unwrap_or_else(|_| batch::progress())
}

/// "Cargar más": revela el siguiente lote al instante; el audio se prepara
/// en segundo plano.
#[tauri::command]
pub async fn youtube_batch_more() -> Result<batch::Snapshot, String> {
    tokio::task::spawn_blocking(batch::load_more)
        .await
        .map_err(|e| format!("tarea de lote: {e}"))?
}

/// Cambiar de vista/playlist: cancela la carga pendiente (lo que ya está
/// resolviendo termina y se guarda en caché).
#[tauri::command]
pub fn youtube_batch_cancel() {
    batch::cancel();
}

/// Estado del «Me gusta» de un tema en YouTube Music. `None` = desconocido
/// (p. ej. el vídeo no permite like). Se usa al pulsar si el listado no lo
/// traía (fallback yt-dlp/caché antigua).
#[tauri::command]
pub async fn youtube_like_status(video_id: String) -> Result<Option<bool>, String> {
    let id = video_id.strip_prefix("ytmusic://").unwrap_or(&video_id);
    crate::yt_reco::like_status(id)
        .await
        .map_err(map_like_error)
}

/// Guarda/quita el «Me gusta» en YouTube Music (corazón de la app web).
#[tauri::command]
pub async fn youtube_set_like(video_id: String, liked: bool) -> Result<(), String> {
    let id = video_id.strip_prefix("ytmusic://").unwrap_or(&video_id);
    crate::yt_reco::set_like(id, liked)
        .await
        .map_err(map_like_error)
}

/// Guarda/quita una playlist de la biblioteca de YouTube Music. En la web se
/// llama «Guardar en la biblioteca» (marcador), no «Me gusta» de una pista.
#[tauri::command]
pub async fn youtube_set_playlist_like(
    playlist_id: String,
    saved: bool,
) -> Result<(), String> {
    let id = playlist_id.strip_prefix("VL").unwrap_or(&playlist_id);
    crate::yt_reco::set_playlist_like(id, saved)
        .await
        .map_err(map_like_error)?;
    crate::batch::mark_playlist_saved(id, saved);
    Ok(())
}

fn map_like_error(e: crate::yt_reco::RecoError) -> String {
    use crate::yt_reco::RecoErrorCode as C;
    match e.code {
        C::NoSession => {
            "Conecta tu cuenta de YouTube Music para guardar «Me gusta»".to_string()
        }
        C::Auth => {
            "La sesión de YouTube Music no es válida o expiró; vuelve a conectar".to_string()
        }
        C::Http if e.message.contains("429") => {
            "YouTube limitó las peticiones; intenta de nuevo en unos minutos".to_string()
        }
        _ => e.message,
    }
}

/// P6: home personalizado de YouTube Music. `force` ignora la caché en
/// memoria de 10 min. El cliente interno vive en `yt_reco`; este puente no
/// toca cookies ni añade datos a los errores.
#[tauri::command]
pub async fn youtube_recommendations(force: bool) -> Result<crate::yt_reco::RecoFeed, crate::yt_reco::RecoError> {
    crate::yt_reco::recommendations(force).await
}

/// Log del frontend (errores no capturados, promesas rechazadas) al archivo
/// de diagnóstico. Sin secretos: el llamante no debe mandar tokens/cookies.
#[tauri::command]
pub fn report_error(app: AppHandle, message: String) {
    let dir = std::env::var_os("MUSICA_DATA_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| app.path().app_data_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let line = format!("{} [ui] {}\n", chrono_now(), message.chars().take(500).collect::<String>());
    std::fs::create_dir_all(&dir).ok();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("musica.log"))
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .ok();
}

fn chrono_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}", d.as_secs()))
        .unwrap_or_default()
}

// ---------- Búsqueda en el catálogo Spotify ----------

/// Busca tracks en Spotify vía Web API (con el token PKCE del keyring).
#[tauri::command]
pub async fn spotify_search(
    _state: State<'_, AppState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<db::Track>, String> {
    let token = crate::keyring::get(crate::keyring::SERVICE_PKCE, "access_token")
        .ok_or("Spotify no conectado (sin token)")?;
    let limit = limit.unwrap_or(20).clamp(1, 50);
    let resp = reqwest::Client::new()
        .get("https://api.spotify.com/v1/search")
        .bearer_auth(&token)
        .query(&[
            ("q", query.as_str()),
            ("type", "track"),
            ("limit", &limit.to_string()),
        ])
        .send()
        .await
        .map_err(|e| format!("búsqueda: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("búsqueda rechazada ({})", resp.status().as_u16()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("respuesta ilegible: {e}"))?;
    let mut out = Vec::new();
    if let Some(items) = body.get("tracks").and_then(|t| t.get("items")).and_then(|i| i.as_array()) {
        for it in items {
            out.push(db::Track {
                id: 0,
                path: it.get("uri").and_then(|u| u.as_str()).unwrap_or_default().to_string(),
                title: it.get("name").and_then(|n| n.as_str()).unwrap_or("?").to_string(),
                artist: it
                    .get("artists")
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|a| a.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string(),
                album: it
                    .get("album")
                    .and_then(|a| a.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string(),
                genre: "spotify".into(),
                year: 0,
                track_no: 0,
                duration_secs: it
                    .get("duration_ms")
                    .and_then(|d| d.as_u64())
                    .map(|d| (d / 1000) as i64)
                    .unwrap_or(0),
                mtime: 0,
                art_url: it
                    .get("album")
                    .and_then(|a| a.get("images"))
                    .and_then(|i| i.as_array())
                    .and_then(|a| a.first())
                    .and_then(|img| img.get("url"))
                    .and_then(|u| u.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    Ok(out)
}

// ---------- helpers ----------

fn parse_backend(s: Option<&str>) -> AudioBackend {
    match s.map(|s| s.to_ascii_lowercase()) {
        Some(b) if b == "alsa" => AudioBackend::Alsa,
        Some(b) if b == "pulse" || b == "pulseaudio" => AudioBackend::PulseAudio,
        Some(b) if b == "rodio" => AudioBackend::Rodio,
        _ => AudioBackend::Auto,
    }
}

/// Helper de testabilidad: parseo de nombres de motor desde el frontend.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_backend_por_nombre() {
        assert_eq!(parse_backend(Some("ALSA")), AudioBackend::Alsa);
        assert_eq!(parse_backend(Some("pulse")), AudioBackend::PulseAudio);
        assert_eq!(parse_backend(Some("rodio")), AudioBackend::Rodio);
        assert_eq!(parse_backend(Some("cualquiera")), AudioBackend::Auto);
        assert_eq!(parse_backend(None), AudioBackend::Auto);
    }

    #[test]
    fn nombres_de_motor_coinciden_con_router() {
        assert_eq!(playback_router::engine_for_track("spotify:track:x"), EngineKind::Spotify);
        assert_eq!(playback_router::engine_for_track("/a.mp3"), EngineKind::Libmpv);
    }
}
