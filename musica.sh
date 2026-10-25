#!/usr/bin/env bash
# Lanzador de Música (Tauri + Svelte + Rust).
# Decide: un .sh evita recordar las variables de node/GPU de este equipo.
#
# Uso:
#   ./musica.sh            → desarrollo (vite + tauri dev, recarga en caliente)
#   ./musica.sh build      → binario release + bundles (.deb/.AppImage)
#   ./musica.sh run        → ejecuta el binario debug ya compilado (usa dist/)
#   ./musica.sh soft       → fuerza render por software (ventana negra/GBM)
#   ./musica.sh check      → verifica toolchain sin arrancar la app
#
#   MUSICA_NO_AUTO_INSTALL=1 → no instala node_modules (mensaje antiguo y sale)
#
# La primera ejecución comprueba node/npm/cargo (si faltan, NO los instala:
# explica cómo conseguirlos) y si falta node_modules instala las dependencias
# de npm con npm ci (o npm install si no hay package-lock.json).
#
# IMPORTANTE: ejecutar SIN sudo. La app vive en tu usuario (datos en
# ~/.local/share) y rustup/cargo se configuran por usuario, no por root.

set -uo pipefail

# ---- Log persistente: si el terminal se cierra, aquí queda el rastro ----
LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/musica"
mkdir -p "$LOG_DIR" 2>/dev/null || LOG_DIR="/tmp"
LOG="$LOG_DIR/launcher.log"
exec > >(tee -a "$LOG") 2>&1
echo "──── $(date '+%F %T') ── música.sh $$ ────"

# ---- Entorno node local de este equipo (archnode) ----
ARCHNODE_LIB="${ARCHNODE_LIB:-$HOME/.local/archnode/usr/lib}"
ARCHNODE_BIN="${ARCHNODE_BIN:-$HOME/.local/archnode/usr/bin}"
if [ -d "$ARCHNODE_LIB" ]; then
  export LD_LIBRARY_PATH="$ARCHNODE_LIB:${LD_LIBRARY_PATH:-}"
fi
if [ -d "$ARCHNODE_BIN" ]; then
  export PATH="$ARCHNODE_BIN:$PATH"
fi

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$PROJECT_DIR" || exit 1

MODE="${1:-dev}"

log()  { printf '\033[1;32m[música]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[música]\033[0m %s\n' "$*" >&2; }
die()  {
  printf '\033[1;31m[música]\033[0m %s\n' "$*" >&2
  printf '\033[1;31m[música]\033[0m log completo: %s\n' "$LOG" >&2
  # Si hay terminal interactiva, no la cierres en silencio (doble clic).
  if [ -t 0 ]; then
    echo
    read -r -p "Presiona Enter para salir…" _
  fi
  exit 1
}

# ---- Guardia anti-sudo: root no tiene toolchain y la app no lo necesita ----
if [ "$(id -u)" = "0" ]; then
  if [ -n "${SUDO_USER:-}" ]; then
    die "NO uses sudo: rustup/cargo se configuran por usuario ('$SUDO_USER'),
       y como root no hay toolchain por defecto. Ejecuta:
       $PROJECT_DIR/musica.sh $MODE"
  else
    die "NO ejecutes como root: ejecuta ./musica.sh con tu usuario normal."
  fi
fi

# Wayland + WebKitGTK: la ventana puede no aparecer; XWayland es fiable.
export GDK_BACKEND="${GDK_BACKEND:-x11}"

# Render por software si la GPU/composición falla (ventana negra, errores GBM).
if [ "${SOFT:-0}" = "1" ] || [ "$MODE" = "soft" ]; then
  export LIBGL_ALWAYS_SOFTWARE=1
  export WEBKIT_DISABLE_COMPOSITING_MODE=1
  export WEBKIT_DISABLE_DMABUF_RENDERER=1
  log "render por software activado (LIBGL/WEBKIT)"
fi

