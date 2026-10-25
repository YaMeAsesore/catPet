//! Motor de audio sobre libmpv (nativo, sin web Audio).
//! Decisión: libmpv porque resuelve gapless/crossfade/EQ/ReplayGain sin GStreamer.
//!
//! Estado confirmado por eventos (BUG grave: la UI marcaba "reproduciendo" de
//! forma optimista y un loadfile atascado/silencioso quedaba sonando en falso).
//! Un hilo dedicado drena `wait_event` de un segundo handle del mismo core
//! (`create_client`: su propia cola, sin pelear comandos): observa pause,
//! time-pos, duration, eof-reached y core-idle ANTES del primer loadfile y
//! registra StartFile/FileLoaded/PlaybackRestart/EndFile+motivo. `status()`
//! expone `confirmed` (PlaybackRestart visto) y `error` (motivo real); si no
//! hay confirmación en ~3s, reintenta el load UNA vez y si no, publica el error.

use libmpv2::events::Event;
use libmpv2::Mpv;
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

// Razones mpv_end_file_reason (client.h, ABI estable): EOF=0 STOP=2 QUIT=3 ERROR=4 REDIRECT=5.
const END_FILE_ERROR: u32 = 4;
const END_FILE_STOP: u32 = 2;
const END_FILE_REDIRECT: u32 = 5;

/// Verbose temporal de diagnóstico (eventos/propiedades mpv). Solo con
/// `MUSICA_DEBUG=1`; los errores siempre se publican en `status().error`.
fn debug_on() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("MUSICA_DEBUG").as_deref() == Ok("1"))
}

macro_rules! dlog {
    ($($a:tt)*) => {
        if debug_on() {
            eprintln!("[mpv] {}", format!($($a)*));
        }
    };
}

struct PendingLoad {
    at: Instant,
    /// Para la línea de tiempo T1b (sin URLs ni IPs: solo id estable).
    gen: u64,
    id: String,
    origin: &'static str,
    file_loaded_at: Option<Instant>,
}

/// Datos para la línea de tiempo de un load (T1b).
pub struct LoadInfo {
    pub gen: u64,
    pub id: String,
    pub origin: &'static str,
}

/// Id corto para logs: video_id de YouTube o nombre de archivo local.
pub fn short_id(path: &str) -> String {
    if let Some(id) = path.strip_prefix("ytmusic://") {
        return id.to_string();
    }
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}

struct PlayerInner {
    /// loadfile emitido sin PlaybackRestart todavía.
    loading: bool,
    /// PlaybackRestart visto para la carga actual: mpv SUENA de verdad.
    confirmed: bool,
    pending: Option<PendingLoad>,
    /// Último error real (EndFile ERROR o reintento agotado). Se limpia al cargar.
    error: Option<String>,
    /// El core murió (evento Shutdown): los comandos deben recrear primero.
    dead: bool,
    /// Último demuxer-cache-time visto (para detectar progreso real).
    last_cache_time: f64,
    /// Cuándo se vio progreso por última vez (extiende el plazo).
    last_progress_at: Instant,
    /// G3: seek pedido mientras mpv no tenía archivo (transición): se guarda
    /// aquí y se reintenta en el siguiente tick (`status()`), sin error.
    pending_seek: Option<f64>,
}

pub struct Player {
    mpv: Mutex<Mpv>,
    inner: Arc<Mutex<PlayerInner>>,
    events: Mutex<EventLoop>,
}

/// Hilo de eventos vivo (para pararlo al recrear el motor).
struct EventLoop {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Serialize, Clone, Debug)]
pub struct PlayerStatus {
    pub paused: bool,
    pub pos_secs: f64,
    pub dur_secs: f64,
    pub volume: i64,
    pub path: Option<String>,
    /// mpv `eof-reached`: se queda latcheado hasta el próximo loadfile.
    /// Lo usa el auto-avance de la UI (BUG: antes nada detectaba el fin).
    pub eof: bool,
    /// mpv confirmó reproducción (PlaybackRestart) para la carga actual.
    /// La UI muestra "cargando" hasta que esto es true.
    pub confirmed: bool,
    /// Motivo real del último fallo (EndFile ERROR o watchdog agotado).
    pub error: Option<String>,
}

