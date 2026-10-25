# Interfaz contextual (fase V0) — inventario, contrato y plan

Generado en la rama `feat/ui-contextos` (base `eccc594` = `v0.6-spotify-csv`)
**antes** de tocar la interfaz. Es el patrón anti-regresión de esta fase: en V6
se repiten los mismos greps de la §1.4 y **ningún conteo baja**.

> Nota de base: en este clon `master` sigue en `c774494` (base inicial), sin el
> merge de v0.6. La rama se creó en la punta `eccc594` (v0.6), cuyo árbol es el
> mismo que tendría «master + merge de v0.6» (master es ancestro de esa punta).
> `master` no se ha tocado.

Reglas de oro de la fase (vigentes para V1–V6):

1. Solo PRESENTACIÓN y composición de vista: no se toca el cuerpo de ninguna
   función de lógica.
2. Componentes nuevos en `src/lib/components/contexts/`; `App.svelte` compone,
   pasa props/callbacks y conserva estado y handlers (patrón Sidebar/ViewHeader).
3. La interfaz clásica (interruptor `musica.ui2`) queda intacta. La contextual
   solo actúa en la interfaz nueva y con `musica.uiContextos` = 1.
4. Interruptor nuevo `musica.uiContextos` (temporal; se retira en V6 solo con OK
   expreso). Apagado = idéntico a hoy.
5. Globales sin cambios: barra lateral, cola, reproductor, engranaje/Ajustes,
   búsqueda, atajos, diálogos, DlPanel y su chip de descargas.
6. Un commit por fase; gates por fase; P6 con `sha256sum src/App.svelte`.
7. Nada de datos de usuario en logs/commits; sin deps nuevas; anti-bucle; nunca
   `git reset --hard` ni borrados sin permiso.

## 1. Inventario (solo lectura, estado en `eccc594`)

### 1.1 Composición de la vista nueva (`ui2`)

De arriba abajo, en `src/App.svelte`:

| zona | línea | nota |
|---|---|---|
| Cabecera global `ViewHeader` | :2330 | [+] Carpeta · buscador · título/subtítulo · [⚙] · [tema] · [UI clásica] |
| Avisos `err` / `spotNotice` | :2348 / :2351 | globales |
| Panel de fuentes `sources-collapsible` | :2361–2556 | en `ui2` queda plegado; su botón vive en la cabecera clásica |
| Barra lateral `Sidebar` | :2564 | navegación de `ui2` (Biblioteca, playlists, fuentes, secciones) |
| `plistbar` (soltar aquí) | :2602 | `.ui2-hide` en `ui2` |
| Barra YouTube `ytbar` | :2679 | solo con vista/lote de YouTube |
| Playlist vacía `plist-empty` | :2699 | solo con playlist local vacía |
| Carga primer lote `yt-loading` | :2711 | solo con lote de YouTube |
| Herramientas de lista guardada `plist-tools` | :2719 | solo playlist local con pistas `ytmusic://` |
| Lista `VirtualList` | :2742 | única lista de pistas (regla T6 en filas) |
| «Cargar más» `yt-more` | :2793 | al FINAL de la lista (regla W0), solo con lote abierto |
| Reproductor/cola/descargas/diálogos | :2858–3153 | globales (`PlayerBar`, `Queue`, `DlPanel` en :3089, Ajustes, +Añadir…) |

Hoy no existe ninguna cabecera contextual: las «barras» de arriba aparecen
según el estado y conviven con la lista en ambos estilos.

### 1.2 Estados que definen el contexto (hoy)

| estado | significado | ejemplos |
|---|---|---|
| `plistName` | playlist local abierta | `"lista.csv guardada"` |
| `sourceId` + `sources[].kind` | fuente abierta | `youtube`, `radio`, `podcast`, `subsonic`, `jellyfin` |
| `ytImport` | vista de importación de YouTube | playlist pegada / fuente guardada |
| `ytBatch` | lote de YouTube en curso (carga, aviso, more) | `shown/total`, `warning` |
| `ytAccount` / sesión | estado de sesión YouTube | activa, caducada, sin verificar |
| `engine` (`"libmpv" \| "spotify"`) | motor de reproducción activo | `→ Spotify` / `→ libmpv` |
| `spotify` | cuenta Spotify Connect | usuario, dispositivo, plan |
| `uiSections` | secciones visibles de la barra lateral | música, youtube, radios, servers, spotify |
| `ready` / `unprepared` / `dead` | estado por fila `ytmusic://` (punto `.rdy`) | 4 variantes |
| `dlSnap` | descargas en curso (DlPanel) | `finished/total`, fallidas |

