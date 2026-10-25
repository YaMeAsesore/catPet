// ¿Es este .webm un AUDIO de verdad?
//
// lofty 0.22 no lee Matroska/WebM (su FileType no lo contempla), así que el
// escáner no puede distinguir con lofty un .webm de audio de uno de vídeo ni
// de basura. Este módulo es un parser EBML propio, MINIMAL y acotado:
//
// - Solo se leen los primeros `READ_LIMIT` bytes del archivo (la cabecera
//   EBML, Info y Tracks viven siempre al principio; Cluster, que ocupa casi
//   todo el archivo, se ignora por completo).
// - Sin dependencias, sin panics: todo acceso a datos pasa por `get(..)` y
//   sumas con `checked_add`.
// - Basura, EBML truncado y archivos con pista de vídeo = "no es audio"
//   (`None`). Un corte del archivo DESPUÉS de la lista de pistas completa no
//   invalida el audio: la verificación de pistas ya se completó.
//
// Política de decisión: entra solo si hay ≥1 pista de audio (TrackType=2 o
// CodecID "A_*") y NINGUNA de vídeo (TrackType=1 o CodecID "V_*").

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Tope de lectura: cabecera + Info + Tracks caben con holgura en 64 KiB.
pub const READ_LIMIT: u64 = 64 * 1024;

// IDs EBML (con su prefijo de longitud incluido, como define la especificación).
const ID_EBML: u32 = 0x1A45DFA3;
const ID_SEGMENT: u32 = 0x18538067;
const ID_INFO: u32 = 0x1549A966;
const ID_TIMECODE_SCALE: u32 = 0x2AD7B1;
const ID_DURATION: u32 = 0x4489;
const ID_TRACKS: u32 = 0x1654AE6B;
const ID_TRACK_ENTRY: u32 = 0xAE;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_CODEC_ID: u32 = 0x86;

const TRACK_TYPE_VIDEO: u64 = 1;
const TRACK_TYPE_AUDIO: u64 = 2;

/// Lo que el sniffer pudo confirmar del archivo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WebmInfo {
    /// Duración en segundos si Info la declaraba (nunca inventada).
    pub duration_secs: Option<u64>,
}

/// Lee como mucho `limit` bytes del archivo y decide si es audio indexable.
pub fn sniff_webm_file(path: &Path, limit: u64) -> Option<WebmInfo> {
    let f = File::open(path).ok()?;
    let mut buf = Vec::with_capacity(8 * 1024);
    f.take(limit).read_to_end(&mut buf).ok()?;
    sniff_webm(&buf)
}

enum Size {
    Known(u64),
    Unknown, // VINT con todos los bits de valor a 1 (streaming)
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    /// ID de elemento EBML (1..=4 bytes; el prefijo de longitud va incluido).
    fn read_id(&mut self) -> Option<u32> {
        let b0 = *self.data.get(self.pos)?;
        let len = if b0 & 0x80 != 0 {
            1
        } else if b0 & 0x40 != 0 {
            2
        } else if b0 & 0x20 != 0 {
            3
        } else if b0 & 0x10 != 0 {
            4
        } else {
            return None; // IDs de 5..=8 bytes no existen en EBML
        };
        let mut v: u32 = 0;
        for i in 0..len {
            v = (v << 8) | (*self.data.get(self.pos + i)? as u32);
        }
        self.pos += len;
        Some(v)
    }

    /// Tamaño del payload. `None` = cabecera ilegible (basura).
    fn read_size(&mut self) -> Option<Size> {
        let b0 = *self.data.get(self.pos)?;
        let len = if b0 & 0x80 != 0 {
            1
        } else if b0 & 0x40 != 0 {
            2
        } else if b0 & 0x20 != 0 {
            3
        } else if b0 & 0x10 != 0 {
            4
        } else if b0 & 0x08 != 0 {
            5
        } else if b0 & 0x04 != 0 {
            6
        } else if b0 & 0x02 != 0 {
            7
        } else if b0 & 0x01 != 0 {
            8
        } else {
            return None; // 0x00 no es un VINT válido
        };
        let bits = 7 * len; // bits de valor en total
        let mask: u64 = (1u64 << bits) - 1;
        let mut v: u64 = (b0 as u64) & (0xFFu64 >> len);
        for i in 1..len {
            v = (v << 8) | (*self.data.get(self.pos + i)? as u64);
        }
        self.pos += len;
        if v == mask {
            Some(Size::Unknown)
        } else {
            Some(Size::Known(v))
        }
    }

