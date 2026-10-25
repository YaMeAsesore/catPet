// Build de Tauri v2: genera capabilities y esquema IPC.
// Decisión: build.rs mínimo porque toda la lógica está en src/.
fn main() {
    tauri_build::build()
}
