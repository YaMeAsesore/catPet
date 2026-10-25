//! Playlists de música local: cada una vive en su propia carpeta en disco.
//!
//! Estructura elegida (documentada para el usuario):
//! ```text
//! <datos-app>/playlists/
//!   Rock de noche/
//!     playlist.m3u8      ← manifiesto con REFERENCIAS (rutas absolutas)
//!   Para correr/
//!     playlist.m3u8
//! ```
//! Formato del manifiesto (m3u8 extendido, estándar y editable a mano):
//! ```text
//! #EXTM3U
//! #EXTINF:214,Artista - Título
//! /ruta/absoluta/cancion.mp3
//! ```
//! Decisión: referencias en vez de copias (duplicar canciones desperdicia disco)
//! y en vez de symlinks (se rompen en Windows/FAT y al mover carpetas). Las rutas
//! relativas se resuelven contra la carpeta de la playlist (portables en bloque).
//! Los nombres de carpeta se sanean (sin `/<>:"\|?*`, máx 64) y se deduplican
//! con " (2)". Escritura atómica (tmp + rename) para no corromper al cortar.
//! La cola y el auto-avance no cambian: una playlist se carga a la cola y suena.

use crate::db::Track;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistInfo {
    pub name: String,
    pub count: usize,
}

pub fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("playlists")
}

fn manifest_path(root: &Path, name: &str) -> PathBuf {
    root.join(name).join("playlist.m3u8")
}

/// Sanea un nombre para usarlo como carpeta. Puro y testeable.
pub fn sanitize(name: &str) -> Option<String> {
    let mut out: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if out.is_empty() || out == "." || out == ".." {
        return None;
    }
    if out.chars().count() > 64 {
        out = out.chars().take(64).collect();
        out = out.trim_end().to_string();
    }
    Some(out)
}

/// Nombre libre (deduplica con " (2)", " (3)"…). Puro y testeable.
pub fn free_name(existing: &[String], want: &str) -> String {
    if !existing.iter().any(|e| e == want) {
        return want.to_string();
    }
    let mut n = 2u32;
    loop {
        let cand = format!("{want} ({n})");
        if !existing.iter().any(|e| e == &cand) {
            return cand;
        }
        n += 1;
    }
}

fn list_names(root: &Path) -> Vec<String> {
    let mut names = vec![];
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(n) = e.file_name().to_str() {
                    names.push(n.to_string());
                }
            }
        }
    }
    names.sort();
    names
}

pub fn list(root: &Path) -> Vec<PlaylistInfo> {
    list_names(root)
        .into_iter()
        .map(|name| {
            let count = read_tracks(root, &name).len();
            PlaylistInfo { name, count }
        })
        .collect()
}

pub fn create(root: &Path, name: &str) -> Result<String, String> {
    let clean = sanitize(name).ok_or("nombre de playlist vacío o inválido")?;
    let final_name = free_name(&list_names(root), &clean);
    let dir = root.join(&final_name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("no se pudo crear: {e}"))?;
    write_manifest(&manifest_path(root, &final_name), &[])?;
    Ok(final_name)
}

pub fn rename(root: &Path, old: &str, new: &str) -> Result<String, String> {
    let clean = sanitize(new).ok_or("nombre de playlist vacío o inválido")?;
    if !root.join(old).is_dir() {
        return Err("la playlist no existe".into());
    }
    if old == clean {
        return Ok(old.to_string());
    }
    let final_name = free_name(&list_names(root), &clean);
    std::fs::rename(root.join(old), root.join(&final_name))
        .map_err(|e| format!("no se pudo renombrar: {e}"))?;
    Ok(final_name)
}

pub fn delete(root: &Path, name: &str) -> Result<(), String> {
    let dir = root.join(name);
    if !dir.is_dir() {
        return Err("la playlist no existe".into());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("no se pudo eliminar: {e}"))?;
    Ok(())
}

