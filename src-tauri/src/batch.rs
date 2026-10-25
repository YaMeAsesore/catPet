//! Carga de playlists de YouTube (W0, revisado en P10).
//!
//! Regla central NUEVA: la lista plana (barata, una sola petición) se muestra
//! al instante. NO se resuelve la URL de audio de cada pista por adelantado:
//! resolver 50 pistas con yt-dlp (Python+node, 1-7 s cada una) era la causa de
//! que abrir una playlist tardara hasta 2 minutos y de que YouTube limitara
//! las peticiones.
//!
//! Ahora:
//! - `open` trae la lista plana y revela el primer lote de inmediato.
//! - Solo se precalientan las 3 primeras pistas (cupo `Warm`, 2 en paralelo)
//!   para que el primer clic suene rápido. El resto se resuelve al sonar
//!   (caché en memoria de ytmusic + precarga de la ventana de reproducción).
//! - `load_more` revela el siguiente lote también al instante y precalienta
//!   sus 2 primeras pistas en segundo plano, sin bloquear la UI.
//!
//! Esto conserva todas las funciones (indicador Lista/Muerta/Sin preparar,
//! paginación, caché negativa, precarga, descarga completa) pero elimina la
//! espera artificial y el gasto masivo de procesos.

use crate::db::Track;
use crate::ytmusic::{self, SlotKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Tamaño de lote para playlists grandes.
pub const BATCH_SIZE: usize = 50;
/// Por debajo de este total se trae TODO en un solo lote.
pub const PAGING_THRESHOLD: usize = 100;
/// Pendiente máximo de la lista plana (una sola petición barata). También lo
/// usan las rutas que NO son por lotes: es el único tope de lista plana.
/// Verificado con yt-dlp: devuelve listas de 500+ pistas completas.
pub const FLAT_CAP: u32 = 10_000;
/// Pistas que se precalientan al abrir/revelar un lote. Suficiente para que
/// el primer clic (y el siguiente) sea casi instantáneo sin inundar YouTube.
const WARM_ROWS: usize = 3;
/// Precalentamientos simultáneos (el cupo Warm global permite 2).
const WARM_WORKERS: usize = 2;
/// Primera página que se pide al abrir: se pinta de inmediato. Si la lista es
/// más larga, el resto se pide en una sola operación de fondo (igual que el
/// tope histórico, pero sin bloquear la primera página).
const FIRST_PAGE: u32 = PAGING_THRESHOLD as u32;
/// TTL de la caché persistente de metadatos. Dentro de esta ventana reabrir
/// una playlist no toca la red; pasado el TTL se vuelve a listar.
const CACHE_TTL_SECS: i64 = 20 * 60;
/// Tope de páginas InnerTube en segundo plano (100 × 100 ≈ 10 000 pistas).
const MAX_INNERTUBE_PAGES: usize = 110;

/// Estado de fila para la UI: lista, muerta (gris + motivo) o sin preparar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RowState {
    Ready,
    Dead,
    Unprepared,
}

/// Estado interno de una pista del lote (incluye el "aún no tocado").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Ready,
    Dead,
    Unprepared,
}

impl Slot {
    fn row(&self) -> RowState {
        match self {
            Slot::Ready => RowState::Ready,
            Slot::Dead => RowState::Dead,
            Slot::Unprepared => RowState::Unprepared,
        }
    }
}

/// Fila para la UI: los campos de `db::Track` (snake_case, tal cual los
/// consume la cola) + estado de preparación y motivo.
#[derive(Debug, Clone, Serialize)]
pub struct BatchRow {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub year: i32,
    pub track_no: i32,
    pub duration_secs: i64,
    pub mtime: i64,
    pub art_url: String,
    pub state: RowState,
    pub motivo: Option<String>,
    /// Estado del corazón de YouTube Music; None = desconocido.
    pub liked: Option<bool>,
}

/// Instantánea para la UI. Mientras `warming` es true la UI polla a ~2/s para
/// pintar los puntos «Lista» de las 3 primeras pistas (payload pequeño: solo
/// el primer lote). Tras `load_more` no se polla: las filas ya están visibles.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// ¿Hay una playlist abierta en modo lotes?
    pub open: bool,
    pub title: String,
    pub total: usize,
    /// Filas ya visibles (ahora se revelan al instante, sin esperar red).
    pub shown: usize,
    pub batch_size: usize,
    /// Compatibilidad: la carga por lotes ya no bloquea. Se mantiene false.
    pub loading: bool,
    /// Procesadas del lote actual (las visibles del lote).
    pub processed: usize,
    /// Tamaño del lote actual.
    pub batch_total: usize,
    /// ¿Quedan pistas sin mostrar? (botón "Cargar más").
    pub more: bool,
    /// ¿Se están precalentando las primeras pistas del lote?
    pub warming: bool,
    /// Id de lista para guardar/quitar la playlist en YouTube Music.
    pub playlist_id: Option<String>,
    /// ¿La playlist ya está en la biblioteca? None = desconocido.
    pub saved: Option<bool>,
    pub rows: Vec<BatchRow>,
    /// Aviso discreto (límite de YouTube, etc.).
    pub warning: Option<String>,
}

