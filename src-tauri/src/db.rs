//! Biblioteca SQLite + FTS5. Una conexión por comando (simple y Send).
//! Decisión: WAL + FTS5 porque soporta 50k filas con búsqueda <20ms sin servidor.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
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
    /// URL de portada remota (solo en memoria; la BD local no la persiste en fase 1).
    #[serde(default)]
    pub art_url: String,
}

/// Fuente remota propia: servidor Subsonic/Jellyfin, radio o feed de podcast.
// Decisión: credenciales en SQLite local (keyring del SO en fase posterior).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: i64,
    /// "subsonic" | "jellyfin" | "radio" | "podcast".
    pub kind: String,
    pub name: String,
    pub base_url: String,
    /// Subsonic: usuario · Jellyfin: user_id (tras login) · radio/podcast: "".
    pub user: String,
    /// Subsonic: salt · Jellyfin: api token · radio/podcast: "".
    pub secret: String,
    /// Subsonic: token · resto: "".
    pub extra: String,
}

/// Abre (o crea) la BD y aplica el esquema.
pub fn open(db_path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS tracks(
           id INTEGER PRIMARY KEY,
           path TEXT UNIQUE NOT NULL,
           title TEXT NOT NULL,
           artist TEXT NOT NULL DEFAULT '',
           album TEXT NOT NULL DEFAULT '',
           genre TEXT NOT NULL DEFAULT '',
           year INTEGER NOT NULL DEFAULT 0,
           track_no INTEGER NOT NULL DEFAULT 0,
           duration_secs INTEGER NOT NULL DEFAULT 0,
           mtime INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist);
         CREATE INDEX IF NOT EXISTS idx_tracks_album ON tracks(album);
         -- Orden por defecto de la biblioteca (artist, album, track_no): sin
         -- este índice SQLite ordenaba TODAS las filas en cada página. Con
         -- 50 000 pistas el scroll/arranque se sentía lento.
         CREATE INDEX IF NOT EXISTS idx_tracks_order ON tracks(artist, album, track_no);
         CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
           title, artist, album, path, content='tracks', content_rowid='id'
         );
         CREATE TRIGGER IF NOT EXISTS trg_tracks_ai AFTER INSERT ON tracks BEGIN
           INSERT INTO tracks_fts(rowid,title,artist,album,path)
           VALUES(new.id,new.title,new.artist,new.album,new.path);
         END;
         CREATE TRIGGER IF NOT EXISTS trg_tracks_ad AFTER DELETE ON tracks BEGIN
           INSERT INTO tracks_fts(tracks_fts,rowid,title,artist,album,path)
           VALUES('delete',old.id,old.title,old.artist,old.album,old.path);
         END;
         CREATE TRIGGER IF NOT EXISTS trg_tracks_au AFTER UPDATE ON tracks BEGIN
           INSERT INTO tracks_fts(tracks_fts,rowid,title,artist,album,path)
           VALUES('delete',old.id,old.title,old.artist,old.album,old.path);
           INSERT INTO tracks_fts(rowid,title,artist,album,path)
           VALUES(new.id,new.title,new.artist,new.album,new.path);
         END;
         CREATE TABLE IF NOT EXISTS sources(
           id INTEGER PRIMARY KEY,
           kind TEXT NOT NULL,
           name TEXT NOT NULL,
           base_url TEXT NOT NULL,
           user TEXT NOT NULL DEFAULT '',
           secret TEXT NOT NULL DEFAULT '',
           extra TEXT NOT NULL DEFAULT ''
         );
         -- Caché de metadatos de playlists de YouTube (stale-while-revalidate).
         -- Solo título/ids/artista/duración, nunca cookies ni URLs de audio.
         CREATE TABLE IF NOT EXISTS yt_playlist_cache(
           key TEXT PRIMARY KEY,
           updated_at INTEGER NOT NULL,
           payload TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_yt_cache_updated
           ON yt_playlist_cache(updated_at);
         -- Estado de sesión (cola, volumen…): se restaura al arrancar.
         CREATE TABLE IF NOT EXISTS app_state(
           key TEXT PRIMARY KEY,
           value TEXT NOT NULL
         );",
    )?;
    Ok(conn)
}

