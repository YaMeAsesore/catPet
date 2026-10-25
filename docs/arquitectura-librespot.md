# Arquitectura: motor Spotify nativo (librespot) + Router de reproducción

> Fecha: 2026-10-11 · Estado: **implementado y verificado en máquina de desarrollo**
> (`cargo check` + `cargo test` 30/30 OK; frontend `svelte-check`+`vite build` OK).
> Sustituye al Web Playback SDK por audio nativo Rust, con doble motor
> (libmpv local + librespot Spotify Connect) y estado de reproducción unificado.

---

## 1. Veredicto de viabilidad

| Requisito | Resultado | Evidencia |
|---|---|---|
| librespot 0.8.0 compila en Linux (ALSA + PulseAudio, rustls, libmdns) | ✅ | Build real en este equipo; `Session::new` + `Player::new` ejecutan sin fricción |
| Compilación cruzada macOS/Windows | ✅ (por diseño) | Backend nativo por plataforma: `rodio` → WASAPI/CoreAudio; keyring nativo por OS |
| Autenticación con token PKCE | ✅ | `Credentials::with_access_token` → APWelcome devuelve credenciales reusable (user+blob) |
| Gate Premium **antes** de inicializar | ✅ | Pre-check Web API `/v1/me.product`; `account_gate` puro + tests |
| Persistencia segura de credenciales | ✅ | keyring del SO (Secret Service/Keychain/Credential Manager) + degradación documentada |
| Play/Pause/Next/Prev/Seek/Volumen/Shuffle/Repeat | ✅ | Handle público de `Spirc` (`connect/src/spirc.rs`) |
| Eventos de reproducción (posición, volumen, track) | ✅ | `PlayerEventChannel` + `position_update_interval=500ms` |
| Atribución visible (artista/álbum/carátula) | ✅ | `librespot-metadata::Track` → `TrackAttribution` + carátula i.scdn.co |
| MPRIS/SMTC para ambos motores | ✅ | souvlaki se alimenta del router, no del motor |
| Crossfade real entre motores | ⚠️ **Limitación** | Ver §6: fade-out + fade-in, no mezcla simultánea |

**Bug upstream hallado durante la verificación:** `librespot-core 0.8.0` no compila
con las últimas versiones de `vergen` (su build-dependency `vergen-gitcl 1.0.8`
implementa traits sobre `vergen-lib 0.1`, pero `vergen 9.1.0` depende de
`vergen-lib 9.x`). **Solución aplicada:** pin `vergen = "=9.0.6"` en
`src-tauri/Cargo.toml` (comentado en el manifiesto).

---

## 2. Diagrama de módulos

```
┌────────────────────────────────────────────────────────────────┐
│ Frontend (Svelte 5)                                            │
│  <App.svelte>          <src/lib/tauri.ts>                      │
│   · buscador/cola        · api.* (wrappers tipados)            │
│   · panel Spotify        · tipos de eventos IPC                 │
└───────────────┬────────────────────────────────────────────────┘
                │ invoke(cmd)            ▲ emit(eventos)
                ▼                        │
┌────────────────────────────────────────────────────────────────┐
│ ipc_bridge.rs  (comandos Tauri + eventos tipados)              │
│  player_play|pause|next|prev|seek|volume|toggle|status          │
│  playback_status|set_engine|shuffle|repeat                      │
│  spotify_connect|disconnect|logout|account|load|search          │
└───────────────────────────┬────────────────────────────────────┘
                            │ (única puerta)
                            ▼
┌────────────────────────────────────────────────────────────────┐
│ playback_router.rs   PlaybackRouter  (State tras Mutex)        │
│  · motor activo (libmpv | spotify) + set_engine()               │
│  · estado unificado: paused/pos/dur/vol/track                   │
│  · engine_for_track("spotify:track:…")                          │
│  · emite playback://state|track|position|volume|queue           │
└─────────┬───────────────────────────────┬──────────────────────┘
          │ motor Libmpv                  │ motor Spotify
          ▼                               ▼
┌─────────────────────┐   ┌───────────────────────────────────────┐
│ player.rs           │   │ librespot_engine.rs                   │
│ libmpv (nativo)     │   │  · account_gate (Premium pre-check)    │
│ gapless · ReplayGain│   │  · credentials_plan (PKCE → reusable)  │
│                     │   │  · SessionConfig + ConnectConfig       │
│                     │   │  · backend ALSA/Pulse│rodio            │
│                     │   │  · Spirc (play/pause/next/…/load)      │
│                     │   │  · loop PlayerEvent → RouteEvent       │
│                     │   │  · metadata → TrackAttribution         │
└─────────────────────┘   └──────────────┬────────────────────────┘
                                         │ credenciales
                                         ▼
┌────────────────────────────────────────────────────────────────┐
│ keyring.rs  (keyring-core + store nativo por plataforma)       │
│  Secret Service (zbus) │ Keychain │ Credential Manager          │
│  fallback: keyutils (sesión) → memoria (avisado)                │
└────────────────────────────────────────────────────────────────┘
        ▲ souvlaki (MPRIS/SMTC) lee SIEMPRE del router
        │ → teclas multimedia funcionan igual con los dos motores
```