impl Snapshot {
    fn idle() -> Self {
        Snapshot {
            open: false,
            title: String::new(),
            total: 0,
            shown: 0,
            batch_size: BATCH_SIZE,
            loading: false,
            processed: 0,
            batch_total: 0,
            more: false,
            warming: false,
            playlist_id: None,
            saved: None,
            rows: vec![],
            warning: None,
        }
    }
}

/// Payload persistido en SQLite. Solo metadatos públicos de la playlist:
/// título, ids de vídeo, artista y duración. Nunca cookies ni URLs de audio.
#[derive(Serialize, Deserialize)]
struct CachedPlaylist {
    title: String,
    #[serde(default)]
    saved: Option<bool>,
    tracks: Vec<ytmusic::YtTrack>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

struct Session {
    /// Token de cancelación: los trabajos de una sesión vieja la ignoran.
    id: u64,
    title: String,
    all: Vec<Track>,
    /// Rutas ya presentes (la paginación en segundo plano puede solaparse).
    seen: std::collections::HashSet<String>,
    batch_size: usize,
    shown: usize,
    /// Tamaño del lote revelado por última vez (para el contador de la UI).
    batch_len: usize,
    processed: usize,
    states: Vec<Slot>,
    motivos: Vec<Option<String>>,
    /// Paralelo a `all`: like de YouTube Music en el momento del listado.
    likes: Vec<Option<bool>>,
    more: bool,
    warning: Option<String>,
    /// Precalentamientos pendientes (para saber cuándo parar el polling).
    warm_left: usize,
    /// Quedan páginas de la lista plana por traer en segundo plano.
    flat_pending: bool,
    /// La lista plana se trajo completa (necesario para descargar la playlist).
    complete: bool,
    /// Caché persistente (SQLite): ruta de la BD y clave de la playlist.
    cache_db: Option<PathBuf>,
    cache_key: Option<String>,
    /// Id de lista (PL/RD/…) para el botón «Guardar playlist».
    playlist_id: Option<String>,
    /// ¿Guardada en la biblioteca de YouTube Music?
    playlist_saved: Option<bool>,
}

static SESSION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn session() -> &'static Mutex<Option<Session>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

/// Tamaño de lote para un total dado (puro, testeable).
fn batch_size_for(total: usize) -> usize {
    if total <= PAGING_THRESHOLD {
        total.max(1)
    } else {
        BATCH_SIZE
    }
}

/// Rango [inicio, fin) del siguiente lote a partir de `shown`; None si no
/// quedan pistas (puro, testeable).
fn next_batch(total: usize, batch_size: usize, shown: usize) -> Option<(usize, usize)> {
    if shown >= total || batch_size == 0 {
        return None;
    }
    Some((shown, (shown + batch_size).min(total)))
}

/// Nº de lotes previstos para un total (redondeo hacia arriba; puro).
#[cfg(test)]
fn batch_count(total: usize, batch_size: usize) -> usize {
    if total == 0 || batch_size == 0 {
        0
    } else {
        total.div_ceil(batch_size)
    }
}

/// Cierra la sesión anterior (solo corta lo que NO ha empezado; lo que está
/// resolviendo termina y se guarda en caché).
pub fn cancel() {
    if let Ok(mut s) = session().lock() {
        *s = None;
    }
}

/// Título y lista plana COMPLETA de la playlist abierta (todas las pistas, no
/// solo el lote visible). La descarga de playlist la usa para bajarlas todas
/// sin resolver una sola URL de audio. None si no hay sesión abierta.
pub fn session_full() -> Option<(String, Vec<Track>)> {
    let g = session().lock().ok()?;
    let s = g.as_ref()?;
    // Si el listado aún se está paginando (o falló), no se entrega una lista
    // parcial: el descargador prefiere pedir la lista completa por su cuenta.
    if !s.complete {
        return None;
    }
    Some((s.title.clone(), s.all.clone()))
}

/// Abre una playlist sin caché (tests y llamadas internas).
#[cfg(test)]
pub async fn open(url: String) -> Result<Snapshot, String> {
    open_with_cache(url, None).await
}

/// Abre una playlist con caché persistente opcional:
/// 1. Si hay una entrada reciente en SQLite, las filas salen al instante.
/// 2. Si no, la primera página se pide por InnerTube (sin proceso Python) y,
///    si la sesión/formato no lo permiten, cae a `yt-dlp` como siempre.
/// 3. El resto se pagina en segundo plano (InnerTube o yt-dlp).
pub async fn open_with_cache(
    url: String,
    db_path: Option<PathBuf>,
) -> Result<Snapshot, String> {
    let (_, canonical) = ytmusic::classify_url(&url)?;
    let list_id = ytmusic::url_list_id(&canonical);
    let cache_key = list_id.as_deref().map(|id| format!("yt:{id}"));

    // 1) Caché persistente: reabrir una playlist reciente no toca la red.
    if let (Some(db), Some(key)) = (db_path.as_deref(), cache_key.as_deref()) {
        if let Some((title, saved, cached)) = read_cached_playlist(db, key) {
            eprintln!(
                "[lote] playlist={title} cache=hit pistas={} (reapertura instantánea)",
                cached.len()
            );
            return open_from_cache(db_path, key.to_string(), title, saved, cached);
        }
    }

    // 2) Primera página por InnerTube (si hay sesión y la lista es estable).
    let mut inner = None;
    if let Some(id) = list_id.as_deref() {
        match crate::yt_reco::playlist_page(id, None).await {
            Ok(page) if !page.tracks.is_empty() => inner = Some(page),
            _ => { /* sin sesión, mix/radio o formato nuevo: respaldo yt-dlp */ }
        }
    }

    let (title, yt_tracks, complete, total_hint, continuation, via_inner, page_saved) = match inner {
        Some(page) => {
            let n = page.tracks.len();
            let complete = match page.total {
                Some(total) => n >= total && page.continuation.is_none(),
                None => page.continuation.is_none(),
            };
            let title = page.title.unwrap_or_else(|| "Playlist de YouTube".into());
            (
                title,
                page.tracks,
                complete,
                page.total,
                page.continuation,
                true,
                page.saved,
            )
        }
        None => {
            let pl = ytmusic::import_page(&canonical, 1, FIRST_PAGE).await?;
            let complete = pl.tracks.len() < FIRST_PAGE as usize;
            (pl.title, pl.tracks, complete, None, None, false, None)
        }
    };

    let likes: Vec<Option<bool>> = yt_tracks.iter().map(|t| t.liked).collect();
    let tracks: Vec<Track> = yt_tracks.iter().map(|t| t.to_track(&title)).collect();
    let total = tracks.len();
    cancel();
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst) + 1;
    let batch_size = if complete && total <= PAGING_THRESHOLD {
        batch_size_for(total)
    } else {
        BATCH_SIZE
    };
    let shown = total;
    let seen: std::collections::HashSet<String> =
        tracks.iter().map(|t| t.path.clone()).collect();
    eprintln!(
        "[lote] playlist={title} total={total}{} via={} tam={batch_size} (filas inmediatas, sin resolver audio)",
        if complete { String::new() } else { " (+resto en segundo plano)".to_string() },
        if via_inner { "innertube" } else { "yt-dlp" }
    );
    if let Ok(mut s) = session().lock() {
        *s = Some(Session {
            id,
            title: title.clone(),
            all: tracks,
            seen,
            batch_size,
            shown,
            batch_len: shown,
            processed: shown,
            states: vec![Slot::Unprepared; total],
            motivos: vec![None; total],
            likes,
            more: false,
            warning: if ytmusic::throttle_active() {
                Some("YouTube limitó las peticiones: las pistas sonarán al hacer clic".into())
            } else {
                None
            },
            warm_left: 0,
            flat_pending: !complete,
            complete,
            cache_db: db_path,
            cache_key,
            playlist_id: list_id.clone(),
            playlist_saved: page_saved,
        });
    }
    if shown > 0 {
        apply_cached_and_dead(id, 0, shown);
        start_warm(id, 0, shown);
    }
    if complete {
        save_cache(id);
    } else if via_inner {
        match (list_id, continuation) {
            (Some(idlist), Some(token)) => {
                spawn_innertube_rest(
                    id,
                    idlist,
                    token,
                    canonical,
                    title.clone(),
                    total_hint,
                    shown + 1,
                );
            }
            // InnerTube se quedó corto sin token: completa por yt-dlp.
            _ => spawn_flat_rest(id, canonical, title.clone(), shown + 1),
        }
    } else {
        spawn_flat_rest(id, canonical, title.clone(), shown + 1);
    }
    Ok(progress())
}

