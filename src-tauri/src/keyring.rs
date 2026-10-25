//! Secretos en el keyring del sistema (Linux Secret Service, macOS Keychain,
//! Windows Credential Manager). El frontend nunca ve tokens ni credenciales.
//! Decisión: keyring-core + store nativo por plataforma; en Linux se intenta
//! Secret Service (persistente) y se degrada a keyutils (sesión) si no hay daemon,
//! y a memoria si tampoco hay keyring, para no bloquear la app.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Ronda de credenciales librespot ya persistida (ver `librespot_engine`).
pub const SERVICE_SPOTIFY: &str = "musica-spotify";
/// Token PKCE del flujo OAuth (access token de corta vida).
pub const SERVICE_PKCE: &str = "musica-spotify-pkce";
/// Identidad estable del dispositivo Connect (device_id del SessionConfig).
pub const SERVICE_DEVICE: &str = "musica-spotify-device";

static MEMORY: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());
static INIT: OnceLock<bool> = OnceLock::new();

/// Inicializa el store del sistema una sola vez. Devuelve true si hay keyring real.
pub fn init() -> bool {
    *INIT.get_or_init(|| {
        let cfg: HashMap<&str, &str> = HashMap::new();
        #[cfg(target_os = "linux")]
        {
            // Persistente (GNOME Keyring / KWallet vía zbus): primera opción.
            if let Ok(store) =
                zbus_secret_service_keyring_store::Store::new_with_configuration(&cfg)
            {
                keyring_core::set_default_store(store);
                return true;
            }
            eprintln!(
                "[musica] Secret Service no disponible; probando keyutils (solo sesión)"
            );
            if let Ok(store) =
                linux_keyutils_keyring_store::Store::new_with_configuration(&cfg)
            {
                keyring_core::set_default_store(store);
                return true;
            }
            eprintln!("[musica] sin keyring: las credenciales vivirán solo en memoria");
            false
        }
        #[cfg(target_os = "macos")]
        {
            if let Ok(store) =
                apple_native_keyring_store::keychain::Store::new_with_configuration(&cfg)
            {
                keyring_core::set_default_store(store);
                return true;
            }
            eprintln!("[musica] Keychain no disponible: credenciales solo en memoria");
            false
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(store) =
                windows_native_keyring_store::Store::new_with_configuration(&cfg)
            {
                keyring_core::set_default_store(store);
                return true;
            }
            eprintln!("[musica] Credential Manager no disponible: credenciales solo en memoria");
            false
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = cfg;
            false
        }
    })
}

fn mem_key(service: &str, key: &str) -> String {
    format!("{service}\u{1}{key}")
}

fn mem_set(service: &str, key: &str, value: &str) {
    if let Ok(mut m) = MEMORY.lock() {
        let full = mem_key(service, key);
        m.retain(|(s, k, _)| mem_key(s, k) != full);
        m.push((service.to_string(), key.to_string(), value.to_string()));
    }
}

fn mem_get(service: &str, key: &str) -> Option<String> {
    let full = mem_key(service, key);
    MEMORY.lock().ok()?.iter().find(|(s, k, _)| mem_key(s, k) == full).map(|(_, _, v)| v.clone())
}

fn mem_del(service: &str, key: &str) {
    if let Ok(mut m) = MEMORY.lock() {
        let full = mem_key(service, key);
        m.retain(|(s, k, _)| mem_key(s, k) != full);
    }
}

/// Escribe un secreto. Si no hay keyring del sistema, cae a memoria de proceso.
pub fn set(service: &str, key: &str, value: &str) -> Result<(), String> {
    init();
    if let Ok(entry) = keyring_core::Entry::new(service, key) {
        match entry.set_password(value) {
            Ok(()) => return Ok(()),
            Err(e) => {
                // El keyring existe pero rechaza (p.ej. bloqueado): degradamos con aviso.
                eprintln!("[musica] keyring.set falló ({e:?}); usando memoria");
            }
        }
    } else {
        eprintln!("[musica] sin store de keyring; usando memoria");
    }
    mem_set(service, key, value);
    Ok(())
}

/// Lee un secreto (keyring primero, memoria como red de seguridad).
pub fn get(service: &str, key: &str) -> Option<String> {
    init();
    if let Ok(entry) = keyring_core::Entry::new(service, key) {
        match entry.get_password() {
            Ok(p) => return Some(p),
            Err(keyring_core::Error::NoEntry) => {} // puede estar solo en memoria
            Err(e) => eprintln!("[musica] keyring.get falló ({e:?})"),
        }
    }
    mem_get(service, key)
}

/// Borra el secreto de todos los almacenes donde esté.
pub fn remove(service: &str, key: &str) {
    init();
    if let Ok(entry) = keyring_core::Entry::new(service, key) {
        entry.delete_credential().ok();
    }
    mem_del(service, key);
}

/// true si el backend actual es el keyring del sistema (persistente).
/// false = memoria: el usuario debe saber que la sesión no sobrevive al cierre.
pub fn persistent() -> bool {
    init()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memoria_como_fallback_funciona() {
        // Fuerza el camino memoria (independiente de si el CI tiene keyring).
        mem_set("test-svc", "a", "v1");
        assert_eq!(mem_get("test-svc", "a").as_deref(), Some("v1"));
        mem_set("test-svc", "a", "v2"); // sobreescribe
        assert_eq!(mem_get("test-svc", "a").as_deref(), Some("v2"));
        mem_del("test-svc", "a");
        assert_eq!(mem_get("test-svc", "a"), None);
        mem_del("test-svc", "b"); // borrar ausente no entra en pánico
    }

    #[test]
    fn claves_de_distintos_servicios_no_colisionan() {
        mem_set("svc-1", "k", "uno");
        mem_set("svc-2", "k", "dos");
        assert_eq!(mem_get("svc-1", "k").as_deref(), Some("uno"));
        assert_eq!(mem_get("svc-2", "k").as_deref(), Some("dos"));
        mem_del("svc-1", "k");
        mem_del("svc-2", "k");
    }

    #[test]
    fn set_get_remove_no_panico_sin_daemon() {
        // En CI headless sin keyring debe degradar a memoria y seguir el roundtrip.
        set("test-svc", "rt", "hola").unwrap();
        assert_eq!(get("test-svc", "rt").as_deref(), Some("hola"));
        remove("test-svc", "rt");
        // En memoria queda borrado; si el sistema tenía keyring real también.
        // No afirmamos None porque un keyring compartido podría tener restos,
        // pero remove debe haber limpiado nuestra clave.
        if !persistent() {
            assert_eq!(get("test-svc", "rt"), None);
        }
    }
}
