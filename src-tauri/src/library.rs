//! Escaneo incremental de archivos locales (proveedor a).
//! Decisión: comparar mtime+size antes de leer tags porque leer 50k con lofty es lo caro.

use crate::db;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// webm: los que produce "Original (sin recodificar)" son de audio, pero el
// contenedor también admite vídeo; en scan_folder el sniffer EBML decide.
const SUPPORTED: &[&str] = &["mp3", "flac", "ogg", "opus", "m4a", "wav", "aac", "webm"];

pub(crate) fn is_supported(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Quita el prefijo numérico de playlist ("01 - ", "12 - ") del nombre base.
/// Solo dígitos (1..=3) seguidos de " - "; cualquier otra cosa se queda igual.
pub(crate) fn quita_prefijo_numerico(stem: &str) -> &str {
    let bytes = stem.as_bytes();
    let mut i = 0;
    while i < bytes.len() && i < 3 && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && stem[i..].starts_with(" - ") {
        stem[i + 3..].trim_start()
    } else {
        stem
    }
}

/// "Dua Lipa - Levitating" → ("Dua Lipa", "Levitating"): parte por el PRIMERO
/// " - ", como pide el formato "Artista - Título" que escribe download.rs.
/// Sin separador (o partes vacías) → ("", nombre entero como título).
pub(crate) fn split_artista_titulo(stem: &str) -> (String, String) {
    match stem.split_once(" - ") {
        Some((a, t)) if !a.trim().is_empty() && !t.trim().is_empty() => {
            (a.trim().to_string(), t.trim().to_string())
        }
        _ => (String::new(), stem.trim().to_string()),
    }
}

fn file_mtime(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Lee tags con lofty; si falla usa el nombre del archivo como título.
/// Reusado por playlists (agregar carpeta) para la misma calidad de metadatos.
pub(crate) fn read_tags(path: &Path) -> db::Track {
    use lofty::prelude::{Accessor, AudioFile, TaggedFileExt};
    let (mut title, mut artist, mut album, mut genre, mut year, mut track_no, mut dur) =
        (String::new(), String::new(), String::new(), String::new(), 0, 0, 0i64);
    if let Ok(f) = lofty::probe::Probe::open(path).and_then(|p| p.read()) {
        dur = f.properties().duration().as_secs() as i64;
        if let Some(tag) = f.primary_tag() {
            title = tag.title().as_deref().unwrap_or("").to_string();
            artist = tag.artist().as_deref().unwrap_or("").to_string();
            album = tag.album().as_deref().unwrap_or("").to_string();
            genre = tag.genre().as_deref().unwrap_or("").to_string();
            year = tag.year().unwrap_or(0) as i32;
            track_no = tag.track().unwrap_or(0) as i32;
        }
    }
    // Sin etiqueta de título: el nombre del archivo manda. Quita el prefijo
    // numérico de playlist ("01 - ") y, si hay "Artista - Título", reparte las
    // dos partes (partiendo por el PRIMERO " - "). Un título real de etiqueta
    // jamás se toca; un artista etiquetado tampoco (solo se rellena vacío).
    if title.is_empty() {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Sin título");
        let limpio = quita_prefijo_numerico(stem);
        let (a, t) = split_artista_titulo(limpio);
        title = t;
        if artist.is_empty() {
            artist = a;
        }
    }
    db::Track {
        id: 0,
        path: path.to_string_lossy().to_string(),
        title,
        artist,
        album,
        genre,
        year,
        track_no,
        duration_secs: dur,
        mtime: file_mtime(path),
        art_url: String::new(),
    }
}

pub struct ScanStats {
    pub added: usize,
    pub kept: usize,
    pub removed: usize,
}

/// Recorre recursivamente; solo relee tags si mtime cambió; purga borrados de ESTA carpeta.
pub fn scan_folder(db_path: &Path, folder: &Path) -> Result<ScanStats, String> {
    let conn = db::open(db_path).map_err(|e| e.to_string())?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut added = 0usize;
    let mut kept = 0usize;

    for entry in WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let p: PathBuf = entry.path().to_path_buf();
        if !is_supported(&p) {
            continue;
        }
        let sp = p.to_string_lossy().to_string();
        // WebM: lofty no lee Matroska, así que el sniffer EBML decide si es
        // audio (sin vídeo) y aporta la Duration. Vídeo, basura o EBML
        // truncado: no se indexa (sin romper el escaneo); si antes sí que
        // estaba indexado, al no entrar en `seen` delete_missing lo purga.
        let mut dur_webm: Option<i64> = None;
        if p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("webm"))
        {
            match crate::webm_sniff::sniff_webm_file(&p, crate::webm_sniff::READ_LIMIT) {
                None => continue,
                Some(info) => dur_webm = info.duration_secs.map(|d| d as i64),
            }
        }
        seen.insert(sp.clone());
        let mtime = file_mtime(&p);
        match db::mtime_of(&conn, &sp) {
            Some(old) if old == mtime => kept += 1, // sin cambios: no toca tags
            _ => {
                let mut t = read_tags(&p);
                if t.duration_secs == 0 {
                    t.duration_secs = dur_webm.unwrap_or(0);
                }
                db::upsert(&conn, &t).map_err(|e| e.to_string())?;
                added += 1;
            }
        }
    }
    let removed =
        db::delete_missing(&conn, &seen, &folder.to_string_lossy()).map_err(|e| e.to_string())?;
    Ok(ScanStats { added, kept, removed })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Escribe un MP3 mínimo con tag ID3v2.3 válido (TIT2/TPE1/TALB) + frames falsos.
    fn write_min_mp3(path: &Path, title: &str, artist: &str, album: &str) {
        fn frame(id: &[u8; 4], text: &str) -> Vec<u8> {
            let mut data = vec![0u8]; // encoding latin1
            data.extend_from_slice(text.as_bytes());
            let mut out = Vec::new();
            out.extend_from_slice(id);
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(&[0u8, 0u8]); // flags
            out.extend(data);
            out
        }
        let mut tag = Vec::new();
        tag.extend(frame(b"TIT2", title));
        tag.extend(frame(b"TPE1", artist));
        tag.extend(frame(b"TALB", album));
        let n = tag.len() as u32; // tamaño syncsafe
        let mut file = vec![
            b'I', b'D', b'3', 3, 0, 0,
            ((n >> 21) & 0x7f) as u8, ((n >> 14) & 0x7f) as u8,
            ((n >> 7) & 0x7f) as u8, (n & 0x7f) as u8,
        ];
        file.extend(tag);
        for _ in 0..10 {
            file.extend([0xFF, 0xFB, 0x90, 0x00]); // cabecera MPEG Layer III 128kbps
            file.extend(vec![0u8; 413]);
        }
        std::fs::write(path, file).unwrap();
    }

    /// Split del nombre: prefijo numérico y "Artista - Título" por el primero.
    #[test]
    fn nombre_prefijo_y_split() {
        // Descarga de playlist: "01 - Artista - Título".
        assert_eq!(quita_prefijo_numerico("01 - Dua Lipa - Levitating"), "Dua Lipa - Levitating");
        let (a, t) = split_artista_titulo("Dua Lipa - Levitating");
        assert_eq!((a.as_str(), t.as_str()), ("Dua Lipa", "Levitating"));
        // Sin artista en el nombre: "01 - Levitating" queda como título solo.
        assert_eq!(quita_prefijo_numerico("01 - Levitating"), "Levitating");
        let (a, t) = split_artista_titulo("Levitating");
        assert_eq!((a.as_str(), t.as_str()), ("", "Levitating"));
        // Reglas defensivas: sin prefijo no se toca; partes vacías no crean artefactos.
        assert_eq!(quita_prefijo_numerico("1984 - Orwell"), "1984 - Orwell");
        let (a, t) = split_artista_titulo(" - Título");
        assert!(a.is_empty() && t == "- Título");
        let (a, t) = split_artista_titulo("Artista - ");
        assert!(a.is_empty() && t == "Artista -"); // trim final: sin artefactos
    }

    /// Un MP3 con etiquetas REALES no cambia aunque el nombre diga otra cosa.
    #[test]
    fn mp3_etiquetado_intacto() {
        let root = std::env::temp_dir().join(format!("musica-tag-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("01 - Nombre Falso - Invented.webm.mp3");
        write_min_mp3(&p, "Titulo Real", "Artista Real", "Album Real");
        let t = read_tags(&p);
        assert_eq!(t.title, "Titulo Real");
        assert_eq!(t.artist, "Artista Real");
        assert_eq!(t.album, "Album Real");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Escaneo end-to-end con webm: el audio entra (con nombre y duración);
    /// vídeo, basura y EBML cortado se saltan sin romper el escaneo.
    #[test]
    fn scan_webm_audio_entra_resto_se_salta() {
        use std::process::Stdio;
        let root = std::env::temp_dir().join(format!("musica-scanwebm-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let ffmpeg = |out: &Path, args: &[&str]| -> bool {
            crate::tools::command("ffmpeg")
                .args(args)
                .arg("-y")
                .arg(out)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        let audio = root.join("01 - Dua Lipa - Levitating.webm");
        if !ffmpeg(
            &audio,
            &["-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-c:a", "libopus"],
        ) {
            eprintln!("ffmpeg sin libopus: test de escaneo webm omitido");
            std::fs::remove_dir_all(&root).ok();
            return;
        }
        assert!(
            ffmpeg(
                &root.join("video.webm"),
                &["-f", "lavfi", "-i", "color=c=red:s=320x240:d=1", "-c:v", "libvpx"],
            ),
            "ffmpeg debe generar el webm de vídeo"
        );
        std::fs::write(root.join("basura.webm"), [0x1Au8, 0x45, 0xDF, 0xA3, 0x42, 0x00, 0x99]).unwrap();
        let bytes = std::fs::read(&audio).unwrap();
        // Corte DENTRO de Tracks (no a media vida: un corte posterior a una
        // lista de pistas completa sigue siendo audio verificable por diseño).
        let tid = [0x16u8, 0x54, 0xAE, 0x6B];
        let pos = bytes.windows(4).position(|w| w == tid).expect("Tracks presente");
        std::fs::write(root.join("cortado.webm"), &bytes[..pos + 2]).unwrap();
        // Un MP3 con etiquetas en la misma carpeta: no debe verse afectado por el gate.
        write_min_mp3(&root.join("mp3.mp3"), "Titulo MP3", "Artista MP3", "Album");

        let dbp = root.join("scan.db");
        let s = scan_folder(&dbp, &root).unwrap();
        assert_eq!(s.added, 2, "solo el webm de audio y el mp3 deben entrar");
        let conn = db::open(&dbp).unwrap();
        let r = db::search(&conn, "Levitating", 50, 0).unwrap();
        assert_eq!(r.len(), 1, "el webm de audio debe estar indexado por su nombre");
        assert_eq!(r[0].artist, "Dua Lipa");
        assert_eq!(r[0].duration_secs, 2, "la duración viene de la Duration EBML");
        assert!(db::search(&conn, "video", 50, 0).unwrap().is_empty());
        assert!(db::search(&conn, "basura", 50, 0).unwrap().is_empty());
        assert!(db::search(&conn, "cortado", 50, 0).unwrap().is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    /// Medición opt-in: `cargo test -- --ignored --nocapture`.
    /// Genera 5000 MP3 etiquetados y cronometra escaneo + búsquedas.
    #[test]
    #[ignore]
    fn perf_scan_5k() {
        let root = std::env::temp_dir().join(format!("musica-perf-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let t0 = std::time::Instant::now();
        for i in 0..5000 {
            let sub = root.join(format!("album-{:02}", i % 50));
            std::fs::create_dir_all(&sub).unwrap();
            write_min_mp3(
                &sub.join(format!("track-{i:05}.mp3")),
                &format!("Titulo {i}"),
                &format!("Artista {:03}", i % 200),
                &format!("Album {:02}", i % 50),
            );
        }
        let gen = t0.elapsed();
        let dbp = root.join("bench.db");
        let t1 = std::time::Instant::now();
        let s1 = scan_folder(&dbp, &root).unwrap();
        let scan1 = t1.elapsed();
        let t2 = std::time::Instant::now();
        let s2 = scan_folder(&dbp, &root).unwrap(); // todo en caché mtime
        let scan2 = t2.elapsed();
        let conn = db::open(&dbp).unwrap();
        let t3 = std::time::Instant::now();
        let r1 = db::search(&conn, "Titulo", 300, 0).unwrap();
        let q1 = t3.elapsed();
        let t4 = std::time::Instant::now();
        let r2 = db::search(&conn, "Artista 007", 300, 0).unwrap();
        let q2 = t4.elapsed();
        println!(
            "perf: fixture={gen:?} scan_frio={scan1:?} (added={} kept={}) scan_caliente={scan2:?} (kept={}) fts_300={q1:?} (n={}) fts_filtrada={q2:?} (n={})",
            s1.added, s1.kept, s2.kept, r1.len(), r2.len()
        );
        assert_eq!(s1.added + s1.kept, 5000);
        assert_eq!(r1.len(), 300);
        std::fs::remove_dir_all(&root).ok();
    }
}