    /// Entero big-endian de 1..=8 bytes (0 bytes = 0).
    fn read_uint(&self, from: usize, to: usize) -> Option<u64> {
        let mut v: u64 = 0;
        for i in from..to {
            v = (v << 8) | (*self.data.get(i)? as u64);
        }
        Some(v)
    }

    /// Float EBML: 4 bytes = f32, 8 = f64 (big-endian).
    fn read_float(&self, from: usize, to: usize) -> Option<f64> {
        let n = to.checked_sub(from)?;
        match n {
            4 => {
                let b = self.data.get(from..to)?;
                Some(f32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64)
            }
            8 => {
                let b = self.data.get(from..to)?;
                Some(f64::from_be_bytes([
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                ]))
            }
            _ => None,
        }
    }
}

/// ¿El buffer (acotado) es un WebM con pista de audio y sin pista de vídeo?
pub fn sniff_webm(data: &[u8]) -> Option<WebmInfo> {
    let mut c = Cursor::new(data);
    // 1) Cabecera EBML: obligatoria; sin ella es basura.
    if c.read_id()? != ID_EBML {
        return None;
    }
    match c.read_size()? {
        Size::Known(n) => {
            let end = c.pos.checked_add(n as usize)?;
            if end > data.len() {
                return None; // cabecera cortada
            }
            c.pos = end;
        }
        Size::Unknown => return None, // la cabecera nunca usa "unknown size"
    }
    // 2) Segment: su payload es el resto del archivo (o hasta donde llegue el
    //    buffer: lo que importa —Info y Tracks— está al principio).
    if c.read_id()? != ID_SEGMENT {
        return None;
    }
    let seg_end = match c.read_size()? {
        Size::Known(n) => c.pos.checked_add(n as usize)?.min(data.len()),
        Size::Unknown => data.len(),
    };

    let mut timescale: u64 = 1_000_000; // por defecto, 1 unidad = 1 ms
    let mut duration_units: Option<f64> = None;
    let mut audio = false;
    let mut video = false;
    let mut tracks_ok = false; // lista de pistas completa y verificada

    while c.pos < seg_end {
        let id = c.read_id()?;
        let (end, payload_excede) = match c.read_size()? {
            Size::Known(n) => match c.pos.checked_add(n as usize) {
                Some(e) if e <= seg_end => (e, false),
                Some(e) => (seg_end, e > data.len()), // ¿cortado o solo Cluster gigante?
                None => return None,
            },
            Size::Unknown => (seg_end, false),
        };
        if end < c.pos || end > data.len() {
            return None;
        }
        match id {
            ID_INFO => parse_info(&mut c, end, &mut timescale, &mut duration_units),
            ID_TRACKS => {
                // Sin lista de pistas completa DENTRO del buffer no decidimos:
                // es basura, vídeo cortado o algo que no podemos verificar.
                if payload_excede || end < c.pos {
                    return None;
                }
                parse_tracks(&mut c, end, &mut audio, &mut video);
                tracks_ok = true;
            }
            _ => {
                // Cluster u otro elemento: si su payload se sale del buffer es
                // normal (solo leemos los primeros KB). Tras tracks_ok, paramos.
                if payload_excede {
                    break;
                }
            }
        }
        c.pos = end;
    }

    if !tracks_ok || !audio || video {
        return None;
    }
    let duration_secs = duration_units
        .map(|u| (u * timescale as f64) / 1e9)
        .filter(|s| s.is_finite() && *s >= 0.0 && *s <= 86_400.0 * 7.0)
        .map(|s| s.round() as u64);
    Some(WebmInfo { duration_secs })
}

/// Info: TimecodeScale (entero, ns por unidad) y Duration (en esas unidades).
fn parse_info(c: &mut Cursor<'_>, end: usize, timescale: &mut u64, duration: &mut Option<f64>) {
    while c.pos < end {
        let Some(id) = c.read_id() else {
            return; // truncado dentro de Info: se ignora (la pistas mandan)
        };
        let Some(size) = c.read_size() else {
            return;
        };
        let p_end = match size {
            Size::Known(n) => match c.pos.checked_add(n as usize) {
                Some(e) if e <= end => e,
                _ => return,
            },
            Size::Unknown => return,
        };
        match id {
            ID_TIMECODE_SCALE => {
                if let Some(v) = c.read_uint(c.pos, p_end) {
                    if v > 0 {
                        *timescale = v;
                    }
                }
            }
            ID_DURATION => {
                *duration = c.read_float(c.pos, p_end);
            }
            _ => {}
        }
        c.pos = p_end;
    }
}

