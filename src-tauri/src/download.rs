//! Descargas de audio de YouTube Music vía yt-dlp.
//!
//! Este módulo es el ÚNICO punto del proyecto autorizado a descargar; todo lo
//! demás (reproducción, precarga, lotes, radios, Spotify) sigue siendo solo
//! streaming y no se toca.
//!
//! Decisión: subproceso yt-dlp propio con `Command` + `args([...])` (nunca
//! `sh -c`, nunca texto del usuario al shell). Si hay una sesión de YouTube
//! válida, se pasa su `cookies.txt` a yt-dlp: YouTube exige login para muchos
//! vídeos («Sign in to confirm you're not a bot») y sin sesión la descarga
//! nunca funcionaría por mucho que se espere. Sin sesión, se intenta de forma
//! anónima y el error se explica en la interfaz. La plantilla de salida usa un
//! token temporal que creamos nosotros (`.musica-dl-<token>.%(ext)s`) y el
//! nombre final "Artista - Título.ext" lo decidimos DESPUÉS con `rename`, así
//! el nombre está saneado y jamás se sobrescribe un archivo del usuario.
//!
//! - Cupo propio de 2 (D3) separado del cupo de reproducción: las descargas
//!   nunca le quitan turno a la reproducción.
//! - Avisos/comportamiento compartidos solo por lectura: `neg_get` (pistas
//!   muertas) y `throttle_active` (429). Las descargas NO escriben en la
//!   caché negativa ni en la de URLs para no alterar su comportamiento.

use crate::tools;
use crate::ytmusic;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Tope de caracteres del nombre base (deja margen para " (99)" y la extensión).
const MAX_BASE: usize = 150;
/// Tope por pista: una descarga normal tarda 1-3 min; 10 min es un colgado.
const TRACK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Separación entre arranques PROPIOS: nunca frena a la reproducción.
const SPAWN_GAP: Duration = Duration::from_millis(150);
/// Sondeo del hijo (mismo ritmo que el resto del proyecto).
const POLL: Duration = Duration::from_millis(100);
/// Prefijo de nuestros temporales: solo tocamos archivos que creamos nosotros.
const TEMP_PREFIX: &str = ".musica-dl-";
/// Descargas simultáneas: máximo 2 con cupo PROPIO. Nunca le quitan turno a
/// la reproducción (sus cupos viven en ytmusic y no se tocan).
const MAX_PARALLEL: usize = 2;
/// Resumen de cola cada 5 s en descargas largas (sin URLs ni rutas).
const LOG_EVERY: Duration = Duration::from_secs(5);
/// Nombre cuando no queda nada tras sanear.
const FALLBACK: &str = "Sin título";

// ---------- tipos IPC ----------

/// Una pista a descargar (id de vídeo plano, sin URL ni cookies).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItem {
    pub id: String,
    pub title: String,
    pub artist: String,
}

/// Opciones ya resueltas por la interfaz (la carpeta elige el usuario).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadOptions {
    /// Carpeta destino (modo "por defecto" o la elegida en "preguntar cada vez").
    pub folder: String,
    /// "original" (por defecto, sin -x: sale el formato del proveedor, p. ej.
    /// webm/opus), "m4a" (AAC con ffmpeg) o "mp3" (AAC 320k con ffmpeg).
    #[serde(default)]
    pub format: String,
    /// Incrustar portada y etiquetas (mp3/m4a con ffmpeg; en "original" solo
    /// etiquetas y también requiere ffmpeg).
    #[serde(default)]
    pub embed: bool,
    /// Prefijo numérico "01 - " (solo en descargas de playlist).
    #[serde(default)]
    pub numeric_prefix: bool,
    /// Subcarpeta de playlist dentro de la destino.
    #[serde(default)]
    pub playlist_subdir: Option<String>,
}

impl DownloadOptions {
    /// Formato efectivo: mp3 o m4a solo si el usuario lo pidió Y hay ffmpeg.
    /// Sin ffmpeg (o en "original") la app no falla: se guarda el original.
    pub fn effective_format(&self, ffmpeg: bool) -> &'static str {
        let f = self.format.trim().to_ascii_lowercase();
        if ffmpeg && f == "mp3" {
            "mp3"
        } else if ffmpeg && f == "m4a" {
            "m4a"
        } else {
            "original"
        }
    }
}

/// Fallo de una pista para el resumen (motivo corto, visible en la interfaz).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub title: String,
    pub motivo: String,
}

/// Instantánea para la UI (poll, igual que `youtube_batch_progress`).
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub active: bool,
    pub total: usize,
    /// Procesadas (completadas + fallidas).
    pub finished: usize,
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Título de la pista en curso (para "pista actual").
    pub current: Option<String>,
    /// Resumen final, p. ej. "48 descargadas, 2 omitidas, 1 fallida".
    pub summary: Option<String>,
    /// Carpeta destino (para el botón "Abrir carpeta"; no sale en logs).
    pub folder: Option<String>,
    pub failures: Vec<Failure>,
    /// Aviso discreto (p. ej. límite de YouTube pausando la cola).
    pub notice: Option<String>,
}

// ---------- utilidades puras ----------