## 3. Flujo de autenticación

```
 Usuario pega token PKCE           keyring del SO
        │                                │
        ▼                                │
 spotify_connect(token)                  │
        │                                │
        ▼                                │
 ┌──────────────────────────┐            │
 │ GET /v1/me (Web API)     │◄───────────┘ (token Bearer)
 │  → product               │
 └──────────┬───────────────┘
            │ account_gate(product)
   free/open│        │ premium              │ desconocido
            ▼        ▼                      ▼
      [Error claro   ┌──────────────────┐   [Error: no se pudo
       "solo         │ Cache con token:  │   verificar]
       Premium"]     │ Cache::new(creds) │
                     └────────┬─────────┘
                              ▼
                     Session::connect(creds, true)
                       Spirc::new(...) ──► APWelcome: reusable creds
                              │
                              ▼
                     persist_credentials(JSON) ──► keyring
                     borra cache temporal
                              │
                     handle listo: Spirc + loop de eventos
```

**Arranques siguientes:** `credentials_plan(None)` lee el keyring y reutiliza las
credenciales reusable (no caducan) → reconexión silenciosa sin nuevo OAuth.

## 4. Tipos y eventos IPC

| Canal | Payload | Notas |
|---|---|---|
| `playback://state` | `{ engine: "libmpv"\|"spotify", state }` | motor siempre conocido |
| `playback://track` | `{ engine, track: TrackAttribution\|null }` | atribución presente siempre que hay track |
| `playback://position` | `{ position_ms, duration_ms }` | tick 500ms con Spotify activo |
| `playback://volume` | `{ percent }` | al cambiar desde cualquier vía |
| `playback://queue` | `{ current, length }` | + legacy `queue://changed` |
| `spotify://account` | `SpotifyAccountInfo` | tras conectar/desconectar |
| `player://state` (legacy) | `"playing"\|"paused"\|"volume"` | compatibilidad UI antigua |

## 5. Backends de audio por plataforma

| Plataforma | Backend librespot (compilado) | keyring |
|---|---|---|
| Linux | ALSA → PulseAudio (reserva) | Secret Service (zbus) → keyutils → memoria |
| macOS | rodio (CoreAudio) | Keychain |
| Windows | rodio (WASAPI) | Credential Manager |

La selección está en `AudioBackend::resolved()` con cadena de reserva probada en
tests (`backend_resuelto_por_plataforma`) y features condicionadas por target en
`Cargo.toml` (Linux no compila rodio; macOS/Windows no compilan ALSA).

## 6. Limitaciones (honestas)

1. **Solo Spotify Premium.** Las cuentas Free rechazan la autenticación de
   librespot. Peor: `check_catalogue` de librespot ejecuta `exit(1)` y **mataría
   el proceso entero**. Mitigación implementada: `account_gate()` deniega por
   defecto (Premium solo con confirmación explícita) antes de crear la sesión.
2. **Sin Secret Service en Linux** (p.ej. sesión mínima): el keyring degrada a
   keyutils (no persiste tras reinicio) o a memoria; la UI lo avisa
   (`persistent_keyring: false`).
