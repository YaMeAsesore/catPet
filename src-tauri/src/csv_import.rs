//! S2: importación de listas desde CSV (parser + vista previa).
//!
//! Solo lectura local y sin red: lee el archivo, detecta la cabecera, normaliza
//! las filas (título, artista, álbum, duración) y las devuelve a la UI. La
//! búsqueda en YouTube (ytsearch) y el pegado al m3u8 llegaron en S3/S4
//! (`csv_match.rs`).
//!
//! Privacidad: los títulos/artistas del usuario solo viajan en la respuesta IPC
//! hacia la UI; este módulo no los escribe en logs ni en mensajes de error.
//! Codificación: se exige UTF-8 válido; si no lo es, error claro (sin adivinar).

use serde::Serialize;

/// Tope de tamaño del archivo: por encima, se pide dividirlo o reexportarlo.
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;
/// Tope de filas útiles que se devuelven en la vista previa.
pub const MAX_ROWS: usize = 5000;

/// Fila normalizada de una lista CSV (lo que la UI pinta como «sin emparejar»).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CsvTrack {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_s: Option<u32>,
}

/// Resultado de la vista previa: recuento + filas útiles.
#[derive(Debug, Clone, Serialize)]
pub struct CsvPreview {
    pub file: String,
    pub total: usize,
    pub skipped: usize,
    pub truncated: bool,
    pub has_duration: bool,
    pub rows: Vec<CsvTrack>,
}

/// Alias de cabecera aceptados (se comparan ya normalizados: minúsculas y sin
/// acentos). Cubren exportadores típicos (Exportify, TuneMyMusic…) y español.
const TITLE_ALIASES: &[&str] = &[
    "title", "titulo", "name", "nombre", "track", "track name", "track title",
    "song", "song name", "song title", "cancion",
];
const ARTIST_ALIASES: &[&str] = &[
    "artist", "artista", "artists", "artistas", "artist name", "artist name(s)",
    "artist names",
];
const ALBUM_ALIASES: &[&str] = &[
    "album", "album name", "album title", "album name(s)", "disco",
];
const DURATION_ALIASES: &[&str] = &[
    "duration", "duracion", "length", "time", "track duration",
    "track duration (ms)", "duration (ms)", "duracion (ms)", "duration_ms",
    "duracion_ms", "duration ms", "duracion ms",
];

/// Minúsculas + sin acentos para comparar cabeceras («Álbum» → «album»).
fn norm(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

fn find_col(cells: &[String], aliases: &[&str]) -> Option<usize> {
    cells.iter().position(|c| {
        let n = norm(c);
        aliases.contains(&n.as_str())
    })
}

/// Separador más probable de la primera línea (`,` `;` o tabulador).
/// Heurística por conteo; los empates se quedan con `,` (el caso normal).
pub fn sniff_delimiter(first_line: &str) -> u8 {
    let mut best = (b',', first_line.matches(',').count());
    for ch in [';', '\t'] {
        let n = first_line.matches(ch).count();
        if n > best.1 {
            best = (ch as u8, n);
        }
    }
    best.0
}

/// Duración a segundos. Formatos aceptados: `m:ss`, `h:mm:ss`, segundos
/// (`225`) y milisegundos (`225000`; también si la cabecera trae «(ms)»).
/// Devuelve `None` si la celda está vacía o no es un tiempo razonable.
pub fn parse_duration(raw: &str, ms_hint: bool) -> Option<u32> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains(':') {
        let mut secs: u64 = 0;
        for part in s.split(':') {
            let n: u64 = part.trim().parse().ok()?;
            secs = secs.checked_mul(60)?.checked_add(n)?;
        }
        return u32::try_from(secs).ok().filter(|secs| *secs > 0);
    }
    let n: f64 = s.replace(',', ".").parse().ok()?;
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    // Heurística de magnitud: no hay pistas de ≥ 30 000 s (8 h 20 min), así
    // que a partir de ahí el número viene en milisegundos.
    let secs = if ms_hint || n >= 30_000.0 { n / 1000.0 } else { n };
    if secs < 0.5 {
        return None;
    }
    Some(secs.round().min(u32::MAX as f64) as u32)
}