/// ¿Hay ffmpeg? (resuelto por tools: sidecar → recursos → PATH). Una vez.
pub fn has_ffmpeg() -> bool {
    static HAS: OnceLock<bool> = OnceLock::new();
    *HAS.get_or_init(|| {
        tools::command("ffmpeg")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Sanea un fragmento para usarlo en un nombre de archivo (puro).
/// Quita `/ \ : * ? " < > |`, controles, "..", puntos colgantes y espacios
/// repetidos; limita la longitud y evita nombres reservados de Windows.
pub fn sanitize_fragment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_space = true; // no empezar con espacio
    for ch in raw.chars() {
        let bad = matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            || (ch as u32) < 0x20
            || ch == '\u{7f}';
        if bad || ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
            continue;
        }
        out.push(ch);
        prev_space = false;
    }
    // Ninguna variante de ".." sobrevive (nada de rutas relativas).
    while out.contains("..") {
        out = out.replace("..", ".");
    }
    let mut s: String = out
        .trim_matches(|c: char| c.is_whitespace() || c == '.')
        .chars()
        .take(MAX_BASE)
        .collect();
    while s.contains("..") {
        s = s.replace("..", ".");
    }
    let s = s.trim_matches(|c: char| c.is_whitespace() || c == '.').to_string();
    if s.is_empty() {
        return FALLBACK.to_string();
    }
    if is_reserved_windows(&s) {
        return format!("_{s}");
    }
    s
}

/// Nombres que Windows rechaza como archivo (puro).
fn is_reserved_windows(s: &str) -> bool {
    let head = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    ["CON", "PRN", "AUX", "NUL"].contains(&head.as_str())
        || (head.len() == 4
            && (head.starts_with("COM") || head.starts_with("LPT"))
            && matches!(head.as_bytes()[3], b'1'..=b'9'))
}

/// Nombre base "Artista - Título" (con prefijo opcional), saneado (puro).
pub fn base_name(artist: &str, title: &str, prefix: Option<&str>) -> String {
    // Artista vacío (o hecho solo de puntos/espacios) = sin artista, no FALLBACK.
    let sin_artista = artist
        .trim_matches(|c: char| c.is_whitespace() || c == '.')
        .is_empty();
    let a = if sin_artista { String::new() } else { sanitize_fragment(artist) };
    let t = sanitize_fragment(title);
    let mut core = if a.is_empty() || a.eq_ignore_ascii_case(&t) {
        t
    } else {
        let mut s = String::with_capacity(a.len() + t.len() + 3);
        s.push_str(&a);
        s.push_str(" - ");
        s.push_str(&t);
        s
    };
    if core.is_empty() {
        core = FALLBACK.to_string();
    }
    let mut base = match prefix.map(sanitize_fragment) {
        Some(p) if !p.is_empty() && p != FALLBACK => format!("{p} - {core}"),
        _ => core,
    };
    if base.chars().count() > MAX_BASE {
        base = base.chars().take(MAX_BASE).collect();
        base = base.trim_matches(|c: char| c.is_whitespace() || c == '.').to_string();
    }
    if base.is_empty() {
        FALLBACK.to_string()
    } else {
        base
    }
}

/// Ruta final NUNCA solapada: "base.ext", luego "base (2).ext", "base (3)"…
pub fn unique_path(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let ext = ext.trim_start_matches('.');
    let first = dir.join(format!("{base}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..10_000u32 {
        let p = dir.join(format!("{base} ({n}).{ext}"));
        if !p.exists() {
            return p;
        }
    }
    // Nunca sobrescribir, aunque la serie se agote.
    dir.join(format!("{base} ({})", u32::try_from(Instant::now().elapsed().as_secs()).unwrap_or(0)))
}

/// ¿La ruta queda dentro de `dir`? Rechaza cualquier ".." (anti path-traversal).
pub fn is_inside(dir: &Path, p: &Path) -> bool {
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    if dir.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    p.starts_with(dir)
}

/// URL de un vídeo con id validado (lista blanca: nada raro llega al proceso).
pub fn video_url(id: &str) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 24
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("id de vídeo inválido".into());
    }
    Ok(format!("https://www.youtube.com/watch?v={id}"))
}

/// Carpeta final: destino + subcarpeta de playlist (saneada).
fn target_dir(opts: &DownloadOptions) -> PathBuf {
    let base = PathBuf::from(opts.folder.trim());
    match opts.playlist_subdir.as_deref().map(sanitize_fragment) {
        Some(sub) if !sub.is_empty() && sub != FALLBACK => base.join(sub),
        _ => base,
    }
}

/// Argumentos completos de yt-dlp para UNA pista (puro, testeable).
/// Un proceso por pista, `--no-playlist`, sin cookies y con plantilla propia.
pub fn build_args(url: &str, dir: &Path, token: &str, opts: &DownloadOptions, ffmpeg: bool) -> Vec<String> {
    let mut a: Vec<String> = ["--no-warnings", "--no-progress", "--socket-timeout", "15", "--no-playlist"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    // Formato de salida:
    // - m4a (con ffmpeg): mejor fuente AAC si existe (remux, sin recodificar);
    //   si no, recodifica a AAC. Etiquetas y portada cuando se pidan.
    // - mp3 (con ffmpeg): recodifica a mp3 320k + portada y etiquetas.
    // - original (por defecto, o sin ffmpeg): se guarda tal cual el proveedor
    //   entrega (casi siempre webm/opus). NUNCA --embed-thumbnail aquí: los
    //   contenedores de vídeo de yt-dlp lo rechazan y abortarían la descarga;
    //   --embed-metadata solo si hay ffmpeg (es un postproceso suyo).
    match opts.effective_format(ffmpeg) {
        "mp3" | "m4a" => {
            let fmt = opts.effective_format(ffmpeg);
            a.push("-f".into());
            a.push(if fmt == "m4a" {
                "bestaudio[ext=m4a]/bestaudio".into()
            } else {
                "bestaudio/best".into()
            });
            a.push("-x".into());
            a.push("--audio-format".into());
            a.push(fmt.to_string());
            if opts.embed {
                a.push("--embed-thumbnail".into());
                a.push("--embed-metadata".into());
            }
        }
        _ => {
            a.push("-f".into());
            a.push("bestaudio/best".into());
            if opts.embed && ffmpeg {
                a.push("--embed-metadata".into());
            }
        }
    }
    a.push("-o".into());
    a.push(
        dir.join(format!("{TEMP_PREFIX}{token}.%(ext)s"))
            .to_string_lossy()
            .into_owned(),
    );
    a.push(url.to_string());
    a
}

/// Mensaje apto para el log: sin query ni hash (nunca URLs completas) y corto.
fn log_safe(msg: &str) -> String {
    let cut = msg.split(['?', '#']).next().unwrap_or(msg);
    let t = cut.trim();
    if t.chars().count() > 120 {
        t.chars().take(120).collect()
    } else {
        t.to_string()
    }
}

/// Clasificación de un fallo de descarga (pura sobre el mensaje clasificado).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailKind {
    /// Pista muerta o restringida: sin reintento (no se saltan restricciones).
    Dead,
    /// YouTube exige sesión («confirma que no eres un bot»): sin reintento
    /// ciego; el usuario debe conectar/renovar la cuenta.
    Auth,
    /// Límite de YouTube (429): pausa la cola y reintenta.
    Throttle,
    /// Transitorio (red, timeout): UN reintento y si falla, fallida.
    Transient,
}

