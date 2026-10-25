//! P6: recomendaciones de YouTube Music.
//!
//! Cliente interno mínimo para `FEmusic_home` y sus continuaciones. No usa
//! `yt-dlp` porque este no expone el home personalizado; la sesión sigue
//! saliendo del jar 0600 que ya valida la app. También expone `playlist_page`
//! para listar playlists por el mismo cliente (sin proceso Python). Todo el
//! material sensible (cookie, SAPISIDHASH, visitorData y cuerpos de respuesta)
//! queda en memoria y nunca se registra ni se devuelve a la UI.
//!
//! Política:
//! - home + como máximo 2 continuaciones;
//! - tope de tarjetas y de tamaño de respuesta;
//! - solo playlists estables (`PL…` / `OLAK5uy…`);
//! - parser tolerante: un renderer desconocido o una sección vacía no rompen;
//! - caché en memoria de 10 min, ligada a la sesión actual;
//! - errores tipados y sin cuerpo de respuesta.

use crate::ytmusic;
use serde::Serialize;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;
use walkdir::WalkDir;

const BROWSE_URL: &str = "https://music.youtube.com/youtubei/v1/browse?prettyPrint=false";
const NEXT_URL: &str = "https://music.youtube.com/youtubei/v1/next?prettyPrint=false";
const LIKE_URL: &str = "https://music.youtube.com/youtubei/v1/like/like?prettyPrint=false";
const REMOVE_LIKE_URL: &str =
    "https://music.youtube.com/youtubei/v1/like/removelike?prettyPrint=false";
const ORIGIN: &str = "https://music.youtube.com";
const TTL: Duration = Duration::from_secs(10 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_CONTINUATIONS: usize = 2;
const MAX_ITEMS: usize = 80;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_COOKIE_HEADER: usize = 16 * 1024;

/// Clave pública del cliente web de YouTube Music. No es una credencial de
/// usuario: solo identifica la aplicación en la API interna. Se puede
/// sobreescribir con la que esté cacheada por el WebView de login.
const FALLBACK_API_KEY: &str = "AIzaSyC9XL3ZjWddXya6X74dJoCTL-WEYFDNX30";
/// Fallback por si el WebView aún no ha cacheado `ytcfg` (sesión importada).
const FALLBACK_CLIENT_VERSION: &str = "1.20261018.16.00";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// Una tarjeta de playlist ya filtrada y normalizada.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Recommendation {
    pub title: String,
    pub author: String,
    pub views: String,
    pub thumbnail: String,
    pub playlist_id: String,
    pub section: String,
}

/// Respuesta que consume la vista. `community_present` permite el aviso
/// inline cuando YouTube Music no devolvió «De la comunidad».
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct RecoFeed {
    pub items: Vec<Recommendation>,
    pub community_present: bool,
    pub truncated: bool,
    /// Aviso no fatal: el home se cargó, pero una continuación falló.
    pub warning: Option<String>,
}

/// Página de una playlist obtenida por InnerTube. Se reutiliza el mismo
/// cliente/cookies que las recomendaciones: sin proceso `yt-dlp`.
#[derive(Debug, Clone)]
pub struct PlaylistPage {
    pub title: Option<String>,
    /// Total declarado por YouTube («182 pistas»), si vino en la cabecera.
    pub total: Option<usize>,
    /// ¿La playlist ya está guardada en la biblioteca del usuario?
    pub saved: Option<bool>,
    pub tracks: Vec<ytmusic::YtTrack>,
    /// Token para pedir la página siguiente (None = última).
    pub continuation: Option<String>,
}

/// Códigos estables para que la UI decida entre estado vacío, ajustes o
/// reintento. El mensaje es corto y nunca incluye cookies ni cuerpos.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoErrorCode {
    NoSession,
    Auth,
    Config,
    Http,
    Network,
    Parse,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecoError {
    pub code: RecoErrorCode,
    pub message: String,
}

impl RecoError {
    fn new(code: RecoErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn no_session() -> Self {
        Self::new(
            RecoErrorCode::NoSession,
            "Conecta tu cuenta de YouTube Music para ver recomendaciones.",
        )
    }

    fn auth() -> Self {
        Self::new(
            RecoErrorCode::Auth,
            "La sesión de YouTube Music no autoriza esta consulta.",
        )
    }

    fn config() -> Self {
        Self::new(
            RecoErrorCode::Config,
            "La configuración de YouTube Music no es válida. Vuelve a conectar la cuenta.",
        )
    }

    fn http(status: u16) -> Self {
        let message = if status == 429 {
            "YouTube Music limitó las peticiones. Inténtalo de nuevo en unos minutos.".to_string()
        } else {
            format!("YouTube Music respondió {status}.")
        };
        Self::new(RecoErrorCode::Http, message)
    }

    fn network() -> Self {
        Self::new(
            RecoErrorCode::Network,
            "No se pudo conectar con YouTube Music.",
        )
    }

    fn parse() -> Self {
        Self::new(
            RecoErrorCode::Parse,
            "YouTube Music devolvió una respuesta que no se pudo interpretar.",
        )
    }
}

impl std::fmt::Display for RecoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Sesión extraída del jar. No implementa `Debug` a propósito.
struct Session {
    cookie_header: String,
    sapisid: String,
    tag: u64,
}

impl Session {
    fn load() -> Result<Self, RecoError> {
        let path = ytmusic::recommendations_cookie_path().ok_or_else(RecoError::no_session)?;
        let raw = std::fs::read_to_string(&path).map_err(|_| RecoError::no_session())?;
        let now = now_secs() as i64;
        let mut cookies: Vec<(String, String)> = Vec::new();
        let mut sapisid: Option<(u8, String)> = None;
        let mut has_login_info = false;

        for raw_line in raw.lines() {
            let line = raw_line.strip_prefix("#HttpOnly_").unwrap_or(raw_line);
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() != 7 {
                continue;
            }
            let domain = fields[0].trim_start_matches('.').to_ascii_lowercase();
            if !is_youtube_domain(&domain) {
                continue;
            }
            let expires = fields[4].parse::<i64>().unwrap_or(0);
            if expires > 0 && expires < now {
                continue;
            }
            let name = fields[5];
            let value = fields[6];
            if !header_safe(name) || !header_safe(value) {
                continue;
            }
            if name == "LOGIN_INFO" {
                has_login_info = true;
            }
            if name == "SAPISID" {
                // Preferimos la cookie del dominio apex y raíz; si hay varias,
                // nos quedamos con la de menor prioridad numérica.
                let score = if domain == "youtube.com" { 0 } else { 1 };
                if sapisid.as_ref().map(|(s, _)| score < *s).unwrap_or(true) {
                    sapisid = Some((score, value.to_string()));
                }
            }
            if !cookies.iter().any(|(n, v)| n == name && v == value) {
                cookies.push((name.to_string(), value.to_string()));
            }
        }

        let (_, sapisid) = sapisid.ok_or_else(RecoError::no_session)?;
        if !has_login_info || sapisid.is_empty() {
            return Err(RecoError::no_session());
        }
        let cookie_header = cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if cookie_header.is_empty() || cookie_header.len() > MAX_COOKIE_HEADER {
            return Err(RecoError::no_session());
        }

        let tag = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            cookie_header.hash(&mut h);
            h.finish()
        };
        Ok(Self {
            cookie_header,
            sapisid,
            tag,
        })
    }
}

