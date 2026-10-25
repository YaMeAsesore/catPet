//! Motor de audio nativo Spotify (librespot 0.8) — reemplaza al Web Playback SDK.
//!
//! Responsabilidades:
//! - Verificar Premium ANTES de inicializar librespot (una cuenta Free llama a
//!   `exit(1)` dentro de librespot `check_catalogue`: mataría la app entera).
//! - Autenticar: reutiliza el token PKCE en un primer Connect y deriva a
//!   credenciales reusable (user+blob) que se persisten en el keyring.
//! - Configurar SessionConfig + ConnectConfig (nombre/icono/volumen) y el
//!   backend de audio nativo por plataforma (ALSA/Pulse en Linux, WASAPI/CoreAudio
//!   vía rodio en Windows/macOS).
//! - Exponer el control de reproducción vía el handle público de Spirc.
//!
//! Limitación de hilo: librespot exige un runtime tokio activo (`Session::new`
//! usa `Handle::current`), por eso `connect` es async y se invoca desde Tauri.

use crate::keyring::{self, SERVICE_DEVICE, SERVICE_PKCE, SERVICE_SPOTIFY};
use crate::spotify_policy::{AccountTier, PolicyDenial, SharedPolicy, shared_policy};
use librespot::connect::{ConnectConfig, LoadRequest, LoadRequestOptions, PlayingTrack, Spirc};
use librespot::core::{
    Session, SessionConfig, SpotifyUri, authentication::Credentials, cache::Cache,
    config::DeviceType,
};
use librespot::playback::audio_backend::{self, Sink};
use librespot::playback::{
    config::{AudioFormat, Bitrate, NormalisationType, PlayerConfig},
    mixer::{Mixer, MixerConfig, softmixer::SoftMixer},
    player::{Player, PlayerEvent},
};
use librespot::metadata::{Metadata, Track as SpTrack};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::AppHandle;

/// Backends nativos elegibles. `Auto` resuelve por plataforma en tiempo de build.
/// Decisión: ALSA primero y Pulse como alternativa en Linux; rodio cubre
/// WASAPI (Windows) y CoreAudio (macOS) con un solo backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioBackend {
    Auto,
    Alsa,
    PulseAudio,
    Rodio,
}

/// Todos los backends declarados (para la cadena de reserva).
const ALL_BACKENDS: [AudioBackend; 4] = [
    AudioBackend::Auto,
    AudioBackend::Alsa,
    AudioBackend::PulseAudio,
    AudioBackend::Rodio,
];

impl AudioBackend {
    /// Nombre que espera `audio_backend::find` (solo los compilados existen).
    fn sink_name(self) -> Option<&'static str> {
        #[cfg(target_os = "linux")]
        {
            match self {
                AudioBackend::Auto | AudioBackend::Alsa => Some("alsa"),
                AudioBackend::PulseAudio => Some("pulseaudio"),
                AudioBackend::Rodio => None,
            }
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            match self {
                AudioBackend::Auto | AudioBackend::Rodio => Some("rodio"),
                _ => None,
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = self;
            None
        }
    }

    fn sink_builder(self) -> Option<audio_backend::SinkBuilder> {
        self.sink_name().and_then(|n| audio_backend::find(Some(n.to_string())))
    }

    /// Backend efectivamente resoluble en esta plataforma (con reserva).
    pub fn resolved(self) -> AudioBackend {
        if self.sink_builder().is_some() {
            return self;
        }
        for alt in ALL_BACKENDS {
            if alt != self && alt.sink_builder().is_some() {
                return alt;
            }
        }
        self // ninguno disponible: se usará sink mudo al construir
    }
}

/// Configuración completa del motor Spotify.
#[derive(Debug, Clone)]
pub struct SpotifyConfig {
    /// Nombre visible en la lista de dispositivos Connect.
    pub device_name: String,
    pub backend: AudioBackend,
    /// Bitrate de Spotify (96/160/320).
    pub bitrate_kbps: u32,
    /// Volumen inicial 0..100.
    pub initial_volume: u8,
    /// Normalización loudness (equivale al ReplayGain de mpv).
    pub normalisation: bool,
    /// Dispositivo ALSA/Pulse explícito; None = "default".
    pub device_out: Option<String>,
}

impl Default for SpotifyConfig {
    fn default() -> Self {
        Self {
            device_name: "Música".to_string(),
            backend: AudioBackend::Auto,
            bitrate_kbps: 160,
            initial_volume: 80,
            normalisation: true,
            device_out: None,
        }
    }
}

/// Fallo de autenticación clasificado para mensajes claros en la UI.
/// Decisión: enum propio porque `librespot::core::Error` solo trae kind genérico
/// y el motivo real del servidor viaja en el texto ("Premium account required",
/// "Bad credentials"…). `classify_login_message` es pura y testeable.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthFailure {
    /// El servidor exige Premium (o el gate detectó cuenta Free).
    FreeAccount,
    /// Token PKCE inválido o caducado (~1h de vida).
    BadToken,
    /// Sin red / AP inaccesible / timeout.
    Network,
    /// Otro rechazo del servidor (lleva el detalle).
    Server(String),
}

