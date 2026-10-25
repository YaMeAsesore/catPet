# Binarios de terceros (no versionados)

Esta carpeta la crean los scripts de `tools/` y contiene los binarios
incluidos para las funciones de YouTube y descargas:

- `yt-dlp-x86_64-unknown-linux-gnu`
- `deno-x86_64-unknown-linux-gnu`
- `ffmpeg-x86_64-unknown-linux-gnu`

En Windows, los equivalentes con extensión `.exe`.

No se suben al repositorio por tamaño y porque se descargan con URL y
`sha256` fijos. Para obtenerlos:

```bash
./tools/fetch-linux-deps.sh
```

En Windows, desde PowerShell:

```powershell
./tools/fetch-windows-deps.ps1
```

La app también puede usar `yt-dlp`/`ffmpeg` del sistema si están en el `PATH`.