/// Añade un track (sin duplicar por path). Devuelve true si se añadió.
/// Rechaza URLs de stream (googlevideo): caducan y están atadas a la IP; en
/// la playlist solo entran referencias estables (`ytmusic://`, locales, http
/// de radios). Si alguna vez llega una, se avisa en vez de corromper la lista.
pub fn add_track(root: &Path, name: &str, track: &Track) -> Result<bool, String> {
    if track.path.contains("googlevideo.com") {
        return Err("URL de stream caducada: agrega la pista desde su lista original".into());
    }
    let mut tracks = read_tracks(root, name);
    if tracks.iter().any(|t| t.path == track.path) {
        return Ok(false);
    }
    tracks.push(track.clone());
    write_manifest(&manifest_path(root, name), &tracks)?;
    Ok(true)
}

/// Quita por path. Devuelve true si existía.
pub fn remove_track(root: &Path, name: &str, path: &str) -> Result<bool, String> {
    if !root.join(name).is_dir() {
        return Err("la playlist no existe".into());
    }
    let before = read_tracks(root, name);
    let after: Vec<Track> = before.into_iter().filter(|t| t.path != path).collect();
    let removed = after.len() != read_tracks(root, name).len();
    write_manifest(&manifest_path(root, name), &after)?;
    Ok(removed)
}

/// Añade todos los audios de una carpeta (recursivo) con sus tags reales.
/// Devuelve cuántos se añadieron (sin duplicar por path).
pub fn playlist_add_folder(
    root: &Path,
    name: &str,
    folder: &Path,
) -> Result<usize, String> {
    if !root.join(name).is_dir() {
        return Err("la playlist no existe".into());
    }
    if !folder.is_dir() {
        return Err("la carpeta no existe".into());
    }
    let mut tracks = read_tracks(root, name);
    let mut added = 0;
    let mut paths: Vec<PathBuf> = walkdir::WalkDir::new(folder)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| p.is_file() && crate::library::is_supported(p))
        .collect();
    paths.sort();
    for p in paths {
        let t = crate::library::read_tags(&p);
        if tracks.iter().any(|x| x.path == t.path) {
            continue;
        }
        tracks.push(t);
        added += 1;
    }
    write_manifest(&manifest_path(root, name), &tracks)?;
    Ok(added)
}

/// Resultado de guardar una lista completa (flujo CSV de S4).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SaveOutcome {
    /// Guardada (o reemplazada) en `name`.
    Saved { name: String },
    /// Ya existía una lista con el mismo nombre y la MISMA huella: no se
    /// escribió nada; el llamante decide si la reemplaza.
    Duplicate { name: String },
}

/// Huella del contenido de una lista (md5 de título|artista|álbum|duración, en
/// orden). No incluye el id del vídeo elegido: el mismo CSV importado dos veces
/// produce la misma huella. Pura y testeable.
pub fn content_fingerprint(tracks: &[Track]) -> String {
    let base: String = tracks
        .iter()
        .map(|t| format!("{}|{}|{}|{}\n", t.title, t.artist, t.album, t.duration_secs))
        .collect();
    format!("{:x}", md5::compute(base))
}

/// Guarda TODA la lista desde el importador CSV reutilizando la carpeta y el
/// manifiesto de siempre (`playlists/<nombre>/playlist.m3u8`).
/// - nombre nuevo: crea.
/// - nombre existente + misma huella: `Duplicate` SIN escribir, salvo
///   `replace=true`, que reescribe esa misma carpeta (no nace una segunda).
/// - nombre existente + otra huella: deduplica como `create` (" (2)") salvo
///   `replace=true`.
pub fn save_tracks(root: &Path, name: &str, tracks: &[Track], replace: bool) -> Result<SaveOutcome, String> {
    if tracks.iter().any(|t| t.path.contains("googlevideo.com")) {
        return Err("URL de stream caducada: guarda la lista desde su origen".into());
    }
    let clean = sanitize(name).ok_or("nombre de playlist vacío o inválido")?;
    if root.join(&clean).is_dir() {
        let same = content_fingerprint(&read_tracks(root, &clean)) == content_fingerprint(tracks);
        if same && !replace {
            return Ok(SaveOutcome::Duplicate { name: clean });
        }
        if !same && !replace {
            let libre = free_name(&list_names(root), &clean);
            write_manifest(&manifest_path(root, &libre), tracks)?;
            return Ok(SaveOutcome::Saved { name: libre });
        }
    }
    write_manifest(&manifest_path(root, &clean), tracks)?;
    Ok(SaveOutcome::Saved { name: clean })
}