3. **Crossfade entre motores:** los dos motores poseen su sink nativo (mpv y el
   player de librespot); no hay grafo compartido. Política real:
   fade-in `crossfade_secs` al activar Spotify; el corte es percibible pero
   documentado. Crossfade *dentro* de librespot sí es nativo (gapless del
   contex resolver).
4. **Token PKCE caduca (1 h).** Si caduca y no hay credenciales reusable en el
   keyring, hay que reconectar; `spotify_connect` lo deja claro en el error.
5. **Cola de Spotify:** mientras Spotify es el motor activo, la cola la gobierna
   el servidor de Spotify (Spirc); la cola local se mantiene para volver a mpv
   sin pérdida.
6. **Rate limiting / DRM:** se usa la API pública (Web API + Connect) con
   credenciales del usuario; ningún componente bypasea DRM.

## 7. Estrategia de testing

**Unit tests (sin red, sin cuenta) — implementados:**

- `librespot_engine::tests`
  - `gate_premium_pasa_free_no` — free/open/unknown denegados, Premium pasa.
  - `volumen_ui_u16_roundtrip` — mapeo UI% ↔ u16 de Spirc (±1 de error).
  - `eventos_basicos_se_mapean`, `eventos_de_posicion_y_track`,
    `unavailable_y_end_of_track_llegan_al_router` — mocks del
    `PlayerEventChannel` (eventos construidos a mano) → `RouteEvent`.
  - `eventos_internos_no_generan_ruido_ipc` — eventos internos de Spirc filtrados.
  - `plan_prefiere_token_fresco`, `credenciales_serializan_json_compatible`.
  - `backend_resuelto_por_plataforma`.
- `playback_router::tests`
  - `enrutado_por_path` — `spotify:*` → Spotify; resto → libmpv.
  - `plan_de_transicion` — crossfade solo entre motores distintos.
  - serialización de eventos IPC.
- `keyring::tests` — roundtrip en memoria/degradación sin daemon.
- Preexistentes (db/queue/remote/providers) intactos: **30/30 en verde**.

**Integration tests manuales (requieren cuenta Premium real):**

```bash
# 1) obtener token PKCE (flujo OAuth del portal de desarrollo de Spotify)
# 2) pegar en el panel Spotify de la app → verifica Premium → conecta
# 3) desde el móvil/escritorio oficial: elegir dispositivo "catPet" →
#    transferir reproducción → los eventos playback://* deben actualizar UI y MPRIS
# 4) teclas multimedia (play/pause/next) deben controlar el motor activo
```

Pendiente de automatizar: mock del canal `PlayerEvent` alimentado con un
fixture (ya cubierto por los tests puros de `map_player_event`).

## 9. Proxy de restricciones, login5 y cuentas Free (2026-10-11)

### Derivación de credenciales: por qué `with_access_token` y no login5
En escritorio **no existe bootstrap por login5**: `login(id, password)` devuelve
`OnlyForMobile` fuera de android/ios y `auth_token()` exige una sesión ya
autenticada (`NoStoredCredentials` en caso contrario). El camino soportado es:
token PKCE → `Credentials::with_access_token` → `Session::connect`, que devuelve
las credenciales reusable vía APWelcome y se persisten en el keyring. Lo que SÍ
se hace con login5 tras conectar: `session.login5().auth_token()` acuña el token
de Web API con la credencial reusable (sirve para búsquedas sin otro OAuth) y se
refresca en el keyring. Los fallos de este paso y de `Spirc::new` se clasifican
(`AuthFailure`: FreeAccount/BadToken/Network/Server) a mensajes claros en español.

### Proxy de política (`spotify_policy.rs`)
Filtra comandos antes de Spirc y emite `spotify://restriction` (la UI muestra un
aviso ámbar auto-descartable con el motivo, saltos usados y espera):
- Cuentas restringidas: 6 saltos/hora deslizante (`next` consume; `prev` no, pues
  reinicia el tema). Bajo demanda de URI concreta: denegado.