### 1.3 Controles condicionales por vista (hoy) — dónde viven

- **`ytbar`** (:2679): Kawaii + título + «(N de M)» + `Reproducir todo` + aviso
  `⚠` + `Guardar fuente` + `⬇ Descargar playlist` + `Cerrar`.
- **`plist-tools`** (:2719) solo con `plistYtCount > 0`: progreso + pausa por
  límite + `Cancelar` (`csvAllRun`); o «N sin match» + `Emparejar todas`; o
  «Todas las pistas tienen match» + `Emparejar todas` deshabilitado; siempre
  `⬇ Descargar playlist`.
- **`plist-empty`** (:2699): `Agregar carpeta…` / `Agregar canciones…`.
- **`yt-more`** (:2793): `Cargar más (quedan N)` — al final, nunca en cabecera.
- **Filas** (:2747–2788): botón fila (reproducir) + `▶` + `+` + `⬇` (solo
  `ytmusic://`) + `🔀 Buscar otra versión` (solo `ytmusic://` en playlist
  guardada) + `⋯` (playlist) + `×` (solo playlist). Punto `.rdy` (solo
  `ytmusic://`) con 4 estados: lista, sin preparar, aún no resuelta, no
  disponible (`title` = motivo).
- **DlPanel** (:3089): panel flotante global con chip minimizado.
- **Barra lateral** (:2564): las acciones de playlist (abrir, añadir cola,
  agregar carpeta/canciones, renombrar, borrar) viven aquí en `ui2`.

### 1.4 Contrato congelado vigente (medido en V0)

**(a) `api.*` — 66 nombres, 99 referencias**

```bash
grep -o "api\.[a-zA-Z]*" src/App.svelte | sort | uniq -c
```

Cada nombre debe seguir apareciendo con conteo **≥** al de la tabla.

| api.* | n | api.* | n |
|---|---|---|---|
| csvMatch | 2 | playlistReplaceTrack | 2 |
| csvPreview | 1 | playlistTracks | 1 |
| csvSavePlaylist | 1 | prev | 1 |
| csvVariants | 1 | queueAdd | 1 |
| downloadCancel | 1 | queueClear | 1 |
| downloadEnv | 1 | queueList | 2 |
| downloadProgress | 1 | queueMove | 2 |
| downloadStart | 1 | queueRemove | 1 |
| downloadStartPlaylist | 2 | queueSet | 3 |
| next | 1 | remoteSearch | 1 |
| playbackStatus | 2 | reportError | 2 |
| playIndex | 4 | retryCurrent | 1 |
| playlistAdd | 6 | scan | 1 |
| playlistAddFolder | 1 | search | 2 |
| playlistCreate | 1 | seek | 3 |
| playlistDelete | 1 | setEngine | 1 |
| playlistList | 1 | sourceAdd | 2 |
| playlistRemove | 1 | sourceList | 1 |
| playlistRename | 1 | sourceRemove | 2 |
| sourceRename | 1 | ytBatchMore | 1 |
| spotifyAccount | 1 | ytBatchOpen | 2 |
| spotifyConnect | 1 | ytBatchProgress | 1 |
| spotifyDisconnect | 1 | ytBrowsers | 1 |
| spotifyLogout | 1 | ytCached | 1 |
| stats | 1 | ytClearLoginData | 1 |
| status | 2 | ytConnect | 1 |
| toggle | 1 | ytDead | 1 |
| trackEnded | 1 | ytDisconnect | 1 |
| volume | 4 | ytGoogleLogin | 1 |
| ytAccount | 3 | ytImportCookies | 1 |
| ytBatchCancel | 1 | ytMyPlaylists | 1 |
| ytProbe | 1 | ytThrottleSecs | 5 |
| ytSetChannel | 1 | ytVerifySession | 1 |

**(b) Eventos `listen(...)` — 9**

| evento | línea |
|---|---|
| queue://changed | :2199 |
| library://changed | :2202 |
| playback://state | :2205 |
| playback://track | :2206 |
| spotify://restriction | :2207 |
| youtube://expired | :2208 |
| youtube://throttled | :2210 |
| youtube://session | :2213 |
| youtube://login | :2216 |