/// Reemplaza una entrada por su path conservando la posición (S4: fijar otra
/// versión de una pista `ytmusic://`). Devuelve true si existía.
pub fn replace_track(root: &Path, name: &str, old_path: &str, new: &Track) -> Result<bool, String> {
    if !root.join(name).is_dir() {
        return Err("la playlist no existe".into());
    }
    let mut tracks = read_tracks(root, name);
    let mut hit = false;
    for t in tracks.iter_mut() {
        if t.path == old_path {
            *t = new.clone();
            hit = true;
            break;
        }
    }
    if hit {
        write_manifest(&manifest_path(root, name), &tracks)?;
    }
    Ok(hit)
}

pub fn read_tracks(root: &Path, name: &str) -> Vec<Track> {
    let path = manifest_path(root, name);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    parse_m3u8(&text, path.parent())
}

/// Parsea un m3u8 extendido. Puro y testeable (sin disco).
/// Líneas `#EXTART:`/`#EXTALB:` tras un path restauran carátula/álbum
/// (otros reproductores las ignoran como comentarios: compatible).
pub fn parse_m3u8(text: &str, base: Option<&Path>) -> Vec<Track> {
    let mut out: Vec<Track> = vec![];
    let mut pending: Option<(i64, String, String)> = None; // dur, artist, title
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let (dur_s, display) = rest.split_once(',').unwrap_or(("0", rest));
            let dur = dur_s.trim().parse().unwrap_or(0);
            let (artist, title) = display.split_once(" - ").unwrap_or(("", display));
            pending = Some((dur, artist.trim().to_string(), title.trim().to_string()));
        } else if let Some(art) = line.strip_prefix("#EXTART:") {
            if let Some(last) = out.last_mut() {
                last.art_url = art.trim().to_string();
            }
        } else if let Some(alb) = line.strip_prefix("#EXTALB:") {
            if let Some(last) = out.last_mut() {
                last.album = alb.trim().to_string();
            }
        } else if line.starts_with('#') {
            continue;
        } else {
            let p = Path::new(line);
            // URIs estables (ytmusic://, http(s), subsonic://…): tal cual, sin
            // unir a la carpeta (antes `ytmusic://id` se corrompía a
            // `<playlist>/ytmusic://id` y la pista ya no sonaba).
            let abs = if p.is_absolute() || line.contains("://") {
                line.to_string()
            } else if let Some(b) = base {
                b.join(p).to_string_lossy().into_owned()
            } else {
                line.to_string()
            };
            let (dur, artist, title) = pending.take().unwrap_or((0, String::new(), String::new()));
            let title = if title.is_empty() {
                Path::new(&abs)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Sin título")
                    .to_string()
            } else {
                title
            };
            out.push(Track {
                id: 0,
                path: abs,
                title,
                artist,
                album: String::new(),
                genre: String::new(),
                year: 0,
                track_no: 0,
                duration_secs: dur,
                mtime: 0,
                art_url: String::new(),
            });
        }
    }
    let _ = pending;
    out
}

/// Renderiza el manifiesto (puro y testeable; el formato está documentado en
/// la cabecera del módulo). S4 lo reutiliza para las listas del CSV: cada
/// entrada lleva EXTINF (duración + "artista - título") y, cuando existen,
/// #EXTART (portada) y #EXTALB (álbum) como comentarios compatibles (los
/// lectores ajenos los saltan). Los metadatos bastan para re-buscar la versión
/// de una pista `ytmusic://` sin depender del CSV original.
pub fn render_m3u8(tracks: &[Track]) -> String {
    let mut text = String::from("#EXTM3U\n");
    for t in tracks {
        let display = if t.artist.is_empty() {
            t.title.clone()
        } else {
            format!("{} - {}", t.artist, t.title)
        };
        text.push_str(&format!("#EXTINF:{},{display}\n{}\n", t.duration_secs.max(0), t.path));
        if !t.art_url.is_empty() {
            text.push_str(&format!("#EXTART:{}\n", t.art_url));
        }
        if !t.album.is_empty() {
            text.push_str(&format!("#EXTALB:{}\n", t.album));
        }
    }
    text
}