- Premium: pass-through (el servidor aplica sus reglas).
- Sin handle: `disconnected`. El tier verificado se persiste en keyring (`tier`).
- Si un salto se deniega, la cola local **no avanza** (router) y el error llega
  como evento IPC, no como fallo silencioso.
- Comando `spotify_policy_status` expone el presupuesto a la UI.

### Anuncios
librespot **no gestiona anuncios** (`ads_supported: false` en la API y aviso en
el panel Spotify). No se sintetizan anuncios locales: sería falsificar contenido
de Spotify y no aporta nada. Es otra razón documentada por la que Free no es
compatible con este motor.

### Límites honestos de los tests Free
Con librespot 0.8.0 es IMPOSIBLE "autenticación exitosa + reproducción inicial
con Free": `check_catalogue` ejecuta `exit(1)` al ver `type != premium`. Los
tests cubren lo que sí existe: gate que deniega Free con mensaje claro,
clasificación del `Premium account required` del servidor, presupuesto de saltos
(6/hora, renovación por ventana, retry), bajo-demanda denegado y serialización
del evento IPC para la UI.

## 10. Incidencias corregidas (2026-10-11)

**Deadlock tras el primer play (síntoma: botones muertos, barra congelada,
pero el audio seguía sonando).** Causa: `play_current` retenía el lock del
router (`Mutex<PlaybackRouter>`, no reentrante) mientras llamaba a
`integrations::sync_playback`, que lo bloqueaba de nuevo. Desde ese momento
ningún comando con lock del router respondía (toggle, next/prev, seek,
volumen, status → el tick de 500ms tampoco actualizaba la barra).
Fix: soltar el guard antes de sincronizar MPRIS (`sync_playback_state` con el
booleano ya leído). Regla: **nunca retener el lock del router a través de una
llamada que pueda re-bloquearlo**.
Mejoras del mismo pase: `toggle()` local lee el estado vivo de mpv (evita
desincronizar el flag si mpv termina solo) y vaciar la cola olvida el track
actual del router. Cobertura: `player::tests::reproduce_tono_local` (audio
real: play/pause/seek/volumen/stop) + `botones_concurrentes_no_bloquean_router`.