/// Reconstruye la sesión desde SQLite. La lista cacheada ya está completa,
/// así que «Cargar más» y la descarga funcionan sin red.
fn open_from_cache(
    db_path: Option<PathBuf>,
    key: String,
    title: String,
    saved: Option<bool>,
    cached: Vec<ytmusic::YtTrack>,
) -> Result<Snapshot, String> {
    let playlist_id = key.strip_prefix("yt:").map(str::to_string);
    let likes: Vec<Option<bool>> = cached.iter().map(|t| t.liked).collect();
    let tracks: Vec<Track> = cached.iter().map(|t| t.to_track(&title)).collect();
    let total = tracks.len();
    cancel();
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst) + 1;
    let batch_size = if total <= PAGING_THRESHOLD {
        batch_size_for(total)
    } else {
        BATCH_SIZE
    };
    let shown = total.min(FIRST_PAGE as usize);
    let seen: std::collections::HashSet<String> =
        tracks.iter().map(|t| t.path.clone()).collect();
    if let Ok(mut s) = session().lock() {
        *s = Some(Session {
            id,
            title,
            all: tracks,
            seen,
            batch_size,
            shown,
            batch_len: shown,
            processed: shown,
            states: vec![Slot::Unprepared; total],
            motivos: vec![None; total],
            likes,
            more: shown < total,
            warning: None,
            warm_left: 0,
            flat_pending: false,
            complete: true,
            cache_db: db_path,
            cache_key: Some(key),
            playlist_id,
            playlist_saved: saved,
        });
    }
    if shown > 0 {
        apply_cached_and_dead(id, 0, shown);
        start_warm(id, 0, shown);
    }
    Ok(progress())
}