**(c) Atajos de teclado** (mismos que `docs/ui-contrato.md` §c; sin cambios):

Ctrl/Cmd+K buscador · Escape cierra portada/menús/confirmación · Espacio
play/pause · N siguiente · P anterior · ←/→ ±5 s · Enter guarda nombre y quita
de la cola.

**(d) `open()` del plugin-dialog — 7 usos** (import en :5):

:388 carpeta de música · :654 carpeta para playlist · :666 canciones para
playlist · :731 carpeta de descarga · :739 carpeta por defecto · :1097 carpeta
de una descarga de YouTube (playlist) · :1669 `cookies.txt` exportado.

**(e) Claves de localStorage — 10, ninguna se renombra**

- `musica.queueMin` (App :133/:697)
- `musica.pendingSourceDel` (App, confirmación de borrado con Deshacer)
- `musica.ui2` (interruptor del rediseño; no se toca en esta fase)
- `musica.ui.sections` (secciones de la barra lateral)
- `musica.dl.mode`, `musica.dl.folder`, `musica.dl.format`, `musica.dl.embed`,
  `musica.dl.prefix`, `musica.dl.v` (`src/lib/dlsettings.ts` :38–44)

Clave NUEVA permitida (esta fase la declara antes de usarla):
`musica.uiContextos` — interruptor temporal de la interfaz contextual
(por defecto 0; se retira en V6 con OK expreso).

Nota: el verificador automático del contrato vivía en `/tmp` y no persiste; se
reconstruye en V6 a partir de esta §1.4 (mismos greps).

## 2. Contextos (contrato de la fase)

### 2.1 Detección (pura, sin lógica nueva)

De arriba abajo; gana la primera que encaje:

```text
1. ytImport != null                          → "youtube"
2. sourceId != null:
     kind "youtube"                          → "youtube"
     kind "radio" | "podcast"                → "radios"
     cualquier otro (subsonic, jellyfin…)    → "servidores"
3. engine === "spotify"                      → "spotify"   (pregunta ③)
4. plistName != null o biblioteca            → "musica"
```

«Biblioteca» = sin `plistName`, sin `sourceId`, sin `ytImport`.
La detección es una expresión derivada de presentación; no cambia funciones.

### 2.2 Qué define cada contexto

**catPet local** (`musica`)
- Cabecera: nombre + recuento (Biblioteca: `N / M canciones`; playlist:
  `N pistas`). Sin acciones nuevas.
- Filas: locales normales (`▶`, `+`, `⋯`, `×`).
- Visibles: lo de hoy para listas locales; en el subcaso CSV (pregunta ②) se
  conserva `plist-tools` completo.
- Ocultos: estado de sesión YouTube, puntos `.rdy`, `⬇` por fila (no existen en
  locales), `Cargar más`, `ytbar`, chip de descargas.

**YouTube Music** (`youtube`)
- Cabecera: Kawaii + título + `(N de M)` + `Reproducir todo` + aviso `⚠` +
  `Guardar fuente` + `⬇ Descargar playlist` + `Cerrar`; línea compacta de sesión;
  leyenda de los 4 puntos de estado.
- Filas: como hoy (`⬇`, `🔀` en lista guardada, punto `.rdy`).
- Visibles: `Cargar más (quedan N)` al final de la lista; chip de descargas.
- Ocultos: nada nuevo; no cambian funciones.

**Radios y podcasts** (`radios`)
- Cabecera ligera: icono + nombre + `Radio`/`Podcast` + origen (URL/RSS).
- Filas remotas (`▶`, `+`, `⋯`). Sin `.rdy`, sin `⬇`, sin `Cargar más`.
- Renombrar/borrar: siguen en el `⋯` de la barra lateral.

**Servidores** (`servidores`)
- Cabecera ligera: icono + nombre + `Subsonic/Navidrome`/`Jellyfin` + host/usuario.
- Filas remotas, como hoy.

**Spotify** (`spotify`, ligero; pregunta ③)
- Cabecera ligera: sesión (`usuario · dispositivo «X» · plan` o `Sin conectar`)
  y conmutador de motor `→ libmpv` / `→ Spotify` (función existente).
- La lista y la cola actuales no cambian; el reproductor global manda.

### 2.3 Invariantes

- Globales intactos (regla 5): barra lateral, cola, reproductor, engranaje,
  búsqueda, atajos, diálogos y DlPanel.