impl AuthFailure {
    pub fn user_message(&self) -> String {
        match self {
            AuthFailure::FreeAccount => "Esta cuenta de Spotify es gratuita (Free): Spotify \
                Connect (librespot) requiere Premium y el servidor rechaza la cuenta (este \
                motor no reproduce anuncios). Alternativa: importa tus playlists a YouTube \
                Music y reprodúcelas con el motor local."
                .into(),
            AuthFailure::BadToken => "El token de Spotify no es válido o caducó (dura ~1 hora). \
                Vuelve a iniciar sesión y pega un token nuevo."
                .into(),
            AuthFailure::Network => "No se pudo contactar con los servidores de Spotify. \
                Revisa tu conexión e inténtalo de nuevo."
                .into(),
            AuthFailure::Server(d) => format!("Spotify rechazó la conexión: {d}"),
        }
    }
}

/// Clasifica el motivo del servidor a partir de su texto (puro, testeable).
/// Textos reales de `connection::login_error_message` en librespot 0.8.
pub fn classify_login_message(msg: &str) -> AuthFailure {
    if msg.contains("Premium account required") {
        AuthFailure::FreeAccount
    } else if msg.contains("Bad credentials") || msg.contains("Could not validate credentials") {
        AuthFailure::BadToken
    } else if msg.contains("transport returned no data")
        || msg.contains("Try another access point")
        || msg.contains("timed out")
        || msg.contains("connection closed")
    {
        AuthFailure::Network
    } else {
        AuthFailure::Server(msg.chars().take(160).collect())
    }
}

/// Clasifica un `librespot::core::Error` completo.
/// `ErrorKind` no se re-exporta en el crate umbrella, así que el kind se lee
/// como entero (PermissionDenied=7, Unauthenticated=16, Unavailable=14).
pub fn classify_auth_error(err: &librespot::core::Error) -> AuthFailure {
    let first = classify_login_message(&err.to_string());
    if matches!(first, AuthFailure::Server(_)) {
        match err.kind as i32 {
            7 | 16 => AuthFailure::BadToken,
            14 => AuthFailure::Network,
            _ => first,
        }
    } else {
        first
    }
}

/// Resultado del gate de cuenta Premium (puro, testeable sin red).
#[derive(Debug, Clone, PartialEq)]
pub enum AccountGate {
    Premium,
    NotPremium(String),
    Unknown,
}

/// Decide si se permite inicializar librespot a partir del `product` de /v1/me.
/// Decisión: denegar por defecto — `check_catalogue` de librespot hace `exit(1)`
/// con cuentas Free si llega a autenticarse, así que solo seguimos con Premium
/// confirmado. `Unknown` (token sin scope o campo ausente) también se deniega
/// porque no podemos descartar que sea Free.
pub fn account_gate(product: Option<&str>) -> AccountGate {
    match product.map(|p| p.to_ascii_lowercase()) {
        Some(p) if p == "premium" => AccountGate::Premium,
        Some(p) if p == "free" || p == "open" => AccountGate::NotPremium(p),
        _ => AccountGate::Unknown,
    }
}

/// true si el gate permite arrancar librespot (Premium confirmado).
/// Decisión: denegar por defecto — permitir Unknown expondría a la app al
/// `exit(1)` de librespot si la cuenta resultara ser Free.
pub fn gate_allows(gate: &AccountGate) -> bool {
    matches!(gate, AccountGate::Premium)
}

/// Mapeo UI% (0..=100) ↔ volumen u16 de Spirc (0..=65535).
pub fn ui_to_u16(pct: u8) -> u16 {
    (pct.min(100) as u32 * 655) as u16
}

/// Mapeo inverso: volumen u16 de PlayerEvent::VolumeChanged → UI%.
pub fn u16_to_ui(raw: u16) -> u8 {
    ((raw as u32 * 100) / 65535).min(100) as u8
}

/// Evento de reproducción normalizado del motor Spotify (independiente de librespot).
/// El PlaybackRouter consume esto igual que el estado de mpv.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteEvent {
    Playing { uri: String, position_ms: u32 },
    Paused { uri: String, position_ms: u32 },
    Stopped,
    Loading { uri: String },
    Unavailable { uri: String },
    EndOfTrack { uri: String },
    Volume { percent: u8 },
    Position { uri: String, position_ms: u32 },
    Seeked { uri: String, position_ms: u32 },
    TrackChanged { uri: String },
    SessionDisconnected,
}

impl RouteEvent {
    /// Nombre del evento IPC para el frontend (para `app.emit`).
    pub fn ipc_kind(&self) -> &'static str {
        match self {
            RouteEvent::Playing { .. } => "playing",
            RouteEvent::Paused { .. } => "paused",
            RouteEvent::Stopped => "stopped",
            RouteEvent::Loading { .. } => "loading",
            RouteEvent::Unavailable { .. } => "unavailable",
            RouteEvent::EndOfTrack { .. } => "ended",
            RouteEvent::Volume { .. } => "volume",
            RouteEvent::Position { .. } => "position",
            RouteEvent::Seeked { .. } => "seeked",
            RouteEvent::TrackChanged { .. } => "track-changed",
            RouteEvent::SessionDisconnected => "session-disconnected",
        }
    }

    /// URI del track implicado, si aplica.
    pub fn uri(&self) -> Option<&str> {
        match self {
            RouteEvent::Playing { uri, .. }
            | RouteEvent::Paused { uri, .. }
            | RouteEvent::Loading { uri }
            | RouteEvent::Unavailable { uri }
            | RouteEvent::EndOfTrack { uri }
            | RouteEvent::Position { uri, .. }
            | RouteEvent::Seeked { uri, .. }
            | RouteEvent::TrackChanged { uri } => Some(uri),
            _ => None,
        }
    }
}

