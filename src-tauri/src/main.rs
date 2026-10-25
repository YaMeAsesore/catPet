//! Punto de entrada: Estado global + comandos IPC usados por el frontend.
//! Decisión: abrir una conexión SQLite por comando porque rusqlite Connection no es Send.
//! Decisión: todo el audio pasa por el PlaybackRouter (libmpv | librespot), que
//! Mantiene una sola cola, un solo estado y unos eventos IPC unificados.

// En Windows release no debe abrirse una consola negra al lanzar el .exe.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod batch;
mod csv_import;
mod csv_match;
mod db;
mod download;
mod integrations;
mod ipc_bridge;
mod keyring;
mod librespot_engine;
mod library;
mod player;
mod playback_router;
mod playlists;
mod providers;
mod queue;
mod remote;
mod spotify_policy;
mod tools;
mod webm_sniff;
mod yt_login;
mod yt_reco;
mod ytmusic;

use playback_router::{EngineKind, PlaybackRouter};
use providers::Registry;
use queue::TrackQueue;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

pub struct AppState {
    db_path: PathBuf,
    /// Raíz de playlists locales (`<datos-app>/playlists/<nombre>/playlist.m3u8`).
    pub playlists_root: PathBuf,
    /// Única puerta de entrada al audio: libmpv y librespot detrás del router.
    pub router: Mutex<PlaybackRouter>,
    /// Handle vivo del motor Spotify (None = desconectado).
    pub spotify: librespot_engine::SharedSpotify,
    queue: Mutex<TrackQueue>,
    /// Mutex porque altas/bajas de fuentes lo reconstruyen en caliente.
    providers: Mutex<Registry>,
}

#[derive(Serialize)]
struct ScanResult {
    added: usize,
    kept: usize,
    removed: usize,
    total: i64,
}

// ---------- Biblioteca ----------

#[tauri::command]
async fn scan_folder(app: AppHandle, state: State<'_, AppState>, folder: String) -> Result<ScanResult, String> {
    // Escanear y leer etiquetas puede tardar minutos con una carpeta grande.
    // Fuera del hilo de la UI para que la ventana siga respondiendo.
    let db_path = state.db_path.clone();
    let (result, total) = tokio::task::spawn_blocking(move || {
        let stats = library::scan_folder(&db_path, std::path::Path::new(&folder))?;
        let conn = db::open(&db_path).map_err(|e| e.to_string())?;
        let total = db::count(&conn);
        Ok::<_, String>((stats, total))
    })
    .await
    .map_err(|e| format!("tarea de escaneo: {e}"))??;
    app.emit("library://changed", &total).ok();
    Ok(ScanResult { added: result.added, kept: result.kept, removed: result.removed, total })
}

/// Búsqueda en la biblioteca. `async` + `spawn_blocking`: en Tauri los
/// comandos síncronos corren en el hilo principal; abrir SQLite y consultar
/// FTS ahí hacía que cada tecla compitiera con el pintado de la UI.
#[tauri::command]
async fn library_search(
    state: State<'_, AppState>,
    query: String,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<db::Track>, String> {
    let db_path = state.db_path.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db::open(&db_path).map_err(|e| e.to_string())?;
        db::search(
            &conn,
            &query,
            limit.unwrap_or(300).clamp(1, 1000),
            offset.unwrap_or(0).max(0), // OFFSET negativo es error SQLite
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("tarea de búsqueda: {e}"))?
}

#[tauri::command]
async fn library_stats(state: State<'_, AppState>) -> Result<i64, String> {
    let db_path = state.db_path.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db::open(&db_path).map_err(|e| e.to_string())?;
        Ok::<i64, String>(db::count(&conn))
    })
    .await
    .map_err(|e| format!("tarea de estadísticas: {e}"))?
}

// ---------- Cola ----------

#[tauri::command]
fn queue_set(app: AppHandle, state: State<AppState>, tracks: Vec<db::Track>, start: usize) -> Result<(), String> {
    {
        let mut q = state.queue.lock().map_err(|_| "queue lock".to_string())?;
        q.set_list(tracks, start);
    }
    emit_queue(&app, &state);
    Ok(())
}

#[tauri::command]
fn queue_add(app: AppHandle, state: State<AppState>, track: db::Track) -> Result<(), String> {
    {
        let mut q = state.queue.lock().map_err(|_| "queue lock".to_string())?;
        q.push(track);
    }
    emit_queue(&app, &state);
    Ok(())
}

/// Añade un lote de pistas al final sin mover el cursor actual (lo usa la
/// paginación de YouTube para que el auto-avance siga más allá del lote).
#[tauri::command]
fn queue_append(
    app: AppHandle,
    state: State<AppState>,
    tracks: Vec<db::Track>,
) -> Result<(), String> {
    {
        let mut q = state.queue.lock().map_err(|_| "queue lock".to_string())?;
        q.append(tracks);
    }
    emit_queue(&app, &state);
    Ok(())
}

/// Mide un comando rápido y avisa si retuvo un lock >200 ms.
/// (El hilo principal + runtime se defienden con el latido de `setup` y los
/// tramos `[perf]`/`[yt-*]`; esto caza contención de mutex en lecturas.)
pub(crate) fn timed<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let t0 = std::time::Instant::now();
    let r = f();
    let ms = t0.elapsed().as_millis();
    if ms > 200 {
        eprintln!("[watchdog] comando={name} {ms}ms (¿contención de lock?)");
    }
    r
}

#[tauri::command]
async fn queue_remove(app: AppHandle, state: State<'_, AppState>, index: usize) -> Result<(), String> {
    let (was_current, empty) = {
        let mut q = state.queue.lock().map_err(|_| "queue lock".to_string())?;
        let was_current = q.current == Some(index);
        q.remove(index);
        (was_current, q.items.is_empty())
    };
    if empty {
        // Sin cola no hay nada que seguir sonando: detiene el motor activo.
        if let Ok(mut r) = state.router.lock() {
            if let Err(e) = r.stop() {
                eprintln!("[musica] stop al vaciar cola: {e}");
            }
            r.clear_track();
        }
        integrations::sync_playback(&app);
    } else if was_current {
        // Quitar el tema que suena = saltarlo: reproduce el nuevo actual en vez
        // de dejar el archivo viejo sonando huérfano (fuera del runtime async).
        play_current_user_blocking(app.clone(), "quitar").await?;
    }
    emit_queue(&app, &state);
    Ok(())
}

