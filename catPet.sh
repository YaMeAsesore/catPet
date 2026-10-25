#!/usr/bin/env bash
# Lanzador universal de la AppImage de catPet.
#
# - Fuerza XWayland por defecto: WebKitGTK no crea una ventana fiable en
#   Wayland con esta app. Se puede sobrescribir con GDK_BACKEND=wayland.
# - Si el sistema tiene FUSE 2, monta la AppImage directamente. Si no, usa el
#   modo "extract-and-run" (no necesita FUSE ni instalar nada).
# - `./catPet.sh soft` fuerza render por software para GPUs/WebKit problemáticos.
set -euo pipefail

MODE=""
if [ "${1:-}" = "soft" ] || [ "${1:-}" = "--soft" ]; then
  MODE="soft"
  shift
fi

# Wayland + WebKitGTK: la ventana puede quedarse invisible. XWayland funciona.
export GDK_BACKEND="${GDK_BACKEND:-x11}"

if [ "$MODE" = "soft" ]; then
  export LIBGL_ALWAYS_SOFTWARE=1
  export WEBKIT_DISABLE_COMPOSITING_MODE=1
  export WEBKIT_DISABLE_DMABUF_RENDERER=1
fi

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP="$DIR/catPet_0.1.0_amd64.AppImage"
if [ ! -x "$APP" ] && [ -x "$DIR/release/catPet_0.1.0_amd64.AppImage" ]; then
  APP="$DIR/release/catPet_0.1.0_amd64.AppImage"
fi

if [ ! -x "$APP" ]; then
  echo "No se encontró la AppImage: $APP" >&2
  echo "Descárgala desde:" >&2
  echo "  https://github.com/YaMeAsesore/catPet/releases/latest" >&2
  echo "y déjala junto a catPet.sh, o en la carpeta release/." >&2
  exit 1
fi

tiene_fuse=0
if [ -e /dev/fuse ] && { command -v fusermount >/dev/null 2>&1 || command -v fusermount3 >/dev/null 2>&1; }; then
  if (ldconfig -p 2>/dev/null || true) | grep -q 'libfuse\.so\.2'; then
    tiene_fuse=1
  fi
fi

if [ "$tiene_fuse" = "1" ]; then
  exec "$APP" "$@"
else
  # Extraer en el disco del usuario (no en /tmp, que puede ser tmpfs pequeño).
  if [ -z "${TMPDIR:-}" ]; then
    TMPDIR="${XDG_CACHE_HOME:-$HOME/.cache}/catPet-appimage"
    mkdir -p "$TMPDIR"
    export TMPDIR
  fi
  exec env APPIMAGE_EXTRACT_AND_RUN=1 "$APP" "$@"
fi
