//! L3b: inicio de sesión de Google en una ventana propia con datos aislados.
//!
//! Flujo: se abre una `WebviewWindow` con `data_directory` propio (no comparte
//! almacén con la ventana principal) en music.youtube.com; al detectar la
//! navegación (`on_navigation`/`on_page_load`) arranca un vigilante que espera
//! a que existan las cookies SAPISID y LOGIN_INFO (login completado), las
//! escribe en un candidato 0600 (solo youtube.com/google.com), lo valida con la
//! misma petición barata de L2 y, solo si valida, sustituye el jar real y quita
//! el marcador; entonces cierra la ventana. Cerrar la ventana nunca cierra la
//! sesión ni borra los datos aislados (eso queda a decisión del usuario).
//! Sin valores de cookies en logs: solo contadores/estados.

use crate::ytmusic;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// Etiqueta de la ventana de login (sin permisos IPC: no está en capabilities).
pub const LABEL: &str = "yt-login";
const START_URL: &str = "https://accounts.google.com/AccountChooser?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";
/// Sondeo del vigilante: local y barato (lectura de cookies del webview).
const POLL: Duration = Duration::from_secs(2);
/// Tope de espera del login (2FA incluido); la ventana se queda abierta.
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);
static WATCHING: AtomicBool = AtomicBool::new(false);

/// Evento `youtube://login` (la UI lo pinta).
#[derive(Clone, serde::Serialize)]
pub struct LoginEvent {
    /// "waiting" | "connected" | "failed" | "cancelled" | "timeout"
    pub state: &'static str,
    pub message: Option<String>,
    pub account: ytmusic::YtAccount,
}

fn emit(app: &AppHandle, state: &'static str, message: Option<String>) {
    let ev = LoginEvent { state, message, account: ytmusic::account() };
    app.emit("youtube://login", ev).ok();
}

/// ¿Host de YouTube? (puro; `notyoutube.com` no cuenta).
fn is_youtube_host(url: &tauri::Url) -> bool {
    url.host_str()
        .map(|h| h == "youtube.com" || h.ends_with(".youtube.com"))
        .unwrap_or(false)
}

/// Abre (o reutiliza) la ventana de login y arma el vigilante cuando la
/// ventana llega a YouTube (nunca antes: evita adoptar cookies viejas del perfil).
pub fn start(app: &AppHandle) -> Result<(), String> {
    // Datos aislados junto al jar: la ventana guarda su propia sesión de Google.
    let dir = ytmusic::login_data_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("no se pudo preparar la ventana de login: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok();
    }
    if let Some(win) = app.get_webview_window(LABEL) {
        win.set_focus().ok();
        // Reutilizada: si ya está en YouTube, rearma el vigilante (pudo haber
        // terminado). Si sigue en Google, se armará al navegar.
        if win.url().map(|u| is_youtube_host(&u)).unwrap_or(false) {
            watch_later(app.clone());
        }
    } else {
        let url: tauri::Url = START_URL
            .parse()
            .map_err(|e| format!("URL de login inválida: {e}"))?;
        let nav_app = app.clone();
        let load_app = app.clone();
        WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(url))
            .title("Iniciar sesión con Google")
            .inner_size(1100.0, 800.0)
            .center()
            .data_directory(dir)
            .on_navigation(move |u| {
                // Detecta páginas de YouTube (incluida music.youtube.com) para
                // empezar a esperar el login; nunca bloquea la navegación.
                if is_youtube_host(u) {
                    watch_later(nav_app.clone());
                }
                true
            })
            .on_page_load(move |_w, payload| {
                if matches!(payload.event(), PageLoadEvent::Finished) && is_youtube_host(payload.url()) {
                    watch_later(load_app.clone());
                }
            })
            .build()
            .map_err(|e| format!("no se pudo abrir la ventana de inicio de sesión: {e}"))?;
    }
    emit(
        app,
        "waiting",
        Some("Inicia sesión con tu cuenta de Google; la ventana se cerrará sola al terminar.".into()),
    );
    Ok(())
}

/// Arranca el vigilante una sola vez (idempotente; se puede rearmar).
fn watch_later(app: AppHandle) {
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        watch(app).await;
        WATCHING.store(false, Ordering::SeqCst);
    });
}

/// Espera a SAPISID + LOGIN_INFO, valida el candidato y adopta el jar si vale.
async fn watch(app: AppHandle) {
    let deadline = Instant::now() + MAX_WAIT;
    loop {
        tokio::time::sleep(POLL).await;
        if Instant::now() >= deadline {
            emit(
                &app,
                "timeout",
                Some("No se detectó el inicio de sesión; vuelve a abrir la ventana si quieres reintentarlo.".into()),
            );
            return;
        }
        let Some(win) = app.get_webview_window(LABEL) else {
            emit(&app, "cancelled", None);
            return;
        };
        let cookies = match win.cookies() {
            Ok(c) => c,
            Err(_) => continue, // ventana a medio cerrar: se reintenta
        };
        // P4-19: un subframe de cuentas (accounts.youtube.com) puede armar el
        // vigilante; solo se adopta si la ventana principal ya está en YouTube.
        if !win.url().map(|u| is_youtube_host(&u)).unwrap_or(false) {
            continue;
        }
        if !ytmusic::has_login_cookies(&cookies) {
            continue; // aún no hay login completado
        }
        // Login completado: candidato 0600 (solo youtube.com/google.com) y LL.
        let netscape = ytmusic::to_netscape(&cookies);
        if let Err(e) = ytmusic::write_login_candidate(&netscape) {
            emit(&app, "failed", Some(e));
            return;
        }
        match ytmusic::validate_login_candidate().await {
            ytmusic::CheckOutcome::Valid => {
                if let Err(e) = ytmusic::adopt_login_candidate() {
                    ytmusic::discard_login_candidate();
                    emit(&app, "failed", Some(e));
                    return;
                }
                win.close().ok();
                emit(&app, "connected", None);
                return;
            }
            ytmusic::CheckOutcome::Expired => {
                ytmusic::discard_login_candidate();
                emit(
                    &app,
                    "failed",
                    Some("Las cookies de esta ventana no abren tu sesión: inténtalo de nuevo o usa «Importar cookies.txt».".into()),
                );
                return;
            }
            ytmusic::CheckOutcome::Unverified => {
                ytmusic::discard_login_candidate();
                emit(
                    &app,
                    "failed",
                    Some("No se pudo validar la sesión ahora (¿límite temporal de YouTube?): inténtalo de nuevo en unos minutos.".into()),
                );
                return;
            }
        }
    }
}

/// L4: borra SOLO el directorio de datos de la ventana de login (a petición
/// explícita del usuario desde "Cerrar sesión": la app ofrece y nunca lo toca
/// por su cuenta). Cierra la ventana si sigue abierta; no toca el jar ni el
/// método de sesión.
pub async fn clear_data(app: &AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(LABEL) {
        win.close().ok();
        // Deja morir el webview: WebKit puede reescribir ficheros al cerrarse.
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let dir = ytmusic::login_data_dir();
    let mut ultimo_error = None;
    for _ in 0..2 {
        if !dir.exists() {
            return Ok(());
        }
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => return Ok(()),
            Err(e) => {
                ultimo_error = Some(e);
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
        }
    }
    if dir.exists() {
        return Err(format!(
            "no se pudieron borrar los datos de la ventana: {}",
            ultimo_error.map(|e| e.to_string()).unwrap_or_default()
        ));
    }
    Ok(())
}
