//! Clientes HTTP de proveedores propios (fase 1). Solo APIs abiertas del usuario.
//! Decisión: Subsonic con `f=json` para evitar parser XML; Jellyfin con `api_key`
//! por query (simple y válido); podcasts con el crate `rss`.

use crate::db::Track;
use serde_json::Value;

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
}