/// Traduce un `PlayerEvent` de librespot al evento del router.
/// Pura: se prueba con eventos construidos a mano (mocks del canal).
pub fn map_player_event(ev: &PlayerEvent) -> Option<RouteEvent> {
    let uri = |u: &SpotifyUri| u.to_string();
    match ev {
        PlayerEvent::Playing { track_id, position_ms, .. } => Some(RouteEvent::Playing {
            uri: uri(track_id),
            position_ms: *position_ms,
        }),
        PlayerEvent::Paused { track_id, position_ms, .. } => Some(RouteEvent::Paused {
            uri: uri(track_id),
            position_ms: *position_ms,
        }),
        PlayerEvent::Stopped { .. } => Some(RouteEvent::Stopped),
        PlayerEvent::Loading { track_id, .. } => {
            Some(RouteEvent::Loading { uri: uri(track_id) })
        }
        PlayerEvent::Unavailable { track_id, .. } => {
            Some(RouteEvent::Unavailable { uri: uri(track_id) })
        }
        PlayerEvent::EndOfTrack { track_id, .. } => {
            Some(RouteEvent::EndOfTrack { uri: uri(track_id) })
        }
        PlayerEvent::VolumeChanged { volume } => {
            Some(RouteEvent::Volume { percent: u16_to_ui(*volume) })
        }
        PlayerEvent::PositionChanged { track_id, position_ms, .. } => {
            Some(RouteEvent::Position { uri: uri(track_id), position_ms: *position_ms })
        }
        PlayerEvent::PositionCorrection { track_id, position_ms, .. } => {
            Some(RouteEvent::Position { uri: uri(track_id), position_ms: *position_ms })
        }
        PlayerEvent::Seeked { track_id, position_ms, .. } => {
            Some(RouteEvent::Seeked { uri: uri(track_id), position_ms: *position_ms })
        }
        PlayerEvent::TrackChanged { audio_item, .. } => Some(RouteEvent::TrackChanged {
            uri: audio_item.track_id.to_string(),
        }),
        PlayerEvent::SessionDisconnected { .. } => Some(RouteEvent::SessionDisconnected),
        _ => None,
    }
}

/// Atribución visible del track actual (artista, álbum, carátula).
/// Se emite SIEMPRE al cambiar de track, sea cual sea el motor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackAttribution {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// URL de carátula (i.scdn.co) o "" si no hay.
    pub art_url: String,
    pub duration_ms: u32,
}

/// Descarga la atribución del track vía librespot-metadata.
async fn fetch_attribution(session: &Session, uri: &SpotifyUri) -> Option<TrackAttribution> {
    let track = SpTrack::get(session, uri).await.ok()?;
    let artist = track.artists.first().map(|a| a.name.clone()).unwrap_or_default();
    let cover = track
        .album
        .covers
        .first()
        .map(|img| format!("https://i.scdn.co/image/{}", img.id));
    Some(TrackAttribution {
        uri: uri.to_string(),
        title: track.name,
        artist,
        album: track.album.name,
        art_url: cover.unwrap_or_default(),
        duration_ms: track.duration.max(0) as u32,
    })
}

/// Handle vivo del motor Spotify compartido con el router y el loop de eventos.
pub struct SpotifyHandle {
    pub spirc: Spirc,
    pub session: Session,
    pub player: Arc<Player>,
    pub config: SpotifyConfig,
    /// Última atribución conocida (None = aún sin track).
    pub current: Option<TrackAttribution>,
    pub username: Option<String>,
    /// Nivel verificado (gate o tier persistido) para la política de saltos.
    pub tier: AccountTier,
    /// Proxy de restricciones: filtra comandos antes de llegar a Spirc.
    pub policy: SharedPolicy,
}

impl SpotifyHandle {
    pub fn load_uri(&self, uri: &str, start_playing: bool, seek_to_ms: u32) -> Result<(), String> {
        // Valida el formato antes de mandarlo al servidor de Spotify.
        SpotifyUri::from_uri(uri).map_err(|e| format!("URI de Spotify inválida: {e}"))?;
        self.spirc
            .load(LoadRequest::from_context_uri(
                uri.to_string(),
                LoadRequestOptions {
                    start_playing,
                    seek_to: seek_to_ms,
                    playing_track: Some(PlayingTrack::Uri(uri.to_string())),
                    ..Default::default()
                },
            ))
            .map_err(|e| format!("spirc.load: {e}"))
    }