/// Configuración pública del cliente web. `visitor` es opcional y solo se usa
/// en las cabeceras; nunca sale de este módulo.
struct WebConfig {
    api_key: String,
    client_version: String,
    visitor: Option<String>,
}

impl WebConfig {
    fn load() -> Self {
        let mut api_key = None;
        let mut client_version = None;
        let mut visitor = None;
        let dir = ytmusic::login_data_dir();
        if dir.is_dir() {
            let mut scanned_bytes = 0usize;
            let mut scanned_files = 0usize;
            for entry in WalkDir::new(&dir)
                .max_depth(6)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
            {
                if scanned_bytes >= 24 * 1024 * 1024 || scanned_files >= 256 {
                    break;
                }
                if !entry.file_type().is_file() {
                    continue;
                }
                if matches!(
                    entry.file_name().to_str(),
                    Some("cookies" | "hsts-storage.sqlite" | "origin" | "salt")
                ) {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                if meta.len() > 8 * 1024 * 1024 {
                    continue;
                }
                let Ok(bytes) = std::fs::read(entry.path()) else {
                    continue;
                };
                scanned_files += 1;
                scanned_bytes = scanned_bytes.saturating_add(bytes.len());
                if api_key.is_none() {
                    api_key =
                        json_string_value_if(&bytes, b"\"INNERTUBE_API_KEY\"", plausible_api_key);
                }
                if client_version.is_none() {
                    client_version = json_string_value_if(
                        &bytes,
                        b"\"INNERTUBE_CLIENT_VERSION\"",
                        plausible_client_version,
                    );
                }
                if visitor.is_none() {
                    visitor = json_string_value_if(&bytes, b"\"VISITOR_DATA\"", plausible_visitor)
                        .or_else(|| {
                            json_string_value_if(&bytes, b"\"visitorData\"", plausible_visitor)
                        });
                }
                if api_key.is_some() && client_version.is_some() && visitor.is_some() {
                    break;
                }
            }
        }
        Self {
            api_key: api_key.unwrap_or_else(|| FALLBACK_API_KEY.to_string()),
            client_version: client_version.unwrap_or_else(|| FALLBACK_CLIENT_VERSION.to_string()),
            visitor,
        }
    }

    fn is_valid(&self) -> bool {
        plausible_api_key(&self.api_key)
            && plausible_client_version(&self.client_version)
            && self.visitor.as_deref().map_or(true, plausible_visitor)
    }
}

/// Distingue un fallo de configuración local (antes de tocar la red) de una
/// respuesta HTTP real. El mensaje nunca incluye la clave ni la versión.
fn ensure_config(cfg: &WebConfig) -> Result<(), RecoError> {
    if cfg.is_valid() {
        Ok(())
    } else {
        Err(RecoError::config())
    }
}

#[derive(Clone)]
struct CacheEntry {
    at: Instant,
    tag: u64,
    feed: RecoFeed,
}

static CACHE: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();
static FETCH_GATE: OnceLock<AsyncMutex<()>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn fetch_gate() -> &'static AsyncMutex<()> {
    FETCH_GATE.get_or_init(|| AsyncMutex::new(()))
}

fn cache_get(tag: u64) -> Option<RecoFeed> {
    let guard = cache().lock().ok()?;
    let entry = guard.as_ref()?;
    if entry.tag == tag && entry.at.elapsed() <= TTL {
        Some(entry.feed.clone())
    } else {
        None
    }
}

fn cache_put(tag: u64, feed: &RecoFeed) {
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(CacheEntry {
            at: Instant::now(),
            tag,
            feed: feed.clone(),
        });
    }
}

/// Entrada del comando `youtube_recommendations`.
pub async fn recommendations(force: bool) -> Result<RecoFeed, RecoError> {
    let session = Session::load()?;
    if !force {
        if let Some(feed) = cache_get(session.tag) {
            return Ok(feed);
        }
    }

    // Evita que dos refrescos simultáneos dupliquen red y escrituras de caché.
    let gate = fetch_gate();
    let _guard = gate.lock().await;

    if !force {
        if let Some(feed) = cache_get(session.tag) {
            return Ok(feed);
        }
    }

    let feed = fetch_feed(&session).await?;
    if feed.warning.is_none() {
        cache_put(session.tag, &feed);
    }
    Ok(feed)
}

/// Lista una página de playlist por InnerTube. `continuation=None` pide la
/// primera página; luego se pasa el token devuelto. La app usa esto para
/// pintar las filas sin arrancar Python/yt-dlp, con respaldo automático en
/// `batch.rs` si la sesión no está disponible o el formato cambia.
pub async fn playlist_page(
    playlist_id: &str,
    continuation: Option<&str>,
) -> Result<PlaylistPage, RecoError> {
    if !valid_playlist_id(playlist_id) {
        return Err(RecoError::parse());
    }
    let session = Session::load()?;
    let cfg = WebConfig::load();
    ensure_config(&cfg)?;
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| RecoError::network())?;

    let call = match continuation {
        Some(token) => continuation_call(&cfg, token),
        None => playlist_call(&cfg, playlist_id),
    };
    let value = browse(&client, &session, &cfg, call).await?;

    let header = find_key(&value, "musicResponsiveHeaderRenderer", 0)
        .or_else(|| find_key(&value, "musicDetailHeaderRenderer", 0))
        .or_else(|| find_key(&value, "musicEditablePlaylistDetailHeaderRenderer", 0));
    let title = header.and_then(|h| text_of(h.get("title")));
    let total = header
        .and_then(|h| text_of(h.get("secondSubtitle")))
        .and_then(|s| parse_playlist_total(&s));
    let saved = header.and_then(parse_playlist_saved);

    let (mut tracks, next) = match playlist_node(&value) {
        Some(node) => {
            let mut items = Vec::new();
            collect_playlist_items(node, &mut items, 0);
            let tracks = items
                .into_iter()
                .filter_map(parse_playlist_track)
                .collect::<Vec<_>>();
            (tracks, playlist_next_token(node))
        }
        None => (Vec::new(), None),
    };

    // Si la respuesta no trae el renderer esperado, no se inventa una lista
    // vacía: el llamante prefiere el respaldo yt-dlp.
    if tracks.is_empty() {
        return Err(RecoError::parse());
    }
    // Tope defensivo: una página nunca debería superar 500 entradas.
    tracks.truncate(500);
    Ok(PlaylistPage { title, total, saved, tracks, continuation: next })
}