/// Parsea el contenido de un CSV ya leído a bytes.
///
/// Exige cabecera reconocible (columna de título obligatoria); las filas
/// vacías o sin título se cuentan como descartes; nunca se inventan valores.
pub fn parse_bytes(bytes: &[u8], file_name: &str) -> Result<CsvPreview, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        "El archivo no es UTF-8 válido; vuelve a exportar la lista en UTF-8.".to_string()
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first_line = text.lines().next().unwrap_or("");
    let delim = sniff_delimiter(first_line);

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delim)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());

    let mut records = rdr.records();
    let header: Vec<String> = match records.next() {
        Some(Ok(r)) => r.iter().map(str::to_string).collect(),
        Some(Err(_)) => return Err("El CSV no se pudo leer en la cabecera.".to_string()),
        None => return Err("El archivo está vacío.".to_string()),
    };

    let title_col = find_col(&header, TITLE_ALIASES)
        .ok_or("No encuentro la columna de título en la cabecera (p. ej. «title» o «título»).")?;
    let artist_col = find_col(&header, ARTIST_ALIASES);
    let album_col = find_col(&header, ALBUM_ALIASES);
    let dur_col = find_col(&header, DURATION_ALIASES);
    let dur_ms = dur_col.is_some_and(|i| norm(&header[i]).contains("ms"));

    let mut out = CsvPreview {
        file: file_name.to_string(),
        total: 0,
        skipped: 0,
        truncated: false,
        has_duration: dur_col.is_some(),
        rows: Vec::new(),
    };
    let cell =
        |r: &csv::StringRecord, i: Option<usize>| i.and_then(|i| r.get(i)).unwrap_or("").to_string();

    for (n, rec) in records.enumerate() {
        let rec = rec.map_err(|_| format!("CSV ilegible a partir de la fila {}", n + 2))?;
        if rec.iter().all(|c| c.trim().is_empty()) {
            out.skipped += 1;
            continue;
        }
        let title = cell(&rec, Some(title_col));
        if title.trim().is_empty() {
            out.skipped += 1;
            continue;
        }
        out.total += 1;
        if out.rows.len() >= MAX_ROWS {
            out.truncated = true;
            continue;
        }
        let dur_raw = cell(&rec, dur_col);
        out.rows.push(CsvTrack {
            title,
            artist: cell(&rec, artist_col),
            album: cell(&rec, album_col),
            duration_s: parse_duration(&dur_raw, dur_ms),
        });
    }
    Ok(out)
}