    pub fn play(&self) {
        self.spirc.play().ok();
    }
    pub fn pause(&self) {
        self.spirc.pause().ok();
    }
    pub fn next(&self) {
        self.spirc.next().ok();
    }
    pub fn prev(&self) {
        self.spirc.prev().ok();
    }
    pub fn set_volume_pct(&self, pct: u8) {
        self.spirc.set_volume(ui_to_u16(pct)).ok();
    }
    pub fn seek_ms(&self, position_ms: u32) {
        self.spirc.set_position_ms(position_ms).ok();
    }
    pub fn shuffle(&self, on: bool) {
        self.spirc.shuffle(on).ok();
    }
    pub fn repeat_ctx(&self, on: bool) {
        self.spirc.repeat(on).ok();
    }
    pub fn repeat_track(&self, on: bool) {
        self.spirc.repeat_track(on).ok();
    }

    /// ¿Puede saltar al siguiente? Consume presupuesto en restringidas.
    pub fn check_skip(&self) -> Result<(), PolicyDenial> {
        self.policy
            .lock()
            .map_err(|_| PolicyDenial::internal())?
            .check_skip()
    }

    /// ¿Puede reproducir un URI concreto bajo demanda?
    pub fn check_play_ondemand(&self) -> Result<(), PolicyDenial> {
        self.policy
            .lock()
            .map_err(|_| PolicyDenial::internal())?
            .check_play_ondemand()
    }

    /// (usados, máximo, espera en segundos) para la UI.
    pub fn budget_snapshot(&self) -> (u32, u32, u64) {
        match self.policy.lock() {
            Ok(mut p) => {
                let s = p.snapshot();
                (s.skips_used, s.skips_max, s.retry_after_secs)
            }
            Err(_) => (0, crate::spotify_policy::FREE_SKIPS_PER_HOUR, 0),
        }
    }

    /// Desconecta el dispositivo Connect (pausando) y termina el task de Spirc.
    pub fn shutdown(&self) {
        self.spirc.disconnect(true).ok();
        self.spirc.shutdown().ok();
    }
}

/// Tipo compartido que guarda el estado de Tauri.
pub type SharedSpotify = Arc<Mutex<Option<SpotifyHandle>>>;

pub fn shared() -> SharedSpotify {
    Arc::new(Mutex::new(None))
}

/// Comprueba el tipo de cuenta con la Web API antes de tocar librespot.
/// Devuelve el `product` leído (para trazabilidad).
async fn web_account_product(pkce_token: &str) -> Result<Option<String>, String> {
    let resp = reqwest::Client::new()
        .get("https://api.spotify.com/v1/me")
        .bearer_auth(pkce_token)
        .send()
        .await
        .map_err(|e| format!("consulta de cuenta: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "token PKCE rechazado ({}) — vuelve a iniciar sesión",
            resp.status().as_u16()
        ));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("respuesta de cuenta ilegible: {e}"))?;
    Ok(body.get("product").and_then(|p| p.as_str()).map(str::to_string))
}

/// Obtiene (o crea y persiste) el device_id estable del dispositivo Connect.
fn device_id() -> String {
    if let Some(id) = keyring::get(SERVICE_DEVICE, "device_id") {
        return id;
    }
    // Sin dependencia extra: librespot solo pide un string único por instalación.
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let id = format!("{pid:016x}-{nanos:032x}");
    keyring::set(SERVICE_DEVICE, "device_id", &id).ok();
    id
}

/// Ruta del cache temporal de credenciales de librespot (temp dir del sistema).
/// Decisión: temp + remove tras migrar al keyring; el secreto nunca queda en
/// texto plano de forma permanente.
fn credentials_cache_file() -> PathBuf {
    std::env::temp_dir().join("musica-librespot-creds.json")
}

/// Decide si hay que negociar credenciales nuevas (primer Connect con token PKCE)
/// o si ya tenemos credenciales reusable en el keyring.
pub enum CredentialsPlan {
    /// Reusar credenciales persistidas (arranque normal).
    Cached(Credentials),
    /// Negociar con token PKCE y persistir el resultado.
    FreshToken(String),
    /// Sin nada almacenado: hace falta pasar token.
    Missing,
}

pub fn credentials_plan(pkce_token: Option<&str>) -> CredentialsPlan {
    if let Some(token) = pkce_token.filter(|t| !t.trim().is_empty()) {
        return CredentialsPlan::FreshToken(token.trim().to_string());
    }
    if let Some(json) = keyring::get(SERVICE_SPOTIFY, "credentials") {
        if let Ok(creds) = serde_json::from_str::<Credentials>(&json) {
            return CredentialsPlan::Cached(creds);
        }
    }
    CredentialsPlan::Missing
}

/// Persiste las credenciales reusable en el keyring (JSON de `Credentials`).
/// Formato idéntico al cache de librespot → compatible con su esquema.
pub fn persist_credentials(creds: &Credentials) {
    if let Ok(json) = serde_json::to_string(creds) {
        keyring::set(SERVICE_SPOTIFY, "credentials", &json).ok();
    }
}

/// Borra credenciales y token (cierre de sesión de Spotify).
pub fn clear_stored() {
    keyring::remove(SERVICE_SPOTIFY, "credentials");
    keyring::remove(SERVICE_PKCE, "access_token");
}

