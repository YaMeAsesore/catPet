//! Router de reproducción: una sola cola y un solo estado, dos motores.
//!
//! - Libmpv: archivos locales/remotos (gapless, EQ, ReplayGain).
//! - Spotify (librespot): catálogo Connect vía cuenta Premium.
//!
//! El router expone una única interfaz (`play/pause/next/prev/seek/volume/status`),
//! enruta al motor activo, y emite eventos IPC unificados (`playback://*`) más la
//! atribución visible del track actual para que MPRIS/SMTC y la UI se alimenten
//! de una sola fuente independientemente del motor.
//!
//! Limitación de crossfade entre motores: los dos motores escriben en su propio
//! sink nativo, así que un crossfade real exigiría un grafo de audio compartido;
//! la política implementada es fade-out breve del motor saliente + arranque del
//! entrante (documentado en docs/arquitectura-librespot.md).

use crate::db::Track;
use crate::librespot_engine::{self, RouteEvent, SpotifyHandle, TrackAttribution};
use crate::ytmusic;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

pub use crate::librespot_engine::SharedSpotify;

/// Motores de audio disponibles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Libmpv,
    Spotify,
}

impl EngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineKind::Libmpv => "libmpv",
            EngineKind::Spotify => "librespot",
        }
    }
}

/// Decide el motor para un path de la cola (puro, testeable).
/// `spotify:track:xxx` / `spotify:episode:xxx` → Spotify; el resto → libmpv.
pub fn engine_for_track(path: &str) -> EngineKind {
    if path.starts_with("spotify:") {
        EngineKind::Spotify
    } else {
        EngineKind::Libmpv
    }
}

/// Estado unificado que el router publica a UI y a MPRIS/SMTC.
#[derive(Debug, Clone, Serialize)]
pub struct UnifiedStatus {
    pub engine: EngineKind,
    pub paused: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// 0..=100
    pub volume: u8,
    /// Track actual (atribución visible) o None si no hay nada cargado.
    pub track: Option<TrackAttribution>,
    /// true si el motor Spotify está conectado (independiente de si está activo).
    pub spotify_ready: bool,
}

/// Evento de estado para IPC (`playback://state`).
#[derive(Debug, Clone, Serialize)]
pub struct PlaybackStateEvent {
    pub engine: EngineKind,
    /// "playing" | "paused" | "stopped"
    pub state: &'static str,
    /// Generación de reproducción (la UI ignora eventos viejos).
    pub gen: u64,
    /// Instante del backend en ms (la UI mide latencia de pintado).
    pub ts_ms: u64,
}

/// Evento de track actual (`playback://track`): atribución siempre presente.
#[derive(Debug, Clone, Serialize)]
pub struct TrackChangedEvent {
    pub engine: EngineKind,
    pub track: Option<TrackAttribution>,
    /// Generación de reproducción (la UI ignora eventos viejos).
    pub gen: u64,
    /// Instante del backend en ms (la UI mide latencia de pintado).
    pub ts_ms: u64,
}

/// Evento de cola (`playback://queue`).
#[derive(Debug, Clone, Serialize)]
pub struct QueueEvent {
    pub current: Option<usize>,
    pub length: usize,
    /// Versión estructural de la cola (ver `TrackQueue::rev`). Si no cambió,
    /// la UI solo actualiza el índice actual y evita clonar toda la cola.
    pub rev: u64,
}

/// Trazo de depuración/limitación del crossfade entre motores.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transition {
    /// Mismo motor: no hay transición.
    None,
    /// Motores distintos: fade-out + parada + arranque del entrante.
    Crossfade { fade_ms: u64 },
}

/// Política de transición entre motores (pura, testeable).
pub fn transition_plan(from: EngineKind, to: EngineKind, crossfade_secs: f64) -> Transition {
    if from == to || crossfade_secs <= 0.0 {
        Transition::None
    } else {
        Transition::Crossfade {
            fade_ms: (crossfade_secs * 1000.0) as u64,
        }
    }
}

/// Origen de una intención de play (T1a/T2: cada gen registra el suyo).
pub type Origin = &'static str;

/// Modo de gen para un play: intención nueva o continuación (salto/reintento
/// de la misma intención: NO bumpea, acaba el desorden de gens).
#[derive(Clone, Copy)]
pub enum GenMode {
    New,
    Reuse(u64),
}

/// Controlador: UN solo dueño asigna gens de forma secuencial (T2). La UI,
/// el auto-avance, el reintento y la precarga piden aquí; nadie bumpea por
/// su cuenta (ni reintentos ni saltos). Los gens salen siempre en orden y
/// cada cambio tiene un origen claro.
static CTRL: std::sync::Mutex<Option<(u64, Origin)>> = std::sync::Mutex::new(None);