/// Vista previa de un CSV local (p. ej. una playlist exportada desde Spotify).
/// Solo lee el archivo; no hay red, escritura ni logs con datos del usuario.
#[tauri::command]
pub async fn csv_preview(path: String) -> Result<CsvPreview, String> {
    let meta =
        std::fs::metadata(&path).map_err(|e| format!("No se pudo abrir el archivo: {e}"))?;
    if meta.len() > MAX_BYTES {
        return Err(
            "El archivo supera 20 MB; divídelo o exporta solo las columnas necesarias.".to_string(),
        );
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("No se pudo leer el archivo: {e}"))?;
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    parse_bytes(&bytes, &name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPORTIFY: &str = "Track Name,Artist Name(s),Album Name,Track Duration (ms)\n\
\"Hello, World\",Ana,Disco uno,225000\n\
Otra,Bea,Disco dos,180500\n";

    #[test]
    fn exportify_ingles_con_comillas_y_ms() {
        let p = parse_bytes(EXPORTIFY.as_bytes(), "lista.csv").unwrap();
        assert_eq!(p.file, "lista.csv");
        assert_eq!(p.total, 2);
        assert_eq!(p.skipped, 0);
        assert!(!p.truncated);
        assert!(p.has_duration);
        assert_eq!(p.rows[0].title, "Hello, World");
        assert_eq!(p.rows[0].artist, "Ana");
        assert_eq!(p.rows[0].album, "Disco uno");
        assert_eq!(p.rows[0].duration_s, Some(225));
        assert_eq!(p.rows[1].duration_s, Some(181));
    }

    #[test]
    fn cabecera_espanola_punto_y_coma_y_bom() {
        let t = "\u{feff}Título;Artista;Álbum;Duración\nCanción uno;Grupo X;Álbum Y;3:45\n";
        let p = parse_bytes(t.as_bytes(), "x.csv").unwrap();
        assert_eq!(p.rows.len(), 1);
        assert_eq!(p.rows[0].title, "Canción uno");
        assert_eq!(p.rows[0].artist, "Grupo X");
        assert_eq!(p.rows[0].duration_s, Some(225));
    }

    #[test]
    fn tabulador_y_fila_con_menos_columnas() {
        let t = "Title\tArtist\tAlbum\nSolo título\nOtra\tDúo\n";
        let p = parse_bytes(t.as_bytes(), "x.csv").unwrap();
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].artist, "");
        assert_eq!(p.rows[1].artist, "Dúo");
    }

    #[test]
    fn sin_cabecera_reconocible_es_error() {
        let e = parse_bytes(b"uno,dos,tres\na,b,c\n", "x.csv").unwrap_err();
        assert!(e.contains("título"));
    }

    #[test]
    fn utf8_invalido_es_error_claro() {
        let bytes = [b'T', b'i', 0xff, 0xfe, b'\n'];
        let e = parse_bytes(&bytes, "x.csv").unwrap_err();
        assert!(e.contains("UTF-8"));
    }

    #[test]
    fn filas_vacias_y_sin_titulo_se_descartan() {
        let t = "title,artist\n,Anon\n,,\nSolo título,\n";
        let p = parse_bytes(t.as_bytes(), "x.csv").unwrap();
        assert_eq!(p.total, 1);
        assert_eq!(p.skipped, 2);
        assert_eq!(p.rows[0].title, "Solo título");
    }

    #[test]
    fn sin_columna_de_duracion_no_inventa() {
        let t = "title,artist\nA,B\n";
        let p = parse_bytes(t.as_bytes(), "x.csv").unwrap();
        assert!(!p.has_duration);
        assert_eq!(p.rows[0].duration_s, None);
    }

    #[test]
    fn duraciones_tipicas() {
        for (raw, ms, want) in [
            ("3:45", false, Some(225)),
            ("0:59", false, Some(59)),
            ("1:02:03", false, Some(3723)),
            ("225", false, Some(225)),
            ("225000", false, Some(225)),
            ("225.4", false, Some(225)),
            ("225,5", false, Some(226)),
            ("225000", true, Some(225)),
            ("3:45", true, Some(225)),
            ("", false, None),
            ("abc", false, None),
            ("0:00", false, None),
            ("0", false, None),
            ("180", true, None),
        ] {
            assert_eq!(parse_duration(raw, ms), want, "raw={raw} ms={ms}");
        }
    }

    #[test]
    fn mas_de_max_rows_se_trunca_sin_perder_cuenta() {
        let mut t = String::from("title,artist\n");
        for i in 0..(MAX_ROWS + 3) {
            t.push_str(&format!("T{i},A\n"));
        }
        let p = parse_bytes(t.as_bytes(), "grande.csv").unwrap();
        assert!(p.truncated);
        assert_eq!(p.rows.len(), MAX_ROWS);
        assert_eq!(p.total, MAX_ROWS + 3);
    }

    #[test]
    fn archivo_vacio_es_error() {
        assert!(parse_bytes(b"", "x.csv").unwrap_err().contains("vacío"));
    }
}