fn write_manifest(path: &Path, tracks: &[Track]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("no se pudo crear: {e}"))?;
    }
    // Atómico: tmp + rename (un corte de luz no deja medio manifiesto).
    let tmp = path.with_extension("m3u8.tmp");
    std::fs::write(&tmp, render_m3u8(tracks)).map_err(|e| format!("no se pudo guardar: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("no se pudo guardar: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Directorio temporal ÚNICO por test (compartir por PID mete carreras:
    /// un test borra el padre mientras otro trabaja dentro).
    fn dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let d = std::env::temp_dir().join(format!(
            "musica-pl-{}-{:?}-{}-{:x}",
            std::process::id(),
            std::thread::current().id(),
            tag,
            nanos & 0xfffff
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mk(path: &str) -> Track {
        Track {
            id: 7, path: path.into(), title: "T".into(), artist: "A".into(),
            album: "B".into(), genre: "".into(), year: 0, track_no: 0,
            duration_secs: 200, mtime: 0, art_url: "".into(),
        }
    }

    #[test]
    fn sanea_nombres_peligrosos() {
        assert_eq!(sanitize("  Rock de noche  ").as_deref(), Some("Rock de noche"));
        assert_eq!(sanitize("a/b\\c:d*e?f\"g<h>i|j").as_deref(), Some("a_b_c_d_e_f_g_h_i_j"));
        assert_eq!(sanitize("   ").as_deref(), None);
        assert_eq!(sanitize("..").as_deref(), None);
        assert_eq!(sanitize("").as_deref(), None);
        assert!(sanitize(&"x".repeat(100)).unwrap().chars().count() <= 64);
    }

    #[test]
    fn deduplica_con_sufijo() {
        let ex = vec!["Rock".to_string(), "Rock (2)".to_string()];
        assert_eq!(free_name(&ex, "Rock"), "Rock (3)");
        assert_eq!(free_name(&ex, "Pop"), "Pop");
    }

    #[test]
    fn crud_completo_con_manifiesto() {
        let base = dir("crud");
        let root = base.join("crud");
        let name = create(&root, "Mi lista").unwrap();
        assert_eq!(name, "Mi lista");
        // Duplicado de nombre se deduplica.
        assert_eq!(create(&root, "Mi lista").unwrap(), "Mi lista (2)");
        // Añadir sin duplicar por path.
        assert!(add_track(&root, &name, &mk("/m/a.mp3")).unwrap());
        assert!(!add_track(&root, &name, &mk("/m/a.mp3")).unwrap());
        assert!(add_track(&root, &name, &mk("/m/b.mp3")).unwrap());
        // Lectura roundtrip con metadatos.
        let tracks = read_tracks(&root, &name);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title, "T");
        assert_eq!(tracks[0].duration_secs, 200);
        assert_eq!(list(&root).iter().find(|p| p.name == name).unwrap().count, 2);
        // Quitar.
        assert!(remove_track(&root, &name, "/m/a.mp3").unwrap());
        assert!(!remove_track(&root, &name, "/m/a.mp3").unwrap());
        assert_eq!(read_tracks(&root, &name).len(), 1);
        // Renombrar.
        let n2 = rename(&root, &name, "Otra").unwrap();
        assert_eq!(n2, "Otra");
        assert_eq!(read_tracks(&root, &n2).len(), 1);
        // Eliminar borra la carpeta.
        delete(&root, &n2).unwrap();
        delete(&root, "Mi lista (2)").unwrap();
        assert!(list(&root).is_empty());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn agregar_carpeta_usa_tags_y_no_duplica() {
        let base = dir("carpeta");
        let music = base.join("music");
        std::fs::create_dir_all(&music).unwrap();
        // Archivos dummy: el walker filtra por extensión (los tags fallan y
        // caen al nombre del archivo, igual que la biblioteca).
        std::fs::write(music.join("uno.mp3"), b"ID3").unwrap();
        std::fs::write(music.join("dos.flac"), b"fLaC").unwrap();
        std::fs::write(music.join("nota.txt"), b"hola").unwrap();
        let root = base.join("pls");
        let name = create(&root, "Carpeta").unwrap();
        assert_eq!(playlist_add_folder(&root, &name, &music).unwrap(), 2);
        // Segunda vez no duplica.
        assert_eq!(playlist_add_folder(&root, &name, &music).unwrap(), 0);
        let tracks = read_tracks(&root, &name);
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().all(|t| t.path.ends_with(".mp3") || t.path.ends_with(".flac")));
        // Carpeta inexistente o playlist inexistente → error claro.
        assert!(playlist_add_folder(&root, &name, &base.join("nope")).is_err());
        assert!(playlist_add_folder(&root, "fantasma", &music).is_err());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn parsea_extm3u_y_rutas_relativas() {
        let text = "#EXTM3U\n#EXTINF:214,Queen - Bohemian\n/m/a.mp3\n#EXTINF:0,\nrel/b.mp3\n";
        let base = Path::new("/pl/rock");
        let ts = parse_m3u8(text, Some(base));
        assert_eq!(ts.len(), 2);
        assert_eq!((ts[0].artist.as_str(), ts[0].title.as_str()), ("Queen", "Bohemian"));
        assert_eq!(ts[0].duration_secs, 214);
        assert_eq!(ts[1].path, "/pl/rock/rel/b.mp3");
        // Sin EXTINF usa el nombre del archivo.
        let ts2 = parse_m3u8("/x/song.flac\n", None);
        assert_eq!(ts2[0].title, "song");
    }

    #[test]
    fn youtube_roundtrip_con_portada_y_rechaza_stream() {
        let base = dir("yt");
        let root = base.join("pls");
        let name = create(&root, "Mix").unwrap();
        // Pista de YouTube con referencia estable + metadatos.
        let yt = Track {
            id: 0, path: "ytmusic://abc123".into(), title: "Tema".into(),
            artist: "Grupo".into(), album: "YouTube".into(), genre: "youtube".into(),
            year: 0, track_no: 0, duration_secs: 214, mtime: 0,
            art_url: "https://i.ytimg.com/vi/abc123/hqdefault.jpg".into(),
        };
        assert!(add_track(&root, &name, &yt).unwrap());
        let back = read_tracks(&root, &name);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].path, "ytmusic://abc123");
        assert_eq!(back[0].art_url, "https://i.ytimg.com/vi/abc123/hqdefault.jpg");
        assert_eq!(back[0].album, "YouTube");
        // Una URL de stream caducada nunca entra a la playlist.
        let mut bad = yt.clone();
        bad.path = "https://rr1---sn.googlevideo.com/videoplayback?expire=1&ip=1.2.3.4".into();
        assert!(add_track(&root, &name, &bad).is_err());
        assert_eq!(read_tracks(&root, &name).len(), 1);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn borrar_playlist_no_toca_los_archivos() {
        let base = dir("borrar");
        let music = base.join("musica");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::write(music.join("a.mp3"), b"ID3").unwrap();
        std::fs::write(music.join("b.mp3"), b"ID3").unwrap();
        let root = base.join("pls");
        let name = create(&root, "Lista").unwrap();
        let otro = create(&root, "Otra").unwrap();
        add_track(&root, &name, &mk(&music.join("a.mp3").to_string_lossy())).unwrap();
        add_track(&root, &name, &mk(&music.join("b.mp3").to_string_lossy())).unwrap();
        delete(&root, &name).unwrap();
        // Se va SOLO la carpeta de la playlist: la música y las demás listas siguen.
        assert!(!root.join(&name).exists());
        assert!(root.join(&otro).is_dir());
        assert!(music.join("a.mp3").is_file());
        assert!(music.join("b.mp3").is_file());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn render_m3u8_ytmusic_comentarios_y_roundtrip() {
        let yt = Track {
            id: 0, path: "ytmusic://vid01".into(), title: "Tema".into(),
            artist: "Grupo".into(), album: "Disco original".into(), genre: "youtube".into(),
            year: 0, track_no: 0, duration_secs: 214, mtime: 0,
            art_url: "https://i.ytimg.com/vi/vid01/hqdefault.jpg".into(),
        };
        let text = render_m3u8(std::slice::from_ref(&yt));
        assert!(text.starts_with("#EXTM3U\n"));
        assert!(text.contains("ytmusic://vid01"), "referencia estable");
        assert!(text.contains("#EXTINF:214,Grupo - Tema\n"), "título/artista/duración");
        assert!(text.contains("#EXTALB:Disco original\n"), "álbum original");
        assert!(text.contains("#EXTART:https://i.ytimg.com/vi/vid01/hqdefault.jpg\n"), "portada");
        // Roundtrip: «Buscar otra versión» relee estos metadatos del m3u8.
        let back = parse_m3u8(&text, None);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].path, "ytmusic://vid01");
        assert_eq!((back[0].artist.as_str(), back[0].title.as_str()), ("Grupo", "Tema"));
        assert_eq!((back[0].album.as_str(), back[0].duration_secs), ("Disco original", 214));
    }

    #[test]
    fn huella_dedupe_segun_contenido() {
        // Metadatos distintos por pista: la huella va por contenido, no por path.
        let mut a = mk("/m/a.mp3");
        a.title = "Uno".into();
        let mut b = mk("/m/b.mp3");
        b.title = "Dos".into();
        let mut otra_version = a.clone();
        otra_version.path = "ytmusic://vid99".into(); // cambia el vídeo, no el contenido
        assert_eq!(content_fingerprint(&[a.clone(), b.clone()]), content_fingerprint(&[otra_version, b.clone()]));
        let mut distinto = b.clone();
        distinto.title = "Otra".into();
        assert_ne!(content_fingerprint(&[a.clone(), b.clone()]), content_fingerprint(&[a.clone(), distinto]));
        assert_ne!(content_fingerprint(&[a.clone(), b.clone()]), content_fingerprint(&[b, a]));
    }

    #[test]
    fn save_tracks_detecta_duplicado_y_reemplaza() {
        let base = dir("save");
        let root = base.join("pls");
        let t1 = mk("/m/a.mp3");
        let t2 = mk("/m/b.mp3");
        let r = save_tracks(&root, "Lista", &[t1.clone(), t2.clone()], false).unwrap();
        assert!(matches!(r, SaveOutcome::Saved { .. }));
        // Mismo nombre + mismo contenido: no se crea una segunda lista…
        let r = save_tracks(&root, "Lista", &[t1.clone(), t2.clone()], false).unwrap();
        match r {
            SaveOutcome::Duplicate { name } => assert_eq!(name, "Lista"),
            other => panic!("se esperaba Duplicate: {other:?}"),
        }
        assert_eq!(list(&root).len(), 1);
        // …salvo que el usuario decida reemplazar (reescribe la misma carpeta).
        let r = save_tracks(&root, "Lista", &[t1.clone(), t2.clone()], true).unwrap();
        assert!(matches!(r, SaveOutcome::Saved { .. }));
        assert_eq!(list(&root).len(), 1);
        // Cambiar solo el vídeo elegido no cambia la huella (mismo contenido).
        let mut t2v = t2.clone();
        t2v.path = "ytmusic://vid77".into();
        let r = save_tracks(&root, "Lista", &[t1.clone(), t2v], false).unwrap();
        assert!(matches!(r, SaveOutcome::Duplicate { .. }));
        // Nombre existente con OTRO contenido: deduplica como create (" (2)").
        let mut otro = t2.clone();
        otro.title = "Otra".into();
        let r = save_tracks(&root, "Lista", &[otro], false).unwrap();
        match r {
            SaveOutcome::Saved { name } => assert_eq!(name, "Lista (2)"),
            other => panic!("se esperaba Saved: {other:?}"),
        }
        assert_eq!(list(&root).len(), 2);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn replace_track_cambia_el_video_y_conserva_metadatos_y_posicion() {
        let base = dir("replace");
        let root = base.join("pls");
        let a = mk("/m/a.mp3");
        let b = mk("/m/b.mp3");
        save_tracks(&root, "L", &[a.clone(), b.clone()], false).unwrap();
        let mut nuevo = b.clone(); // mismos metadatos: solo cambia la referencia
        nuevo.path = "ytmusic://vid99".into();
        nuevo.art_url = "https://i.ytimg.com/vi/vid99/hqdefault.jpg".into();
        assert!(replace_track(&root, "L", "/m/b.mp3", &nuevo).unwrap());
        let back = read_tracks(&root, "L");
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].path, "/m/a.mp3", "el orden no cambia");
        assert_eq!(back[1].path, "ytmusic://vid99");
        assert_eq!(back[1].art_url, "https://i.ytimg.com/vi/vid99/hqdefault.jpg");
        assert_eq!((back[1].title.as_str(), back[1].duration_secs), ("T", 200));
        assert!(!replace_track(&root, "L", "/m/nope.mp3", &nuevo).unwrap());
        std::fs::remove_dir_all(base).ok();
    }
}
