//! Localización y lanzamiento de binarios externos (yt-dlp, ffmpeg, node...).
//!
//! W3: un único punto de lanzamiento para todo el proyecto. `command()` (std)
//! y `tokio_command()` (async) resuelven el binario con `find_tool` (sidecar
//! junto al ejecutable → directorio de recursos → PATH) y, en Windows, marcan
//! CREATE_NO_WINDOW para que nunca aparezca una consola. `kill_child` corta el
//! árbol de procesos (Windows: taskkill /T, también por este punto).

use std::path::{Path, PathBuf};

/// CREATE_NO_WINDOW (Win32): el proceso hijo no abre ventana de consola.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Busca `name` (y en Windows también `name.exe`) dentro de `dir`. Función
/// pura con directorio parametrizable: es lo que cubren los tests con tmp.
fn find_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    let nombres = [format!("{name}.exe"), name.to_string()];
    #[cfg(not(windows))]
    let nombres = [name.to_string()];
    nombres.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// Resuelve un binario externo: sidecar junto al ejecutable (externalBin en
/// Windows se instala junto al .exe) → directorio de recursos (Tauri `resources/`;
/// en Windows `resource_dir` es el propio directorio del exe) → PATH.
/// En Linux, sin sidecars, devuelve `name` y Command resuelve el PATH: mismo
/// comportamiento de siempre.
pub fn find_tool(name: &str) -> PathBuf {
    // (1) Herramienta puesta a mano por el usuario (gana a la incluida).
    if let Some(dir) = user_tool_dir() {
        if let Some(p) = find_in_dir(&dir, name) {
            return p;
        }
    }
    // (2) Incluida junto al ejecutable o en resources/ del paquete.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for d in [
                dir.to_path_buf(),
                dir.join("bin"),
                dir.join("resources"),
                dir.join("resources").join("bin"),
            ] {
                if let Some(p) = find_in_dir(&d, name) {
                    return p;
                }
            }
            // Linux: recursos en /usr/lib/<producto>/bin (deb y AppImage).
            // Se acepta catPet (nombre actual), catpet (variante) y musica
            // (paquetes anteriores) para no romper instalaciones existentes.
            #[cfg(not(windows))]
            for rel in ["../lib/catPet/bin", "../lib/catpet/bin", "../lib/musica/bin"] {
                if let Some(p) = find_in_dir(&dir.join(rel), name) {
                    return p;
                }
            }
        }
    }
    // (3) PATH.
    PathBuf::from(name)
}

/// (1) Carpeta de usuario para reemplazar herramientas incluidas:
/// Linux `$XDG_DATA_HOME/com.musica.app/bin` (o ~/.local/share/...);
/// Windows `%APPDATA%\com.musica.app\bin`.
#[cfg(not(windows))]
fn user_tool_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))?;
    Some(base.join("com.musica.app").join("bin"))
}

#[cfg(windows)]
fn user_tool_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| Path::new(&p).join("com.musica.app").join("bin"))
}

#[cfg(not(windows))]
fn data_base_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))
}

#[cfg(windows)]
fn data_base_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