impl FailKind {
    pub fn retryable(self) -> bool {
        matches!(self, FailKind::Throttle | FailKind::Transient)
    }
}

/// ¿Qué clase de fallo es este mensaje? (puro; sobre el ya clasificado).
pub fn fail_kind(msg: &str) -> FailKind {
    if ytmusic::looks_like_gone(msg) {
        FailKind::Dead
    } else if ytmusic::looks_like_auth(msg) || msg.to_lowercase().contains("not a bot") {
        // YouTube pide login/confirmación: esperar 5 min no lo arregla.
        FailKind::Auth
    } else if ytmusic::looks_like_throttle(msg) {
        FailKind::Throttle
    } else {
        FailKind::Transient
    }
}

// ---------- hijos yt-dlp propios ----------

/// Hijos de DESCARGA (registry propio: no comparte nada con los de
/// reproducción, salvo que `kill_all` se llama al cerrar la ventana).
static CHILDREN: OnceLock<Mutex<Vec<Option<Child>>>> = OnceLock::new();

fn children() -> &'static Mutex<Vec<Option<Child>>> {
    CHILDREN.get_or_init(|| Mutex::new(Vec::new()))
}

/// Registra un hijo y devuelve su slot (barre terminados antes de añadir).
fn register(child: Child) -> usize {
    let Ok(mut c) = children().lock() else {
        return usize::MAX;
    };
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

fn unregister(slot: usize) {
    if let Ok(mut c) = children().lock() {
        if slot < c.len() {
            c[slot] = None;
        }
    }
}

fn kill_slot(slot: usize) {
    if let Ok(mut c) = children().lock() {
        if slot < c.len() {
            if let Some(mut ch) = c[slot].take() {
                tools::kill_child(&mut ch); // árbol Windows vía taskkill /T (W3: tools.rs)
            }
        }
    }
}

/// Corta todos los hijos de descarga (cancelar o cerrar la app: sin colgados).
/// También marca la cancelación para que la cola corte lo que quede pendiente.
pub fn kill_all() {
    CANCEL.store(true, Ordering::SeqCst);
    if let Ok(mut c) = children().lock() {
        for slot in c.iter_mut() {
            if let Some(mut ch) = slot.take() {
                tools::kill_child(&mut ch); // árbol Windows vía taskkill /T (W3: tools.rs)
            }
        }
    }
}

/// Salida del hijo tras salir (mismo patrón que ytmusic: se lee al terminar).
fn take_output(slot: usize) -> (String, String) {
    use std::io::Read;
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Ok(mut c) = children().lock() {
        if let Some(ch) = c.get_mut(slot).and_then(|s| s.as_mut()) {
            if let Some(mut o) = ch.stdout.take() {
                o.read_to_string(&mut stdout).ok();
            }
            if let Some(mut e) = ch.stderr.take() {
                e.read_to_string(&mut stderr).ok();
            }
        }
    }
    (stdout, stderr)
}

/// Separación mínima entre arranques PROPIOS (estático propio: la de
/// reproducción no se usa para no acoplar sus tiempos de arranque).
static LAST_SPAWN: Mutex<Option<Instant>> = Mutex::new(None);

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

// ---------- cola ----------

/// Trabajo pendiente: pista + prefijo numérico (solo playlist, D3).
struct Job {
    item: DownloadItem,
    prefix: Option<String>,
}

static QUEUE: OnceLock<Mutex<VecDeque<Job>>> = OnceLock::new();
static SNAP: OnceLock<Mutex<Snapshot>> = OnceLock::new();
static RUNNING: AtomicBool = AtomicBool::new(false);
/// Cancelación: el usuario pulsó "Cancelar" (o se está cerrando la app).
static CANCEL: AtomicBool = AtomicBool::new(false);
static TOKEN: AtomicU64 = AtomicU64::new(0);
/// Último resumen de cola impreso (máx. uno cada LOG_EVERY).
static LAST_LOG: Mutex<Option<Instant>> = Mutex::new(None);
/// Handle vivo para el aviso ÚNICO de límite (evento youtube://throttled).
static APP: OnceLock<Mutex<Option<tauri::AppHandle>>> = OnceLock::new();

fn handle() -> &'static Mutex<Option<tauri::AppHandle>> {
    APP.get_or_init(|| Mutex::new(None))
}

fn queue() -> &'static Mutex<VecDeque<Job>> {
    QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn snap() -> &'static Mutex<Snapshot> {
    SNAP.get_or_init(|| Mutex::new(Snapshot::default()))
}

fn with_snap<T>(f: impl FnOnce(&mut Snapshot) -> T) -> T {
    let mut g = snap().lock().unwrap_or_else(|e| e.into_inner());
    f(&mut g)
}

fn next_token() -> String {
    let n = TOKEN.fetch_add(1, Ordering::SeqCst) + 1;
    format!("{}-{n}", std::process::id())
}

/// Instantánea actual (para `download_progress` y el propio módulo).
pub fn snapshot() -> Snapshot {
    with_snap(|s| s.clone())
}

/// Aviso ÚNICO de límite: reutiliza el evento ya existente `youtube://throttled`
/// (el mismo que escucha la interfaz para la reproducción) y marca el panel.
fn note_limit_once() {
    if !ytmusic::take_throttle_notice() {
        return; // ya se avisó en este episodio (producción o descargas)
    }
    with_snap(|s| {
        if s.notice.is_none() {
            s.notice = Some("YouTube limitó las peticiones: descargas en pausa unos minutos".into());
        }
    });
    let handle_ref = handle().lock().ok().and_then(|g| g.clone());
    if let Some(h) = handle_ref {
        use tauri::Emitter;
        h.emit("youtube://throttled", ()).ok();
    }
}

