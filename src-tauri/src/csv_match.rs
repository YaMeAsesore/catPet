//! S3: emparejado de filas CSV con candidatos de YouTube.
//! - `score_match` y `best_match` son PUROS (sin red ni caché): umbrales
//!   explícitos y tests puros. Estados: exacta / dudosa / sin_match / no_encontrada.
//! - El comando `csv_match` orquesta: caché negativa CSV (nivel lista) + una
//!   búsqueda por texto con el cupo compartido de ytmusic.
//! - Los vídeos muertos NO usan la clave CSV: los marca la cadena de resolución
//!   existente (neg_store del id de vídeo) y aquí solo se descartan al elegir.
//! - S4: el candidato vive en la UI hasta que `csv_save_playlist` guarda la
//!   lista (playlist local: m3u8 con `ytmusic://` + metadatos originales del
//!   CSV) y `csv_variants` da candidatos FRESCOS para «Buscar otra versión».
//! - S4.2a (título permisivo, reglas documentadas aquí):
//!   1) antes de comparar se pliegan mayúsculas/diacríticos latinos (NFD +
//!      strip de combinantes, hecho a mano sin dependencias) y se recorta el
//!      ruido final (official/audio/hd/hq/4k/live/videoclip/visualizer/
//!      remaster… y los pares «en vivo»/«en directo»);
//!   2) `title_ok` = el título del CSV está CONTENIDO en el del candidato,
//!      O contención de la parte corta con ratio ≥ 0.6 (antes: contención
//!      obligatoria y ratio ≥ 0.8, causa de los sin_match en masa);
//!   3) `sin_match` queda RESERVADO a «ningún candidato contiene el título
//!      mínimamente»: cualquier dudosa se acepta automáticamente (el chip y
//!      el toast ya avisan);
//!   4) si el pase 1 («título artista») queda en sin_match, se reintenta UNA
//!      vez con solo el título (`match_passes`): una petición extra solo en el
//!      caso malo, y el negativo de lista se guarda solo tras agotarlo.

use crate::csv_import::CsvTrack;
use crate::db::Track;
use crate::playlists::{self, SaveOutcome};
use crate::ytmusic::{self, SlotKind, YtTrack};
use crate::AppState;
use serde::Serialize;
use tauri::State;

/// Candidatos por búsqueda (S4.2a). Valor EFECTIVO: 10 — el núcleo clampa el
/// count a 1..=10 (`ytmusic::search_candidates`), así que este 12 solo expresa
/// intención; no ajustarlo sin revisar antes ese clamp (S4.2b.1: se documenta
/// el valor real para evitar confusión futura). Una invocación de yt-dlp
/// devuelve metadatos planos.
const SEARCH_COUNT: u32 = 12;

/// Desfase de duración tolerado (segundos) entre CSV y candidato. El límite
/// es INCLUSIVO (≤ 5 s): una diferencia de exactamente 5 s sigue contando como
/// exacta (regla fijada en S3, cubierta por `duracion_en_el_limite_no_penaliza`).
const DUR_TOLERANCE_S: i64 = 5;

/// Regla de título (S4.2a): si el título del CSV está CONTENIDO en el del
/// candidato, ya vale; si no, contención de la parte corta con este ratio.
/// (Antes: 0.8 con contención obligatoria, y "Canal - Título Video Oficial"
/// delataba un sin_match falso.)
const TITLE_MIN_RATIO: f64 = 0.6;

/// Grupos de marcadores de "otra versión" (sinónimos en el mismo grupo).
/// Si el candidato trae un grupo que el original no tiene → dudosa.
const VERSION_MARKERS: &[&[&str]] = &[
    &["live", "en vivo", "directo"],
    &["remix"],
    &["cover"],
    &["acoustic", "acustico"],
];

/// Ruido de YouTube que no cambia la canción (se recorta al final del título).
/// S4.2a amplía la lista: hq/8k, videoclip, visualizer, remaster(ed) y "live"
/// (un directo suelto pasa a dudosa por marcador, nunca a sin_match).
const NOISE_WORDS: &[&str] = &[
    "official", "oficial", "video", "audio", "lyric", "lyrics", "hd", "hq", "4k", "8k",
    "videoclip", "visualizer", "remaster", "remastered", "live",
];

/// Pares de ruido que se recortan juntos al final ("en vivo", "en directo").
const NOISE_PAIRS: &[&str] = &["en vivo", "en directo"];

