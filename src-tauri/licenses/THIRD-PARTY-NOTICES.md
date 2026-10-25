# THIRD-PARTY NOTICES — Música

El código propio de este proyecto se distribuye bajo la licencia **MIT**
(ver `LICENSE`). Los componentes de terceros conservan sus licencias.

## Incluidos en Linux y Windows

| Componente | Versión / origen | Licencia | Fuente |
|---|---|---|---|
| yt-dlp (Linux) | 2026.08.19, `yt-dlp_linux`, sha256 `58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a` | Unlicense | https://github.com/yt-dlp/yt-dlp |
| yt-dlp (Windows) | 2026.08.19, `yt-dlp.exe`, sha256 `66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a` | Unlicense | https://github.com/yt-dlp/yt-dlp |
| Deno (Linux) | 2.9.8, sha256 `e890975bc2549807bbcc0b584ffafda72cc7c93349dd99c80483dd784f6c4609` | MIT | https://github.com/denoland/deno |
| Deno (Windows) | 2.9.8, sha256 `cba84b26e0df5ae7f74fc66b86a95b866872a86292ebd84fa1e959927edd93bf` | MIT | https://github.com/denoland/deno |
| FFmpeg Linux | BtbN `autobuild-2026-10-22-13-06`, `ffmpeg-n9.0.2-33-g4cf298e66a-linux64-lgpl-9.0.tar.xz`, sha256 `9cecf478d8a0ac5fbb41cc8d6c0ef9d318c1950f546670673c7f63fd11dae23b`; build **LGPL, estático** (no shared) | LGPL-2.1-or-later | https://github.com/FFmpeg/FFmpeg (commit `4cf298e66a`); scripts: https://github.com/BtbN/FFmpeg-Builds |
| FFmpeg Windows | zhongfly `2026-10-14-7c4f662c08`, `ffmpeg-lgpl-x86_64-git-21441b340.7z`, sha256 `13939d7166e1335792338a88666061326f552ed786e44f57e6bfaddd33a517c7` | LGPL-2.1-or-later | https://github.com/zhongfly/mpv-winbuild |
| libmpv Linux | PPA `ubuntuhandbook1/mpv`, `0.38.0-0ubuntu1~jammy`; `libmpv.so.2` empaquetado | libmpv: LGPL-2.1-or-later (mpv: GPL-2.0-or-later; ver copyright del paquete) | https://launchpad.net/~ubuntuhandbook1/+archive/ubuntu/mpv ; https://github.com/mpv-player/mpv |
| libmpv Windows | zhongfly `2026-10-14-7c4f662c08`, `mpv-dev-lgpl-x86_64-20261014-git-7c4f662c08.7z`, sha256 `37fd9c57c2c050e23162c08b6fe3eea0555a6b6e0b2d08470555834aea0a18d3`; `libmpv-2.dll` | LGPL-2.1-or-later / LGPLv3 | https://github.com/zhongfly/mpv-winbuild ; fuente mpv commit `7c4f662c0` |

## Bibliotecas del AppImage

El AppImage incluye bibliotecas de Ubuntu 22.04 (WebKitGTK, GTK, GStreamer y
dependencias). Sus textos de licencia están en los paquetes de Ubuntu; el código
fuente se obtiene en https://archive.ubuntu.com/ubuntu/ (paquetes fuente de la
misma versión). No se afirma compatibilidad entre licencias.

## Reemplazo y obligaciones

- **yt-dlp**: Unlicense permite reemplazarlo libremente.
- **Deno**: MIT exige conservar aviso de copyright y licencia.
- **FFmpeg LGPL estático**: al estar enlazado estáticamente, redistribuir una
  versión modificada exige poder relinkar/reemplazar; el binario se puede
  sustituir por otro build LGPL. Si se necesita reemplazo sencillo, usar un
  build **LGPL shared** y decidir qué hacer con sus `.so`.
- **libmpv LGPL**: se incluye como `.so`/DLL dinámica; hay que permitir su
  sustitución y conservar avisos. Decisión pendiente: mantener libmpv dinámico
  y su reemplazo, o enlazarlo de otra forma.
- **Licencia del proyecto**: MIT para el código propio; los binarios y
  bibliotecas de terceros mantienen sus licencias.