#[tauri::command]
fn queue_move(app: AppHandle, state: State<AppState>, from: usize, to: usize) -> Result<(), String> {
    {
        let mut q = state.queue.lock().map_err(|_| "queue lock".to_string())?;
        q.move_item(from, to);
    }
    emit_queue(&app, &state);
    Ok(())
}

#[tauri::command]
fn queue_clear(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    {
        let mut q = state.queue.lock().map_err(|_| "queue lock".to_string())?;
        q.clear();
    }
    // Limpiar detiene la reproducción del motor activo.
    if let Ok(mut r) = state.router.lock() {
        if let Err(e) = r.stop() {
            eprintln!("[musica] stop al limpiar cola: {e}");
        }
        r.clear_track();
    }
    integrations::sync_playback(&app);
    emit_queue(&app, &state);
    Ok(())
}

#[tauri::command]
async fn queue_list(app: AppHandle) -> Result<TrackQueue, String> {
    // Clonar 10 000 pistas y serializarlas no debe bloquear el hilo de la UI.
    tokio::task::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or("sin estado".to_string())?;
        timed("queue_list", || {
            Ok(state.queue.lock().map_err(|_| "queue lock".to_string())?.clone())
        })
    })
    .await
    .map_err(|e| format!("tarea de cola: {e}"))?
}

/// Emite eventos unificados de cola (`playback://queue` + legacy `queue://changed`).
fn emit_queue(app: &AppHandle, state: &State<AppState>) {
    let (current, length, rev) = {
        let q = match state.queue.lock() {
            Ok(q) => q,
            Err(_) => return,
        };
        (q.current, q.items.len(), q.rev)
    };
    playback_router::emit_queue(app, current, length, rev);
    persist_session_debounced(app);
}

// ---- Persistencia de sesión (cola + volumen) ----
// Escrituras agrupadas: una ráfaga de 30 saltos no debe serializar la cola 30
// veces. El hilo dedicado guarda ~1,2 s después del último cambio.

static SESSION_DIRTY: AtomicBool = AtomicBool::new(false);
static SESSION_SAVER: AtomicBool = AtomicBool::new(false);

pub(crate) fn persist_session_debounced(app: &AppHandle) {
    SESSION_DIRTY.store(true, Ordering::SeqCst);
    if SESSION_SAVER.swap(true, Ordering::SeqCst) {
        return; // ya hay un guardador activo
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("musica-save".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(1200));
            if SESSION_DIRTY.swap(false, Ordering::SeqCst) {
                save_session(&app);
                continue;
            }
            SESSION_SAVER.store(false, Ordering::SeqCst);
            // Ventana de carrera: si llegó un cambio justo al soltar el flag,
            // reclamamos el guardado en vez de perderlo.
            if SESSION_DIRTY.load(Ordering::SeqCst) && !SESSION_SAVER.swap(true, Ordering::SeqCst) {
                continue;
            }
            break;
        });
    if spawned.is_err() {
        SESSION_SAVER.store(false, Ordering::SeqCst);
    }
}

fn save_session(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let db_path = state.db_path.clone();
    let (queue, volume) = {
        let Ok(q) = state.queue.lock() else { return };
        (q.clone(), state.router.lock().map(|r| r.volume()).unwrap_or(80))
    };
    // Serializar 10 000 pistas FUERA del lock: no debe frenar un clic/salto.
    let Ok(queue_json) = serde_json::to_string(&queue) else { return };
    if let Ok(conn) = db::open(&db_path) {
        if let Err(e) = db::state_set(&conn, "queue", &queue_json) {
            eprintln!("[musica] no se pudo guardar la cola: {e}");
        }
        db::state_set(&conn, "volume", &volume.to_string()).ok();
    }
}

fn save_session_now(app: &AppHandle) {
    SESSION_DIRTY.store(false, Ordering::SeqCst);
    save_session(app);
}

// ---------- Playlists locales ----------

#[tauri::command]
async fn playlist_list(state: State<'_, AppState>) -> Result<Vec<playlists::PlaylistInfo>, String> {
    let root = state.playlists_root.clone();
    tokio::task::spawn_blocking(move || Ok(playlists::list(&root)))
        .await
        .map_err(|e| format!("tarea de playlists: {e}"))?
}

#[tauri::command]
fn playlist_create(state: State<'_, AppState>, name: String) -> Result<playlists::PlaylistInfo, String> {
    let name = playlists::create(&state.playlists_root, &name)?;
    Ok(playlists::PlaylistInfo { count: 0, name })
}

#[tauri::command]
fn playlist_rename(
    state: State<'_, AppState>,
    old_name: String,
    new_name: String,
) -> Result<playlists::PlaylistInfo, String> {
    let name = playlists::rename(&state.playlists_root, &old_name, &new_name)?;
    let count = playlists::read_tracks(&state.playlists_root, &name).len();
    Ok(playlists::PlaylistInfo { name, count })
}

#[tauri::command]
fn playlist_delete(state: State<'_, AppState>, name: String) -> Result<(), String> {
    playlists::delete(&state.playlists_root, &name)
}

#[tauri::command]
async fn playlist_tracks(state: State<'_, AppState>, name: String) -> Result<Vec<db::Track>, String> {
    let root = state.playlists_root.clone();
    tokio::task::spawn_blocking(move || {
        if !root.join(&name).is_dir() {
            return Err("la playlist no existe".into());
        }
        Ok(playlists::read_tracks(&root, &name))
    })
    .await
    .map_err(|e| format!("tarea de playlist: {e}"))?
}