/// Motivos estables; el chip muestra el prefijo antes de ":" y el resto va al tooltip.
pub const MOTIVO_NO_CANDIDATOS: &str = "no_encontrada:0 resultados";
pub const MOTIVO_ERROR: &str = "no_encontrada:error permanente (429/red)";
pub const MOTIVO_SIN_MATCH: &str = "sin_match:ningún candidato supera umbral";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchState {
    Exacta,
    Dudosa,
    SinMatch,
    NoEncontrada,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatchResult {
    pub state: MatchState,
    pub candidate: Option<YtTrack>,
    pub motivo: String,
}

/// Pliega un carácter: minúscula Unicode y sin el diacrítico de las letras
/// latinas habituales. Equivale a NFD + quitar marcas combinantes para el
/// rango latino, hecho a mano para no añadir dependencias.
fn fold_char(c: char) -> char {
    let lc = c.to_lowercase().next().unwrap_or(c);
    match lc {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'ǎ' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'í' | 'ì' | 'î' | 'ï' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'ǒ' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' => 't',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' | 'ǔ' => 'u',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

/// Minúsculas, sin acentos y solo [a-z0-9 ] con espacios simples; el contenido
/// entre paréntesis/corchetes se descarta ("(Official Video)" no es título).
fn norm_text(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            _ => {
                let f = fold_char(c);
                if f.is_ascii_alphanumeric() {
                    out.push(f);
                } else if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
            }
        }
    }
    let t = out.trim().to_string();
    // Sufijos de canal autogenerado: no distinguen artista.
    let t = t.strip_suffix(" topic").unwrap_or(&t).to_string();
    let t = t.strip_suffix("vevo").unwrap_or(&t).trim_end().to_string();
    t
}

/// Recorta palabras de ruido al final ("... official video", "... lyrics hd")
/// y los pares de S4.2a ("... en vivo", "... en directo").
fn strip_noise(n: &str) -> String {
    let mut parts: Vec<&str> = n.split(' ').filter(|p| !p.is_empty()).collect();
    loop {
        if parts.len() >= 2 {
            let par = format!("{} {}", parts[parts.len() - 2], parts[parts.len() - 1]);
            if NOISE_PAIRS.contains(&par.as_str()) {
                parts.truncate(parts.len() - 2);
                continue;
            }
        }
        match parts.last() {
            Some(p) if NOISE_WORDS.contains(p) => { parts.pop(); }
            _ => break,
        }
    }
    parts.join(" ")
}

/// ¿El título del candidato corresponde al del CSV? (Regla S4.2a, documentada
/// en la cabecera del módulo.)
/// 1) Igualdad tras normalizar.
/// 2) El título del CSV contenido tal cual en el del candidato: basta (muchos
///    canales anteponen "Artista - " o cuelan ruido tras el título).
/// 3) En cualquier otro caso, contención de la parte corta en la larga con
///    ratio ≥ TITLE_MIN_RATIO (0.6): cubre que el CSV sea más largo.
fn title_ok(orig: &str, cand: &str) -> bool {
    let a = strip_noise(&norm_text(orig));
    let b = strip_noise(&norm_text(cand));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b || b.contains(a.as_str()) {
        return true;
    }
    let (short, long) = if a.len() <= b.len() { (&a, &b) } else { (&b, &a) };
    long.contains(short.as_str()) && short.len() as f64 / long.len() as f64 >= TITLE_MIN_RATIO
}

/// Conserva puntuación al plegar: "(en vivo)" debe poder detectarse.
fn fold_keep_punct(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

/// ¿`text` trae el marcador como palabra (los compuestos, como "en vivo",
/// se buscan por subcadena)?
fn has_marker(text: &str, marker: &str) -> bool {
    let t = fold_keep_punct(text);
    if marker.contains(' ') {
        t.contains(marker)
    } else {
        t.split(|c: char| !c.is_alphanumeric()).any(|w| w == marker)
    }
}

/// Grupo de marcadores que el candidato trae y el original no (otra versión).
fn version_conflict(orig_title: &str, orig_album: &str, cand_title: &str) -> Option<&'static str> {
    let orig = format!("{orig_title} {orig_album}");
    for group in VERSION_MARKERS {
        let Some(ci) = (0..group.len()).find(|&i| has_marker(cand_title, group[i])) else {
            continue;
        };
        if !(0..group.len()).any(|i| has_marker(&orig, group[i])) {
            return Some(group[ci]);
        }
    }
    None
}

/// El artista debe confirmar por igualdad normalizada (los canales "X - Topic"
/// / "XVEVO" ya se pliegan). El CSV sin artista se trata antes, en `score_match`.
fn artist_ok(orig: &str, cand: &str) -> bool {
    let a = norm_text(orig);
    let b = norm_text(cand);
    !a.is_empty() && !b.is_empty() && b != "na" && a == b
}

/// Duración: tolera ±DUR_TOLERANCE_S. Si falta en cualquiera de los dos lados
/// NO penaliza (decisión S3 documentada: un CSV sin columna de duración no
/// debe volver todo dudoso; la UI ya avisa de esa carencia).
fn dur_ok(orig: Option<u32>, cand: i64) -> bool {
    match (orig, cand) {
        (Some(a), c) if c > 0 => (a as i64 - c).abs() <= DUR_TOLERANCE_S,
        _ => true,
    }
}

fn dudosa(c: &YtTrack, motivo: String) -> MatchResult {
    MatchResult { state: MatchState::Dudosa, candidate: Some(c.clone()), motivo }
}

/// Decide el estado de un candidato frente a la fila CSV (puro).
/// `None` = la búsqueda no devolvió nada.
pub fn score_match(original: &CsvTrack, candidate: Option<&YtTrack>) -> MatchResult {
    let Some(c) = candidate else {
        return MatchResult { state: MatchState::NoEncontrada, candidate: None, motivo: MOTIVO_NO_CANDIDATOS.into() };
    };
    if !title_ok(&original.title, &c.title) {
        return MatchResult { state: MatchState::SinMatch, candidate: None, motivo: MOTIVO_SIN_MATCH.into() };
    }
    if let Some(m) = version_conflict(&original.title, &original.album, &c.title) {
        return dudosa(c, format!("dudosa:posible otra versión ({m})"));
    }
    let artista = norm_text(&original.artist);
    if artista.is_empty() || artista == "na" || artista == "desconocido" {
        // Sin artista no se puede confirmar: nunca exacta (como mucho dudosa).
        return dudosa(c, "dudosa:sin artista en el CSV".into());
    }
    if !artist_ok(&original.artist, &c.artist) {
        return dudosa(c, "dudosa:artista distinto".into());
    }
    if !dur_ok(original.duration_s, c.duration_secs) {
        return dudosa(c, "dudosa:duración fuera de ±5 s".into());
    }
    MatchResult { state: MatchState::Exacta, candidate: Some(c.clone()), motivo: "exacta:título y artista coinciden".into() }
}

/// Elige el mejor candidato: la primera exacta, o entre las dudosas la de
/// duración más cercana al CSV (las sin dato pierden por lejos). Puro.
pub fn best_match(original: &CsvTrack, candidates: &[YtTrack]) -> MatchResult {
    let mut elegida: Option<(i64, MatchResult)> = None;
    for c in candidates {
        let r = score_match(original, Some(c));
        match r.state {
            MatchState::Exacta => return r,
            MatchState::Dudosa => {
                let d = match (original.duration_s, c.duration_secs) {
                    (Some(a), b) if b > 0 => (a as i64 - b).abs(),
                    _ => i64::MAX,
                };
                if elegida.as_ref().map_or(true, |(bd, _)| d < *bd) {
                    elegida = Some((d, r));
                }
            }
            _ => {}
        }
    }
    match elegida {
        Some((_, r)) => r,
        None if candidates.is_empty() => score_match(original, None),
        None => MatchResult { state: MatchState::SinMatch, candidate: None, motivo: MOTIVO_SIN_MATCH.into() },
    }
}

/// Clave estable de la fila (título + artista normalizados): md5 ya es
/// dependencia y evita guardar texto del usuario en la caché.
pub fn csv_key(t: &CsvTrack) -> String {
    format!("{:x}", md5::compute(format!("{}|{}", norm_text(&t.title), norm_text(&t.artist))))
}

/// Envoltorio de la caché negativa existente para claves CSV (nivel lista).
pub fn neg_get_csv(md5: &str) -> Option<String> {
    ytmusic::neg_get(&format!("ytmusic://csv:{md5}"))
}

fn neg_store_csv(md5: &str, motivo: &str) {
    ytmusic::neg_store(&format!("csv:{md5}"), motivo);
    // UNA línea por clave: permite verificar que una fila sin match se marca
    // una sola vez y no se reintenta en bucle (sin títulos ni artistas).
    eprintln!("[csv-neg] clave=csv:{md5} motivo={motivo}");
}

/// Reconstruye el estado al releer la caché negativa (el prefijo manda).
fn result_from_motive(motivo: String) -> MatchResult {
    let state = if motivo.starts_with("sin_match") { MatchState::SinMatch } else { MatchState::NoEncontrada };
    MatchResult { state, candidate: None, motivo }
}

/// Descarta los candidatos marcados como no disponibles por la cadena de
/// resolución (caché negativa de VÍDEO). No escribe nada: solo consulta.
/// `is_dead` se inyecta para poder probar la omisión sin red ni caché global.
fn skip_dead(candidates: &[YtTrack], is_dead: &dyn Fn(&str) -> bool) -> Vec<YtTrack> {
    candidates.iter().filter(|c| !is_dead(&c.id)).cloned().collect()
}

/// Fila del selector «Buscar otra versión» (S4): candidato + veredicto puro.
/// `flatten` deja el JSON como un YtTrack con `state` y `motivo` extra.
#[derive(Debug, Clone, Serialize)]
pub struct VariantRow {
    #[serde(flatten)]
    pub track: YtTrack,
    pub state: MatchState,
    pub motivo: String,
}

/// Mapea candidatos a filas del selector (puro): mismo scoring que el
/// emparejado, pero SIN tocar ninguna caché (ni de lista ni de vídeo): cada
/// apertura de «Buscar otra versión» muestra lo que devuelva YouTube ahora.
fn variant_rows(original: &CsvTrack, cands: &[YtTrack]) -> Vec<VariantRow> {
    cands
        .iter()
        .map(|c| {
            let r = score_match(original, Some(c));
            VariantRow { track: c.clone(), state: r.state, motivo: r.motivo }
        })
        .collect()
}

/// S4.2c: consulta de «Buscar otra versión». Sin texto manual, la de siempre
/// (título + artista del original); con texto, se respeta tal cual. Los
/// veredictos SIEMPRE se calculan contra el original (`variant_rows`), así que
/// los chips no dependen de la consulta escrita.
fn variant_query(original: &CsvTrack, manual: Option<&str>) -> String {
    match manual.map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => q.chars().take(200).collect(),
        None => format!("{} {}", original.title.trim(), original.artist.trim())
            .trim()
            .chars()
            .take(200)
            .collect(),
    }
}

