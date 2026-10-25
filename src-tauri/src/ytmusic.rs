//! YouTube Music vía yt-dlp (binario externo). Solo streaming, jamás descargas.
//!
//! Decisión: subproceso yt-dlp en vez de otra dependencia porque ya resuelve
//! flat-playlist, audio-URL perezosa y cookies; el proyecto no usaba yt-dlp ni
//! ytdl en ningún lado (se verificó con grep) y libmpv no está configurado
//! para ello. Cero crates nuevos: std::process + tokio (ya presente).
//!
//! - Listar: `yt-dlp --flat-playlist --print ...` (sin descargar nada).
//! - Reproducir: `yt-dlp -f bestaudio --get-url` SOLO al momento de sonar
//!   (carga perezosa) + caché en memoria con TTL (las URLs caducan en horas).
//! - Sesión (Parte 2): `--cookies-from-browser` exporta a un cookies.txt propio
//!   (0600, en datos de la app, fuera del repo); jamás se pide ni guarda la
//!   contraseña de Google.

use crate::db::Track;
use crate::tools;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::webview::Cookie;

/// Servicio keyring para metadatos (navegador elegido, canal). Los secretos
/// (cookies) van en archivo 0600, no en keyring.
pub const SERVICE_YT: &str = "musica-youtube";
const COOKIES_FILE: &str = "ytmusic-cookies.txt";
/// Testigo de "sesión no validada": un Conectar fallido lo deja junto al jar
/// (sin borrar nada) y el jar no cuenta como sesión hasta validar de nuevo.
const INVALID_MARK_FILE: &str = "ytmusic-cookies.invalid";
/// L3b: candidato del login integrado; solo sustituye al jar si valida.
const LOGIN_CANDIDATE_FILE: &str = "ytmusic-cookies.login.tmp";
/// L3b: datos aislados de la ventana de login (su propia sesión de Google).
const LOGIN_DATA_DIR_NAME: &str = "ytmusic-login-webview";
/// L4: método con el que se conectó la sesión (navegador/perfil, cookies.txt o
/// ventana integrada). Fichero 0600 junto al jar: no depende del llavero.
const METHOD_FILE: &str = "ytmusic-method.txt";
/// URL de "Me gustan": prueba barata de que la sesión de YouTube funciona.
const LIKED_URL: &str = "https://www.youtube.com/playlist?list=LL";
/// Las URLs de audio de Google caducan en ~6h; 5h deja margen.
const RESOLVE_TTL: Duration = Duration::from_secs(90 * 60);
const CMD_TIMEOUT: Duration = Duration::from_secs(25);
/// Resolver debe ser rápido: 7 s por intento (antes 15). Si se agota, el hijo
/// se mata (kill) y se pasa a la siguiente estrategia. 15 s colgaba el
/// comando y, sumado a una espera interna, daba transiciones de ~16 s.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(7);
/// Una espera de cupo/dedup mayor que esto se registra como lenta.
const SLOW_MS: u128 = 5_000;

fn app_dir() -> PathBuf {
    // Directorio de datos de la app (ver main.rs: app_data_dir). En tests se
    // puede sobreescribir con MUSICA_DATA_DIR.
    if let Ok(d) = std::env::var("MUSICA_DATA_DIR") {
        return PathBuf::from(d);
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(h) = std::env::var("HOME") {
            return PathBuf::from(h).join(".local/share/com.musica.app");
        }
    }
    // W2: Windows = %APPDATA%\<identifier> de tauri.conf.json (coincide con el
    // app_data_dir de Tauri); sin ella (o si APPDATA no está) cae al temp.
    #[cfg(target_os = "windows")]
    {
        if let Ok(d) = std::env::var("APPDATA") {
            return PathBuf::from(d).join("com.musica.app");
        }
    }
    std::env::temp_dir().join("com.musica.app")
}

fn cookies_path() -> PathBuf {
    app_dir().join(COOKIES_FILE)
}

pub(crate) fn session_cookie_path() -> Option<PathBuf> {
    (has_cookies() && !session_invalid()).then(cookies_path)
}

/// P6: jar de sesión para el cliente interno de recomendaciones. Devuelve
/// `None` si no hay jar o si la sesión está marcada como no validada; así el
/// módulo de recomendaciones no intenta usar cookies que la app no considera
/// una sesión activa. No expone el contenido ni el nombre de las cookies.
pub(crate) fn recommendations_cookie_path() -> Option<PathBuf> {
    session_cookie_path()
}

/// L3b: candidato del login integrado (junto al jar, mismo sistema de archivos
/// para poder renombrarlo de forma atómica al validarse).
fn login_candidate_path() -> PathBuf {
    app_dir().join(LOGIN_CANDIDATE_FILE)
}

/// L3b: directorio de datos aislado de la ventana de login integrado.
pub fn login_data_dir() -> PathBuf {
    app_dir().join(LOGIN_DATA_DIR_NAME)
}

/// L4: ruta del fichero del método de sesión.
fn method_path() -> PathBuf {
    app_dir().join(METHOD_FILE)
}

/// L4: método guardado en fichero (si lo hay). Fuente primaria: sin keyutils.
fn read_method() -> Option<String> {
    let s = std::fs::read_to_string(method_path()).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// L4: persiste el método (best-effort: es informativo para la UI).
fn write_method(method: &str) {
    let m = method.trim();
    if m.is_empty() {
        return;
    }
    if write_private(&method_path(), m.as_bytes()).is_ok() {
        chmod_600(&method_path());
    }
}

/// L4: olvida el método (al desconectar).
fn remove_method() {
    std::fs::remove_file(method_path()).ok();
}

fn has_cookies() -> bool {
    cookies_path().is_file()
}

/// ¿Hay jar de sesión? (el arranque lo usa para verificar en segundo plano).
pub fn has_session() -> bool {
    has_cookies()
}

/// Testigo "sesión no validada" (vacío, sin secretos): evita que un Conectar
/// fallido se siga mostrando como sesión tras reiniciar la app.
fn invalid_mark_path() -> PathBuf {
    app_dir().join(INVALID_MARK_FILE)
}

fn session_invalid() -> bool {
    invalid_mark_path().is_file()
}

fn mark_session_invalid() {
    let p = invalid_mark_path();
    if std::fs::create_dir_all(app_dir()).is_ok() && std::fs::write(&p, b"").is_ok() {
        chmod_600(&p);
    }
}

fn clear_session_invalid() {
    std::fs::remove_file(invalid_mark_path()).ok();
}

/// Detección real de yt-dlp/node/deno. Cada llamada lanza 3 procesos, así
/// que solo la usan `probe()` (cacheada) y el botón explícito de la UI.
fn detect_probe() -> Probe {
    let home = std::env::var("HOME").ok();
    let (available, version, error) = classify_probe(
        tools::command("yt-dlp").arg("--version").output(),
        home.as_deref(),
    );
    Probe {
        available,
        version,
        error,
        // Las versiones recientes necesitan runtime JS (deno o node) para los
        // desafíos de YouTube; sin él cada extracción tarda o falla.
        deno: bin_version("deno").is_some(),
        node: bin_version("node").is_some(),
    }
}

/// Caché de la detección. Antes `probe()` se llamaba en CADA resolución y
/// lanzaba `yt-dlp --version` + `deno --version` + `node --version` (tres
/// procesos Python/JS extra por pista). Con 50 pistas eran 150 procesos
/// inútiles: eso solo ya sumaba varios minutos de carga de playlist.
static PROBE_CACHE: OnceLock<Mutex<Option<(Instant, Probe)>>> = OnceLock::new();
const PROBE_TTL: Duration = Duration::from_secs(60);

fn probe_cache() -> &'static Mutex<Option<(Instant, Probe)>> {
    PROBE_CACHE.get_or_init(|| Mutex::new(None))
}

/// ¿Está yt-dlp disponible? (cacheado 60 s para el camino caliente).
/// El lock se mantiene durante la detección para que dos precargas simultáneas
/// no lancen 6 procesos `--version` a la vez.
pub fn probe() -> Probe {
    let Ok(mut g) = probe_cache().lock() else { return detect_probe() };
    if let Some((at, p)) = g.as_ref() {
        if at.elapsed() < PROBE_TTL {
            return p.clone();
        }
    }
    let p = detect_probe();
    *g = Some((Instant::now(), p.clone()));
    p
}

/// Detección sin caché (botón «Verificar» de la UI): refleja una instalación
/// hecha con la app abierta sin esperar al TTL.
pub fn probe_fresh() -> Probe {
    let Ok(mut g) = probe_cache().lock() else { return detect_probe() };
    let p = detect_probe();
    *g = Some((Instant::now(), p.clone()));
    p
}

/// Separa "no encontrado" (error=None) de "existe pero falla" (error=Some).
/// `home` solo se usa para no filtrar rutas del usuario en el mensaje.
fn classify_probe(
    salida: std::io::Result<std::process::Output>,
    home: Option<&str>,
) -> (bool, Option<String>, Option<String>) {
    use std::io::ErrorKind;
    match salida {
        Ok(o) if o.status.success() => (
            true,
            Some(String::from_utf8_lossy(&o.stdout).trim().to_string()),
            None,
        ),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = first_line(&stderr, home)
                .unwrap_or_else(|| format!("yt-dlp --version terminó con {}", o.status));
            (false, None, Some(msg))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => (false, None, None),
        Err(e) => (false, None, first_line(&e.to_string(), home)),
    }
}

/// Primera línea no vacía, con el HOME del usuario redactado.
fn first_line(text: &str, home: Option<&str>) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut s = line.to_string();
    if let Some(h) = home.filter(|h| !h.is_empty()) {
        s = s.replace(h, "<ruta>");
    }
    Some(s)
}

fn bin_version(bin: &str) -> Option<String> {
    tools::command(bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, Serialize)]