/// Refleja en la sesión/caché el «Guardar en la biblioteca» de la playlist.
pub fn mark_playlist_saved(playlist_id: &str, saved: bool) {
    let key = format!("yt:{playlist_id}");
    let id = {
        let Ok(mut guard) = session().lock() else { return };
        let Some(s) = guard.as_mut() else { return };
        if s.cache_key.as_deref() != Some(key.as_str()) {
            return;
        }
        s.playlist_saved = Some(saved);
        s.id
    };
    save_cache(id);
}

fn read_cached_playlist(
    db_path: &std::path::Path,
    key: &str,
) -> Option<(String, Option<bool>, Vec<ytmusic::YtTrack>)> {
    let conn = crate::db::open(db_path).ok()?;
    let (updated_at, payload) = crate::db::yt_cache_get(&conn, key).ok()??;
    if now_secs().saturating_sub(updated_at) > CACHE_TTL_SECS {
        return None;
    }
    let cached: CachedPlaylist = serde_json::from_str(&payload).ok()?;
    if cached.tracks.is_empty() {
        return None;
    }
    Some((cached.title, cached.saved, cached.tracks))
}

/// Guarda la lista COMPLETA en SQLite. La llama el final del listado inicial
/// o el paginador de fondo; nunca se guarda una lista parcial como completa.
fn save_cache(id: u64) {
    let (db, key, title, saved, tracks) = {
        let Ok(guard) = session().lock() else { return };
        let Some(s) = guard.as_ref() else { return };
        if s.id != id || !s.complete {
            return;
        }
        let (Some(db), Some(key)) = (s.cache_db.clone(), s.cache_key.clone()) else {
            return;
        };
        let tracks: Vec<ytmusic::YtTrack> = s
            .all
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                let id = t.path.strip_prefix("ytmusic://")?;
                Some(ytmusic::YtTrack {
                    id: id.to_string(),
                    title: t.title.clone(),
                    artist: t.artist.clone(),
                    duration_secs: t.duration_secs,
                    thumbnail: String::new(),
                    liked: s.likes.get(i).copied().flatten(),
                })
            })
            .collect();
        (db, key, s.title.clone(), s.playlist_saved, tracks)
    };
    let Ok(payload) = serde_json::to_string(&CachedPlaylist { title, saved, tracks }) else {
        return;
    };
    let Ok(conn) = crate::db::open(&db) else { return };
    if let Err(e) = crate::db::yt_cache_put(&conn, &key, now_secs(), &payload) {
        eprintln!("[lote] no se pudo guardar la caché de la playlist: {e}");
    }
}