/// S4.2a: orquesta los DOS pases con el cupo dado y decide el negativo de
/// lista (como en S3: «0 resultados» solo si el pase 1 no trajo candidatos;
/// «sin match» solo tras agotar el reintento). Sincrónica (bloquea): el comando
/// la ejecuta en spawn_blocking y el banco manual la usa directa.
fn match_passes(original: CsvTrack, slot: SlotKind) -> Result<MatchResult, String> {
    let key = csv_key(&original);
    let query: String = format!("{} {}", original.title.trim(), original.artist.trim())
        .trim()
        .chars()
        .take(200)
        .collect();
    let cands = ytmusic::search_candidates(&query, SEARCH_COUNT, slot)?;
    // Nivel vídeo: los muertos/age-gate se descartan al elegir; solo se
    // CONSULTA esa caché (omitirlos no escribe la clave csv:<md5>).
    let vivos = skip_dead(&cands, &|id| ytmusic::neg_get(&format!("ytmusic://{id}")).is_some());
    let mut r = if cands.is_empty() {
        neg_store_csv(&key, MOTIVO_NO_CANDIDATOS);
        result_from_motive(MOTIVO_NO_CANDIDATOS.into())
    } else {
        best_match(&original, &vivos)
    };
    // Pase 2: si NINGÚN candidato del pase 1 contenía el título, UN reintento
    // con solo el título (un artista inventado/parafraseado en el CSV estorba
    // la búsqueda). Una petición extra solo en el caso malo; los errores del
    // reintento se ignoran y se conserva el veredicto del pase 1.
    if r.state == MatchState::SinMatch {
        let q2: String = original.title.trim().chars().take(200).collect();
        if let Ok(cands2) = ytmusic::search_candidates(&q2, SEARCH_COUNT, slot) {
            let vivos2 = skip_dead(&cands2, &|id| ytmusic::neg_get(&format!("ytmusic://{id}")).is_some());
            let r2 = best_match(&original, &vivos2);
            if matches!(r2.state, MatchState::Exacta | MatchState::Dudosa) {
                r = r2;
            }
        }
        if r.state == MatchState::SinMatch {
            neg_store_csv(&key, MOTIVO_SIN_MATCH);
        }
    }
    Ok(r)
}

