//! Arquitectura de proveedores modular (solo fuentes legales).
//! Decisión: trait Provider porque añadir Subsonic/Jellyfin/Radio no toca player ni DB.
//! Decisión: una instancia de proveedor por cuenta (`subsonic://{id}/…`) porque cada
//! cuenta tiene sus propias credenciales y el `resolve` debe elegir la correcta.

use crate::remote::{JellyfinApi, SubsonicApi};
use std::path::{Path, PathBuf};

/// Todo proveedor debe resolver un id lógico a un archivo o URL reproducible por mpv.
pub trait Provider: Send + Sync {
    /// Devuelve path local o URL http(s) lista para `loadfile`.
    fn resolve(&self, track_path: &str) -> Option<String>;
    /// Prefijo para saber si un path pertenece a este proveedor.
    fn owns(&self, track_path: &str) -> bool;
}

/// a) Archivos locales: pasa el path tal cual.
pub struct LocalProvider;
impl Provider for LocalProvider {
    fn resolve(&self, track_path: &str) -> Option<String> {
        Some(track_path.to_string())
    }
    fn owns(&self, _p: &str) -> bool {
        true // fallback por defecto
    }
}

/// b) Una cuenta Subsonic/Navidrome: construye `/rest/stream.view` autenticada.
pub struct SubsonicProvider {
    pub source_id: i64,
    pub base_url: String,
    pub user: String,
    pub salt: String,
    pub token: String,
}

impl SubsonicProvider {
    fn api(&self) -> SubsonicApi {
        SubsonicApi {
            source_id: self.source_id,
            base_url: self.base_url.clone(),
            user: self.user.clone(),
            salt: self.salt.clone(),
            token: self.token.clone(),
        }
    }
    fn prefix(&self) -> String {
        format!("subsonic://{}/", self.source_id)
    }
}

impl Provider for SubsonicProvider {
    fn resolve(&self, track_path: &str) -> Option<String> {
        let song_id = track_path.strip_prefix(&self.prefix())?;
        Some(self.api().stream_url(song_id))
    }
    fn owns(&self, p: &str) -> bool {
        p.starts_with(&self.prefix())
    }
}

/// b2) Una cuenta Jellyfin: `/Audio/{id}/universal` con api_key.
pub struct JellyfinProvider {
    pub source_id: i64,
    pub base_url: String,
    pub token: String,
    pub user_id: String,
}

impl JellyfinProvider {
    fn prefix(&self) -> String {
        format!("jellyfin://{}/", self.source_id)
    }
}

impl Provider for JellyfinProvider {
    fn resolve(&self, track_path: &str) -> Option<String> {
        let item_id = track_path.strip_prefix(&self.prefix())?;
        Some(JellyfinApi {
            source_id: self.source_id,
            base_url: self.base_url.clone(),
            token: self.token.clone(),
            user_id: self.user_id.clone(),
        }
        .stream_url(item_id))
    }
    fn owns(&self, p: &str) -> bool {
        p.starts_with(&self.prefix())
    }
}

/// c) Radios y podcasts RSS/ICY: la URL ya es reproducible.
pub struct StreamProvider;
impl Provider for StreamProvider {
    fn resolve(&self, track_path: &str) -> Option<String> {
        if track_path.starts_with("http://") || track_path.starts_with("https://") {
            Some(track_path.to_string())
        } else {
            None
        }
    }
    fn owns(&self, p: &str) -> bool {
        p.starts_with("http")
    }
}

/// d) Registro: las cuentas van primero (específicas), local al final (fallback).
pub struct Registry {
    pub providers: Vec<Box<dyn Provider>>,
}

impl Registry {
    pub fn mvp() -> Self {
        Self { providers: vec![Box::new(StreamProvider), Box::new(LocalProvider)] }
    }

    /// Reconstruye el registro desde la tabla `sources` (tras alta/baja).
    pub fn load(db_path: &Path) -> Self {
        let mut reg = Self::mvp();
        if let Ok(conn) = crate::db::open(db_path) {
            if let Ok(srcs) = crate::db::list_sources(&conn) {
                for s in srcs {
                    match s.kind.as_str() {
                        "subsonic" => reg.providers.insert(
                            0,
                            Box::new(SubsonicProvider {
                                source_id: s.id,
                                base_url: s.base_url,
                                user: s.user,
                                salt: s.secret,
                                token: s.extra,
                            }),
                        ),
                        "jellyfin" => reg.providers.insert(
                            0,
                            Box::new(JellyfinProvider {
                                source_id: s.id,
                                base_url: s.base_url,
                                token: s.secret,
                                user_id: s.user,
                            }),
                        ),
                        _ => {} // radio/podcast usan StreamProvider (URLs directas)
                    }
                }
            }
        }
        reg
    }

    pub fn resolve(&self, track_path: &str) -> String {
        for p in &self.providers {
            if p.owns(track_path) {
                if let Some(url) = p.resolve(track_path) {
                    return url;
                }
            }
        }
        track_path.to_string()
    }

    #[allow(dead_code)]
    pub fn local_db_path(_suggested: &str) -> PathBuf {
        PathBuf::from(_suggested)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resuelve_cuenta_correcta() {
        let mut reg = Registry::mvp();
        reg.providers.insert(
            0,
            Box::new(SubsonicProvider {
                source_id: 7,
                base_url: "https://srv.local".into(),
                user: "u".into(),
                salt: "s".into(),
                token: "t".into(),
            }),
        );
        let url = reg.resolve("subsonic://7/42");
        assert!(url.contains("/rest/stream.view") && url.contains("id=42"));
        // Otra cuenta no debe interceptar este path.
        assert!(!url.contains("jellyfin"));
        // Local sigue pasando tal cual.
        assert_eq!(reg.resolve("/m/a.mp3"), "/m/a.mp3");
    }
}