fn row_to_track(row: &rusqlite::Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        album: row.get(4)?,
        genre: row.get(5)?,
        year: row.get(6)?,
        track_no: row.get(7)?,
        duration_secs: row.get(8)?,
        mtime: row.get(9)?,
        art_url: String::new(), // la BD local no guarda portadas en fase 1
    })
}

/// Inserta o actualiza por path (clave incremental: mtime).
pub fn upsert(conn: &Connection, t: &Track) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO tracks(path,title,artist,album,genre,year,track_no,duration_secs,mtime)
         VALUES(?,?,?,?,?,?,?,?,?)
         ON CONFLICT(path) DO UPDATE SET
           title=excluded.title, artist=excluded.artist, album=excluded.album,
           genre=excluded.genre, year=excluded.year, track_no=excluded.track_no,
           duration_secs=excluded.duration_secs, mtime=excluded.mtime",
        params![
            t.path, t.title, t.artist, t.album, t.genre,
            t.year, t.track_no, t.duration_secs, t.mtime
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// mtime guardado para decidir si re-leer tags (indexado incremental).
pub fn mtime_of(conn: &Connection, path: &str) -> Option<i64> {
    conn.query_row("SELECT mtime FROM tracks WHERE path=?1", [path], |r| {
        r.get(0)
    })
    .ok()
}

/// Búsqueda full-text; query vacía = últimos por título (paginado).
pub fn search(conn: &Connection, q: &str, limit: i64, offset: i64) -> rusqlite::Result<Vec<Track>> {
    let sql_all = "SELECT id,path,title,artist,album,genre,year,track_no,duration_secs,mtime
                   FROM tracks ORDER BY artist,album,track_no LIMIT ?1 OFFSET ?2";
    if q.trim().is_empty() {
        let mut st = conn.prepare(sql_all)?;
        return st.query_map(params![limit, offset], row_to_track)?.collect();
    }
    // FTS5: escapa comillas dobles envolviendo la consulta.
    let q_fts = format!("\"{}\"*", q.replace('"', "\"\""));
    let mut st = conn.prepare(
        "SELECT t.id,t.path,t.title,t.artist,t.album,t.genre,t.year,t.track_no,t.duration_secs,t.mtime
         FROM tracks_fts f JOIN tracks t ON t.id=f.rowid
         WHERE tracks_fts MATCH ?1 LIMIT ?2 OFFSET ?3",
    )?;
    let out: rusqlite::Result<Vec<Track>> =
        st.query_map(params![q_fts, limit, offset], row_to_track)?.collect();
    out
}

pub fn count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .unwrap_or(0)
}

/// Lee un valor de estado de sesión (cola, volumen…).
pub fn state_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM app_state WHERE key=?1",
        [key],
        |r| r.get(0),
    )
    .ok()
}

/// Guarda un valor de estado de sesión (sobrescribe).
pub fn state_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_state(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Lee una playlist cacheada. Devuelve (epoch de actualización, JSON).
pub fn yt_cache_get(conn: &Connection, key: &str) -> rusqlite::Result<Option<(i64, String)>> {
    let mut st = conn.prepare(
        "SELECT updated_at, payload FROM yt_playlist_cache WHERE key=?1",
    )?;
    let mut rows = st.query_map([key], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.next().transpose()
}

/// Guarda/actualiza una playlist y poda entradas viejas o sobrantes.
/// Tope: 100 playlists y 7 días (los metadatos son pequeños, pero una lista de
/// 10 000 pistas puede ocupar ~2 MB; no conviene crecer sin límite).
pub fn yt_cache_put(
    conn: &Connection,
    key: &str,
    updated_at: i64,
    payload: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO yt_playlist_cache(key,updated_at,payload) VALUES(?1,?2,?3)
         ON CONFLICT(key) DO UPDATE SET updated_at=excluded.updated_at,
                                       payload=excluded.payload",
        params![key, updated_at, payload],
    )?;
    let cutoff = updated_at - 7 * 24 * 60 * 60;
    conn.execute(
        "DELETE FROM yt_playlist_cache WHERE updated_at < ?1",
        [cutoff],
    )?;
    conn.execute(
        "DELETE FROM yt_playlist_cache WHERE key NOT IN (
           SELECT key FROM yt_playlist_cache ORDER BY updated_at DESC LIMIT 100
         )",
        [],
    )?;
    Ok(())
}