/// Comando: empareja una fila CSV (persistir es cosa de `csv_save_playlist`).
/// `warm=true` usa el cupo de precarga: con backoff/plazo se rinde en silencio
/// (Err(STALE)) y la UI detiene el calentado. Los errores de red del clic se
/// devuelven como "no encontrada" SIN cachear (son transitorios).
#[tauri::command]
pub async fn csv_match(
    title: String,
    artist: String,
    album: String,
    duration_s: Option<u32>,
    warm: bool,
) -> Result<MatchResult, String> {
    let original = CsvTrack { title, artist, album, duration_s };
    let key = csv_key(&original);
    // Nivel lista: un negativo ya conocido responde al instante (sin yt-dlp).
    if let Some(motivo) = neg_get_csv(&key) {
        return Ok(result_from_motive(motivo));
    }
    let slot = if warm { SlotKind::Warm } else { SlotKind::User };
    match tokio::task::spawn_blocking(move || match_passes(original, slot)).await {
        Ok(Ok(r)) => Ok(r),
        // El calentado cede sin ruido (backoff de límite, cupo o plazo): la UI para.
        Ok(Err(_)) if warm => Err(ytmusic::STALE.into()),
        Ok(Err(_)) => Ok(result_from_motive(MOTIVO_ERROR.into())),
        Err(e) => Err(format!("tarea de búsqueda: {e}")),
    }
}

