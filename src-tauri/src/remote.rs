//! Clientes HTTP de proveedores propios (fase 1). Solo APIs abiertas del usuario.
//! Decisión: Subsonic con `f=json` para evitar parser XML; Jellyfin con `api_key`
//! por query (simple y válido); podcasts con el crate `rss`.

use crate::db::Track;
use base64::Engine;
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Salt único por login (el servidor solo exige que varíe).
pub fn make_salt() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{n:012x}")
}

/// Token Subsonic: hex(md5(password + salt)).
pub fn make_token(password: &str, salt: &str) -> String {
    format!("{:x}", md5::compute(format!("{password}{salt}")))
}

// ---------------- Subsonic / Navidrome ----------------

pub struct SubsonicApi {
    pub source_id: i64,
    pub base_url: String,
    pub user: String,
    pub salt: String,
    pub token: String,
}

impl SubsonicApi {
    fn base(&self) -> String {
        self.base_url.trim_end_matches('/').to_string()
    }

    fn endpoint(&self, view: &str, params: &[(&str, &str)]) -> Result<reqwest::Url, String> {
        let mut u = reqwest::Url::parse(&format!("{}/rest/{view}.view", self.base()))
            .map_err(|e| format!("URL inválida: {e}"))?;
        u.query_pairs_mut()
            .extend_pairs([
                ("u", self.user.as_str()),
                ("t", self.token.as_str()),
                ("s", self.salt.as_str()),
                ("v", "1.16.1"),
                ("c", "musica"),
                ("f", "json"),
            ])
            .extend_pairs(params.iter().copied());
        Ok(u)
    }

    fn resp(response: &Value) -> Result<&Value, String> {
        let r = response
            .get("subsonic-response")
            .ok_or("respuesta no Subsonic")?;
        if r.get("status").and_then(|s| s.as_str()) == Some("ok") {
            Ok(r)
        } else {
            Err(r
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("error del servidor")
                .to_string())
        }
    }

    fn jstr(v: &Value, k: &str) -> String {
        v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    }

    fn ji64(v: &Value, k: &str) -> i64 {
        v.get(k).and_then(|x| x.as_i64()).unwrap_or(0)
    }

    /// Login con validación: construye credenciales y hace `ping`.
    pub async fn login(
        source_id: i64,
        base_url: &str,
        user: &str,
        password: &str,
    ) -> Result<Self, String> {
        let salt = make_salt();
        let api = Self {
            source_id,
            base_url: base_url.to_string(),
            user: user.to_string(),
            salt: salt.clone(),
            // El token debe calcularse con EL MISMO salt que se envía.
            token: make_token(password, &salt),
        };
        let body: Value = http()
            .get(api.endpoint("ping", &[]).map_err(|e| e.to_string())?)
            .send()
            .await
            .map_err(|e| format!("sin conexión: {e}"))?
            .json()
            .await
            .map_err(|e| format!("respuesta inválida: {e}"))?;
        Self::resp(&body)?;
        Ok(api)
    }

    /// `search3`: canciones como Tracks efímeros (`subsonic://{src}/{songId}`).
    pub async fn search_songs(&self, q: &str) -> Result<Vec<Track>, String> {
        let body: Value = http()
            .get(self.endpoint("search3", &[("query", q), ("songCount", "100")])?)
            .send()
            .await
            .map_err(|e| format!("búsqueda fallida: {e}"))?
            .json()
            .await
            .map_err(|e| format!("respuesta inválida: {e}"))?;
        let r = Self::resp(&body)?;
        // Subsonic devuelve objeto único cuando hay 1 resultado: acepta ambas formas.
        let songs: Vec<&Value> = match r.pointer("/searchResult3/song") {
            Some(Value::Array(a)) => a.iter().collect(),
            Some(o) if o.is_object() => vec![o],
            _ => vec![],
        };
        Ok(songs
            .iter()
            .filter_map(|s| {
                let id = Self::jstr(s, "id");
                if id.is_empty() {
                    return None;
                }
                let cover = Self::jstr(s, "coverArt");
                Some(Track {
                    id: 0,
                    path: format!("subsonic://{}/{id}", self.source_id),
                    title: {
                        let t = Self::jstr(s, "title");
                        if t.is_empty() { "Sin título".into() } else { t }
                    },
                    artist: Self::jstr(s, "artist"),
                    album: Self::jstr(s, "album"),
                    genre: Self::jstr(s, "genre"),
                    year: Self::ji64(s, "year") as i32,
                    track_no: Self::ji64(s, "track") as i32,
                    duration_secs: Self::ji64(s, "duration"),
                    mtime: 0,
                    art_url: if cover.is_empty() {
                        String::new()
                    } else {
                        self.cover_url(&cover)
                    },
                })
            })
            .collect())
    }