/// Crea `com.musica.app/tmp` bajo la base de datos con permisos 0700.
fn ensure_private_dir(base: &Path) -> std::io::Result<PathBuf> {
    let dir = base.join("com.musica.app").join("tmp");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

fn ytdlp_tmp_dir() -> Option<PathBuf> {
    ensure_private_dir(&data_base_dir()?).ok()
}

/// ¿El binario resuelto es de los incluidos en el paquete (no del usuario/PATH)?
fn is_included_path(tool: &Path, exe: &Path) -> bool {
    if !tool.is_absolute() {
        return false;
    }
    let Some(dir) = exe.parent() else { return false };
    let bases = [
        dir.to_path_buf(),
        dir.join("bin"),
        dir.join("resources"),
        dir.join("resources").join("bin"),
        dir.join("../lib/catPet/bin"),
        dir.join("../lib/catpet/bin"),
        dir.join("../lib/musica/bin"),
    ];
    bases.iter().any(|b| tool.starts_with(b))
}

fn needs_ytdlp_tmp(name: &str, tool: &Path, exe: &Path) -> bool {
    name == "yt-dlp" && is_included_path(tool, exe)
}

fn apply_ytdlp_tmp_std(cmd: &mut std::process::Command, name: &str, tool: &Path, exe: Option<&Path>) {
    let Some(exe) = exe else { return };
    if !needs_ytdlp_tmp(name, tool, exe) {
        return;
    }
    let Some(dir) = ytdlp_tmp_dir() else { return };
    #[cfg(windows)]
    {
        cmd.env("TEMP", &dir).env("TMP", &dir);
    }
    #[cfg(not(windows))]
    {
        cmd.env("TMPDIR", &dir);
    }
}

fn apply_ytdlp_tmp_tokio(cmd: &mut tokio::process::Command, name: &str, tool: &Path, exe: Option<&Path>) {
    let Some(exe) = exe else { return };
    if !needs_ytdlp_tmp(name, tool, exe) {
        return;
    }
    let Some(dir) = ytdlp_tmp_dir() else { return };
    #[cfg(windows)]
    {
        cmd.env("TEMP", &dir).env("TMP", &dir);
    }
    #[cfg(not(windows))]
    {
        cmd.env("TMPDIR", &dir);
    }
}

/// Herramientas externas cuyo entorno se limpia dentro de un AppImage.
const EXTERNAL_TOOLS: &[&str] = &["yt-dlp", "node", "deno", "ffmpeg"];

fn is_external(name: &str) -> bool {
    EXTERNAL_TOOLS.contains(&name)
}

/// Solo dentro de un AppImage y solo para herramientas externas: el AppRun
/// apunta PYTHONHOME/PYTHONPATH y LD_LIBRARY_PATH al AppDir; un Python del
/// sistema cargaría el OpenSSL empaquetado y fallaría. El PATH original no es
/// recuperable (AppRun lo reemplaza); por eso importa la carpeta de usuario y
/// los binarios incluidos, no el PATH.
fn clean_external_env(cmd: &mut std::process::Command, name: &str, appdir_present: bool) {
    if !appdir_present || !is_external(name) {
        return;
    }
    cmd.env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .env_remove("LD_LIBRARY_PATH");
}

fn clean_external_env_tokio(cmd: &mut tokio::process::Command, name: &str, appdir_present: bool) {
    if !appdir_present || !is_external(name) {
        return;
    }
    cmd.env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .env_remove("LD_LIBRARY_PATH");
}

fn appdir_present() -> bool {
    std::env::var_os("APPDIR").is_some()
}

/// Si la herramienta vino del paquete (ruta absoluta), añade su carpeta al
/// PATH del hijo. Así `yt-dlp` encuentra `deno`/`ffmpeg` incluidos en el
/// AppImage/deb y no depende de que estén instalados en el sistema.
fn prepend_tool_dir(cmd: &mut std::process::Command, tool: &Path) {
    let Some(dir) = tool.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return;
    };
    let mut dirs = vec![dir.to_path_buf()];
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Ok(joined) = std::env::join_paths(dirs) {
        cmd.env("PATH", joined);
    }
}

fn prepend_tool_dir_tokio(cmd: &mut tokio::process::Command, tool: &Path) {
    let Some(dir) = tool.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return;
    };
    let mut dirs = vec![dir.to_path_buf()];
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Ok(joined) = std::env::join_paths(dirs) {
        cmd.env("PATH", joined);
    }
}