check_toolchain() {
  command -v node  >/dev/null || die "node no encontrado (¿archnode activo?).
       Node se instala con el gestor de paquetes de tu distro (pacman -S nodejs,
       apt install nodejs) o con nvm (https://github.com/nvm-sh/nvm).
       Este script NO lo instala por ti."
  command -v npm   >/dev/null || die "npm no encontrado (¿node de una instalación
       manual a medias?). npm viene con Node: gestor de paquetes de la distro
       (pacman -S npm, apt install npm) o nvm (https://github.com/nvm-sh/nvm).
       Este script NO lo instala por ti."
  command -v cargo >/dev/null || die "cargo no encontrado (Rust ≥1.77).
       Instálalo desde https://rustup.rs (curl | sh) o con el gestor de paquetes
       de la distro. Este script NO lo instala por ti."
  # OJO: con sudo, `cargo` existe (proxy rustup) pero FALLA al ejecutarse.
  cargo --version >/dev/null 2>&1 || die "cargo no arranca: ¿ejecutaste con sudo?
       Como root rustup no tiene toolchain por defecto. Quita el sudo; si aun
       así falla, ejecuta: rustup default stable"
  ensure_node_modules
  log "toolchain OK: $(node --version) · $(cargo --version | cut -d' ' -f1-2)"
}

# node_modules: si falta, se instala solo (solo la primera vez). La salida de
# npm va a la terminal Y al log (el exec+tee de arriba duplica todo ahí).
# MUSICA_NO_AUTO_INSTALL=1 lo desactiva y deja el comportamiento antiguo.
ensure_node_modules() {
  if [ -d node_modules ]; then
    return 0
  fi
  if [ "${MUSICA_NO_AUTO_INSTALL:-0}" = "1" ]; then
    die "faltan dependencias: ejecuta 'npm install'
       (MUSICA_NO_AUTO_INSTALL=1 está activo: esta app no las instala sola)"
  fi
  if [ -f package-lock.json ]; then
    log "node_modules no existe: instalando dependencias (solo la primera vez, vía npm ci)…"
    if ! npm ci; then
       die "falló 'npm ci': revisa tu red o tu npm. Log completo: $LOG"
    fi
  else
    log "node_modules no existe: instalando dependencias (solo la primera vez, vía npm install)…"
    if ! npm install; then
      die "falló 'npm install': revisa tu red o tu npm. Log completo: $LOG"
    fi
  fi
  [ -d node_modules ] || die "npm terminó pero node_modules no existe. Log completo: $LOG"
}

# Primera compilación de Rust: avisar (no bloquear) si aún no hay target.
warn_first_build() {
  if [ ! -d src-tauri/target ]; then
    log "primera ejecución: Rust se compila desde cero y puede tardar varios minutos…"
  fi
}

# Puerto del dev server: si quedó un vite zombie de antes, avisa en claro.
check_port() {
  if command -v ss >/dev/null 2>&1; then
    if ss -tln 2>/dev/null | grep -q ':1420 '; then
      die "el puerto 1420 está ocupado (¿vite viejo?). Libéralo:
       pkill -f 'vite' && pkill -f 'target/debug/musica'"
    fi
  fi
}

case "$MODE" in
  check)
    check_toolchain
    log "comprobando backend (cargo check)…"
    cargo check --manifest-path src-tauri/Cargo.toml --message-format short 2>&1 | tail -3
    ;;

  dev|soft)
    check_toolchain
    check_port
    warn_first_build
    log "arrancando modo desarrollo (vite :1420 + tauri)…"
    log "Ctrl+C detiene ambos. La primera compilación puede tardar minutos."
    exec npx tauri dev
    ;;

  build)
    check_toolchain
    log "compilando frontend + backend release…"
    npm run build || die "falló el build del frontend"
    ( cd src-tauri && cargo build --release ) || die "falló el build de Rust"
    log "binario: src-tauri/target/release/musica"
    log "bundles .deb/.AppImage: npx tauri build"
    ;;

  run)
    BIN="src-tauri/target/debug/musica"
    [ -x "$BIN" ] || die "no existe $BIN: usa './musica.sh dev' o './musica.sh build'"
    if [ ! -f dist/index.html ]; then
      warn "sin dist/index.html: el binario debug buscará el dev server de vite"
      warn "→ usa './musica.sh dev' (recomendado en desarrollo)"
    fi
    log "ejecutando $BIN"
    exec "$BIN"
    ;;

  *)
    die "modo desconocido: '$MODE' (usa: dev | build | run | soft | check)"
    ;;
esac