pub struct Probe {
    pub available: bool,
    pub version: Option<String>,
    /// None = no encontrado; Some = existe pero falló (primera línea).
    pub error: Option<String>,
    pub deno: bool,
    pub node: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct YtAccount {
    pub connected: bool,
    /// Hay jar pero sin validar (marcador): la UI no debe llamarlo sesión.
    pub unverified: bool,
    pub browser: Option<String>,
    /// L4: existen datos aislados de la ventana de login integrado; la UI
    /// ofrece borrarlos al desconectar (solo ese directorio, con permiso).
    pub login_data: bool,
}

pub fn account() -> YtAccount {
    // Un jar marcado como no validado no cuenta como sesión.
    let jar = has_cookies();
    let marked = session_invalid();
    // L4: el método vive en un fichero 0600 (sin keyutils). Migración única:
    // si solo está en el llavero, se copia al fichero y deja de depender de él.
    let browser = read_method().or_else(|| {
        let from_keyring = crate::keyring::get(SERVICE_YT, "browser");
        if let Some(m) = &from_keyring {
            write_method(m);
        }
        from_keyring
    });
    YtAccount {
        connected: jar && !marked,
        unverified: jar && marked,
        browser,
        login_data: login_data_dir().is_dir(),
    }
}

/// Ejecuta yt-dlp (async, con timeout). Todo trabajo de red pasa por aquí.
async fn run_ytdlp(args: &[&str], with_cookies: bool) -> Result<String, String> {
    let jar = cookies_path();
    let used = with_cookies && has_cookies();
    run_ytdlp_at(args, used.then_some(jar.as_path())).await
}

/// Igual que `run_ytdlp` pero con un jar explícito (o sin cookies): L3b valida
/// así el candidato de la ventana de login sin tocar el jar real.
async fn run_ytdlp_at(args: &[&str], cookie_path: Option<&Path>) -> Result<String, String> {
    if !probe().available {
        return Err("yt-dlp no está instalado. Instálalo con: sudo pacman -S yt-dlp".into());
    }
    let mut cmd = tools::tokio_command("yt-dlp");
    cmd.args(["--no-warnings", "--no-progress", "--socket-timeout", "15"]);
    if let Some(p) = cookie_path {
        cmd.args(["--cookies", &p.to_string_lossy()]);
    }
    cmd.args(args);
    let out = tokio::time::timeout(CMD_TIMEOUT, cmd.output())
        .await
        .map_err(|_| "yt-dlp tardó demasiado (¿sin internet?)".to_string())?
        .map_err(|e| format!("no se pudo ejecutar yt-dlp: {e}"))?;
    // yt-dlp reescribe el jar al salir (también si falla): reaplica 0600.
    if let Some(p) = cookie_path {
        chmod_600(p);
    }
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(classify_stderr(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// Resultado detallado de un intento (para dividir el tiempo por tramos).
pub struct AttemptOut {
    pub stdout: String,
    pub stderr_tail: String,
    pub exit_code: i32,
    pub spawn_ms: u128,
    pub exec_ms: u128,
}

/// Error de un intento con su diagnóstico (el `msg` es lo que ve la UI).
pub struct AttemptErr {
    pub msg: String,
    pub stderr_tail: String,
    pub exit_code: i32,
    pub exec_ms: u128,
    pub spawn_ms: u128,
}

impl AttemptErr {
    fn new(msg: String) -> Self {
        Self { msg, stderr_tail: String::new(), exit_code: -1, exec_ms: 0, spawn_ms: 0 }
    }
}

/// Últimas líneas del stderr, enmascaradas (sin query ni firmas) y cortas.
/// Solo para el log de intentos lentos; nunca lleva URLs completas ni IPs.
fn last_stderr_lines(stderr: &str) -> String {
    let tail: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).rev().take(2).collect();
    let mut parts: Vec<String> = tail
        .iter()
        .rev()
        .map(|l| {
            let no_query = l.split(['?', '#']).next().unwrap_or("");
            let t = no_query.trim();
            if t.len() > 160 { format!("{}…", &t[..160]) } else { t.to_string() }
        })
        .collect();
    parts.retain(|s| !s.is_empty());
    parts.join(" | ")
}

/// W3: separación mínima entre arranques de yt-dlp (3 en paralelo no deben
/// pisarse en el arranque de Python+red). Se duerme fuera del lock.
static LAST_SPAWN: Mutex<Option<Instant>> = Mutex::new(None);
const SPAWN_GAP: Duration = Duration::from_millis(100);

fn spawn_gap() {
    let wait = match LAST_SPAWN.lock() {
        Ok(mut g) => {
            let now = Instant::now();
            let next = g.map(|t| t + SPAWN_GAP).unwrap_or(now);
            *g = Some(next);
            (next > now).then_some(next - now)
        }
        Err(_) => None,
    };
    if let Some(w) = wait {
        std::thread::sleep(w);
    }
}

/// Núcleo con timeout parametrizable (la resolución usa uno más corto).
/// `abort`: si devuelve true, mata al hijo y sale con STALE (cambio de pista).
/// Devuelve tiempos por tramo: spawn (crear proceso) y exec (ejecución).
/// El parseo lo mide el llamante (es µs, pero se registra igual).
fn run_ytdlp_blocking_timeout(
    args: &[&str],
    with_cookies: bool,
    timeout: Duration,
    abort: Option<&dyn Fn() -> bool>,
) -> Result<AttemptOut, AttemptErr> {
    if !probe().available {
        return Err(AttemptErr::new("yt-dlp no está instalado. Instálalo con: sudo pacman -S yt-dlp".into()));
    }
    let t_spawn = Instant::now();
    let mut cmd = tools::command("yt-dlp");
    cmd.args(["--no-warnings", "--no-progress", "--socket-timeout", "15"]);
    let used_cookies = with_cookies && has_cookies();
    if used_cookies {
        cmd.args(["--cookies", &cookies_path().to_string_lossy()]);
    }
    cmd.args(args);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    // W3: los arranques de yt-dlp se espacian ~100 ms entre sí.
    spawn_gap();
    let child = cmd
        .spawn()
        .map_err(|e| AttemptErr::new(format!("no se pudo ejecutar yt-dlp: {e}")))?;
    // Registrado: visible para kill_all/kill_slot y el diagnóstico.
    let slot = register_child(child);
    let spawn_ms = t_spawn.elapsed().as_millis();
    let t_exec = Instant::now();
    let deadline = t_exec + timeout;
    // Lee la salida SIN retener el lock (el hijo puede tardar).
    let take_output = |slot: usize| -> (String, String) {
        let mut stdout = String::new();
        let mut stderr = String::new();
        use std::io::Read;
        if let Ok(mut c) = children().lock() {
            if slot < c.len() {
                if let Some(ch) = c[slot].as_mut() {
                    if let Some(mut o) = ch.stdout.take() {
                        o.read_to_string(&mut stdout).ok();
                    }
                    if let Some(mut e) = ch.stderr.take() {
                        e.read_to_string(&mut stderr).ok();
                    }
                }
            }
        }
        (stdout, stderr)
    };
    loop {
        // Observa sin retener el lock (el hijo puede tardar).
        enum Obs {
            Done(std::process::ExitStatus),
            Running,
            Gone,
        }
        let obs = match children().lock() {
            Ok(mut c) => {
                if slot >= c.len() {
                    Obs::Gone // alguien lo mató (cancelación): salir sin error
                } else {
                    match c[slot].as_mut() {
                        Some(ch) => match ch.try_wait() {
                            Ok(Some(status)) => Obs::Done(status),
                            _ => Obs::Running,
                        },
                        None => Obs::Gone,
                    }
                }
            }
            Err(_) => Obs::Gone,
        };
        match obs {
            Obs::Done(status) => {
                let (stdout, stderr) = take_output(slot);
                unregister(slot);
                // yt-dlp reescribe el jar al salir: reaplica 0600.
                if used_cookies {
                    chmod_600(&cookies_path());
                }
                let exec_ms = t_exec.elapsed().as_millis();
                let code = status.code().unwrap_or(-1);
                if std::env::var("MUSICA_DEBUG").as_deref() == Ok("1") && !stderr.trim().is_empty() {
                    eprintln!("[ytdlp] stderr: {}", stderr.trim().lines().last().unwrap_or(""));
                }
                if status.success() {
                    return Ok(AttemptOut {
                        stdout,
                        stderr_tail: last_stderr_lines(&stderr),
                        exit_code: code,
                        spawn_ms,
                        exec_ms,
                    });
                }
                // P1: si el proceso murió por señal (code == -1) Y hubo abort
                // (cambio de pista), NO es fallo real: devolvemos STALE para
                // que no cuente como fallo ni cambie LAST_WIN.
                if code == -1 && abort.is_some_and(|f| f()) {
                    return Err(AttemptErr::new(STALE.into()));
                }
                return Err(AttemptErr {
                    msg: classify_stderr(&stderr),
                    stderr_tail: last_stderr_lines(&stderr),
                    exit_code: code,
                    exec_ms,
                    spawn_ms,
                });
            }
            Obs::Running => {
                if abort.is_some_and(|f| f()) {
                    kill_slot(slot);
                    return Err(AttemptErr::new(STALE.into()));
                }
                if Instant::now() >= deadline {
                    kill_slot(slot);
                    let exec_ms = t_exec.elapsed().as_millis();
                    return Err(AttemptErr {
                        msg: "yt-dlp tardó demasiado (¿sin internet?)".to_string(),
                        stderr_tail: String::new(),
                        exit_code: 124,
                        exec_ms,
                        spawn_ms,
                    });
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Obs::Gone => {
                // Alguien mató el proceso (cancelación por cambio de pista).
                // Devolvemos STALE para que no cuente como fallo real ni cambie LAST_WIN.
                return Err(AttemptErr::new(STALE.into()));
            }
        }
    }
}

/// Traduce el stderr de yt-dlp a mensajes claros y breves (puro, testeable).
/// Marca si parece expiración de sesión (para el aviso discreto de reconectar).
pub fn classify_stderr(stderr: &str) -> String {
    let s = stderr.trim();
    let last = s.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("error desconocido");
    // Limpia el prefijo "ERROR: [extractor] ".
    let msg = last.split_once("ERROR:").map(|(_, m)| m.trim()).unwrap_or(last);
    let msg = msg.split(']').last().map(str::trim).filter(|m| !m.is_empty()).unwrap_or(msg);
    if msg.len() > 220 {
        format!("yt-dlp: {}…", &msg[..220])
    } else {
        format!("yt-dlp: {msg}")
    }
}

/// ¿Huele a sesión expirada/faltante? (puro, testeable).
pub fn looks_like_auth(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["sign in", "log in", "login required", "authentication credential",
     "cookies are no longer valid", "cookie is no longer valid",
     "private", "join this channel", "confirm your age"]
        .iter()
        .any(|k| m.contains(k))
}

/// ¿Huele a limitación de YouTube (429/bot-check)? No es sesión: no la invalida,
/// pero conviene pausar en vez de quemar 6 reintentos. (puro, testeable).
pub fn looks_like_throttle(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["http error 429", "too many requests", "confirm you're not a bot",
     "not a bot", "po token", "nsig"]
        .iter()
        .any(|k| m.contains(k))
}

/// Error amable para importar listas privadas sin sesión o con sesión muerta.
pub fn friendly_import_error(url: &str, raw: &str) -> String {
    let lower = url.to_lowercase();
    let needs_auth = lower.contains("list=lm")
        || lower.contains("list=ll")
        || lower.contains("list=wl")
        || looks_like_auth(raw);
    if needs_auth && !has_cookies() {
        return "Esta lista es privada o requiere sesión: conecta tu cuenta de YouTube primero".into();
    }
    if needs_auth && has_cookies() {
        return format!(
            "Sesión no válida o expirada para esta lista: vuelve a conectar. Detalle: {raw}"
        );
    }
    raw.to_string()
}

/// Extrae el id de lista (`list=`) de una URL de YouTube, normalizado.
/// Sirve para la caché persistente y para InnerTube (puro, testeable).
pub fn url_list_id(url: &str) -> Option<String> {
    let query = url.trim().split_once('?')?.1;
    for kv in query.split('&') {
        let Some((k, v)) = kv.split_once('=') else { continue };
        if k != "list" {
            continue;
        }
        let id = v.trim().split('#').next().unwrap_or("").trim();
        let id = id.strip_prefix("VL").unwrap_or(id);
        if id.len() >= 2
            && id.len() <= 100
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Some(id.to_string());
        }
    }
    None
}

/// Valida y clasifica URLs aceptadas (puro, testeable).
/// Devuelve ("playlist"|"video", url canónica) o el motivo del rechazo.
pub fn classify_url(url: &str) -> Result<(&'static str, String), String> {
    let u = url.trim();
    let lower = u.to_lowercase();
    let host_ok = lower.contains("music.youtube.com")
        || lower.contains("youtube.com")
        || lower.contains("youtu.be");
    if !(u.starts_with("http://") || u.starts_with("https://")) || !host_ok {
        return Err("link inválido: pega un link de music.youtube.com o youtube.com".into());
    }
    if lower.contains("list=") {
        Ok(("playlist", u.to_string()))
    } else if lower.contains("/watch") || lower.contains("youtu.be/") || lower.contains("/shorts/") {
        Ok(("video", u.to_string()))
    } else {
        Err("link inválido: se esperaba una playlist (list=...) o un video".into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YtTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration_secs: i64,
    pub thumbnail: String,
    /// Estado del «Me gusta» de YouTube Music. `None` = aún no se conoce
    /// (p. ej. listado por yt-dlp). InnerTube lo entrega al listar.
    #[serde(default)]
    pub liked: Option<bool>,
}

impl YtTrack {
    /// Track reproducible por el router (lazy: el path se resuelve al sonar).
    pub fn to_track(&self, album: &str) -> Track {
        Track {
            id: 0,
            path: format!("ytmusic://{}", self.id),
            title: if self.title.is_empty() { "Sin título".into() } else { self.title.clone() },
            artist: self.artist.clone(),
            album: album.to_string(),
            genre: "youtube".into(),
            year: 0,
            track_no: 0,
            duration_secs: self.duration_secs,
            mtime: 0,
            // i.ytimg.com siempre resuelve desde el ID: portada sin costo.
            art_url: format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", self.id),
        }
    }
}

/// Parsea una línea `--print "%(id)s\t%(title)s\t%(duration)s\t%(uploader)s"`.
/// Puro y testeable.
pub fn parse_flat_line(line: &str) -> Option<YtTrack> {
    let mut parts = line.split('\t');
    let id = parts.next()?.trim();
    if id.is_empty() || id == "NA" {
        return None;
    }
    let title = parts.next().unwrap_or("").trim().to_string();
    let duration = parts.next().unwrap_or("").trim().parse::<i64>().unwrap_or(0).max(0);
    let artist = parts.next().unwrap_or("").trim().to_string();
    Some(YtTrack {
        id: id.to_string(),
        title,
        artist,
        duration_secs: duration,
        thumbnail: String::new(),
        liked: None,
    })
}

/// Igual que `parse_flat_line` pero con el título de la lista como primer
/// campo. Así la importación necesita UNA sola ejecución de yt-dlp (antes se
/// lanzaba otra solo para pedir `playlist_title`, duplicando el arranque de
/// Python y la petición de red). Puro y testeable.
pub fn parse_flat_line_with_playlist(line: &str) -> Option<(Option<String>, YtTrack)> {
    let (pl, rest) = line.split_once('\t')?;
    let track = parse_flat_line(rest)?;
    let pl = pl.trim();
    let pl = if pl.is_empty() || pl == "NA" { None } else { Some(pl.to_string()) };
    Some((pl, track))
}

#[derive(Debug, Clone, Serialize)]
pub struct YtPlaylist {
    pub title: String,
    pub tracks: Vec<YtTrack>,
}

/// Importa una página (carga progresiva: start/end para cientos de pistas).
/// Una sola ejecución de yt-dlp devuelve el título de la lista y las pistas.
pub async fn import_page(url: &str, start: u32, end: u32) -> Result<YtPlaylist, String> {
    let (kind, _) = classify_url(url)?;
    let out = run_ytdlp(
        &[
            "--flat-playlist",
            "--playlist-start",
            &start.to_string(),
            "--playlist-end",
            &end.to_string(),
            "--print",
            "%(playlist_title)s\t%(id)s\t%(title)s\t%(duration)s\t%(uploader)s",
            url,
        ],
        true,
    )
    .await
    .map_err(|e| friendly_import_error(url, &e))?;
    let mut title: Option<String> = None;
    let mut tracks: Vec<YtTrack> = Vec::new();
    for line in out.lines() {
        let Some((pl, track)) = parse_flat_line_with_playlist(line) else { continue };
        if title.is_none() {
            title = pl;
        }
        tracks.push(track);
    }
    if tracks.is_empty() {
        return Err(friendly_import_error(url, "la lista no devolvió pistas").into());
    }
    // Un video suelto no tiene `playlist_title`: su título es el del tema.
    let title = title.unwrap_or_else(|| {
        if kind == "playlist" {
            "Playlist de YouTube".into()
        } else {
            tracks.first().map(|t| t.title.clone()).filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Video de YouTube".into())
        }
    });
    Ok(YtPlaylist { title, tracks })
}

// ---- S3: búsqueda por texto (emparejado de listas CSV) ----

/// Tiempo de la búsqueda en frío: más laxo que RESOLVE_TIMEOUT porque aquí no
/// hay caché que salvar y una búsqueda lenta aún da candidatos útiles.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);

/// S3: busca por texto (`ytsearch`) y devuelve metadatos planos. Reutiliza el
/// núcleo bloqueante y el cupo/espaciado de yt-dlp: la precarga (Warm) se
/// retira en silencio durante el backoff y nunca frena un clic.
pub fn search_candidates(query: &str, count: u32, slot: SlotKind) -> Result<Vec<YtTrack>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("búsqueda vacía".into());
    }
    let count = count.clamp(1, 10);
    // Deadline con holgura (timeout + 3 s): el hijo puede agotar su timeout
    // entero y aún hay que leer la salida y cerrar; así el guardia no suelta
    // el cupo con el proceso todavía vivo.
    let deadline = Instant::now() + SEARCH_TIMEOUT + Duration::from_secs(3);
    let nunca = || false;
    let _slot = ResolveSlotGuard::acquire(slot, &nunca, deadline)?.0;
    let n = count.to_string();
    let q = format!("ytsearch{count}:{query}");
    let args = [
        "--flat-playlist",
        "--playlist-end",
        &n,
        "--print",
        "%(id)s\t%(title)s\t%(duration)s\t%(uploader)s",
        &q,
    ];
    match run_ytdlp_blocking_timeout(&args, true, SEARCH_TIMEOUT, None) {
        Ok(out) => Ok(out.stdout.lines().filter_map(parse_flat_line).collect()),
        Err(e) => {
            if e.msg != STALE && looks_like_throttle(&e.msg) {
                note_throttle(); // 429 real: refuerza el backoff, como la resolución
            }
            Err(e.msg)
        }
    }
}

// ---- caché en memoria (las URLs caducan en horas) ----

/// Entrada de caché: URL + cuándo se guardó + vencimiento REAL declarado en
/// la propia URL de googlevideo (parámetro `expire`, si la lleva).
struct CacheEntry {
    url: String,
    added: Instant,
    expires: Option<Instant>,
}

static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Segundos restantes hasta el vencimiento declarado en la URL de googlevideo
/// (parámetro `expire=<unix>`). None si no se pudo leer (puro, testeable).
pub fn url_remaining_secs(url: &str) -> Option<i64> {
    let query = url.split('?').nth(1)?;
    for kv in query.split(['&', ' ']) {
        if let Some(v) = kv.strip_prefix("expire=") {
            let secs: i64 = v.trim().parse().ok()?;
            let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
            return Some(secs.saturating_sub(now));
        }
    }
    None
}

/// Capacidad de la caché de URLs. Antes eran 100 entradas con purga de 50:
/// una playlist de 300 pistas expulsaba ids ya resueltos y luego se
/// re-resolvían (mismo id ×9 en [yt-chain]). 600 URLs (~250 KB) cubren
/// varias playlists grandes sin expulsar nada en la práctica.
const CACHE_MAX: usize = 600;

/// Inserta en caché con tope LRU (elimina las 150 más antiguas al pasar).
/// Toda resolución exitosa pasa por aquí, sin importar gen ni obsolescencia.
fn cache_insert(id: &str, url: String) {
    let expires = url_remaining_secs(&url)
        .map(|s| Instant::now() + Duration::from_secs(s.max(0) as u64));
    if let Ok(mut c) = cache().lock() {
        if c.len() >= CACHE_MAX {
            let mut all: Vec<_> = c.iter().map(|(k, e)| (k.clone(), e.added)).collect();
            all.sort_by_key(|(_, t)| *t);
            for (k, _) in all.into_iter().take(CACHE_MAX / 4) {
                c.remove(&k);
            }
        }
        c.insert(id.to_string(), CacheEntry { url, added: Instant::now(), expires });
    }
    dedup_notify();
}

/// URL vigente en caché: TTL duro de 90 min Y, si la URL declara su propio
/// vencimiento, aún no pasados 60 s de margen (mpv necesita margen para
/// abrir el stream).
fn cache_get(id: &str) -> Option<String> {
    let c = cache().lock().ok()?;
    let e = c.get(id)?;
    if e.added.elapsed() >= RESOLVE_TTL {
        return None;
    }
    if let Some(x) = e.expires {
        if Instant::now() + Duration::from_secs(60) >= x {
            return None;
        }
    }
    Some(e.url.clone())
}

/// Vigencia restante de una entrada (para renovar ~10 min antes de vencer).
fn cache_remaining(id: &str) -> Option<Duration> {
    let c = cache().lock().ok()?;
    let e = c.get(id)?;
    let mut rem = RESOLVE_TTL.checked_sub(e.added.elapsed())?;
    if let Some(x) = e.expires {
        rem = rem.min(x.saturating_duration_since(Instant::now()));
    }
    Some(rem)
}

/// Hijos yt-dlp vivos (para matarlos al superar o al cerrar la app).
/// Slots estables: el índice no se mueve aunque otros terminen.
static CHILDREN: OnceLock<Mutex<Vec<Option<std::process::Child>>>> = OnceLock::new();

/// Caché negativa: video_id → (motivo, instante). Los errores PERMANENTES
/// (no disponible, privado, eliminado, región) no se re-resuelven en la
/// sesión (~6 h): ni precarga ni auto-avance lanzan yt-dlp para ellos.
/// Solo un clic explícito del usuario (que limpia la entrada) o "reintentar"
/// los intenta de nuevo. Los transitorios (red, timeout) nunca entran aquí.
static NEG: OnceLock<Mutex<HashMap<String, (String, Instant)>>> = OnceLock::new();

/// TTL de la caché negativa (una sesión larga).
const NEG_TTL: Duration = Duration::from_secs(6 * 60 * 60);

fn neg() -> &'static Mutex<HashMap<String, (String, Instant)>> {
    NEG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// W5: ids cuya marca "no disponible" ya se registró en el log. Una línea
/// por id y por transición; los sondeos repetidos van a nivel debug.
/// (Antes: [yt-neg] repetido decenas de veces por id.)
static NEG_LOGGED: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();

fn neg_log_once(id: &str, motivo: &str) {
    let mut set = match NEG_LOGGED.get_or_init(|| Mutex::new(std::collections::HashSet::new())).lock() {
        Ok(s) => s,
        Err(_) => return,
    };
    if set.insert(id.to_string()) {
        if set.len() > 1000 {
            set.clear(); // tope sano: las transiciones viejas ya no importan
        }
        eprintln!("[yt-neg] id={id} motivo={motivo} (sin yt-dlp)");
    } else {
        debug_log(&format!("neg hit {id}"));
    }
}

/// Motivo guardado si el id está marcado no disponible y vigente.
pub fn neg_get(path: &str) -> Option<String> {
    let id = path.strip_prefix("ytmusic://")?;
    let mut n = neg().lock().ok()?;
    match n.get(id) {
        Some((motivo, at)) if at.elapsed() < NEG_TTL => Some(motivo.clone()),
        Some(_) => {
            n.remove(id);
            None
        }
        None => None,
    }
}

/// Olvida la marca (clic explícito del usuario o "reintentar").
pub fn neg_clear(path: &str) {
    if let Some(id) = path.strip_prefix("ytmusic://") {
        if let Ok(mut n) = neg().lock() {
            n.remove(id);
        }
    }
}

/// Olvida solo las marcas de edad (cambió la sesión: puede que ya valga).
pub fn neg_clear_age() {
    if let Ok(mut n) = neg().lock() {
        n.retain(|_, (motivo, _)| motivo != AGE_MOTIVE);
    }
}

/// Marca un id como permanentemente no disponible (solo errores `gone`).
pub fn neg_store(id: &str, motivo: &str) {
    if id.is_empty() {
        return;
    }
    if let Ok(mut n) = neg().lock() {
        // Tope 500: las listas de "Me gusta" pueden traer cientos de muertas.
        if n.len() > 500 {
            if let Some(oldest) = n.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k.clone()) {
                n.remove(&oldest);
            }
        }
        let m = motivo.trim().chars().take(140).collect::<String>();
        n.insert(id.to_string(), (m, Instant::now()));
    }
    // Transición nueva: se permite UNA línea más de [yt-neg] para este id.
    if let Ok(mut set) = NEG_LOGGED.get_or_init(|| Mutex::new(std::collections::HashSet::new())).lock() {
        set.remove(id);
    }
}

/// Motivos de no disponible para una lista de paths (gris + tooltip sin
/// intentar nada). `None` = sin marca (hay que resolver normal).
pub fn neg_motives(paths: &[String]) -> Vec<Option<String>> {
    paths.iter().map(|p| neg_get(p)).collect()
}

fn children() -> &'static Mutex<Vec<Option<std::process::Child>>> {
    CHILDREN.get_or_init(|| Mutex::new(Vec::new()))
}

/// Registra un hijo y devuelve su slot. Reutiliza huecos libres.
fn register_child(child: std::process::Child) -> usize {
    let mut c = match children().lock() {
        Ok(c) => c,
        Err(_) => return usize::MAX,
    };
    // Barre terminados antes de añadir (evita acumular zombies).
    for slot in c.iter_mut() {
        if let Some(ch) = slot {
            if matches!(ch.try_wait(), Ok(Some(_))) {
                *slot = None;
            }
        }
    }
    if let Some(i) = c.iter().position(|s| s.is_none()) {
        c[i] = Some(child);
        return i;
    }
    c.push(Some(child));
    c.len() - 1
}

/// Retira un slot SIN matar (el hijo ya terminó por sí solo).
fn unregister(slot: usize) {
    if let Ok(mut c) = children().lock() {
        if slot < c.len() {
            c[slot] = None;
        }
    }
}

/// Mata un slot concreto (cancelar una resolución superada).
pub fn kill_slot(slot: usize) {
    if let Ok(mut c) = children().lock() {
        if slot < c.len() {
            if let Some(mut ch) = c[slot].take() {
                tools::kill_child(&mut ch);
            }
        }
    }
}

/// Mata TODOS los yt-dlp hijos (al cerrar la app o al cambiar de pista).
pub fn kill_all_children() {
    if let Ok(mut c) = children().lock() {
        for slot in c.iter_mut() {
            if let Some(mut ch) = slot.take() {
                tools::kill_child(&mut ch);
            }
        }
    }
}

/// Cuántos hijos siguen vivos (diagnóstico).
pub fn live_children() -> usize {
    children()
        .lock()
        .map(|mut c| {
            let mut n = 0;
            for slot in c.iter_mut() {
                match slot {
                    Some(ch) => match ch.try_wait() {
                        Ok(Some(_)) => {
                            *slot = None;
                        }
                        _ => n += 1,
                    },
                    None => {}
                }
            }
            n
        })
        .unwrap_or(0)
}

/// Generación de reproducción: UN SOLO dueño la incrementa (el controlador,
/// de forma secuencial): cada intención de play tiene un gen en orden. Las
/// respuestas con gen viejo se descartan ("la última gana", sin retardar).
/// Nadie más la bumpea (ni reintentos ni saltos: reutilizan el gen vigente).
static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Asigna el siguiente gen (solo el controlador). Devuelve el id vigente.
pub fn bump_gen() -> u64 {
    let g = GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    dedup_notify();
    g
}

/// Id vigente (la precarga lo captura y aborta si cambia).
pub fn current_gen() -> u64 {
    GEN.load(std::sync::atomic::Ordering::SeqCst)
}

/// Error interno: respuesta de una generación vieja (se ignora en silencio).
pub const STALE: &str = "superseded";

/// ¿Está esta URL resuelta y vigente en caché? (para el indicador "listo").
pub fn cached_paths(paths: &[String]) -> Vec<bool> {
    paths
        .iter()
        .map(|p| {
            p.strip_prefix("ytmusic://")
                .map(|id| cache_get(id).is_some())
                .unwrap_or(false)
        })
        .collect()
}

/// Invalida la caché de un path (el reintento debe re-resolver con otra
/// estrategia, nunca repetir la misma URL de googlevideo).
pub fn invalidate_cache(path: &str) {
    if let Some(id) = path.strip_prefix("ytmusic://") {
        if let Ok(mut c) = cache().lock() {
            c.remove(id);
        }
    }
}

/// Resoluciones en vuelo: evita dos yt-dlp simultáneos para el mismo video
/// (la transición y la precarga pueden pedirlo a la vez).
static INFLIGHT: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();

fn inflight() -> &'static Mutex<std::collections::HashSet<String>> {
    INFLIGHT.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// Cupos separados usuario/precarga (T3): 1 reservado para el usuario + 2
/// para precarga. Una ráfaga de precargas nunca bloquea un clic.
static RESOLVE_ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static RESOLVE_USER_ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// W3: ante 429 / "not a bot" se baja a UN yt-dlp (solo cupo de usuario)
/// durante unos minutos, en vez de quemar reintentos contra el límite.
static THROTTLE_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);
/// Aviso de límite ya mostrado en la interfaz en este episodio (solo una vez).
static THROTTLE_NOTIFIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const THROTTLE_BACKOFF: Duration = Duration::from_secs(5 * 60);

/// Activa el backoff de límite (se llama al detectar 429 / not-a-bot).
pub fn note_throttle() {
    let mut t = match THROTTLE_UNTIL.lock() {
        Ok(t) => t,
        Err(_) => return,
    };
    let first = !t.is_some_and(|x| Instant::now() < x);
    *t = Some(Instant::now() + THROTTLE_BACKOFF);
    if first {
        THROTTLE_NOTIFIED.store(false, std::sync::atomic::Ordering::SeqCst);
        eprintln!(
            "[yt-throttle] backoff={}s motivos=429/not-a-bot (solo cupo de usuario activo)",
            THROTTLE_BACKOFF.as_secs()
        );
    }
}

/// ¿Estamos dentro del backoff de límite? (lo consulta el cargador por lotes
/// para bajar a un solo yt-dlp).
pub fn throttle_active() -> bool {
    THROTTLE_UNTIL
        .lock()
        .ok()
        .is_some_and(|t| t.is_some_and(|x| Instant::now() < x))
}

/// Devuelve true UNA sola vez por episodio de límite (aviso en la interfaz).
pub fn take_throttle_notice() -> bool {
    throttle_active() && !THROTTLE_NOTIFIED.swap(true, std::sync::atomic::Ordering::SeqCst)
}

/// S4.2b: segundos que quedan del backoff de límite (0 si no hay episodio
/// activo). «Emparejar todas» lo sondea para pausar con cuenta atrás SIN
/// lanzar yt-dlp: sondearlo no refuerza el 429 como haría una búsqueda nueva.
pub fn throttle_remaining_secs() -> u32 {
    let t = match THROTTLE_UNTIL.lock() {
        Ok(t) => *t,
        Err(_) => None,
    };
    t.map(|x| x.saturating_duration_since(Instant::now()).as_secs() as u32)
        .unwrap_or(0)
}

/// Prioridad del solicitante (cupo propio).
#[derive(Clone, Copy, PartialEq)]
pub enum SlotKind {
    /// Clic, flechas, reintento del usuario: cupo reservado (máx 1).
    User,
    /// Precarga en segundo plano (máx 2, nunca frena un clic).
    Warm,
}

struct ResolveSlotGuard {
    user: bool,
}

impl ResolveSlotGuard {
    /// Espera un hueco en SU cupo sin bloquear para siempre; aborta si stale.
    /// El permiso SIEMPRE se libera al terminar/cancelar/fallar (RAII/Drop).
    /// Devuelve el guard y la espera de cupo en ms (tramo `espera_de_cupo_ms`).
    fn acquire(kind: SlotKind, stale: &dyn Fn() -> bool, deadline: Instant) -> Result<(Self, u128), String> {
        let t0 = Instant::now();
        let seq = std::sync::atomic::Ordering::SeqCst;
        loop {
            if stale() {
                return Err(STALE.into());
            }
            // W3: en backoff de límite (429/not-a-bot) solo sigue el cupo de
            // usuario; la precarga/lotes ceden sin consumir yt-dlp.
            if kind == SlotKind::Warm && throttle_active() {
                return Err(STALE.into());
            }
            let (counter, max) = match kind {
                SlotKind::User => (&RESOLVE_USER_ACTIVE, 1),
                // La precarga siempre tiene 2 cupos; el usuario conserva el
                // suyo. Antes la carga inicial podía usar 3, pero ya no
                // resuelve lotes completos: no hace falta invadir ese cupo.
                SlotKind::Warm => (&RESOLVE_ACTIVE, 2),
            };
            let n = counter.load(seq);
            if n < max
                && counter
                    .compare_exchange(n, n + 1, seq, seq)
                    .is_ok()
            {
                // Tope DURO de 3 yt-dlp en total (usuario + lote): la ronda
                // de cupos por separado no lo garantiza si ambos arrancan
                // a la vez. Si nos pasamos, se suelta y se reintenta.
                if RESOLVE_ACTIVE.load(seq) + RESOLVE_USER_ACTIVE.load(seq) > 3 {
                    counter.fetch_sub(1, seq);
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                let w = t0.elapsed().as_millis();
                if w > 1000 {
                    eprintln!("[yt-cupo] espera={w}ms cupo={} (lento)", if kind == SlotKind::User { "usuario" } else { "precarga" });
                }
                return Ok((Self { user: kind == SlotKind::User }, w));
            }
            // W2: un comando de usuario NUNCA se rechaza. Su cupo se espera
            // (los trabajos que lo ocupan están acotados por su propio
            // timeout); si el usuario lanza otra intención, el gen viejo sale
            // arriba como STALE y libera el turno. La precarga, en cambio, se
            // rinde en silencio al agotar su plazo: es fire-and-forget y este
            // error jamás debe llegar a la interfaz.
            if Instant::now() >= deadline && kind == SlotKind::Warm {
                return Err(STALE.into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
impl Drop for ResolveSlotGuard {
    fn drop(&mut self) {
        let counter = if self.user { &RESOLVE_USER_ACTIVE } else { &RESOLVE_ACTIVE };
        counter.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// W3: el único bucle de carga por sesión pasa a ser el cargador por lotes
/// (src/batch.rs); este static sin usar se eliminó en su lugar.

/// Última estrategia que funcionó (sticky): 1=android, 2=web. El siguiente
/// resolve la prueba primero en vez de repetir siempre el mismo orden.
static LAST_WIN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Orden anterior para log de cambios (0=none, 1=android-web, 2=web-android).
static LAST_ORDER: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn log_order_change(current_order: u8) {
    let prev = LAST_ORDER.swap(current_order, std::sync::atomic::Ordering::SeqCst);
    if prev != 0 && prev != current_order {
        let prev_str = if prev == 1 { "android-web" } else { "web-android" };
        let cur_str = if current_order == 1 { "android-web" } else { "web-android" };
        eprintln!("[yt-order] orden={cur_str} (antes={prev_str}) motivo=cambio de estrategia tras 2 fallos reales");
    }
}

/// ¿Hay runtime JS node? (el cliente web lo necesita; sin él va degradado).
/// Se evalúa una vez por proceso.
fn has_node() -> bool {
    static HAS: OnceLock<bool> = OnceLock::new();
    *HAS.get_or_init(|| bin_version("node").is_some())
}

fn debug_log(msg: &str) {
    if std::env::var("MUSICA_DEBUG").as_deref() == Ok("1") {
        eprintln!("[ytdlp] {msg}");
    }
}

/// Claim del inflight (RAII): quien lo tiene resuelve; al soltarlo libera.
struct InflightClaim {
    id: String,
}

impl InflightClaim {
    /// Reclama el id; None si otro hilo ya lo está resolviendo.
    fn claim(id: &str) -> Option<Self> {
        inflight().lock().ok().and_then(|mut s| {
            if s.insert(id.to_string()) {
                Some(Self { id: id.to_string() })
            } else {
                None
            }
        })
    }
}

impl Drop for InflightClaim {
    fn drop(&mut self) {
        inflight().lock().map(|mut s| s.remove(&self.id)).ok();
        dedup_notify();
    }
}

enum Claim {
    /// Nadie lo resolvía: resuelve tú (conserva el claim hasta terminar).
    Mine(InflightClaim),
    /// Apareció en caché mientras esperabas: úsala (0 yt-dlp).
    Cached(String),
}

/// Versión del dedup para despertar esperas (cada claim liberado y cada
/// inserción en caché la incrementan; ver `dedup_notify`).
static DEDUP_SEQ: Mutex<u64> = Mutex::new(0);
static DEDUP_CV: std::sync::Condvar = std::sync::Condvar::new();

/// Despierta a los que esperan un id (llamar al liberar claim e insertar).
fn dedup_notify() {
    if let Ok(mut s) = DEDUP_SEQ.lock() {
        *s = s.wrapping_add(1);
    }
    DEDUP_CV.notify_all();
}

/// Reclama o espera el inflight (un solo yt-dlp en vuelo por id, ningún
/// camino lo esquiva: transición, precarga y reintento pasan por aquí).
/// Espera dirigida por Condvar (despierta al instante, sin sondeo de 200 ms;
/// el plazo total sigue acotando por seguridad).
fn claim_or_wait(id: &str, stale: &dyn Fn() -> bool) -> Result<Claim, String> {
    loop {
        if let Some(c) = InflightClaim::claim(id) {
            return Ok(Claim::Mine(c));
        }
        let t_dedup = Instant::now();
        let deadline = t_dedup + RESOLVE_TIMEOUT;
        let mut seq = DEDUP_SEQ.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if stale() {
                return Err(STALE.into());
            }
            if let Some(url) = cache_get(id) {
                debug_log(&format!("cache compartida {id}"));
                eprintln!("[yt-dedup] id={id} espera={}ms resultado=cache", t_dedup.elapsed().as_millis());
                return Ok(Claim::Cached(url));
            }
            // El dueño ya no está y no cacheó: no esperes el plazo, reclama tú.
            let owner_gone = inflight().lock().map(|s| !s.contains(id)).unwrap_or(true);
            if owner_gone {
                eprintln!("[yt-dedup] id={id} espera={}ms resultado=dueno-fin", t_dedup.elapsed().as_millis());
                break;
            }
            if Instant::now() >= deadline {
                eprintln!("[yt-dedup] id={id} espera={}ms resultado=timeout", t_dedup.elapsed().as_millis());
                break;
            }
            // Dormir hasta el próximo aviso o el plazo (sin sondeo fijo).
            let remain = deadline.saturating_duration_since(Instant::now());
            seq = DEDUP_CV.wait_timeout(seq, remain).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

/// Variante con generación: si `expect_gen` deja de ser vigente, aborta el
/// hijo y devuelve STALE ("la última gana", sin retardar nada).
/// Con `None` resuelve sin generación (tests y usos puntuales).
/// `slot` separa cupos: el usuario nunca espera a la precarga (T3).
pub fn resolve_audio_gen(path: &str, expect_gen: Option<u64>, slot: SlotKind) -> Result<String, String> {
    let stale_now = || expect_gen.is_some_and(|g| g != current_gen());
    let id = path.strip_prefix("ytmusic://").ok_or("URI de YouTube inválida")?;
    if id.is_empty() {
        return Err("URI de YouTube inválida".into());
    }
    if stale_now() {
        return Err(STALE.into());
    }
    // Caché negativa: no disponible permanente → sin yt-dlp, sin cupo, al
    // instante (el auto-avance la salta; el clic de usuario la limpia antes).
    if let Some(motivo) = neg_get(path) {
        neg_log_once(id, &motivo);
        return Err(motivo);
    }
    let t0 = Instant::now();
    if let Some(url) = cache_get(id) {
        debug_log(&format!("cache hit {id}"));
        return Ok(url);
    }
    // Dedup con vuelo único por id (un solo yt-dlp en vuelo por id): si otro
    // hilo lo resuelve, se espera su caché; si termina SIN cachear (falló o
    // lo canceló un salto), se sale DE INMEDIATO al intento propio (antes la
    // espera fantasma de ~15 s con hijos_ytdlp=0 daba transiciones de ~16 s).
    // El claim (RAII) libera el cupo al terminar/cancelar/fallar.
    let _claim = match claim_or_wait(id, &stale_now)? {
        Claim::Cached(url) => return Ok(url),
        Claim::Mine(c) => c,
    };
    // Máximo 2 yt-dlp a la vez (cada uno es Python+node; más degrada todo).
    // La precarga fallida nunca ocupa el cupo del usuario: usa el mismo cupo
    // pero con aborto por generación (sale rápido si el usuario ya saltó).
    let (_slot, cupo_ms) = ResolveSlotGuard::acquire(slot, &stale_now, Instant::now() + RESOLVE_TIMEOUT)?;
    let url = audio_url_chain(id, &stale_now, cupo_ms)?;
    debug_log(&format!("resuelto {id} en {}ms", t0.elapsed().as_millis()));
    cache_insert(id, url.clone());
    Ok(url)
}

/// Cadena de intentos con aborto por generación (el núcleo real).
/// Estrategia sticky: la que funcionó la última vez va primero (android suele
/// entregar sin throttling, pero con el experimento SABR a veces falla y el
/// web gana; no tiene sentido repetir siempre el mismo orden). Lo imposible
/// (borrado/privado/región) no se reintenta. `cupo_ms` es la espera de cupo
/// ya medida por el llamante (tramo `espera_de_cupo_ms`).
fn audio_url_chain(video_id: &str, stale: &dyn Fn() -> bool, cupo_ms: u128) -> Result<String, String> {
    let r = audio_url_chain_inner(video_id, stale, cupo_ms);
    // Solo lo PERMANENTE entra en caché negativa (lo transitorio reintenta).
    if let Err(e) = &r {
        if e != STALE && looks_like_gone(e) {
            neg_store(video_id, e);
        }
    }
    r
}

/// Cadena de intentos con aborto por generación (el núcleo real).
fn audio_url_chain_inner(video_id: &str, stale: &dyn Fn() -> bool, cupo_ms: u128) -> Result<String, String> {
    let t_chain = Instant::now();
    let abort = || stale();
    let web_first = LAST_WIN.load(std::sync::atomic::Ordering::SeqCst) == 2;
    let orden = if web_first { "web-android" } else { "android-web" };
    let first_android = !web_first;
    let e1 = match audio_url_attempt(video_id, false, first_android, Some(&abort), cupo_ms) {
        Ok(u) => {
            LAST_WIN.store(if first_android { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
            eprintln!("[yt-chain] id={video_id} orden={orden} ganador={} total={}ms", if first_android { "android" } else { "web" }, t_chain.elapsed().as_millis());
            return Ok(u);
        }
        Err(e) => {
            if e == STALE {
                return Err(e);
            }
            e
        }
    };
    // Log si cambió el orden de estrategias (tras 2 fallos reales).
    let current_order = if web_first { 2 } else { 1 };
    log_order_change(current_order);
    if looks_like_gone(&e1) {
        return Err(e1);
    }
    // Barrera de edad: UNA oportunidad con sesión; sin sesión o segundo fallo
    // → caché negativa con motivo propio (no se reintenta hasta que cambie).
    if looks_like_age(&e1) {
        if has_cookies() {
            match audio_url_attempt(video_id, true, false, Some(&abort), cupo_ms) {
                Ok(u) => {
                    LAST_WIN.store(2, std::sync::atomic::Ordering::SeqCst);
                    log_order_change(2);
                    eprintln!("[yt-chain] id={video_id} orden={orden} ganador=web-cookies total={}ms", t_chain.elapsed().as_millis());
                    return Ok(u);
                }
                Err(e2) => {
                    if e2 == STALE {
                        return Err(e2);
                    }
                    neg_store(video_id, AGE_MOTIVE);
                    return Err(AGE_MOTIVE.into());
                }
            }
        }
        neg_store(video_id, AGE_MOTIVE);
        return Err(AGE_MOTIVE.into());
    }
    if looks_like_auth(&e1) && has_cookies() {
        return audio_url_attempt(video_id, true, false, Some(&abort), cupo_ms).map_err(|e2| {
            if e2 == STALE {
                e2
            } else if looks_like_auth(&e2) {
                format!("{e2} (sesión no válida o expirada: vuelve a conectar)")
            } else {
                e2
            }
        });
    }
    if looks_like_auth(&e1) {
        // Sin cookies no hay nada que mejore un web anónimo: falla con el
        // mensaje que pide conectar (no quemes un segundo intento inútil).
        return Err(format!("{e1} (conecta tu cuenta para este contenido)"));
    }
    // Último recurso ante desafío técnico o error genérico: el otro cliente
    // (puede ir throttled, pero a veces resuelve lo que el primero no).
    if looks_like_challenge(&e1) || !looks_like_gone(&e1) {
        let second_android = web_first;
        match audio_url_attempt(video_id, false, second_android, Some(&abort), cupo_ms) {
            Ok(u) => {
                LAST_WIN.store(if second_android { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
                log_order_change(if second_android { 1 } else { 2 });
                eprintln!("[yt-chain] id={video_id} orden={orden} ganador={} total={}ms", if second_android { "android" } else { "web" }, t_chain.elapsed().as_millis());
                return Ok(u);
            }
            Err(e2) => {
                if e2 != STALE {
                    eprintln!("[yt-chain] id={video_id} orden={orden} ganador=ninguno total={}ms", t_chain.elapsed().as_millis());
                }
                return Err(e2);
            }
        }
    }
    Err(e1)
}

/// ¿Huele a desafío técnico (PO token/nsig/recarga) y no a sesión?
/// (puro, testeable). Estos SÍ reintentan con otro player_client.
pub fn looks_like_challenge(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["po token", "nsig", "needs to be reloaded", "player response",
     "unable to extract", "did not get any formats", "requested format not available",
     "http error 429", "too many requests", "not a bot"]
        .iter()
        .any(|k| m.contains(k))
}

/// ¿Contenido imposible (borrado/privado/región)? No se reintenta.
/// (puro, testeable). OJO: "video is not available" es permanente, pero
/// "requested FORMAT not available" es transitorio (SABR) y NO entra aquí.
pub fn looks_like_gone(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["video unavailable", "video is not available", "has been removed", "has been deleted",
     "not available in your country", "blocked it on copyright",
     "private video", "does not exist"]
        .iter()
        .any(|k| m.contains(k))
}

/// ¿Barrera de edad ("Sign in to confirm your age")? No es "sesión expirada":
/// lleva motivo propio y solo se reintenta con sesión. (puro, testeable).
pub fn looks_like_age(msg: &str) -> bool {
    msg.to_lowercase().contains("confirm your age")
}

/// Motivo guardado para la barrera de edad (tooltip + caché negativa).
pub const AGE_MOTIVE: &str = "requiere sesión (edad): confirma tu edad en YouTube";

/// Núcleo con timeout corto de resolución.
/// Registra por intento: estrategia (cliente/args, SIN la URL), resultado,
/// código de salida, tramos (cupo/spawn/exec/parseo) y cola del stderr si el
/// intento tarda más de 5 s. Nunca registra URLs ni IPs.
fn audio_url_attempt(
    video_id: &str,
    cookies: bool,
    android: bool,
    abort: Option<&dyn Fn() -> bool>,
    cupo_ms: u128,
) -> Result<String, String> {
    let url = format!("https://music.youtube.com/watch?v={video_id}");
    // -4: las URLs llevan ip= del cliente; si yt-dlp resuelve por IPv6 y mpv
    // conecta por IPv4 (o al revés), googlevideo responde 403. Forzar IPv4 en
    // ambos lados mantiene la misma familia.
    let mut args: Vec<&str> =
        vec!["-4", "-f", "bestaudio[ext=m4a]/bestaudio/best", "--get-url", "--no-playlist"];
    // El cliente web necesita runtime JS; sin él va degradado (medido: aviso
    // "No supported JavaScript runtime" y formatos faltantes). Si hay node,
    // se declara explícito (por defecto yt-dlp solo habilita deno).
    if !android && has_node() {
        args.push("--js-runtimes");
        args.push("node");
    }
    if android {
        args.extend(["--extractor-args", "youtube:player_client=android"]);
    }
    args.push(&url);
    let cliente = if android { "android" } else { "web" };
    let out = match run_ytdlp_blocking_timeout(&args, cookies, RESOLVE_TIMEOUT, abort) {
        Ok(o) => o,
        Err(e) => {
            if e.msg != STALE {
                if looks_like_throttle(&e.msg) {
                    note_throttle();
                }
                eprintln!(
                    "[yt-attempt] id={video_id} cliente={cliente} cookies={} resultado=error codigo={} espera_cupo={cupo_ms}ms spawn={}ms ejec={}ms detalle={}",
                    cookies as u8, e.exit_code, e.spawn_ms, e.exec_ms,
                    if e.stderr_tail.is_empty() { e.msg.clone() } else { format!("{} | {}", e.msg, e.stderr_tail) }
                );
            }
            return Err(e.msg);
        }
    };
    let t_parse = Instant::now();
    let found = out
        .stdout
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("http"))
        .map(str::to_string);
    let parse_ms = t_parse.elapsed().as_millis();
    // Lento (>5 s) o con avisos: una línea con estrategia y tramos.
    if out.exec_ms > SLOW_MS || !out.stderr_tail.is_empty() {
        eprintln!(
            "[yt-attempt] id={video_id} cliente={cliente} cookies={} resultado={} codigo={} espera_cupo={cupo_ms}ms spawn={}ms ejec={}ms parseo={}ms detalle={}",
            cookies as u8,
            if found.is_some() { "ok" } else { "sin-url" },
            out.exit_code, out.spawn_ms, out.exec_ms, parse_ms, out.stderr_tail
        );
    }
    found.ok_or_else(|| "yt-dlp no devolvió URL de audio (¿video no disponible o restringido?)".into())
}

/// Re-resolución con OTRA estrategia (el reintento nunca repite la misma URL):
/// invalida la caché y prueba cliente web (el normal usa android primero).
/// Si lo imposible, no reintenta.
pub fn resolve_audio_retry(path: &str, expect_gen: Option<u64>) -> Result<String, String> {
    let stale_now = || expect_gen.is_some_and(|g| g != current_gen());
    let id = path.strip_prefix("ytmusic://").ok_or("URI de YouTube inválida")?;
    if id.is_empty() {
        return Err("URI de YouTube inválida".into());
    }
    invalidate_cache(path);
    // La caché negativa también vale aquí (reintento automático del tick):
    // no quemar otro yt-dlp en un video muerto.
    if let Some(motivo) = neg_get(path) {
        neg_log_once(id, &motivo);
        return Err(motivo);
    }
    // Vuelo único por id (sin esquives): si otro hilo lo resuelve, se usa su
    // caché; si no, se reclama antes del primer yt-dlp.
    let _claim = match claim_or_wait(id, &stale_now)? {
        Claim::Cached(url) => return Ok(url),
        Claim::Mine(c) => c,
    };
    let (_slot, cupo_ms) = ResolveSlotGuard::acquire(SlotKind::User, &stale_now, Instant::now() + RESOLVE_TIMEOUT)?;
    let abort = || stale_now();
    // Web primero (distinto CDN/familia IP que el android que falló).
    match audio_url_attempt(id, false, false, Some(&abort), cupo_ms) {
        Ok(u) => {
            LAST_WIN.store(2, std::sync::atomic::Ordering::SeqCst);
            log_order_change(2);
            // Nunca purgar la caché entera por un reintento: solo se había
            // invalidado el path propio; lo demás sigue vigente y se guarda.
            cache_insert(id, u.clone());
            return Ok(u);
        }
        Err(e) => {
            if e == STALE {
                return Err(e);
            }
            // Lo permanente se guarda aquí mismo (este retorno no pasa por la
            // cadena, que es la que alimenta la caché negativa).
            if looks_like_gone(&e) {
                neg_store(id, &e);
                return Err(e);
            }
        }
    }
    let url = audio_url_chain(id, &stale_now, cupo_ms)?;
    cache_insert(id, url.clone());
    Ok(url)
}

/// Margen de renovación: una URL cercana que venza en <10 min se re-resuelve
/// en segundo plano antes de que mpv reciba un 403.
const RENEW_MARGIN: Duration = Duration::from_secs(10 * 60);

/// Precarga con renovación de vigencia (W3): solo las pistas cercanas a la
/// actual a punto de vencer se re-resuelven; el resto ya está en caché y no
/// consume yt-dlp.
pub fn warm_renew_if_needed(path: &str) {
    let id = match path.strip_prefix("ytmusic://") {
        Some(i) => i,
        None => return,
    };
    if let Some(rem) = cache_remaining(id) {
        if rem > RENEW_MARGIN {
            return;
        }
        // Casi vencida: fuerza la re-resolución (el dedup por id evita un
        // segundo yt-dlp si otro hilo ya la está refrescando).
        invalidate_cache(path);
    }
    resolve_audio_gen(path, None, SlotKind::Warm).ok();
}

/// Precarga en paralelo (hasta 2 simultáneas, cupo warm) una ventana de
/// 5 pistas adelante + 1 atrás. Prioridad por cercanía a la pista actual.
/// Fire-and-forget: respeta "la última gana" y el cupo de 2 warm.
pub fn warm_next(paths: &[String], gen: u64, current_idx: Option<usize>) {
    // W5: las muertas conocidas no se intentan de nuevo (ni se loguean):
    // el auto-avance las salta y el clic de usuario las limpia a propósito.
    let mut items: Vec<(usize, String)> = paths
        .iter()
        .enumerate()
        .filter(|(_, p)| p.starts_with("ytmusic://") && neg_get(p).is_none())
        .map(|(i, p)| (i, p.clone()))
        .collect();
    if items.is_empty() {
        return;
    }
    // Ordenar por distancia a current_idx (si hay), los más cercanos primero.
    if let Some(idx) = current_idx {
        items.sort_by_key(|(i, _)| i.abs_diff(idx));
    }
    // Ventana: 5 adelante + 1 atrás = máx 6.
    let window: Vec<String> = items.into_iter().take(6).map(|(_, p)| p).collect();
    if window.is_empty() {
        return;
    }
    // Disparamos en paralelo hasta 2 (cupo warm), sin bloqueo global.
    for p in window {
        if gen != current_gen() {
            break;
        }
        let p = p.clone();
        std::thread::Builder::new()
            .name("yt-warm".into())
            .spawn(move || {
                if gen != current_gen() {
                    return;
                }
                // W1: una vez empezado no se cancela por gen: termina, se
                // guarda en caché y sirve aunque la pista se haya abandonado.
                // W3: además renueva las URLs a punto de vencer.
                warm_renew_if_needed(&p);
            })
            .ok();
    }
}

// ---- sesión (Parte 2) ----

#[derive(Debug, Clone, Serialize)]
pub struct BrowserProfile {
    /// Etiqueta para el selector ("Brave Nightly (Default)").
    pub label: String,
    /// Nombre base válido para yt-dlp (lista blanca interna).
    pub browser: String,
    /// Perfil o ruta del perfil (None = por defecto de yt-dlp).
    pub profile: Option<String>,
}

/// BUG 1: yt-dlp SOLO acepta brave/chrome/chromium/edge/firefox/opera/safari/
/// vivaldi/whale. Pasar "brave-nightly" como nombre falla. Por eso se separa
/// el nombre para mostrar del argumento real: base válida + ruta del perfil
/// (`brave:/ruta/perfil`), que sí acepta variantes, Flatpak y Snap.
pub fn yt_spec(browser: &str, profile: Option<&str>) -> Result<String, String> {
    const ALLOWED: &[&str] =
        &["brave", "chrome", "chromium", "edge", "firefox", "opera", "safari", "vivaldi", "whale"];
    let base = browser.trim().to_lowercase();
    if !ALLOWED.contains(&base.as_str()) {
        return Err(format!(
            "navegador no soportado para yt-dlp (usa: {} o importa cookies.txt)",
            ALLOWED.join(", ")
        ));
    }
    match profile.map(str::trim).filter(|p| !p.is_empty()) {
        None => Ok(base),
        Some(p) => Ok(format!("{base}:{p}")),
    }
}

/// Detecta navegadores instalados con base de cookies (estable, Beta,
/// Nightly, Flatpak, Snap, Firefox, Chromium, Chrome). Solo se listan los
/// encontrados: el selector nunca ofrece rutas fantasma.
///
/// W2: en Windows no se detectan navegadores: las cookies de Chrome/Brave/
/// Firefox van cifradas con DPAPI y sus bases quedan bloqueadas mientras el
/// navegador corre, así que "Conectar cuenta" queda fuera de alcance (se usa
/// cookies.txt importado). Lista vacía = selector sin rutas fantasma.
#[cfg(windows)]
pub fn detect_browsers() -> Vec<BrowserProfile> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn detect_browsers() -> Vec<BrowserProfile> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut out: Vec<BrowserProfile> = vec![];
    let mut push = |label: String, browser: &str, profile: Option<String>, db: &Path| {
        if db.is_file() && !out.iter().any(|b: &BrowserProfile| b.label == label) {
            out.push(BrowserProfile { label, browser: browser.to_string(), profile });
        }
    };
    // Chromium y derivados: <base>/<Perfil>/Cookies.
    let chromium_like: &[(&str, &str, &[&str])] = &[
        ("brave", "Brave", &["BraveSoftware/Brave-Browser"]),
        ("brave", "Brave Beta", &["BraveSoftware/Brave-Browser-Beta"]),
        ("brave", "Brave Nightly", &["BraveSoftware/Brave-Browser-Nightly"]),
        ("chrome", "Chrome", &["google-chrome", "google-chrome-beta", "google-chrome-unstable"]),
        ("chromium", "Chromium", &["chromium"]),
    ];
    let roots = [
        format!("{home}/.config"),
        format!("{home}/.var/app/com.brave.Browser/config"),
        format!("{home}/snap/brave/common/.config"),
        format!("{home}/snap/chromium/common/chromium"),
        format!("{home}/Library/Application Support"),
    ];
    for (base, label, bases) in chromium_like {
        for root in &roots {
            for b in *bases {
                // ¿Es la ruta estándar de yt-dlp para esta base? Entonces basta
                // el nombre (perfil Default) o `base:Perfil`.
                let standard = *b == "BraveSoftware/Brave-Browser" && *base == "brave";
                for profile in ["Default", "Profile 1", "Profile 2"] {
                    let dir = PathBuf::from(format!("{root}/{b}/{profile}"));
                    let db = dir.join("Cookies");
                    if !db.is_file() {
                        continue;
                    }
                    if standard && profile == "Default" {
                        push(format!("{label} ({profile})"), base, None, &db);
                    } else if standard {
                        push(
                            format!("{label} ({profile})"),
                            base,
                            Some(profile.to_string()),
                            &db,
                        );
                    } else {
                        push(
                            format!("{label} ({profile})"),
                            base,
                            Some(dir.to_string_lossy().into_owned()),
                            &db,
                        );
                    }
                }
            }
        }
    }
    // Firefox: perfiles con cookies.sqlite (se pasa el NOMBRE del perfil).
    for root in [
        format!("{home}/.mozilla/firefox"),
        format!("{home}/snap/firefox/common/.mozilla/firefox"),
        format!("{home}/Library/Application Support/Firefox/Profiles"),
    ] {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                let db = e.path().join("cookies.sqlite");
                if db.is_file() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    push(format!("Firefox ({name})"), "firefox", Some(name), &db);
                }
            }
        }
    }
    out
}

/// Los tres fallos de exportación con acción concreta (puro, testeable).
fn specific_export_error(stderr: &str) -> Option<String> {
    let m = stderr.to_lowercase();
    if m.contains("could not find") && m.contains("cookies database") {
        return Some("No se encontró la base de cookies: elige el navegador y perfil correctos o importa un cookies.txt".into());
    }
    if m.contains("lock") && (m.contains("database") || m.contains("sqlite")) {
        return Some("El navegador está abierto y bloquea sus cookies: ciérralo e inténtalo de nuevo".into());
    }
    if m.contains("decrypt") || m.contains("keyring") || m.contains("secret service") || m.contains("kwallet") {
        return Some("No se pudieron descifrar las cookies (llavero bloqueado): desbloquea tu llavero e inténtalo de nuevo, o usa Importar cookies.txt".into());
    }
    None
}

/// Traduce fallos de exportación (puro, testeable): los tres fallos con acción
/// concreta tal cual; el resto, con la misma traducción de sesión que usa
/// importar listas (sin doble prefijo).
pub fn classify_export_error(stderr: &str) -> String {
    specific_export_error(stderr)
        .unwrap_or_else(|| friendly_import_error(LIKED_URL, &classify_stderr(stderr)))
}

/// Fuerza 0600 en el archivo de cookies (avisa si no se puede).
fn chmod_600(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !path.is_file() {
            return; // aún no existe (p. ej. yt-dlp no llegó a crearlo)
        }
        if std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).is_err() {
            eprintln!("[musica] aviso: no se pudo poner 0600 a {}", path.display());
        }
    }
}

/// Exporta cookies del navegador a nuestro archivo 0600 y valida con LL.
/// `browser` es el nombre base (lista blanca) y `profile` el perfil o ruta.
/// NUNCA pide ni guarda contraseñas.
pub async fn connect_browser(browser: &str, profile: Option<&str>) -> Result<(), String> {
    // Mientras el intento no valide, el jar no cuenta como sesión (sin
    // borrar nada); se limpia al final si la validación sale bien.
    mark_session_invalid();
    let spec = yt_spec(browser, profile)?;
    // Verifica la ruta ANTES de llamar a yt-dlp (nunca un nombre no
    // soportado ni una ruta fantasma).
    if let Some((base, path)) = spec.split_once(':') {
        let dir = Path::new(path);
        let db = if base == "firefox" {
            dir.join("cookies.sqlite")
        } else {
            dir.join("Cookies")
        };
        // Solo las rutas explícitas se verifican (los nombres simples los
        // resuelve yt-dlp en sus rutas estándar).
        if path.contains('/') && !db.is_file() {
            return Err(format!(
                "No encontré cookies en {path}: elige otro perfil o usa 'Importar cookies.txt'"
            ));
        }
    }
    if !probe().available {
        return Err("yt-dlp no está instalado. Instálalo con: sudo pacman -S yt-dlp".into());
    }
    let dir = app_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("datos de app: {e}"))?;
    let cookies = cookies_path();
    let export = tools::tokio_command("yt-dlp")
        .args([
            "--no-warnings", "--no-progress",
            "--cookies-from-browser", &spec,
            "--cookies", &cookies.to_string_lossy(),
            "--skip-download", "--print", "%(id)s",
            "--playlist-end", "1",
            LIKED_URL,
        ])
        .output();
    let out = tokio::time::timeout(CMD_TIMEOUT, export)
        .await
        .map_err(|_| "yt-dlp tardó demasiado".to_string())?
        .map_err(|e| format!("no se pudo ejecutar yt-dlp: {e}"))?;
    // yt-dlp reescribe el jar al salir (también si falla): reaplica 0600.
    chmod_600(&cookies);
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(classify_export_error(&stderr));
    }
    // Validada: el jar vuelve a contar como sesión.
    clear_session_invalid();
    // Guarda el spec para reconectar sin elegir de nuevo (L4: fichero 0600).
    crate::keyring::set(SERVICE_YT, "browser", &spec).ok();
    write_method(&spec);
    // Sesión nueva: las marcas de edad pueden haber caducado.
    neg_clear_age();
    Ok(())
}

/// Respaldo: importa un cookies.txt exportado del navegador (p. ej. con la
/// extensión "Get cookies.txt LOCALLY"). Lo copia con 0600 y valida con LL.
pub async fn import_cookies_file(src: &str) -> Result<(), String> {
    let src = Path::new(src.trim());
    if !src.is_file() {
        return Err("el archivo no existe".into());
    }
    let dir = app_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("datos de app: {e}"))?;
    let dst = cookies_path();
    std::fs::copy(src, &dst).map_err(|e| format!("no se pudo copiar: {e}"))?;
    chmod_600(&dst);
    // Valida que sirva (LL exige sesión).
    let probe_out = run_ytdlp(
        &["--flat-playlist", "--playlist-end", "1", "--print", "%(id)s",
          LIKED_URL],
        true,
    )
    .await;
    match probe_out {
        Ok(_) => {
            crate::keyring::set(SERVICE_YT, "browser", "cookies.txt").ok();
            write_method("cookies.txt");
            // Validada: el jar vuelve a contar como sesión.
            clear_session_invalid();
            // Sesión nueva: las marcas de edad pueden haber caducado.
            neg_clear_age();
            Ok(())
        }
        Err(e) => {
            std::fs::remove_file(&dst).ok();
            Err(format!("esas cookies no abren tu sesión: {e}"))
        }
    }
}

// ---------- L3b: login integrado de Google (ventana con datos aislados) ----------

/// ¿Dominio de sesión de YouTube/Google? (puro). `evil-youtube.com` NO cuenta.
pub fn is_session_cookie_domain(domain: &str) -> bool {
    let d = domain.trim_start_matches('.');
    d == "youtube.com"
        || d.ends_with(".youtube.com")
        || d == "google.com"
        || d.ends_with(".google.com")
}

/// ¿Login completado? (puro) Hacen falta las DOS cookies: SAPISID y LOGIN_INFO,
/// en dominios de sesión. Son las que aparecen solo con sesión iniciada.
pub fn has_login_cookies(cookies: &[Cookie<'static>]) -> bool {
    let (mut sapisid, mut login) = (false, false);
    for c in cookies {
        if !c.domain().map(is_session_cookie_domain).unwrap_or(false) {
            continue;
        }
        match c.name() {
            "SAPISID" => sapisid = true,
            "LOGIN_INFO" => login = true,
            _ => {}
        }
    }
    sapisid && login
}

/// Serializa a formato Netscape (puro): SOLO cookies de youtube.com/google.com,
/// orden estable, 0 = cookie de sesión. Nunca se registra el valor en logs.
pub fn to_netscape(cookies: &[Cookie<'static>]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for c in cookies {
        let Some(domain) = c.domain() else { continue };
        if !is_session_cookie_domain(domain) {
            continue;
        }
        let (name, value) = (c.name(), c.value());
        if name.contains(['\t', '\n', '\r']) || value.contains(['\t', '\n', '\r']) {
            continue; // no rompen el formato tabulado
        }
        // La crate de cookies normaliza `Domain=.youtube.com` → `youtube.com`;
        // Google emite estas con punto (valen para subdominios), así que el jar
        // recupera los apex como cookie de dominio; el resto, de host.
        let host = domain.trim_start_matches('.');
        let (jar_domain, sub) = if host == "youtube.com" || host == "google.com" {
            (format!(".{host}"), "TRUE")
        } else {
            (host.to_string(), "FALSE")
        };
        let path = c.path().unwrap_or("/");
        let secure = if c.secure().unwrap_or(false) { "TRUE" } else { "FALSE" };
        let expires = c.expires_datetime().map(|d| d.unix_timestamp()).unwrap_or(0);
        lines.push(format!("{jar_domain}\t{sub}\t{path}\t{secure}\t{expires}\t{name}\t{value}"));
    }
    lines.sort();
    lines.dedup();
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// Escribe un fichero privado (0600 en Unix) truncando el anterior.
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(data)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data)
    }
}

/// L3b: escribe el candidato (aún sin validar) del login integrado.
pub fn write_login_candidate(netscape: &str) -> Result<(), String> {
    std::fs::create_dir_all(app_dir()).map_err(|e| format!("datos de app: {e}"))?;
    let p = login_candidate_path();
    write_private(&p, netscape.as_bytes())
        .map_err(|e| format!("no se pudo escribir el candidato: {e}"))?;
    chmod_600(&p);
    Ok(())
}

/// L3b: valida el candidato con la misma petición barata de L2 (LL, un ítem).
/// Respeta el backoff de 429 (lo refuerza si el 429 es real).
pub async fn validate_login_candidate() -> CheckOutcome {
    let p = login_candidate_path();
    let res = run_ytdlp_at(
        &["--flat-playlist", "-I", "1", "--print", "%(id)s", LIKED_URL],
        Some(p.as_path()),
    )
    .await;
    let outcome = classify_check(res.is_ok(), res.as_deref().unwrap_or(""));
    if let Err(e) = &res {
        if looks_like_throttle(e) {
            note_throttle();
        }
    }
    outcome
}

/// L3b: validación OK → el candidato sustituye al jar (atómico) y la sesión
/// vuelve a contar. Se guarda el método para mostrarlo en la UI.
pub fn adopt_login_candidate() -> Result<(), String> {
    std::fs::create_dir_all(app_dir()).map_err(|e| format!("datos de app: {e}"))?;
    let src = login_candidate_path();
    let dst = cookies_path();
    replace_file(&src, &dst).map_err(|e| format!("no se pudo guardar el jar: {e}"))?;
    chmod_600(&dst);
    clear_session_invalid();
    crate::keyring::set(SERVICE_YT, "browser", "Google (ventana integrada)").ok();
    write_method("Google (ventana integrada)");
    // Sesión nueva: las marcas de edad pueden haber caducado.
    neg_clear_age();
    Ok(())
}

/// L3b: descarta el candidato sin tocar el jar real (fallo de validación).
pub fn discard_login_candidate() {
    std::fs::remove_file(login_candidate_path()).ok();
}

/// Renombra sustituyendo el destino (en Windows `rename` no pisa).
fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::fs::remove_file(dst).ok();
    }
    std::fs::rename(src, dst)
}

/// L2: resultado de verificar la sesión (la UI lo pinta).
#[derive(Debug, Clone, Serialize)]
pub struct SessionCheck {
    /// "valid" | "expired" | "unverified" | "none" (sin jar que verificar).
    pub status: &'static str,
    /// Estado ya actualizado (marcador aplicado) para la UI.
    pub account: YtAccount,
}

/// Clase de un resultado de verificación (puro, testeable).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CheckOutcome {
    Valid,
    Expired,
    Unverified,
}

/// Decide el resultado sin red (puro, testeable): OK = válida; 429/bot = no
/// concluyente (manda el throttle); auth o "playlist does not exist" en LL =
/// caducada; todo lo demás (red, timeout) = no concluyente.
pub fn classify_check(ok: bool, raw: &str) -> CheckOutcome {
    if ok {
        return CheckOutcome::Valid;
    }
    if looks_like_throttle(raw) {
        return CheckOutcome::Unverified;
    }
    if looks_like_auth(raw) || raw.to_lowercase().contains("playlist does not exist") {
        return CheckOutcome::Expired;
    }
    CheckOutcome::Unverified
}

/// Efecto del resultado sobre el marcador: válida lo quita, caducada lo deja
/// puesto, no concluyente no lo toca.
fn apply_check_mark(outcome: CheckOutcome) {
    match outcome {
        CheckOutcome::Valid => clear_session_invalid(),
        CheckOutcome::Expired => mark_session_invalid(),
        CheckOutcome::Unverified => {}
    }
}

/// L2: verifica la sesión con la petición más barata (LL, un ítem) y el mismo
/// cookies. Respeta el backoff de 429 (con throttle activo ni la lanza) y no
/// la marca caducada por fallos no concluyentes (red/timeout/429).
pub async fn verify_session() -> SessionCheck {
    if !has_cookies() {
        return SessionCheck { status: "none", account: account() };
    }
    if throttle_active() {
        return SessionCheck { status: "unverified", account: account() };
    }
    let res = run_ytdlp(
        &["--flat-playlist", "-I", "1", "--print", "%(id)s", LIKED_URL],
        true,
    )
    .await;
    let outcome = classify_check(res.is_ok(), res.as_deref().unwrap_or(""));
    if let Err(e) = &res {
        if looks_like_throttle(e) {
            note_throttle(); // 429 real: refuerza el backoff para el resto
        }
    }
    apply_check_mark(outcome);
    SessionCheck {
        status: match outcome {
            CheckOutcome::Valid => "valid",
            CheckOutcome::Expired => "expired",
            CheckOutcome::Unverified => "unverified",
        },
        account: account(),
    }
}

pub fn disconnect() {
    std::fs::remove_file(cookies_path()).ok();
    clear_session_invalid();
    remove_method();
    crate::keyring::remove(SERVICE_YT, "browser");
    crate::keyring::remove(SERVICE_YT, "channel_url");
    // Sin sesión: las marcas de edad se olvidan (se reevalúan al reconectar).
    neg_clear_age();
    if let Ok(mut c) = cache().lock() {
        c.clear();
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct YtMine {
    pub name: String,
    pub url: String,
    pub count: Option<usize>,
}

/// "Mis playlists": Me gustan (Music + YouTube) + canal propio si se configuró.
/// Las privadas aparecen porque van con las cookies de la sesión.
pub async fn my_playlists() -> Result<Vec<YtMine>, String> {
    if !has_cookies() {
        return Err("sin sesión: conecta tu cuenta primero".into());
    }
    let mut out = vec![
        YtMine {
            name: "Canciones que me gustan (Music)".into(),
            url: "https://music.youtube.com/playlist?list=LM".into(),
            count: None,
        },
        YtMine {
            name: "Me gustan (YouTube)".into(),
            url: LIKED_URL.into(),
            count: None,
        },
    ];
    // Canal propio (opcional): sus playlists, incluidas no listadas.
    if let Some(channel) = crate::keyring::get(SERVICE_YT, "channel_url") {
        let channel = channel.trim().trim_end_matches('/').to_string();
        if !channel.is_empty() {
            let url = format!("{channel}/playlists");
            match run_ytdlp(
                &["--flat-playlist", "--print", "%(playlist_id)s\t%(playlist_title)s", &url],
                true,
            )
            .await
            {
                Ok(text) => {
                    for line in text.lines() {
                        let mut p = line.split('\t');
                        let (Some(id), Some(title)) = (p.next(), p.next()) else { continue };
                        if id.trim().is_empty() {
                            continue;
                        }
                        out.push(YtMine {
                            name: title.trim().to_string(),
                            url: format!("https://music.youtube.com/playlist?list={}", id.trim()),
                            count: None,
                        });
                    }
                }
                Err(e) => {
                    // El canal puede estar mal: no rompe lo demás, se avisa.
                    out.push(YtMine {
                        name: format!("(canal inaccesible: {e})"),
                        url: String::new(),
                        count: None,
                    });
                }
            }
        }
    }
    Ok(out)
}

pub fn set_channel(url: &str) -> Result<(), String> {
    let u = url.trim();
    if u.is_empty() {
        crate::keyring::remove(SERVICE_YT, "channel_url");
        return Ok(());
    }
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("URL de canal inválida".into());
    }
    crate::keyring::set(SERVICE_YT, "channel_url", u).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clasifica_urls() {
        let (k, _) = classify_url("https://music.youtube.com/playlist?list=PLabc123_-").unwrap();
        assert_eq!(k, "playlist");
        let (k, _) = classify_url("https://www.youtube.com/playlist?list=PLabc").unwrap();
        assert_eq!(k, "playlist");
        let (k, _) = classify_url("https://music.youtube.com/watch?v=rFZHOHl-L8A").unwrap();
        assert_eq!(k, "video");
        let (k, _) = classify_url("https://youtu.be/rFZHOHl-L8A").unwrap();
        assert_eq!(k, "video");
        assert!(classify_url("https://example.com/x").is_err());
        assert!(classify_url("nota musical").is_err());
        assert!(classify_url("").is_err());
    }

    #[test]
    fn extrae_id_de_lista_de_la_url() {
        assert_eq!(
            url_list_id("https://music.youtube.com/playlist?list=PLdMI5QqYLPnM").as_deref(),
            Some("PLdMI5QqYLPnM")
        );
        assert_eq!(
            url_list_id("https://music.youtube.com/watch?v=x&list=RDrFZHOHl-L8A&si=z").as_deref(),
            Some("RDrFZHOHl-L8A")
        );
        assert_eq!(
            url_list_id("https://www.youtube.com/playlist?list=VLPLabc").as_deref(),
            Some("PLabc")
        );
        assert!(url_list_id("https://music.youtube.com/watch?v=x").is_none());
        assert!(url_list_id("no-es-url").is_none());
        assert!(url_list_id("https://x/playlist?list=con espacios").is_none());
    }

    #[test]
    fn parsea_lineas_flat() {
        let t = parse_flat_line("abc123\tMi canción\t214\tAlgún Artista").unwrap();
        assert_eq!((t.id.as_str(), t.duration_secs), ("abc123", 214));
        assert_eq!(t.artist, "Algún Artista");
        // Duración ausente/NA → 0 (directos).
        let t2 = parse_flat_line("xyz\tDirecto\tNA\tRadio").unwrap();
        assert_eq!(t2.duration_secs, 0);
        assert!(parse_flat_line("").is_none());
        assert!(parse_flat_line("NA\tx\t1\ty").is_none());
        // Una sola llamada de yt-dlp trae título de lista + pista.
        let (pl, t3) = parse_flat_line_with_playlist("Mi Lista\tabc123\tTema\t200\tArtista").unwrap();
        assert_eq!(pl.as_deref(), Some("Mi Lista"));
        assert_eq!(t3.id, "abc123");
        let (sin, _) = parse_flat_line_with_playlist("NA\tabc123\tTema\t200\tArtista").unwrap();
        assert!(sin.is_none());
        // to_track: URI perezosa + portada derivada del ID.
        let db = t.to_track("Mi Lista");
        assert_eq!(db.path, "ytmusic://abc123");
        assert_eq!(db.art_url, "https://i.ytimg.com/vi/abc123/hqdefault.jpg");
        assert_eq!(db.genre, "youtube");
    }

    #[test]
    fn errores_claros_y_sesion() {
        assert!(classify_stderr("ERROR: [youtube:tab] LL: YouTube said: The playlist does not exist.")
            .contains("does not exist"));
        assert!(looks_like_auth("ERROR: Sign in to confirm your age"));
        assert!(looks_like_auth("missing required authentication credential"));
        assert!(looks_like_auth("This video is private"));
        assert!(!looks_like_auth("The playlist does not exist"));
        assert!(!looks_like_auth("HTTP Error 429"));
        // Throttle no es sesión.
        assert!(looks_like_throttle("ERROR: HTTP Error 429: Too Many Requests"));
        assert!(looks_like_throttle("confirm you're not a bot"));
        assert!(!looks_like_throttle("The playlist does not exist"));
        // Export: mensajes traducidos con acción.
        assert!(classify_export_error("could not find brave cookies database in /x").contains("cookies.txt"));
        assert!(classify_export_error("ERROR: database is locked").contains("ciérralo"));
        assert!(classify_export_error("Failed to decrypt with keyring").contains("llavero"));
        // Import sin sesión / con sesión muerta: mensajes útiles, no crudo.
        // (En este equipo puede haber cookies: acepta ambas ramas honestas.)
        let m = friendly_import_error("https://music.youtube.com/playlist?list=LM", "yt-dlp: ...");
        assert!(m.contains("conecta tu cuenta") || m.contains("vuelve a conectar"), "{m}");
    }

    #[test]
    fn mapeo_de_errores_de_sesion() {
        // Aísla el dir de datos para no tocar la sesión real del equipo.
        let tmp = std::env::temp_dir().join(format!("musica-yt-test-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::env::set_var("MUSICA_DATA_DIR", &tmp);
        // (1) Sin jar: una lista privada pide conectar la cuenta.
        let m = friendly_import_error(LIKED_URL, "yt-dlp: ...");
        assert!(m.contains("conecta tu cuenta"), "{m}");
        // (2) Con jar: pide reconectar y conserva el detalle crudo.
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("ytmusic-cookies.txt"), "# Netscape HTTP Cookie File\n").unwrap();
        let m = friendly_import_error(LIKED_URL, "yt-dlp: ...");
        assert!(m.contains("vuelve a conectar") && m.contains("Detalle"), "{m}");
        // (3) El fallo de LL en Conectar usa esa misma traducción…
        let m = classify_export_error("ERROR: [youtube:tab] LL: YouTube said: The playlist does not exist.");
        assert!(m.contains("Sesión no válida o expirada") && m.contains("Detalle: yt-dlp: LL:"), "{m}");
        // (4) …salvo los 3 fallos con acción concreta: tal cual, sin doble prefijo.
        let m = classify_export_error("could not find brave cookies database in /x");
        assert!(m.contains("cookies.txt") && !m.contains("Sesión no válida"), "{m}");
        // (5) El jar sin marca cuenta como sesión; marcado (Conectar fallido), no.
        assert!(account().connected, "jar sin marca debe contar como sesión");
        mark_session_invalid();
        assert!(session_invalid() && !account().connected, "jar marcado no debe contar como sesión");
        clear_session_invalid();
        assert!(account().connected, "sin marca debe volver a contar como sesión");
        // (6) Verificación (L2): "valid" quita el marcador; "unverified" no lo toca.
        mark_session_invalid();
        assert!(account().unverified && !account().connected, "con marcador: sin verificar");
        apply_check_mark(CheckOutcome::Unverified);
        assert!(session_invalid(), "unverified no debe tocar el marcador");
        apply_check_mark(CheckOutcome::Valid);
        assert!(!session_invalid() && account().connected, "valid quita el marcador");
        // (7) L3b: el candidato del login integrado no toca el jar hasta validar;
        // al adoptarlo (validación OK) el jar pasa a ser el candidato.
        std::fs::write(tmp.join("ytmusic-cookies.txt"), "# viejo\n").unwrap();
        write_login_candidate("# Netscape HTTP Cookie File\nSAPISID=v\n").unwrap();
        assert!(tmp.join("ytmusic-cookies.login.tmp").is_file(), "candidato escrito");
        assert_eq!(
            std::fs::read_to_string(tmp.join("ytmusic-cookies.txt")).unwrap(),
            "# viejo\n",
            "el jar real no se toca hasta validar"
        );
        mark_session_invalid();
        adopt_login_candidate().unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.join("ytmusic-cookies.txt")).unwrap(),
            "# Netscape HTTP Cookie File\nSAPISID=v\n"
        );
        assert!(!session_invalid() && account().connected, "adoptar quita el marcador");
        assert_eq!(read_method().as_deref(), Some("Google (ventana integrada)"), "adoptar guarda el método");
        discard_login_candidate();
        discard_login_candidate();
        // (8) L4: el método se persiste en fichero 0600 junto al jar (manda
        // sobre el llavero: no se depende de keyutils) y los datos aislados de
        // la ventana se detectan para ofrecer su borrado.
        write_method("firefox:default");
        write_method("firefox:default"); // idempotente
        assert_eq!(read_method().as_deref(), Some("firefox:default"));
        assert_eq!(account().browser.as_deref(), Some("firefox:default"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let modo = std::fs::metadata(tmp.join("ytmusic-method.txt"))
                .unwrap()
                .permissions()
                .mode() & 0o777;
            assert_eq!(modo, 0o600, "método 0600");
        }
        assert!(!account().login_data, "sin datos aislados no hay oferta");
        std::fs::create_dir_all(tmp.join("ytmusic-login-webview")).unwrap();
        assert!(account().login_data, "detecta los datos de la ventana");
        std::fs::remove_dir_all(tmp.join("ytmusic-login-webview")).unwrap();
        assert!(!account().login_data);
        remove_method();
        assert!(read_method().is_none(), "desconectar olvida el método");
        std::env::remove_var("MUSICA_DATA_DIR");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn verificacion_clasifica_tres_resultados() {
        // Puro (sin red): los tres resultados de la verificación de sesión.
        assert_eq!(classify_check(true, ""), CheckOutcome::Valid);
        assert_eq!(
            classify_check(false, "yt-dlp: LL: YouTube said: The playlist does not exist."),
            CheckOutcome::Expired
        );
        assert_eq!(classify_check(false, "yt-dlp: Sign in to confirm your age"), CheckOutcome::Expired);
        assert_eq!(classify_check(false, "yt-dlp: HTTP Error 429: Too Many Requests"), CheckOutcome::Unverified);
        assert_eq!(classify_check(false, "yt-dlp: Sign in to confirm you're not a bot"), CheckOutcome::Unverified);
        assert_eq!(classify_check(false, "yt-dlp tardó demasiado (¿sin internet?)"), CheckOutcome::Unverified);
        assert_eq!(classify_check(false, "yt-dlp: Unable to download webpage"), CheckOutcome::Unverified);
    }

    #[test]
    fn login_detecta_sapisid_y_login_info_y_serializa_netscape() {
        let ck = |s: &str| -> Cookie<'static> { s.parse().unwrap() };
        // Dominios de sesión: con y sin punto, sin colar dominios parecidos.
        assert!(is_session_cookie_domain("youtube.com"));
        assert!(is_session_cookie_domain(".youtube.com"));
        assert!(is_session_cookie_domain("music.youtube.com"));
        assert!(is_session_cookie_domain(".google.com"));
        assert!(!is_session_cookie_domain("evil-youtube.com"));
        assert!(!is_session_cookie_domain("notyoutube.com"));
        // Login = SAPISID + LOGIN_INFO (las dos, en dominios de sesión).
        let sapisid = ck("SAPISID=s1; Domain=.youtube.com; Path=/; Secure; HttpOnly");
        let login = ck("LOGIN_INFO=l1; Domain=.youtube.com; Path=/; HttpOnly");
        assert!(!has_login_cookies(std::slice::from_ref(&sapisid)));
        assert!(!has_login_cookies(std::slice::from_ref(&login)));
        assert!(has_login_cookies(&[sapisid.clone(), login.clone()]));
        let ajena = ck("SAPISID=a; Domain=.example.com; Path=/");
        assert!(!has_login_cookies(&[ajena, login.clone()]));
        // Netscape: solo youtube.com/google.com, orden estable, sesión = 0,
        // TRUE con punto inicial, y `example.com` fuera.
        let google = ck("NID=g1; Domain=.google.com; Path=/; Secure");
        let otra = ck("otra=o1; Domain=.example.com; Path=/");
        let sub = ck("X=x1; Domain=music.youtube.com; Path=/");
        let exp = ck("SAPISID=s2; Domain=.youtube.com; Path=/; Expires=Wed, 01 Jan 2031 00:00:00 GMT");
        let out = to_netscape(&[login.clone(), google, otra, sub, exp.clone()]);
        assert!(out.starts_with("# Netscape HTTP Cookie File\n"), "{out}");
        assert!(out.contains(".youtube.com\tTRUE\t/\tFALSE\t0\tLOGIN_INFO\tl1"), "{out}");
        assert!(out.contains(".google.com\tTRUE\t/\tTRUE\t0\tNID\tg1"), "{out}");
        assert!(out.contains("music.youtube.com\tFALSE\t/\tFALSE\t0\tX\tx1"), "{out}");
        assert!(!out.contains("example.com"), "{out}");
        let ts = exp.expires_datetime().map(|d| d.unix_timestamp()).unwrap_or(0);
        assert!(ts > 1_900_000_000 && ts < 2_100_000_000, "Expires debería parsearse: {ts}");
        assert!(out.contains(&format!(".youtube.com\tTRUE\t/\tFALSE\t{ts}\tSAPISID\ts2")), "{out}");
    }

    #[test]
    fn probe_no_panica_sin_binario() {
        // Solo afirma forma; en CI puede o no estar instalado.
        let p = probe();
        assert_eq!(p.available, p.version.is_some());
    }

    /// Red real: importa 3 pistas de un mix público (requiere yt-dlp + red).
    #[tokio::test]
    #[ignore = "red: extrae de YouTube de verdad"]
    async fn importa_mix_real() {
        let pl = import_page(
            "https://music.youtube.com/watch?v=rFZHOHl-L8A&list=RDrFZHOHl-L8A",
            1,
            3,
        )
        .await
        .expect("el mix público debe listar");
        assert!(pl.tracks.len() >= 1);
        assert!(!pl.tracks[0].id.is_empty());
    }

    /// Red real: resuelve audio sin descargar (requiere yt-dlp + red).
    #[test]
    #[ignore = "red: resuelve URL de audio real"]
    fn resuelve_audio_real() {
        let url = resolve_audio_gen("ytmusic://rFZHOHl-L8A", None, SlotKind::User).expect("debe resolver audio");
        assert!(url.starts_with("https://"));
    }

    #[test]
    fn spec_nunca_envia_nombres_no_soportados() {        // BUG 1: "brave-nightly" como nombre fallaba; ahora es base + ruta.
        assert_eq!(
            yt_spec("brave", Some("/home/u/.config/BraveSoftware/Brave-Browser-Nightly/Default")).unwrap(),
            "brave:/home/u/.config/BraveSoftware/Brave-Browser-Nightly/Default"
        );
        assert_eq!(yt_spec("brave", None).unwrap(), "brave");
        assert_eq!(yt_spec("firefox", Some("abc123.default")).unwrap(), "firefox:abc123.default");
        assert!(yt_spec("brave-nightly", None).is_err());
        assert!(yt_spec("edge", None).is_ok());
        assert!(yt_spec("", None).is_err());
        assert!(yt_spec("internet-explorer", None).is_err());
    }

    #[test]
    fn clasifica_desafios_e_imposibles() {
        assert!(looks_like_challenge("ERROR: [youtube] H-X: The page needs to be reloaded"));
        assert!(looks_like_challenge("HTTP Error 429"));
        assert!(looks_like_challenge("confirm you're not a bot"));
        assert!(!looks_like_challenge("Video unavailable"));
        assert!(looks_like_gone("ERROR: Video unavailable"));
        assert!(looks_like_gone("This video is not available"));
        assert!(looks_like_gone("ERROR: [youtube] abc: This video is not available"));
        assert!(looks_like_gone("This video has been removed"));
        assert!(looks_like_gone("not available in your country"));
        assert!(!looks_like_gone("Sign in to confirm"));
        // "format not available" es transitorio (SABR), NO permanente.
        assert!(!looks_like_gone("Requested format not available"));
        assert!(looks_like_challenge("Requested format not available"));
        // Edad: motivo propio, no "sesión expirada" genérica.
        assert!(looks_like_age("Sign in to confirm your age"));
        assert!(!looks_like_age("Video unavailable"));
    }

    #[test]
    fn neg_edad_se_olvida_al_cambiar_sesion() {
        neg_store("edad999", AGE_MOTIVE);
        neg_store("muerto999", "yt-dlp: Video unavailable");
        assert!(neg_get("ytmusic://edad999").is_some());
        neg_clear_age();
        assert!(neg_get("ytmusic://edad999").is_none());
        // Las muertas reales se conservan.
        assert!(neg_get("ytmusic://muerto999").is_some());
        neg_clear("ytmusic://muerto999");
    }

    /// Vigencia: la URL de googlevideo declara su propio vencimiento
    /// (parámetro `expire=<unix>`); sin él se usa el TTL duro.
    #[test]
    fn parsea_expire_de_url_de_googlevideo() {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let future = format!("https://rr4---sn.example/videoplayback?expire={}&itag=251&ip=1.2.3.4", now + 3600);
        let rem = url_remaining_secs(&future).expect("debe leer expire");
        assert!((3580..=3600).contains(&rem), "{rem}");
        let past = format!("https://x/y?expire={}", now - 120);
        assert!(url_remaining_secs(&past).unwrap() < 0, "vencida da negativo");
        assert!(url_remaining_secs("https://x/y?itag=251&ratebypass=yes").is_none());
        assert!(url_remaining_secs("https://x/y").is_none());
        assert!(url_remaining_secs("").is_none());
    }

    #[test]
    fn cola_stderr_sin_urls_ni_firmas() {
        // El log de intentos lentos nunca lleva URLs completas ni IPs/firmas.
        let tail = last_stderr_lines(
            "WARNING: [youtube] abc: SABR-only experiment https://github.com/x/y/issues/1\nERROR: [youtube] abc: needs reload?expire=1&ip=1.2.3.4&sig=AAAA\n",
        );
        assert!(!tail.contains("ip="), "{tail}");
        assert!(!tail.contains("sig="), "{tail}");
        assert!(!tail.contains('?'), "{tail}");
        assert!(tail.len() <= 330, "{tail}");
        assert_eq!(last_stderr_lines("  \n \n"), "");
    }

    #[test]
    fn neg_marca_muertas_y_limpia_con_clic() {
        let dead = "ytmusic://muerto123";
        let live = "ytmusic://vivo456";
        neg_clear(dead);
        assert!(neg_get(dead).is_none());
        neg_store("muerto123", "yt-dlp: Video unavailable");
        assert!(neg_get(dead).unwrap().contains("unavailable"));
        // Por lotes para pintar filas sin resolver.
        assert_eq!(neg_motives(&[dead.to_string()]).len(), 1);
        assert!(neg_motives(&[dead.to_string()])[0].is_some());
        assert!(neg_motives(&[live.to_string()])[0].is_none());
        // El clic explícito olvida la marca y reintenta.
        neg_clear(dead);
        assert!(neg_get(dead).is_none());
        // Rutas no-YouTube nunca tienen marca.
        assert!(neg_get("/m/a.mp3").is_none());
    }

    #[test]
    fn detecta_navegadores_reales() {
        // En este equipo debe ver Brave Beta/Nightly con perfil Default.
        let found = detect_browsers();
        for b in &found {
            assert!(!b.label.is_empty() && !b.browser.is_empty());
            // El spec armado debe validar (nunca un nombre inventado).
            assert!(yt_spec(&b.browser, b.profile.as_deref()).is_ok());
        }
    }

    #[test]
    fn gen_vieja_aborta_sin_red() {
        // Sin tocar la red: una generación superada sale al instante.
        let t0 = Instant::now();
        let r = resolve_audio_gen("ytmusic://cualquiera", Some(current_gen() + 999), SlotKind::User);
        assert_eq!(r.unwrap_err(), STALE);
        assert!(t0.elapsed() < Duration::from_secs(2), "abortar no debe spawnear nada");
        assert_eq!(live_children(), 0);
    }

    /// Tormenta: 8 clics "simultáneos" con IDs distintos (red real).
    /// Solo el último sobrevive; al final no quedan hijos ni hilos colgados.
    #[test]
    #[ignore = "red: 8 resoluciones concurrentes"]
    fn tormenta_concurrente_no_acumula() {
        let ids = [
            "rFZHOHl-L8A", "y7qZFji19Rg", "lQTtojKRYKE", "IPg9DRpm9xQ",
            "JD-kMIpDfnY", "0er2m_JIBdc", "0QKQlf8r7ls", "1B-BW28iNsc",
        ];
        let t0 = Instant::now();
        let mut hs = vec![];
        for (n, id) in ids.iter().enumerate() {
            // Cada "clic" bumpea: solo el último (n==7) debe completar.
            let gen = bump_gen();
            let path = format!("ytmusic://{id}");
            hs.push(std::thread::spawn(move || (n, resolve_audio_gen(&path, Some(gen), SlotKind::User))));
        }
        let mut oks = 0;
        for h in hs {
            let (n, r) = h.join().unwrap();
            match r {
                Ok(_) => {
                    eprintln!("[tormenta {n}] completó");
                    oks += 1;
                }
                Err(e) if e == STALE => eprintln!("[tormenta {n}] abortada (bien)"),
                Err(e) => eprintln!("[tormenta {n}] falló: {e}"),
            }
        }
        let dt = t0.elapsed();
        eprintln!("[tormenta] {oks}/8 OK en {}ms, hijos vivos: {}", dt.as_millis(), live_children());
        assert_eq!(live_children(), 0, "sin procesos colgados");
        assert!(oks >= 1, "al menos la última debe completar");
        assert!(dt < Duration::from_secs(60), "la tormenta no debe alargarse");
    }

    #[cfg(unix)]
    fn salida_de(cmd: &str) -> std::process::Output {
        std::process::Command::new("sh")
            .args(["-c", cmd])
            .output()
            .unwrap()
    }

    #[test]
    fn probe_distingue_no_encontrado() {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let (ok, version, error) = classify_probe(Err(e), None);
        assert!(!ok);
        assert!(version.is_none());
        assert!(error.is_none(), "NotFound no debe fabricar un error");
    }

    #[test]
    fn probe_propaga_primera_linea_sin_home() {
        let o = salida_de("printf 'boom /home/u/x\\nsegunda\\n' >&2; exit 7");
        let (ok, version, error) = classify_probe(Ok(o), Some("/home/u"));
        assert!(!ok);
        assert!(version.is_none());
        assert_eq!(error.as_deref(), Some("boom <ruta>/x"));
    }

    #[test]
    fn probe_ok_lee_version() {
        let o = salida_de("printf '2026.08.19\\n'");
        let (ok, version, error) = classify_probe(Ok(o), None);
        assert!(ok);
        assert_eq!(version.as_deref(), Some("2026.08.19"));
        assert!(error.is_none());
    }
}