/// Asigna el siguiente gen (secuencial) y registra gen+origen+id.
pub fn request_gen(origin: Origin, id: &str) -> u64 {
    let g = crate::ytmusic::bump_gen();
    if let Ok(mut c) = CTRL.lock() {
        *c = Some((g, origin));
    }
    eprintln!("[gen] gen={g} origen={origin} id={id} ts={}", now_ms());
    g
}

/// Intención vigente (gen, origen), si hay.
pub fn current_intent() -> Option<(u64, Origin)> {
    CTRL.lock().ok().and_then(|c| *c)
}

/// Router con estado vivo. Vive en `AppState` tras un `Mutex`.
pub struct PlaybackRouter {
    engine: EngineKind,
    mpv: crate::player::Player,
    spotify: SharedSpotify,
    // Estado unificado (el motor Spotify lo actualiza por eventos).
    paused: bool,
    position_ms: u64,
    duration_ms: u64,
    volume: u8,
    track: Option<TrackAttribution>,
    /// Segundos de fade al cambiar de motor (0 = corte directo).
    crossfade_secs: f64,
    /// Fin de pista detectado y aún no consumido (solo motor Libmpv).
    eof_seen: bool,
    /// A qué pista pertenece el fin latcheado (R4: solo el eof del gen
    /// actual avanza; si el usuario ya cambió, se descarta).
    eof_track: Option<String>,
    /// El vacío de mpv es esperado (stop()/cambio de motor): no es un fin.
    idle_expected: bool,
    /// Reintentos de la pista actual (BUG grave: máx 1 con estrategia nueva).
    retry_uri: Option<String>,
    retry_count: u8,
    /// Fallos seguidos (cortafuegos: a los 3 se detiene el auto-avance).
    consec_fails: u8,
    /// Generación de reproducción: sube en cada pista que empieza a sonar.
    /// Los eventos llevan este gen; la UI ignora los viejos (llegan tarde).
    play_gen: u64,
    /// (uri, gen) de la última pista que empezó a sonar (el reintento
    /// reutiliza su gen en vez de crear otro).
    play_gen_for: Option<(String, u64)>,
}