/// Construye el sink de audio para `Player::new` según backend/dispositivo.
fn make_sink_builder(cfg: &SpotifyConfig) -> impl FnOnce() -> Box<dyn Sink> + Send + 'static {
    let backend = cfg.backend.resolved();
    let device = cfg.device_out.clone();
    let builder = backend.sink_builder().unwrap_or_else(|| {
        // Sin backend nativo: sink mudo; el motor sigue respondiendo a comandos.
        eprintln!("[musica] sin backend de audio nativo: sink mudo");
        |_: Option<String>, _: AudioFormat| -> Box<dyn Sink> {
            struct Mute;
            impl Sink for Mute {
                fn write(
                    &mut self,
                    _: librespot::playback::decoder::AudioPacket,
                    _: &mut librespot::playback::convert::Converter,
                ) -> audio_backend::SinkResult<()> {
                    Ok(())
                }
            }
            Box::new(Mute)
        }
    });
    move || (builder)(device, AudioFormat::F64)
}

/// Punto de entrada principal: crea sesión, player y Spirc; arranca los loops.
/// Debe invocarse dentro de un runtime tokio (comando Tauri `async`).
pub async fn connect(
    app: &AppHandle,
    cfg: SpotifyConfig,
    pkce_token: Option<&str>,
    shared: SharedSpotify,
) -> Result<SpotifyHandle, String> {
    // 1) Gate Premium ANTES de crear nada de librespot.
    // El tier verificado se persiste junto a las credenciales para la política.
    // Decisión de seguridad: solo Premium confirmado llega a Session::connect.
    // Con Free/Unknown, librespot haría exit(1) en check_catalogue y mataría
    // el proceso entero sin mensaje: por eso se deniega aquí, con texto claro.
    let (credentials, credential_cache, tier) = match credentials_plan(pkce_token) {
        CredentialsPlan::Cached(c) => {
            // Estas credenciales ya sobrevivieron a un Connect Premium (con Free,
            // librespot aborta antes de persistir nada). Migración: sin tier
            // guardado se asume premium heredado.
            let tier = match keyring::get(SERVICE_SPOTIFY, "tier").as_deref() {
                Some("premium") => AccountTier::Premium,
                Some(_) => AccountTier::Restricted,
                None => AccountTier::Premium,
            };
            (c, None, tier)
        }
        CredentialsPlan::FreshToken(token) => {
            let product = web_account_product(&token).await?;
            let gate = account_gate(product.as_deref());
            if !gate_allows(&gate) {
                return Err(match gate {
                    AccountGate::NotPremium(_) => AuthFailure::FreeAccount.user_message(),
                    // Unknown: si fuera Free, librespot haría exit(1) y tumbar la app.
                    _ => "no se pudo verificar el tipo de cuenta: Spotify Connect (librespot) \
                        requiere Premium. Alternativa: importa tus playlists a YouTube Music y \
                        reprodúcelas con el motor local."
                        .into(),
                });
            }
            keyring::set(SERVICE_SPOTIFY, "tier", AccountTier::Premium.as_str()).ok();
            (Credentials::with_access_token(token), Some(credentials_cache_file()), AccountTier::Premium)
        }
        CredentialsPlan::Missing => {
            return Err("falta token PKCE: inicia sesión con Spotify".into());
        }
    };
    if let Some(token) = pkce_token.filter(|t| !t.trim().is_empty()) {
        // Guardamos el token PKCE aparte (caduca en 1h; las reusable no caducan).
        keyring::set(SERVICE_PKCE, "access_token", token.trim()).ok();
    }

    // 2) Sesión + cache solo-credenciales si venimos de token.
    let cache_path = credential_cache.clone();
    let mut session_cfg = SessionConfig::default();
    session_cfg.device_id = device_id();
    session_cfg.autoplay = Some(false);
    let cache = credential_cache.and_then(|path| {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        match Cache::new(Some(path), None, None, None) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("[musica] cache de credenciales no disponible: {e}");
                None
            }
        }
    });
    let session = Session::new(session_cfg, cache);

    // 3) Player (mixer software → el volumen lo manda Spirc).
    let mut pcfg = PlayerConfig::default();
    pcfg.bitrate = match cfg.bitrate_kbps {
        96 => Bitrate::Bitrate96,
        320 => Bitrate::Bitrate320,
        _ => Bitrate::Bitrate160,
    };
    pcfg.normalisation = cfg.normalisation;
    if cfg.normalisation {
        pcfg.normalisation_type = NormalisationType::Track;
    }
    // Eventos de posición cada 500ms (barra de progreso unificada).
    pcfg.position_update_interval = Some(std::time::Duration::from_millis(500));
    let mixer: Arc<dyn Mixer> = Arc::new(
        SoftMixer::open(MixerConfig::default()).expect("SoftMixer: mixer por software"),
    );
    let soft = mixer.get_soft_volume();
    let sink = make_sink_builder(&cfg);
    let player = Player::new(pcfg, session.clone(), soft, sink);

    // 4) ConnectConfig: nombre personalizable, icono por plataforma.
    let connect_cfg = ConnectConfig {
        name: cfg.device_name.clone(),
        device_type: DeviceType::Computer,
        initial_volume: ui_to_u16(cfg.initial_volume),
        ..Default::default()
    };

    // 5) Spirc: arranca el dispositivo Connect y autentica.
    // NOTA login5: en escritorio NO existe bootstrap por login5
    // (`login(id, password)` devuelve `OnlyForMobile` fuera de android/ios y
    // `auth_token()` exige sesión previa). El camino soportado es
    // `with_access_token` + credenciales reusable del APWelcome (paso 6); tras
    // conectar se acuña el token de API vía login5 con la credencial reusable.
    let (spirc, spirc_task) = Spirc::new(
        connect_cfg,
        session.clone(),
        credentials.clone(),
        player.clone(),
        mixer,
    )
    .await
    .map_err(|e| {
        let friendly = classify_auth_error(&e).user_message();
        format!("{friendly}\n(detalle técnico: {e})")
    })?;

    // 5b) Token de API vía login5 con la credencial reusable: demuestra que lo
    // persistido sirve también para la Web API (búsquedas) sin otro OAuth y lo
    // refresca en el keyring (el PKCE caduca en ~1h).
    match session.login5().auth_token().await {
        Ok(token) => {
            keyring::set(SERVICE_PKCE, "access_token", &token.access_token).ok();
        }
        Err(e) => {
            eprintln!("[musica] login5 auth_token falló: {e}");
            return Err(classify_auth_error(&e).user_message());
        }
    }

    // 6) Migrar credenciales reusable del cache al keyring y borrar el archivo.
    let mut username = credentials.username.clone();
    let mut cache_path_to_clean = None;
    if let Some(cache) = session.cache() {
        if let Some(fresh) = cache.credentials() {
            persist_credentials(&fresh);
            username = fresh.username.clone();
            if let Some(path) = cache_path.as_ref() {
                cache_path_to_clean = Some(path.clone());
            }
        }
    }
    if let Some(path) = cache_path_to_clean {
        let _ = std::fs::remove_file(path);
    }

    // 7) Loop de eventos: PlayerEvent → RouteEvent → IPC + MPRIS unificado.
    let ev_player = player.clone();
    let ev_session = session.clone();
    let ev_app = app.clone();
    let ev_shared = shared.clone();
    tauri::async_runtime::spawn(async move {
        let mut channel = ev_player.get_player_event_channel();
        while let Some(ev) = channel.recv().await {
            if let Some(route) = map_player_event(&ev) {
                crate::playback_router::forward_spotify_event(
                    &ev_app,
                    &ev_session,
                    ev_shared.clone(),
                    route,
                )
                .await;
            }
        }
        // El canal solo se cierra si el player muere: marca desconectado.
        eprintln!("[musica] loop de eventos Spotify terminado");
    });

    // El task de Spirc debe correr dentro del runtime de Tauri.
    tauri::async_runtime::spawn(spirc_task);

    Ok(SpotifyHandle {
        spirc,
        session: session.clone(),
        player: player.clone(),
        config: cfg,
        current: None,
        username,
        tier,
        policy: shared_policy(tier),
    })
}