/// Tracks: recorre TrackEntry y clasifica cada pista (vídeo manda sobre audio).
fn parse_tracks(c: &mut Cursor<'_>, end: usize, audio: &mut bool, video: &mut bool) {
    while c.pos < end {
        let Some(id) = c.read_id() else {
            return; // truncado: lo visto se conserva; tracks_ok igualmente (payload completo)
        };
        let Some(size) = c.read_size() else {
            return;
        };
        let p_end = match size {
            Size::Known(n) => match c.pos.checked_add(n as usize) {
                Some(e) if e <= end => e,
                _ => return,
            },
            Size::Unknown => return,
        };
        if id == ID_TRACK_ENTRY {
            parse_track_entry(c, p_end, audio, video);
        }
        c.pos = p_end;
    }
}

/// Un TrackEntry: TrackType (0x83) y CodecID (0x86) deciden audio/vídeo.
fn parse_track_entry(c: &mut Cursor<'_>, end: usize, audio: &mut bool, video: &mut bool) {
    let mut ttype: Option<u64> = None;
    let mut codec = String::new();
    while c.pos < end {
        let Some(id) = c.read_id() else {
            return;
        };
        let Some(size) = c.read_size() else {
            return;
        };
        let p_end = match size {
            Size::Known(n) => match c.pos.checked_add(n as usize) {
                Some(e) if e <= end => e,
                _ => return,
            },
            Size::Unknown => return,
        };
        match id {
            ID_TRACK_TYPE => ttype = c.read_uint(c.pos, p_end),
            ID_CODEC_ID => {
                if let Some(bytes) = c.data.get(c.pos..p_end) {
                    codec = String::from_utf8_lossy(bytes).into_owned();
                }
            }
            _ => {}
        }
        c.pos = p_end;
    }
    // Vídeo gana: un archivo con audio Y vídeo no es música descargada.
    if ttype == Some(TRACK_TYPE_VIDEO) || codec.starts_with("V_") {
        *video = true;
    }
    if ttype == Some(TRACK_TYPE_AUDIO) || codec.starts_with("A_") {
        *audio = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- fixtures EBML mínimas (bytes escritos a mano) ----------

    /// Tamaño EBML como VINT (1 byte si cabe en 6 bits, etc.).
    fn vint(n: u64) -> Vec<u8> {
        for len in 1..=8usize {
            let bits = 7 * len;
            if n < (1u64 << bits) - 1 {
                let mut out = vec![0u8; len];
                let mut v = n;
                for i in (0..len).rev() {
                    out[i] = (v & 0xFF) as u8;
                    v >>= 8;
                }
                out[0] |= 0x80u8 >> (len - 1);
                return out;
            }
        }
        unreachable!("n demasiado grande para un VINT")
    }

    fn elem(id: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.extend(vint(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    fn uint_be(v: u64) -> Vec<u8> {
        let mut b = v.to_be_bytes().to_vec();
        while b.len() > 1 && b[0] == 0 {
            b.remove(0);
        }
        b
    }

    /// WebM mínimo: cabecera EBML + Segment(Info + Tracks(pistas…)).
    /// duration: en unidades de TimecodeScale (ms por defecto).
    fn fixture_webm(pistas: &[(&str, u64)], duration: Option<f64>) -> Vec<u8> {
        let mut info = elem(&[0x2A, 0xD7, 0xB1], &uint_be(1_000_000)); // TimecodeScale
        if let Some(d) = duration {
            info.extend(elem(&[0x44, 0x89], &d.to_be_bytes())); // Duration f64
        }
        let mut tracks_payload = Vec::new();
        for (codec, ttype) in pistas {
            let mut te = elem(&[0x83], &uint_be(*ttype)); // TrackType
            te.extend(elem(&[0x86], codec.as_bytes())); // CodecID
            tracks_payload.extend(elem(&[0xAE], &te)); // TrackEntry
        }
        let mut seg = elem(&[0x15, 0x49, 0xA9, 0x66], &info); // Info
        seg.extend(elem(&[0x16, 0x54, 0xAE, 0x6B], &tracks_payload)); // Tracks
        let mut out = elem(&[0x1A, 0x45, 0xDF, 0xA3], b"\x42\x86\x81\x01"); // cabecera EBML
        out.extend(elem(&[0x18, 0x53, 0x80, 0x67], &seg)); // Segment
        out
    }

    // ---------- decisión: audio/vídeo/basura/corte ----------

    #[test]
    fn audio_entra_con_duration() {
        let buf = fixture_webm(&[("A_OPUS", 2)], Some(2500.0));
        let info = sniff_webm(&buf).expect("webm de audio debe entrar");
        assert_eq!(info.duration_secs, Some(3)); // 2500 ms → 3 s (redondeo)
    }

    #[test]
    fn audio_sin_duration_entra_igual() {
        let buf = fixture_webm(&[("A_OPUS", 2)], None);
        let info = sniff_webm(&buf).expect("sin Duration también es audio");
        assert_eq!(info.duration_secs, None);
    }

    #[test]
    fn audio_con_vorbis_entra() {
        let buf = fixture_webm(&[("A_VORBIS", 2)], None);
        assert!(sniff_webm(&buf).is_some());
    }

    #[test]
    fn video_se_salta() {
        let buf = fixture_webm(&[("V_VP9", 1)], None);
        assert!(sniff_webm(&buf).is_none(), "vídeo no debe indexarse");
    }

    #[test]
    fn video_mas_audio_se_salta() {
        // Un MV con música Y vídeo (típico "video webm"): no es pista de música.
        let buf = fixture_webm(&[("V_VP8", 1), ("A_OPUS", 2)], None);
        assert!(sniff_webm(&buf).is_none());
    }

    #[test]
    fn basura_aleatoria_se_salta() {
        // PRNG determinista (xorshift): sin dependencias ni aleatoriedad real.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut buf = Vec::with_capacity(4096);
        for _ in 0..4096 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            buf.push((x & 0xFF) as u8);
        }
        assert!(sniff_webm(&buf).is_none(), "basura no debe parecer audio");
    }

    #[test]
    fn sin_pistas_se_salta() {
        // Cabecera y Segment correctos, pero sin ningún TrackEntry.
        let info = elem(&[0x15, 0x49, 0xA9, 0x66], &[]);
        let mut out = elem(&[0x1A, 0x45, 0xDF, 0xA3], b"\x42\x86\x81\x01");
        out.extend(elem(&[0x18, 0x53, 0x80, 0x67], &info));
        assert!(sniff_webm(&out).is_none());
    }

    #[test]
    fn ebml_cortado_se_salta() {
        let buf = fixture_webm(&[("A_OPUS", 2)], Some(1000.0));
        for cut in [3, 10, buf.len() / 2, buf.len() - 1] {
            assert!(
                sniff_webm(&buf[..cut]).is_none(),
                "corte en {cut} no debe pasar por audio"
            );
        }
    }

    #[test]
    fn vacio_se_salta() {
        assert!(sniff_webm(&[]).is_none());
    }

    #[test]
    fn header_invalido_se_salta() {
        // No empieza por el ID EBML (0x1A45DFA3).
        assert!(sniff_webm(b"\x00\x00\x00\x00algo").is_none());
    }

    // ---------- prueba real con ffmpeg (opt-in) ----------
    // `cargo test webm_real -- --ignored --nocapture`

    #[test]
    #[ignore = "red/binario: genera webm reales con ffmpeg; ejecutar con --ignored"]
    fn webm_real_ffmpeg() {
        let root = std::env::temp_dir().join(format!("musica-webm-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let audio = root.join("a.webm");
        let video = root.join("v.webm");
        let gen = |out: &std::path::Path, args: &[&str]| -> bool {
            crate::tools::command("ffmpeg")
                .args(args)
                .arg("-y")
                .arg(out)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        assert!(
            gen(
                &audio,
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=2",
                    "-c:a",
                    "libopus",
                ],
            ),
            "ffmpeg debe generar el webm de audio"
        );
        assert!(gen(&video, &["-f", "lavfi", "-i", "color=c=red:s=320x240:d=1", "-c:v", "libvpx"]));
        let ia = sniff_webm_file(&audio, READ_LIMIT).expect("audio real debe entrar");
        assert_eq!(ia.duration_secs, Some(2));
        assert!(sniff_webm_file(&video, READ_LIMIT).is_none(), "vídeo real no entra");
        // Cortado DENTRO de Tracks: EBML truncado donde importa → no audio.
        let bytes = std::fs::read(&audio).unwrap();
        let tid = [0x16u8, 0x54, 0xAE, 0x6B];
        let pos = bytes.windows(4).position(|w| w == tid).expect("Tracks presente");
        std::fs::write(root.join("cortado.webm"), &bytes[..pos + 2]).unwrap();
        assert!(sniff_webm_file(&root.join("cortado.webm"), READ_LIMIT).is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