/// Pausa la cola de descargas mientras dure el backoff de límite (429).
/// Devuelve false si el usuario canceló mientras esperaba.
fn wait_throttle() -> bool {
    while ytmusic::throttle_active() {
        if CANCEL.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    true
}

// ---------- ejecución de una pista ----------

enum Outcome {
    Ok,
    /// No se intentó (cancelación u otra causa): cuenta como omitida.
    Skip(String),
    Err(String),
}

/// Archura temporal creada por yt-dlp para este token (ignora .part/.ytdl y
/// miniaturas transitorias que el post-procesador ya se encargará de borrar).
fn temp_file(dir: &Path, token: &str) -> Option<PathBuf> {
    let prefix = format!("{TEMP_PREFIX}{token}");
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&prefix) {
            continue;
        }
        if name.ends_with(".part") || name.ends_with(".ytdl") || name.ends_with(".temp") {
            continue;
        }
        let p = e.path();
        let es_imagen = matches!(
            p.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref(),
            Some("webp" | "jpg" | "jpeg" | "png")
        );
        if p.is_file() && !es_imagen {
            return Some(p);
        }
    }
    None
}

/// Borra SOLO nuestros temporales de ESTA pista (`.musica-dl-<token>.*`:
/// .part, .ytdl y restos sin renombrar). Nunca un nombre final ni ajeno.
fn cleanup_temp(dir: &Path, token: &str, id: &str) {
    let prefix = format!("{TEMP_PREFIX}{token}");
    let mut n = 0;
    for e in std::fs::read_dir(dir).ok().into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        eprintln!("[descarga] id={id} parciales_borrados={n}");
    }
}

/// Descarga UNA pista: valida, lanza yt-dlp, espera y renombra sin sobrescribir.
fn download_one(job: &Job, opts: &DownloadOptions) -> Outcome {
    let dir = target_dir(opts);
    let token = next_token();
    let out = download_inner(job, opts, &dir, &token);
    // Éxito: solo podrían quedar miniaturas sobrantes; fallo/cancelación:
    // también .part. Siempre son temporales NUESTROS de este token.
    cleanup_temp(&dir, &token, job.item.id.trim());
    out
}

fn download_inner(job: &Job, opts: &DownloadOptions, dir: &Path, token: &str) -> Outcome {
    let id = job.item.id.trim().to_string();
    eprintln!("[descarga] id={id} iniciada");
    // Pista muerta conocida: sin yt-dlp, sin reintento, con su motivo.
    if let Some(m) = ytmusic::neg_get(&format!("ytmusic://{id}")) {
        let motivo = log_safe(&m);
        eprintln!("[descarga] id={id} fallida motivo={motivo}");
        return Outcome::Err(m);
    }
    let url = match video_url(&id) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("[descarga] id={id} fallida motivo=id invalido");
            return Outcome::Err(e);
        }
    };
    if !dir.is_dir() {
        eprintln!("[descarga] id={id} fallida motivo=carpeta inexistente");
        return Outcome::Err("la carpeta de destino no existe".into());
    }
    let base = base_name(&job.item.artist, &job.item.title, job.prefix.as_deref());
    let mut args = build_args(&url, dir, token, opts, has_ffmpeg());
    // Sesión válida → yt-dlp puede autenticarse y evitar el bot-check.
    let cookies = ytmusic::session_cookie_path();
    if let Some(c) = &cookies {
        args.push("--cookies".into());
        args.push(c.to_string_lossy().into_owned());
        eprintln!("[descarga] id={id} cookies=si");
    } else {
        eprintln!("[descarga] id={id} cookies=no");
    }

    let mut cmd = tools::command("yt-dlp");
    cmd.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
    spawn_gap();
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[descarga] id={id} fallida motivo=no se pudo ejecutar yt-dlp");
            return Outcome::Err(format!("no se pudo ejecutar yt-dlp: {e}"));
        }
    };
    let slot = register(child);

    let deadline = Instant::now() + TRACK_TIMEOUT;
    let status = loop {
        enum Obs {
            Done(std::process::ExitStatus),
            Running,
            Gone,
        }
        let obs = match children().lock() {
            Ok(mut c) => match c.get_mut(slot).and_then(|s| s.as_mut()) {
                Some(ch) => match ch.try_wait() {
                    Ok(Some(st)) => Obs::Done(st),
                    _ => Obs::Running,
                },
                None => Obs::Gone,
            },
            Err(_) => Obs::Gone,
        };
        match obs {
            Obs::Done(st) => break st,
            Obs::Gone => {
                // Alguien mató al hijo (cierre de la app / cancelación).
                return Outcome::Skip("cancelada".into());
            }
            Obs::Running => {
                // Cancelación explícita: corta el hijo y no deja .part (wrapper).
                if CANCEL.load(Ordering::SeqCst) {
                    kill_slot(slot);
                    unregister(slot);
                    eprintln!("[descarga] id={id} omitida motivo=cancelada");
                    return Outcome::Skip("cancelada".into());
                }
                if Instant::now() >= deadline {
                    kill_slot(slot);
                    unregister(slot);
                    eprintln!("[descarga] id={id} fallida motivo=tiempo agotado");
                    return Outcome::Err("la descarga tardó demasiado".into());
                }
                std::thread::sleep(POLL);
            }
        }
    };
    let (_stdout, stderr) = take_output(slot);
    unregister(slot);

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        let msg = ytmusic::classify_stderr(&stderr);
        // Sin escribir en la caché negativa (eso es comportamiento de playback).
        eprintln!("[descarga] id={id} fallida codigo={code} motivo={}", log_safe(&msg));
        return Outcome::Err(msg);
    }

    // Éxito: localizar NUESTRO temporal y darle el nombre definitivo.
    let Some(tmp) = temp_file(dir, token) else {
        eprintln!("[descarga] id={id} fallida motivo=sin archivo resultante");
        return Outcome::Err("yt-dlp no produjo ningún archivo".into());
    };
    let ext = tmp
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("m4a")
        .to_string();
    let final_path = unique_path(dir, &base, &ext);
    if !is_inside(dir, &final_path) {
        eprintln!("[descarga] id={id} fallida motivo=ruta fuera de la carpeta");
        return Outcome::Err("la ruta final quedaría fuera de la carpeta destino".into());
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        eprintln!("[descarga] id={id} fallida motivo=no se pudo nombrar el archivo");
        return Outcome::Err(format!("no se pudo nombrar el archivo: {e}"));
    }
    let name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    eprintln!("[descarga] id={id} completada archivo={name}");
    Outcome::Ok
}