/// Instantánea actual (la pinta la UI cada ~500 ms mientras precalienta).
pub fn progress() -> Snapshot {
    let guard = match session().lock() {
        Ok(g) => g,
        Err(_) => return Snapshot::idle(),
    };
    let Some(s) = guard.as_ref() else {
        return Snapshot::idle();
    };
    let rows: Vec<BatchRow> = s.all[..s.shown.min(s.all.len())]
        .iter()
        .enumerate()
        .map(|(i, t)| BatchRow {
            id: t.id,
            path: t.path.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            genre: t.genre.clone(),
            year: t.year,
            track_no: t.track_no,
            duration_secs: t.duration_secs,
            mtime: t.mtime,
            art_url: t.art_url.clone(),
            state: s.states[i].row(),
            motivo: s.motivos[i].clone(),
            liked: s.likes.get(i).copied().flatten(),
        })
        .collect();
    Snapshot {
        open: true,
        title: s.title.clone(),
        total: s.all.len(),
        shown: s.shown,
        batch_size: s.batch_size,
        loading: false,
        processed: s.processed,
        batch_total: s.batch_len,
        more: s.more,
        warming: s.warm_left > 0 || s.flat_pending,
        playlist_id: s.playlist_id.clone(),
        saved: s.playlist_saved,
        rows,
        warning: s.warning.clone(),
    }
}

/// Pide el siguiente lote ("Cargar más"). Ahora es instantáneo: revela las
/// filas y precalienta sus primeras pistas en segundo plano.
pub fn load_more() -> Result<Snapshot, String> {
    let (id, start, end) = {
        let mut guard = session().lock().map_err(|_| "batch lock".to_string())?;
        let s = guard.as_mut().ok_or("no hay playlist abierta")?;
        match next_batch(s.all.len(), s.batch_size, s.shown) {
            Some((start, end)) => {
                s.shown = end;
                s.batch_len = end - start;
                s.processed = end;
                s.more = end < s.all.len();
                if ytmusic::throttle_active() {
                    s.warning =
                        Some("YouTube limitó las peticiones: las pistas sonarán al hacer clic".into());
                }
                (s.id, start, end)
            }
            None if !s.complete => {
                // La lista aún se está paginando en segundo plano: no es un
                // error, simplemente todavía no hay más filas. La UI reintenta.
                drop(guard);
                return Ok(progress());
            }
            None => return Err("no quedan pistas por cargar".into()),
        }
    };
    apply_cached_and_dead(id, start, end);
    // Sin polling para este precalentado: las filas ya están pintadas y el
    // payload de progress() crecería con toda la lista. El calentado igual
    // ocurre y deja las URLs en caché para que el clic sea rápido.
    start_warm(id, start, end);
    Ok(progress())
}

/// Trae el resto de la lista plana en segundo plano en UNA sola operación.
/// La primera página ya está pintada; aquí solo se amplía `all/total` y se
/// avisa a la UI mediante el polling de `progress()`. Una sola operación evita
/// que los mixes/radios (que no paginan igual que una lista fija) devuelvan
/// páginas solapadas al repetir `--playlist-start`.
fn spawn_flat_rest(id: u64, url: String, title: String, start_at: usize) {
    tokio::spawn(async move {
        let start = start_at.max(1) as u32;
        match ytmusic::import_page(&url, start, FLAT_CAP).await {
            Ok(pl) => {
                append_flat_page(id, &title, pl.tracks);
                finish_flat(id, true);
            }
            Err(e) if e.contains("no devolvió pistas") => {
                // La lista terminó justo donde empezaba esta petición (p. ej.
                // 100 exactas): no es un fallo, no hay más pistas.
                finish_flat(id, true);
            }
            Err(e) => {
                eprintln!("[lote] lista plana incompleta (quedan pistas por listar): {e}");
                finish_flat(id, false);
            }
        }
    });
}