- `VirtualList` sigue siendo la única lista; regla T6 (botones hermanos) intacta.
- `Cargar más` se queda al final de la lista (regla W0), no en la cabecera.
- Los textos de estado de sesión de YouTube no se reescriben.
- Con `uiContextos` = 0 (o `ui2` = 0) el DOM es el de hoy, tal cual.

## 3. Maquetas ASCII de las 5 cabeceras

La cabecera global (ViewHeader) no cambia; la contextual va justo debajo, entre
ella y la lista:

```text
[+ Carpeta] [Buscar (Ctrl+K)…]        Título            [⚙] [🌓] [UI clásica]
                                      subtítulo
```

**1) catPet local — biblioteca / playlist local**

```text
┌ Contexto «catPet local» ─────────────────────────────────────────────────────┐
│ ♪  Biblioteca                                         123 / 456 canciones    │
└──────────────────────────────────────────────────────────────────────────────┘
  ▶ filas locales  [▶] [+] [⋯]        (sin ytbar, sin puntos de estado, sin
  ▶ filas locales  [▶] [+] [⋯]         descargas, sin «Cargar más»)
```

**1b) catPet local — subcaso CSV (playlist con pistas `ytmusic://`, pregunta ②)**

```text
┌ Contexto «catPet local» ─────────────────────────────────────────────────────┐
│ ♪  Mi lista del CSV                        42 pistas · 6 sin match           │
│ [Emparejar todas]   [⬇ Descargar playlist]                                   │
└──────────────────────────────────────────────────────────────────────────────┘
  ▶ filas  [▶] [+] [⬇] [🔀] [⋯] [×]   (los controles S4 se conservan tal cual)
```

**2) YouTube Music**

```text
┌ Contexto «YouTube Music» ────────────────────────────────────────────────────┐
│ 🎵  Título de la playlist        (37 de 120)      Sesión: ✓ activa           │
│ [▶ Reproducir todo] [Guardar fuente] [⬇ Descargar playlist] [Cerrar]        │
│ Puntos: ● lista · ◌ sin preparar · ○ aún no resuelta · ✕ no disponible      │
│ [⬇ 2/12 · 1 fallida]  ← chip de descargas (mismos datos de DlPanel)         │
└──────────────────────────────────────────────────────────────────────────────┘
  ▶ filas con punto (.rdy) [⬇] [🔀] [⋯]
  ▶ …                                                    [Cargar más (quedan N)]
```

**3) Radios y podcasts**

```text
┌ Contexto «Radios y podcasts» ────────────────────────────────────────────────┐
│ 📻  Mi emisora                         · Radio · URL directa                 │
└──────────────────────────────────────────────────────────────────────────────┘
  (o bien:  🎙 Mi podcast  · Podcast · RSS)
  ▶ filas remotas  [▶] [+] [⋯]     renombrar/borrar: ⋯ de la barra lateral
```

**4) Servidores**

```text
┌ Contexto «Servidores» ───────────────────────────────────────────────────────┐
│ 🔌  Mi servidor                       · Subsonic/Navidrome · host            │
└──────────────────────────────────────────────────────────────────────────────┘
  ▶ filas remotas  [▶] [+] [⋯]
```

**5) Spotify (ligero)**

```text
┌ Contexto «Spotify» ──────────────────────────────────────────────────────────┐
│ ♫  Spotify   Conectado: usuario · dispositivo «X» · plan premium  [→ libmpv] │
└──────────────────────────────────────────────────────────────────────────────┘
  (sin conectar)
│ ♫  Spotify   Sin conectar                                   [Abrir Ajustes ›] │
  ▶ la lista central no cambia (cola y reproductor globales mandan)
```

## 4. Plan de componentes y fases

### 4.1 Componentes (nuevos, solo presentación)

```text
src/lib/components/contexts/
  ContextBar.svelte    — marco común: recibe {ctx} + datos/callbacks ya
                         existentes en App y decide qué sub-cabecera pinta.
  CtxMusica.svelte     — biblioteca / playlist local (+ subcaso CSV)
  CtxYoutube.svelte    — ytbar re-hogarizada + sesión + leyenda de puntos
  CtxRadios.svelte
  CtxServidores.svelte
  CtxSpotify.svelte
```