// ---------- supervisor ----------

/// Contadores en un solo sitio: `finished` siempre cuadra con el reparto.
fn apply_outcome(s: &mut Snapshot, out: &Outcome, title: &str) {
    match out {
        Outcome::Ok => s.downloaded += 1,
        Outcome::Skip(m) => {
            s.skipped += 1;
            if s.notice.is_none() && m == "cancelada" {
                s.notice = Some("Descarga cancelada".into());
            }
        }
        Outcome::Err(m) => {
            s.failed += 1;
            s.failures.push(Failure { title: title.to_string(), motivo: m.clone() });
        }
    }
    s.finished += 1;
    s.current = None;
}

/// Cancelación: lo que quede en la cola pasa a "omitida" sin intentarse.
fn drain_pending() -> usize {
    let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
    let n = q.len();
    q.clear();
    if n > 0 {
        with_snap(|s| s.skipped += n);
    }
    n
}

/// Resumen de cola cada 5 s en descargas largas (sin URLs ni rutas).
fn progress_log() {
    let mut g = LAST_LOG.lock().unwrap_or_else(|e| e.into_inner());
    if g.map(|t| t.elapsed() < LOG_EVERY).unwrap_or(false) {
        return;
    }
    *g = Some(Instant::now());
    drop(g);
    let s = snapshot();
    eprintln!(
        "[descarga] cola {}/{} descargadas={} fallidas={} omitidas={}",
        s.finished, s.total, s.downloaded, s.failed, s.skipped
    );
}

fn worker(opts: Arc<DownloadOptions>) {
    loop {
        // Cancelar corta lo que aún no ha empezado; lo en curso ya se mató.
        if CANCEL.load(Ordering::SeqCst) {
            drain_pending();
            return;
        }
        let job = { queue().lock().unwrap_or_else(|e| e.into_inner()).pop_front() };
        let Some(job) = job else { return };
        with_snap(|s| s.current = Some(job.item.title.clone()));
        // Límite de YouTube: pausa NUESTRA cola (el cupo de reproducción no se toca).
        if ytmusic::throttle_active() && !wait_throttle() {
            let t = job.item.title.clone();
            with_snap(|s| apply_outcome(s, &Outcome::Skip("cancelada".into()), &t));
            continue;
        }
        let mut out = download_one(&job, &opts);
        let kind = match &out {
            Outcome::Err(m) => {
                let k = fail_kind(m);
                match k {
                    FailKind::Auth => {
                        // El mensaje de yt-dlp habla de cookies/bot; se traduce a
                        // una acción clara y NO se activa el backoff de 429.
                        out = Outcome::Err(if ytmusic::session_cookie_path().is_some() {
                            "YouTube rechazó la sesión para descargar: vuelve a conectar tu cuenta en Ajustes".into()
                        } else {
                            "YouTube pide iniciar sesión para descargar: conecta tu cuenta en Ajustes".into()
                        });
                    }
                    FailKind::Throttle => {
                        // Reutiliza el backoff existente y su aviso único.
                        ytmusic::note_throttle();
                        note_limit_once();
                    }
                    _ => {}
                }
                Some(k)
            }
            _ => None,
        };
        match kind {
            // Un solo reintento para lo transitorio; lo muerto jamás.
            Some(k) if k.retryable() => {
                let sigue = if k == FailKind::Throttle { wait_throttle() } else { true };
                if sigue && !CANCEL.load(Ordering::SeqCst) {
                    eprintln!("[descarga] id={} reintento=1", job.item.id.trim());
                    out = download_one(&job, &opts);
                }
            }
            Some(_) => eprintln!("[descarga] id={} sin_reintento=permanente", job.item.id.trim()),
            None => {}
        }
        let title = job.item.title.clone();
        with_snap(|s| apply_outcome(s, &out, &title));
        progress_log();
    }
}

fn supervisor(opts: Arc<DownloadOptions>) {
    let mut hs = Vec::with_capacity(MAX_PARALLEL);
    for i in 0..MAX_PARALLEL {
        let o = opts.clone();
        hs.push(
            std::thread::Builder::new()
                .name(format!("musica-dl-{i}"))
                .spawn(move || worker(o))
                .ok(),
        );
    }
    for h in hs.into_iter().flatten() {
        h.join().ok();
    }
    with_snap(|s| {
        s.active = false;
        s.current = None;
        s.summary = Some(summary_text(s.downloaded, s.skipped, s.failed));
    });
    RUNNING.store(false, Ordering::SeqCst);
    let s = snapshot();
    eprintln!("[descarga] resumen {}", s.summary.unwrap_or_default());
}

/// "48 descargadas, 2 omitidas, 1 fallida" (solo partes > 0; puro).
pub fn summary_text(downloaded: usize, skipped: usize, failed: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    if downloaded > 0 {
        parts.push(format!("{downloaded} descargada{}", if downloaded == 1 { "" } else { "s" }));
    }
    if skipped > 0 {
        parts.push(format!("{skipped} omitida{}", if skipped == 1 { "" } else { "s" }));
    }
    if failed > 0 {
        parts.push(format!("{failed} fallida{}", if failed == 1 { "" } else { "s" }));
    }
    if parts.is_empty() {
        "0 descargadas".into()
    } else {
        parts.join(", ")
    }
}

// ---------- comandos Tauri ----------

/// Prefijo numérico de playlist: "01", "02"… (puro).
pub fn prefix_for(index: usize) -> String {
    format!("{:02}", index + 1)
}