/// La cabecera usa `toggleButtonRenderer` con icono BOOKMARK para «Guardar en
/// la biblioteca». `isToggled` es el estado persistente de la playlist.
fn parse_playlist_saved(header: &Value) -> Option<bool> {
    let buttons = header.get("buttons")?.as_array()?;
    for button in buttons {
        let Some(toggle) = button.get("toggleButtonRenderer") else {
            continue;
        };
        let icon = toggle
            .pointer("/defaultIcon/iconType")
            .or_else(|| toggle.pointer("/toggledIcon/iconType"))
            .and_then(Value::as_str);
        if matches!(icon, Some("BOOKMARK_BORDER") | Some("BOOKMARK")) {
            return toggle.get("isToggled").and_then(Value::as_bool);
        }
    }
    None
}

/// Estado actual del «Me gusta» de un tema. `None` = la app/el vídeo no
/// permite like (o no vino el botón). Usa `next`, que es una respuesta
/// pequeña (~20 KB) sin cargar la cola de reproducción.
pub async fn like_status(video_id: &str) -> Result<Option<bool>, RecoError> {
    if !valid_video_id(video_id) {
        return Err(RecoError::parse());
    }
    let session = Session::load()?;
    let cfg = WebConfig::load();
    ensure_config(&cfg)?;
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| RecoError::network())?;
    let body = json!({
        "context": request_context(&cfg),
        "videoId": video_id,
    });
    let value = post_innertube(
        &client,
        &session,
        &cfg,
        reqwest::Method::POST,
        NEXT_URL,
        &body,
    )
    .await?;
    Ok(parse_like_status(&value))
}

/// Guarda (LIKE) o quita (removelike) el «Me gusta» de un tema en YouTube
/// Music. Es el equivalente al corazón de la app web.
pub async fn set_like(video_id: &str, liked: bool) -> Result<(), RecoError> {
    if !valid_video_id(video_id) {
        return Err(RecoError::parse());
    }
    let session = Session::load()?;
    let cfg = WebConfig::load();
    ensure_config(&cfg)?;
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| RecoError::network())?;
    let body = json!({
        "context": request_context(&cfg),
        "target": { "videoId": video_id },
    });
    let url = if liked { LIKE_URL } else { REMOVE_LIKE_URL };
    let _ = post_innertube(
        &client,
        &session,
        &cfg,
        reqwest::Method::POST,
        url,
        &body,
    )
    .await?;
    Ok(())
}

/// Guarda o quita una playlist/álbum de la biblioteca de YouTube Music.
/// En la app web este botón se llama «Guardar en la biblioteca».
pub async fn set_playlist_like(playlist_id: &str, saved: bool) -> Result<(), RecoError> {
    if !valid_playlist_id(playlist_id) {
        return Err(RecoError::parse());
    }
    let session = Session::load()?;
    let cfg = WebConfig::load();
    ensure_config(&cfg)?;
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| RecoError::network())?;
    let body = json!({
        "context": request_context(&cfg),
        "target": { "playlistId": playlist_id },
    });
    let url = if saved { LIKE_URL } else { REMOVE_LIKE_URL };
    let _ = post_innertube(
        &client,
        &session,
        &cfg,
        reqwest::Method::POST,
        url,
        &body,
    )
    .await?;
    Ok(())
}