/// Punto único de lanzamiento (std). En Windows, CREATE_NO_WINDOW: sin consolas.
pub fn command(name: &str) -> std::process::Command {
    // En Windows se le añaden creation_flags (por eso mut); en Linux no.
    let tool = find_tool(name);
    let exe = std::env::current_exe().ok();
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = std::process::Command::new(&tool);
    prepend_tool_dir(&mut cmd, &tool);
    clean_external_env(&mut cmd, name, appdir_present());
    apply_ytdlp_tmp_std(&mut cmd, name, &tool, exe.as_deref());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Punto único de lanzamiento (async, tokio). Misma resolución y mismas
/// banderas Windows que `command`.
pub fn tokio_command(name: &str) -> tokio::process::Command {
    // En Windows se le añaden creation_flags (por eso mut); en Linux no.
    let tool = find_tool(name);
    let exe = std::env::current_exe().ok();
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = tokio::process::Command::new(&tool);
    prepend_tool_dir_tokio(&mut cmd, &tool);
    clean_external_env_tokio(&mut cmd, name, appdir_present());
    apply_ytdlp_tmp_tokio(&mut cmd, name, &tool, exe.as_deref());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Mata un hijo y su árbol de procesos. `Child::kill()` en Windows termina
/// solo al proceso directo (yt-dlp) y deja huérfanos a ffmpeg; `taskkill /T`
/// recorre el árbol completo. En Unix es el `kill()` de siempre.
pub fn kill_child(ch: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let _ = command("taskkill")
            .args(["/PID", &ch.id().to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        ch.kill().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// find_in_dir con directorio temporal: sin binario → None; con el
    /// sidecar creado → lo devuelve por el nombre propio de la plataforma.
    #[test]
    fn find_in_dir_localiza_sidecar_en_tmp() {
        let dir = std::env::temp_dir().join(format!("musica-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(find_in_dir(&dir, "yt-dlp"), None);
        let bin = dir.join(if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" });
        std::fs::write(&bin, b"x").unwrap();
        assert_eq!(find_in_dir(&dir, "yt-dlp"), Some(bin.clone()));
        assert_eq!(find_in_dir(&dir, "ffmpeg"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// find_tool sin sidecar (nombre inexistente en exe/recursos) devuelve el
    /// nombre simple: Command resuelve el PATH, idéntico a antes en Linux.
    #[test]
    fn find_tool_sin_sidecar_es_el_nombre() {
        let n = "yt-dlp-no-existe-musica";
        assert_eq!(find_tool(n), PathBuf::from(n));
    }

    /// Solo APPDIR + herramienta externa: se quitan PYTHONHOME/PYTHONPATH y
    /// LD_LIBRARY_PATH (el AppRun los apunta al AppDir). Fuera de eso, nada.
    #[test]
    fn entorno_limpio_solo_bajo_appdir_y_externas() {
        let removed = |cmd: &std::process::Command| -> Vec<String> {
            cmd.get_envs()
                .filter(|(_, v)| v.is_none())
                .map(|(k, _)| k.to_string_lossy().into_owned())
                .collect()
        };

        let mut externa = std::process::Command::new("true");
        clean_external_env(&mut externa, "yt-dlp", true);
        let quitadas = removed(&externa);
        for k in ["PYTHONHOME", "PYTHONPATH", "LD_LIBRARY_PATH"] {
            assert!(quitadas.iter().any(|q| q == k), "falta {k}");
        }

        let mut sin_appdir = std::process::Command::new("true");
        clean_external_env(&mut sin_appdir, "yt-dlp", false);
        assert!(removed(&sin_appdir).is_empty());

        let mut no_externa = std::process::Command::new("true");
        clean_external_env(&mut no_externa, "curl", true);
        assert!(removed(&no_externa).is_empty());
    }

    #[test]
    fn tmpdir_solo_para_ytdlp_incluido() {
        let base = std::env::temp_dir().join(format!("musica-tmp-{}", std::process::id()));
        let exe = base.join("app/usr/bin/musica");
        let incluido = base.join("app/usr/bin/../lib/musica/bin/yt-dlp");
        let usuario = base.join("home/.local/share/com.musica.app/bin/yt-dlp");
        assert!(needs_ytdlp_tmp("yt-dlp", &incluido, &exe));
        assert!(!needs_ytdlp_tmp("yt-dlp", &usuario, &exe));
        assert!(!needs_ytdlp_tmp("deno", &incluido, &exe));
        assert!(!needs_ytdlp_tmp("yt-dlp", Path::new("yt-dlp"), &exe));
        std::fs::remove_dir_all(&base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn tmp_privado_0700() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("musica-tmp-perm-{}", std::process::id()));
        let dir = ensure_private_dir(&base).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(&base).ok();
    }
}