#[tauri::command]
fn playlist_add(
    state: State<'_, AppState>,
    name: String,
    track: db::Track,
) -> Result<bool, String> {
    playlists::add_track(&state.playlists_root, &name, &track)
}

#[tauri::command]
fn playlist_remove(
    state: State<'_, AppState>,
    name: String,
    path: String,
) -> Result<bool, String> {
    playlists::remove_track(&state.playlists_root, &name, &path)
}

/// S4: fija otra versión de una pista `ytmusic://` (mismo lugar, mismos
/// metadatos: solo cambia la referencia del vídeo).
#[tauri::command]
fn playlist_replace_track(
    state: State<'_, AppState>,
    name: String,
    old_path: String,
    track: db::Track,
) -> Result<bool, String> {
    playlists::replace_track(&state.playlists_root, &name, &old_path, &track)
}

#[tauri::command]
async fn playlist_add_folder(
    state: State<'_, AppState>,
    name: String,
    folder: String,
) -> Result<usize, String> {
    let root = state.playlists_root.clone();
    tokio::task::spawn_blocking(move || {
        playlists::playlist_add_folder(&root, &name, std::path::Path::new(&folder))
    })
    .await
    .map_err(|e| format!("tarea de playlist: {e}"))?
}

// ---------- Fuentes remotas (fase 1) ----------

/// Recarga el registro de proveedores desde la tabla `sources`.
fn reload_registry(state: &State<AppState>) -> Result<(), String> {
    let fresh = Registry::load(&state.db_path);
    *state.providers.lock().map_err(|_| "providers lock".to_string())? = fresh;
    Ok(())
}

#[tauri::command]
async fn source_list(state: State<'_, AppState>) -> Result<Vec<db::Source>, String> {
    let db_path = state.db_path.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db::open(&db_path).map_err(|e| e.to_string())?;
        db::list_sources(&conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("tarea de fuentes: {e}"))?
}

/// Alta con validación: Subsonic/Jellyfin hacen login, podcast descarga el feed.
/// Guarda solo material de autenticación (salt+token / api token), nunca la clave.
#[tauri::command]
async fn source_add(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: String,
    name: String,
    base_url: String,
    user: String,
    secret: String,
) -> Result<db::Source, String> {
    let kind = kind.to_lowercase();
    if base_url.trim().is_empty() {
        return Err("la URL es obligatoria".into());
    }
    if kind != "radio" && name.trim().is_empty() {
        return Err("el nombre es obligatorio".into());
    }
    let mut final_name = name.trim().to_string();
    let mut final_url = base_url.trim().to_string();
    let (user, secret, extra) = match kind.as_str() {
        "subsonic" => {
            // `secret` es la contraseña: se valida y se descarta (queda salt+token).
            let api = remote::SubsonicApi::login(-1, &base_url, &user, &secret).await?;
            (api.user, api.salt, api.token)
        }
        "jellyfin" => {
            let api = remote::JellyfinApi::login(-1, &base_url, &user, &secret).await?;
            (api.user_id, api.token, String::new())
        }
        "radio" => {
            // Acepta un stream directo o un enlace de emisora de Radio Garden
            // (se resuelve al stream real). El nombre es opcional.
            let r = remote::resolve_radio(&final_url, &final_name).await?;
            final_name = r.name;
            final_url = r.url;
            let extra = serde_json::json!({
                "artwork": r.art_url,
                "place": r.place,
                "resolved": true,
            })
            .to_string();
            (String::new(), String::new(), extra)
        }
        "podcast" => {
            // Valida que el feed descargue y parsee antes de guardar.
            remote::fetch_episodes(&base_url).await.map(|_| ())?;
            (String::new(), String::new(), String::new())
        }
        "youtube" => {
            // Playlist o video de YouTube Music: valida forma + 1 pista.
            crate::ytmusic::classify_url(&base_url)?;
            crate::ytmusic::import_page(&base_url, 1, 1).await.map(|_| ())?;
            (String::new(), String::new(), String::new())
        }
        _ => return Err("tipo de fuente desconocido".into()),
    };
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    // Radio: la misma emisora resuelta no se guarda dos veces.
    if kind == "radio" {
        if let Some(existing) =
            db::find_source_by_url(&conn, "radio", &final_url).map_err(|e| e.to_string())?
        {
            return Ok(existing);
        }
    }
    let src = db::add_source(
        &conn,
        &db::Source { id: 0, kind, name: final_name, base_url: final_url, user, secret, extra },
    )
    .map_err(|e| e.to_string())?;
    reload_registry(&state)?;
    app.emit("library://changed", db::count(&conn)).ok();
    Ok(src)
}

#[tauri::command]
fn source_remove(app: AppHandle, state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    db::remove_source(&conn, id).map_err(|e| e.to_string())?;
    reload_registry(&state)?;
    app.emit("library://changed", db::count(&conn)).ok();
    Ok(())
}

/// Renombra una fuente: SOLO el nombre (ni URL ni credenciales).
#[tauri::command]
fn source_rename(app: AppHandle, state: State<AppState>, id: i64, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("el nombre no puede estar vacío".into());
    }
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    let n = db::rename_source(&conn, id, &name).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("la fuente no existe".into());
    }
    reload_registry(&state)?;
    app.emit("library://changed", db::count(&conn)).ok();
    Ok(())
}