/// Continúa una playlist por InnerTube (páginas de ~100) sin arrancar
/// Python. Si una página falla o el total declarado no cuadra, completa lo
/// que falte con yt-dlp desde la posición actual.
fn spawn_innertube_rest(
    id: u64,
    list_id: String,
    first_token: String,
    fallback_url: String,
    title: String,
    expected_total: Option<usize>,
    start_at: usize,
) {
    tokio::spawn(async move {
        let mut token = Some(first_token);
        let mut pages = 0usize;
        while let Some(tok) = token.take() {
            if !session_alive(id) {
                return;
            }
            if pages >= MAX_INNERTUBE_PAGES {
                break;
            }
            pages += 1;
            match crate::yt_reco::playlist_page(&list_id, Some(&tok)).await {
                Ok(page) => {
                    append_flat_page(id, &title, page.tracks);
                    token = page.continuation;
                }
                Err(e) => {
                    eprintln!("[lote] InnerTube falló en la página {pages}; respaldo yt-dlp: {e}");
                    let loaded = session_len(id).unwrap_or(start_at.saturating_sub(1));
                    match ytmusic::import_page(&fallback_url, (loaded + 1) as u32, FLAT_CAP).await {
                        Ok(pl) => {
                            append_flat_page(id, &title, pl.tracks);
                            finish_flat(id, true);
                        }
                        Err(e2) => {
                            eprintln!("[lote] tampoco se pudo completar por yt-dlp: {e2}");
                            finish_flat(id, false);
                        }
                    }
                    return;
                }
            }
        }
        let loaded = session_len(id).unwrap_or(0);
        if expected_total.is_some_and(|total| loaded < total) {
            eprintln!(
                "[lote] InnerTube terminó con {loaded}/{}, completando con yt-dlp",
                expected_total.unwrap_or(0)
            );
            match ytmusic::import_page(&fallback_url, (loaded + 1) as u32, FLAT_CAP).await {
                Ok(pl) => {
                    append_flat_page(id, &title, pl.tracks);
                    finish_flat(id, true);
                }
                Err(e) => {
                    eprintln!("[lote] no se pudo completar la lista: {e}");
                    finish_flat(id, false);
                }
            }
            return;
        }
        finish_flat(id, true);
    });
}

fn session_len(id: u64) -> Option<usize> {
    let guard = session().lock().ok()?;
    let s = guard.as_ref()?;
    (s.id == id).then_some(s.all.len())
}

fn append_flat_page(id: u64, title: &str, nuevas: Vec<ytmusic::YtTrack>) {
    let Ok(mut guard) = session().lock() else { return };
    let Some(s) = guard.as_mut() else { return };
    if s.id != id {
        return;
    }
    for t in nuevas {
        if s.all.len() >= FLAT_CAP as usize {
            break;
        }
        let liked = t.liked;
        let tr = t.to_track(title);
        if s.seen.insert(tr.path.clone()) {
            s.states.push(Slot::Unprepared);
            s.motivos.push(None);
            s.likes.push(liked);
            s.all.push(tr);
        }
    }
    s.more = s.shown < s.all.len();
}

fn finish_flat(id: u64, ok: bool) {
    {
        let Ok(mut guard) = session().lock() else { return };
        let Some(s) = guard.as_mut() else { return };
        if s.id != id {
            return;
        }
        s.flat_pending = false;
        s.complete = ok;
        s.more = s.shown < s.all.len();
        eprintln!(
            "[lote] lista plana {} total={}",
            if ok { "completa" } else { "incompleta" },
            s.all.len()
        );
    }
    // Fuera del lock: serializar 10 000 pistas y escribir SQLite no debe
    // bloquear a la UI ni a los hilos de precarga.
    save_cache(id);
}

fn session_alive(id: u64) -> bool {
    session()
        .lock()
        .ok()
        .is_some_and(|s| s.as_ref().is_some_and(|x| x.id == id))
}

/// Marca Lista/Muerta las filas ya conocidas sin lanzar ni un yt-dlp.
fn apply_cached_and_dead(id: u64, start: usize, end: usize) {
    let paths: Vec<String> = {
        let Ok(guard) = session().lock() else { return };
        let Some(s) = guard.as_ref() else { return };
        if s.id != id {
            return;
        }
        s.all[start..end.min(s.all.len())]
            .iter()
            .map(|t| t.path.clone())
            .collect()
    };
    if paths.is_empty() {
        return;
    }
    let cached = ytmusic::cached_paths(&paths);
    let dead = ytmusic::neg_motives(&paths);
    let Ok(mut guard) = session().lock() else { return };
    let Some(s) = guard.as_mut() else { return };
    if s.id != id {
        return;
    }
    for k in 0..paths.len() {
        let idx = start + k;
        if idx >= s.states.len() {
            break;
        }
        if let Some(motivo) = dead.get(k).and_then(|m| m.clone()) {
            s.states[idx] = Slot::Dead;
            s.motivos[idx] = Some(motivo);
        } else if cached.get(k).copied().unwrap_or(false) {
            s.states[idx] = Slot::Ready;
        } else if s.states[idx] == Slot::Dead {
            // El clic explícito limpia la caché negativa; no resucitar aquí.
            s.states[idx] = Slot::Unprepared;
            s.motivos[idx] = None;
        }
    }
}