Props = valores ya presentes en `App.svelte` (`plistName`, `ytImport`,
`ytBatch`, `ytAccount`, `engine`, `spotify`, `dlSnap`…) y callbacks a funciones
ya existentes (`playYtAll`, `saveYtAsSource`, `askDownloadPlaylist`,
`askDownloadSavedPlist`, `csvAllAskPlist`, `ytLoadMore`, `closeYtView`,
`setEngine`…). Estado y `api.*` se quedan en `App.svelte`.

### 4.2 Integración y semántica del interruptor

- `musica.uiContextos` en `App.svelte`; por defecto 0. Toggle temporal en
  Ajustes › Apariencia (pregunta ④).
- `{#if ui2 && uiContextos}` inserta `ContextBar` justo encima del bloque de
  barras actual. En V1 las cinco sub-cabeceras salen vacías (solo marcador del
  contexto) y las barras actuales siguen como hoy.
- A partir de V2/V3, cuando el interruptor está encendido, las barras viejas de
  ese contexto se ocultan con un envoltorio presentacional
  (`{#if !(ui2 && uiContextos)}` alrededor del bloque, sin tocar nada más) y su
  contenido se re-hogariza en el componente. Apagado = DOM idéntico a hoy.

### 4.3 Fases

- **V1** — Armazón: detección + 5 cabeceras vacías + interruptor. Apagado =
  idéntico a hoy. Gates + P6. **✔** (commit `aa34780`).
- **V2** — Contexto catPet local completo. **✔** (ver §8).
- **V3** — Contexto YouTube Music completo. *Prueba manual del usuario antes de
  V4* (protocolo acordado).
- **V4** — Radios, Servidores y Spotify (ligeros).
- **V5** — Transiciones, foco/teclado y ancho estrecho (~1000 px).
- **V6** — Verificación del contrato (§1.4) y retirada del interruptor SOLO con
  OK expreso. No se toca `musica.ui2` (su retirada es otra fase, U6b).

Gates por fase: `cargo check` 0/0, `cargo test` sin fallos, `npm run build`
(svelte-check 0/0 + vite build), `npm test` 4/4, contrato §1.4 ≥, P6 con
`sha256sum src/App.svelte`.

## 5. Riesgos y mitigaciones

| riesgo | mitigación |
|---|---|
| `master` sin merge de v0.6 (la rama cuelga de `eccc594`) | contenido idéntico; master no se toca; se decide en la pregunta ① |
| Duplicación temporal de marcado (barra vieja + componente) entre V1 y V6 | «mover, no copiar» en cada fase; el envoltorio apagado deja el DOM de hoy tal cual; limpieza en V6 |
| Perder funciones al re-hogarizar | checklist por contexto + contrato §1.4 que no baja + regla T6/W0 |
| Detección ambigua (playlist local con `ytmusic://`, fuente `youtube`, motor Spotify) | prioridad fija de §2.1 + subcaso CSV explícito (pregunta ②) |
| Regresión visual en el clásico | la contextual solo existe con `ui2 && uiContextos`; clases/variables actuales, sin tocar estilos base |
| Ancho estrecho (~1000 px) y textos largos (títulos, sesión) | truncado con ellipsis y `title`; se afina en V5 |
| Foco/teclado en botones nuevos | orden natural de tabulación, `title` en todos; Escape/atajos no cambian; revisión en V5 |
| Claves de usuario | solo se añade `musica.uiContextos`; las 10 existentes no se renombran |

## 6. Preguntas para tu OK

1. **Base**: `master` no tiene el merge de v0.6. La rama `feat/ui-contextos`
   cuelga de `eccc594` (v0.6), contenido idéntico a «master + merge». ¿Te vale
   así (tú haces el merge cuando quieras) o prefieres fusionar master antes?
2. **Subcaso CSV**: ¿conservamos en «catPet local» los controles actuales de
   las playlists con pistas `ytmusic://` (`Emparejar todas`, `⬇ Descargar
   playlist`, `🔀`, `⬇` y punto por fila)? Recomendado: sí («TODAS las
   funciones»).
3. **Contexto Spotify**: A) aparece cuando el motor activo es Spotify
   (recomendada); B) convertir la sección Spotify de la barra lateral en
   clicable (tocaría `Sidebar.svelte`).
4. **Interruptor temporal**: ¿toggle en Ajustes › Apariencia, como el de la
   interfaz clásica?
5. **Confirmación del plan**: la cabecera contextual SUSTITUYE a las barras
   actuales del contexto cuando el interruptor está encendido (conviven solo en
   V1, vacías). ¿OK?