/// Búsqueda en una fuente remota (las radios devuelven su emisora; podcasts filtran).
#[tauri::command]
async fn remote_search(
    state: State<'_, AppState>,
    source_id: i64,
    query: String,
) -> Result<Vec<db::Track>, String> {
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    let src = db::get_source(&conn, source_id)
        .map_err(|e| e.to_string())?
        .ok_or("fuente inexistente")?;
    match src.kind.as_str() {
        "subsonic" => {
            remote::SubsonicApi {
                source_id: src.id,
                base_url: src.base_url,
                user: src.user,
                salt: src.secret,
                token: src.extra,
            }
            .search_songs(&query)
            .await
        }
        "jellyfin" => {
            remote::JellyfinApi {
                source_id: src.id,
                base_url: src.base_url,
                token: src.secret,
                user_id: src.user,
            }
            .search_audio(&query)
            .await
        }
        "podcast" => {
            let eps = remote::fetch_episodes(&src.base_url).await?;
            let q = query.to_lowercase();
            Ok(if q.trim().is_empty() {
                eps
            } else {
                eps.into_iter().filter(|t| t.title.to_lowercase().contains(&q)).collect()
            })
        }
        "radio" => {
            // `extra` guarda carátula, ciudad y si la URL ya es el stream final.
            let meta: serde_json::Value =
                serde_json::from_str(&src.extra).unwrap_or(serde_json::Value::Null);
            let already = meta.get("resolved").and_then(|v| v.as_bool()).unwrap_or(false);
            let (url, title, art_url, place) = if already {
                (
                    src.base_url.clone(),
                    src.name.clone(),
                    meta.get("artwork").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    meta.get("place").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                )
            } else {
                // Radios guardadas antes del resolver: la URL puede ser una
                // página (p. ej. radio.garden). Se resuelve ahora y se guarda.
                let r = remote::resolve_radio(&src.base_url, &src.name).await?;
                let extra = serde_json::json!({
                    "artwork": r.art_url,
                    "place": r.place,
                    "resolved": true,
                })
                .to_string();
                db::update_source_radio(&conn, src.id, &r.name, &r.url, &extra)
                    .map_err(|e| e.to_string())?;
                (r.url, r.name, r.art_url, r.place)
            };
            Ok(vec![db::Track {
                id: 0,
                path: url,
                title,
                artist: "Radio".into(),
                album: if place.is_empty() { "Directo".into() } else { place },
                genre: "radio".into(),
                year: 0,
                track_no: 0,
                duration_secs: 0, // directo: duración desconocida
                mtime: 0,
                art_url,
            }])
        }
        "youtube" => {
            // G1: antes estaba topado a 100 ("--playlist-end 100"), así que
            // una playlist de 114 solo mostraba 100 filas y sin posibilidad
            // de cargar el resto. Usa el MISMO tope que la ruta por lotes.
            let pl = crate::ytmusic::import_page(&src.base_url, 1, crate::batch::FLAT_CAP).await?;
            eprintln!(
                "[lote] fuente-guardada={} lista-plana total={}",
                src.name,
                pl.tracks.len()
            );
            Ok(pl.tracks.into_iter().map(|t| t.to_track(&src.name)).collect())
        }
        _ => Err("tipo de fuente desconocida".into()),
    }
}

// ---------- Reproducción (vía PlaybackRouter) ----------

