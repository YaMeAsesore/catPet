# Windows: build, instalación y pruebas

## CI

`.github/workflows/windows.yml` corre en `windows-latest` y publica el
artefacto **`catPet-windows-nsis`** con `catPet_0.1.0_x64-setup.exe`. Se
dispara a mano (**Actions → Windows → Run workflow**) o al crear un tag `v*`.
No usa secretos ni firma de código.

Orden: `cargo check --target x86_64-pc-windows-msvc`, descargas con
verificación `sha256`, generación de `mpv.lib` desde los exports reales de la
DLL, `npm ci`, `tauri build`, comprobación de que `libmpv-2.dll`, `yt-dlp.exe`
y `ffmpeg.exe` están en la salida y dentro del instalador, y subida del
artefacto.

## Binarios de terceros

| Binario | Fuente (tag fijo) | sha256 |
|---|---|---|
| `libmpv-2.dll` (LGPL) | zhongfly/mpv-winbuild `2026-10-14-7c4f662c08` | `37fd9c57c2c050e23162c08b6fe3eea0555a6b6e0b2d08470555834aea0a18d3` |
| `ffmpeg.exe` (LGPL) | zhongfly/mpv-winbuild `2026-10-14-7c4f662c08` | `13939d7166e1335792338a88666061326f552ed786e44f57e6bfaddd33a517c7` |
| `yt-dlp.exe` | yt-dlp/yt-dlp `2026.08.19` | `66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a` |

Los binarios se descargan con `tools/fetch-windows-deps.ps1` (o
`tools/fetch-windows-deps.sh` desde Linux) y no se versionan.

## Instalación y tamaño

- Instalación para el usuario actual en `%LOCALAPPDATA%\Programs\catPet`.
- Datos en `%APPDATA%\com.musica.app\library.db`.
- Tamaño del instalador: del orden de 100 MB, sobre todo por `libmpv-2.dll`,
  `ffmpeg.exe` y `yt-dlp.exe`.
- `libmpv-2.dll` se puede sustituir sin reinstalar; sigue el
  [LEEME de licencias](../src-tauri/licenses/LEEME-licencias-y-sustitucion-libmpv.txt).

## SmartScreen

El instalador no está firmado. Windows puede mostrar «Windows protegió su PC»
al ejecutarlo. Para continuar: **Más información → Ejecutar de todos modos**.
Hazlo solo si el `.exe` viene de tu propio workflow o de una fuente de
confianza.

## Checklist de pruebas manuales

- [ ] El artefacto `catPet-windows-nsis` descarga y el `.exe` instala sin pedir
      administrador.
- [ ] Queda `%LOCALAPPDATA%\Programs\catPet` con `catpet.exe`,
      `libmpv-2.dll`, `yt-dlp.exe`, `ffmpeg.exe` y `licenses\`.
- [ ] Arranque correcto: ventana «catPet», sin consola negra; WebView2
      presente.
- [ ] Escaneo de una carpeta con mp3/m4a/webm: entran con etiquetas y duración;
      la búsqueda responde.
- [ ] Reproducción local: play/pausa, siguiente/anterior, seek, volumen y
      gapless.
- [ ] Teclas multimedia / SMTC de Windows.
- [ ] Descarga en m4a y mp3: se genera el archivo final y, con «embed», las
      etiquetas y la portada.
- [ ] Descarga sin `ffmpeg.exe`: guarda el original sin romperse.
- [ ] Spotify Premium: conectar cuenta, atribución y reproducción; cuenta Free
      muestra un mensaje claro.
- [ ] Radios/podcasts/Subsonic/Jellyfin, si se usan.
- [ ] Reinicio: la BD conserva biblioteca, playlists y cola; SMTC vuelve a
      registrarse.
- [ ] Desinstalador: quita la app y sus accesos.

**Estado:** el instalador y la app todavía no se han probado en una máquina
Windows real.