/// Instante Unix en ms (para `ts_ms` de los eventos).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl PlaybackRouter {
    pub fn new(mpv: crate::player::Player, spotify: SharedSpotify, volume: u8) -> Self {
        Self {
            engine: EngineKind::Libmpv,
            mpv,
            spotify,
            paused: true,
            position_ms: 0,
            duration_ms: 0,
            volume,
            track: None,
            crossfade_secs: 1.5,
            eof_seen: false,
            eof_track: None,
            idle_expected: true, // al arrancar no hay nada cargado: el vacío es normal
            retry_uri: None,
            retry_count: 0,
            consec_fails: 0,
            play_gen: 0,
            play_gen_for: None,
        }
    }

    /// Generación vigente (para los eventos).
    pub fn play_gen(&self) -> u64 {
        self.play_gen
    }

    /// Gen con el que empezó a sonar `uri` (para reutilizar en reintentos).
    pub fn gen_for(&self, uri: &str) -> Option<u64> {
        match &self.play_gen_for {
            Some((u, g)) if u == uri => Some(*g),
            _ => None,
        }
    }

    pub fn engine(&self) -> EngineKind {
        self.engine
    }

    /// ¿Está el motor Spotify conectado (handle vivo)?
    pub fn spotify_ready(&self) -> bool {
        self.spotify
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }

    /// Cambia el motor activo. Pausa el saliente, arranca el entrante con el
    /// track actual ya precargado (o lo carga si hace falta).
    pub fn set_engine(
        &mut self,
        app: &AppHandle,
        to: EngineKind,
    ) -> Result<(), String> {
        if to == self.engine {
            return Ok(());
        }
        if to == EngineKind::Spotify && !self.spotify_ready() {
            return Err("Spotify no está conectado (requiere cuenta Premium)".into());
        }
        let plan = transition_plan(self.engine, to, self.crossfade_secs);
        match plan {
            Transition::None => {}
            Transition::Crossfade { .. } => {
                //Fade-out del motor saliente antes de soltarlo.
                self.fade_out();
            }
        }
        // 2) Pausa y parada del motor saliente.
        match self.engine {
            EngineKind::Libmpv => {
                if let Err(e) = self.mpv.set_paused(true) {
                    eprintln!("[musica] pausa saliente mpv: {e}");
                }
                if let Err(e) = self.mpv.stop() {
                    eprintln!("[musica] stop saliente mpv: {e}");
                }
            }
            EngineKind::Spotify => {
                self.with_spotify(|h| h.pause());
            }
        }
        self.engine = to;
        self.paused = true;
        self.position_ms = 0;
        // El saliente queda parado a propósito: su vacío no es un fin de pista.
        self.eof_seen = false;
        self.eof_track = None;
        self.idle_expected = true;
        self.reset_retry();
        // 3) El motor entrante arranca en silencio y hace fade-in.
        if to == EngineKind::Spotify {
            let start = self.volume;
            self.volume = 0;
            self.with_spotify(|h| h.set_volume_pct(0));
            let fade_ms = match plan {
                Transition::Crossfade { fade_ms } => fade_ms,
                Transition::None => 0,
            };
            fade_in(
                {
                    let spotify = self.spotify.clone();
                    move |pct| {
                        if let Ok(g) = spotify.lock() {
                            if let Some(h) = g.as_ref() {
                                h.set_volume_pct(pct);
                            }
                        }
                    }
                },
                start,
                fade_ms,
            );
        }
        emit_state(app, self);
        Ok(())
    }

    /// Fade-out del motor saliente.
    /// Limitación aceptada (documentada): bajar volumen en mpv es instantáneo y
    /// el sink de librespot no expone rampa; la percepción la suaviza el fade-in
    /// del motor entrante. Un crossfade real exigiría grafo de audio compartido.
    fn fade_out(&self) {
        if let Err(e) = self.mpv.set_volume(self.volume as i64) {
            eprintln!("[musica] fade-out mpv: {e}");
        }
    }

    /// Carga un track en el motor. La resolución de YouTube Music ocurre AQUÍ
    /// para que el router pueda detectar clics duplicados (mismo track+gen) y
    /// evitar resolver dos veces la misma pista.
    /// `gen` es la intención asignada por el controlador (UN solo dueño).
    /// `resolved_url`: URL ya resuelta para pistas no-YouTube (locales, radios, etc.).
    pub fn play_track(
        &mut self,
        app: &AppHandle,
        track: &Track,
        gen: u64,
        resolved_url: Option<&str>,
    ) -> Result<(), String> {
        let target = engine_for_track(&track.path);
        // Nueva carga: reinicia el presupuesto de reintentos salvo que sea el
        // mismo URI (el reintento conserva su contador).
        if self.retry_uri.as_deref() != Some(track.path.as_str()) {
            self.reset_retry();
        }
        if target != self.engine {
            // El path impone el motor: conmutamos antes de cargar.
            if target == EngineKind::Spotify && !self.spotify_ready() {
                return Err("Spotify no está conectado (requiere cuenta Premium)".into());
            }
            self.set_engine(app, target)?;
        }
        match target {
            EngineKind::Libmpv => {
                // Nuevo archivo: cualquier fin anterior queda consumido.
                self.eof_seen = false;
                self.eof_track = None;
                self.idle_expected = false;
                // Si el core murió (ventana cerrada), recrearlo y reintentar UNA vez.
                if self.mpv.is_dead() {
                    self.mpv.recreate()?;
                }
                // Resolución: YouTube se resuelve AQUÍ; otras fuentes usan resolved_url.
                let resolved = if track.path.starts_with("ytmusic://") {
                    ytmusic::resolve_audio_gen(&track.path, Some(gen), ytmusic::SlotKind::User)?
                } else {
                    resolved_url.ok_or("URL no resuelta para pista no-YouTube")?.to_string()
                };
                let load = crate::player::LoadInfo {
                    gen,
                    id: crate::player::short_id(&track.path),
                    origin: current_intent().map(|(_, o)| o).unwrap_or("?"),
                };
                if let Err(e) = self.mpv.play_file(&resolved, &load) {
                    // Un handle muerto a mitad de comando también se recrea una vez.
                    if self.mpv.is_dead() {
                        self.mpv.recreate()?;
                        self.mpv.play_file(&resolved, &load)?;
                    } else {
                        return Err(e);
                    }
                }
                self.consec_fails = 0;
                // El gen del evento SIGUE al de la intención (una sola línea).
                self.play_gen = gen;
                self.play_gen_for = Some((track.path.clone(), gen));
                self.track = Some(TrackAttribution {
                    uri: track.path.clone(),
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                    album: track.album.clone(),
                    art_url: track.art_url.clone(),
                    duration_ms: (track.duration_secs.max(0) as u32) * 1000,
                });
            }
            EngineKind::Spotify => {
                let started = {
                    let g = self.spotify.lock().map_err(|_| "spotify lock".to_string())?;
                    match g.as_ref() {
                        Some(h) => {
                            // Proxy de restricciones: el bajo demanda de un URI
                            // concreto se filtra aquí con mensaje claro en la UI.
                            if let Err(d) = h.check_play_ondemand() {
                                crate::spotify_policy::emit_restriction(app, h.tier, &d);
                                return Err(d.message.clone());
                            }
                            h.load_uri(&track.path, true, 0)?;
                            Some(h.session.clone())
                        }
                        None => None,
                    }
                };
                self.consec_fails = 0;
                self.play_gen = gen;
                self.play_gen_for = Some((track.path.clone(), gen));
                // Atribución inmediata con lo que trae la cola; si librespot-metadata
                // responde, una tarea en background la reemplaza con artista/álbum/caratula.
                self.track = Some(TrackAttribution {
                    uri: track.path.clone(),
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                    album: track.album.clone(),
                    art_url: track.art_url.clone(),
                    duration_ms: (track.duration_secs.max(0) as u32) * 1000,
                });
                if let Some(session) = started {
                    let app2 = app.clone();
                    let uri = track.path.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(att) = librespot_engine::current_attribution(&session, &uri).await {
                            if let Some(state) = app2.try_state::<crate::AppState>() {
                                if let Ok(mut router) = state.router.lock() {
                                    router.set_track_attribution(att);
                                    emit_track(&app2, &router);
                                }
                            }
                        }
                    });
                }
            }
        }
        self.paused = false;
        self.position_ms = 0;
        if let Some(t) = &self.track {
            self.duration_ms = t.duration_ms as u64;
        }
        emit_track(app, self);
        emit_state(app, self);
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), String> {
        match self.engine {
            EngineKind::Libmpv => self.mpv.set_paused(false)?,
            EngineKind::Spotify => {
                self.with_spotify(|h| h.play());
            }
        }
        self.paused = false;
        Ok(())
    }

    pub fn pause(&mut self) -> Result<(), String> {
        match self.engine {
            EngineKind::Libmpv => self.mpv.set_paused(true)?,
            EngineKind::Spotify => {
                self.with_spotify(|h| h.pause());
            }
        }
        self.paused = true;
        Ok(())
    }

    /// Devuelve el nuevo estado (true = reproduciendo).
    /// Para libmpv lee el estado VIVO de mpv (el flag del router puede
    /// desincronizarse si mpv termina el archivo solo); para Spotify usa el
    /// flag del router alimentado por eventos.
    pub fn toggle(&mut self) -> Result<bool, String> {
        match self.engine {
            EngineKind::Libmpv => {
                let paused = self.mpv.status().paused;
                if paused {
                    self.play()?;
                    Ok(true)
                } else {
                    self.pause()?;
                    Ok(false)
                }
            }
            EngineKind::Spotify => {
                if self.paused {
                    self.play()?;
                    Ok(true)
                } else {
                    self.pause()?;
                    Ok(false)
                }
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), String> {
        match self.engine {
            EngineKind::Libmpv => self.mpv.stop()?,
            EngineKind::Spotify => {
                self.with_spotify(|h| h.pause());
            }
        }
        self.paused = true;
        // Parada explícita: el vacío resultante no es un fin de pista.
        self.eof_seen = false;
        self.eof_track = None;
        self.idle_expected = true;
        self.reset_retry();
        self.consec_fails = 0;
        Ok(())
    }

    /// ¿Se permite otro reintento de este URI? (máx 1; URI distinto reinicia).
    pub fn retry_allowed(&mut self, uri: &str) -> bool {
        if self.retry_uri.as_deref() != Some(uri) {
            self.retry_uri = Some(uri.to_string());
            self.retry_count = 0;
        }
        if self.retry_count < 1 {
            self.retry_count += 1;
            true
        } else {
            false
        }
    }

    fn reset_retry(&mut self) {
        self.retry_uri = None;
        self.retry_count = 0;
    }

    /// Cortafuegos de fallos seguidos: a los 3 se detiene el auto-avance.
    pub fn consec_fails(&self) -> u8 {
        self.consec_fails
    }
    /// Una pista sí empezó a sonar: el contador vuelve a 0.
    pub fn note_success(&mut self) {
        self.consec_fails = 0;
    }
    /// Una pista falló en el auto-avance: suma 1 y devuelve el total.
    pub fn note_failure(&mut self) -> u8 {
        self.consec_fails = self.consec_fails.saturating_add(1);
        self.consec_fails
    }
    /// ¿Hay una carga mpv en curso sin confirmar?
    pub fn is_loading(&self) -> bool {
        self.engine == EngineKind::Libmpv && self.mpv.is_loading()
    }
    /// Si el core murió (Shutdown/ventana cerrada), recrearlo con las mismas
    /// opciones/volumen/mute. Devuelve true si recreó.
    pub fn ensure_mpv_alive(&mut self) -> Result<bool, String> {
        if self.mpv.is_dead() {
            self.mpv.recreate()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// ¿Permite la política saltar al siguiente? (consume presupuesto en
    /// cuentas restringidas; en Premium siempre pasa).
    pub fn check_next(&self) -> Result<(), crate::spotify_policy::PolicyDenial> {
        match self.with_spotify(|h| h.check_skip()) {
            Some(r) => r,
            None => Err(crate::spotify_policy::PolicyDenial::disconnected()),
        }
    }

    /// ¿Hay handle de Spotify? (`prev` no consume presupuesto).
    pub fn check_prev(&self) -> Result<(), crate::spotify_policy::PolicyDenial> {
        match self.with_spotify(|_| ()) {
            Some(_) => Ok(()),
            None => Err(crate::spotify_policy::PolicyDenial::disconnected()),
        }
    }

    /// Tier del handle (Restricted si no hay handle: lo más conservador).
    pub fn spotify_tier(&self) -> crate::spotify_policy::AccountTier {
        self.with_spotify(|h| h.tier)
            .unwrap_or(crate::spotify_policy::AccountTier::Restricted)
    }

    /// next/prev: en Spotify los maneja Spirc (la cola vive en el servidor).
    /// `prev` no consume presupuesto (normalmente reinicia el tema actual).
    pub fn next(&mut self) -> bool {
        match self.engine {
            EngineKind::Libmpv => false, // main.rs mueve el cursor y llama play_track
            EngineKind::Spotify => {
                self.with_spotify(|h| h.next());
                true
            }
        }
    }

    pub fn prev(&mut self) -> bool {
        match self.engine {
            EngineKind::Libmpv => false,
            EngineKind::Spotify => {
                self.with_spotify(|h| h.prev());
                true
            }
        }
    }

    pub fn seek_secs(&mut self, secs: f64) -> Result<(), String> {
        let ms = (secs.max(0.0) * 1000.0) as u64;
        match self.engine {
            EngineKind::Libmpv => {
                self.mpv.seek_abs(secs)?;
                self.position_ms = ms;
            }
            EngineKind::Spotify => {
                self.with_spotify(|h| h.seek_ms(ms as u32));
                self.position_ms = ms;
            }
        }
        Ok(())
    }

    pub fn set_volume(&mut self, pct: u8) -> Result<(), String> {
        let pct = pct.min(100);
        self.volume = pct;
        match self.engine {
            EngineKind::Libmpv => self.mpv.set_volume(pct as i64)?,
            EngineKind::Spotify => {
                self.with_spotify(|h| h.set_volume_pct(pct));
            }
        }
        Ok(())
    }

    pub fn volume(&self) -> u8 {
        self.volume
    }

    /// Estado de pausa actual del router (eventos unificados).
    pub fn paused_state(&self) -> bool {
        self.paused
    }

    /// Posición conocida en ms (eventos en Spotify, mpv en vivo).
    pub fn position_state(&self) -> u64 {
        self.position_ms
    }

    /// Duración conocida en ms.
    pub fn duration_state(&self) -> u64 {
        self.duration_ms
    }

    /// Estado nativo de mpv (para compatibilidad de IPC).
    pub fn mpv_status(&self) -> crate::player::PlayerStatus {
        self.mpv.status()
    }

    pub fn shuffle(&self, on: bool) {
        self.with_spotify(|h| h.shuffle(on));
    }

    pub fn repeat_ctx(&self, on: bool) {
        self.with_spotify(|h| h.repeat_ctx(on));
    }

    pub fn repeat_track(&self, on: bool) {
        self.with_spotify(|h| h.repeat_track(on));
    }

    pub fn set_crossfade_secs(&mut self, secs: f64) {
        self.crossfade_secs = secs.clamp(0.0, 12.0);
    }

    /// Acceso directo al handle de Spotify (para comandos deattribution/estado).
    pub fn with_spotify<T>(&self, f: impl FnOnce(&SpotifyHandle) -> T) -> Option<T> {
        let g = self.spotify.lock().ok()?;
        g.as_ref().map(f)
    }

    /// Estado unificado para IPC/UI/MPRIS.
    pub fn status(&mut self) -> UnifiedStatus {
        match self.engine {
            EngineKind::Libmpv => {
                let st = self.mpv.status();
                self.paused = st.paused;
                self.position_ms = (st.pos_secs.max(0.0) * 1000.0) as u64;
                self.duration_ms = (st.dur_secs.max(0.0) * 1000.0) as u64;
                self.volume = st.volume.clamp(0, 100) as u8;
            }
            EngineKind::Spotify => {
                // posición/duración vienen de los eventos del PlayerEventChannel;
                // paused/volume se mantienen en el router.
            }
        }
        UnifiedStatus {
            engine: self.engine,
            paused: self.paused,
            position_ms: self.position_ms,
            duration_ms: self.duration_ms,
            volume: self.volume,
            track: self.track.clone(),
            spotify_ready: self.spotify_ready(),
        }
    }

    /// Aplica un evento del motor Spotify al estado unificado.
    /// La llama el loop de eventos de librespot (vía `forward_spotify_event`).
    pub fn on_spotify_event(&mut self, ev: &RouteEvent) {
        if self.engine != EngineKind::Spotify {
            // Aun así trackeamos posición por si el motor se activa después.
        }
        match ev {
            RouteEvent::Playing { position_ms, .. } => {
                self.paused = false;
                self.position_ms = *position_ms as u64;
                // Sin bumpear play_gen: el evento sigue a la intención que lo
                // originó (el gen lo fijó play_track).
                self.consec_fails = 0;
            }
            RouteEvent::Paused { position_ms, .. } => {
                self.paused = true;
                self.position_ms = *position_ms as u64;
            }
            RouteEvent::Stopped => {
                self.paused = true;
                self.position_ms = 0;
            }
            RouteEvent::Position { position_ms, .. } => {
                self.position_ms = *position_ms as u64;
            }
            RouteEvent::Seeked { position_ms, .. } => {
                self.position_ms = *position_ms as u64;
            }
            RouteEvent::Volume { percent } => {
                self.volume = *percent;
            }
            RouteEvent::TrackChanged { .. }
            | RouteEvent::Loading { .. }
            | RouteEvent::Unavailable { .. } => {}
            RouteEvent::EndOfTrack { .. } => {}
            RouteEvent::SessionDisconnected => {}
        }
    }

    /// Marca la atribución del track de Spotify (descargada por el loop de eventos).
    pub fn set_track_attribution(&mut self, att: TrackAttribution) {
        self.duration_ms = att.duration_ms as u64;
        self.track = Some(att);
    }

    /// Olvida el track actual (al vaciar la cola: nada que seguir mostrando).
    pub fn clear_track(&mut self) {
        self.track = None;
        self.position_ms = 0;
        self.duration_ms = 0;
        self.paused = true;
        self.eof_seen = false;
        self.eof_track = None;
        self.reset_retry();
        self.consec_fails = 0;
    }

    /// Evalúa si mpv se vació solo (fin de pista) y latchea `eof_seen`.
    /// Causa raíz del BUG de auto-avance: tras el EOF mpv queda idle con
    /// path=None y `eof-reached` NO se latchea; la única señal fiable es el
    /// flanco "había track cargado → vacío inesperado".
    pub fn refresh_eof(&mut self) {
        if self.engine != EngineKind::Libmpv {
            return;
        }
        // Durante la carga (incluido el reintento del watchdog) el vacío es
        // normal: no latchear o el auto-avance saltaría el tema al cargar.
        if self.mpv.is_loading() {
            return;
        }
        let empty = self.mpv.status().path.is_none();
        if empty && self.track.is_some() && !self.idle_expected {
            self.eof_seen = true;
            // Etiqueta a quién pertenece el fin (R4): si el usuario ya cambió
            // de pista, este latch es viejo y se descarta al consumir.
            self.eof_track = self.track.as_ref().map(|t| t.uri.clone());
        }
        if !empty {
            self.idle_expected = false;
        }
    }

    /// Fin pendiente (para el tick de la UI).
    pub fn eof_seen(&self) -> bool {
        self.eof_seen
    }

    /// Consume el fin solo si pertenece a la pista actual del router (R4).
    /// Si el usuario ya cambió de pista, el latch es viejo: se descarta y el
    /// clic NO se pierde ni se salta.
    pub fn take_eof_if_current(&mut self) -> bool {
        if !self.eof_seen {
            return false;
        }
        let mine = match (&self.eof_track, &self.track) {
            (Some(a), Some(b)) => a == &b.uri,
            _ => false,
        };
        self.eof_seen = false;
        self.eof_track = None;
        if mine {
            true
        } else {
            eprintln!("[musica] eof viejo descartado (el usuario ya cambió de pista)");
            false
        }
    }

    pub fn current_track(&self) -> Option<TrackAttribution> {
        self.track.clone()
    }
}

/// Emite track + estado juntos (helper para play_current).
pub fn emit_track_and_state(app: &AppHandle, state: &crate::AppState) {
    if let Ok(router) = state.router.lock() {
        emit_track(app, &router);
        emit_state(app, &router);
    }
}

// ---- Emisión de eventos IPC unificados ----

/// Emite estado (`playback://state`) y mantiene el evento legacy `player://state`
/// que el frontend actual ya escucha.
pub fn emit_state(app: &AppHandle, router: &PlaybackRouter) {
    let state: &'static str = if router.paused { "paused" } else { "playing" };
    app.emit(
        "playback://state",
        PlaybackStateEvent { engine: router.engine, state, gen: router.play_gen, ts_ms: now_ms() },
    )
    .ok();
    app.emit("player://state", if router.paused { "paused" } else { "playing" })
        .ok();
}

/// Emite el track actual (atribución visible siempre, también al cambiar motor).
pub fn emit_track(app: &AppHandle, router: &PlaybackRouter) {
    app.emit(
        "playback://track",
        TrackChangedEvent { engine: router.engine, track: router.track.clone(), gen: router.play_gen, ts_ms: now_ms() },
    )
    .ok();
}

pub fn emit_queue(app: &AppHandle, current: Option<usize>, length: usize, rev: u64) {
    app.emit("playback://queue", QueueEvent { current, length, rev }).ok();
    app.emit("queue://changed", ()).ok();
}

/// Emite posición (`playback://position`).
pub fn emit_position(app: &AppHandle, router: &PlaybackRouter) {
    #[derive(Serialize, Clone)]
    struct P {
        position_ms: u64,
        duration_ms: u64,
    }
    app.emit(
        "playback://position",
        P {
            position_ms: router.position_ms,
            duration_ms: router.duration_ms,
        },
    )
    .ok();
}

// ---- Loop de eventos de Spotify ----

/// Punto de entrada del loop de eventos de librespot: actualiza router, descarga
/// atribución cuando cambia el track, emite IPC y sincroniza MPRIS/SMTC.
pub async fn forward_spotify_event(
    app: &AppHandle,
    session: &librespot::core::Session,
    shared: SharedSpotify,
    ev: RouteEvent,
) {
    // 1) Estado del router (posición/pausa/volumen).
    if let Some(state) = app.try_state::<crate::AppState>() {
        if let Ok(mut router) = state.router.lock() {
            router.on_spotify_event(&ev);
            emit_position(app, &router);
            match &ev {
                RouteEvent::Playing { .. } | RouteEvent::Paused { .. } | RouteEvent::Stopped => {
                    emit_state(app, &router);
                    crate::integrations::sync_playback_state(app, router.paused);
                }
                _ => {}
            }
        }
    }

    // 2) Atribución visible al cambiar de track (artista/álbum/carátula).
    if matches!(ev, RouteEvent::TrackChanged { .. } | RouteEvent::Playing { .. }) {
        if let Some(uri) = ev.uri() {
            if let Some(att) = librespot_engine::current_attribution(session, uri).await {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    if let Ok(mut router) = state.router.lock() {
                        router.set_track_attribution(att.clone());
                        emit_track(app, &router);
                    }
                }
                // El handle vivo conserva la última atribución.
                if let Ok(mut g) = shared.lock() {
                    if let Some(h) = g.as_mut() {
                        h.current = Some(att);
                    }
                }
            }
        }
    }
}

// ---- Fades de volumen (transición entre motores) ----

/// Sube el volumen del motor entrante de 0 a `target` en `fade_ms`.
/// Decisión: tarea async con sleep → no bloquea el hilo de UI ni el comando.
pub fn fade_in(set: impl Fn(u8) + Send + 'static, target: u8, fade_ms: u64) {
    if fade_ms == 0 {
        set(target);
        return;
    }
    tauri::async_runtime::spawn(async move {
        let steps = 10u8;
        for i in 1..=steps {
            tokio::time::sleep(std::time::Duration::from_millis(fade_ms / steps as u64)).await;
            let pct = (target as u32 * i as u32 / steps as u32) as u8;
            set(pct);
        }
        set(target);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::librespot_engine::ui_to_u16;

    #[test]
    fn enrutado_por_path() {
        assert_eq!(engine_for_track("spotify:track:4cOdK2wGLETKBW3PvgPWqT"), EngineKind::Spotify);
        assert_eq!(engine_for_track("spotify:episode:512ojhOuo1ktJprKbVcKyQ"), EngineKind::Spotify);
        assert_eq!(engine_for_track("/home/yo/musica/a.mp3"), EngineKind::Libmpv);
        assert_eq!(engine_for_track("https://sub.example/stream.mp3"), EngineKind::Libmpv);
        assert_eq!(engine_for_track(""), EngineKind::Libmpv);
    }

    #[test]
    fn plan_de_transicion() {
        assert_eq!(transition_plan(EngineKind::Libmpv, EngineKind::Libmpv, 2.0), Transition::None);
        assert_eq!(transition_plan(EngineKind::Libmpv, EngineKind::Spotify, 0.0), Transition::None);
        assert_eq!(
            transition_plan(EngineKind::Libmpv, EngineKind::Spotify, 1.5),
            Transition::Crossfade { fade_ms: 1500 }
        );
        assert_eq!(
            transition_plan(EngineKind::Spotify, EngineKind::Libmpv, 0.5),
            Transition::Crossfade { fade_ms: 500 }
        );
    }

    #[test]
    fn estado_unificado_serializa_motor() {
        let ev = PlaybackStateEvent { engine: EngineKind::Spotify, state: "playing", gen: 7, ts_ms: 0 };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"engine\":\"spotify\""));
        assert!(json.contains("\"state\":\"playing\""));
    }

    #[test]
    fn eventos_de_spotify_actualizan_estado() {
        // Router sin mpv real: verificamos on_spotify_event sobre la parte de estado.
        // (La construcción completa exige libmpv; aquí probamos el mapeo puro.)
        let ev = RouteEvent::Playing { uri: "spotify:track:x".into(), position_ms: 3000 };
        let mut paused = true;
        let mut position = 0u64;
        match ev {
            RouteEvent::Playing { position_ms, .. } => {
                paused = false;
                position = position_ms as u64;
            }
            _ => {}
        }
        assert!(!paused && position == 3000);
        let vol = RouteEvent::Volume { percent: 42 };
        assert!(matches!(vol, RouteEvent::Volume { percent: 42 }));
        // coherencia del volumen con el motor
        assert_eq!(ui_to_u16(42), 27510);
    }

    #[test]
    fn cola_event_lleva_cursor_longitud_y_rev() {
        let ev = QueueEvent { current: Some(2), length: 7, rev: 3 };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"current\":2"));
        assert!(json.contains("\"length\":7"));
        assert!(json.contains("\"rev\":3"));
    }

    /// Cortafuegos: el contador arranca en 0, suma fallos y vuelve a 0 al
    /// sonar una pista; a los 3 el auto-avance debe detenerse (el llamante
    /// comprueba `>= 3`).
    #[test]
    fn cortafuegos_tres_fallos_seguidos() {
        std::env::set_var("LC_NUMERIC", "C");
        let mpv = crate::player::Player::new().expect("libmpv debe iniciar");
        let mut r = PlaybackRouter::new(mpv, crate::librespot_engine::shared(), 80);
        assert_eq!(r.consec_fails(), 0);
        assert_eq!(r.note_failure(), 1);
        assert_eq!(r.note_failure(), 2);
        r.note_success();
        assert_eq!(r.consec_fails(), 0, "sonar una pista reinicia a 0");
        assert_eq!(r.note_failure(), 1);
        assert_eq!(r.note_failure(), 2);
        assert_eq!(r.note_failure(), 3, "a los 3 se detiene el auto-avance");
    }

    /// Los botones (toggle/seek/volumen) y el tick de progreso (status) golpean
    /// el router desde hilos distintos. Si algún comando retuviera el lock del
    /// router mientras otro lo necesita (el deadlock que congeló la UI tras el
    /// primer play), este test falla por timeout en vez de colgar el CI.
    /// Nota: el deadlock original necesitaba un AppHandle real (emits de Tauri),
    /// así que esto cubre la clase de síntoma, no aquel camino exacto.
    #[test]
    fn botones_concurrentes_no_bloquean_router() {
        use std::sync::{Arc, Mutex, mpsc};
        use std::time::Duration;
        std::env::set_var("LC_NUMERIC", "C");
        let mpv = crate::player::Player::new().expect("libmpv debe iniciar");
        let router = Arc::new(Mutex::new(PlaybackRouter::new(
            mpv,
            crate::librespot_engine::shared(),
            80,
        )));
        let (tx, rx) = mpsc::channel();
        // Hilo "botones": lo que hace la UI al pulsar ▶/⏸, seek y volumen.
        let r1 = router.clone();
        let tx1 = tx.clone();
        let h1 = std::thread::spawn(move || {
            for i in 0..50 {
                if let Ok(mut r) = r1.lock() {
                    r.toggle().ok();
                    r.seek_secs(i as f64).ok();
                    r.set_volume((i % 101) as u8).ok();
                    let _ = r.status();
                }
            }
            tx1.send(()).ok();
        });
        // Hilo "tick 500ms": lo que hace la barra de progreso.
        let r2 = router.clone();
        let tx2 = tx.clone();
        let h2 = std::thread::spawn(move || {
            for _ in 0..50 {
                if let Ok(mut r) = r2.lock() {
                    let _ = r.status();
                }
            }
            tx2.send(()).ok();
        });
        drop(tx);
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(20))
                .expect("el router se bloqueó: posible deadlock entre comandos");
        }
        h1.join().ok();
        h2.join().ok();
    }
}