---

P6 V0: `sha256(App.svelte)` =
`89adb44062ef0829f8cb14496488dc5acf6b7d6a33e4029038e78bf0cd324d9e`
(sin cambios respecto a `eccc594`; V0 no toca la interfaz).

## 7. Diseño visual «Noche cálida con identidad por contexto» (V1)

Decisiones de la revisión de V0 (aplicadas): ① la rama cuelga de v0.6 (el merge
de `master` lo hace el usuario); ② el subcaso CSV conserva TODOS los controles;
③ contexto Spotify con `engine === "spotify"` (opción A); ④ interruptor
temporal en Ajustes › Apariencia; ⑤ la cabecera contextual sustituye a las
barras del contexto con el interruptor encendido (en V1 conviven, vacías).

### 7.1 Selector de los tokens

- Claro: `body.ui-ctx`
- Oscuro («Noche cálida»): `[data-theme="dark"] body.ui-ctx`

La clase `ui-ctx` en `<body>` la pone `App.svelte` ($effect) **solo** con
`ui2 && uiContextos`; apagada no existe y el DOM/CSS son los de siempre.

### 7.2 Tokens

| token | oscuro | claro | uso |
|---|---|---|---|
| `--bg` | #201912 | #EFE7D8 | fondo |
| `--bg-sunken` | #171009 | #E3D9C6 | fondo hundido |
| `--surface` | #2A2218 | #F7F1E4 | tarjeta |
| `--surface-2` | #362C1F | #FDF9F1 | elevado |
| `--border` / `--border-soft` | #4A3C29 / #3C3122 | #C9BAA0 / #DAD0BB | bordes finos |
| `--text` | #EFE5D3 | #332A1F | texto (crema, nunca blanco puro) |
| `--muted` | #B7A284 | #6C5D49 | subtítulos/ayudas |
| `--accent` | #D08A4E | #A6562C | acento naranja actual (acción principal y activo) |
| `--on-accent` | #2A1D10 | #FBF6E9 | texto sobre acento |
| `--ctx-musica` | #E2A94F | #8A5A10 | ámbar |
| `--ctx-youtube` | #EC8F76 | #B0472D | coral suave |
| `--ctx-radios` | #60B7A7 | #14695E | verde azulado |
| `--ctx-servidores` | #89A2CA | #43567C | azul pizarra |
| `--ctx-spotify` | #83A97B | #456840 | verde apagado |

El matiz por contexto se usa SOLO en el chip de icono de la cabecera, la línea
fina superior de la tarjeta y el indicador de la barra lateral (activo y punto
de sesión); **nunca como fondo**.

### 7.3 Contraste WCAG AA medido (script `wcag.mjs`)

Umbrales: texto ≥ 4.5:1; componentes (matiz, acento) ≥ 3:1.

| par | oscuro | claro |
|---|---|---|
| texto / fondo | 13.91 | 11.46 |
| texto / tarjeta | 12.55 | 12.51 |
| texto / elevado | 10.94 | 13.41 |
| muted / fondo | 7.04 | 5.18 |
| muted / tarjeta | 6.35 | 5.65 |
| on-accent / acento | 5.80 | 4.88 |
| acento / tarjeta | 5.54 | 4.68 |
| ámbar / tarjeta | 7.48 | 5.25 |
| coral / tarjeta | 6.53 | 4.93 |
| verde azulado / tarjeta | 6.59 | 5.81 |
| azul pizarra / tarjeta | 6.03 | 6.52 |
| verde apagado / tarjeta | 5.91 | 5.64 |

### 7.4 Reglas visuales y de rendimiento (V1–V5)

- Movimiento 150–220 ms ease-out, solo opacity/transform; al cambiar de
  contexto solo la cabecera hace fundido; `prefers-reduced-motion` ya anula
  animaciones y transiciones en el bloque global de `styles.css`.
- PROHIBIDO: `backdrop-filter`, `filter: blur`, sombras difusas grandes y
  degradados animados (render por software, `./musica.sh soft`). La
  profundidad se logra con niveles de color y bordes finos.
- Cabecera contextual: tarjeta, chip 38 px con icono en el matiz, título 17 px
  y subtítulo discreto; en V2–V4 se añaden la acción principal (único botón
  con acento) y las secundarias en un menú ⋯.