    pub fn stream_url(&self, song_id: &str) -> String {
        self.endpoint("stream", &[("id", song_id)])
            .map(|u| u.to_string())
            .unwrap_or_default()
    }

    pub fn cover_url(&self, cover_id: &str) -> String {
        self.endpoint("getCoverArt", &[("id", cover_id), ("size", "300")])
            .map(|u| u.to_string())
            .unwrap_or_default()
    }
}

// ---------------- Jellyfin ----------------

pub struct JellyfinApi {
    pub source_id: i64,
    pub base_url: String,
    pub token: String,
    pub user_id: String,
}

impl JellyfinApi {
    fn base(&self) -> String {
        self.base_url.trim_end_matches('/').to_string()
    }

    /// `POST /Users/AuthenticateByName` → (token, user_id).
    pub async fn login(
        source_id: i64,
        base_url: &str,
        user: &str,
        password: &str,
    ) -> Result<Self, String> {
        let base = base_url.trim_end_matches('/').to_string();
        let body: Value = http()
            .post(format!("{base}/Users/AuthenticateByName"))
            .header(
                "X-Emby-Authorization",
                "MediaBrowser Client=\"musica\", Device=\"desktop\", DeviceId=\"musica-desktop\", Version=\"0.1.0\"",
            )
            .json(&serde_json::json!({"Username": user, "Pw": password}))
            .send()
            .await
            .map_err(|e| format!("sin conexión: {e}"))?
            .json()
            .await
            .map_err(|e| format!("respuesta inválida: {e}"))?;
        let token = body
            .get("AccessToken")
            .and_then(|t| t.as_str())
            .ok_or("login rechazado (usuario/clave)")?
            .to_string();
        let user_id = body
            .pointer("/User/Id")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string();
        Ok(Self { source_id, base_url: base, token, user_id })
    }

    /// `/Search/Hints?IncludeItemTypes=Audio` → Tracks (`jellyfin://{src}/{itemId}`).
    pub async fn search_audio(&self, q: &str) -> Result<Vec<Track>, String> {
        let mut u = reqwest::Url::parse(&format!("{}/Search/Hints", self.base()))
            .map_err(|e| format!("URL inválida: {e}"))?;
        u.query_pairs_mut().extend_pairs([
            ("searchTerm", q),
            ("IncludeItemTypes", "Audio"),
            ("Limit", "100"),
            ("api_key", self.token.as_str()),
        ]);
        let body: Value = http()
            .get(u)
            .send()
            .await
            .map_err(|e| format!("búsqueda fallida: {e}"))?
            .json()
            .await
            .map_err(|e| format!("respuesta inválida: {e}"))?;
        let items = body
            .get("Items")
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(items
            .iter()
            .filter(|it| it.get("Type").and_then(|t| t.as_str()) == Some("Audio"))
            .filter_map(|it| {
                let id = it.get("Id").and_then(|x| x.as_str())?;
                let artists = it
                    .get("Artists")
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")
                    })
                    .unwrap_or_default();
                let album_id = it.get("AlbumId").and_then(|x| x.as_str()).unwrap_or("");
                Some(Track {
                    id: 0,
                    path: format!("jellyfin://{}/{id}", self.source_id),
                    title: it
                        .get("Name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("Sin título")
                        .to_string(),
                    artist: artists,
                    album: it
                        .get("Album")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    genre: String::new(),
                    year: 0,
                    track_no: 0,
                    duration_secs: it
                        .get("RunTimeTicks")
                        .and_then(|x| x.as_i64())
                        .map(|t| t / 10_000_000)
                        .unwrap_or(0),
                    mtime: 0,
                    art_url: if album_id.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "{}/Items/{album_id}/Images/Primary?fillWidth=300&api_key={}",
                            self.base(),
                            self.token
                        )
                    },
                })
            })
            .collect())
    }

    pub fn stream_url(&self, item_id: &str) -> String {
        format!(
            "{}/Audio/{item_id}/universal?userId={}&audioCodec=mp3&api_key={}",
            self.base(),
            self.user_id,
            self.token
        )
    }
}