/// Alta de fuente (el llamante ya validó la conexión).
pub fn add_source(conn: &Connection, s: &Source) -> rusqlite::Result<Source> {
    conn.execute(
        "INSERT INTO sources(kind,name,base_url,user,secret,extra) VALUES(?,?,?,?,?,?)",
        params![s.kind, s.name, s.base_url, s.user, s.secret, s.extra],
    )?;
    let mut out = s.clone();
    out.id = conn.last_insert_rowid();
    Ok(out)
}

fn row_to_source(row: &rusqlite::Row) -> rusqlite::Result<Source> {
    Ok(Source {
        id: row.get(0)?,
        kind: row.get(1)?,
        name: row.get(2)?,
        base_url: row.get(3)?,
        user: row.get(4)?,
        secret: row.get(5)?,
        extra: row.get(6)?,
    })
}

pub fn list_sources(conn: &Connection) -> rusqlite::Result<Vec<Source>> {
    let mut st = conn.prepare(
        "SELECT id,kind,name,base_url,user,secret,extra FROM sources ORDER BY id",
    )?;
    let out: rusqlite::Result<Vec<Source>> =
        st.query_map([], row_to_source)?.collect();
    out
}

pub fn get_source(conn: &Connection, id: i64) -> rusqlite::Result<Option<Source>> {
    let mut st = conn.prepare(
        "SELECT id,kind,name,base_url,user,secret,extra FROM sources WHERE id=?1",
    )?;
    let mut rows = st.query_map([id], row_to_source)?;
    Ok(rows.next().transpose()?)
}

pub fn remove_source(conn: &Connection, id: i64) -> rusqlite::Result<usize> {
    Ok(conn.execute("DELETE FROM sources WHERE id=?1", [id])?)
}

/// Renombra una fuente: SOLO `name` (ni URL ni credenciales).
pub fn rename_source(conn: &Connection, id: i64, name: &str) -> rusqlite::Result<usize> {
    Ok(conn.execute("UPDATE sources SET name=?2 WHERE id=?1", params![id, name])?)
}

/// ¿Pertenece el path `p` a la carpeta `folder`? (prefijo con separador).
// Decisión: función aparte porque así se puede testear sin montar una BD.
pub fn path_in_folder(p: &str, folder: &str) -> bool {
    if p == folder {
        return true;
    }
    let sep = std::path::MAIN_SEPARATOR;
    if folder.ends_with(sep) {
        p.starts_with(folder)
    } else {
        p.starts_with(&format!("{folder}{sep}"))
    }
}

