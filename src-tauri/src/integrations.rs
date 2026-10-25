//! Integración SO: MPRIS (Linux) + SMTC (Windows) vía souvlaki.
//! Decisión: souvlaki porque expone una sola API para playerctl/widgets y teclas multimedia.
//! Wayland/X11 agnóstico: MPRIS va por D-Bus, no captura teclas de X11.
//! Los controles hablan SIEMPRE con el PlaybackRouter, nunca con un motor
//! concreto: así las teclas multimedia funcionan igual con libmpv y librespot.

use crate::AppState;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

pub type SharedControls = Arc<Mutex<Option<MediaControls>>>;

/// Estado de pausa actual del router (motor agnóstico).
fn router_paused(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .and_then(|s| s.router.lock().ok().map(|r| r.paused_state()))
        .unwrap_or(true)
}

fn apply_event(app: &AppHandle, ev: MediaControlEvent) {
    // Usa los comandos internos (los mismos que la UI) para no duplicar lógica.
    let state: tauri::State<AppState> = app.state();
    match ev {
        MediaControlEvent::Play => {
            ipc_bridge_play(app);
        }
        MediaControlEvent::Pause => {
            ipc_bridge_pause(app);
        }
        MediaControlEvent::Toggle => {
            if let Ok(mut r) = state.router.lock() {
                r.toggle().ok();
            }
            app.emit("player://state", if router_paused(app) { "paused" } else { "playing" }).ok();
            sync_playback(app);
        }
        MediaControlEvent::Next => crate::cmd_next_impl(app),
        MediaControlEvent::Previous => crate::cmd_prev_impl(app),
        // souvlaki 0.8: Seek(dirección) y SeekBy(dirección, duración).
        MediaControlEvent::Seek(dir) => {
            let pos = state
                .router
                .lock()
                .map(|r| r.position_state() as f64 / 1000.0)
                .unwrap_or(0.0);
            let delta = match dir {
                souvlaki::SeekDirection::Forward => 5.0,
                souvlaki::SeekDirection::Backward => -5.0,
            };
            if let Ok(mut r) = state.router.lock() {
                r.seek_secs((pos + delta).max(0.0)).ok();
            }
        }
        MediaControlEvent::SeekBy(dir, d) => {
            let pos = state
                .router
                .lock()
                .map(|r| r.position_state() as f64 / 1000.0)
                .unwrap_or(0.0);
            let secs = d.as_secs() as f64;
            let delta = match dir {
                souvlaki::SeekDirection::Forward => secs,
                souvlaki::SeekDirection::Backward => -secs,
            };
            if let Ok(mut r) = state.router.lock() {
                r.seek_secs((pos + delta).max(0.0)).ok();
            }
        }
        MediaControlEvent::SetVolume(v) => {
            if let Ok(mut r) = state.router.lock() {
                r.set_volume((v * 100.0) as u8).ok();
            }
            // Avisa a la UI para que refresque el deslizador (el poll no cubre volumen).
            app.emit("player://state", "volume").ok();
        }
        _ => {}
    }
}

/// Play vía comando interno (mismo camino que la UI).
fn ipc_bridge_play(app: &AppHandle) {
    if let Ok(mut r) = app.state::<AppState>().router.lock() {
        r.play().ok();
    }
    app.emit("player://state", "playing").ok();
    sync_playback(app);
}

/// Pause vía comando interno (mismo camino que la UI).
fn ipc_bridge_pause(app: &AppHandle) {
    if let Ok(mut r) = app.state::<AppState>().router.lock() {
        r.pause().ok();
    }
    app.emit("player://state", "paused").ok();
    sync_playback(app);
}

/// HWND de la ventana principal (solo Windows; souvlaki/SMTC lo exige).
// Decisión: resolverlo aquí porque `MediaControls::new` hace expect (panic) si falta.
#[cfg(windows)]
fn platform_hwnd(app: &AppHandle) -> Result<*mut std::ffi::c_void, String> {
    let win = app
        .get_webview_window("main")
        .ok_or("ventana main no encontrada")?;
    let hwnd = win.hwnd().map_err(|e| format!("hwnd: {e}"))?;
    Ok(hwnd.0 as *mut std::ffi::c_void)
}

#[cfg(not(windows))]
fn platform_hwnd(_app: &AppHandle) -> Result<*mut std::ffi::c_void, String> {
    Ok(std::ptr::null_mut()) // MPRIS va por D-Bus y no necesita HWND
}

/// Crea los controles del SO y los guarda en el State compartido.
pub fn init(app: &AppHandle, shared: SharedControls) -> Result<(), String> {
    let raw = platform_hwnd(app)?;
    let mut controls = MediaControls::new(PlatformConfig {
        dbus_name: "musica",
        display_name: "Música",
        hwnd: if raw.is_null() { None } else { Some(raw) },
    })
    .map_err(|e| format!("media controls: {e:?}"))?;

    let handle = app.clone();
    controls
        .attach(move |ev| apply_event(&handle, ev))
        .map_err(|e| format!("attach: {e:?}"))?;

    *shared.lock().map_err(|_| "controls lock".to_string())? = Some(controls);
    Ok(())
}

/// Actualiza título/artista/álbum/duración en el widget del escritorio.
pub fn set_track(app: &AppHandle, title: &str, artist: &str, album: &str, duration_secs: i64) {
    if let Some(shared) = app.try_state::<SharedControls>() {
        if let Ok(mut g) = shared.lock() {
            if let Some(c) = g.as_mut() {
                c.set_metadata(MediaMetadata {
                    title: Some(title),
                    artist: Some(artist),
                    album: Some(album),
                    duration: Some(std::time::Duration::from_secs(duration_secs.max(0) as u64)),
                    ..Default::default()
                })
                .ok();
            }
        }
    }
}

/// Sincroniza playing/paused con playerctl y el panel de Windows leyendo el router.
pub fn sync_playback(app: &AppHandle) {
    sync_playback_state(app, router_paused(app));
}

/// Igual, pero con el estado explícito (lo usan los comandos IPC recién ejecutados).
pub fn sync_playback_state(app: &AppHandle, paused: bool) {
    if let Some(shared) = app.try_state::<SharedControls>() {
        if let Ok(mut g) = shared.lock() {
            if let Some(c) = g.as_mut() {
                c.set_playback(if paused {
                    MediaPlayback::Paused { progress: None }
                } else {
                    MediaPlayback::Playing { progress: None }
                })
                .ok();
            }
        }
    }
}