// ---------------- Podcasts RSS ----------------

/// Descarga un feed y devuelve sus episodios como Tracks (mpv reproduce el enclosure).
pub async fn fetch_episodes(feed_url: &str) -> Result<Vec<Track>, String> {
    if !feed_url.starts_with("http://") && !feed_url.starts_with("https://") {
        return Err("el feed debe ser http(s)".into());
    }
    let bytes = http()
        .get(feed_url)
        .send()
        .await
        .map_err(|e| format!("feed inaccesible: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("descarga fallida: {e}"))?;
    Ok(episodes_of(&bytes, feed_url))
}

fn episodes_of(bytes: &[u8], _feed_url: &str) -> Vec<Track> {
    let ch = match rss::Channel::read_from(bytes) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let show = ch.title().to_string();
    let fallback_art = ch
        .itunes_ext()
        .and_then(|e| e.image())
        .map(str::to_string)
        .or_else(|| ch.image().map(|i| i.url().to_string()))
        .unwrap_or_default();
    ch.items()
        .iter()
        .filter_map(|it| {
            let url = it.enclosure()?.url().to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return None;
            }
            Some(Track {
                id: 0,
                path: url,
                title: it.title().unwrap_or("Episodio").to_string(),
                artist: show.clone(),
                album: show.clone(),
                genre: "podcast".into(),
                year: 0,
                track_no: 0,
                duration_secs: it
                    .itunes_ext()
                    .and_then(|e| e.duration())
                    .map(parse_itunes_duration)
                    .unwrap_or(0),
                mtime: 0,
                art_url: it
                    .itunes_ext()
                    .and_then(|e| e.image())
                    .map(str::to_string)
                    .unwrap_or_else(|| fallback_art.clone()),
            })
        })
        .collect()
}

/// Duración iTunes: "SS" | "MM:SS" | "HH:MM:SS".
fn parse_itunes_duration(s: &str) -> i64 {
    let parts: Vec<i64> = s.split(':').filter_map(|p| p.trim().parse().ok()).collect();
    match parts.len() {
        1 => parts[0],
        2 => parts[0] * 60 + parts[1],
        3 => parts[0] * 3600 + parts[1] * 60 + parts[2],
        _ => 0,
    }
}


// ---------------- Radios / Radio Garden ----------------

/// Resultado de resolver un enlace de radio a algo reproducible por mpv.
#[derive(Debug, Clone)]
pub struct RadioResolved {
    pub url: String,
    pub name: String,
    pub art_url: String,
    pub place: String,
}

const RADIO_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Safari/537.36";

fn radio_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(RADIO_UA)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Normaliza una URL de radio: añade https:// si falta y exige http(s).
fn normalize_radio_url(raw: &str) -> Result<reqwest::Url, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("la URL de la radio está vacía".into());
    }
    let with_scheme = if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    let u = reqwest::Url::parse(&with_scheme)
        .map_err(|_| "la URL de la radio no es válida".to_string())?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return Err("la radio debe usar http(s)".into());
    }
    Ok(u)
}