impl Player {
    pub fn new() -> Result<Self, String> {
        let now = Instant::now();
        let inner = Arc::new(Mutex::new(PlayerInner {
            loading: false,
            confirmed: false,
            pending: None,
            error: None,
            dead: false,
            last_cache_time: 0.0,
            last_progress_at: now,
            pending_seek: None,
        }));
        let (mpv, events) = Self::spawn_core(80, false, &inner)?;
        Ok(Self { mpv: Mutex::new(mpv), inner, events: Mutex::new(events) })
    }

    /// Crea un core mpv SOLO-AUDIO con las mismas opciones siempre, más su
    /// hilo de eventos. BUG 1: sin ventana ni video (antes mpv abría ventana
    /// con la portada/URL). Todo se fija pre-init, ANTES del primer loadfile;
    /// cada fallo de opción se registra (nunca en silencio). El estado es el
    /// Arc compartido del Player (sobrevive recreaciones); el EventLoop nuevo
    /// se guarda en el struct.
    fn spawn_core(
        volume: i64,
        mute: bool,
        inner: &Arc<Mutex<PlayerInner>>,
    ) -> Result<(Mpv, EventLoop), String> {
        // Motor SOLO-AUDIO (BUG 1): vid=no, sin ventana, sin portada
        // incrustada, sin subtítulos, sin entrada/atajos ni OSD de ventana.
        // `stream-lavf-o` y red van aquí también (pre-init = aplicado seguro).
        let mpv = Mpv::with_initializer(|init| {
            for (k, v) in [
                ("ytdl", "no"), // URLs ya resueltas: cero doble resolución
                ("vid", "no"),
                ("force-window", "no"),
                ("audio-display", "no"),
                ("sub", "no"),
                ("input-default-bindings", "no"),
                ("input-vo-keyboard", "no"),
                ("osc", "no"),
                ("input-terminal", "no"),
                ("network-timeout", "15"),
                ("stream-lavf-o", "reconnect=1,reconnect_streamed=1,reconnect_delay_max=4"),
                ("cache", "yes"),
                ("demuxer-readahead-secs", "20"),
                // googlevideo rechaza el UA por defecto "libmpv": pide UA de
                // navegador + referer de YouTube Music (mismo que usa yt-dlp).
                ("user-agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"),
                ("referrer", "https://music.youtube.com/"),
                ("gapless-audio", "yes"),
                ("audio-client-name", "catPet"),
                ("prefetch-playlist", "yes"),
                ("replaygain", "track"),
            ] {
                if let Err(e) = init.set_option(k, v) {
                    eprintln!("[musica] opción mpv {k} no aplicada: {e}");
                }
            }
            Ok(())
        })
        .map_err(|e| format!("mpv init: {e}"))?;

        // Hilo de eventos con SU PROPIO handle (create_client): su propia cola,
        // sin pelear comandos. Drena StartFile/FileLoaded/PlaybackRestart/
        // EndFile+motivo y cambios de propiedad.
        // NOTA: libmpv2 6.0 no exporta el enum Format, así que observe_property
        // no se puede llamar desde fuera; no importa: los eventos de ciclo de
        // vida llegan igual y las props se leen en vivo en status().
        let events = mpv
            .create_client(Some("musica_events"))
            .map_err(|e| format!("mpv eventos: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let inner = inner.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("mpv-events".into())
                .spawn(move || {
                    dlog!("hilo de eventos iniciado");
                    loop {
                        match events.wait_event(0.5) {
                            Some(Ok(ev)) => handle_event(&inner, ev),
                            Some(Err(e)) => dlog!("wait_event error: {e}"),
                            None => {} // timeout: revisa parada
                        }
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    dlog!("hilo de eventos terminado");
                })
                .map_err(|e| format!("hilo mpv: {e}"))?
        };

        // Registro inicial de propiedades (petición del diagnóstico).
        if let Ok(m) = mpv.get_property::<String>("audio-device") {
            eprintln!("[musica] audio-device inicial: {m}");
        }
        // Volumen y mute iniciales aplicados desde el arranque (el motor no
        // debe heredar un 0/mute de una sesión anterior del servidor de audio).
        mpv.set_property("volume", volume).ok();
        mpv.set_property("mute", mute).ok();
        Ok((mpv, EventLoop { stop, thread: Some(thread) }))
    }

    /// ¿Murió el core? (evento Shutdown visto).
    pub fn is_dead(&self) -> bool {
        self.inner.lock().map(|g| g.dead).unwrap_or(false)
    }

    /// RECREA el motor con las mismas opciones/volumen/mute. Una sola instancia
    /// en toda la app: para el hilo viejo (join fuera de locks), suelta sus
    /// handles y levanta core + hilo nuevos sobre el MISMO estado compartido
    /// (reseteado a sano).
    pub fn recreate(&self) -> Result<(), String> {
        // 1) Conserva volumen/mute (si el handle aún responde).
        let (volume, mute) = match self.mpv.lock() {
            Ok(m) => (
                m.get_property::<i64>("volume").unwrap_or(80),
                m.get_property::<bool>("mute").unwrap_or(false),
            ),
            Err(_) => (80, false),
        };
        // 2) Para el hilo viejo: clona su flag ANTES del take, señaliza y join
        // sin locks tomados (el hilo solo toca `inner` brevemente).
        let old_thread = match self.events.lock() {
            Ok(mut g) => {
                let stop = g.stop.clone();
                let t = g.thread.take();
                stop.store(true, Ordering::Relaxed);
                t
            }
            Err(_) => None,
        };
        if let Some(t) = old_thread {
            t.join().ok();
        }
        // 3) Resetea el estado y levanta core + hilo nuevos.
        if let Ok(mut g) = self.inner.lock() {
            g.loading = false;
            g.confirmed = false;
            g.pending = None;
            g.error = None;
            g.dead = false;
            g.last_cache_time = 0.0;
            g.last_progress_at = Instant::now();
            g.pending_seek = None; // el core nuevo arranca en 0: sin seek heredado
        }
        let (mpv, events) = Self::spawn_core(volume, mute, &self.inner)?;
        if let Ok(mut g) = self.mpv.lock() {
            *g = mpv;
        } else {
            return Err("player lock".into());
        }
        if let Ok(mut g) = self.events.lock() {
            *g = events;
        } else {
            return Err("player lock".into());
        }
        eprintln!("[musica] motor mpv recreado (vol={volume} mute={mute})");
        Ok(())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Para el hilo de eventos (best-effort; sale en ≤0.5s por su timeout).
        if let Ok(g) = self.events.lock() {
            g.stop.store(true, Ordering::Relaxed);
        }
    }
}

/// Clasifica EndFileReason (u32 crudo): solo ERROR genera mensaje.
fn end_file_error(reason: u32) -> Option<String> {
    if reason == END_FILE_ERROR {
        Some("mpv no pudo reproducir el archivo (formato, códec o dispositivo de audio)".into())
    } else {
        None
    }
}

/// G3: `MPV_ERROR_PROPERTY_UNAVAILABLE` (-10) no es un fallo: significa que la
/// propiedad de reproducción (time-pos, duration, eof-reached, path…) no
/// existe AHORA porque mpv está entre archivos (transición). Verificado con
/// un test diagnóstico: `loadfile` y `pause` responden ok en la transición y
/// las lecturas de `status()` ya se tragaban el -10 con `unwrap_or`; solo el
/// seek lo propagaba al usuario como error.
const MPV_PROPERTY_UNAVAILABLE: i32 = -10;

fn es_transitorio(e: &libmpv2::Error) -> bool {
    matches!(e, libmpv2::Error::Raw(c) if *c == MPV_PROPERTY_UNAVAILABLE)
}

/// Enmascara una URL para logs: solo esquema+host, sin query (firmas) ni IP.
pub fn mask_url(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or("");
    // Conserva esquema://host + inicio del path (suficiente para depurar).
    if let Some(rest) = without_query.split_once("://") {
        let (host, path) = (rest.0, rest.1);
        let short: String = path.chars().take(24).collect();
        return format!("{}://{}/…", host, short);
    }
    "[url]".into()
}

fn handle_event(inner: &Arc<Mutex<PlayerInner>>, ev: Event<'_>) {
    let mut st = match inner.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    match ev {
        Event::StartFile => {
            dlog!("StartFile");
            st.loading = true;
        }
        Event::FileLoaded => {
            dlog!("FileLoaded");
            st.loading = true;
            if let Some(p) = st.pending.as_mut() {
                p.file_loaded_at = Some(Instant::now());
            }
        }
        Event::PlaybackRestart => {
            if let Some(p) = st.pending.as_ref() {
                let audio_ms = p.at.elapsed().as_millis();
                let loaded = p
                    .file_loaded_at
                    .map(|f| format!("{}ms", f.duration_since(p.at).as_millis()))
                    .unwrap_or_else(|| "-".into());
                // T1b: cuándo empieza a SONAR de verdad (loadfile=0ms no lo mide).
                eprintln!(
                    "[latencia] id={} gen={} origen={} file_loaded={loaded} mpv_hasta_audio={audio_ms}ms",
                    p.id, p.gen, p.origin
                );
                dlog!("PlaybackRestart → confirmado en {audio_ms}ms");
            } else {
                dlog!("PlaybackRestart → confirmado");
            }
            st.loading = false;
            st.confirmed = true;
            st.pending = None;
            st.error = None;
        }
        Event::EndFile(reason) => {
            let code = reason as u32;
            dlog!("EndFile reason={code}");
            // Cargas abandonadas (loadfile replace por salto rápido): el EndFile
            // STOP/REDIRECT de la carga vieja no debe pisar la nueva en curso.
            if (code == END_FILE_STOP || code == END_FILE_REDIRECT) && st.pending.is_some() {
                dlog!("EndFile abandonado ignorado reason={code}");
                return;
            }
            st.loading = false;
            st.confirmed = false;
            if let Some(msg) = end_file_error(code) {
                st.error = Some(msg);
                st.pending = None;
            }
        }
        Event::PropertyChange { name, .. } => {
            dlog!("propiedad: {name}");
        }
        Event::Shutdown => {
            dlog!("Shutdown");
            st.dead = true;
            st.loading = false;
            st.pending = None;
        }
        _ => {}
    }
}

impl Player {
    pub fn play_file(&self, path: &str, load: &LoadInfo) -> Result<(), String> {
        // Core muerto (ventana cerrada por el usuario): recrear ANTES del load.
        if self.is_dead() {
            self.recreate()?;
        }
        dlog!("loadfile <- {}", mask_url(path));
        let res = (|| -> Result<(), String> {
            let mpv = self.mpv.lock().map_err(|_| "player lock".to_string())?;
            mpv.command("loadfile", &[path, "replace"])
                .map_err(|e| format!("loadfile: {e}"))?;
            // pause=false SIEMPRE en cada load iniciado por el usuario.
            mpv.set_property("pause", false).map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(e) = res {
            // Handle muerto a mitad de comando: recrear y reintentar UNA vez.
            // Cualquier otro error cuenta como fallo de la pista (no silencio).
            if self.is_dead() {
                eprintln!("[musica] loadfile con motor muerto, recreando: {e}");
                self.recreate()?;
                let mpv = self.mpv.lock().map_err(|_| "player lock".to_string())?;
                mpv.command("loadfile", &[path, "replace"])
                    .map_err(|e2| format!("loadfile tras recrear: {e2}"))?;
                mpv.set_property("pause", false).map_err(|e2| e2.to_string())?;
            } else {
                return Err(e);
            }
        }
        let mut st = self.inner.lock().map_err(|_| "player lock".to_string())?;
        let now = Instant::now();
        st.loading = true;
        st.confirmed = false;
        st.error = None;
        st.pending = Some(PendingLoad {
            at: now,
            gen: load.gen,
            id: load.id.clone(),
            origin: load.origin,
            file_loaded_at: None,
        });
        st.last_cache_time = 0.0;
        st.last_progress_at = now;
        Ok(())
    }

    /// Carga en curso sin confirmación (el router lo usa para no latchear EOF
    /// durante la ventana de carga/reintento).
    pub(crate) fn is_loading(&self) -> bool {
        self.inner.lock().map(|g| g.pending.is_some()).unwrap_or(false)
    }

    pub fn set_paused(&self, paused: bool) -> Result<(), String> {
        if self.is_dead() {
            self.recreate()?;
        }
        let mpv = self.mpv.lock().map_err(|_| "player lock".to_string())?;
        match mpv.set_property("pause", paused) {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                drop(mpv);
                if self.is_dead() {
                    self.recreate()?;
                    self.mpv
                        .lock()
                        .map_err(|_| "player lock".to_string())?
                        .set_property("pause", paused)
                        .map_err(|e2| e2.to_string())?;
                    Ok(())
                } else {
                    Err(msg)
                }
            }
        }
    }

    pub fn toggle(&self) -> Result<bool, String> {
        let paused: bool = self
            .mpv
            .lock()
            .map_err(|_| "player lock".to_string())?
            .get_property("pause")
            .map_err(|e| e.to_string())?;
        self.set_paused(!paused)?;
        Ok(!paused)
    }

    pub fn stop(&self) -> Result<(), String> {
        if self.is_dead() {
            // Parada sobre motor muerto: recrear para dejar estado sano.
            self.recreate()?;
            return Ok(());
        }
        let mpv = self.mpv.lock().map_err(|_| "player lock".to_string())?;
        if let Err(e) = mpv.command("stop", &[]) {
            let msg = e.to_string();
            drop(mpv);
            if self.is_dead() {
                self.recreate()?;
            } else {
                return Err(msg);
            }
        } else {
            drop(mpv);
        }
        if let Ok(mut st) = self.inner.lock() {
            st.loading = false;
            st.confirmed = false;
            st.pending = None;
        }
        Ok(())
    }

    pub fn seek_abs(&self, secs: f64) -> Result<(), String> {
        if self.is_dead() {
            return Err("motor mpv muerto (recrea antes de seek)".into());
        }
        let mpv = self.mpv.lock().map_err(|_| "player lock".to_string())?;
        match mpv.set_property("time-pos", secs) {
            Ok(()) => Ok(()),
            // G3: sin archivo en plena transición (RAW(-10)) → transitorio:
            // se recuerda y se reintenta en el siguiente tick. Nada de error
            // al usuario ni fallo que contar.
            Err(e) if es_transitorio(&e) => {
                drop(mpv);
                if let Ok(mut g) = self.inner.lock() {
                    g.pending_seek = Some(secs);
                }
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// G3: reintenta el seek pospuesto por una transición (lo llama `status()`
    /// en cada tick de 500 ms). Si vuelve a no haber archivo, se queda para el
    /// próximo tick; los errores distintos de -10 se registran y se descartan.
    fn flush_pending_seek(&self) {
        let target = match self.inner.lock() {
            Ok(mut g) => g.pending_seek.take(),
            Err(_) => return,
        };
        let Some(target) = target else { return };
        let res = match self.mpv.lock() {
            Ok(mpv) => mpv.set_property("time-pos", target).map_err(|e| e.to_string()),
            Err(_) => Err("player lock".to_string()),
        };
        match res {
            Ok(()) => dlog!("seek pendiente aplicado: {target:.2}s"),
            Err(e) if e.contains("Raw(-10)") => {
                // Sigue sin archivo: se conserva para el próximo tick.
                if let Ok(mut g) = self.inner.lock() {
                    g.pending_seek = Some(target);
                }
            }
            Err(e) => eprintln!("[musica] seek pendiente descartado ({target:.2}s): {e}"),
        }
    }

    pub fn set_volume(&self, v: i64) -> Result<(), String> {
        if self.is_dead() {
            self.recreate()?;
        }
        let mpv = self.mpv.lock().map_err(|_| "player lock".to_string())?;
        mpv.set_property("volume", v.clamp(0, 130)).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Crossfade (fase 2 completa): por ahora usa pausa entre tracks = 0.
    /// Decisión: exponer el setter ya para no romper IPC cuando se añada lavfi-acrossfade.
    pub fn set_crossfade_secs(&self, _secs: f64) -> Result<(), String> {
        Ok(()) // TODO: af-add lavfi-acrossfade=d=SECS
    }

    /// Ecualizador 10 bandas (fase 2): stub que no falla para mantener IPC estable.
    pub fn set_equalizer(&self, _gains: &[f64]) -> Result<(), String> {
        Ok(()) // TODO: af=equalizer=f=...:t=h:width-type=o:g=G
    }

    pub fn status(&self) -> PlayerStatus {
        // G3: reintenta aquí el seek pospuesto por una transición (tick 500ms).
        self.flush_pending_seek();
        // Watchdog: sin confirmación en ~3s, UN reintento; si no, error visible.
        // (Va aquí porque el tick de la UI lo llama cada 500ms.)
        self.watchdog();
        let lock = self.mpv.lock();
        let (paused, pos, dur, vol, path, eof) = match lock {
            Ok(mpv) => (
                mpv.get_property::<bool>("pause").unwrap_or(true),
                mpv.get_property::<f64>("time-pos").unwrap_or(0.0),
                mpv.get_property::<f64>("duration").unwrap_or(0.0),
                mpv.get_property::<i64>("volume").unwrap_or(80),
                mpv.get_property::<String>("path").ok(),
                mpv.get_property::<bool>("eof-reached").unwrap_or(false),
            ),
            Err(_) => (true, 0.0, 0.0, 80, None, false),
        };
        let (confirmed, error) = self
            .inner
            .lock()
            .map(|g| (g.confirmed, g.error.clone()))
            .unwrap_or((false, None));
        PlayerStatus { paused, pos_secs: pos, dur_secs: dur, volume: vol, path, eof, confirmed, error }
    }

    /// Watchdog consciente de progreso: solo falla si NO hay progreso.
    /// Si mpv está recibiendo datos (paused-for-cache, cache-speed>0 o
    /// demuxer-cache-time avanzando), la fecha límite se extiende; tope
    /// absoluto 15s. Sin progreso, 12s (3s era insuficiente para googlevideo
    /// lento). NO relanza loadfile a ciegas (eso emitía end-file de la carga
    /// anterior y cascaba saltos): el reintento con estrategia nueva lo hace
    /// `player_retry_current`.
    fn watchdog(&self) {
        let now = Instant::now();
        let (caching, speed, cache_time) = match self.mpv.lock() {
            Ok(m) => (
                m.get_property::<bool>("paused-for-cache").unwrap_or(false),
                m.get_property::<i64>("cache-speed").unwrap_or(0),
                m.get_property::<f64>("demuxer-cache-time").unwrap_or(0.0),
            ),
            Err(_) => (false, 0, 0.0),
        };
        let mut st = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(p) = st.pending.as_ref() else {
            return;
        };
        let elapsed = now.duration_since(p.at);
        if st.confirmed {
            return;
        }
        // ¿Avanza la red? Cualquiera de las tres señales vale.
        let advanced = cache_time > st.last_cache_time + f64::EPSILON;
        if advanced {
            st.last_cache_time = cache_time;
        }
        let progressing = caching || speed > 0 || advanced;
        if progressing {
            st.last_progress_at = now;
            // Progresando (recibiendo datos): extiende, con tope absoluto.
            if elapsed.as_secs() >= 15 {
                st.error = Some(
                    "La carga se quedó atascada recibiendo datos tras 15 s \
                     (red lenta o servidor sin respuesta)"
                        .into(),
                );
                st.pending = None;
                st.loading = false;
            }
            return;
        }
        if now.duration_since(st.last_progress_at).as_secs() >= 12 {
            st.error = Some(
                "mpv no confirmó la reproducción en 12 s sin recibir datos \
                 (revisa el archivo, la red y el dispositivo de audio)"
                    .into(),
            );
            st.pending = None;
            st.loading = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Smoke test con audio REAL: reproduce un tono, pausa, seek, volumen.
    /// Es lo que hacen los botones del footer; si esto pasa, el backend local
    /// responde. Requiere backend de audio (ALSA/Pulse en esta máquina).
    fn li() -> LoadInfo {
        LoadInfo { gen: 0, id: "test".into(), origin: "test" }
    }

    #[test]
    #[ignore = "audio real: ejecutar con --ignored"]
    fn reproduce_tono_local() {
        std::env::set_var("LC_NUMERIC", "C");
        let wav = "/tmp/musica-tone.mp3";
        assert!(std::path::Path::new(wav).exists(), "genera el tono primero");
        let p = Player::new().expect("libmpv debe iniciar");

        // play
        p.play_file(wav, &li()).expect("loadfile");
        std::thread::sleep(Duration::from_millis(1500));
        let st = p.status();
        eprintln!("[smoke] tras play: paused={} pos={:.2} dur={:.2} vol={} path={:?} confirmed={} err={:?}",
            st.paused, st.pos_secs, st.dur_secs, st.volume, st.path, st.confirmed, st.error);
        assert!(!st.paused, "debe estar sonando tras play_file");
        assert!(st.confirmed, "PlaybackRestart debe confirmar (eventos vivos)");
        assert!(st.error.is_none());
        assert!(st.dur_secs > 5.0, "duración real del mp3 (~10s): {}", st.dur_secs);
        assert!(st.pos_secs > 0.2, "la posición debe avanzar: {}", st.pos_secs);

        // pause / resume (botón ▶/⏸)
        assert!(p.toggle().expect("toggle a pausa"));
        assert!(p.status().paused, "toggle debe pausar");
        assert!(!p.toggle().expect("toggle a play"), "toggle debe reanudar");

        // seek (barra de progreso)
        p.seek_abs(5.0).expect("seek");
        std::thread::sleep(Duration::from_millis(600));
        let st2 = p.status();
        eprintln!("[smoke] tras seek 5s: pos={:.2}", st2.pos_secs);
        assert!((st2.pos_secs - 5.0).abs() < 1.5, "seek a ~5s: {}", st2.pos_secs);

        // volumen (deslizador)
        p.set_volume(42).expect("volumen");
        assert_eq!(p.status().volume, 42);

        // stop (limpiar cola)
        p.stop().expect("stop");
        eprintln!("[smoke] local OK: play/pause/seek/volumen/stop responden");
    }

    /// Al terminar, mpv vacía el estado (path=None, dur=0, sin pausa).
    /// Este flanco es la señal que usa el router para el auto-avance.
    #[test]
    #[ignore = "audio real: ejecutar con --ignored"]
    fn fin_de_pista_vacia_mpv() {
        std::env::set_var("LC_NUMERIC", "C");
        let wav = "/tmp/musica-short.mp3";
        assert!(std::path::Path::new(wav).exists(), "genera el tono corto primero");
        let p = Player::new().expect("libmpv debe iniciar");
        p.play_file(wav, &li()).expect("loadfile");
        let mut saw_audio = false;
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(500));
            let st = p.status();
            if st.dur_secs > 1.0 {
                saw_audio = true;
            }
            if saw_audio && st.path.is_none() && st.dur_secs == 0.0 && !st.paused {
                eprintln!("[smoke] EOF: mpv vaciado tras terminar");
                return;
            }
        }
        panic!("mpv no se vació tras el EOF en 10s");
    }

    /// Archivo corrupto: el error REAL debe publicarse en status().error
    /// (antes se tragaba en silencio y la UI quedaba en "sonando" mudo).
    #[test]
    #[ignore = "audio real: ejecutar con --ignored"]
    fn archivo_corrupto_publica_error() {
        std::env::set_var("LC_NUMERIC", "C");
        let bad = "/tmp/musica-garbage.mp3";
        assert!(std::path::Path::new(bad).exists());
        let p = Player::new().expect("libmpv debe iniciar");
        // loadfile puede fallar sync o async: ambos caminos deben dejar rastro.
        let sync_err = p.play_file(bad, &li()).is_err();
        let mut surfaced = sync_err;
        for _ in 0..16 {
            std::thread::sleep(Duration::from_millis(500));
            let st = p.status();
            if st.error.is_some() {
                eprintln!("[smoke] error publicado: {:?}", st.error);
                surfaced = true;
                break;
            }
            if st.confirmed {
                panic!("un archivo corrupto nunca debe confirmarse");
            }
        }
        assert!(surfaced, "el fallo debe ser visible (sync o vía EndFile)");
    }

    /// Eventos EndFile: solo ERROR publica mensaje; EOF/STOP/REDIRECT no.
    #[test]
    fn endfile_solo_error_publica() {
        assert!(end_file_error(4).is_some());
        for code in [0u32, 2, 3, 5] {
            assert!(end_file_error(code).is_none(), "código {code} no es error");
        }
    }

    /// G3: mientras mpv está entre archivos (transición), `time-pos` no existe
    /// y devuelve RAW(-10). Eso NO es un fallo: el seek se pospone al siguiente
    /// tick y jamás se muestra al usuario (antes salía como error en rojo).
    #[test]
    fn seek_en_transicion_no_publica_error() {
        std::env::set_var("LC_NUMERIC", "C");
        let p = Player::new().expect("libmpv debe iniciar (la enlazamos para compilar)");
        // Sin archivo cargado: time-pos → MPV_ERROR_PROPERTY_UNAVAILABLE (-10).
        assert!(
            p.seek_abs(3.0).is_ok(),
            "RAW(-10) es transitorio: no debe devolver error al usuario"
        );
        assert_eq!(
            p.inner.lock().map(|g| g.pending_seek).ok().flatten(),
            Some(3.0),
            "el seek queda pendiente para el siguiente tick"
        );
        let st = p.status(); // tick de la UI: reintenta el seek pendiente
        assert!(st.error.is_none(), "el tick nunca debe publicar RAW(-10)");
        assert_eq!(
            p.inner.lock().map(|g| g.pending_seek).ok().flatten(),
            Some(3.0),
            "sigue sin archivo: el seek se conserva (no se pierde)"
        );
        p.stop().ok();
    }

    /// 20 cambios rápidos de pista en el mismo Player (audio real).
    /// mpv debe quedar sano y sonando la ÚLTIMA, sin trabarse.
    #[test]
    #[ignore = "audio real: 20 loadfile seguidos"]
    fn cambios_rapidos_no_traban_mpv() {
        std::env::set_var("LC_NUMERIC", "C");
        let files = ["/tmp/musica-tone.mp3", "/tmp/musica-short.mp3"];
        assert!(files.iter().all(|f| std::path::Path::new(f).exists()));
        let p = Player::new().expect("libmpv");
        for i in 0..20 {
            p.play_file(files[i % 2], &li()).expect("loadfile");
            std::thread::sleep(Duration::from_millis(120));
        }
        // La última (short.mp3, 3s) debe confirmarse y sonar.
        let mut ok = false;
        for _ in 0..16 {
            std::thread::sleep(Duration::from_millis(500));
            let st = p.status();
            if st.confirmed && !st.paused {
                ok = true;
                break;
            }
        }
        let st = p.status();
        eprintln!("[cambios] final: confirmed={} paused={} pos={:.1}", st.confirmed, st.paused, st.pos_secs);
        assert!(ok, "tras 20 cambios debe sonar la última: {st:?}");
    }

    /// Medición de transición completa (red+mpv reales): resolver + confirmar
    /// por canción. Tabla de tiempos para comparar antes/después de optimizar.
    /// Cada transición debe resolverse en segundos, sin crecer con el índice.
    #[test]
    #[ignore = "red+audio: mide 10 transiciones reales"]
    fn transicion_mide_tiempos() {
        std::env::set_var("LC_NUMERIC", "C");
        let ids = [
            "rFZHOHl-L8A", "y7qZFji19Rg", "lQTtojKRYKE", "IPg9DRpm9xQ", "JD-kMIpDfnY",
            "0er2m_JIBdc", "0QKQlf8r7ls", "1B-BW28iNsc", "1-LpQekNa9g", "1uiyOL3DLWw",
        ];
        let mut rows = vec![];
        let p = Player::new().expect("libmpv");
        for (n, id) in ids.iter().enumerate() {
            let t0 = Instant::now();
            let url = match crate::ytmusic::resolve_audio_gen(&format!("ytmusic://{id}"), None, crate::ytmusic::SlotKind::User) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("[medición {n}] resolve FALLA: {e}");
                    continue;
                }
            };
            let t_resolve = t0.elapsed();
            if p.play_file(&url, &li()).is_err() {
                // URL directa (no ytmusic://): play_file la carga tal cual.
                eprintln!("[medición {n}] loadfile falla");
                continue;
            }
            let mut confirmed_ms = None;
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(500));
                let st = p.status();
                if st.confirmed {
                    confirmed_ms = Some(t0.elapsed());
                    break;
                }
            }
            match confirmed_ms {
                Some(t) => {
                    eprintln!("[medición {n}] resolve={}ms confirmado={}ms",
                        t_resolve.as_millis(), t.as_millis());
                    rows.push((t_resolve.as_millis(), t.as_millis()));
                    assert!(t.as_secs() < 25, "transición {n} tardó demasiado");
                }
                None => panic!("transición {n}: sin confirmación en 15s"),
            }
        }
        assert!(!rows.is_empty(), "ninguna transición completó");
        eprintln!("[medición] {}/{} completadas", rows.len(), ids.len());
    }

}