/// Precalienta hasta `WARM_ROWS` pistas del rango en 2 hilos (cupo Warm).
fn start_warm(id: u64, start: usize, end: usize) {
    let count = end.saturating_sub(start).min(WARM_ROWS);
    if count == 0 {
        return;
    }
    // No gastar dos veces el cupo con pistas ya resueltas o muertas.
    let indexes: Vec<usize> = {
        let Ok(guard) = session().lock() else { return };
        let Some(s) = guard.as_ref() else { return };
        if s.id != id {
            return;
        }
        (start..start + count)
            .filter(|i| matches!(s.states.get(*i), Some(Slot::Unprepared)))
            .collect()
    };
    if indexes.is_empty() {
        return;
    }
    {
        let Ok(mut guard) = session().lock() else { return };
        let Some(s) = guard.as_mut() else { return };
        if s.id != id {
            return;
        }
        s.warm_left += indexes.len();
    }
    let workers = indexes.len().min(WARM_WORKERS);
    for w in 0..workers {
        let mine: Vec<usize> = indexes.iter().copied().skip(w).step_by(workers).collect();
        if mine.is_empty() {
            continue;
        }
        let pending = mine.len();
        let spawned = std::thread::Builder::new()
            .name("yt-warm-lote".into())
            .spawn(move || warm_worker(id, mine))
            .is_ok();
        if !spawned {
            // El hilo no llegó a arrancar: descuenta exactamente sus pistas.
            for _ in 0..pending {
                finish_warm(id, usize::MAX, Slot::Unprepared, None);
            }
        }
    }
}

fn warm_worker(id: u64, indexes: Vec<usize>) {
    for idx in indexes {
        if !session_alive(id) {
            // La sesión cambió: el contador vive en la sesión vieja, que ya
            // no se muestra; no hay nada que apagar aquí.
            return;
        }
        let path = {
            let Ok(guard) = session().lock() else { return };
            let Some(s) = guard.as_ref() else { return };
            if s.id != id {
                return;
            }
            match s.all.get(idx) {
                Some(t) => t.path.clone(),
                None => {
                    drop(guard);
                    finish_warm(id, idx, Slot::Unprepared, None);
                    continue;
                }
            }
        };
        let (state, motivo) = match ytmusic::resolve_audio_gen(&path, None, SlotKind::Warm) {
            Ok(_) => (Slot::Ready, None),
            Err(e) if e == ytmusic::STALE => (Slot::Unprepared, None),
            Err(e) if ytmusic::looks_like_gone(&e) => (Slot::Dead, Some(e)),
            Err(_) => (Slot::Unprepared, None),
        };
        finish_warm(id, idx, state, motivo);
    }
}