/// Extrae el id de una emisora de Radio Garden:
/// - https://radio.garden/listen/<slug>/<id>
/// - https://radio.garden/visit/<slug>/<id>
/// - https://radio.garden/api/ara/content/listen/<id>/channel.mp3
/// Devuelve None para /settings, la portada o enlaces sin emisora.
pub fn radio_garden_uid(raw: &str) -> Option<String> {
    let u = reqwest::Url::parse(raw).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    if host != "radio.garden" && !host.ends_with(".radio.garden") {
        return None;
    }
    let segs: Vec<&str> = u.path().split('/').filter(|s| !s.is_empty()).collect();
    let pos = segs.iter().position(|s| *s == "listen" || *s == "visit")?;
    let valid = |s: &str| {
        s.len() >= 4
            && s.len() <= 24
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    if pos == 0 {
        // /listen/<slug>/<id> o /visit/<slug>/<id>
        if segs.len() >= 3 {
            let id = segs[segs.len() - 1];
            return valid(id).then(|| id.to_string());
        }
        return None;
    }
    // /api/ara/content/listen/<id>/channel.mp3
    segs[pos + 1..]
        .iter()
        .find(|s| valid(s) && !s.starts_with("channel."))
        .map(|s| s.to_string())
}

/// Sigue redirecciones SOLO dentro de radio.garden. La primera redirección
/// (307/302) apunta al stream real; no se descarga audio para resolverla.
async fn resolve_radio_redirects(url: &str) -> String {
    let mut current = url.to_string();
    for _ in 0..4 {
        if !current.contains("radio.garden") {
            break;
        }
        let Ok(resp) = radio_client()
            .get(&current)
            .header(reqwest::header::ACCEPT, "audio/*,*/*")
            .send()
            .await
        else {
            break;
        };
        if !resp.status().is_redirection() {
            break;
        }
        let Some(loc) = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
        else {
            break;
        };
        current = if loc.starts_with("http://") || loc.starts_with("https://") {
            loc.to_string()
        } else if let Ok(base) = reqwest::Url::parse(&current) {
            base.join(loc).map(|u| u.to_string()).unwrap_or_else(|_| loc.to_string())
        } else {
            break;
        };
    }
    current
}

fn default_radio_name(host: &str) -> String {
    let h = host.trim();
    if h.is_empty() { "Radio".to_string() } else { format!("Radio {h}") }
}

/// Resuelve un enlace de radio a un stream reproducible.
/// - Radio Garden: consulta su endpoint interno y devuelve el stream real.
/// - URL directa: normaliza y rechaza páginas HTML (error claro).

fn page_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(RADIO_UA)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#x2F;", "/")
        .replace("&#x2f;", "/")
        .replace("&#x27;", "'")
}

fn normalize_candidate(base: &reqwest::Url, raw: &str) -> Option<String> {
    let c = html_unescape(raw.trim()).trim_end_matches(';').trim().to_string();
    if c.is_empty() {
        return None;
    }
    let u = if c.starts_with("http://") || c.starts_with("https://") {
        reqwest::Url::parse(&c).ok()?
    } else {
        base.join(&c).ok()?
    };
    if u.scheme() != "http" && u.scheme() != "https" {
        return None;
    }
    Some(u.to_string())
}

/// Igual que `normalize_candidate`, pero conserva el `;` final: los servidores
/// Shoutcast usan `http://host:puerto/;` como endpoint del stream.
fn normalize_candidate_keep_semicolon(base: &reqwest::Url, raw: &str) -> Option<String> {
    let c = html_unescape(raw.trim()).to_string();
    if c.is_empty() {
        return None;
    }
    let u = if c.starts_with("http://") || c.starts_with("https://") {
        reqwest::Url::parse(&c).ok()?
    } else {
        base.join(&c).ok()?
    };
    if u.scheme() != "http" && u.scheme() != "https" {
        return None;
    }
    Some(u.to_string())
}

fn looks_like_stream_url(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    [".mp3", ".aac", ".ogg", ".opus", ".m3u8", ".pls", ".m3u"]
        .iter()
        .any(|e| l.contains(e))
        || l.contains("/stream")
        || l.contains("/listen")
        || l.contains(":8000")
        || l.contains(":8080")
        || l.contains("airtime.pro")
}

fn is_known_page(u: &reqwest::Url) -> bool {
    let h = u.host_str().unwrap_or("").to_ascii_lowercase();
    h.ends_with("streema.com")
        || h.ends_with("liveonlineradio.net")
        || h.ends_with("onlineradiobox.com")
        || h.ends_with("radio.net")
}