/// Carga el track actual de la cola en el motor que corresponda a su path.
/// Punto único usado por UI, MPRIS/SMTC y atajos (T2: toda intención de play
/// pasa por aquí con su origen; el gen lo asigna el controlador o se reutiliza).
///
/// OJO concurrencia: NO mantener el lock del router mientras se llama a
/// `sync_playback` (ese re-bloquea el router y era un deadlock: tras el primer
/// play ningún botón respondía aunque el audio seguía sonando).
pub(crate) fn play_current(
    app: &AppHandle,
    state: &State<AppState>,
    origin: playback_router::Origin,
    gen_mode: playback_router::GenMode,
) -> Result<(), String> {
    let t_all = std::time::Instant::now();
    // Instantánea de la cola SIN retener el lock durante la resolución: retener
    // un Mutex durante 1-7 s de red bloqueaba queue_list/avance y apilaba
    // comandos hasta colgar la ventana ("no responde").
    let track = {
        state
            .queue
            .lock()
            .map_err(|_| "queue lock".to_string())?
            .current_track()
            .ok_or("cola vacía")?
    };
    // G4: el índice de cola ya lo movió quien llamó (siguiente/anterior/eof/
    // clic). Se avisa ANTES de resolver para que la UI resalte el destino en
    // <100 ms en lugar de esperar 1-7 s a la resolución de red.
    emit_queue(app, state);
    // Gen de la intención (T1a/T2): nuevo (secuencial, en orden) o reutilizado
    // (salto/reintento de la misma intención: no bumpea, no desordena).
    let vid = track
        .path
        .strip_prefix("ytmusic://")
        .unwrap_or(track.path.as_str())
        .to_string();
    let gen = match gen_mode {
        playback_router::GenMode::New => playback_router::request_gen(origin, &vid),
        playback_router::GenMode::Reuse(g) => {
            eprintln!("[gen] gen={g} origen={origin} id={vid} reutilizado ts={}", playback_router::now_ms());
            g
        }
    };
    // YouTube Music: URL de audio perezosa (solo al sonar) + caché con TTL.
    // Corre SIN locks retenidos (ver arriba); mpv tardaría lo mismo en abrirla.
    // Con generación: si el usuario cambia de pista a mitad de resolución,
    // la vieja se aborta y NO pisa a la actual ("la última gana").
    let mut resolve_ms = 0u128;
    let resolved = if track.path.starts_with("ytmusic://") {
        // Una línea [perf] por transición, siempre (id, gen, resultado).
        // Sin URLs ni IPs: el motivo ya viene clasificado y corto.
        let t0 = std::time::Instant::now();
        match ytmusic::resolve_audio_gen(&track.path, Some(gen), ytmusic::SlotKind::User) {
            Ok(url) => {
                resolve_ms = t0.elapsed().as_millis();
                eprintln!(
                    "[perf] transicion id={vid} gen={gen} origen={origin} resultado=ok resolve={resolve_ms}ms hijos={}",
                    ytmusic::live_children()
                );
                url
            }
            Err(e) if e == ytmusic::STALE => {
                // T1c: todo descarte se registra con su porqué (gen actual).
                eprintln!(
                    "[gen] gen={gen} id={vid} origen={origin} resultado=descartado-stale gen_actual={}",
                    ytmusic::current_gen()
                );
                eprintln!("[perf] transicion id={vid} gen={gen} origen={origin} resultado=cancelada-stale");
                return Ok(());
            }
            Err(e) => {
                let motivo: String = e.chars().take(120).collect();
                eprintln!("[perf] transicion id={vid} gen={gen} origen={origin} resultado=error motivo={motivo}");
                // Sin sesión / expirada: aviso discreto sin romper nada.
                if ytmusic::looks_like_auth(&e) {
                    app.emit("youtube://expired", ()).ok();
                }
                // W3: límite de YouTube (429/not-a-bot): UN aviso por episodio.
                if ytmusic::looks_like_throttle(&e) && ytmusic::take_throttle_notice() {
                    app.emit("youtube://throttled", ()).ok();
                }
                return Err(e);
            }
        }
    } else {
        state
            .providers
            .lock()
            .map_err(|_| "providers lock".to_string())?
            .resolve(&track.path)
    };
    // Una intención posterior pudo ganar mientras resolvíamos la URL. Sin esta
    // comprobación, una resolución lenta podía cargar en mpv una pista VIEJA
    // justo después de que el usuario saltara varias veces (audio/UI
    // desincronizados y sensación de bloqueo).
    if gen != ytmusic::current_gen() {
        let actual = ytmusic::current_gen();
        eprintln!(
            "[gen] gen={gen} id={vid} origen={origin} resultado=descartado-antes-de-cargar gen_actual={actual}"
        );
        return Ok(());
    }
    // 1) Cargar en el motor y soltar el lock (play_track emite sus eventos).
    // Si el core murió, play_track lo recrea y reintenta UNA vez.
    // Para YouTube: resuelve DENTRO de play_track (para detectar clics duplicados).
    // Para otras fuentes: resuelve aquí y pasa la URL.
    let resolved_for_router = if track.path.starts_with("ytmusic://") {
        None
    } else {
        Some(resolved.as_str())
    };
    let t_load = std::time::Instant::now();
    let att = {
        let mut router = state.router.lock().map_err(|_| "router lock".to_string())?;
        match router.play_track(app, &track, gen, resolved_for_router) {
            Ok(()) => router.current_track(),
            Err(e) => {
                if track.path.starts_with("ytmusic://") {
                    let motivo: String = e.chars().take(120).collect();
                    eprintln!("[perf] load id={vid} gen={gen} origen={origin} resultado=error-fase-load motivo={motivo}");
                }
                return Err(e);
            }
        }
    };
    let load_ms = t_load.elapsed().as_millis();
    if track.path.starts_with("ytmusic://") {
        eprintln!(
            "[perf] load id={vid} gen={gen} origen={origin} resolve={resolve_ms}ms loadfile={load_ms}ms total={}ms",
            t_all.elapsed().as_millis()
        );
    }
    // Precarga en paralelo una ventana de 5 adelante + 1 atrás (máx 6).
    // Prioridad por cercanía a la pista actual; hasta 2 yt-dlp simultáneos.
    {
        if let Ok(q) = state.queue.lock() {
            if let Some(cur) = q.current {
                let all: Vec<String> = q.items.iter().map(|t| t.path.clone()).collect();
                ytmusic::warm_next(&all, ytmusic::current_gen(), Some(cur));
            }
        }
    }
    // 2) Atribución visible + MPRIS alimentados por el router (motor agnóstico).
    if let Some(att) = att {
        integrations::set_track(
            app,
            &att.title,
            &att.artist,
            &att.album,
            (att.duration_ms / 1000) as i64,
        );
    }
    // 3) Sincronizar MPRIS/SMTC SIN lock ajeno retenido.
    {
        let paused = state
            .router
            .lock()
            .map(|r| r.paused_state())
            .unwrap_or(true);
        integrations::sync_playback_state(app, paused);
    }
    playback_router::emit_track_and_state(app, state);
    Ok(())
}

/// Reproducción PEDIDA por el usuario (clic, flechas, MPRIS, quitar la actual):
/// siempre se ejecuta, reinicia el cortafuegos y limpia la caché negativa de
/// esa pista (un clic explícito reintenta incluso un video marcado muerto).
/// El auto-avance usa `play_current` directo (respeta la caché negativa).
pub(crate) fn play_current_user(
    app: &AppHandle,
    state: &State<AppState>,
    origin: playback_router::Origin,
) -> Result<(), String> {
    if let Ok(q) = state.queue.lock() {
        if let Some(t) = q.current_track() {
            if t.path.starts_with("ytmusic://") {
                crate::ytmusic::neg_clear(&t.path);
            }
        }
    }
    play_current(app, state, origin, playback_router::GenMode::New)
}

/// Versión async de `play_current_user` (red en el pool de bloqueo).
pub(crate) async fn play_current_user_blocking(app: AppHandle, origin: playback_router::Origin) -> Result<(), String> {
    tokio::task::spawn_blocking(move || match app.try_state::<AppState>() {
        Some(state) => play_current_user(&app, &state, origin),
        None => Err("sin estado".into()),
    })
    .await
    .map_err(|e| format!("tarea de reproducción: {e}"))?
}

