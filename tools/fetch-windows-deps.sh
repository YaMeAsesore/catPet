#!/usr/bin/env bash
# Descarga las dependencias de Windows a src-tauri/binaries y resources
# (ignoradas por git). URL fijas + sha256. Uso desde Linux (7z + python3).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/src-tauri/binaries"
RES="$ROOT/src-tauri/resources"
TRIPLE="x86_64-pc-windows-msvc"
mkdir -p "$BIN" "$RES"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

verificar() {
  local esperado="$1" fichero="$2" real
  real="$(sha256sum "$fichero" | cut -d' ' -f1)"
  [ "$real" = "$esperado" ] || { echo "ERROR sha256 $fichero: $real" >&2; exit 1; }
}

curl -fsSL -o "$BIN/yt-dlp-$TRIPLE.exe" \
  https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/yt-dlp.exe
verificar 66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a "$BIN/yt-dlp-$TRIPLE.exe"

curl -fsSL -o "$TMP/deno.zip" \
  https://github.com/denoland/deno/releases/download/v2.9.8/deno-x86_64-pc-windows-msvc.zip
verificar cba84b26e0df5ae7f74fc66b86a95b866872a86292ebd84fa1e959927edd93bf "$TMP/deno.zip"
python3 -m zipfile -e "$TMP/deno.zip" "$TMP/deno"
install -m 755 "$TMP/deno/deno.exe" "$BIN/deno-$TRIPLE.exe"

curl -fsSL -o "$TMP/ffmpeg.7z" \
  https://github.com/zhongfly/mpv-winbuild/releases/download/2026-10-14-7c4f662c08/ffmpeg-lgpl-x86_64-git-21441b340.7z
verificar 13939d7166e1335792338a88666061326f552ed786e44f57e6bfaddd33a517c7 "$TMP/ffmpeg.7z"
7z e -y "-o$TMP/ff" "$TMP/ffmpeg.7z" ffmpeg.exe >/dev/null
install -m 755 "$TMP/ff/ffmpeg.exe" "$BIN/ffmpeg-$TRIPLE.exe"

curl -fsSL -o "$TMP/mpv.7z" \
  https://github.com/zhongfly/mpv-winbuild/releases/download/2026-10-14-7c4f662c08/mpv-dev-lgpl-x86_64-20261014-git-7c4f662c08.7z
verificar 37fd9c57c2c050e23162c08b6fe3eea0555a6b6e0b2d08470555834aea0a18d3 "$TMP/mpv.7z"
7z e -y "-o$RES" "$TMP/mpv.7z" libmpv-2.dll >/dev/null

ls -lh "$BIN"/{yt-dlp,deno,ffmpeg}-$TRIPLE.exe "$RES/libmpv-2.dll"