/// Valor de un atributo HTML (`attr="..."` o `attr='...'`).
fn attr_value(fragment: &str, attr: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let key = format!("{attr}={quote}");
        if let Some(i) = fragment.find(&key) {
            let start = i + key.len();
            if let Some(end) = fragment[start..].find(quote) {
                return Some(fragment[start..start + end].to_string());
            }
        }
    }
    None
}

fn find_tag_src(html: &str, tag: &str) -> Option<String> {
    let needle = format!("<{tag}");
    let mut pos = 0;
    while let Some(i) = html[pos..].find(&needle) {
        let start = pos + i;
        let end = html[start..].find('>').map(|e| start + e).unwrap_or(html.len());
        if let Some(v) = attr_value(&html[start..end], "src") {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
        pos = end.saturating_add(1);
        if pos >= html.len() {
            break;
        }
    }
    None
}

fn json_string(html: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let i = html.find(&needle)?;
    let rest = &html[i + needle.len()..];
    let colon = rest.find(':')?;
    let rest = &rest[colon + 1..];
    let q = rest.find(|c| c == '"' || c == '\'')?;
    let quote = rest[q..].chars().next()?;
    let rest = &rest[q + 1..];
    let end = rest.find(quote)?;
    Some(html_unescape(&rest[..end]))
}

fn decode_zlib_b64(s: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()?;
    let mut d = flate2::read::ZlibDecoder::new(&bytes[..]);
    let mut out = String::new();
    d.read_to_string(&mut out).ok()?;
    let out = out.trim().to_string();
    out.starts_with("http").then_some(out)
}

/// Streema guarda el stream en un payload Nuxt comprimido (base64 + zlib).
fn streema_stream(html: &str) -> Option<String> {
    let i = html.find("id=\"__NUXT_DATA__\"")?;
    let start = html[i..].find('>')? + i + 1;
    let end = html[start..].find("</script>")? + start;
    let data: Value = serde_json::from_str(&html[start..end]).ok()?;
    let arr = data.as_array()?;
    for obj in arr.iter().filter_map(|v| v.as_object()) {
        if let Some(idx) = obj.get("encoded_stream_url").and_then(|v| v.as_u64()) {
            if let Some(enc) = arr.get(idx as usize).and_then(|v| v.as_str()) {
                if let Some(s) = decode_zlib_b64(enc) {
                    return Some(s);
                }
            }
        }
    }
    None
}

fn page_title(html: &str) -> String {
    if let Some(i) = html.find("<title") {
        if let Some(s) = html[i..].find('>') {
            let st = i + s + 1;
            if let Some(e) = html[st..].find("</title>") {
                return html_unescape(html[st..st + e].trim());
            }
        }
    }
    String::new()
}

fn og_image(html: &str) -> String {
    for key in ["property=\"og:image\"", "name=\"twitter:image\"", "property='og:image'"] {
        if let Some(i) = html.find(key) {
            let frag = &html[i..(i + 600).min(html.len())];
            if let Some(c) = attr_value(frag, "content") {
                return html_unescape(&c);
            }
        }
    }
    String::new()
}

fn find_generic_urls(html: &str, base: &reqwest::Url) -> Option<String> {
    let mut pos = 0;
    while let Some(i) = html[pos..].find("http") {
        let start = pos + i;
        let rest = &html[start..];
        let end = rest
            .find(|c: char| c.is_whitespace() || "\"'<>)".contains(c))
            .unwrap_or(rest.len());
        if let Some(u) = normalize_candidate(base, &rest[..end]) {
            if looks_like_stream_url(&u) {
                return Some(u);
            }
        }
        pos = start + 4;
        if pos >= html.len() {
            break;
        }
    }
    None
}

/// Busca un stream dentro de una página web (LiveOnlineRadio, Streema, etc.).
fn extract_page_stream(html: &str, base: &reqwest::Url) -> Option<String> {
    // 1) Reproductores que declaran el stream directamente.
    if let Some(raw) = attr_value(html, "data-stream") {
        if let Some(u) = normalize_candidate_keep_semicolon(base, &raw) {
            return Some(u);
        }
    }
    // 2) <audio src>, <source src>
    for tag in ["audio", "source"] {
        if let Some(raw) = find_tag_src(html, tag) {
            if let Some(u) = normalize_candidate(base, &raw) {
                return Some(u);
            }
        }
    }
    // 3) Streema: payload Nuxt comprimido.
    if let Some(s) = streema_stream(html) {
        return Some(s);
    }
    // 4) JSON típico de reproductores.
    for key in ["stream_url", "streamUrl", "audio_url", "audioUrl", "stream"] {
        if let Some(raw) = json_string(html, key) {
            if let Some(u) = normalize_candidate(base, &raw) {
                if raw.starts_with("http") || looks_like_stream_url(&u) {
                    return Some(u);
                }
            }
        }
    }
    // 5) Cualquier URL con extensión de audio.
    find_generic_urls(html, base)
}

async fn resolve_html_page(u: &reqwest::Url, name: &str) -> Result<RadioResolved, String> {
    let resp = page_client()
        .get(u.to_string())
        .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml,*/*")
        .send()
        .await
        .map_err(|e| format!("no se pudo abrir la página: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("la página respondió {}", resp.status()));
    }
    let body = resp
        .text()
        .await
        .map_err(|_| "no se pudo leer la página".to_string())?;
    let body = if body.len() > 2_000_000 {
        body.chars().take(2_000_000).collect()
    } else {
        body
    };
    let stream = extract_page_stream(&body, u).ok_or_else(|| {
        "no encontré un stream de audio en esa página; abre la web y usa el enlace directo del reproductor"
            .to_string()
    })?;
    let title = page_title(&body);
    let art = og_image(&body);
    let resolved_name = if name.trim().is_empty() {
        if title.is_empty() {
            default_radio_name(u.host_str().unwrap_or(""))
        } else {
            title
        }
    } else {
        name.trim().to_string()
    };
    Ok(RadioResolved { url: stream, name: resolved_name, art_url: art, place: String::new() })
}

async fn resolve_page_or_stream(u: &reqwest::Url, name: &str) -> Result<RadioResolved, String> {
    let head = page_client()
        .head(u.to_string())
        .header(reqwest::header::ACCEPT, "audio/*,text/html,*/*")
        .send()
        .await;
    if let Ok(r) = head {
        if r.status().is_success() {
            let ct = r
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ct.contains("text/html") || ct.contains("application/xhtml") {
                return resolve_html_page(u, name).await;
            }
            let resolved_name = if name.trim().is_empty() {
                default_radio_name(u.host_str().unwrap_or(""))
            } else {
                name.trim().to_string()
            };
            return Ok(RadioResolved {
                url: u.to_string(),
                name: resolved_name,
                art_url: String::new(),
                place: String::new(),
            });
        }
        if r.status().as_u16() == 404 || r.status().as_u16() == 410 {
            return Err(format!("la emisora no existe ({})", r.status()));
        }
    }
    // Si es un directorio conocido, la página puede no responder a HEAD.
    if is_known_page(u) {
        return resolve_html_page(u, name).await;
    }
    let resolved_name = if name.trim().is_empty() {
        default_radio_name(u.host_str().unwrap_or(""))
    } else {
        name.trim().to_string()
    };
    Ok(RadioResolved {
        url: u.to_string(),
        name: resolved_name,
        art_url: String::new(),
        place: String::new(),
    })
}

/// Limpia URLs de Streema con texto pegado detrás del slug.
fn normalize_provider_page(u: reqwest::Url) -> reqwest::Url {
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    if host.ends_with("streema.com") && u.path().starts_with("/radios/") {
        let rest = u.path().trim_start_matches("/radios/");
        let slug = rest
            .split(|c| c == '/' || c == ' ')
            .next()
            .unwrap_or(rest);
        let slug = slug.split("%20").next().unwrap_or(slug).trim();
        if !slug.is_empty() {
            if let Ok(clean) = reqwest::Url::parse(&format!("https://{host}/radios/{slug}")) {
                return clean;
            }
        }
    }
    u
}

pub async fn resolve_radio(input: &str, name: &str) -> Result<RadioResolved, String> {
    let u = normalize_provider_page(normalize_radio_url(input)?);
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();

    if let Some(uid) = radio_garden_uid(u.as_str()) {
        let api = format!("https://radio.garden/api/ara/content/channel/{uid}");
        let resp = radio_client()
            .get(&api)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| format!("no se pudo contactar con Radio Garden: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "Radio Garden no devolvió la emisora ({}); pega el stream directo si persiste",
                resp.status()
            ));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|_| "Radio Garden devolvió una respuesta no válida".to_string())?;
        let data = v.get("data").cloned().unwrap_or(Value::Null);
        let title = data.get("title").and_then(|x| x.as_str()).unwrap_or("").trim();
        let place = data.pointer("/place/title").and_then(|x| x.as_str()).unwrap_or("").trim();
        let country = data.pointer("/country/title").and_then(|x| x.as_str()).unwrap_or("").trim();
        let place_id = data.pointer("/place/id").and_then(|x| x.as_str()).unwrap_or("").trim();
        let fmt = data.get("format").and_then(|x| x.as_str()).unwrap_or("mp3").trim();
        let fmt = if !fmt.is_empty() && fmt.chars().all(|c| c.is_ascii_alphanumeric()) {
            fmt
        } else {
            "mp3"
        };
        let proxy = format!("https://radio.garden/api/ara/content/listen/{uid}/channel.{fmt}");
        let url = resolve_radio_redirects(&proxy).await;
        let art_url = if place_id.is_empty() {
            String::new()
        } else {
            format!("https://px.radio.garden/small/{place_id}.jpg")
        };
        let place_label = [place, country]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        let resolved_name = if name.trim().is_empty() {
            if title.is_empty() { "Radio Garden".to_string() } else { title.to_string() }
        } else {
            name.trim().to_string()
        };
        return Ok(RadioResolved { url, name: resolved_name, art_url, place: place_label });
    }

    if host == "radio.garden" || host.ends_with(".radio.garden") {
        return Err(
            "ese enlace de Radio Garden no es una emisora; usa un enlace /listen/<emisora>/<id>"
                .into(),
        );
    }

    // Stream directo o página de un directorio (Streema, LiveOnlineRadio…).
    resolve_page_or_stream(&u, name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_formato_subsonic() {
        let t = make_token("pass", "abc123");
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(t, make_token("pass", "abc123")); // determinista
    }

    #[test]
    fn urls_stream_y_cover() {
        let api = SubsonicApi {
            source_id: 7,
            base_url: "https://musica.local/navidrome/".into(),
            user: "u".into(),
            salt: "s".into(),
            token: "t".into(),
        };
        let st = api.stream_url("42");
        assert!(st.contains("/rest/stream.view") && st.contains("id=42") && st.contains("u=u"));
        let cv = api.cover_url("al-1");
        assert!(cv.contains("/rest/getCoverArt.view") && cv.contains("id=al-1"));
        let jf = JellyfinApi {
            source_id: 3,
            base_url: "http://jf:8096".into(),
            token: "tok".into(),
            user_id: "uid".into(),
        };
        let js = jf.stream_url("abc");
        assert!(js.contains("/Audio/abc/universal") && js.contains("api_key=tok"));
    }

    #[test]
    fn duracion_itunes() {
        assert_eq!(parse_itunes_duration("90"), 90);
        assert_eq!(parse_itunes_duration("04:20"), 260);
        assert_eq!(parse_itunes_duration("1:02:03"), 3723);
        assert_eq!(parse_itunes_duration("basura"), 0);
    }

    #[test]
    fn rss_episodios() {
        let xml = br#"<?xml version="1.0"?>
        <rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
        <channel><title>Show</title>
        <item><title>Ep 1</title>
          <enclosure url="https://cdn.local/ep1.mp3" length="1" type="audio/mpeg"/>
          <itunes:duration>04:20</itunes:duration></item>
        <item><title>Sin audio</title><description>nada</description></item>
        </channel></rss>"#;
        let eps = episodes_of(xml, "https://f.local/rss");
        assert_eq!(eps.len(), 1); // el item sin enclosure se descarta
        assert_eq!(eps[0].path, "https://cdn.local/ep1.mp3");
        assert_eq!(eps[0].duration_secs, 260);
        assert_eq!(eps[0].album, "Show");
    }

    #[test]
    fn radio_garden_uid_extrae_emisoras() {
        assert_eq!(
            radio_garden_uid("https://radio.garden/listen/live-fm/QIfHrHjn"),
            Some("QIfHrHjn".into())
        );
        assert_eq!(
            radio_garden_uid("https://radio.garden/visit/live-fm/QIfHrHjn"),
            Some("QIfHrHjn".into())
        );
        assert_eq!(
            radio_garden_uid("https://radio.garden/api/ara/content/listen/QIfHrHjn/channel.mp3"),
            Some("QIfHrHjn".into())
        );
        assert_eq!(radio_garden_uid("https://radio.garden/settings?hl=es"), None);
        assert_eq!(radio_garden_uid("https://radio.garden/listen/solo-slug"), None);
    }

    #[test]
    fn normaliza_radio_sin_esquema() {
        let u = normalize_radio_url("radio.garden/listen/x/QIfHrHjn").unwrap();
        assert_eq!(u.scheme(), "https");
        let u = normalize_radio_url("http://stream.example/radio.mp3").unwrap();
        assert_eq!(u.as_str(), "http://stream.example/radio.mp3");
        assert!(normalize_radio_url("   ").is_err());
    }

    #[tokio::test]
    #[ignore = "red: resuelve una emisora real de Radio Garden"]
    async fn radio_garden_resuelve_emisora() {
        let r = resolve_radio("https://radio.garden/listen/live-fm/QIfHrHjn", "")
            .await
            .expect("Radio Garden debe resolver");
        assert!(r.url.starts_with("http"), "stream: {}", r.url);
        assert!(!r.name.is_empty(), "nombre de emisora");
    }

    #[test]
    fn streema_normaliza_url_y_decodifica_stream() {
        let u = normalize_provider_page(
            reqwest::Url::parse(
                "https://streema.com/radios/Shonan_BeachFM_78.9%20Listen%20to%20FM",
            )
            .unwrap(),
        );
        assert_eq!(u.as_str(), "https://streema.com/radios/Shonan_BeachFM_78.9");
        let dec = decode_zlib_b64(
            "eJzLKCkpKLbS1y/OyM9LzEtKTUzOSMvVyy8t0UvMLCrJzE3VKyjKR5WNTwQAJB8Ucw==",
        );
        assert_eq!(
            dec.as_deref(),
            Some("https://shonanbeachfm.out.airtime.pro/shonanbeachfm_a")
        );
    }

    #[test]
    fn pagina_extrae_data_stream() {
        let html = r#"<div id="a-player" data-stream="https://kathy.torontocast.com:3060/;"></div>"#;
        let base = reqwest::Url::parse("https://liveonlineradio.net/es/kawaii-radio").unwrap();
        assert_eq!(
            extract_page_stream(html, &base).as_deref(),
            Some("https://kathy.torontocast.com:3060/;")
        );
    }

    #[tokio::test]
    #[ignore = "red: resuelve una página real de Streema"]
    async fn streema_resuelve_pagina() {
        let r = resolve_radio("https://streema.com/radios/Shonan_BeachFM_78.9", "FM")
            .await
            .expect("Streema debe resolver");
        assert!(r.url.contains("airtime.pro"), "stream: {}", r.url);
    }

    #[tokio::test]
    #[ignore = "red: resuelve una página real de LiveOnlineRadio"]
    async fn liveonlineradio_resuelve_pagina() {
        let r = resolve_radio("https://liveonlineradio.net/es/kawaii-radio", "Kawaii")
            .await
            .expect("LiveOnlineRadio debe resolver");
        assert!(r.url.ends_with("/;"), "stream Shoutcast: {}", r.url);
    }
}