/// Siguiente tema (también llamado desde MPRIS/SMTC).
pub(crate) fn cmd_next_impl(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        // Con Spotify activo, Spirc gestiona la cola del servidor, pero el
        // proxy de restricciones filtra primero: si se deniega, la cola local
        // NO avanza y la UI recibe `spotify://restriction` con el motivo.
        let handled = state
            .router
            .lock()
            .map(|mut r| {
                if r.engine() == EngineKind::Spotify && r.spotify_ready() {
                    match r.check_next() {
                        Ok(()) => {
                            r.next();
                        }
                        Err(d) => {
                            crate::spotify_policy::emit_restriction(app, r.spotify_tier(), &d);
                        }
                    }
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if handled {
            return;
        }
        // G5: UN solo paso por pulsación (antes eran dos: el avance previo de
        // "has_next" y el de advance_skipping_dead) + debounce de ráfaga (W2).
        step_impl(app, &state, 1, "siguiente");
    }
}

/// G5/W2: silencio (ms) exigido antes de resolver una pista SIN URL vigente.
/// Durante una ráfaga el índice de cola AVANZA (la UI resalta el destino en
/// <100 ms), pero no se lanza yt-dlp: solo se resuelve donde el usuario se
/// detiene.
const STEP_QUIET_MS: u64 = 250;
/// Ráfaga con URLs YA cacheadas: antes sonaba un `loadfile` por pulsación
/// (30 saltos = 30 cargas de mpv y 30 eventos). Una ventana corta agrupa la
/// ráfaga y conserva la sensación de salto inmediato (90 ms).
const STEP_CACHED_MS: u64 = 90;
/// Época de pulsaciones N/P: cada pulsación la incrementa; el trabajo
/// diferido se cancela si al despertar hay una época más nueva (la última
/// manda). Así una ráfaga de 30 pulsaciones produce UNA sola resolución.
static STEP_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// G5 (puro, testeable): avanza `step` en la cola saltando las pistas marcadas
/// en la caché negativa. No reproduce ni usa red: devuelve la pista destino y
/// deja el cursor ahí. Err si todo lo alcanzado está muerto o no hay nada.
fn advance_queue_skipping_dead(q: &mut TrackQueue, step: i32) -> Result<String, String> {
    let mut skipped = 0;
    loop {
        let next_path = match q.advance(step) {
            Some(t) => t.path.clone(),
            None => return Err("no hay más pistas".into()),
        };
        // Muerta conocida: salto instantáneo, SIN yt-dlp, SIN cupo y SIN
        // cortafuegos (la caché negativa se consulta ANTES de resolver).
        match crate::ytmusic::neg_get(&next_path) {
            Some(motivo) => {
                eprintln!("[musica] salta pista muerta (neg): {motivo}");
                skipped += 1;
                if skipped >= 6 {
                    return Err("demasiadas pistas muertas seguidas".into());
                }
            }
            None => return Ok(next_path),
        }
    }
}

/// Igual que la anterior pero con la cola bloqueada (la usan los pasos de
/// usuario y el auto-avance cuando el fallo marca la pista como muerta).
fn advance_to_target(state: &State<AppState>, step: i32) -> Result<String, String> {
    let mut q = state.queue.lock().map_err(|_| "queue lock".to_string())?;
    advance_queue_skipping_dead(&mut q, step)
}

/// G5: reproduce la pista ACTUAL de la cola. Un fallo PERMANENTE se marca en
/// caché negativa y salta a la siguiente pista reproducible (máx. 6 saltos).
fn play_current_skipping_dead(
    app: &AppHandle,
    state: &State<AppState>,
    origin: playback_router::Origin,
) -> Result<(), String> {
    let mut skipped = 0;
    loop {
        match crate::play_current_user(app, state, origin) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if !crate::ytmusic::looks_like_gone(&e) {
                    return Err(e);
                }
                let current = state
                    .queue
                    .lock()
                    .map_err(|_| "queue lock".to_string())?
                    .current_track()
                    .map(|t| t.path)
                    .unwrap_or_default();
                crate::ytmusic::neg_store(current.strip_prefix("ytmusic://").unwrap_or(&current), &e);
                skipped += 1;
                if skipped >= 6 {
                    return Err("demasiadas pistas muertas seguidas".into());
                }
                advance_to_target(state, 1)?;
            }
        }
    }
}

/// G5: paso de usuario (siguiente/anterior) con debounce de ráfaga (W2).
/// El índice avanza SIEMPRE (evento de cola en <100 ms: la UI resalta el
/// destino). La carga real espera una ventana corta aunque la URL ya esté
/// cacheada: así una ráfaga de 30 pulsaciones no lanza 30 `loadfile`/eventos.
fn step_impl(app: &AppHandle, state: &State<AppState>, step: i32, origin: playback_router::Origin) {
    let target = match advance_to_target(state, step) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[musica] {origin}: {e}");
            return;
        }
    };
    // Índice visible YA (evento de cola): sin esperar a la resolución.
    emit_queue(app, state);
    // Cancela cualquier paso diferido anterior: la última pulsación manda.
    let my = STEP_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let cached = crate::ytmusic::cached_paths(std::slice::from_ref(&target))
        .first()
        .copied()
        .unwrap_or(false);
    let wait_ms = if cached { STEP_CACHED_MS } else { STEP_QUIET_MS };
    std::thread::sleep(std::time::Duration::from_millis(wait_ms));
    if STEP_EPOCH.load(std::sync::atomic::Ordering::SeqCst) != my {
        return; // hubo otra pulsación: ella carga el destino final
    }
    if let Err(e) = play_current_skipping_dead(app, state, origin) {
        eprintln!("[musica] {origin}: {e}");
    }
}

/// Anterior tema (también llamado desde MPRIS/SMTC).
pub(crate) fn cmd_prev_impl(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        let handled = state
            .router
            .lock()
            .map(|mut r| {
                if r.engine() == EngineKind::Spotify && r.spotify_ready() {
                    // `prev` no consume presupuesto, pero exige handle válido.
                    if r.check_prev().is_ok() {
                        r.prev();
                    } else {
                        crate::spotify_policy::emit_restriction(
                            app,
                            r.spotify_tier(),
                            &crate::spotify_policy::PolicyDenial::disconnected(),
                        );
                    }
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if handled {
            return;
        }
        // Salta pistas muertas conocidas (caché negativa) antes de reproducir.
        step_impl(app, &state, -1, "anterior");
    }
}

#[tauri::command]
async fn player_next(app: AppHandle) {
    // El avance puede resolver red: fuera del runtime async.
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || cmd_next_impl(&app2))
        .await
        .ok();
}

#[tauri::command]
async fn player_prev(app: AppHandle) {
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || cmd_prev_impl(&app2))
        .await
        .ok();
}