fn valid_video_id(id: &str) -> bool {
    (2..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn parse_like_status(value: &Value) -> Option<bool> {
    let button = find_key(value, "likeButtonRenderer", 0)?;
    if button.get("likesAllowed").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    match button.get("likeStatus").and_then(Value::as_str)? {
        "LIKE" => Some(true),
        "INDIFFERENT" | "DISLIKE" => Some(false),
        _ => None,
    }
}

fn valid_playlist_id(id: &str) -> bool {
    (2..=100).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn playlist_call(cfg: &WebConfig, playlist_id: &str) -> BrowseCall {
    let browse_id = if playlist_id.starts_with("VL") {
        playlist_id.to_string()
    } else {
        format!("VL{playlist_id}")
    };
    BrowseCall {
        method: reqwest::Method::POST,
        url: BROWSE_URL,
        body: json!({
            "context": request_context(cfg),
            "browseId": browse_id,
        }),
    }
}

fn playlist_node(value: &Value) -> Option<&Value> {
    find_key(value, "musicPlaylistShelfRenderer", 0)
        .or_else(|| find_key(value, "musicPlaylistShelfContinuation", 0))
        .or_else(|| find_key(value, "appendContinuationItemsAction", 0))
}

fn collect_playlist_items<'a>(node: &'a Value, out: &mut Vec<&'a Value>, depth: u8) {
    if depth > 14 {
        return;
    }
    match node {
        Value::Object(map) => {
            if let Some(item) = map.get("musicResponsiveListItemRenderer") {
                out.push(item);
            }
            for value in map.values() {
                collect_playlist_items(value, out, depth + 1);
            }
        }
        Value::Array(items) => {
            for value in items {
                collect_playlist_items(value, out, depth + 1);
            }
        }
        _ => {}
    }
}

fn playlist_next_token(node: &Value) -> Option<String> {
    find_key(node, "continuationCommand", 0)
        .and_then(|c| c.get("token"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            find_key(node, "nextContinuationData", 0)
                .and_then(|c| c.get("continuation"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            find_key(node, "nextRadioContinuationData", 0)
                .and_then(|c| c.get("continuation"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn parse_playlist_track(item: &Value) -> Option<ytmusic::YtTrack> {
    let id = item
        .pointer("/playlistItemData/videoId")
        .and_then(Value::as_str)
        .or_else(|| {
            item.pointer(
                "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId",
            )
            .and_then(Value::as_str)
        })
        .or_else(|| {
            item.pointer(
                "/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/watchEndpoint/videoId",
            )
            .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|id| !id.is_empty() && *id != "NA")?;

    let title = text_of(
        item.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text"),
    )
    .unwrap_or_default();
    let artist_raw = text_of(
        item.pointer("/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text"),
    )
    .unwrap_or_default();
    // «Artista • Álbum • Año»: el nombre visible es el primer tramo.
    let artist = artist_raw
        .split(" • ")
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let duration = text_of(
        item.pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text"),
    )
    .or_else(|| text_of(item.pointer("/lengthText")))
    .map(|s| parse_yt_duration(&s))
    .unwrap_or(0);

    Some(ytmusic::YtTrack {
        id: id.to_string(),
        title: title.trim().to_string(),
        artist,
        duration_secs: duration,
        thumbnail: String::new(),
        liked: parse_like_status(item),
    })
}

/// «1:54» → 114; «1:02:03» → 3723; valores no numéricos → 0.
fn parse_yt_duration(text: &str) -> i64 {
    let mut total = 0i64;
    for part in text.trim().split(':') {
        let Ok(n) = part.trim().parse::<i64>() else {
            return 0;
        };
        total = total * 60 + n;
    }
    total.max(0)
}

fn parse_playlist_total(text: &str) -> Option<usize> {
    for part in text.split(['•', '·', ',', '|']) {
        let lower = part.to_lowercase();
        if lower.contains("pista")
            || lower.contains("canci")
            || lower.contains("song")
            || lower.contains("track")
        {
            let digits: String = part.chars().filter(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<usize>() {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    None
}

async fn fetch_feed(session: &Session) -> Result<RecoFeed, RecoError> {
    let cfg = WebConfig::load();
    ensure_config(&cfg)?;
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| RecoError::network())?;

    let mut feed = RecoFeed::default();
    let mut seen = HashSet::new();

    let home = browse(&client, session, &cfg, home_call(&cfg)).await?;

    let some_list = section_list(&home).ok_or_else(RecoError::parse)?;
    absorb_sections(some_list, &mut feed, &mut seen);
    let mut next = next_continuation(some_list);
    let mut used = 0;

    while used < MAX_CONTINUATIONS {
        if feed.items.len() >= MAX_ITEMS {
            feed.truncated = true;
            break;
        }
        let Some(token) = next.take() else { break };
        used += 1;
        let response = browse(&client, session, &cfg, continuation_call(&cfg, &token)).await;
        let Some(more_token) = absorb_continuation_result(&mut feed, &mut seen, response) else {
            break;
        };
        next = Some(more_token);
    }

    if next.is_some() && feed.items.len() < MAX_ITEMS {
        feed.truncated = true;
    }
    Ok(feed)
}

/// Consume una continuación sin convertir un fallo parcial en error total.
/// Devuelve el token siguiente si la página se entendió; en caso de fallo,
/// conserva las tarjetas ya absorbidas y deja un aviso inline.
fn absorb_continuation_result(
    feed: &mut RecoFeed,
    seen: &mut HashSet<String>,
    result: Result<Value, RecoError>,
) -> Option<String> {
    match result {
        Ok(value) => {
            let Some(more) = section_list(&value) else {
                feed.truncated = true;
                feed.warning =
                    Some("El home se cargó, pero no se pudo leer la continuación.".to_string());
                return None;
            };
            absorb_sections(more, feed, seen);
            next_continuation(more)
        }
        Err(err) => {
            feed.truncated = true;
            feed.warning = Some(format!(
                "El home se cargó, pero no se pudieron cargar más recomendaciones: {}",
                err.message
            ));
            None
        }
    }
}

struct BrowseCall {
    method: reqwest::Method,
    url: &'static str,
    body: Value,
}

fn home_call(cfg: &WebConfig) -> BrowseCall {
    BrowseCall {
        method: reqwest::Method::POST,
        url: BROWSE_URL,
        body: json!({
            "context": request_context(cfg),
            "browseId": "FEmusic_home",
        }),
    }
}

fn continuation_call(cfg: &WebConfig, token: &str) -> BrowseCall {
    BrowseCall {
        method: reqwest::Method::POST,
        url: BROWSE_URL,
        body: json!({
            "context": request_context(cfg),
            "continuation": token,
        }),
    }
}

async fn browse(
    client: &reqwest::Client,
    session: &Session,
    cfg: &WebConfig,
    call: BrowseCall,
) -> Result<Value, RecoError> {
    post_innertube(client, session, cfg, call.method, call.url, &call.body).await
}

/// POST genérico a cualquier endpoint youtubei con las mismas cabeceras
/// autenticadas (browse, next, like/…). Nunca registra cookies ni cuerpos.
async fn post_innertube(
    client: &reqwest::Client,
    session: &Session,
    cfg: &WebConfig,
    method: reqwest::Method,
    url: &str,
    body: &Value,
) -> Result<Value, RecoError> {
    let ts = now_secs();
    let auth = format!("SAPISIDHASH {}_{}", ts, sapisid_hash(ts, &session.sapisid));
    let mut headers = reqwest::header::HeaderMap::new();
    insert_header(&mut headers, "content-type", "application/json")?;
    insert_header(&mut headers, "accept", "application/json")?;
    insert_header(&mut headers, "accept-language", "es-419,es;q=0.9")?;
    insert_header(&mut headers, "origin", ORIGIN)?;
    insert_header(&mut headers, "referer", &format!("{ORIGIN}/"))?;
    insert_header(&mut headers, "user-agent", USER_AGENT)?;
    insert_header(&mut headers, "x-goog-api-key", &cfg.api_key)?;
    insert_header(&mut headers, "x-youtube-client-name", "67")?;
    insert_header(
        &mut headers,
        "x-youtube-client-version",
        &cfg.client_version,
    )?;
    insert_header(&mut headers, "x-goog-authuser", "0")?;
    insert_header(&mut headers, "authorization", &auth)?;
    insert_header(&mut headers, "cookie", &session.cookie_header)?;
    if let Some(visitor) = &cfg.visitor {
        // visitorData es opcional: si trae un carácter no válido se omite.
        if let Ok(value) = reqwest::header::HeaderValue::from_str(visitor) {
            headers.insert(
                reqwest::header::HeaderName::from_static("x-goog-visitor-id"),
                value,
            );
        }
    }

    let response = client
        .request(method, url)
        .headers(headers)
        .json(body)
        .send()
        .await
        .map_err(|_| RecoError::network())?;

    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(RecoError::auth());
    }
    if !status.is_success() {
        return Err(RecoError::http(status.as_u16()));
    }

    let bytes = response.bytes().await.map_err(|_| RecoError::network())?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(RecoError::parse());
    }
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| RecoError::parse())?;
    if let Some(code) = value.pointer("/error/code").and_then(Value::as_u64) {
        return Err(if code == 401 || code == 403 {
            RecoError::auth()
        } else {
            RecoError::http(code as u16)
        });
    }
    Ok(value)
}

fn insert_header(
    headers: &mut reqwest::header::HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), RecoError> {
    let value = reqwest::header::HeaderValue::from_str(value).map_err(|_| {
        RecoError::new(
            RecoErrorCode::Auth,
            "La sesión contiene valores de cabecera no válidos.",
        )
    })?;
    headers.insert(reqwest::header::HeaderName::from_static(name), value);
    Ok(())
}

fn request_context(cfg: &WebConfig) -> Value {
    let mut client = json!({
        "clientName": "WEB_REMIX",
        "clientVersion": cfg.client_version,
        "hl": "es-419",
        "gl": "MX",
        "userAgent": USER_AGENT,
    });
    if let Some(visitor) = &cfg.visitor {
        client["visitorData"] = Value::String(visitor.clone());
    }
    json!({
        "client": client,
        "user": { "lockedSafetyMode": false },
        "request": {
            "useSsl": true,
            "internalExperimentFlags": [],
            "consistencyTokenJars": [],
        },
    })
}

/// SHA-1 de `timestamp SAPISID origin`; formato exigido por SAPISIDHASH.
fn sapisid_hash(ts: u64, sapisid: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{ts} {sapisid} {ORIGIN}").as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Localiza la lista de secciones tanto en el home como en una continuación.
fn section_list(value: &Value) -> Option<&Value> {
    if let Some(list) = value.pointer("/continuationContents/sectionListContinuation") {
        return Some(list);
    }
    if let Some(list) =
        value.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer")
    {
        return Some(list);
    }
    find_key(value, "sectionListContinuation", 0)
        .or_else(|| find_key(value, "sectionListRenderer", 0))
}

fn find_key<'a>(value: &'a Value, key: &str, depth: u8) -> Option<&'a Value> {
    if depth > 16 {
        return None;
    }
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(key) {
                return Some(found);
            }
            map.values().find_map(|v| find_key(v, key, depth + 1))
        }
        Value::Array(items) => items.iter().find_map(|v| find_key(v, key, depth + 1)),
        _ => None,
    }
}

fn next_continuation(section_list: &Value) -> Option<String> {
    section_list
        .get("continuations")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find_map(|item| {
                item.pointer("/nextContinuationData/continuation")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
        })
}

fn absorb_sections(section_list: &Value, feed: &mut RecoFeed, seen: &mut HashSet<String>) {
    let Some(sections) = section_list.get("contents").and_then(Value::as_array) else {
        return;
    };
    for section in sections {
        absorb_section(section, feed, seen, 0);
    }
}

fn absorb_section(node: &Value, feed: &mut RecoFeed, seen: &mut HashSet<String>, depth: u8) {
    if depth > 12 {
        return;
    }
    if let Some(shelf) = node.get("musicCarouselShelfRenderer") {
        absorb_shelf(shelf, feed, seen);
        return;
    }
    match node {
        Value::Object(map) => {
            for value in map.values() {
                absorb_section(value, feed, seen, depth + 1);
            }
        }
        Value::Array(items) => {
            for value in items {
                absorb_section(value, feed, seen, depth + 1);
            }
        }
        _ => {}
    }
}

/// La sección de comunidad no tiene un id estable: se reconoce por título,
/// sin distinguir mayúsculas y aceptando variantes como «... en la comunidad».
fn is_community_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    lower.contains("comunidad") || lower.contains("community")
}

fn absorb_shelf(shelf: &Value, feed: &mut RecoFeed, seen: &mut HashSet<String>) {
    let title = text_of(shelf.pointer("/header/musicCarouselShelfBasicHeaderRenderer/title"))
        .or_else(|| text_of(shelf.pointer("/header/musicShelfRenderer/title")))
        .or_else(|| text_of(shelf.pointer("/header/title")))
        .unwrap_or_default();
    if is_community_title(&title) {
        feed.community_present = true;
    }

    let Some(items) = shelf.get("contents").and_then(Value::as_array) else {
        return;
    };
    for item in items {
        if feed.items.len() >= MAX_ITEMS {
            feed.truncated = true;
            break;
        }
        let Some(card) = item.get("musicTwoRowItemRenderer") else {
            continue;
        };
        let Some(recommendation) = parse_card(card, &title) else {
            continue;
        };
        if seen.insert(recommendation.playlist_id.clone()) {
            feed.items.push(recommendation);
        }
    }
}

fn parse_card(card: &Value, section: &str) -> Option<Recommendation> {
    let playlist_id = playlist_id(card)?;
    let title = text_of(card.get("title")).unwrap_or_default();
    let (author, views) = parse_subtitle(card.get("subtitle"));
    Some(Recommendation {
        title: if title.trim().is_empty() {
            "Playlist".to_string()
        } else {
            title.trim().to_string()
        },
        author: author.trim().to_string(),
        views: views.trim().to_string(),
        thumbnail: thumbnail_url(card),
        playlist_id,
        section: section.trim().to_string(),
    })
}

/// `musicTwoRowItemRenderer` no usa `thumbnail`: la portada real viene en
/// `thumbnailRenderer.musicThumbnailRenderer.thumbnail.thumbnails`.
/// Se elige la menor de 320 px o más; si no, la mayor disponible.
fn thumbnail_url(card: &Value) -> String {
    const TARGET: u64 = 320;
    let Some(thumbnails) = card
        .pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(Value::as_array)
    else {
        return String::new();
    };
    let mut best: Option<(u64, &str)> = None;
    let mut fallback: Option<(u64, &str)> = None;
    for item in thumbnails {
        let Some(url) = item
            .get("url")
            .and_then(Value::as_str)
            .filter(|url| url.starts_with("https://"))
        else {
            continue;
        };
        let width = item.get("width").and_then(Value::as_u64).unwrap_or(0);
        if width >= TARGET && best.map_or(true, |(best_width, _)| width < best_width) {
            best = Some((width, url));
        }
        if fallback.map_or(true, |(fallback_width, _)| width > fallback_width) {
            fallback = Some((width, url));
        }
    }
    best.or(fallback)
        .map(|(_, url)| url.to_string())
        .unwrap_or_default()
}

fn playlist_id(card: &Value) -> Option<String> {
    const PATHS: &[&str] = &[
        "/navigationEndpoint/browseEndpoint/browseId",
        "/navigationEndpoint/watchEndpoint/playlistId",
        "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/playlistId",
        "/thumbnailOverlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/playlistId",
        "/playlistItemData/playlistId",
    ];
    for path in PATHS {
        let Some(raw) = card.pointer(path).and_then(Value::as_str) else {
            continue;
        };
        let id = raw.strip_prefix("VL").unwrap_or(raw);
        if stable_playlist_id(id) {
            return Some(id.to_string());
        }
    }
    None
}

fn stable_playlist_id(id: &str) -> bool {
    (id.starts_with("PL") && id.len() > 2) || (id.starts_with("OLAK5uy") && id.len() > 7)
}

/// Dominios de cookie que sirven para la API de YouTube Music. El jar puede
/// traer `music.youtube.com` como cookie de host; el resto de subdominios de
/// youtube.com también vale. `evil-youtube.com` no.
fn is_youtube_domain(domain: &str) -> bool {
    let domain = domain.trim_start_matches('.');
    domain == "youtube.com" || domain.ends_with(".youtube.com")
}

fn parse_subtitle(subtitle: Option<&Value>) -> (String, String) {
    let Some(runs) = subtitle
        .and_then(|s| s.get("runs"))
        .and_then(Value::as_array)
    else {
        return (String::new(), String::new());
    };
    let mut author_nav: Option<String> = None;
    let mut author_any: Option<String> = None;
    let mut views: Option<String> = None;
    for run in runs {
        let Some(text) = text_of(Some(run)) else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if is_views(text) {
            if views.is_none() {
                views = Some(text.to_string());
            }
            continue;
        }
        if is_separator(text) || is_type(text) {
            continue;
        }
        if author_any.is_none() {
            author_any = Some(text.to_string());
        }
        if run.get("navigationEndpoint").is_some() && author_nav.is_none() {
            author_nav = Some(text.to_string());
        }
    }
    (
        author_nav.or(author_any).unwrap_or_default(),
        views.unwrap_or_default(),
    )
}

fn text_of(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(runs) = value.get("runs").and_then(Value::as_array) {
        let joined = runs
            .iter()
            .filter_map(|run| run.get("text").and_then(Value::as_str))
            .collect::<String>();
        if !joined.is_empty() {
            return Some(joined);
        }
    }
    if let Some(text) = value.get("simpleText").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    value.as_str().map(str::to_string)
}

fn is_views(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("vista") || lower.contains("view") || lower.contains("reproduc")
}

fn is_type(text: &str) -> bool {
    matches!(
        text.to_lowercase().as_str(),
        "playlist"
            | "lista"
            | "lista de reproducción"
            | "álbum"
            | "album"
            | "sencillo"
            | "single"
            | "ep"
            | "mix"
            | "radio"
            | "canción"
            | "cancion"
            | "video"
            | "vídeo"
            | "video musical"
    )
}

fn is_separator(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_whitespace() || "•·|/-–—".contains(c))
}

fn header_safe(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b';' && b != b'"' && b != b'\\')
}

fn plausible_api_key(value: &str) -> bool {
    value.len() == 39
        && value.starts_with("AIza")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn plausible_client_version(value: &str) -> bool {
    (8..=32).contains(&value.len())
        && (value.starts_with("1.") || value.starts_with("2."))
        && value.matches('.').count() >= 2
        && !value.ends_with('.')
        && value.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

fn plausible_visitor(value: &str) -> bool {
    (16..=512).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\' && b != b';')
}

/// Busca `"CLAVE": "valor"` dentro de un blob de caché (ytcfg suele estar
/// incrustado en JS, no como JSON puro). Exige que tras la clave vengan dos
/// puntos y una comilla, y acepta el primer valor que pase `accept`; así una
/// aparición de la clave en código no sustituye la configuración real.
fn json_string_value_if(bytes: &[u8], key: &[u8], accept: impl Fn(&str) -> bool) -> Option<String> {
    let mut from = 0usize;
    while let Some(rel) = find_subslice(&bytes[from..], key) {
        let pos = from + rel;
        if let Some(value) = json_string_value_at(bytes, pos + key.len()) {
            if accept(&value) {
                return Some(value);
            }
        }
        from = pos + key.len();
    }
    None
}

fn json_string_value_at(bytes: &[u8], start: usize) -> Option<String> {
    let mut rest = bytes.get(start..)?;
    while rest.first().is_some_and(|b| b.is_ascii_whitespace()) {
        rest = &rest[1..];
    }
    rest = rest.strip_prefix(b":")?;
    while rest.first().is_some_and(|b| b.is_ascii_whitespace()) {
        rest = &rest[1..];
    }
    rest = rest.strip_prefix(b"\"")?;
    let end = rest.iter().position(|&b| b == b'"')?;
    let value = std::str::from_utf8(&rest[..end]).ok()?;
    if value.contains('\\') {
        return None;
    }
    Some(value.to_string())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(title: &str, author: &str, views: &str, browse_id: &str) -> Value {
        json!({
            "musicTwoRowItemRenderer": {
                "title": { "runs": [{ "text": title }] },
                "subtitle": {
                    "runs": [
                        { "text": author, "navigationEndpoint": { "browseEndpoint": { "browseId": "UC_sintetico" } } },
                        { "text": "•" },
                        { "text": views }
                    ]
                },
                "navigationEndpoint": { "browseEndpoint": { "browseId": browse_id } },
                "thumbnailRenderer": { "musicThumbnailRenderer": { "thumbnail": {
                    "thumbnails": [
                        { "url": "https://i.ytimg.com/vi/sintetica/hqdefault.jpg", "width": 192, "height": 108 },
                        { "url": "https://yt3.ggpht.com/sintetica=w576-h576", "width": 576, "height": 576 }
                    ]
                } } }
            }
        })
    }

    fn section(title: &str, items: Vec<Value>) -> Value {
        json!({
            "musicCarouselShelfRenderer": {
                "header": { "musicCarouselShelfBasicHeaderRenderer": { "title": { "runs": [{ "text": title }] } } },
                "contents": items
            }
        })
    }

    fn test_cfg() -> WebConfig {
        WebConfig {
            api_key: FALLBACK_API_KEY.to_string(),
            client_version: FALLBACK_CLIENT_VERSION.to_string(),
            visitor: Some("VISITOR_SINTETICO".to_string()),
        }
    }

    #[test]
    fn peticiones_de_home_y_continuacion() {
        let cfg = test_cfg();
        let home = home_call(&cfg);
        assert_eq!(home.method, reqwest::Method::POST);
        assert_eq!(
            home.url,
            "https://music.youtube.com/youtubei/v1/browse?prettyPrint=false"
        );
        assert_eq!(home.body["browseId"], "FEmusic_home");
        assert!(home.body.get("continuation").is_none());
        assert_eq!(home.body["context"]["client"]["clientName"], "WEB_REMIX");

        let cont = continuation_call(&cfg, "TOKEN_SINTETICO");
        assert_eq!(cont.method, reqwest::Method::POST);
        assert_eq!(cont.url, home.url);
        assert_eq!(cont.body["continuation"], "TOKEN_SINTETICO");
        assert!(cont.body.get("browseId").is_none());
        assert_eq!(cont.body["context"]["client"]["clientName"], "WEB_REMIX");
    }

    #[test]
    fn cache_solo_acepta_config_valida() {
        let key = format!("AIza{}", "x".repeat(35));
        assert_eq!(key.len(), 39);
        let cache = format!(
            "if (x) {{ \"INNERTUBE_CLIENT_VERSION\": d.endsWith(\"x\") }}\n\
             ytcfg.set({{\"INNERTUBE_CLIENT_VERSION\":\"1.20261018.16.00\",\
             \"INNERTUBE_API_KEY\":\"{key}\"}});"
        );
        assert_eq!(
            json_string_value_if(
                cache.as_bytes(),
                b"\"INNERTUBE_CLIENT_VERSION\"",
                plausible_client_version
            )
            .as_deref(),
            Some("1.20261018.16.00")
        );
        assert_eq!(
            json_string_value_if(
                cache.as_bytes(),
                b"\"INNERTUBE_API_KEY\"",
                plausible_api_key
            )
            .as_deref(),
            Some(key.as_str())
        );
        assert!(json_string_value_if(
            b"{\"INNERTUBE_CLIENT_VERSION\": d.endsWith(\"x\")}",
            b"\"INNERTUBE_CLIENT_VERSION\"",
            plausible_client_version
        )
        .is_none());
        assert!(!plausible_client_version(").matches&&!d.endsWith("));
        assert!(!plausible_api_key("AIza_corta"));
    }

    #[test]
    fn dominios_de_cookie_de_youtube() {
        assert!(is_youtube_domain("youtube.com"));
        assert!(is_youtube_domain(".youtube.com"));
        assert!(is_youtube_domain("music.youtube.com"));
        assert!(is_youtube_domain("www.youtube.com"));
        assert!(!is_youtube_domain("evil-youtube.com"));
        assert!(!is_youtube_domain("youtube.com.evil.example"));
        assert!(!is_youtube_domain("google.com"));
    }

    #[test]
    fn sapisid_hash_es_estable() {
        // Valor sintético; no es una cookie real.
        assert_eq!(
            sapisid_hash(1_700_000_000, "SAPISID_SINTETICO"),
            "ad4066723bd648c664433769908817709fd8a3e8"
        );
    }

    #[test]
    fn filtro_estable_y_deduplicacion() {
        let value = json!({
            "contents": { "singleColumnBrowseResultsRenderer": { "tabs": [{
                "tabRenderer": { "content": { "sectionListRenderer": {
                    "contents": [
                        section("Volver a escuchar", vec![
                            card("Playlist estable", "Autor A", "1,2 M de vistas", "VLPL_sintetica_1"),
                            card("Playlist OLAK", "Autor B", "30 mil vistas", "VLOLAK5uy_sintetica_1"),
                            card("Radio", "Autor C", "10 mil vistas", "VLRD_sintetica_1"),
                            card("Duplicada", "Autor A", "1,2 M de vistas", "VLPL_sintetica_1")
                        ]),
                        section("Sección vacía", vec![]),
                        { "musicCarouselShelfRenderer": { "header": { "musicCarouselShelfBasicHeaderRenderer": { "title": { "runs": [{ "text": "Desconocida" }] } } }, "contents": [{ "musicTwoRowItemRenderer": { "title": { "simpleText": "Sin id" } } }, { "rendererDesconocido": {} }] } }
                    ],
                    "continuations": [{ "nextContinuationData": { "continuation": "TOKEN_SINTETICO" } }]
                } } }
            }] } }
        });
        let list = section_list(&value).expect("sectionListRenderer");
        let mut feed = RecoFeed::default();
        let mut seen = HashSet::new();
        absorb_sections(list, &mut feed, &mut seen);
        assert_eq!(feed.items.len(), 2);
        assert_eq!(feed.items[0].playlist_id, "PL_sintetica_1");
        assert_eq!(feed.items[0].author, "Autor A");
        assert_eq!(feed.items[0].views, "1,2 M de vistas");
        assert_eq!(feed.items[1].playlist_id, "OLAK5uy_sintetica_1");
        assert_eq!(next_continuation(list).as_deref(), Some("TOKEN_SINTETICO"));
        assert!(!feed.community_present);
    }

    #[test]
    fn comunidad_por_titulo_flexible() {
        let value = json!({
            "contents": [
                section("Playlists del momento en la comunidad", vec![
                    card("Comunidad A", "Autor A", "12 mil vistas", "VLPL_comunidad_1")
                ])
            ]
        });
        let mut feed = RecoFeed::default();
        let mut seen = HashSet::new();
        absorb_sections(&value, &mut feed, &mut seen);
        assert!(feed.community_present);
        assert_eq!(feed.items.len(), 1);
        assert_eq!(
            feed.items[0].section,
            "Playlists del momento en la comunidad"
        );
        assert!(is_community_title("COMMUNITY picks"));
        assert!(!is_community_title("Selección rápida"));
    }

    #[test]
    fn home_valido_y_continuacion_500_conserva_tarjetas() {
        let home = json!({
            "contents": { "singleColumnBrowseResultsRenderer": { "tabs": [{
                "tabRenderer": { "content": { "sectionListRenderer": {
                    "contents": [
                        section("Volver a escuchar", vec![
                            card("Home A", "Autor A", "10 vistas", "VLPL_home_1"),
                            card("Home B", "Autor B", "20 vistas", "VLPL_home_2")
                        ])
                    ],
                    "continuations": [{ "nextContinuationData": { "continuation": "TOKEN_SINTETICO" } }]
                } } }
            }] } }
        });
        let list = section_list(&home).expect("home válido");
        let mut feed = RecoFeed::default();
        let mut seen = HashSet::new();
        absorb_sections(list, &mut feed, &mut seen);
        assert_eq!(feed.items.len(), 2);

        let next = absorb_continuation_result(&mut feed, &mut seen, Err(RecoError::http(500)));
        assert!(next.is_none());
        assert_eq!(feed.items.len(), 2, "las tarjetas del home se conservan");
        assert!(feed.truncated);
        let warning = feed.warning.expect("aviso inline");
        assert!(warning.contains("500"), "{warning}");
        assert!(!warning.contains("SAPISID"), "{warning}");
    }

    #[test]
    fn configuracion_invalida_no_se_confunde_con_http() {
        let cfg = WebConfig {
            api_key: "CLAVE_SINTETICA".to_string(),
            client_version: "NO_VERSION".to_string(),
            visitor: None,
        };
        let err = ensure_config(&cfg).expect_err("configuración inválida");
        assert_eq!(err.code, RecoErrorCode::Config);
        assert!(err.message.contains("configuración"), "{}", err.message);
        assert!(!err.message.contains("CLAVE_SINTETICA"), "{}", err.message);
        assert_eq!(RecoError::http(500).code, RecoErrorCode::Http);
        assert!(RecoError::http(500).message.contains("500"));
        assert!(ensure_config(&test_cfg()).is_ok());
    }

    #[test]
    fn comunidad_y_seccion_sin_tarjetas() {
        let value = json!({
            "continuationContents": { "sectionListContinuation": {
                "contents": [
                    section("De la comunidad", vec![
                        card("Comunidad A", "Autor A", "12 mil vistas", "VLPL_comunidad_1"),
                        card("Comunidad B", "Autor B", "8 mil vistas", "VLPL_comunidad_2")
                    ]),
                    section("Sin tarjetas", vec![]),
                    { "rendererDesconocido": { "contents": [{ "musicTwoRowItemRenderer": { "title": { "runs": [{ "text": "Ignorada" }] } } }] } }
                ]
            } }
        });
        let list = section_list(&value).expect("sectionListContinuation");
        let mut feed = RecoFeed::default();
        let mut seen = HashSet::new();
        absorb_sections(list, &mut feed, &mut seen);
        assert!(feed.community_present);
        assert_eq!(feed.items.len(), 2);
        assert_eq!(feed.items[0].section, "De la comunidad");
    }

    #[test]
    fn continuacion_sin_token_y_respuesta_rara_no_rompen() {
        let value =
            json!({ "continuationContents": { "sectionListContinuation": { "contents": [] } } });
        let list = section_list(&value).expect("sección");
        assert!(next_continuation(list).is_none());
        let mut feed = RecoFeed::default();
        let mut seen = HashSet::new();
        absorb_sections(list, &mut feed, &mut seen);
        assert!(feed.items.is_empty());
        assert!(!feed.community_present);
        assert!(section_list(&json!({ "respuesta": "rara" })).is_none());
    }

    #[test]
    fn portada_elige_320_y_solo_acepta_https() {
        let value = card("Playlist", "Autor", "1 vista", "PL_portada");
        let card_value = value.get("musicTwoRowItemRenderer").expect("tarjeta");
        let item = parse_card(card_value, "Sección").expect("tarjeta válida");
        assert_eq!(item.thumbnail, "https://yt3.ggpht.com/sintetica=w576-h576");

        let mut variado = card("Playlist", "Autor", "1 vista", "PL_variado");
        variado["musicTwoRowItemRenderer"]["thumbnailRenderer"]["musicThumbnailRenderer"]
            ["thumbnail"]["thumbnails"] = json!([
            { "url": "https://i.ytimg.com/vi/a/hqdefault.jpg", "width": 192 },
            { "url": "https://i.ytimg.com/vi/a/maxresdefault.jpg", "width": 800 },
            { "url": "https://i.ytimg.com/vi/a/mqdefault.jpg", "width": 400 }
        ]);
        let variado = parse_card(
            variado
                .get("musicTwoRowItemRenderer")
                .expect("tarjeta variada"),
            "Sección",
        )
        .expect("válida");
        assert_eq!(variado.thumbnail, "https://i.ytimg.com/vi/a/mqdefault.jpg");

        let mut inseguro = card("Playlist", "Autor", "1 vista", "PL_inseguro");
        inseguro["musicTwoRowItemRenderer"]["thumbnailRenderer"]["musicThumbnailRenderer"]
            ["thumbnail"]["thumbnails"] = json!([
            { "url": "http://i.ytimg.com/vi/a/hqdefault.jpg", "width": 800 }
        ]);
        let inseguro = parse_card(
            inseguro
                .get("musicTwoRowItemRenderer")
                .expect("tarjeta insegura"),
            "Sección",
        )
        .expect("válida");
        assert!(inseguro.thumbnail.is_empty());
    }

    #[test]
    fn autor_sin_enlace_y_vistas_se_extraen_igual() {
        let value = json!({
            "title": { "runs": [{ "text": "Playlist" }] },
            "subtitle": { "runs": [
                { "text": "Playlist" },
                { "text": "·" },
                { "text": "Autor sin enlace" },
                { "text": "·" },
                { "text": "42 vistas" }
            ] },
            "navigationEndpoint": { "watchEndpoint": { "playlistId": "PL_sin_enlace" } }
        });
        let (author, views) = parse_subtitle(value.get("subtitle"));
        assert_eq!(author, "Autor sin enlace");
        assert_eq!(views, "42 vistas");
        assert!(stable_playlist_id("PL_sin_enlace"));
        assert!(!stable_playlist_id("RD_sin_enlace"));
    }

    /// Red real: requiere el jar de sesión validado (por eso está ignorado).
    #[tokio::test]
    #[ignore = "red: requiere sesión de YouTube"]
    async fn pagina_playlist_real_innertube() {
        let page = playlist_page("PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI", None)
            .await
            .expect("la playlist pública debe listar");
        assert!(!page.tracks.is_empty(), "debe haber pistas");
        assert!(page.total.unwrap_or(0) >= page.tracks.len(), "total declarado");
        assert!(page.tracks.iter().all(|t| !t.id.is_empty()), "ids válidos");
        assert!(
            page.tracks.iter().any(|t| t.liked.is_some()),
            "InnerTube debe traer el estado del corazón"
        );
        assert!(page.saved.is_some(), "la cabecera debe traer si está guardada");
    }

    /// Red real y SOLO LECTURA: consulta el corazón sin cambiar la biblioteca.
    #[tokio::test]
    #[ignore = "red: requiere sesión de YouTube"]
    async fn estado_like_real_sin_modificar_nada() {
        let status = like_status("fOT0BUpITw8").await.expect("debe responder");
        assert!(status.is_some(), "un vídeo normal debe informar LIKE/INDIFFERENT");
    }

    #[test]
    fn parser_de_playlist_innertube() {
        assert_eq!(parse_yt_duration("1:54"), 114);
        assert_eq!(parse_yt_duration("1:02:03"), 3723);
        assert_eq!(parse_yt_duration("NA"), 0);
        assert_eq!(parse_playlist_total("18 k vistas • 182 pistas • 13 horas"), Some(182));
        assert_eq!(parse_playlist_total("1.045 pistas"), Some(1045));
        assert_eq!(parse_playlist_total("18 k vistas"), None);
        let header = json!({
            "buttons": [{
                "toggleButtonRenderer": {
                    "defaultIcon": { "iconType": "BOOKMARK_BORDER" },
                    "isToggled": true
                }
            }]
        });
        assert_eq!(parse_playlist_saved(&header), Some(true));
        assert_eq!(parse_playlist_saved(&json!({ "buttons": [] })), None);
        assert!(valid_playlist_id("PLdMI5QqYLPnM"));
        assert!(valid_playlist_id("RDrFZHOHl-L8A"));
        assert!(!valid_playlist_id("VL PL con espacios"));
        // Renderer real simplificado: videoId + título + artista + duración.
        let item = json!({
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": [{ "text": "Mi canción" }] } } },
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": [{ "text": "Artista • Álbum" }] } } }
            ],
            "fixedColumns": [
                { "musicResponsiveListItemFixedColumnRenderer": {
                    "text": { "runs": [{ "text": "3:21" }] } } }
            ],
            "playlistItemData": { "videoId": "abc123" },
            "likeButtonRenderer": { "likeStatus": "LIKE", "likesAllowed": true }
        });
        let t = parse_playlist_track(&item).expect("pista");
        assert_eq!(t.id, "abc123");
        assert_eq!(t.title, "Mi canción");
        assert_eq!(t.artist, "Artista");
        assert_eq!(t.duration_secs, 201);
        assert_eq!(t.liked, Some(true));
        let indiferente = json!({
            "likeButtonRenderer": { "likeStatus": "INDIFFERENT", "likesAllowed": true }
        });
        assert_eq!(parse_like_status(&indiferente), Some(false));
        let no_permitido = json!({
            "likeButtonRenderer": { "likeStatus": "LIKE", "likesAllowed": false }
        });
        assert_eq!(parse_like_status(&no_permitido), None);
        // Un renderer sin videoId (separador) no inventa pista.
        assert!(parse_playlist_track(&json!({ "flexColumns": [] })).is_none());
    }
}