## 8. Cómo se prueba esto sin cuenta Premium

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # 42 tests, 0 red
cargo check --manifest-path src-tauri/Cargo.toml  # gate de compilación
```

El panel Spotify de la UI muestra el error claro de gate si se prueba con una
cuenta Free (sin riesgo de `exit(1)` porque librespot nunca llega a iniciar).

## 11. Cuadros blancos, thumb, auto-avance y modal (2026-10-11)

- **Cuadros blancos (selects/inputs):** la página no declaraba `color-scheme` y
  los `select` conservaban `appearance` nativa: WebKitGTK pintaba la cara con el
  tema claro del sistema ignorando el fondo, pero sí aplicaba nuestro texto
  crema → ilegible. Fix: `color-scheme` por tema + `appearance: none` con
  flecha propia (`▾` en variable) y `option` tematizados.
- **Thumb desalineado:** el `margin-top` compensaba pista de 6px pero en hover
  la pista crece a 9px (1.5px de desfase). Ahora el margen acompaña a la altura
  por estado (6px→-4.5px, 9px→-3px) y el input lleva padding lateral para que el
  thumb no sobresalga de los extremos.
- **Auto-avance:** nada observaba el fin (causa del BUG 3). Tras el EOF mpv queda
  idle con `path=None` y `eof-reached` NO se latchea (verificado con sonda), así
  que el router latchea el flanco "había track → vacío inesperado"
  (`refresh_eof`/`take_eof`, con `idle_expected` para stop/cambios de motor) y el
  comando atómico `player_track_ended` avanza sin envolver (la última pausa).
  El siguiente manual sigue circular. Sin modos aleatorio/repetir locales que
  respetar (solo existen en Spotify vía Spirc).
- **Modal de portada:** se abre al empezar cada canción (y al clicar la
  carátula), cierre con × o Esc, sin tapar la franja del reproductor;
  placeholder ♪ si no hay carátula; todo en variables y con reduced-motion.

## 12. Ronda UI crema: modal, ondas, iconos, playlists, paneles (2026-10-11)

- **Modal que no reabría:** la apertura vivía solo en el evento de cambio de
  track; al cerrar y reproducir el MISMO tema no hay evento y no reaparecía.
  Ahora también se abre en cada intención de play con éxito (`playAt`,
  `playQueueAt`, `doToggle` con retorno true). Animación con transiciones
  Svelte (fade 300ms / scale 340ms, solo transform+opacity, duraciones a 0 con
  `prefers-reduced-motion`).
- **Ondas:** SVG de 3 anillos punteados contrarrotantes + pulsación, solo
  transform/opacity, pausados y atenuados en pausa. SIMULADAS: libmpv no expone
  FFT ni espectro en su API cliente, así que no reaccionan al audio real.
- **Iconos:** set propio `src/lib/Icon.svelte` dibujado a mano (20 glifos,
  trazo redondeado, `currentColor`); sin dependencias ni licencias de terceros.
- **Playlists:** `<datos-app>/playlists/<Nombre>/playlist.m3u8` (EXTM3U con
  rutas absolutas). Referencias en vez de copias (disco) o symlinks (Windows).
  Cargar una playlist la pone en la cola: convive con auto-avance y MPRIS.
- **Fuentes ocultas / cola minimizable:** colapsable con grid-rows animado +
  clic-fuera; cola con riel de 46px, `inert` al minimizar y estado en
  localStorage (`musica.queueMin`).

## 13. Ronda UI: ondas canvas, kawaii, cola/lista compactas, playlists usables (2026-10-11)

- **Modal que no reabría:** la apertura vivía solo en el evento de cambio de
  track; reproducir el MISMO tema no emite ese evento. Ahora también se abre en
  cada play con éxito (`playAt`, `playQueueAt`, `doToggle` con retorno).
- **Ondas:** canvas + rAF con ruido suave (senos inconmensurables + envolvente
  de pulso); SIMULADAS porque libmpv no expone FFT. En pausa se congela y
  atenúa (sin CPU); RM dibuja un fotograma estático.
- **Kawaii:** `src/lib/Kawaii.svelte` dibujado a mano (misma chica, expresiones
  por acción), sin dependencias. En play/pausa/anterior/siguiente,
  volumen/mute, agregar, quitar, cola, playlist y placeholder.
- **Playlists usables:** la causa era doble: sin cola no pasaba nada (retorno
  silencioso) y no había cómo añadir canciones sueltas. Ahora hay menú por
  canción, arrastre a la plistbar, agregar carpeta (con tags reales vía
  `library::read_tags`) y agregar archivos, más estado vacío con guía.
  Estructura en disco sin cambios: `<datos-app>/playlists/<Nombre>/playlist.m3u8`.

## 14. Ronda UI: kawaii por archivos, clic/doble-clic, popovers, dropdown, punteros (2026-10-11)

- **Kawaii iguales:** el componente único compartía una cara base y solo
  variaban ojos/boca (ilegibles a 20px). Ahora cada icono es un archivo en
  `src/assets/icons/kawaii/<nombre>.svg` (símbolo grande + carita/accesorio),
  cargados con `?raw` + `{@html}` (heredan currentColor y variables). Cambiar
  el set = sustituir archivos. Dibujos propios, sin licencias.
- **Clic/doble clic:** el primer clic abría el modal y el segundo caía sobre él,
  así que el doble clic nunca se completaba. El clic espera 220ms y se cancela
  si llega el doble clic (que reproduce). Vista previa (preview) separada de
  reproducción (nowPlaying): el modal muestra la clickeada sin reiniciar nada.
- **Menú mezclado:** vivía dentro de la fila (wrapper de 44px con transform) y
  se recortaba/transparentaba con las filas de abajo. Ahora es popover fijo con
  z-index alto, fondo sólido, anclado al botón con flip hacia arriba, toast de
  confirmación y contadores actualizados.
- **Dropdown propio:** reemplaza al select nativo (fondo gris + popup verde del
  sistema) con secciones Biblioteca/Playlists/Fuentes, hover suave y flip.
  Atajos en una línea con tooltip nativo.
- **Cola con puntero:** el DnD HTML5 falla en webview/Wayland. Reordenar ahora
  usa pointerdown en el asa + move/up en ventana, indicador de inserción
  (incluido al final), `animate:flip` al reordenar y clic suprimido tras
  arrastrar. Botones subir/bajar intactos; el orden mueve el cursor real.

## 15. Estado confirmado por eventos mpv + icono determinista (2026-10-11)

- **"Sonando" sin sonar:** `loadfile` es asíncrono y sus fallos (AO, formato)
  llegaban tarde y en silencio; la UI marcaba playing de forma optimista.
  `Player` ahora drena `wait_event` en un hilo con handle propio
  (`create_client`), registra StartFile/FileLoaded/PlaybackRestart/EndFile+motivo
  y expone `confirmed` + `error` en el estado. Sin confirmación en ~3s reintenta
  UNA vez; si no, publica el error real en vez de quedarse mudo. La UI muestra
  "Cargando…" hasta la confirmación. El router no latchea EOF durante la carga
  (`is_loading`). Nota: libmpv2 6.0 no exporta `Format`, así que
  `observe_property` no es llamable desde fuera (los eventos de ciclo de vida
  llegan igual; las props se leen en vivo).
- **Verbose:** `MUSICA_DEBUG=1` vuelca eventos/propiedades mpv; los errores
  siempre se publican.
- **Icono de volumen vacío:** el `import.meta.glob` dinámico no garantizaba
  contenido en el primer frame del dev-server. `Kawaii.svelte` ahora usa 17
  imports estáticos `?raw`: determinista en dev y build.

## 16. Estado por eventos mpv, un clic reproduce, iconos sólidos (2026-10-11)

- **"Sonando" sin sonar:** `loadfile` es asíncrono y sus fallos llegaban tarde y
  en silencio mientras la UI marcaba playing optimista. `Player` drena
  `wait_event` en un hilo con handle propio y expone `confirmed` + `error`;
  watchdog con UN reintento a los 3s y error visible si no; "Cargando…" en la UI
  hasta `PlaybackRestart`. Sondas headless: cold-play, doble-load y martilleo
  de status pasan; archivo corrupto publica su error.
- **Un clic reproduce:** fuera el retardo de 220ms y el doble clic; clic en la
  que suena solo abre el cuadro (sin reiniciar); textos actualizados.
- **Iconos:** transporte en sólido `currentColor` (las caritas interiores se
  fundían a 18px y la pausa parecía "U"); volumen con imports estáticos `?raw`
  (determinista en dev y build).

## 17. Ronda YT: medición de transiciones, sesión y navegador (2026-10-12)

- **Transiciones lentas:** medido headless (32 URLs distintas, con y sin
  cookies): 1.4–1.9s estables, sin crecimiento y sin procesos colgados. El
  coste no está en yt-dlp plano sino en la amplificación de fallos (timeouts
  de 25s × hasta 6 saltos) y el throttling. Fixes: `ytdl=no` en mpv (cero
  doble resolución), dedup de resoluciones en vuelo, timeout de resolución a
  15s, pausa+aviso ante 429/bot en vez de quemar intentos, precarga existente
  verificada, logs de tiempo con MUSICA_DEBUG. Tabla 10 transiciones reales:
  resolve 1.6–2.1s, confirmado 2.1–3.5s, sin crecimiento.
- **LM "does not exist":** formato válido; es falta de sesión. Importación con
  mensajes amables (sin sesión / expirada) y entradas Liked Music + Liked.
- **Brave sin DB:** el usuario usa Beta/Nightly y yt-dlp `brave` solo mira el
  estable vacío. Autodetección de navegadores+perfiles (incl. Flatpak/Snap),
  sintaxis `navegador:/ruta`, importar cookies.txt, errores traducidos,
  cookies con 0600.
- **Icono volumen vacío:** `volName` devolvía `"vol"`/`"plus"` pero las claves
  eran `"volume"`/`"add"`: varios botones quedaban vacíos en silencio. Alias +
  aviso en consola + auditoría exhaustiva (ningún nombre sin clave).

## 18. Ronda YT grave: transiciones, sesión, navegador, icono (2026-10-12)

- **Transiciones 30-60s:** medido headless, 32 URLs distintas con y sin
  cookies: 1.4–1.9s estables, sin crecimiento ni zombies. El coste está en
  amplificar fallos (timeout 25s × 6 saltos). Fixes: `ytdl=no` en mpv,
  dedup en vuelo, timeout de resolución a 15s, pausa+aviso ante 429/bot en vez
  de quemar intentos, cadena plano→cookies→android, logs de tiempo con
  MUSICA_DEBUG. Tabla 10 transiciones: resolve 1.6–2.1s, confirmado 2.1–3.5s.
- **LM "does not exist":** formato válido; es falta de sesión. Mensajes amables
  (conectar / expirada) y entradas Liked-Music + Liked. Cookies 0600.
- **Brave sin DB:** el equipo usa Beta/Nightly y yt-dlp `brave` solo mira el
  estable. Ahora nombre-base + ruta (`brave:/ruta`), autodetección
  (estable/Beta/Nightly/Flatpak/Snap/Firefox/Chromium), importar cookies.txt,
  errores traducidos. Nunca contraseñas.
- **Volumen vacío:** `volName`→`"vol"` y botones→`"plus"` no existían como
  claves (`"volume"`/`"add"`): quedaban vacíos en silencio. Alias + aviso en
  consola + auditoría total (cero nombres sin clave).

## 19. Estabilidad YT: generaciones, slots, throttle, navegador real (2026-10-12)

- **Traba/cierre al cambiar rápido:** cada clic lanzaba yt-dlp sin cancelar el
  anterior (hilos y procesos sin cota) y la respuesta vieja pisaba a la actual.
  Ahora: contador de generación por play (`bump_gen`); el hijo muere al quedar
  superado (STALE silencioso); precarga de 3 en un solo hilo que aborta al
  saltar; registro de hijos con `kill_all` al cerrar. Tormenta de 8 concurrentes:
  7 abortadas, 1 completa en 1.6s, 0 hijos vivos. mpv aguanta 20 loadfile
  seguidos y suena la última.
- **Transiciones:** medidas 1.4–1.9s estables (sin/con cookies); tabla de 10
  transiciones reales 1.6–3.5s. `ytdl=no` en mpv, timeout de resolución a 15s,
  cadena plano→cookies→android, pausa+aviso ante 429/bot.
- **Brave sin DB:** el equipo usa Beta/Nightly; yt-dlp `brave` solo mira el
  estable. Nombre-base + ruta (`brave:/ruta`), autodetección (estable/Beta/
  Nightly/Flatpak/Snap/Firefox/Chromium), importar cookies.txt, errores
  traducidos, cookies 0600. Sin contraseñas.
- **Volumen vacío:** `vol`/`plus` no existían como claves (`volume`/`add`):
  botones vacíos en silencio. Alias + aviso en consola + auditoría total.

## 20. Ronda YT urgente: throttling, navegador real, icono (2026-10-12)

- **Lentitud cada 3-4 canciones:** medido con curl que las URLs del cliente
  web/VISIONOS descargan a ~32 KB/s (throttling ~2x) en IPv4 e IPv6, mientras
  que las del cliente android van a ~10 MB/s (`ratebypass=yes`). Causa raíz
  confirmada. Ahora la cadena es android primero, web+cookies solo ante
  edad/sign-in, web plano como último recurso; `ytdl=no` en mpv; timeout de
  resolución a 15s; opciones de red en mpv (timeout, reconnect lavf, readahead).
  Tabla 10 transiciones: peor caso 7.7s → 3.5s, resto ~2-3s.
- **Traba al cambiar rápido:** generación por play (la última gana, mata al
  hijo superado), precarga de 3 en un hilo que aborta al saltar, registro de
  hijos con kill al cerrar, sin doble avance (latch consumido una vez).
- **Brave sin DB:** era Beta/Nightly vs estable vacío. Nombre-base + ruta,
  autodetección, importar cookies.txt, errores traducidos, 0600.
- **Volumen vacío:** `vol`/`plus` no eran claves (`volume`/`add`). Alias +
  aviso + auditoría total.