/// RSS del proceso (kB) para el muestreo [res]. Linux: /proc/self/status;
/// Windows: GetProcessMemoryInfo (WorkingSetSize) vía windows-sys. Si algo
/// falla se devuelve "?" (es solo diagnóstico, nunca rompe el hilo).
fn rss_kb() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        unsafe {
            let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            let ok = GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut c,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            );
            if ok != 0 {
                format!("{}kB", c.WorkingSetSize / 1024)
            } else {
                "?".into()
            }
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmRSS:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|n| n.parse::<u64>().ok())
            })
            .map(|kb| format!("{kb}kB"))
            .unwrap_or_else(|| "?".into())
    }
}

/// Windows portable: si junto al .exe va `WebView2Runtime\`, úsalo sin
/// instalar WebView2 en el sistema. Debe ejecutarse ANTES de crear la ventana
/// (el loader de WebView2 lee la variable al inicializarse).
#[cfg(windows)]
fn configure_portable_paths() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };

    // WebView2 fijo incluido junto al .exe: no requiere instalación ni red.
    if std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_none() {
        let runtime = dir.join("WebView2Runtime");
        if runtime.join("msedgewebview2.exe").is_file() {
            std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", runtime);
        }
    }

    // Modo portable real (como un juego): datos junto al ejecutable.
    if dir.join("musica.portable").is_file() {
        let data = dir.join("data");
        std::fs::create_dir_all(&data).ok();
        std::env::set_var("MUSICA_DATA_DIR", &data);
        if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none() {
            let web = data.join("webview2");
            std::fs::create_dir_all(&web).ok();
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", web);
        }
        // Windows 10 exige que el runtime fijo sea legible por los SIDs de
        // AppContainer (WebView2 >= 120). Se hace en local y sin UAC.
        let runtime = dir.join("WebView2Runtime");
        if runtime.join("msedgewebview2.exe").is_file() {
            for sid in ["*S-1-15-2-2:(OI)(CI)(RX)", "*S-1-15-2-1:(OI)(CI)(RX)"] {
                let _ = tools::command("icacls")
                    .arg(&runtime)
                    .arg("/grant")
                    .arg(sid)
                    .output();
            }
        }
    }
}

#[cfg(not(windows))]
fn configure_portable_paths() {}

