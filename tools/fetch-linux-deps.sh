#!/usr/bin/env bash
# Descarga las herramientas incluidas de Linux a src-tauri/binaries/ (ignorada
# por git). URL fijas + sha256; no instala nada en el sistema.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/src-tauri/binaries"
TRIPLE="x86_64-unknown-linux-gnu"
mkdir -p "$DEST"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

verificar() {
  local esperado="$1" fichero="$2"
  local real
  real="$(sha256sum "$fichero" | cut -d' ' -f1)"
  if [ "$real" != "$esperado" ]; then
    echo "ERROR sha256 en $(basename "$fichero"): $real != $esperado" >&2
    exit 1
  fi
}

# yt-dlp independiente oficial (sin Python). Unlicense.
curl -fsSL -o "$DEST/yt-dlp-$TRIPLE" \
  https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/yt-dlp_linux
verificar 58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a "$DEST/yt-dlp-$TRIPLE"
chmod 755 "$DEST/yt-dlp-$TRIPLE"

# Deno (MIT). Runtime JS para yt-dlp.
curl -fsSL -o "$TMP/deno.zip" \
  https://github.com/denoland/deno/releases/download/v2.9.8/deno-x86_64-unknown-linux-gnu.zip
verificar e890975bc2549807bbcc0b584ffafda72cc7c93349dd99c80483dd784f6c4609 "$TMP/deno.zip"
python3 -m zipfile -e "$TMP/deno.zip" "$TMP/deno"
install -m 755 "$TMP/deno/deno" "$DEST/deno-$TRIPLE"

# FFmpeg LGPL (BtbN, autobuild fijado). Necesario para mp3/portada.
curl -fsSL -o "$TMP/ffmpeg.tar.xz" \
  https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-22-13-06/ffmpeg-n9.0.2-33-g4cf298e66a-linux64-lgpl-9.0.tar.xz
verificar 9cecf478d8a0ac5fbb41cc8d6c0ef9d318c1950f546670673c7f63fd11dae23b "$TMP/ffmpeg.tar.xz"
tar -xJf "$TMP/ffmpeg.tar.xz" -C "$TMP"
install -m 755 "$(find "$TMP" -path '*/bin/ffmpeg' -type f | head -1)" "$DEST/ffmpeg-$TRIPLE"

ls -lh "$DEST"/{yt-dlp,deno,ffmpeg}-$TRIPLE