- Por fase: filas ~56 px con portada 40 px redondeada, acciones al pasar el
  ratón/enfocar con etiqueta accesible y pista activa con barra de acento y
  ecualizador sencillo (V2); puntos de estado de YouTube con leyenda (V3);
  reproductor como tarjeta con progreso más grueso al enfocar (V5).

## 8. V2: contexto «catPet local» (filas y subcaso CSV)

### 8.1 Qué se re-hogariza (mover, no copiar)

- `plist-tools` (subcaso CSV de la lista guardada) se oculta **solo** con
  `ui2 && uiContextos` (`&& !(ui2 && uiContextos)` en el `{#if}`) y sus
  controles viven ahora en la cabecera (`CtxMusica.svelte`): «Emparejar todas»
  es la acción principal (único botón con acento) con los estados exactos de
  S4.2b —progreso, backoff, cancelación, «todas con match»— y «Descargar
  playlist» pasa al menú «⋯» (siempre disponible, también durante el barrido).
  Mismos handlers (`csvAllAskPlist`, `csvAllAbort`, `askDownloadSavedPlist`) y
  mismos textos. `plist-empty` no es una barra: se queda como está.
- `ContextBar.svelte` gana el snippet opcional `actions` (contenedor `.ctx-acts`
  a la derecha del título); sin snippet, V1 queda idéntica.
- Apagado, el DOM es el de hoy: el `{#if}` clásico vuelve a ser el de siempre,
  el arte clásico (34 px) se renderiza como siempre y las etiquetas accesibles
  nuevas no se emiten (`aria-label={ctxRows ? … : undefined}`).
- Decisión a revisar: si se prefiere «Descargar playlist» como principal y
  «Emparejar todas» dentro del «⋯», es un intercambio de una línea.

### 8.2 Filas (~56 px) y detalle visual

- Sincronía de altura: `rowHeight={ui2 && uiContextos ? 56 : 44}` en
  `VirtualList` y `body.ui-ctx { --row-h: 56px }` en CSS (apagado: 44 px, como
  hoy); `--row-h` solo lo usa `.row` → sin saltos de scroll.
- Portada 40 px redondeada o **marcador** (`Icon note`) cuando no hay arte —la
  BD local no guarda portadas (`db.rs:108`), así que es el caso común—; las
  imágenes siguen con `loading="lazy"` (+`decoding="async"`).
- Duración con cifras tabulares y ancho mínimo; acciones al pasar el ratón o al
  enfocar con teclado (regla existente) sobre fondo sólido (sin degradado) y
  `aria-label` en todas; pista activa con barra de acento + ecualizador de
  3 barras (solo `transform`; `prefers-reduced-motion` lo deja estático).
  T6, arrastre a la barra lateral y menú ⋯ de fila intactos.

### 8.3 Rendimiento medido (`./musica.sh soft`, perfiles aislados)

Método: 3 perfiles aislados (`XDG_*` en `/tmp`, nunca datos de usuario) con
0 / 300 / 20 000 pistas sintéticas insertadas en la BD del perfil; ventana por
defecto con render por software. Host = muestreo propio `[res]` (una línea cada
2 s); WebKit = `ps` a los ~30 s del arranque (una muestra; orientativo).
MiB = kB/1024.

| pistas | primer `[res]` | host | WebKitWebProcess | árbol app* | errores |
|---|---|---|---|---|---|
| 0 | 8.0 s | 262.9 MiB | 168.9 MiB | 501.3 MiB | ninguno |
| 300 | 8.0 s | 261.9 MiB | 174.8 MiB | 505.4 MiB | ninguno |
| 20 000 | 8.0 s | 264.4 MiB | 229.4 MiB | 562.8 MiB | ninguno |

\* árbol app = `musica` + `WebKitWebProcess` + `WebKitNetworkProcess`.

Lectura: arranque y proceso principal estables con la biblioteca (±2.5 MiB; el
total se consulta por `COUNT`, no cargando filas). WebKit crece +6 MiB con 300
pistas (100 filas iniciales, `INIT_PAGE`) y +60 MiB con 20 000, pero en **los
tres casos se cargan las mismas 100 filas y se montan ~27 nodos** (windowed):
el salto acompaña al tamaño del contenedor virtual (20 000 × 56 px ≈ 1.1 M px)
y al estado de scroll de WebKit, no al DOM de las filas. Cierre limpio en los
tres casos; `hijos_ytdlp=0` y `[res]` plano en la ventana medida.