/// Borra los paths que ya no existen en disco, SOLO dentro de la carpeta escaneada.
// Decisión: ámbito por prefijo porque la BD es multi-carpeta (escanear B no debe purgar A).
pub fn delete_missing(
    conn: &Connection,
    existing: &std::collections::HashSet<String>,
    folder: &str,
) -> rusqlite::Result<usize> {
    let mut st = conn.prepare("SELECT path FROM tracks")?;
    let all: Vec<String> = st.query_map([], |r| r.get(0))?.filter_map(|r| r.ok()).collect();
    drop(st);
    let mut n = 0;
    for p in all {
        // Solo purga lo que pertenece a esta carpeta y falta en disco.
        if path_in_folder(&p, folder) && !existing.contains(&p) {
            conn.execute("DELETE FROM tracks WHERE path=?1", [&p])?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn scope_por_prefijo() {
        assert!(path_in_folder("/m/a.mp3", "/m"));
        assert!(path_in_folder("/m/sub/b.flac", "/m"));
        assert!(path_in_folder("/m/a.mp3", "/m/"));
        assert!(!path_in_folder("/music2/x.mp3", "/music"));
        assert!(!path_in_folder("/other/y.mp3", "/m"));
        assert!(!path_in_folder("/m2", "/m"));
    }

    #[test]
    fn purga_solo_su_carpeta() {
        let dir = std::env::temp_dir().join(format!("musica-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = open(&dir.join("t.db")).unwrap();
        for p in ["/a/1.mp3", "/a/2.mp3", "/b/3.mp3"] {
            upsert(&conn, &Track {
                id: 0, path: p.into(), title: "t".into(), artist: "".into(),
                album: "".into(), genre: "".into(), year: 0, track_no: 0,
                duration_secs: 0, mtime: 1, art_url: "".into(),
            }).unwrap();
        }
        // Re-escaneo de /a donde solo sigue existiendo /a/1.mp3: /b/3.mp3 debe sobrevivir.
        let seen: HashSet<String> = ["/a/1.mp3".to_string()].into_iter().collect();
        assert_eq!(delete_missing(&conn, &seen, "/a").unwrap(), 1);
        assert_eq!(count(&conn), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn estado_de_sesion_ida_y_vuelta() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE app_state(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        ).unwrap();
        assert!(state_get(&conn, "queue").is_none());
        state_set(&conn, "queue", "{\"items\":[]}").unwrap();
        assert_eq!(state_get(&conn, "queue").as_deref(), Some("{\"items\":[]}"));
        state_set(&conn, "queue", "{\"items\":[1]}").unwrap();
        assert_eq!(state_get(&conn, "queue").as_deref(), Some("{\"items\":[1]}"));
    }

    #[test]
    fn cache_de_playlist_guarda_y_reemplaza() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE yt_playlist_cache(
               key TEXT PRIMARY KEY, updated_at INTEGER NOT NULL, payload TEXT NOT NULL);",
        ).unwrap();
        let now = 1_700_000_000i64;
        assert!(yt_cache_get(&conn, "yt:PL1").unwrap().is_none());
        yt_cache_put(&conn, "yt:PL1", now, "{\"a\":1}").unwrap();
        assert_eq!(yt_cache_get(&conn, "yt:PL1").unwrap().unwrap().0, now);
        // Misma clave: reemplaza, no duplica.
        yt_cache_put(&conn, "yt:PL1", now + 1, "{\"a\":2}").unwrap();
        let (at, body) = yt_cache_get(&conn, "yt:PL1").unwrap().unwrap();
        assert_eq!((at, body.as_str()), (now + 1, "{\"a\":2}"));
        // Una entrada más vieja que 7 días se poda al guardar otra.
        yt_cache_put(&conn, "yt:vieja", now - 8 * 24 * 60 * 60, "{}").unwrap();
        yt_cache_put(&conn, "yt:nueva", now, "{}").unwrap();
        assert!(yt_cache_get(&conn, "yt:vieja").unwrap().is_none());
    }

    #[test]
    fn renombrar_fuente_solo_cambia_el_nombre() {
        let dir = std::env::temp_dir().join(format!(
            "musica-ren-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = open(&dir.join("t.db")).unwrap();
        let a = add_source(&conn, &Source {
            id: 0, kind: "radio".into(), name: "Antes".into(),
            base_url: "https://radio.example/stream".into(),
            user: "u".into(), secret: "s".into(), extra: "e".into(),
        }).unwrap();
        assert_eq!(rename_source(&conn, a.id, "Después").unwrap(), 1);
        let got = get_source(&conn, a.id).unwrap().unwrap();
        assert_eq!(got.name, "Después");
        // URL y credenciales intactas.
        assert_eq!(got.kind, "radio");
        assert_eq!(got.base_url, "https://radio.example/stream");
        assert_eq!((got.user.as_str(), got.secret.as_str(), got.extra.as_str()), ("u", "s", "e"));
        // Id inexistente: no toca nada (el comando lo convierte en error claro).
        assert_eq!(rename_source(&conn, 99999, "X").unwrap(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn borrar_fuente_no_toca_los_archivos() {
        let dir = std::env::temp_dir().join(format!(
            "musica-del-{}-{:?}", std::process::id(), std::thread::current().id()));
        let music = dir.join("Musica");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::write(music.join("una.mp3"), b"ID3").unwrap();
        let conn = open(&dir.join("t.db")).unwrap();
        // Fuente apuntando a esa carpeta: borrarla quita el registro, no el disco.
        let s = add_source(&conn, &Source {
            id: 0, kind: "subsonic".into(), name: "Servidor".into(),
            base_url: music.to_string_lossy().to_string(),
            user: String::new(), secret: String::new(), extra: String::new(),
        }).unwrap();
        assert_eq!(remove_source(&conn, s.id).unwrap(), 1);
        assert!(get_source(&conn, s.id).unwrap().is_none());
        assert!(music.join("una.mp3").is_file());
        std::fs::remove_dir_all(&dir).ok();
    }
}