/// Comando S4 «Buscar otra versión»: candidatos FRESCOS para una pista ya
/// guardada (los metadatos llegan del m3u8, no del CSV original). No consulta
/// ni escribe ninguna caché: la lista pudo cambiar desde el último emparejado.
/// S4.2c: `query` opcional permite una búsqueda libre; los chips se siguen
/// calculando contra el título/artista/álbum/duración originales.
#[tauri::command]
pub async fn csv_variants(
    title: String,
    artist: String,
    album: String,
    duration_s: Option<u32>,
    query: Option<String>,
) -> Result<Vec<VariantRow>, String> {
    let original = CsvTrack { title, artist, album, duration_s };
    let query = variant_query(&original, query.as_deref());
    let cands = match tokio::task::spawn_blocking(move || ytmusic::search_candidates(&query, SEARCH_COUNT, SlotKind::User)).await {
        Ok(Ok(v)) => v,
        // Mensaje estable: nunca el stderr crudo de yt-dlp (podría traer la
        // consulta o URLs).
        Ok(Err(_)) => return Err("no se pudo buscar en YouTube (429/red); reinténtalo".into()),
        Err(e) => return Err(format!("tarea de búsqueda: {e}")),
    };
    Ok(variant_rows(&original, &cands))
}

/// Comando S4: guarda la lista emparejada como playlist local reutilizando el
/// flujo de siempre (`<datos-app>/playlists/<nombre>/playlist.m3u8`).
/// `Duplicate` no escribe nada: la UI avisa y ofrece reemplazar.
#[tauri::command]
pub fn csv_save_playlist(
    state: State<'_, AppState>,
    name: String,
    tracks: Vec<Track>,
    replace: bool,
) -> Result<SaveOutcome, String> {
    if tracks.is_empty() {
        return Err("no hay pistas emparejadas que guardar".into());
    }
    playlists::save_tracks(&state.playlists_root, &name, &tracks, replace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csv(title: &str, artist: &str, album: &str, dur: Option<u32>) -> CsvTrack {
        CsvTrack { title: title.into(), artist: artist.into(), album: album.into(), duration_s: dur }
    }

    fn yt(title: &str, artist: &str, dur: i64) -> YtTrack {
        YtTrack { id: "abc123".into(), title: title.into(), artist: artist.into(), duration_secs: dur, thumbnail: String::new(), liked: None }
    }

    #[test]
    fn normalizacion_descarta_parentesis_topic_y_vevo() {
        assert_eq!(norm_text("Canción (Official Video)"), "cancion");
        assert_eq!(norm_text("Grupo X - Topic"), "grupo x");
        assert_eq!(norm_text("AnaVEVO"), "ana");
        assert!(title_ok("Cancion Uno", "Canción Uno (Video Oficial)"));
        assert!(title_ok("Cancion Uno", "Cancion Uno Lyrics HD"));
    }

    #[test]
    fn exacta_con_canal_topic_y_ruido() {
        let o = csv("Cancion Uno", "Grupo X", "Album Azul", Some(201));
        let r = score_match(&o, Some(&yt("Cancion Uno (Official Video)", "Grupo X - Topic", 203)));
        assert_eq!(r.state, MatchState::Exacta);
        assert!(r.candidate.is_some());
    }

    #[test]
    fn duracion_en_el_limite_no_penaliza() {
        let o = csv("Cancion Uno", "Grupo X", "", Some(200));
        assert_eq!(score_match(&o, Some(&yt("Cancion Uno", "Grupo X", 205))).state, MatchState::Exacta);
        assert_eq!(score_match(&o, Some(&yt("Cancion Uno", "Grupo X", 195))).state, MatchState::Exacta);
    }

    #[test]
    fn duracion_fuera_del_limite_es_dudosa() {
        let o = csv("Cancion Uno", "Grupo X", "", Some(200));
        let r = score_match(&o, Some(&yt("Cancion Uno", "Grupo X", 206)));
        assert_eq!(r.state, MatchState::Dudosa);
        assert!(r.motivo.contains("fuera de"));
    }

    #[test]
    fn artista_distinto_es_dudosa() {
        let o = csv("Cancion Uno", "Grupo X", "", Some(200));
        let r = score_match(&o, Some(&yt("Cancion Uno", "Otro Canal", 200)));
        assert_eq!(r.state, MatchState::Dudosa);
        assert!(r.motivo.contains("artista"));
    }

    #[test]
    fn marcador_de_otra_version_es_dudosa() {
        let o = csv("Cancion Uno", "Grupo X", "Album Azul", Some(200));
        for (t, aguja) in [
            ("Cancion Uno (Live)", "live"),
            ("Cancion Uno (Remix)", "remix"),
            ("Cancion Uno (Cover)", "cover"),
            ("Cancion Uno (Acoustic)", "acoustic"),
            ("Cancion Uno (En Vivo)", "vivo"),
        ] {
            let r = score_match(&o, Some(&yt(t, "Grupo X", 200)));
            assert_eq!(r.state, MatchState::Dudosa, "{t}");
            assert!(r.motivo.contains(aguja), "{}", r.motivo);
        }
    }

    #[test]
    fn mismo_marcador_en_el_original_no_penaliza() {
        let o = csv("Cancion Uno (En Vivo)", "Grupo X", "Album Azul", Some(200));
        let r = score_match(&o, Some(&yt("Cancion Uno (Live)", "Grupo X", 200)));
        assert_eq!(r.state, MatchState::Exacta);
    }

    #[test]
    fn duracion_desconocida_no_penaliza() {
        let a = csv("Cancion Uno", "Grupo X", "", None);
        assert_eq!(score_match(&a, Some(&yt("Cancion Uno", "Grupo X", 200))).state, MatchState::Exacta);
        let b = csv("Cancion Uno", "Grupo X", "", Some(200));
        assert_eq!(score_match(&b, Some(&yt("Cancion Uno", "Grupo X", 0))).state, MatchState::Exacta);
    }

    #[test]
    fn sin_artista_en_csv_nunca_exacta() {
        for a in ["", "NA", "na"] {
            let o = csv("Cancion Uno", a, "", Some(200));
            let r = score_match(&o, Some(&yt("Cancion Uno", "Grupo X", 200)));
            assert_eq!(r.state, MatchState::Dudosa, "artista={a:?}");
            assert!(r.motivo.contains("sin artista"));
        }
    }

    #[test]
    fn titulo_distinto_es_sin_match() {
        let o = csv("Cancion Uno", "Grupo X", "", Some(200));
        let r = score_match(&o, Some(&yt("Otra Cancion Distinta", "Grupo X", 200)));
        assert_eq!(r.state, MatchState::SinMatch);
        assert!(r.candidate.is_none());
        assert!(r.motivo.starts_with("sin_match:"));
    }

    #[test]
    fn sin_candidatos_es_no_encontrada() {
        let o = csv("Cancion Uno", "Grupo X", "", None);
        assert_eq!(score_match(&o, None).state, MatchState::NoEncontrada);
        assert_eq!(best_match(&o, &[]).state, MatchState::NoEncontrada);
        assert!(MOTIVO_NO_CANDIDATOS.starts_with("no_encontrada:"));
    }

    #[test]
    fn best_match_prefiere_exacta_y_luego_la_mas_cercana() {
        let o = csv("Cancion Uno", "Grupo X", "", Some(200));
        let lejos = yt("Cancion Uno", "Otro Canal", 230);
        let cerca = yt("Cancion Uno", "Otro Canal", 201);
        let exacta = yt("Cancion Uno", "Grupo X", 205);
        let r = best_match(&o, &[lejos.clone(), cerca]);
        assert_eq!(r.state, MatchState::Dudosa);
        assert_eq!(r.candidate.unwrap().duration_secs, 201);
        assert_eq!(best_match(&o, &[exacta]).state, MatchState::Exacta);
    }

    #[test]
    fn video_muerto_en_cache_negativa_se_omite_sin_escribir_la_clave_csv() {
        let o = csv("Cancion Uno", "Grupo X", "", Some(200));
        // Vídeo marcado como muerto en la caché negativa real (memoria, sin red;
        // id de prueba único para no chocar con otros tests).
        ytmusic::neg_store("csvtest-muerto", "yt-dlp: Video unavailable");
        let mut muerto = yt("Cancion Uno", "Grupo X", 200);
        muerto.id = "csvtest-muerto".into();
        let mut vivo = yt("Cancion Uno", "Grupo X", 201);
        vivo.id = "csvtest-vivo".into();
        let es_muerto = |id: &str| ytmusic::neg_get(&format!("ytmusic://{id}")).is_some();
        // Solo se CONSULTA la caché de vídeo: el muerto se omite y sigue el vivo.
        let vivos = skip_dead(&[muerto, vivo], &es_muerto);
        assert_eq!(vivos.len(), 1, "el vídeo muerto se omite");
        assert_eq!(vivos[0].id, "csvtest-vivo", "se pasa al siguiente candidato");
        let r = best_match(&o, &vivos);
        assert_eq!(r.state, MatchState::Exacta);
        assert_eq!(r.candidate.unwrap().id, "csvtest-vivo");
        // Y la clave csv:<md5> del original NO se escribió por culpa del muerto.
        assert!(neg_get_csv(&csv_key(&o)).is_none(), "un vídeo muerto no escribe csv:<md5>");
        ytmusic::neg_clear("ytmusic://csvtest-muerto"); // higiene de la caché global
    }

    #[test]
    fn variantes_mapean_el_veredicto_sin_tocar_caches() {
        let o = csv("Cancion Uno", "Grupo X", "Album Azul", Some(200));
        let exacta = yt("Cancion Uno", "Grupo X", 204);
        // El marcador va entre paréntesis: el título empareja, el veredicto es dudoso.
        let mut live = yt("Cancion Uno (En Vivo)", "Grupo X", 200);
        live.id = "vid-live".into();
        let mala = yt("Otra Cosa", "Otro Canal", 100);
        // `variant_rows` es pura: no consulta ni escribe ninguna caché; el
        // "muerto" hipotético no se filtra aquí (la UI enseña lo que hay).
        let rows = variant_rows(&o, &[exacta, live, mala]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].state, MatchState::Exacta);
        assert_eq!(rows[1].state, MatchState::Dudosa);
        assert_eq!(rows[1].track.id, "vid-live");
        assert!(rows[1].motivo.starts_with("dudosa:posible otra versión"));
        assert_eq!(rows[2].state, MatchState::SinMatch);
        // El motivo viaja para el tooltip (misma convención "prefijo:resto").
        assert!(rows.iter().all(|r| r.motivo.contains(':')));
    }

    // ---- S4.2a: título permisivo (los 12 tests de S3 quedan intactos) ----

    #[test]
    fn s42a_feat_en_parentesis_no_es_sin_match() {
        let o = csv("Titulo (feat. X)", "Grupo", "", Some(200));
        assert_eq!(score_match(&o, Some(&yt("Titulo", "Grupo", 202))).state, MatchState::Exacta);
    }

    #[test]
    fn s42a_official_video_no_es_sin_match() {
        let o = csv("Titulo", "Grupo", "", Some(200));
        assert_eq!(score_match(&o, Some(&yt("Titulo (Official Video)", "Grupo", 200))).state, MatchState::Exacta);
    }

    #[test]
    fn s42a_uploader_topic_no_penaliza() {
        let o = csv("Titulo Uno", "Artista", "", Some(200));
        assert_eq!(score_match(&o, Some(&yt("Titulo Uno", "Artista - Topic", 200))).state, MatchState::Exacta);
    }

    #[test]
    fn s42a_acentos_distintos_no_penalizan() {
        let o = csv("Canción Ñandú", "Artísta", "", Some(200));
        assert_eq!(score_match(&o, Some(&yt("Cancion Nandu", "Artista", 204))).state, MatchState::Exacta);
    }

    #[test]
    fn s42a_canal_antepuesto_y_ruido_final() {
        let o = csv("Blinding Nights", "Grupo", "", Some(200));
        // Caso real: el canal antepone su nombre y cuela ruido al final.
        let c = yt("Grupo - Blinding Nights Official Video HD", "Grupo", 201);
        assert_eq!(score_match(&o, Some(&c)).state, MatchState::Exacta);
    }

    #[test]
    fn s42a_ratio_060_con_candidato_corto() {
        let o = csv("Cancion Uno Deluxe", "Grupo", "", Some(200));
        // 11/18 ≈ 0.61 ≥ 0.6: la contención del candidato corto cuenta.
        assert_eq!(score_match(&o, Some(&yt("Cancion Uno", "Grupo", 200))).state, MatchState::Exacta);
    }

    #[test]
    fn s42a_remaster_y_live_suelto() {
        let o = csv("Cancion Uno", "Grupo", "", Some(200));
        // Corchetes: el contenido se descarta al normalizar.
        let r = score_match(&o, Some(&yt("Cancion Uno [Remastered 2011]", "Grupo", 200)));
        assert_eq!(r.state, MatchState::Exacta);
        // "live" suelto al final: el título empareja y el marcador baja a dudosa.
        let r = score_match(&o, Some(&yt("Cancion Uno Live", "Grupo", 200)));
        assert_eq!(r.state, MatchState::Dudosa);
        assert!(r.motivo.contains("live"));
    }

    #[test]
    fn s42a_sin_match_solo_con_titulos_ajenos() {
        let o = csv("Cancion Uno", "Grupo", "", Some(200));
        let r = score_match(&o, Some(&yt("Documental Sobre Otra Cosa", "Otro Canal", 60)));
        assert_eq!(r.state, MatchState::SinMatch);
        assert!(r.motivo.starts_with("sin_match:"));
    }

    #[test]
    fn s42c_consulta_de_variantes_respeta_la_busqueda_libre() {
        let o = csv("Cancion Uno", "Grupo X", "", None);
        assert_eq!(variant_query(&o, None), "Cancion Uno Grupo X");
        assert_eq!(variant_query(&o, Some("  Daft Punk Get Lucky  ")), "Daft Punk Get Lucky");
        assert_eq!(variant_query(&o, Some("   ")), "Cancion Uno Grupo X");
        let larga = variant_query(&o, Some(&"x".repeat(300)));
        assert_eq!(larga.chars().count(), 200, "la consulta se recorta a 200");
    }

    /// Banquillo manual (red real): mide el flujo COMPLETO del comando
    /// (`match_passes`: scoring S4.2a + reintento por título) sobre CSV reales.
    /// Uso: MUSICA_S42_CSV=ruta1[,ruta2] cargo test banco_manual_42a -- --ignored --nocapture
    #[test]
    #[ignore = "banco manual S4.2a (red real): MUSICA_S42_CSV=ruta1,ruta2"]
    fn banco_manual_42a() {
        let Ok(paths) = std::env::var("MUSICA_S42_CSV") else {
            eprintln!("[banco] define MUSICA_S42_CSV con una o dos rutas separadas por coma");
            return;
        };
        for (ix, p) in paths.split(',').map(str::trim).filter(|s| !s.is_empty()).enumerate() {
            let bytes = std::fs::read(p).expect("csv ilegible");
            let prev = crate::csv_import::parse_bytes(&bytes, "banco.csv").expect("csv inválido");
            let (mut exa, mut dud, mut sin, mut none, mut err) = (0, 0, 0, 0, 0);
            for row in &prev.rows {
                match match_passes(row.clone(), SlotKind::User) {
                    Ok(r) => match r.state {
                        MatchState::Exacta => exa += 1,
                        MatchState::Dudosa => dud += 1,
                        MatchState::SinMatch => sin += 1,
                        MatchState::NoEncontrada => none += 1,
                    },
                    Err(_) => err += 1,
                }
            }
            // Solo recuentos: nunca títulos, artistas ni rutas.
            eprintln!("[banco] csv=#{} filas={} exacta={exa} dudosa={dud} sin_match={sin} no_encontrada={none} errores={err}",
                ix + 1, prev.rows.len());
        }
    }
}