fn main() {
    // Decisión: mpv_create devuelve NULL si LC_NUMERIC no es "C" y tumba el arranque.
    std::env::set_var("LC_NUMERIC", "C");
    configure_portable_paths();
    // P8: evita el fallo de GBM/DMABUF de WebKitGTK en algunos equipos Linux.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .on_window_event(|win, ev| {
            // Al cerrar: guarda cola/volumen y mata yt-dlp para no dejar
            // procesos colgados.
            if matches!(ev, tauri::WindowEvent::CloseRequested { .. }) {
                let app = win.app_handle();
                save_session_now(app);
                crate::ytmusic::kill_all_children();
                // Descargas en curso: mismos motivos (sin procesos colgados).
                crate::download::kill_all();
            }
        })
        .setup(|app| {
            // MUSICA_DATA_DIR permite el modo portable (Windows junto al .exe).
            let data_dir = std::env::var_os("MUSICA_DATA_DIR")
                .map(PathBuf::from)
                .or_else(|| app.path().app_data_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            let db_path = data_dir.join("library.db");
            let conn = db::open(&db_path).expect("no se pudo crear library.db");
            // Restaura la sesión anterior: cola/cursor y volumen. La pista
            // actual queda preparada (sin auto-reproducir) y suena al pulsar ▶.
            let saved_queue = db::state_get(&conn, "queue")
                .and_then(|raw| serde_json::from_str::<TrackQueue>(&raw).ok());
            let saved_volume = db::state_get(&conn, "volume")
                .and_then(|raw| raw.parse::<u8>().ok())
                .unwrap_or(80);
            let playlists_root = playlists::root(&data_dir);
            std::fs::create_dir_all(&playlists_root).ok();
            // Keyring del sistema antes de tocar credenciales (degrada a memoria).
            keyring::init();
            // Decisión: crear Player aquí porque necesita vivir dentro del State de Tauri.
            let player = player::Player::new().expect("no se pudo iniciar libmpv (instala mpv/libmpv)");
            let spotify = librespot_engine::shared();
            let mut router = PlaybackRouter::new(player, spotify.clone(), saved_volume);
            // Player::new arranca mpv a 80; aplica el volumen restaurado.
            if let Err(e) = router.set_volume(saved_volume) {
                eprintln!("[musica] no se pudo restaurar el volumen: {e}");
            }
            app.manage(AppState {
                db_path: db_path.clone(),
                playlists_root: playlists::root(&data_dir),
                router: Mutex::new(router),
                spotify: spotify.clone(),
                queue: Mutex::new(saved_queue.unwrap_or_default()),
                providers: Mutex::new(Registry::load(&db_path)),
            });
            // Controles del SO (MPRIS en Linux, SMTC en Windows). No bloquean si fallan.
            let shared: integrations::SharedControls = std::sync::Arc::new(Mutex::new(None));
            app.manage(shared.clone());
            if let Err(e) = integrations::init(&app.handle(), shared) {
                eprintln!("[musica] integración SO desactivada: {e}");
            }
            // Muestreo de recursos cada 5s (punto C): RSS propia + hijos yt-dlp
            // vivos. Una línea [res] por muestra: si RSS/hijos crecen con las
            // transiciones, la fuga es backend; si no, es UI.
            std::thread::Builder::new()
                .name("musica-res".into())
                .spawn(|| loop {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    let rss = rss_kb();
                    eprintln!("[res] rss={rss} hijos_ytdlp={}", crate::ytmusic::live_children());
                })
                .ok();
            // Latido del hilo principal cada 1 s: si un comando síncrono lo
            // retiene >200 ms, se registra con su duración (antes la ventana
            // mostraba "no responde" sin decir quién la bloqueaba).
            let hb = app.handle().clone();
            std::thread::Builder::new()
                .name("musica-hb".into())
                .spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let t0 = std::time::Instant::now();
                    let (tx, rx) = std::sync::mpsc::channel();
                    if hb.run_on_main_thread(move || { tx.send(()).ok(); }).is_err() {
                        continue;
                    }
                    match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                        Err(_) => eprintln!("[watchdog] hilo principal bloqueado >200ms"),
                        Ok(_) => {
                            let dt = t0.elapsed().as_millis();
                            if dt > 200 {
                                eprintln!("[watchdog] latido principal lento: {dt}ms");
                            }
                        }
                    }
                })
                .ok();
            // L2: UNA verificación de sesión por arranque, en segundo plano
            // (no bloquea la ventana); la UI la pinta al recibir el evento.
            if crate::ytmusic::has_session() {
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let check = crate::ytmusic::verify_session().await;
                    h.emit("youtube://session", check).ok();
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan_folder,
            library_search,
            library_stats,
            source_list,
            source_add,
            source_remove,
            source_rename,
            remote_search,
            queue_set,
            queue_add,
            queue_append,
            queue_remove,
            queue_move,
            queue_clear,
            queue_list,
            playlist_list,
            playlist_create,
            playlist_rename,
            playlist_delete,
            playlist_tracks,
            playlist_add,
            playlist_remove,
            playlist_replace_track,
            playlist_add_folder,
            csv_import::csv_preview,
            csv_match::csv_match,
            csv_match::csv_variants,
            csv_match::csv_save_playlist,
            ipc_bridge::player_play_index,
            player_next,
            player_prev,
            ipc_bridge::player_toggle,
            ipc_bridge::player_play,
            ipc_bridge::player_pause,
            ipc_bridge::player_seek,
            ipc_bridge::player_volume,
            ipc_bridge::player_status,
            ipc_bridge::player_retry_current,
            ipc_bridge::player_track_ended,
            ipc_bridge::playback_status,
            ipc_bridge::playback_set_engine,
            ipc_bridge::playback_shuffle,
            ipc_bridge::playback_repeat,
            ipc_bridge::spotify_connect,
            ipc_bridge::spotify_disconnect,
            ipc_bridge::spotify_logout,
            ipc_bridge::spotify_account,
            ipc_bridge::spotify_load,
            ipc_bridge::spotify_search,
            ipc_bridge::spotify_policy_status,
            ipc_bridge::youtube_probe,
            ipc_bridge::youtube_import,
            ipc_bridge::youtube_account,
            ipc_bridge::youtube_verify_session,
            ipc_bridge::youtube_connect,
            ipc_bridge::youtube_google_login,
            ipc_bridge::youtube_clear_login_data,
            ipc_bridge::youtube_disconnect,
            ipc_bridge::youtube_my_playlists,
            ipc_bridge::youtube_set_channel,
            ipc_bridge::youtube_browsers,
            ipc_bridge::youtube_import_cookies,
            ipc_bridge::youtube_cached,
            ipc_bridge::youtube_dead,
            ipc_bridge::youtube_throttle_secs,
            ipc_bridge::youtube_warm,
            ipc_bridge::youtube_batch_open,
            ipc_bridge::youtube_batch_progress,
            ipc_bridge::youtube_batch_more,
            ipc_bridge::youtube_batch_cancel,
            ipc_bridge::youtube_like_status,
            ipc_bridge::youtube_set_like,
            ipc_bridge::youtube_set_playlist_like,
            ipc_bridge::youtube_recommendations,
            download::download_start,
            download::download_start_playlist,
            download::download_cancel,
            download::download_progress,
            download::download_env,
            ipc_bridge::report_error
        ])
        .run(tauri::generate_context!())
        .expect("error al arrancar Tauri");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Track;

    fn t(id: &str) -> Track {
        Track {
            id: 0,
            path: format!("ytmusic://{id}"),
            title: id.to_string(),
            artist: "a".into(),
            album: "b".into(),
            genre: "youtube".into(),
            year: 0,
            track_no: 0,
            duration_secs: 0,
            mtime: 0,
            art_url: String::new(),
        }
    }

    /// G5: una pulsación avanza UNA pista (antes saltaba dos) y las muertas
    /// conocidas se saltan sin red: la caché negativa se consulta antes.
    #[test]
    fn paso_avanza_una_y_salta_muertas() {
        let dead = "g5muerta1";
        crate::ytmusic::neg_store(dead, "yt-dlp: Video unavailable");
        let mut q = TrackQueue::default();
        q.set_list(vec![t("a"), t(dead), t("b"), t("c")], 0);
        // Una pulsación = un paso: "a" → "b" (la muerta se salta sola).
        assert_eq!(advance_queue_skipping_dead(&mut q, 1).unwrap(), "ytmusic://b");
        assert_eq!(q.current, Some(2));
        // "anterior" también es un solo paso.
        assert_eq!(advance_queue_skipping_dead(&mut q, -1).unwrap(), "ytmusic://a");
        assert_eq!(q.current, Some(0));
        crate::ytmusic::neg_clear(&format!("ytmusic://{dead}"));
    }

    /// G5: si todo lo alcanzado está muerto, se corta (máx. 6 saltos) en vez
    /// de quedarse saltando en bucle.
    #[test]
    fn todas_muertas_no_entra_en_bucle() {
        // Ids PROPIOS: sin solape con "g5muerta1" de otro test (aislamiento:
        // las dos pruebas comparten la caché negativa global de la sesión).
        let ids: Vec<String> = (0..7).map(|i| format!("g5colam{i}")).collect();
        for id in &ids {
            crate::ytmusic::neg_store(id, "yt-dlp: Video unavailable");
        }
        let mut q = TrackQueue::default();
        q.set_list(ids.iter().map(|i| t(i)).collect(), 0);
        assert!(advance_queue_skipping_dead(&mut q, 1).is_err());
        for id in &ids {
            crate::ytmusic::neg_clear(&format!("ytmusic://{id}"));
        }
    }
}