/// «No instalado» solo cuando el spawn dio NotFound (error=None); si el
/// binario existe pero falla, se muestra su primera línea real.
fn mensaje_ytdlp(p: &ytmusic::Probe) -> String {
    match &p.error {
        Some(e) => format!("yt-dlp falló: {e}"),
        None => "yt-dlp no está instalado. Instálalo con: sudo pacman -S yt-dlp".into(),
    }
}

/// Lanza la cola en segundo plano (común a pistas sueltas y playlists):
/// valida primero y jamás bloquea la interfaz ni la reproducción.
fn start_jobs(app: tauri::AppHandle, jobs: Vec<Job>, opts: DownloadOptions) -> Result<Snapshot, String> {
    if jobs.is_empty() {
        return Err("no hay pistas para descargar".into());
    }
    let probe = ytmusic::probe();
    if !probe.available {
        return Err(mensaje_ytdlp(&probe));
    }
    let dir = target_dir(&opts);
    if opts.playlist_subdir.is_some() {
        // Playlist: carpeta nueva dentro del destino (nombre ya saneado).
        std::fs::create_dir_all(&dir).map_err(|e| format!("no se pudo crear la carpeta de la playlist: {e}"))?;
    }
    if !dir.is_dir() {
        return Err("la carpeta de destino no existe".into());
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("ya hay una descarga en curso".into());
    }
    CANCEL.store(false, Ordering::SeqCst);
    if let Ok(mut h) = handle().lock() {
        *h = Some(app);
    }
    with_snap(|s| {
        *s = Snapshot {
            active: true,
            total: jobs.len(),
            folder: Some(dir.to_string_lossy().into_owned()),
            ..Snapshot::default()
        };
    });
    {
        let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
        q.clear();
        q.extend(jobs);
    }
    let o = Arc::new(opts);
    if std::thread::Builder::new().name("musica-dl-sup".into()).spawn(move || supervisor(o)).is_err() {
        RUNNING.store(false, Ordering::SeqCst);
        with_snap(|s| *s = Snapshot::default());
        return Err("no se pudo lanzar la cola de descargas".into());
    }
    Ok(snapshot())
}

/// Descarga UNA pista (o varias sueltas), sin prefijo ni subcarpeta.
#[tauri::command]
pub fn download_start(app: tauri::AppHandle, items: Vec<DownloadItem>, opts: DownloadOptions) -> Result<Snapshot, String> {
    let jobs = items.into_iter().map(|item| Job { item, prefix: None }).collect();
    start_jobs(app, jobs, opts)
}

/// Descarga TODA la playlist abierta: usa la lista plana COMPLETA (no solo el
/// lote visible) sin resolver ninguna URL de audio. Crea su subcarpeta.
///
/// S4.2c: `items`/`title` permiten descargar una lista guardada sin URL de
/// origen (p. ej. creada desde CSV) con ESTE mismo comando y el mismo
/// comportamiento (subcarpeta + prefijo). Los elementos ya vienen resueltos y
/// el título da nombre a la subcarpeta; el resto del flujo no cambia.
#[tauri::command]
pub async fn download_start_playlist(
    app: tauri::AppHandle,
    url: String,
    opts: DownloadOptions,
    items: Option<Vec<DownloadItem>>,
    title: Option<String>,
) -> Result<Snapshot, String> {
    let (title, items): (String, Vec<DownloadItem>) = if let Some(items) = items {
        (title.unwrap_or_else(|| "Playlist".into()), items)
    } else {
        match crate::batch::session_full() {
            Some((t, tracks)) => (
                t,
                tracks
                    .into_iter()
                    .map(|tr| DownloadItem {
                        id: tr.path.strip_prefix("ytmusic://").unwrap_or(&tr.path).to_string(),
                        title: tr.title,
                        artist: tr.artist,
                    })
                    .collect(),
            ),
            None => {
                // Sin sesión abierta: una única petición plana (cero resolución de audio).
                let pl = ytmusic::import_page(&url, 1, crate::batch::FLAT_CAP)
                    .await
                    .map_err(|e| format!("no se pudo leer la playlist: {e}"))?;
                (
                    pl.title,
                    pl.tracks
                        .into_iter()
                        .map(|t| DownloadItem { id: t.id, title: t.title, artist: t.artist })
                        .collect(),
                )
            }
        }
    };
    if items.is_empty() {
        return Err("la playlist no tiene pistas".into());
    }
    let con_prefijo = opts.numeric_prefix;
    let jobs: Vec<Job> = items
        .into_iter()
        .enumerate()
        .map(|(i, item)| Job { item, prefix: con_prefijo.then(|| prefix_for(i)) })
        .collect();
    let mut opts = opts;
    // Requisito: carpeta nueva dentro del destino con el nombre saneado.
    opts.playlist_subdir = Some(sanitize_fragment(&title));
    start_jobs(app, jobs, opts)
}

/// Cancela la descarga en curso: corta los yt-dlp, no deja .part ni colgados.
#[tauri::command]
pub fn download_cancel() -> Snapshot {
    if !RUNNING.load(Ordering::SeqCst) {
        return snapshot();
    }
    kill_all(); // marca CANCEL + mata los hijos en curso
    let pendientes = drain_pending();
    with_snap(|s| {
        if s.notice.is_none() {
            s.notice = Some("Descarga cancelada".into());
        }
    });
    eprintln!("[descarga] cancelada pendientes={pendientes}");
    snapshot()
}

/// Entorno de descarga para Ajustes: con ffmpeg hay mp3/portada; sin él la
/// app NO falla, solo deshabilita esas opciones con la nota de instalación.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadEnv {
    pub ffmpeg: bool,
    pub ffmpeg_hint: String,
}

#[tauri::command]
pub fn download_env() -> DownloadEnv {
    DownloadEnv {
        ffmpeg: has_ffmpeg(),
        ffmpeg_hint: "Para activar mp3 y portada instala ffmpeg: sudo pacman -S ffmpeg".into(),
    }
}