fn finish_warm(id: u64, idx: usize, state: Slot, motivo: Option<String>) {
    let Ok(mut guard) = session().lock() else { return };
    let Some(s) = guard.as_mut() else { return };
    if s.id != id {
        return;
    }
    if idx < s.states.len() {
        s.states[idx] = state;
        s.motivos[idx] = motivo;
    }
    // Si el precalentado choca con el límite de YouTube, avisa en la cabecera
    // (la UI sincroniza así la cuenta atrás del backoff).
    if state == Slot::Unprepared && ytmusic::throttle_active() && s.warning.is_none() {
        s.warning = Some("YouTube limitó las peticiones: las pistas sonarán al hacer clic".into());
    }
    s.warm_left = s.warm_left.saturating_sub(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_solo_lote_hasta_100() {
        // Regla 2/4: 80 y 100 pistas → un solo lote con todas.
        assert_eq!(batch_size_for(80), 80);
        assert_eq!(batch_size_for(100), 100);
        assert_eq!(batch_size_for(57), 57);
        assert_eq!(next_batch(80, 80, 0), Some((0, 80)));
        assert_eq!(next_batch(80, 80, 80), None);
    }

    #[test]
    fn lotes_de_50_desde_101() {
        // Regla 2/4: 101+ → lotes de 50.
        assert_eq!(batch_size_for(101), 50);
        assert_eq!(batch_size_for(300), 50);
        assert_eq!(next_batch(300, 50, 0), Some((0, 50)));
        assert_eq!(next_batch(300, 50, 250), Some((250, 300)));
        assert_eq!(next_batch(300, 50, 300), None);
        // El último lote puede ser corto (7 restantes).
        assert_eq!(next_batch(107, 50, 100), Some((100, 107)));
    }

    /// Reparte `total` en lotes y devuelve sus tamaños (50+50+14, etc.).
    fn reparto(total: usize) -> (Vec<usize>, usize) {
        let bs = batch_size_for(total);
        let mut sizes = Vec::new();
        let mut shown = 0;
        while let Some((s, e)) = next_batch(total, bs, shown) {
            sizes.push(e - s);
            shown = e;
        }
        (sizes, batch_count(total, bs))
    }

    #[test]
    fn lotes_reales_101_114_149_150_151_300() {
        // G1: totales no múltiplos de 50 → el último lote es corto y el
        // nº de lotes previsto (para el log [lote]) usa redondeo hacia arriba.
        assert_eq!(reparto(101), (vec![50, 50, 1], 3));
        assert_eq!(reparto(114), (vec![50, 50, 14], 3));
        assert_eq!(reparto(149), (vec![50, 50, 49], 3));
        assert_eq!(reparto(150), (vec![50, 50, 50], 3));
        assert_eq!(reparto(151), (vec![50, 50, 50, 1], 4));
        assert_eq!(reparto(300), (vec![50; 6], 6));
        // La suma de lotes siempre es el total (nada se pierde ni se duplica).
        for total in [101, 114, 149, 150, 151, 300] {
            let (sizes, _) = reparto(total);
            assert_eq!(sizes.iter().sum::<usize>(), total, "total={total}");
        }
        // El botón "Cargar más" aparece cuando, tras lo mostrado, queda
        // al menos 1 pista (≤100 → un solo lote y nunca hay botón).
        for (total, espera_boton) in [(100, false), (114, true), (150, true), (151, true)] {
            let (sizes, _) = reparto(total);
            let mostradas: usize = sizes.iter().take(2).sum();
            assert_eq!(mostradas < total, espera_boton, "total={total}");
        }
    }

    /// Red real: la lista plana se revela al instante, sin resolver audio.
    #[tokio::test]
    #[ignore = "red: abre un mix público"]
    async fn abre_con_filas_inmediatas() {
        let t0 = std::time::Instant::now();
        let s = open("https://music.youtube.com/watch?v=rFZHOHl-L8A&list=RDrFZHOHl-L8A".into())
            .await
            .expect("el mix público debe abrir");
        assert!(s.open);
        assert!(s.shown > 0, "debe haber filas visibles");
        assert!(!s.loading, "la carga ya no bloquea filas");
        assert!(t0.elapsed() < std::time::Duration::from_secs(30), "la lista plana debe ser rápida");
        cancel();
    }

    /// Red real: el resto de la lista llega en segundo plano y aparece en
    /// `total`/`more` sin bloquear la primera página.
    #[tokio::test]
    #[ignore = "red: pagina un mix público en segundo plano"]
    async fn completa_la_lista_en_segundo_plano() {
        let s = open("https://music.youtube.com/watch?v=rFZHOHl-L8A&list=RDrFZHOHl-L8A".into())
            .await
            .expect("el mix público debe abrir");
        let inicial = s.total;
        assert_eq!(inicial, FIRST_PAGE as usize, "debe devolver la primera página");
        let t0 = std::time::Instant::now();
        let mut creció = false;
        while t0.elapsed() < std::time::Duration::from_secs(45) {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let p = progress();
            if p.total > inicial {
                creció = true;
                break;
            }
        }
        assert!(creció, "el total debe crecer al paginar en segundo plano");
        cancel();
    }

    /// Red real + SQLite: primera apertura (InnerTube/yt-dlp), resto en
    /// segundo plano y reapertura desde caché sin red.
    #[tokio::test]
    #[ignore = "red: abre una playlist pública y mide la caché"]
    async fn cache_persistente_reabre_sin_red() {
        let dir = std::env::temp_dir().join(format!(
            "musica-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("cache.db");
        let url = "https://music.youtube.com/playlist?list=PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI";
        let first = open_with_cache(url.to_string(), Some(db.clone()))
            .await
            .expect("primera apertura");
        assert!(first.open && first.shown > 0);
        assert!(first.warming, "el resto debe estar cargándose en fondo");
        // Espera a que termine el listado de fondo y se escriba la caché.
        let t0 = std::time::Instant::now();
        while t0.elapsed() < std::time::Duration::from_secs(60) {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if !progress().warming {
                break;
            }
        }
        let full = progress();
        assert!(full.total > 100, "la playlist debe superar la primera página");
        cancel();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let t1 = std::time::Instant::now();
        let cached = open_with_cache(url.to_string(), Some(db))
            .await
            .expect("reapertura desde caché");
        assert!(t1.elapsed() < std::time::Duration::from_secs(2), "debe ser instantánea");
        assert_eq!(cached.total, full.total);
        assert!(cached.shown > 0 && cached.shown < cached.total);
        cancel();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn estados_de_fila() {
        assert_eq!(Slot::Ready.row(), RowState::Ready);
        assert_eq!(Slot::Dead.row(), RowState::Dead);
        assert_eq!(Slot::Unprepared.row(), RowState::Unprepared);
    }
}