/// Metadatos de cuenta devueltos al frontend (sin material de auth).
#[derive(Debug, Clone, Serialize)]
pub struct SpotifyAccountInfo {
    pub connected: bool,
    pub username: Option<String>,
    pub device_name: Option<String>,
    pub persistent_keyring: bool,
    pub backend: Option<String>,
    /// "premium" | "restricted" | None (desconectado).
    pub tier: Option<String>,
    /// librespot no inserta anuncios: siempre false (ver limitaciones).
    pub ads_supported: bool,
}

pub fn account_info(handle: Option<&SpotifyHandle>) -> SpotifyAccountInfo {
    SpotifyAccountInfo {
        connected: handle.is_some(),
        username: handle.and_then(|h| h.username.clone()),
        device_name: handle.map(|h| h.config.device_name.clone()),
        persistent_keyring: keyring::persistent(),
        backend: handle.map(|h| format!("{:?}", h.config.backend.resolved())),
        tier: handle.map(|h| h.tier.as_str().to_string()),
        ads_supported: false,
    }
}

/// Atribución del track actual del motor Spotify (si hay).
/// Usada por el router cuando el motor activo es Spotify.
pub async fn current_attribution(session: &Session, uri: &str) -> Option<TrackAttribution> {
    let parsed = SpotifyUri::from_uri(uri).ok()?;
    fetch_attribution(session, &parsed).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use librespot::metadata::audio::{AudioItem, UniqueFields};

    fn track_uri(id: &str) -> SpotifyUri {
        SpotifyUri::from_uri(&format!("spotify:track:{id}")).unwrap()
    }

    // ---- gate de cuenta Premium ----

    #[test]
    fn gate_premium_pasa_free_no() {
        assert!(gate_allows(&account_gate(Some("premium"))));
        assert!(matches!(
            account_gate(Some("free")),
            AccountGate::NotPremium(_)
        ));
        assert!(matches!(
            account_gate(Some("open")),
            AccountGate::NotPremium(_)
        ));
        assert!(matches!(account_gate(None), AccountGate::Unknown));
        // La Web API no garantiza minúsculas.
        assert!(gate_allows(&account_gate(Some("Premium"))));
        // Unknown NO arranca: permitirlo expondría a la app al exit(1) de
        // librespot si la cuenta resultara ser Free (denegar por defecto).
        assert!(!gate_allows(&account_gate(Some("gold"))));
        assert!(!gate_allows(&account_gate(Some(""))));
    }

    // ---- mapeo volumen ----

    #[test]
    fn volumen_ui_u16_roundtrip() {
        assert_eq!(ui_to_u16(0), 0);
        assert_eq!(ui_to_u16(100), 65500);
        assert_eq!(ui_to_u16(200), 65500); // satura, no desborda
        for pct in [0u8, 1, 25, 50, 80, 100] {
            let back = u16_to_ui(ui_to_u16(pct));
            assert!((back as i16 - pct as i16).abs() <= 1, "pct={pct} back={back}");
        }
        assert_eq!(u16_to_ui(u16::MAX), 100);
    }

    // ---- mapeo de PlayerEvent (mock del canal) ----

    #[test]
    fn eventos_basicos_se_mapean() {
        let uri = track_uri("4cOdK2wGLETKBW3PvgPWqT");
        assert_eq!(
            map_player_event(&PlayerEvent::Playing {
                play_request_id: 1,
                track_id: uri.clone(),
                position_ms: 4200,
            }),
            Some(RouteEvent::Playing {
                uri: "spotify:track:4cOdK2wGLETKBW3PvgPWqT".into(),
                position_ms: 4200
            })
        );
        assert_eq!(
            map_player_event(&PlayerEvent::Paused {
                play_request_id: 1,
                track_id: uri.clone(),
                position_ms: 4200,
            }),
            Some(RouteEvent::Paused {
                uri: "spotify:track:4cOdK2wGLETKBW3PvgPWqT".into(),
                position_ms: 4200
            })
        );
        assert_eq!(
            map_player_event(&PlayerEvent::VolumeChanged { volume: 32767 }),
            Some(RouteEvent::Volume { percent: 49 })
        );
        assert_eq!(
            map_player_event(&PlayerEvent::VolumeChanged { volume: 65535 }),
            Some(RouteEvent::Volume { percent: 100 })
        );
        let stopped = PlayerEvent::Stopped { play_request_id: 1, track_id: uri };
        assert_eq!(map_player_event(&stopped), Some(RouteEvent::Stopped));
    }

    #[test]
    fn eventos_de_posicion_y_track() {
        let uri = track_uri("1301WleyT98MSxVHPZCA6M");
        assert_eq!(
            map_player_event(&PlayerEvent::PositionChanged {
                play_request_id: 7,
                track_id: uri.clone(),
                position_ms: 12345,
            }),
            Some(RouteEvent::Position {
                uri: "spotify:track:1301WleyT98MSxVHPZCA6M".into(),
                position_ms: 12345
            })
        );
        let changed = PlayerEvent::TrackChanged {
            audio_item: Box::new(AudioItem {
                track_id: uri,
                uri: "spotify:track:1301WleyT98MSxVHPZCA6M".into(),
                files: Default::default(),
                name: "Prueba".into(),
                covers: vec![],
                language: vec![],
                duration_ms: 200_000,
                is_explicit: false,
                availability: Ok(()),
                alternatives: None,
                unique_fields: UniqueFields::Track {
                    artists: Default::default(),
                    album: "Álbum".into(),
                    album_artists: vec![],
                    popularity: 0,
                    number: 0,
                    disc_number: 0,
                },
            }),
        };
        assert_eq!(
            map_player_event(&changed).unwrap().uri(),
            Some("spotify:track:1301WleyT98MSxVHPZCA6M")
        );
        assert_eq!(map_player_event(&changed).unwrap().ipc_kind(), "track-changed");
        let disc = PlayerEvent::SessionDisconnected {
            connection_id: "c".into(),
            user_name: "u".into(),
        };
        assert_eq!(map_player_event(&disc), Some(RouteEvent::SessionDisconnected));
    }

    #[test]
    fn eventos_internos_no_generan_ruido_ipc() {
        // TimeToPreloadNextTrack/Preloading son para Spirc: el router los ignora.
        let uri = track_uri("1301WleyT98MSxVHPZCA6M");
        assert_eq!(
            map_player_event(&PlayerEvent::TimeToPreloadNextTrack {
                play_request_id: 7,
                track_id: uri.clone(),
            }),
            None
        );
        assert_eq!(
            map_player_event(&PlayerEvent::Preloading { track_id: uri }),
            None
        );
    }

    #[test]
    fn unavailable_y_end_of_track_llegan_al_router() {
        let uri = track_uri("1301WleyT98MSxVHPZCA6M");
        assert_eq!(
            map_player_event(&PlayerEvent::Unavailable {
                play_request_id: 3,
                track_id: uri.clone(),
            }),
            Some(RouteEvent::Unavailable {
                uri: "spotify:track:1301WleyT98MSxVHPZCA6M".into()
            })
        );
        assert_eq!(
            map_player_event(&PlayerEvent::EndOfTrack {
                play_request_id: 3,
                track_id: uri,
            })
            .unwrap()
            .ipc_kind(),
            "ended"
        );
    }

    // ---- plan de credenciales ----

    #[test]
    fn plan_prefiere_token_fresco() {
        let p = credentials_plan(Some("nuevo-token"));
        assert!(matches!(&p, CredentialsPlan::FreshToken(t) if t == "nuevo-token"));
        // Token en blanco: se ignora y se busca en keyring.
        let p2 = credentials_plan(Some("   "));
        assert!(!matches!(p2, CredentialsPlan::FreshToken(_)));
    }

    #[test]
    fn credenciales_serializan_json_compatible() {
        // Con token se pueden guardar/restaurar por keyring sin perder campos.
        let creds = Credentials::with_access_token("token-abc");
        let json = serde_json::to_string(&creds).unwrap();
        assert!(json.contains("auth_data"));
        let back: Credentials = serde_json::from_str(&json).unwrap();
        assert_eq!(back.auth_data, creds.auth_data);
    }

    #[test]
    fn backend_resuelto_por_plataforma() {
        // `Auto` debe resolver a un sink real en cada plataforma.
        #[cfg(target_os = "linux")]
        {
            assert!(AudioBackend::Auto.sink_builder().is_some());
            assert!(matches!(
                AudioBackend::Auto.resolved(),
                AudioBackend::Auto | AudioBackend::Alsa | AudioBackend::PulseAudio
            ));
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            assert_eq!(AudioBackend::Auto.resolved(), AudioBackend::Rodio);
        }
    }

    // ---- clasificación de errores de autenticación (mensajes de la UI) ----

    #[test]
    fn servidor_que_exige_premium_se_reporta_como_free() {
        let f = classify_login_message("Login failed with reason: Premium account required");
        assert_eq!(f, AuthFailure::FreeAccount);
        assert!(f.user_message().contains("Premium"));
        assert!(f.user_message().contains("anuncios"));
    }

    #[test]
    fn credenciales_malas_piden_nuevo_token() {
        for msg in [
            "Login failed with reason: Bad credentials",
            "Login failed with reason: Could not validate credentials",
        ] {
            let f = classify_login_message(msg);
            assert_eq!(f, AuthFailure::BadToken, "{msg}");
            assert!(f.user_message().contains("caducó"));
        }
    }

    #[test]
    fn transporte_roto_es_problema_de_red() {
        for msg in ["transport returned no data", "connection closed", "timed out"] {
            assert_eq!(classify_login_message(msg), AuthFailure::Network, "{msg}");
        }
    }

    #[test]
    fn error_desconocido_conserva_el_detalle() {
        let f = classify_login_message("Login failed with reason: Travel restriction");
        assert!(matches!(f, AuthFailure::Server(_)));
        assert!(f.user_message().contains("Travel restriction"));
    }

    /// Error real de librespot (no solo texto): el wrapper también clasifica.
    #[test]
    fn error_real_de_librespot_se_clasifica() {
        #[derive(Debug)]
        struct Msg(String);
        impl std::fmt::Display for Msg {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl std::error::Error for Msg {}
        let err = librespot::core::Error::permission_denied(Msg(
            "Login failed with reason: Premium account required".into(),
        ));
        assert_eq!(classify_auth_error(&err), AuthFailure::FreeAccount);
        let err2 = librespot::core::Error::permission_denied(Msg(
            "Login failed with reason: Bad credentials".into(),
        ));
        assert_eq!(classify_auth_error(&err2), AuthFailure::BadToken);
    }

    // ---- Smoke tests de runtime (requieren --ignored) ----
    // No son unitarios puros: construyen el stack nativo REAL (Session + Player
    // + sink) que usa `connect()`, para demostrar que libmpv y librespot
    // coexisten dentro del binario de la app.

    /// Construye exactamente lo que `connect()` construye antes de Spirc.
    #[tokio::test]
    #[ignore = "runtime: abre el stack nativo; ejecutar con --ignored"]
    async fn smoke_stack_nativo_librespot() {
        let mut cfg = SessionConfig::default();
        cfg.device_id = format!("smoke-{}", std::process::id());
        let session = Session::new(cfg, None);

        let mut pcfg = PlayerConfig::default();
        pcfg.bitrate = Bitrate::Bitrate160;
        pcfg.position_update_interval = Some(std::time::Duration::from_millis(500));
        let mixer: std::sync::Arc<dyn Mixer> =
            std::sync::Arc::new(SoftMixer::open(MixerConfig::default()).unwrap());
        let soft = mixer.get_soft_volume();
        let spotify_cfg = SpotifyConfig::default();
        let sink = make_sink_builder(&spotify_cfg);
        let player = Player::new(pcfg, session.clone(), soft, sink);

        // El canal de eventos debe existir y no colapsar el player.
        let mut ch = player.get_player_event_channel();
        assert!(ch.try_recv().is_err(), " canal recién creado debe estar vacío");
        assert!(!session.device_id().is_empty());
        eprintln!("[smoke] Session+Player+sink nativos OK (device={})", session.device_id());
    }

    /// El gate debe rechazar un token inválido contra la Web API real (401).
    /// Verifica el camino de error que ve el usuario con cuenta Free/expirada.
    #[tokio::test]
    #[ignore = "red: golpea api.spotify.com; ejecutar con --ignored"]
    async fn gate_rechaza_token_invalido_real() {
        let res = web_account_product("token-invalido-de-prueba").await;
        assert!(res.is_err(), "un token basura debe ser rechazado: {res:?}");
        let err = res.unwrap_err();
        assert!(err.contains("rechazado"), "mensaje claro al usuario: {err}");
        // El web API ya rechaza tokens inválidos (401). El gate de account_gate
        // ya no bloquea la inicialización; el tier se establece y la política
        // aplica las restricciones correspondientes.
    }
}