/// Instantánea de la cola de descargas (poll de la interfaz).
#[tauri::command]
pub fn download_progress() -> Snapshot {
    snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("musica-dl-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn opts(folder: &str, format: &str, embed: bool) -> DownloadOptions {
        DownloadOptions {
            folder: folder.into(),
            format: format.into(),
            embed,
            numeric_prefix: false,
            playlist_subdir: None,
        }
    }

    #[test]
    fn sanea_caracteres_prohibidos_y_puntos() {
        let s = sanitize_fragment(r#"a/b\c:d*e?f"g<h>i|j .. .."#);
        for c in ['/', '\\', ':', '*', '?', '"', '<', '>', '|'] {
            assert!(!s.contains(c), "sobrevivió {c}: {s}");
        }
        assert!(!s.contains(".."), "sobrevivió '..': {s}");
        assert!(!s.starts_with('.'), "no puede ser oculto: {s}");
    }

    #[test]
    fn sanea_longitud_y_vacio() {
        let long = "x".repeat(500);
        assert!(sanitize_fragment(&long).chars().count() <= MAX_BASE);
        assert_eq!(sanitize_fragment("   ...   "), FALLBACK);
        assert_eq!(sanitize_fragment(""), FALLBACK);
    }

    #[test]
    fn nombres_reservados_de_windows_se_escapan() {
        assert_eq!(sanitize_fragment("CON"), "_CON");
        assert_eq!(sanitize_fragment("com1"), "_com1");
        assert_ne!(sanitize_fragment("canción"), "_canción");
    }

    #[test]
    fn base_nombre_con_prefijo_y_sin_artista() {
        assert_eq!(base_name("Queen", "Bohemian Rhapsody", Some("01")), "01 - Queen - Bohemian Rhapsody");
        assert_eq!(base_name("", "Solo", None), "Solo");
        assert_eq!(base_name(" ... ", "Solo", None), "Solo");
        assert_eq!(base_name("Queen", "Queen", None), "Queen");
        // Prefijo con intento de ruta: se sanea, no escapa.
        let b = base_name("A", "B", Some("../01"));
        assert!(!b.contains('/') && !b.contains(".."), "{b}");
    }

    #[test]
    fn nunca_sobrescribe_anade_sufijo() {
        let d = tmp("dup");
        std::fs::write(d.join("Queen - Song.m4a"), b"a").unwrap();
        assert!(unique_path(&d, "Queen - Song", "m4a").ends_with("Queen - Song (2).m4a"));
        std::fs::write(d.join("Queen - Song (2).m4a"), b"b").unwrap();
        assert!(unique_path(&d, "Queen - Song", "m4a").ends_with("Queen - Song (3).m4a"));
        // Un nombre libre se usa tal cual.
        assert!(unique_path(&d, "Otra", "m4a").ends_with("Otra.m4a"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rechaza_path_traversal() {
        let dir = Path::new("/musica/destino");
        assert!(is_inside(dir, &dir.join("a.m4a")));
        assert!(!is_inside(dir, &dir.join("../externo.m4a")));
        assert!(!is_inside(dir, Path::new("/musica/destino2/a.m4a")));
        assert!(!is_inside(dir, Path::new("/etc/passwd")));
        // El nombre que sale de base_name nunca puede salir de la carpeta.
        let base = base_name("../../etc", "passwd", None);
        assert!(!base.contains('/') && !base.contains(".."), "{base}");
        assert!(is_inside(dir, &dir.join(format!("{base}.m4a"))));
    }

    #[test]
    fn url_solo_ids_limpios() {
        assert_eq!(video_url("dQw4w9WgXcQ").unwrap(), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert!(video_url("../../etc/passwd").is_err());
        assert!(video_url("abc&x=1").is_err());
        assert!(video_url("").is_err());
        assert!(video_url("x".repeat(40).as_str()).is_err());
    }

    #[test]
    fn argumentos_original_nunca_lleva_x() {
        let d = tmp("args1");
        let o = opts(d.to_str().unwrap(), "original", true);
        // Con ffmpeg: sí etiquetas (--embed-metadata), jamás portada (webm aborta).
        let con = build_args("https://www.youtube.com/watch?v=abc", &d, "t1", &o, true);
        assert!(!con.iter().any(|x| x == "-x"), "original no recodifica: {con:?}");
        assert!(!con.iter().any(|x| x == "--embed-thumbnail"));
        assert!(con.iter().any(|x| x == "--embed-metadata"));
        assert!(con.iter().any(|x| x == "bestaudio/best"));
        assert!(con.iter().any(|x| x == "--no-playlist"));
        // Sin ffmpeg: sin recodificar y sin postprocesos (los haría fallar).
        let sin = build_args("https://www.youtube.com/watch?v=abc", &d, "t1b", &o, false);
        assert!(!sin.iter().any(|x| x == "-x"));
        assert!(!sin.iter().any(|x| x == "--embed-thumbnail"));
        assert!(!sin.iter().any(|x| x == "--embed-metadata"));
        assert!(sin.iter().any(|x| x == "bestaudio/best"));
        assert_eq!(o.effective_format(false), "original");
        let out = con
            .iter()
            .position(|x| x == "-o")
            .map(|i| con[i + 1].clone())
            .unwrap();
        assert!(out.starts_with(d.to_str().unwrap()), "{out}");
        assert!(out.contains(".musica-dl-t1.%(ext)s"), "{out}");
        assert!(is_inside(&d, Path::new(&out.replace("%(ext)s", "m4a"))));
        std::fs::remove_dir_all(&d).ok();
    }

    /// B1: m4a real con ffmpeg — mejor fuente AAC (remux) y embeds completos.
    #[test]
    fn argumentos_m4a_con_ffmpeg() {
        let d = tmp("argsm4a");
        let o = opts(d.to_str().unwrap(), "m4a", true);
        let con = build_args("https://www.youtube.com/watch?v=abc", &d, "t4", &o, true);
        assert!(con.iter().any(|x| x == "bestaudio[ext=m4a]/bestaudio"));
        assert!(con.iter().any(|x| x == "-x"));
        assert!(con.iter().any(|x| x == "--audio-format"));
        assert!(con.iter().any(|x| x == "m4a"));
        assert!(con.iter().any(|x| x == "--embed-thumbnail"));
        assert!(con.iter().any(|x| x == "--embed-metadata"));
        assert_eq!(o.effective_format(true), "m4a");
        // Sin ffmpeg la m4a se deshabilita y se guarda el original.
        let sin = build_args("https://www.youtube.com/watch?v=abc", &d, "t5", &o, false);
        assert!(!sin.iter().any(|x| x == "-x"));
        assert!(!sin.iter().any(|x| x == "--embed-thumbnail"));
        assert!(!sin.iter().any(|x| x == "--embed-metadata"));
        assert!(sin.iter().any(|x| x == "bestaudio/best"));
        assert_eq!(o.effective_format(false), "original");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn argumentos_mp3_solo_con_ffmpeg() {
        let d = tmp("args2");
        let o = opts(d.to_str().unwrap(), "mp3", true);
        let con = build_args("https://www.youtube.com/watch?v=abc", &d, "t2", &o, true);
        assert!(con.iter().any(|x| x == "-x"));
        assert!(con.iter().any(|x| x == "mp3"));
        assert!(con.iter().any(|x| x == "--embed-thumbnail"));
        assert!(con.iter().any(|x| x == "--embed-metadata"));
        assert_eq!(o.effective_format(true), "mp3");
        // Sin ffmpeg la app no falla: degrada a original sin recodificar.
        let sin = build_args("https://www.youtube.com/watch?v=abc", &d, "t3", &o, false);
        assert!(!sin.iter().any(|x| x == "-x"));
        assert!(!sin.iter().any(|x| x == "--embed-thumbnail"));
        assert_eq!(o.effective_format(false), "original");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn resumen_concorda_con_las_cuentas() {
        assert_eq!(summary_text(48, 2, 1), "48 descargadas, 2 omitidas, 1 fallida");
        assert_eq!(summary_text(1, 0, 0), "1 descargada");
        assert_eq!(summary_text(0, 0, 0), "0 descargadas");
        assert_eq!(summary_text(3, 1, 2), "3 descargadas, 1 omitida, 2 fallidas");
    }

    #[test]
    fn clasifica_fallos_para_reintento() {
        // Muerta/restringida: sin reintento (no se saltan restricciones).
        assert_eq!(fail_kind("yt-dlp: Video unavailable"), FailKind::Dead);
        assert_eq!(fail_kind("ERROR: Private video"), FailKind::Dead);
        // Sin sesión / sesión rechazada: conectar, no esperar.
        assert_eq!(
            fail_kind("ERROR: [youtube] x: Sign in to confirm you’re not a bot. Use --cookies for the authentication."),
            FailKind::Auth
        );
        assert_eq!(
            fail_kind("ERROR: The provided YouTube account cookies are no longer valid"),
            FailKind::Auth
        );
        // Límite real: pausa la cola (backoff existente) y reintenta.
        assert_eq!(fail_kind("ERROR: HTTP Error 429: Too Many Requests"), FailKind::Throttle);
        // Transitorio: un solo reintento.
        assert_eq!(fail_kind("network unreachable"), FailKind::Transient);
        assert!(!FailKind::Dead.retryable());
        assert!(!FailKind::Auth.retryable());
        assert!(FailKind::Throttle.retryable());
        assert!(FailKind::Transient.retryable());
    }

    #[test]
    fn prefijo_numérico_de_playlist() {
        assert_eq!(prefix_for(0), "01");
        assert_eq!(prefix_for(9), "10");
        assert_eq!(prefix_for(123), "124");
        assert_eq!(base_name("A", "B", Some(&prefix_for(1))), "02 - A - B");
    }

    /// D4/B1: sin ffmpeg la app no falla; se guarda el original (sin -x ni embeds).
    #[test]
    fn sin_ffmpeg_la_app_degrada() {
        let ffmpeg = has_ffmpeg();
        let o = opts("/tmp", "mp3", true);
        assert_eq!(o.effective_format(ffmpeg), if ffmpeg { "mp3" } else { "original" });
        assert_eq!(o.effective_format(false), "original");
        assert_eq!(opts("/tmp", "m4a", true).effective_format(false), "original");
        assert_eq!(opts("/tmp", "original", true).effective_format(ffmpeg), "original");
    }

    /// Lógica de cola: contadores que siempre cuadran y drenado al cancelar.
    #[test]
    fn cola_contadores_y_drenado() {
        with_snap(|s| *s = Snapshot::default());
        with_snap(|s| {
            apply_outcome(s, &Outcome::Ok, "x");
            apply_outcome(s, &Outcome::Skip("cancelada".into()), "y");
            apply_outcome(s, &Outcome::Err("fallo".into()), "z");
        });
        let (d, sk, f, fin) = with_snap(|s| (s.downloaded, s.skipped, s.failed, s.finished));
        assert_eq!((d, sk, f), (1, 1, 1));
        assert_eq!(fin, d + sk + f, "finished siempre cuadra con el reparto");
        // Cancelar: lo que quede pendiente pasa a "omitida" sin intentarse.
        {
            let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
            q.clear();
            for n in 0..2 {
                q.push_back(Job {
                    item: DownloadItem { id: format!("id{n}"), title: "t".into(), artist: "a".into() },
                    prefix: None,
                });
            }
        }
        assert_eq!(drain_pending(), 2);
        assert_eq!(queue().lock().unwrap_or_else(|e| e.into_inner()).len(), 0);
        assert_eq!(with_snap(|s| s.skipped), 1 + 2);
        with_snap(|s| *s = Snapshot::default());
    }

    #[test]
    fn mensaje_ytdlp_distingue_ausente_de_fallo() {
        let ausente = ytmusic::Probe {
            available: false,
            version: None,
            error: None,
            deno: false,
            node: false,
        };
        assert!(mensaje_ytdlp(&ausente).contains("no está instalado"));

        let roto = ytmusic::Probe {
            available: false,
            version: None,
            error: Some("Fatal Python error: boom".into()),
            deno: false,
            node: false,
        };
        let m = mensaje_ytdlp(&roto);
        assert_eq!(m, "yt-dlp falló: Fatal Python error: boom");
        assert!(!m.contains("no está instalado"));
    }
}
