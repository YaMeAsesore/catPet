<script lang="ts">
  // Vista MVP: buscar, reproducir locales, cola con drag&drop, atajos y MPRIS/SMTC.
  // Decisión: un solo componente porque el MVP tiene 2 vistas y evita router.
  import { onMount, tick } from "svelte";
import { cropBlackBars } from "./lib/cropBlackBars";
  import { open } from "@tauri-apps/plugin-dialog";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { fade, scale } from "svelte/transition";
  import VirtualList from "./lib/VirtualList.svelte";
  import Waves from "./lib/Waves.svelte";
  import { runWarm } from "./lib/csvwarm";
  // U1: armazón nuevo (cola y reproductor como componentes; tras musica.ui2).
  import Queue from "./lib/components/Queue.svelte";
  import PlayerBar from "./lib/components/PlayerBar.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import SettingsDialog from "./lib/components/SettingsDialog.svelte";
  import AddDialog from "./lib/components/AddDialog.svelte";
  // M2: barra de menú 1-bit (menús + búsqueda + estado) y ventana de atajos.
  import MenuBar from "./lib/components/MenuBar.svelte";
  import ShortcutsDialog from "./lib/components/ShortcutsDialog.svelte";
  import AboutDialog from "./lib/components/AboutDialog.svelte";
  // V1/V2: cabeceras contextuales (solo presentación; estado y handlers aquí).
  // M3: los cinco contextos completos, con sus controles re-hogarizados.
  import CtxMusica from "./lib/components/contexts/CtxMusica.svelte";
  import CtxYoutube from "./lib/components/contexts/CtxYoutube.svelte";
  import CtxRadios from "./lib/components/contexts/CtxRadios.svelte";
  import CtxServidores from "./lib/components/contexts/CtxServidores.svelte";
  import CtxSpotify from "./lib/components/contexts/CtxSpotify.svelte";
  // P6: vista de recomendaciones del home de YouTube Music.
  import CtxReco from "./lib/components/contexts/CtxReco.svelte";
  import RecoView from "./lib/components/RecoView.svelte";
  // M2/M3: adaptadores para el marcado COMPARTIDO (filas, estados vacíos,
  // avisos y diálogos del CSV): con pixel=true pintan el set 1-bit.
  import IcoAdapt from "./lib/components/onebit/IcoAdapt.svelte";
  import KawaiiAdapt from "./lib/components/onebit/KawaiiAdapt.svelte";
  // M4: ventana modal 1-bit (scrim + Window + × + Esc) y botón base.
  import ObModal from "./lib/components/onebit/ObModal.svelte";
  import Btn from "./lib/components/onebit/Btn.svelte";
  import Popover from "./lib/components/Popover.svelte";
  import DlPanel from "./lib/components/DlPanel.svelte";
  import ConfirmDialog from "./lib/components/ConfirmDialog.svelte";
  import NameDialog from "./lib/components/NameDialog.svelte";
  import {
    api, fmtSecs, type Source, type Track, type SpotifyAccountInfo, type TrackAttribution,
    type PlaybackStateEvent, type TrackChangedEvent, type RestrictionEvent, type UnifiedStatus, type QueueEvent,
    type PlaylistInfo, type CsvPreview, type CsvTrack, type CsvMatchResult, type YtTrack, type VariantRow, type YtMine, type YtAccount, type YtSessionCheck, type YtLoginEvent, type BrowserProfile, type YtBatchSnapshot,
    type RecoFeed, type RecoError, type RecoItem,
    type DownloadEnv, type DownloadSnapshot, type DownloadOptions,
  } from "./lib/tauri";
  import { loadDlSettings, saveDlSettings, effectiveFolder, type DlSettings, type DlFormat } from "./lib/dlsettings";
  // Portadas de la app (también usadas en README): la oscura con el gato
  // geométrico y la clara con el personaje. Se elige según el tema.
  import portadaOscura from "./assets/brand/portada-oscura.svg";
  import portadaClara from "./assets/brand/portada-clara.svg";

  // Movimiento reducido también para las transiciones JS (las CSS ya las cubre
  // la media query global): duraciones a 0 cuando el usuario lo pide.
  const RM = typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  const PAGE = 300; // mismo tope que el backend; el scroll infinito pagina con OFFSET
  // T5: la carga inicial trae 100 (el resto por scroll): menos IPC/retenido
  // de golpe y la ventana pinta antes (el pico 260→405 MB era la lista entera).
  const INIT_PAGE = 100;

  let query = $state("");
  let tracks = $state<Track[]>([]);
  let total = $state(0);
  let qItems = $state<Track[]>([]);
  let qCurrent = $state<number | null>(null);
  let paused = $state(true);
  let pos = $state(0);
  let dur = $state(0);
  let vol = $state(80);
  let searchEl: HTMLInputElement | null = $state(null);
  let debounce: number = 0;
  let err = $state(""); // último error visible (antes: promesas rechazadas mudas)
  let loadingMore = $state(false);
  let endReached = $state(false);
  // Evita enseñar la portada de bienvenida durante el primer refresh.
  let booted = $state(false);
  // Fase 1: fuente seleccionada (null = biblioteca local) y formulario de alta.
  let sourceId = $state<number | null>(null);
  let sources = $state<Source[]>([]);
  let fKind = $state("subsonic");
  let fName = $state("");
  let fUrl = $state("");
  let fUser = $state("");
  let fSecret = $state("");
  // Motor de audio unificado (libmpv local | librespot Spotify Connect).
  let engine = $state<"libmpv" | "spotify">("libmpv");
  let spotify = $state<SpotifyAccountInfo | null>(null);
  let pkceToken = $state("");
  // Atribución visible del track actual (eventos unificados del router).
  let nowPlaying = $state<TrackAttribution | null>(null);
  // Borrador de seek: mientras se arrastra, el tick de 500ms no debe mover el
  // deslizador (antes la barra "peleaba" y saltaba sola durante el arrastre).
  let seekDraft = $state<number | null>(null);
  // Arrastrar el progreso muy rápido enviaba un `player_seek` por píxel
  // (cientos de IPC/bloqueos de mpv y tirones). Se envía como máximo uno cada
  // 180 ms y siempre el valor final al soltar.
  let seekSendTimer = 0;
  let seekPending: number | null = null;
  function seekInput(v: number) {
    seekDraft = v;
    seekPending = v;
    if (seekSendTimer) return;
    seekSendTimer = window.setTimeout(() => {
      seekSendTimer = 0;
      const target = seekPending;
      seekPending = null;
      if (target != null) api.seek(target).catch(showErr);
    }, 180);
  }
  function seekDone() {
    seekDraft = null;
    if (seekSendTimer) {
      window.clearTimeout(seekSendTimer);
      seekSendTimer = 0;
    }
    const target = seekPending;
    seekPending = null;
    if (target != null) api.seek(target).catch(showErr);
  }
  function cancelSeek() {
    if (seekSendTimer) {
      window.clearTimeout(seekSendTimer);
      seekSendTimer = 0;
    }
    seekPending = null;
    seekDraft = null;
  }

  // ---- Volumen: icono por nivel + silencio (solo frontend, usa api.volume) ----
  let lastVol = $state(80);
  // Arrastrar volumen tampoco debe enviar un IPC por píxel.
  let volumeSendTimer = 0;
  let volumePending: number | null = null;
  function volumeInput(v: number) {
    vol = v;
    volumePending = v;
    if (volumeSendTimer) return;
    volumeSendTimer = window.setTimeout(() => {
      volumeSendTimer = 0;
      const target = volumePending;
      volumePending = null;
      if (target != null) api.volume(target).catch(showErr);
    }, 80);
  }
  function cancelVolume() {
    if (volumeSendTimer) {
      window.clearTimeout(volumeSendTimer);
      volumeSendTimer = 0;
    }
    volumePending = null;
  }
  // MEJORA 3: chica kawaii cantando / calladita en vez de emojis.
  const volName = $derived(vol === 0 ? "mute" : "vol");
  async function toggleMute() {
    try {
      if (vol === 0) {
        const v = lastVol > 0 ? lastVol : 80;
        await api.volume(v);
        vol = v;
      } else {
        lastVol = vol;
        await api.volume(0);
        vol = 0;
      }
    } catch (e) { showErr(e); }
  }

  // Estado "cargando": hay intención de sonar pero mpv no lo confirma.
  // El estado de reproducción se deriva de eventos mpv, nunca del clic.
  let loading = $state(false);
  let lastPlayerError: string | null = null;
  let confirmedState = $state(false);
  // URI con reintento ya consumido (un solo reintento por pista).
  let retryUri: string | null = null;
  // Pistas marcadas no disponibles (gris + motivo) por el auto-avance.
  let dead = $state<Record<string, string>>({});
  // Última generación aplicada (se ignoran eventos viejos que llegan tarde).
  let lastGen = $state(0);
  // Fila en "cargando" tras un clic (el resaltado de sonar lo pone el backend).
  let pendingPath = $state<string | null>(null);
  // Pistas de YouTube ya resueltas (indicador "listo" en filas).
  let ready = $state<Record<string, boolean>>({});
  // Suena de verdad solo si mpv lo confirmó: el botón nunca muestra pausa en falso.
  const sounding = $derived(!paused && confirmedState);

  // Relleno de la barra de progreso (0–100) para el degradado CSS.
  const posShown = $derived(seekDraft ?? pos);
  const progPct = $derived(dur > 0 ? Math.min(100, (posShown / Math.max(dur, 1)) * 100) : 0);
  // Aviso de restricción de Spotify (proxy de política: p. ej. sin saltos).
  let spotNotice = $state<RestrictionEvent | null>(null);
  let spotNoticeTimer: number = 0;
  // Modal de portada: se abre al empezar cada canción; no tapa el footer.
  let showCover = $state(false);
  let lastCoverUri: string | null = null;
  // MEJORA 3: menú "Agregar a playlist" por canción + arrastre a la plistbar.
  // BUG 3: el menú vivía dentro de la fila (wrapper de 44px con transform) y se
  // mezclaba con las filas de abajo. Ahora es un popover fijo anclado al botón
  // (fuera de la lista): la fila no se expande ni mueve nada.
  let menu = $state<{ path: string; track: Track; x: number; y: number; flip: boolean; vh: number } | null>(null);
  let pendingAdd = $state<Track | null>(null);
  let dropIndex = $state<number | null>(null);
  // MEJORA 6: cola minimizable con estado persistido entre sesiones.
  let queueMin = $state(
    typeof localStorage !== "undefined" && localStorage.getItem("musica.queueMin") === "1",
  );
  // M5: la interfaz clásica se retiró. La piel «Mac 1-bit» (`body.onebit`) y
  // la vista contextual (filas de 56 px + cabecera por contexto) van SIEMPRE
  // activas; las claves viejas `musica.ui2`/`musica.uiContextos` se ignoran y
  // se descartan una sola vez (ver el efecto de abajo). No hay migración rota:
  // ningún otro dato depende de ellas.
  // U1: diálogo de Ajustes (armazón; el contenido llega en fases siguientes).
  let settingsOpen = $state(false);
  // M2: ventana «Atajos de teclado» (menú Ayuda) y segundos del aviso de
  // límite de YouTube que muestra la barra de menú (solo interfaz nueva).
  let shortcutsOpen = $state(false);
  let aboutOpen = $state(false);
  let ytLimitSecs = $state(0);
  // U4: asistente «+ Añadir» (interfaz nueva).
  let addOpen = $state(false);
  let addStart = $state<string | null>(null); // tarjeta inicial («youtube» desde «Pegar link»)
  let settingsFocus = $state<string | null>(null); // sección de Ajustes a la que saltar
  // U3: «Mostrar secciones» (clave nueva `musica.ui.sections`; por defecto,
  // todas visibles). Solo presentación: oculta secciones de la barra lateral.
  type UiSections = { musica: boolean; youtube: boolean; radios: boolean; servers: boolean; spotify: boolean };
  function loadUiSections(): UiSections {
    const def: UiSections = { musica: true, youtube: true, radios: true, servers: true, spotify: true };
    try {
      const raw = localStorage.getItem("musica.ui.sections");
      if (raw != null) {
        const o = JSON.parse(raw) as Partial<UiSections>;
        return {
          musica: o.musica !== false, youtube: o.youtube !== false, radios: o.radios !== false,
          servers: o.servers !== false, spotify: o.spotify !== false,
        };
      }
    } catch { /* sin almacenamiento o JSON inválido: por defecto */ }
    return def;
  }
  let uiSections = $state<UiSections>(loadUiSections());
  function setUiSection(k: keyof UiSections, v: boolean) {
    uiSections = { ...uiSections, [k]: v };
    try {
      localStorage.setItem("musica.ui.sections", JSON.stringify(uiSections));
    } catch { /* sin almacenamiento: no bloquea */ }
  }
  // U2: gestión de fuentes (renombrar/borrar) — misma ruta para la barra
  // lateral y para el botón clásico. Del borrado pendiente solo se persiste
  // el id (una clave nueva; al arrancar se confirma si quedó a medias).
  let srcRename = $state<{ id: number; name: string } | null>(null);
  let srcConfirmDel = $state<Source | null>(null);
  let undoDel = $state<{ id: number; name: string } | null>(null);
  let plistDeleteAsk = $state<string | null>(null);
  // U6a.1: renombrar playlist con NameDialog (la UI nueva oculta la fila).
  let plistRename = $state<string | null>(null);
  let undoTimer: number | null = null;
  // Descargas (D4): ajustes en localStorage (mismo mecanismo que queueMin).
  let dl = $state<DlSettings>(loadDlSettings());
  let dlEnv = $state<DownloadEnv | null>(null);
  // MEJORA 4: playlists locales (null = vista biblioteca).
  let plists = $state<PlaylistInfo[]>([]);
  let plistName = $state<string | null>(null);
  // YouTube Music: vista explícita de la importación (null = vista normal).
  let ytImport = $state<{ title: string; url: string; tracks: Track[] } | null>(null);
  // W0/W4: cargador por lotes. `ytImport.tracks` SOLO contiene las filas ya
  // mostradas (lotes procesados): la cola, "Reproducir todo" y el auto-avance
  // nunca ven pistas fuera de lo mostrado (regla 10).
  let ytBatch = $state<YtBatchSnapshot | null>(null);
  let ytBatchTimer = 0;
  /// Override optimista de «Guardar playlist» mientras el backend responde y
  /// para que un snapshot de progreso en vuelo no revierta el icono.
  let ytSavedOverride = $state<boolean | null>(null);
  let ytPlaylistSaving = $state(false);
  /// ¿La cola actual salió de esta vista de YouTube? Permite ampliarla al
  /// revelar más lotes sin tocar una cola ajena (p. ej. de una playlist local).
  let ytQueueActive = $state(false);
  // P6: vista «Recomendaciones». El backend cachea 10 min en memoria; aquí
  // solo se conserva la última carga para no vaciar la pantalla si falla un
  // refresco. Los errores del IPC ya vienen tipados y sin secretos.
  let recoOpen = $state(false);
  let recoFeed = $state<RecoFeed | null>(null);
  let recoLoading = $state(false);
  let recoError = $state<RecoError | null>(null);
  let recoSeq = 0;
  // Marcas por fila del propio lote: "sin preparar" (punto tenue).
  let unprepared = $state<Record<string, boolean>>({});
  // Corazón de YouTube Music por path. Se rellena con el estado que entrega
  // InnerTube al listar; si es desconocido, el primer clic lo consulta.
  let liked = $state<Record<string, boolean>>({});
  let likeBusy = $state<Record<string, boolean>>({});
  // Portada de estreno: solo con la biblioteca realmente vacía (sin búsqueda,
  // playlist, fuente, YouTube ni recomendaciones), nunca sobre resultados.
  const showWelcome = $derived(
    booted && total === 0 && query.trim() === "" && plistName == null &&
    sourceId == null && !recoOpen && ytImport == null && !(ytBatch?.open ?? false),
  );
  /// Destino del desplegable: biblioteca, playlist o fuente ya agregada.
  function selectDropTarget(target: string) {
    forgetYtView(); // cualquier vista explícita sale de la importación (y cancela su lote)
    recoOpen = false;
    if (target === "library") {
      plistName = null;
      pickSource("local");
    } else if (target.startsWith("playlist:")) {
      sourceId = null;
      plistName = target.slice("playlist:".length);
      refresh().catch(showErr);
    } else if (target.startsWith("source:")) {
      plistName = null;
      pickSource(target.slice("source:".length));
    } else if (target.startsWith("yt:")) {
      plistName = null;
      sourceId = null;
      ytLink = target.slice("yt:".length);
      importYtLink(false);
    }
  }
  let plistForm = $state<"new" | "rename" | null>(null);
  let plistDraft = $state("");
  // Tema actual para el icono sol/luna.
  let themeDark = $state(false);
  // Portada acorde al tema: oscura (gato) o clara (personaje).
  const portada = $derived(themeDark ? portadaOscura : portadaClara);

  const cur: Track | null = $derived(qCurrent != null ? qItems[qCurrent] ?? null : null);
  // G4: qué pista DEBE mostrar la interfaz. Tras "siguiente" la cola ya apunta
  // al destino (evento en <100 ms) pero playback://track llega tras resolver;
  // mientras tanto manda el DESTINO PENDIENTE, y al confirmarse manda
  // nowPlaying. Así fila resaltada, barra inferior y portada no se quedan
  // una pista atrás esperando a la resolución.
  const shownTrack = $derived.by<{
    uri: string; title: string; artist: string; album: string; art_url: string;
  } | null>(() => {
    const p =
      pendingPath != null
        ? qItems.find((t) => t.path === pendingPath) ??
          tracks.find((t) => t.path === pendingPath)
        : undefined;
    if (p) return { uri: p.path, title: p.title, artist: p.artist, album: p.album, art_url: p.art_url };
    if (nowPlaying)
      return {
        uri: nowPlaying.uri, title: nowPlaying.title, artist: nowPlaying.artist,
        album: nowPlaying.album, art_url: nowPlaying.art_url,
      };
    if (cur) return { uri: cur.path, title: cur.title, artist: cur.artist, album: cur.album, art_url: cur.art_url };
    return null;
  });

  function showErr(e: unknown) {
    err = e instanceof Error ? e.message : String(e);
  }

  /// ¿El fallo es "no disponible" permanente? (gris + toast, no error duro).
  function unavailableReason(e: unknown): string | null {
    const m = (e instanceof Error ? e.message : String(e)).toLowerCase();
    if (["unavailable", "not available", "no disponible", "privad", "elimin", "bloque", "region", "país"].some((k) => m.includes(k))) {
      return e instanceof Error ? e.message : String(e);
    }
    return null;
  }

  let refreshSeq = 0; // ignora respuestas viejas si hay refresh() solapados
  // Última versión ESTRUCTURAL de la cola recibida del backend. Si un evento
  // solo mueve el cursor (misma rev), no se reenvían las 10 000 filas.
  let queueRev = -1;

  /// Solo estado del reproductor (IPC diminuto). Se usa tras acciones que no
  /// cambian la lista y para no reenviar la cola entera en cada clic.
  async function refreshPlayer() {
    const st = await api.status();
    paused = st.paused; pos = st.pos_secs; dur = st.dur_secs; vol = st.volume;
  }

  /// Carga inicial/ligera de la cola y el estado. Los cambios posteriores
  /// llegan por `playback://queue`, así que no hace falta clonar la cola en
  /// cada refresco (con 10 000 pistas eso era un payload enorme por clic).
  async function loadQueueState() {
    const [q, st] = await Promise.all([api.queueList(), api.status()]);
    // Una respuesta vieja no debe pisar una cola más nueva llegada por evento.
    if (typeof q.rev === "number") {
      if (q.rev < queueRev) return;
      queueRev = q.rev;
    }
    qItems = q.items; qCurrent = q.current;
    // Sin cola no hay track que mostrar: olvida la atribución vieja (antes el
    // footer seguía mostrando el último tema tras Limpiar).
    if (q.items.length === 0) { nowPlaying = null; lastCoverUri = null; }
    paused = st.paused; pos = st.pos_secs; dur = st.dur_secs; vol = st.volume;
  }

  async function refresh(keepList = false) {
    // P4-02: las acciones de cola/transporte no deben tirar las páginas ya
    // cargadas (p. ej. 20 000 filas) ni volver a pedir la lista/cola entera.
    // La cola ya se actualiza por evento y el estado por refreshPlayer().
    if (keepList) {
      await refreshPlayer();
      return;
    }
    const my = ++refreshSeq;
    const alive = () => my === refreshSeq;
    // Vista de importación YouTube (si hay) > playlist > fuente remota > local.
    const found = ytImport != null
      ? ytImport.tracks
      : plistName != null
        ? await api.playlistTracks(plistName).catch((e) => {
            // La playlist pudo borrarse fuera: vuelve a biblioteca.
            plistName = null;
            loadPlists();
            throw e;
          })
        : sourceId == null
          ? await api.search(query, INIT_PAGE, 0)
          : await api.remoteSearch(sourceId, query);
    if (!alive()) return;
    tracks = found;
    endReached = ytImport != null || sourceId != null || plistName != null || found.length < INIT_PAGE;
    const paths = found
      .filter((t) => t.path.startsWith("ytmusic://"))
      .map((t) => t.path);
    // Las llamadas independientes van EN PARALELO: antes se encadenaban 5
    // IPC (~búsqueda + estados + stats + cola + player) por cada tecla.
    const [indicators, stats] = await Promise.all([
      paths.length > 0
        ? Promise.all([api.ytCached(paths), api.ytDead(paths)]).catch(() => null)
        : Promise.resolve(null),
      api.stats(),
    ]);
    if (!alive()) return;
    total = stats;
    if (indicators != null) {
      const [flags, motivos] = indicators;
      const m: Record<string, boolean> = {};
      paths.forEach((p, k) => { m[p] = flags[k] ?? false; });
      ready = m;
      // Marcas de no disponible (caché negativa): gris + tooltip sin
      // intentar nada. Se podan las de vistas viejas para no acumular.
      const keep = new Set([...paths, ...qItems.map((t) => t.path)]);
      for (const k of Object.keys(dead)) if (!keep.has(k)) delete dead[k];
      paths.forEach((p, k) => { const r = motivos[k]; if (r) dead[p] = r; });
    } else if (Object.keys(ready).length > 0) {
      ready = {};
    }
  }

  // G4: actualización LIGERA de la cola (un solo IPC, sin repintar la lista).
  // El backend avisa aquí cuando cambia el índice actual (siguiente/anterior/
  // eof/clic), antes de resolver la pista: en <100 ms la cola resalta el
  // destino y, si no es lo que suena, se marca como "cargando" para que fila,
  // barra inferior y portada lo muestren ya.
  let queueSeq = 0;
  function applyQueueCursor(current: number | null) {
    qCurrent = current;
    const t = current != null ? qItems[current] : undefined;
    if (t && t.path !== nowPlaying?.uri) pendingPath = t.path;
    else if (!t) pendingPath = null;
    if (qItems.length === 0) {
      nowPlaying = null;
      lastCoverUri = null;
      pendingPath = null;
    }
  }

  async function onQueueChanged(ev?: QueueEvent) {
    // Camino rápido: la estructura no cambió (solo siguiente/anterior/eof).
    // Actualiza el índice con el payload del evento, sin pedir la cola entera.
    if (
      ev != null && typeof ev.rev === "number" &&
      ev.rev === queueRev && ev.length === qItems.length
    ) {
      applyQueueCursor(ev.current);
      return;
    }
    const my = ++queueSeq;
    try {
      const q = await api.queueList();
      if (my !== queueSeq) return; // llegaron eventos más nuevos
      qItems = q.items;
      if (typeof q.rev === "number") queueRev = q.rev;
      applyQueueCursor(q.current);
    } catch { /* cola opcional: sin ruido */ }
  }

  // Scroll infinito: añade la siguiente página sin perder la posición.
  // Decisión: solo la fuente local pagina (remota y playlists vienen completas).
  async function loadMore() {
    if (loadingMore || endReached || tracks.length === 0 || sourceId != null || plistName != null) return;
    const q = query;
    const off = tracks.length;
    loadingMore = true;
    try {
      const more = await api.search(q, PAGE, off);
      if (query === q && tracks.length === off) { // descarta páginas de una búsqueda vieja
        tracks = [...tracks, ...more];
        if (more.length < PAGE) endReached = true;
      }
    } catch (e) { showErr(e); }
    loadingMore = false;
  }

  function onSearchInput() {
    window.clearTimeout(debounce);
    debounce = window.setTimeout(() => refresh().catch(showErr), 90); // debounce 90ms: búsqueda fluida
  }

  async function pickFolder() {
    try {
      const dir = await open({ directory: true, multiple: false, title: "Carpeta de música" });
      if (typeof dir === "string" && dir) {
        await api.scan(dir);
        await refresh();
      }
    } catch (e) { showErr(e); }
  }

  /// Id corto para logs (sin URLs ni rutas completas).
  function clicId(t: Track): string {
    return t.path.startsWith("ytmusic://")
      ? t.path.slice("ytmusic://".length)
      : (t.path.split(/[/\\]/).pop() ?? "?");
  }

  /// ¿La cola actual ya es exactamente esta lista? Evita reenviar 10 000
  /// pistas por IPC en cada clic dentro de la misma playlist.
  function sameQueue(a: Track[], b: Track[]): boolean {
    if (a.length !== b.length) return false;
    for (let k = 0; k < a.length; k++) if (a[k].path !== b[k].path) return false;
    return true;
  }

  async function playAt(i: number) {
    const t = tracks[i];
    // T1b: marca temporal del clic (el backend la une con gen/id).
    if (t) console.debug(`[clic] id=${clicId(t)} tCli=${Date.now()}`);
    // El clic solo marca "cargando": el resaltado de "en reproducción" y la
    // barra inferior salen del evento now_playing del backend.
    pendingPath = t?.path ?? null;
    try {
      // Solo manda la lista si cambió; el índice se fija con playIndex.
      if (!sameQueue(qItems, tracks)) {
        await api.queueSet(tracks, i); // lo cargado hasta ahora se vuelve la cola
      }
      if (ytImport != null) ytQueueActive = true; // cola ligada a la vista YT
      await api.playIndex(i);
      // Si sonó, su URL ya está resuelta: pinta el punto «Lista» sin pedir
      // otra vez el estado de toda la lista al backend.
      if (t && t.path.startsWith("ytmusic://")) {
        ready = { ...ready, [t.path]: true };
        if (dead[t.path] !== undefined) {
          const { [t.path]: _, ...rest } = dead;
          dead = rest;
        }
      }
      // BUG 1: el modal se abría solo al cambiar de track; al cerrar y volver a
      // reproducir el MISMO tema no hay evento de track y no reaparecía. Abrir
      // también en cada intención de play resuelve todos los caminos.
      openCover();
      await refresh(true);
    } catch (e) {
      pendingPath = null;
      // La cola ya avanzó pero nada suena: resincroniza con el backend.
      // Si es no disponible: gris + toast (no queda marcada como si sonara).
      const r = unavailableReason(e);
      if (t && r) {
        dead[t.path] = r;
        showToast(`No disponible: ${t.title}`);
      } else {
        showErr(e);
      }
      await refresh(true).catch(showErr);
    }
  }

  async function playQueueAt(i: number) {
    try {
      // Si es la que suena: solo abrir el cuadro, SIN reiniciar.
      if (qItems[i]?.path === nowPlaying?.uri) {
        openCover();
        return;
      }
      const t = qItems[i];
      if (t) console.debug(`[clic] id=${clicId(t)} tCli=${Date.now()}`);
      pendingPath = t?.path ?? null;
      await api.playIndex(i);
      openCover();
      await refresh(true);
    } catch (e) {
      pendingPath = null;
      const r = unavailableReason(e);
      const t = qItems[i];
      if (t && r) {
        dead[t.path] = r;
        showToast(`No disponible: ${t.title}`);
      } else {
        showErr(e);
      }
      await refresh(true).catch(showErr);
    }
  }

  // CAMBIO: un solo clic reproduce (sin retardo ni doble clic). Si es la que
  // suena, solo abre el cuadro sin reiniciar. Los botones usan stopPropagation.
  async function playRow(i: number, t: Track) {
    if (nowPlaying?.uri === t.path) {
      openCover();
      return;
    }
    await playAt(i);
  }

  // BUG 1 (continuación): toggle con retorno — al reanudar el mismo tema
  // también debe reaparecer el cuadro aunque se hubiera cerrado.
  async function doToggle() {
    try {
      const playing = await api.toggle();
      await refresh(true);
      if (playing) openCover();
    } catch (e) { showErr(e); }
  }

  function addOne(t: Track) {
    run(api.queueAdd(t));
  }

  /// Corazón de YouTube Music: guarda/quita el tema de «Me gusta». El estado
  /// visual cambia al instante (optimista) y se revierte si la red falla.
  /// Si el listado no traía el estado, el primer clic guarda (idempotente).
  /// Guarda/quita la playlist abierta de la biblioteca de YouTube Music.
  /// Es el botón de marcador de la app web, no el corazón de una canción.
  async function togglePlaylistSaved() {
    const id = ytBatch?.playlistId;
    if (!id || ytPlaylistSaving) return;
    if (ytAccount != null && !ytAccount.connected) {
      showToast("Conecta tu cuenta de YouTube Music para guardar la playlist", 3200);
      return;
    }
    const previous = ytBatch?.saved ?? null;
    const next = previous !== true;
    ytSavedOverride = next;
    ytBatch = ytBatch ? { ...ytBatch, saved: next } : ytBatch;
    ytPlaylistSaving = true;
    try {
      await api.ytSetPlaylistLike(id, next);
      showToast(next
        ? "Playlist guardada en tu biblioteca de YouTube Music"
        : "Playlist quitada de tu biblioteca");
    } catch (e) {
      ytSavedOverride = previous;
      ytBatch = ytBatch ? { ...ytBatch, saved: previous } : ytBatch;
      showErr(e);
    } finally {
      ytPlaylistSaving = false;
    }
  }

  async function toggleLike(path: string) {
    if (!path.startsWith("ytmusic://") || likeBusy[path]) return;
    if (ytAccount != null && !ytAccount.connected) {
      showToast("Conecta tu cuenta de YouTube Music para guardar «Me gusta»", 3200);
      return;
    }
    const id = path.slice("ytmusic://".length);
    const current = liked[path] ?? false;
    const next = !current;
    liked = { ...liked, [path]: next };
    likeBusy = { ...likeBusy, [path]: true };
    try {
      await api.ytSetLike(id, next);
      showToast(next ? "Guardada en tus Me gusta de YouTube Music" : "Quitada de tus Me gusta");
    } catch (e) {
      // Revertir el corazón: no puede quedar “guardado” si YouTube falló.
      liked = { ...liked, [path]: current };
      showErr(e);
    } finally {
      const { [path]: _, ...rest } = likeBusy;
      likeBusy = rest;
    }
  }

  function removeOne(e: MouseEvent | KeyboardEvent, i: number) {
    e.stopPropagation(); // evita que el clic reproduzca (el <li> también reproduce)
    run(api.queueRemove(i));
  }

  // U1: clic en una fila de la cola (cuerpo idéntico al inline que estaba en el
  // marcado: el clic que cierra un arrastre no debe reproducir).
  function queueRowClick(i: number) {
    if (qSuppressClick) { qSuppressClick = false; return; }
    playQueueAt(i);
  }

  // U1: vaciar la cola (antes era inline en el marcado de la cola).
  function clearQueue() {
    run(api.queueClear());
  }

  // Ejecuta una acción IPC, refresca y muestra el error si falla.
  function run(p: Promise<unknown>) {
    p.then(() => refresh(true).catch(showErr)).catch(showErr);
  }

  // G4: siguiente/anterior SIN refresco completo (no se repinta la lista): el
  // índice llega por el evento ligero de cola en <100 ms y la atribución por
  // playback://track. Al terminar, si nadie confirmó el destino (fallo de
  // resolución), se retira la marca optimista: no puede quedar resaltada una
  // pista que no suena.
  let stepClearTimer = 0;
  function step(delta: 1 | -1) {
    const p = delta === 1 ? api.next() : api.prev();
    p.then(() => {
      // En una ráfaga, el backend agrupa las cargas y el destino final puede
      // tardar en sonar. Un único temporizador (no uno por pulsación) evita
      // que la UI quite el "cargando" antes de tiempo y parece bloqueada.
      if (stepClearTimer) window.clearTimeout(stepClearTimer);
      stepClearTimer = window.setTimeout(() => {
        stepClearTimer = 0;
        const t = qCurrent != null ? qItems[qCurrent] : undefined;
        if (t && t.path === pendingPath && t.path !== nowPlaying?.uri) pendingPath = null;
      }, 3000);
    }).catch(showErr);
  }

  // ---- Playlists locales (MEJORA 4) ----

  async function loadPlists() {
    try {
      plists = await api.playlistList();
      if (plistName != null && !plists.some((p) => p.name === plistName)) plistName = null;
    } catch (e) { showErr(e); }
  }

  // M2: «Nueva playlist…» del menú Archivo tal y como se crea en la interfaz
  // nueva: abre el asistente en el paso «Playlist local» (misma ruta que el
  // popover de una fila, sin pista pendiente).
  function newPlistAssistant() {
    pendingAdd = null;
    plistDraft = "";
    addStart = "playlist";
    addOpen = true;
  }

  function renamePlist() {
    if (plistName == null) return;
    plistDraft = plistName;
    plistForm = "rename";
  }

  async function savePlist() {
    try {
      if (plistForm === "new") {
        const p = await api.playlistCreate(plistDraft);
        plistName = p.name;
        // "Nueva playlist…" desde el menú de una canción: la añade al crear.
        if (pendingAdd) {
          await api.playlistAdd(p.name, pendingAdd);
          pendingAdd = null;
        }
      } else if (plistForm === "rename" && plistName != null) {
        const p = await api.playlistRename(plistName, plistDraft);
        plistName = p.name;
      }
      plistForm = null;
      plistDraft = "";
      pendingAdd = null;
      await loadPlists();
      await refresh();
    } catch (e) { showErr(e); }
  }

  async function delPlist() {
    if (plistName == null) return;
    try {
      await api.playlistDelete(plistName);
      plistName = null;
      await loadPlists();
      await refresh();
    } catch (e) { showErr(e); }
  }

  async function removePlistTrack(path: string) {
    if (plistName == null) return;
    try {
      await api.playlistRemove(plistName, path);
      await loadPlists();
      await refresh();
    } catch (e) { showErr(e); }
  }

  /// Añade la cola actual a la playlist (la cola es la zona de staging).
  async function addQueueToPlist() {
    if (plistName == null || qItems.length === 0) return;
    try {
      for (const t of qItems) await api.playlistAdd(plistName, t);
      await loadPlists();
      await refresh();
    } catch (e) { showErr(e); }
  }

  // BUG 3 (continuación): ancla el menú bajo su botón; si no cabe abajo se
  // abre hacia arriba. Posición fija + z-index alto: nunca se mezcla con filas.
  function toggleMenu(path: string, t: Track, e: MouseEvent) {
    if (menu?.path === path) {
      menu = null;
      return;
    }
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const W = 232;
    const estH = 60 + (plists.length + 1) * 40;
    const x = Math.max(8, Math.min(r.right - W, window.innerWidth - W - 8));
    const flip = r.bottom + Math.min(estH, 320) > window.innerHeight - 16;
    menu = {
      path, track: t, x,
      y: flip ? r.top - 8 : r.bottom + 6,
      flip, vh: window.innerHeight,
    };
  }

  function closeMenu() {
    menu = null;
  }

  // --- Lámina de portada (no modal) ---
  // La lámina se renderiza en PlayerBar.svelte anclada a la miniatura (CSS position:absolute).
  // Aquí solo gestionamos el estado showCover y los listeners globales (clic fuera / Esc).
  function openCover() {
    showCover = true;
    // Registrar listeners SOLO mientras la lámina está abierta
    window.addEventListener('click', onCoverClickOutside);
    window.addEventListener('keydown', onCoverEsc);
  }

  function closeCover() {
    showCover = false;
    // Limpiar listeners al cerrar: cerrada = cero oyentes
    window.removeEventListener('click', onCoverClickOutside);
    window.removeEventListener('keydown', onCoverEsc);
    // Foco vuelve a .cover-btn
    queueMicrotask(() => {
      const btn = document.querySelector('.cover-btn') as HTMLButtonElement | null;
      btn?.focus();
    });
  }

  function onCoverClickOutside(e: MouseEvent) {
    const target = e.target as Node;
    // Ignorar clics dentro de la lámina (buscar .ob-cover en el DOM)
    const cover = document.querySelector('.ob-cover');
    if (cover?.contains(target)) return;
    // Ignorar clics en la miniatura (para que alterne abre/cierra sin cerrar y reabrir)
    if (target instanceof Element && target.closest('.cover-btn')) return;
    closeCover();
  }

  function onCoverEsc(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation(); // No interferir con otros Esc de la app (menús, diálogos)
      closeCover();
    }
  }

  async function addTrackToPlist(name: string, t: Track) {
    try {
      const added = await api.playlistAdd(name, t);
      closeMenu();
      // Sin redirect: el usuario se queda donde está (vista, scroll,
      // selección, reproducción). Solo toast + contador al día.
      await loadPlists();
      await refresh();
      showToast(added ? `Agregada a "${name}"` : `Ya estaba en "${name}"`);
    } catch (e) { showErr(e); }
  }

  function newPlistWithTrack(t: Track) {
    closeMenu();
    pendingAdd = t;
    plistDraft = "";
    plistForm = "new";
  }

  // U6a.1: «Nueva playlist…» desde el popover de una fila (UI nueva, sin fila
  // en línea): abre el asistente en el paso de playlist con la pista pendiente.
  // Al crear, savePlist la añade usando la misma condición `pendingAdd` y el
  // mismo comando playlist_add de siempre.
  function addNewPlistFromPopover(t: Track) {
    closeMenu();
    pendingAdd = t;
    plistDraft = "";
    addStart = "playlist";
    addOpen = true;
  }

  async function addFolderToPlist() {
    if (plistName == null) return;
    try {
      const dir = await open({ directory: true, multiple: false, title: "Carpeta para la playlist" });
      if (typeof dir !== "string" || !dir) return;
      const n = await api.playlistAddFolder(plistName, dir);
      await loadPlists();
      await refresh();
      if (n === 0) showErr("Sin audios nuevos en esa carpeta");
    } catch (e) { showErr(e); }
  }

  async function addFilesToPlist() {
    if (plistName == null) return;
    try {
      const files = await open({
        multiple: true, title: "Canciones para la playlist",
        filters: [{ name: "Audio", extensions: ["mp3", "flac", "ogg", "opus", "m4a", "wav", "aac"] }],
      });
      if (!files) return;
      const list = Array.isArray(files) ? files : [files];
      for (const f of list) {
        const stem = (f as string).split(/[/\\]/).pop()?.replace(/\.[^.]+$/, "") ?? "Sin título";
        await api.playlistAdd(plistName, {
          id: 0, path: f as string, title: stem, artist: "", album: "",
          genre: "", year: 0, track_no: 0, duration_secs: 0, mtime: 0, art_url: "",
        });
      }
      await loadPlists();
      await refresh();
    } catch (e) { showErr(e); }
  }

  /// Añade el tema en reproducción a la playlist.
  async function addCurrentToPlist() {
    if (plistName == null || cur == null) return;
    try {
      await api.playlistAdd(plistName, cur);
      await loadPlists();
      await refresh();
    } catch (e) { showErr(e); }
  }

  function toggleQueue() {
    queueMin = !queueMin;
    try {
      localStorage.setItem("musica.queueMin", queueMin ? "1" : "0");
    } catch { /* almacenamiento no disponible: no bloquea */ }
  }

  // M5: la piel 1-bit y la vista contextual son FIJAS (antes las encendían
  // musica.ui2 y musica.uiContextos). Se descartan las dos claves viejas para
  // no dejar estado residual; no se toca ninguna otra clave del usuario.
  onMount(() => {
    try {
      localStorage.removeItem("musica.ui2");
      localStorage.removeItem("musica.uiContextos");
    } catch { /* almacenamiento no disponible: no bloquea */ }
  });
  $effect(() => {
    document.body.classList.add("onebit", "ui-ctx");
    return () => document.body.classList.remove("onebit", "ui-ctx");
  });

  // M2: la barra de menú avisa del límite de YouTube. Antes se consultaba al
  // backend CADA segundo para siempre (un IPC/s incluso sin límite). Ahora el
  // valor real se pide una vez al arrancar y al recibir `youtube://throttled`;
  // la cuenta atrás es local y el intervalo solo existe mientras dura.
  function syncYtLimit() {
    api.ytThrottleSecs()
      .then((s) => { ytLimitSecs = s > 0 ? s : 0; })
      .catch(() => { /* backoff no disponible: sin aviso */ });
  }
  $effect(() => {
    if (ytLimitSecs <= 0) return;
    const id = window.setInterval(() => {
      ytLimitSecs = Math.max(0, ytLimitSecs - 1);
    }, 1000);
    return () => window.clearInterval(id);
  });

  // ---- Descargas (ajustes D4) ----
  function setDl(patch: Partial<DlSettings>) {
    dl = { ...dl, ...patch };
    saveDlSettings(dl);
  }

  /// ffmpeg puede faltar: la app no falla, solo deshabilita m4a/mp3/portada.
  async function loadDlEnv() {
    try {
      dlEnv = await api.downloadEnv();
      if (!dlEnv.ffmpeg && (dl.format === "mp3" || dl.format === "m4a")) setDl({ format: "original", embed: false });
    } catch { dlEnv = null; }
  }

  /// Carpeta de la próxima descarga: la por defecto si está elegida y el modo
  /// lo permite; si no, el selector del sistema (una vez por descarga).
  async function pickDlFolder(): Promise<string | null> {
    const f = effectiveFolder(dl);
    if (f) return f;
    try {
      const dir = await open({ directory: true, multiple: false, title: "Carpeta para las descargas" });
      return typeof dir === "string" && dir.length > 0 ? dir : null;
    } catch { return null; }
  }

  /// Botón "Cambiar": selector del sistema + guardar como carpeta por defecto.
  async function changeDlFolder() {
    try {
      const dir = await open({ directory: true, multiple: false, title: "Carpeta por defecto de descargas" });
      if (typeof dir === "string" && dir.length > 0) setDl({ folder: dir, mode: "default" });
    } catch (e) { showErr(e); }
  }

  function onDlMode(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    setDl({ mode: v === "ask" ? "ask" : "default" });
  }

  function onDlFormat(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    // m4a y mp3 necesitan ffmpeg; sin él solo cabe guardar el original.
    if ((v === "m4a" || v === "mp3") && dlEnv && !dlEnv.ffmpeg) return;
    if (v === "original" || v === "m4a" || v === "mp3") setDl({ format: v as DlFormat });
  }

  // ---- Descargas (D5: encolar y progreso) ----
  // El backend hace el trabajo en segundo plano; aquí solo se encola y se
  // sondea `download_progress` (mismo patrón que youtube_batch_progress). El
  // panel flotante se pinta aparte: la lista NO se repinta ni se bloquea.
  let dlSnap = $state<DownloadSnapshot | null>(null);
  let dlDone = $state<DownloadSnapshot | null>(null); // resumen del último lote terminado
  let dlConfirm = $state<{ url: string; total: number; title: string; saved?: boolean } | null>(null);
  // S2/S3: vista previa efímera de una lista importada desde CSV + emparejado
  // bajo demanda (nada se persiste: S4 guardará la lista). `csvStatus` lleva el
  // estado por índice de fila y `csvRange` el último rango visible de la lista.
  let csvView = $state<CsvPreview | null>(null);
  type CsvRowMatch = {
    status: "idle" | "loading" | "exacta" | "dudosa" | "sin_match" | "no_encontrada";
    candidate?: YtTrack | null;
    motivo?: string;
  };
  let csvStatus = $state<Record<number, CsvRowMatch>>({});
  let csvRange = $state<{ start: number; end: number } | null>(null);
  let csvWarmStarted = $state(false);
  let csvPlayNoticed = false; // aviso «solo esta pista»: solo la primera vez
  // S4: guardar la lista como playlist local (nombre + conflicto de huella) y
  // «Buscar otra versión» por pista guardada (candidatos frescos).
  let csvSaveOpen = $state(false);
  let csvSaving = $state(false);
  let csvSaveDup = $state<string | null>(null); // nombre en conflicto: reemplazar
  // S4.2c: aviso al guardar (null = sin aviso): no encontradas + sin intentar.
  let csvSaveWarn = $state<{ notFound: number; idle: number } | null>(null);
  let csvVarFor = $state<Track | null>(null);
  let csvVarRow = $state<number | null>(null); // S4.2c: fila de la vista previa
  let csvVarRows = $state<VariantRow[] | null>(null); // null = buscando
  let csvVarQuery = $state(""); // S4.2c: búsqueda libre del diálogo
  let csvVarGen = 0;            // invalida respuestas de búsquedas anteriores
  // S4.2b: «Emparejar todas» (vista previa o lista guardada): barrido
  // secuencial con pausa por backoff de límite, cancelación y resumen.
  let csvAllAsk = $state<{ kind: "preview" | "plist"; n: number } | null>(null);
  let csvAllRun = $state<{ total: number; plist: boolean } | null>(null);
  let csvAllEmp = $state(0);   // emparejadas del barrido (exactas + dudosas)
  let csvAllDud = $state(0);   // dudosas (⊂ emparejadas)
  let csvAllNone = $state(0);  // no encontradas del barrido
  let csvAllDone = $state(0);  // intentos terminados
  let csvAllWait = $state(0);  // segundos de backoff (0 = sin pausa)
  let csvAllCur = $state("");  // pista en curso (solo UI; nunca a consola)
  let csvAllGen = 0;           // generación: cancelar/cerrar aborta el bucle
  let dlTimer = 0;

  function stopDlPoll() {
    if (dlTimer) { window.clearInterval(dlTimer); dlTimer = 0; }
  }

  function startDlPoll() {
    if (dlTimer) return;
    const tick = async () => {
      try {
        const s = await api.downloadProgress();
        dlSnap = s;
        if (!s.active) {
          if (s.summary != null) dlDone = s; // resumen visible hasta que el usuario lo cierre
          stopDlPoll();
        }
      } catch { /* sondeo best-effort: un fallo puntual no rompe la descarga */ }
    };
    dlTimer = window.setInterval(tick, 700);
    tick();
  }

  function dlOpts(folder: string): DownloadOptions {
    // El backend decide qué incrustar según el modo: en "original" solo
    // etiquetas (y solo con ffmpeg); en m4a/mp3, portada + etiquetas.
    return { folder, format: dl.format, embed: dl.embed, numericPrefix: dl.prefix, playlistSubdir: null };
  }

  async function downloadTrack(t: Track) {
    if (!t.path.startsWith("ytmusic://")) return;
    const folder = await pickDlFolder();
    if (!folder) return; // el usuario canceló el selector
    try {
      dlDone = null;
      dlSnap = await api.downloadStart([{ id: t.path.slice("ytmusic://".length), title: t.title, artist: t.artist }], dlOpts(folder));
      startDlPoll();
      showToast("En cola de descargas");
    } catch (e) { showErr(e); }
  }

  function askDownloadPlaylist() {
    if (!ytImport) return;
    dlConfirm = { url: ytImport.url, total: ytBatch?.total ?? ytImport.tracks.length, title: ytImport.title };
  }

  /// S4.2c: descarga de la lista guardada abierta (p. ej. creada desde CSV).
  /// No hay URL de origen: se envían sus pistas ytmusic:// al MISMO comando de
  /// descarga de playlists (misma cola, subcarpeta y prefijo).
  function askDownloadSavedPlist() {
    if (plistName == null) return;
    const n = plistYtCount;
    if (n === 0) return;
    dlConfirm = { url: "", total: n, title: plistName, saved: true };
  }

  async function confirmDownloadPlaylist() {
    const c = dlConfirm;
    dlConfirm = null;
    if (!c) return;
    const folder = await pickDlFolder();
    if (!folder) return;
    try {
      dlDone = null;
      if (c.saved) {
        // La vista pudo cambiar mientras se elegía la carpeta.
        if (plistName == null || plistName !== c.title) return;
        const items = tracks
          .filter((t) => t.path.startsWith("ytmusic://"))
          .map((t) => ({ id: t.path.slice("ytmusic://".length), title: t.title, artist: t.artist }));
        if (items.length === 0) return;
        dlSnap = await api.downloadStartPlaylist("", dlOpts(folder), items, c.title);
      } else {
        dlSnap = await api.downloadStartPlaylist(c.url, dlOpts(folder));
      }
      startDlPoll();
      showToast(`Playlist en cola: ${dlSnap.total} pistas`);
    } catch (e) { showErr(e); }
  }

  async function cancelDownloads() {
    try { dlSnap = await api.downloadCancel(); } catch (e) { showErr(e); }
  }

  /// Nombre de la carpeta destino para el resumen: la ruta completa solo va
  /// en el tooltip (ni en logs ni en el panel aparecen rutas completas).
  function dlFolderName(p: string | null): string {
    if (!p) return "";
    return p.split(/[\\/]/).filter(Boolean).pop() ?? p;
  }

  // MEJORA 1: subir/bajar como alternativa al arrastre (mismo comando).
  async function moveQueue(i: number, dir: -1 | 1) {
    const j = i + dir;
    if (j < 0 || j >= qItems.length) return;
    try {
      await api.queueMove(i, j);
      await refresh(true);
    } catch (e) { showErr(e); }
  }

  // ---- YouTube Music (yt-dlp, solo streaming) ----
  // (ytImport declarado arriba, junto a playlists, por orden de declaración.)
  let ytProbeOk = $state<boolean | null>(null);
  let ytProbeInfo = $state<string | null>(null);
  let ytBrowsers = $state<BrowserProfile[]>([]);
  let ytCustomPath = $state("");
  let ytAccount = $state<YtAccount | null>(null);
  let ytBrowser = $state("brave");
  let ytLink = $state("");
  let ytLoading = $state(false);
  let ytMine = $state<YtMine[]>([]);
  let ytChannel = $state("");
  let showYtMine = $state(false);
  let ytExpired = $state(false);
  let ytVerifyMsg = $state<string | null>(null);
  let ytVerifying = $state(false);
  // L3b: login integrado de Google (ventana con datos aislados).
  let ytLoginMsg = $state<string | null>(null);
  let ytLoggingIn = $state(false);
  // L4/P4-17: oferta de borrar los datos aislados de la ventana (tras
  // desconectar o al arrancar sin sesión con un perfil huérfano).
  let ytDataOffer = $state(false);

  async function checkYt() {
    try {
      const p = await api.ytProbe();
      ytProbeOk = p.available;
      ytProbeInfo = p.available
        ? `yt-dlp ${p.version ?? "?"} · runtime JS: ${p.deno ? "deno" : p.node ? "node" : "ninguno (puede ir lento)"}`
        : p.error
          ? `yt-dlp falló: ${p.error}`
          : null;
    } catch { ytProbeOk = false; }
  }

  async function loadYtAccount() {
    try {
      ytAccount = await api.ytAccount();
      // P4-17: si queda un perfil de login huérfano (p. ej. tras reiniciar),
      // vuelve a ofrecerse borrarlo; solo mientras no haya sesión activa.
      ytDataOffer = !!ytAccount && !ytAccount.connected && ytAccount.login_data;
      await loadYtBrowsers();
      if (ytAccount.connected) {
        ytExpired = false;
        await loadYtMine();
      }
    } catch (e) { showErr(e); }
  }

  async function loadYtMine() {
    try {
      ytMine = await api.ytMyPlaylists();
    } catch {
      ytMine = []; // sin sesión o canal mal: vacío silencioso
    }
  }

  /// URL de la playlist abierta en modo lotes (para "Guardar fuente").
  let ytBatchUrl = $state("");

  /// Aplica una instantánea del cargador a la vista: filas mostradas +
  /// marcas de estado, sin llamadas extra al backend.
  function applyBatch(s: YtBatchSnapshot) {
    ytBatch = ytSavedOverride == null ? s : { ...s, saved: ytSavedOverride };
    if (s.warning && ytLimitSecs === 0) syncYtLimit(); // límite: cuenta atrás visible
    // M3.1: el nombre local no puede ser `tracks` (sombreaba el estado que
    // pinta la lista; asignar ahí era un error de compilación).
    const filas: Track[] = s.rows.map((r) => {
      const { state: _st, motivo: _mo, liked: _lk, ...t } = r;
      return t;
    });
    ytImport = { title: s.title, url: ytBatchUrl, tracks: filas };
    const rd: Record<string, boolean> = {};
    const dd: Record<string, string> = {};
    const ud: Record<string, boolean> = {};
    // Los estados del corazón llegan gratis en el listado. Se conserva lo que
    // el usuario ya cambió en esta sesión (no se pisa con un snapshot viejo).
    const lk: Record<string, boolean> = { ...liked };
    for (const r of s.rows) {
      if (r.state === "ready") rd[r.path] = true;
      else if (r.state === "dead" && r.motivo) dd[r.path] = r.motivo;
      else if (r.state === "unprepared") ud[r.path] = true;
      if (r.liked != null && lk[r.path] === undefined) lk[r.path] = r.liked;
    }
    ready = rd;
    dead = dd;
    unprepared = ud;
    liked = lk;
    // M3.1: la vista de YouTube YA está en `ytImport`, pero `tracks` (lo que
    // pinta la lista) solo se re-derivaba en refresh(), que corrió ANTES de
    // que hubiera filas → la lista quedaba vacía hasta navegar. Misma regla de
    // prioridad que refresh(): si hay import, esa es la lista. Sin llamadas
    // extra al backend (los lotes siguen siendo los mismos).
    tracks = filas;
  }

  /// Encuesta de progreso (~2/s): solo mientras se precalientan las primeras
  /// pistas del PRIMER lote (payload pequeño). La carga ya no bloquea filas.
  function pollYtBatch() {
    if (ytBatchTimer !== 0) return; // ya hay una encuesta programada
    ytBatchTimer = window.setTimeout(async () => {
      ytBatchTimer = 0;
      if (!ytBatch?.open || (!ytBatch.loading && !ytBatch.warming)) return;
      try {
        const s = await api.ytBatchProgress();
        if (!ytBatch?.open) return; // la cerraron mientras esperábamos
        applyBatch(s);
        if (s.loading || s.warming) pollYtBatch();
      } catch { /* progreso opcional: sin ruido */ }
    }, 500);
  }

  /// "Cargar más": revela el siguiente lote al instante (sin resolver audio)
  /// y precalienta sus primeras pistas en segundo plano.
  let ytMorePending = false;
  async function ytLoadMore() {
    if (!ytBatch?.open || !ytBatch.more || ytMorePending) return;
    ytMorePending = true;
    try {
      const before = ytBatch.shown;
      const s = await api.ytBatchMore();
      const nuevas: Track[] = s.rows.slice(before).map((r) => {
        const { state: _st, motivo: _mo, liked: _lk, ...t } = r;
        return t;
      });
      applyBatch(s);
      // Si se está reproduciendo esta lista, amplía la cola con las filas
      // nuevas (si no, el auto-avance se detendría al final del lote). Sin
      // reiniciar nada: el cursor actual se conserva en el backend.
      if (ytQueueActive && nuevas.length > 0) {
        // Se espera el append para que un clic inmediato no reenvíe la cola
        // y acabe duplicando las filas nuevas.
        await api.queueAppend(nuevas).catch(() => {});
      }
      // No se polla aquí: con la lista completa ya pintada, cada encuesta
      // reenviaría miles de filas. El calentado de fondo no bloquea el clic.
      if (s.loading) pollYtBatch();
    } catch (e) { showErr(e); }
    finally { ytMorePending = false; }
  }

  /// Olvida la vista YT y cancela su carga pendiente (regla 11): lo que ya
  /// estaba resolviendo termina y se guarda en caché.
  function forgetYtView() {
    window.clearTimeout(ytBatchTimer);
    ytBatchTimer = 0;
    ytBatch = null;
    ytBatchUrl = "";
    ytImport = null;
    ytQueueActive = false;
    ytSavedOverride = null;
    ytPlaylistSaving = false;
    unprepared = {};
    api.ytBatchCancel();
  }

  /// Cerrar la vista de YouTube desde su cabecera.
  function closeYtView() {
    const s = sourceId != null ? sources.find((x) => x.id === sourceId) : null;
    // G2: si la vista viene de una fuente guardada, se deselecciona: así
    // "Cerrar" no vuelve a la ruta vieja (sin lotes) de esa misma fuente.
    if (s?.kind === "youtube") sourceId = null;
    forgetYtView();
    refresh().catch(showErr);
  }

  // ---- P6: recomendaciones de YouTube Music ----

  function recoErrorFrom(e: unknown): RecoError {
    if (e != null && typeof e === "object") {
      const o = e as { code?: unknown; message?: unknown };
      if (typeof o.code === "string") {
        const codes = ["no_session", "auth", "config", "http", "network", "parse"];
        const code = (codes.includes(o.code) ? o.code : "network") as RecoError["code"];
        const message = typeof o.message === "string" && o.message.trim()
          ? o.message
          : "No se pudieron cargar las recomendaciones.";
        return { code, message };
      }
    }
    const text = typeof e === "string" ? e : e instanceof Error ? e.message : "";
    return { code: "network", message: text.trim() || "No se pudieron cargar las recomendaciones." };
  }

  /// Carga el home. `force` salta la caché de 10 min del backend. Si falla un
  /// refresco se conserva `recoFeed` y la vista muestra un aviso inline.
  async function loadReco(force = false) {
    if (!recoOpen) return;
    if (!ytSessionOk) {
      recoFeed = null;
      recoLoading = false;
      recoError = {
        code: "no_session",
        message: "Conecta tu cuenta de YouTube Music para ver recomendaciones.",
      };
      return;
    }
    const my = ++recoSeq;
    recoLoading = true;
    recoError = null;
    try {
      const feed = await api.ytRecommendations(force);
      if (my !== recoSeq || !recoOpen) return;
      recoFeed = feed;
    } catch (e) {
      if (my !== recoSeq) return;
      recoError = recoErrorFrom(e);
    } finally {
      if (my === recoSeq) recoLoading = false;
    }
  }

  function openRecommendations() {
    if (recoOpen) return;
    forgetYtView();
    plistName = null;
    sourceId = null;
    recoOpen = true;
    if (!ytSessionOk) {
      recoFeed = null;
      recoLoading = false;
      recoError = {
        code: "no_session",
        message: "Conecta tu cuenta de YouTube Music para ver recomendaciones.",
      };
      return;
    }
    void loadReco(false);
  }

  function closeReco() {
    recoOpen = false;
    refresh().catch(showErr);
  }

  function openRecoSettings() {
    settingsFocus = "yt";
    settingsOpen = true;
  }

  /// Reutiliza el importador existente: la tarjeta solo aporta el id de lista.
  function openRecoItem(item: RecoItem) {
    ytLink = `https://music.youtube.com/playlist?list=${encodeURIComponent(item.playlist_id)}`;
    void importYtLink(false);
  }

  async function importYtLink(more = false) {
    if (more) { await ytLoadMore(); return; }
    const url = ytLink.trim();
    if (!url || ytLoading) return;
    recoOpen = false;
    ytLoading = true;
    try {
      // W0: lista plana + primer lote resuelto EN SEGUNDO PLANO. Ninguna
      // fila aparece hasta que su lote esté procesado (Lista o Muerta).
      window.clearTimeout(ytBatchTimer);
      ytBatchTimer = 0;
      ytBatchUrl = url;
      ytSavedOverride = null;
      applyBatch(await api.ytBatchOpen(url));
      plistName = null;
      sourceId = null;
      pollYtBatch();
      await refresh();
    } catch (e) { showErr(e); }
    ytLoading = false;
  }

  async function playYtAll() {
    if (!ytImport || ytImport.tracks.length === 0) return;
    try {
      await api.queueSet(ytImport.tracks, 0);
      ytQueueActive = true;
      await api.playIndex(0);
      openCover();
      await refresh();
    } catch (e) { showErr(e); }
  }

  async function saveYtAsSource() {
    if (!ytImport) return;
    // U2: prevención de duplicados por id de lista (list=), no por URL cruda.
    const dup = findDupSource("youtube", ytImport.url);
    if (dup) { showToast(`Ya está guardada: «${dup.name}»`); return; }
    try {
      await api.sourceAdd("youtube", ytImport.title.slice(0, 60), ytImport.url, "", "");
      await loadSources();
      showToast("Fuente guardada");
    } catch (e) { showErr(e); }
  }

  // U4: acciones del asistente «+ Añadir». Reutilizan los flujos clásicos y se
  // cierran solas cuando el guardado se completó, sin tocar su lógica:
  // saveSource asigna un id nuevo; saveYtAsSource recarga las fuentes;
  // importYtLink sustituye ytImport por el resultado importado.
  async function addSaveSource() {
    const before = sourceId;
    await saveSource();
    if (sourceId !== before) addOpen = false;
  }
  async function addImportYtLink() {
    const before = ytImport;
    await importYtLink(false);
    if (ytImport !== before) addOpen = false;
  }
  async function addSaveYtAsSource() {
    const before = sources;
    await saveYtAsSource();
    if (sources !== before) addOpen = false;
  }

  // U4b: el asistente reutiliza savePlist para crear la playlist (el backend
  // aplica la deduplicación « (2)» de siempre). plistForm="new" es la condición
  // que savePlist espera; si falla, no se deja el formulario clásico detrás.
  async function addSavePlaylist() {
    plistForm = "new";
    await savePlist();
    if (plistForm === "new") plistForm = null;
  }

  // S2/S3: elegir un CSV local y mostrar su vista previa (parser en
  // csv_import.rs); el emparejado con YouTube es bajo demanda en la propia vista.
  async function pickCsvImport() {
    try {
      const f = await open({
        multiple: false,
        title: "Importar lista desde CSV",
        filters: [{ name: "CSV", extensions: ["csv", "txt"] }],
      });
      if (typeof f !== "string" || !f) return;
      csvView = await api.csvPreview(f);
      csvResetAux(); // vista nueva: sin estados ni rango del CSV anterior
    } catch (e) { showErr(e); }
  }

  // ---- S3: emparejado CSV → YouTube (efímero; candidato solo en memoria) ----

  const CSV_WARM_ROWS = 8; // calentado inicial: primeras 8 filas (cupo precarga)
  const CSV_VISIBLE_MARGIN = 2; // ±2 filas sobre el rango real del VirtualList

  function csvRowState(i: number): CsvRowMatch {
    return csvStatus[i] ?? { status: "idle" };
  }

  const csvMatched = $derived(
    Object.values(csvStatus).filter((s) => s.status === "exacta" || s.status === "dudosa").length
  );

  // S4.2c: filas que se quedarán fuera al guardar (chip «no encontrada»).
  const csvNotFoundCount = $derived(
    Object.values(csvStatus).filter((s) => s.status === "no_encontrada" || s.status === "sin_match").length
  );

  // S4.2b: pistas de YouTube de la lista abierta (con o sin match).
  const plistYtCount = $derived(tracks.filter((t) => t.path.startsWith("ytmusic://")).length);

  // S4.2b.1: pistas SIN match válido = ytmusic:// marcadas como no disponibles
  // (caché negativa). Las que ya tienen match NO se re-buscan: para cambiarlas
  // está «Buscar otra versión» por pista.
  const plistMatchable = $derived(
    tracks.filter((t) => t.path.startsWith("ytmusic://") && dead[t.path] !== undefined).length
  );

  function csvChip(st: CsvRowMatch): string {
    if (st.status === "loading") return "emparejando…";
    if (st.status === "dudosa") return "dudosa";
    // La UI colapsa los dos negativos en un mismo chip (el tooltip distingue).
    if (st.status === "no_encontrada" || st.status === "sin_match") return "no encontrada";
    return "sin emparejar";
  }

  // Motivo estable en Rust ("prefijo:resto"): el chip muestra el prefijo ya
  // traducido y el tooltip, el resto.
  function csvTip(st: CsvRowMatch): string {
    if (st.status === "idle") return "Se buscará en YouTube al reproducir";
    if (st.status === "loading") return "Buscando en YouTube…";
    const m = st.motivo ?? "";
    const i = m.indexOf(":");
    return i >= 0 ? m.slice(i + 1).trim() : m;
  }

  function csvResetAux() {
    csvStatus = {};
    csvRange = null;
    csvWarmStarted = false;
    csvVarClose(); // S4.2c: el diálogo de variantes no sobrevive a la vista
    csvSaveWarn = null;
    csvAllAbort(); // S4.2b: cerrar/importar otra vista corta el barrido en curso
  }

  // Empareja una fila y guarda su estado. Devuelve null si no se lanzó (ya en
  // vuelo / calentado sobre fila ya tocada) o si la búsqueda falló (el
  // calentado debe parar en seco y vuelve a «sin emparejar» sin toast).
  async function csvMatchRow(i: number, warm: boolean): Promise<CsvRowMatch | null> {
    const cv = csvView;
    const ct = cv?.rows[i];
    if (cv == null || ct == null) return null;
    const st = csvStatus[i];
    if (st?.status === "loading") return null;
    if (warm && st != null && st.status !== "idle") return null; // respeta clics
    csvStatus = { ...csvStatus, [i]: { status: "loading" } };
    try {
      const r = await api.csvMatch(ct.title, ct.artist, ct.album, ct.duration_s, warm);
      if (csvView !== cv) return null; // se cambió de vista: resultado huérfano
      const next: CsvRowMatch = { status: r.state, candidate: r.candidate, motivo: r.motivo };
      csvStatus = { ...csvStatus, [i]: next };
      return next;
    } catch (e) {
      if (csvView === cv) csvStatus = { ...csvStatus, [i]: { status: "idle" } };
      if (!warm) showErr(e); // precarga: retirada silenciosa (backoff/cupo)
      return null;
    }
  }

  // Reproduce una fila: empareja si hace falta y deja SOLO esa pista en la cola
  // (la lista completa del CSV llegará en S4). El clic sobre una fila ya
  // descartada reintenta una vez: si el negativo está cacheado responde al
  // instante, sin yt-dlp y sin bucle.
  async function csvPlay(i: number) {
    const cv = csvView;
    const ct = cv?.rows[i];
    if (cv == null || ct == null) return;
    let st = csvRowState(i);
    if (st.status === "loading") return;
    if (st.status !== "exacta" && st.status !== "dudosa") {
      const r = await csvMatchRow(i, false);
      if (r == null) return;
      st = r;
      if (st.status !== "exacta" && st.status !== "dudosa") {
        showToast("Sin coincidencia en YouTube (podrás buscar otra versión en S4)", 2600);
        return;
      }
    }
    const cand = st.candidate;
    if (cand == null) return;
    try {
      const track: Track = {
        id: 0, path: `ytmusic://${cand.id}`, title: cand.title, artist: cand.artist,
        album: ct.album || "YouTube", genre: "youtube", year: 0, track_no: 0,
        duration_secs: cand.duration_secs, mtime: 0,
        art_url: cand.thumbnail || `https://i.ytimg.com/vi/${cand.id}/hqdefault.jpg`,
      };
      await api.queueSet([track], 0);
      await api.playIndex(0);
      openCover();
      if (st.status === "dudosa") {
        // Un único aviso (texto pedido): la duda y el no-guardado, juntos.
        showToast(`Coincidencia dudosa con ${ct.title} — ${ct.artist}. Reproduciendo solo esta pista; la lista no se guarda aún (podrás elegir otra versión en S4)`, 4500);
      } else if (!csvPlayNoticed) {
        csvPlayNoticed = true;
        showToast("Reproduciendo sólo esta pista (la lista del CSV no se guarda aún)", 3000);
      }
      await refresh();
    } catch (e) { showErr(e); }
  }

  // «Emparejar visibles»: rango real del VirtualList + margen ±2 (documentado:
  // si el usuario hace scroll durante la operación, las filas del borde no se
  // pierden). Secuencial: el cupo de usuario es de un yt-dlp a la vez.
  async function csvMatchVisible() {
    const cv = csvView;
    if (cv == null || csvRange == null) return;
    const start = Math.max(0, csvRange.start - CSV_VISIBLE_MARGIN);
    const end = Math.min(cv.rows.length, csvRange.end + CSV_VISIBLE_MARGIN);
    for (let i = start; i < end; i++) {
      if (csvView !== cv) return;
      if (csvRowState(i).status !== "idle") continue;
      await csvMatchRow(i, false);
    }
  }

  // Calentado inicial (≤8 primeras filas, con cupo de precarga). S4.2b.1:
  // recorre las 8 aunque alguna falle (el fallo salta a la siguiente, no
  // corta) y si el backoff de límite se activa, espera en silencio y reanuda
  // en la MISMA fila (control puro en `runWarm`, con tests en `npm test`).
  // Arranca solo cuando el VirtualList emite su primer rango visible.
  async function csvWarmFirstRows() {
    const cv = csvView;
    if (cv == null) return;
    const gen = csvAllGen;
    await runWarm(Math.min(CSV_WARM_ROWS, cv.rows.length), {
      alive: () => gen === csvAllGen && csvView === cv,
      touched: (i) => csvRowState(i).status !== "idle", // ya la tocó el usuario
      attempt: async (i) => {
        const r = await csvMatchRow(i, true);
        if (r != null) return "ok";
        // Fallo del cupo de precarga o de red: con backoff activo se pausa y
        // reintenta; sin backoff se salta la fila y sigue el calentado.
        return (await api.ytThrottleSecs().catch(() => 0)) > 0 ? "throttle" : "fail";
      },
      waitThrottle: () => csvWaitThrottle(gen, () => csvView === cv, true),
    });
  }

  // Rango visible del VirtualList (llega solo cuando cambia; ver VirtualList).
  function csvOnRange(start: number, end: number) {
    const prev = csvRange;
    if (prev != null && prev.start === start && prev.end === end) return;
    csvRange = { start, end };
    console.debug(`[csv] rango ${start}..${end}`); // verificable en consola
    if (!csvWarmStarted && csvView != null) {
      csvWarmStarted = true;
      void csvWarmFirstRows();
    }
  }

  // ---- S4: persistencia en playlist local + «Buscar otra versión» ----

  /// Nombre sugerido: el del propio archivo CSV, sin extensión.
  function csvSuggestName(): string {
    const f = csvView?.file ?? "";
    return f.replace(/\.[^.]+$/, "") || "Lista de YouTube";
  }

  /// Entradas a guardar: solo filas con candidato; metadatos ORIGINALES del CSV
  /// (el vídeo elegido va en el path `ytmusic://`). Así «Buscar otra versión»
  /// re-busca desde los metadatos guardados, sin depender del CSV.
  function csvEntries(): Track[] {
    const cv = csvView;
    if (cv == null) return [];
    const out: Track[] = [];
    for (let i = 0; i < cv.rows.length; i++) {
      const st = csvStatus[i];
      const cand = st?.candidate;
      if (cand == null || (st.status !== "exacta" && st.status !== "dudosa")) continue;
      const ct = cv.rows[i];
      out.push({
        id: 0, path: `ytmusic://${cand.id}`, title: ct.title, artist: ct.artist,
        album: ct.album, genre: "youtube", year: 0, track_no: 0,
        // Duración original del CSV; si el CSV no traía, la del vídeo elegido.
        duration_secs: ct.duration_s ?? cand.duration_secs, mtime: 0,
        art_url: cand.thumbnail || `https://i.ytimg.com/vi/${cand.id}/hqdefault.jpg`,
      });
    }
    return out;
  }

  /// S4.2c: filas aún «sin emparejar» (sin entrada o devueltas a idle tras un
  /// fallo): también se quedarían fuera al guardar.
  function csvIdleCount(): number {
    const cv = csvView;
    if (cv == null) return 0;
    let n = 0;
    for (let i = 0; i < cv.rows.length; i++) if (csvRowState(i).status === "idle") n += 1;
    return n;
  }

  /// S4.2c: «Guardar como playlist» con filas que se quedarán fuera (no
  /// encontradas o sin intentar): primero se pregunta.
  function csvAskSave() {
    if (csvSaving) return;
    const idle = csvIdleCount();
    if (csvNotFoundCount > 0 || idle > 0) {
      csvSaveWarn = { notFound: csvNotFoundCount, idle };
      return;
    }
    csvSaveOpen = true;
  }

  function csvSaveWarnMsg(w: { notFound: number; idle: number }): string {
    const parts: string[] = [];
    if (w.notFound > 0) parts.push(`Hay ${w.notFound} no encontradas.`);
    if (w.idle > 0) parts.push(`Quedan ${w.idle} sin intentar.`);
    parts.push("¿Guardar así o volver a intentar?");
    return parts.join(" ");
  }

  async function csvSave(name: string, replace: boolean) {
    if (csvSaving) return;
    const ents = csvEntries();
    if (ents.length === 0) { showToast("No hay pistas emparejadas que guardar", 2400); return; }
    csvSaving = true;
    try {
      const r = await api.csvSavePlaylist(name, ents, replace);
      if (r.status === "duplicate") {
        // Mismo nombre y misma huella: no se creó nada; se ofrece reemplazar.
        csvSaveOpen = false;
        csvSaveDup = r.name;
        return;
      }
      csvSaveOpen = false;
      csvSaveDup = null;
      await loadPlists();
      showToast(`Playlist «${r.name}» guardada (${ents.length} pistas)`, 3000);
    } catch (e) { showErr(e); }
    finally { csvSaving = false; }
  }

  /// Metadatos ORIGINALES de la pista que se está buscando: de la lista
  /// guardada (m3u8) o de la fila CSV. Nunca de la consulta libre.
  function csvVarMeta(): { title: string; artist: string; album: string; duration_secs: number } | null {
    if (csvVarFor != null) {
      const t = csvVarFor;
      return { title: t.title, artist: t.artist, album: t.album, duration_secs: Math.max(0, t.duration_secs) };
    }
    const ct = csvVarRow != null ? csvView?.rows[csvVarRow] : null;
    if (ct == null) return null;
    return { title: ct.title, artist: ct.artist, album: ct.album, duration_secs: Math.max(0, ct.duration_s ?? 0) };
  }

  function csvVarClose() {
    csvVarGen++;
    csvVarFor = null;
    csvVarRow = null;
    csvVarRows = null;
    csvVarQuery = "";
  }

  /// Candidatos frescos con la consulta del campo (o la de siempre si está
  /// vacío). Los chips se calculan en el backend contra el ORIGINAL.
  async function csvVarSearch() {
    const meta = csvVarMeta();
    if (meta == null) return;
    const g = ++csvVarGen;
    csvVarRows = null; // «Buscando…»
    const manual = csvVarQuery.trim();
    try {
      const rows = await api.csvVariants(meta.title, meta.artist, meta.album, meta.duration_secs, manual.length > 0 ? manual : null);
      if (g !== csvVarGen) return; // se cerró/reabrió mientras buscaba
      csvVarRows = rows;
    } catch (e) {
      if (g !== csvVarGen) return;
      csvVarClose();
      showErr(e);
    }
  }

  /// «Buscar otra versión» de una pista guardada (metadatos del m3u8).
  function openCsvVariant(t: Track) {
    if (!t.path.startsWith("ytmusic://")) return;
    csvVarFor = t;
    csvVarRow = null;
    csvVarQuery = `${t.title} ${t.artist}`.trim();
    void csvVarSearch();
  }

  /// S4.2c: misma búsqueda desde una fila de la vista previa (no persiste).
  function openCsvVariantRow(i: number) {
    const ct = csvView?.rows[i];
    if (ct == null) return;
    csvVarFor = null;
    csvVarRow = i;
    csvVarQuery = `${ct.title} ${ct.artist}`.trim();
    void csvVarSearch();
  }

  /// Fija el candidato elegido: en la lista guardada solo cambia la referencia
  /// del vídeo; en la vista previa solo la fila en memoria hasta guardar (S4).
  async function csvPickVariant(v: VariantRow) {
    if (csvVarFor == null) {
      const i = csvVarRow;
      if (i == null || csvView == null) return;
      const st: CsvRowMatch = {
        status: v.state === "exacta" ? "exacta" : "dudosa",
        candidate: v,
        // Veredicto del backend relativo al original; si no encajaba y el
        // usuario lo elige a mano, queda como dudosa (hay candidato).
        motivo: v.state === "sin_match" ? "dudosa:elegida a mano" : v.motivo,
      };
      csvStatus = { ...csvStatus, [i]: st };
      csvVarClose();
      return;
    }
    const t = csvVarFor;
    const name = plistName;
    if (name == null) return;
    try {
      const nuevo: Track = {
        ...t, path: `ytmusic://${v.id}`,
        art_url: v.thumbnail || `https://i.ytimg.com/vi/${v.id}/hqdefault.jpg`,
      };
      const ok = await api.playlistReplaceTrack(name, t.path, nuevo);
      csvVarClose();
      if (ok) await refresh();
      else showToast("La pista ya no está en la lista", 2600);
    } catch (e) { showErr(e); }
  }

  function variantChip(v: VariantRow): string {
    if (v.state === "exacta") return "exacta";
    if (v.state === "dudosa") return "dudosa";
    return "no encaja";
  }

  function variantTip(v: VariantRow): string {
    const i = v.motivo.indexOf(":");
    return i >= 0 ? v.motivo.slice(i + 1).trim() : v.motivo;
  }

  // ---- S4.2b: «Emparejar todas» (barrido completo con pausa y resumen) ----

  /// Espera con cuenta atrás mientras el backoff de límite siga activo, SIN
  /// lanzar yt-dlp (sondear no refuerza el 429). Devuelve false si se canceló
  /// o la vista de origen dejó de estar viva. `silencioso` no pinta la pausa
  /// en la UI (calentado: tarea de fondo, sin progreso visible).
  async function csvWaitThrottle(gen: number, sigue: () => boolean, silencioso = false): Promise<boolean> {
    let s = 0;
    try { s = await api.ytThrottleSecs(); } catch { return gen === csvAllGen && sigue(); }
    while (s > 0) {
      if (gen !== csvAllGen || !sigue()) { if (!silencioso) csvAllWait = 0; return false; }
      if (!silencioso) csvAllWait = s;
      await new Promise((r) => setTimeout(r, 1000));
      try { s = await api.ytThrottleSecs(); } catch { s = 0; }
    }
    if (!silencioso) csvAllWait = 0;
    return gen === csvAllGen && sigue();
  }

  /// Cancelación del barrido (botón Cancelar, cierre de vista o import nuevo):
  /// sube la generación; el bucle lo detecta en el siguiente punto de control.
  function csvAllAbort() {
    csvAllGen++;
    csvAllWait = 0;
  }

  /// Resumen final/cancelado del barrido (mismos contadores que el progreso).
  function csvAllResumen(total: number, k: number, canceled: boolean) {
    const pend = Math.max(0, total - k);
    const base = `${csvAllEmp} emparejadas, ${csvAllDud} dudosas, ${csvAllNone} no encontradas${canceled ? `, ${pend} canceladas` : ""}`;
    showToast(canceled ? `Cancelado: ${csvAllEmp} emparejadas, ${pend} pendientes — ${base}` : base, 6000);
  }

  /// «Emparejar todas» de la vista previa: N = filas aún sin candidato (las
  /// negativas cacheadas responden al instante; los errores de red se
  /// reintentan de verdad). Siempre pide confirmación con N.
  function csvAllAskPreview() {
    const cv = csvView;
    if (cv == null || csvAllRun != null) return;
    let n = 0;
    for (let i = 0; i < cv.rows.length; i++) {
      const st = csvRowState(i);
      if (st.status !== "exacta" && st.status !== "dudosa") n++;
    }
    if (n === 0) { showToast("No queda nada por emparejar en esta vista", 2400); return; }
    csvAllAsk = { kind: "preview", n };
  }

  /// «Emparejar todas» de la lista guardada (S4.2b.1): N = pistas SIN match
  /// válido (ytmusic:// no disponibles). Las ya emparejadas no se tocan.
  function csvAllAskPlist() {
    if (plistName == null || csvAllRun != null) return;
    if (plistMatchable === 0) {
      showToast("Todas las pistas tienen match. Usa «Buscar otra versión» por pista si quieres cambiarlas", 3400);
      return;
    }
    csvAllAsk = { kind: "plist", n: plistMatchable };
  }

  function csvAllAskConfirm() {
    const ask = csvAllAsk;
    csvAllAsk = null;
    if (ask == null) return;
    if (ask.kind === "preview") void csvAllRunPreview();
    else void csvAllRunPlist();
  }

  /// Estado «en marcha» compartido por ambos barridos.
  function csvAllBegin(total: number, plist: boolean): number {
    const gen = ++csvAllGen;
    csvAllRun = { total, plist };
    csvAllEmp = 0; csvAllDud = 0; csvAllNone = 0; csvAllDone = 0; csvAllWait = 0;
    return gen;
  }

  /// Barrido de la vista previa: secuencial (cupo de usuario = 1 yt-dlp),
  /// saltando filas que ya se emparejaron por el camino y reintentando la
  /// MISMA fila cuando el fallo activó el backoff (se pausa y se cuenta atrás).
  async function csvAllRunPreview() {
    const cv = csvView;
    if (cv == null || csvAllRun != null) return;
    const pend: number[] = [];
    for (let i = 0; i < cv.rows.length; i++) {
      const st = csvRowState(i);
      if (st.status !== "exacta" && st.status !== "dudosa") pend.push(i);
    }
    if (pend.length === 0) return;
    const gen = csvAllBegin(pend.length, false);
    let k = 0;
    let canceled = false;
    while (k < pend.length) {
      if (gen !== csvAllGen || csvView !== cv) { canceled = true; break; }
      const i = pend[k];
      const st = csvRowState(i);
      if (st.status === "exacta" || st.status === "dudosa") { k++; csvAllDone = k; continue; }
      if (!(await csvWaitThrottle(gen, () => csvView === cv))) { canceled = true; break; }
      csvAllCur = cv.rows[i].title;
      const r = await csvMatchRow(i, false);
      if (gen !== csvAllGen || csvView !== cv) { canceled = true; break; }
      // 429 en ESTA fila: vuelve a «sin emparejar» y se reintenta tras el
      // backoff (la pausa la hace csvAllWaitThrottle al volver arriba).
      if (r != null && r.status === "no_encontrada" && (await api.ytThrottleSecs().catch(() => 0)) > 0) {
        csvStatus = { ...csvStatus, [i]: { status: "idle" } };
        continue;
      }
      if (r == null) {
        if (csvRowState(i).status === "idle") csvAllNone++; // fallo sin backoff
      } else if (r.status === "exacta" || r.status === "dudosa") {
        csvAllEmp++;
        if (r.status === "dudosa") csvAllDud++;
      } else {
        csvAllNone++;
      }
      k++;
      csvAllDone = k;
    }
    csvAllRun = null;
    csvAllCur = "";
    csvAllWait = 0;
    csvAllResumen(pend.length, k, canceled);
  }

  /// Barrido de la lista guardada (S4.2b.1): actúa SOLO sobre pistas SIN match
  /// válido (ytmusic:// marcadas como no disponibles); re-busca versión fresca
  /// y sustituye únicamente la referencia del vídeo (posición y metadatos
  /// intactos). Las ya emparejadas no se tocan.
  async function csvAllRunPlist() {
    const name = plistName;
    if (name == null || csvAllRun != null) return;
    const lista = tracks.filter((t) => t.path.startsWith("ytmusic://") && dead[t.path] !== undefined);
    if (lista.length === 0) return;
    const gen = csvAllBegin(lista.length, true);
    let k = 0;
    let canceled = false;
    let cambiado = false;
    while (k < lista.length) {
      if (gen !== csvAllGen || plistName !== name) { canceled = true; break; }
      if (!(await csvWaitThrottle(gen, () => plistName === name))) { canceled = true; break; }
      const t = lista[k];
      csvAllCur = t.title;
      let r: CsvMatchResult | null = null;
      try { r = await api.csvMatch(t.title, t.artist, t.album, Math.max(0, t.duration_secs), false); } catch { r = null; }
      if (gen !== csvAllGen || plistName !== name) { canceled = true; break; }
      if (r != null && r.state === "no_encontrada" && (await api.ytThrottleSecs().catch(() => 0)) > 0) {
        continue; // 429: misma pista tras el backoff
      }
      if (r != null && r.candidate != null && (r.state === "exacta" || r.state === "dudosa")) {
        const id = `ytmusic://${r.candidate.id}`;
        if (id !== t.path) {
          try {
            const nuevo: Track = {
              ...t, path: id,
              art_url: r.candidate.thumbnail || `https://i.ytimg.com/vi/${r.candidate.id}/hqdefault.jpg`,
            };
            await api.playlistReplaceTrack(name, t.path, nuevo);
            cambiado = true;
          } catch { /* best-effort: una pista no frena al resto */ }
        }
        csvAllEmp++;
        if (r.state === "dudosa") csvAllDud++;
      } else {
        csvAllNone++; // sin candidato o error puntual sin backoff
      }
      k++;
      csvAllDone = k;
    }
    csvAllRun = null;
    csvAllCur = "";
    csvAllWait = 0;
    if (cambiado) { try { await refresh(); } catch { /* la lista ya está guardada */ } }
    csvAllResumen(lista.length, k, canceled);
  }

  async function connectYt() {
    try {
      // Ruta manual (Flatpak/Snap/otro perfil): califica al navegador elegido.
      const custom = ytCustomPath.trim();
      let browser = ytBrowser;
      let profile: string | null = null;
      const found = ytBrowsers.find((b) => b.label === ytBrowser);
      if (found) {
        browser = found.browser;
        profile = found.profile;
      }
      if (custom.length > 0) profile = custom;
      ytAccount = await api.ytConnect(browser, profile);
      ytCustomPath = "";
      ytExpired = false;
      ytVerifyMsg = null;
      ytLoginMsg = null;
      ytDataOffer = false;
      await loadYtMine();
    } catch (e) { showErr(e); }
  }

  async function loadYtBrowsers() {
    try {
      ytBrowsers = await api.ytBrowsers();
      const labels = ytBrowsers.map((b) => b.label);
      if (!labels.includes(ytBrowser) && labels.length > 0) {
        ytBrowser = labels[0];
      }
    } catch { ytBrowsers = []; }
  }

  async function importYtCookies() {
    try {
      const f = await open({ multiple: false, title: "cookies.txt exportado del navegador" });
      if (typeof f !== "string" || !f) return;
      ytAccount = await api.ytImportCookies(f);
      ytExpired = false;
      ytVerifyMsg = null;
      ytLoginMsg = null;
      ytDataOffer = false;
      await loadYtMine();
    } catch (e) { showErr(e); }
  }

  async function disconnectYt() {
    try {
      await api.ytDisconnect();
    } catch (e) { showErr(e); return; }
    let avisoLimpieza: string | null = null;
    try {
      // P4-16: «Cerrar sesión» borra SIEMPRE el perfil aislado, no solo el jar.
      await api.ytClearLoginData();
    } catch (e) {
      const detalle = e instanceof Error ? e.message : String(e);
      avisoLimpieza = `Sesión cerrada, pero no se pudieron borrar los datos de la ventana de inicio de sesión: ${detalle}`;
    }
    try {
      ytAccount = await api.ytAccount();
      ytMine = [];
      ytExpired = false;
      ytVerifyMsg = null;
      ytLoginMsg = null;
      showYtMine = false;
      // Si la limpieza falló, `login_data` sigue true y se ofrece reintentarla.
      ytDataOffer = ytAccount?.login_data ?? false;
    } catch (e) { showErr(e); }
    if (avisoLimpieza) showErr(new Error(avisoLimpieza));
  }

  /// L4: borra solo los datos aislados de la ventana de login, a petición del
  /// usuario (oferta de "Cerrar sesión"); el jar y el método no se tocan aquí.
  async function clearYtLoginData() {
    try {
      await api.ytClearLoginData();
      ytDataOffer = false;
      ytAccount = await api.ytAccount();
      showToast("Datos de la ventana de inicio de sesión borrados");
    } catch (e) { showErr(e); }
  }

  async function saveYtChannel() {
    try {
      await api.ytSetChannel(ytChannel);
      ytChannel = "";
      await loadYtMine();
    } catch (e) { showErr(e); }
  }

  async function openYtMine(m: YtMine) {
    if (!m.url) return;
    ytLink = m.url;
    await importYtLink(false);
  }

  function onYtExpired() {
    // Aviso discreto sin romper la reproducción actual.
    ytExpired = true;
  }

  /// L2: aplica el resultado de una verificación (arranque o botón). Con
  /// marcador, la etiqueta es "Sesión sin verificar": nunca "navegador".
  function applyYtSession(r: YtSessionCheck) {
    ytAccount = r.account;
    if (r.status === "valid") {
      ytVerifyMsg = null;
      ytExpired = false;
      loadYtMine();
    } else if (r.status === "expired") {
      ytVerifyMsg = "Sesión caducada: reconecta o importa cookies.txt";
    } else if (r.status === "unverified") {
      ytVerifyMsg = "No se pudo verificar ahora";
    }
  }

  async function verifyYtSession() {
    ytVerifying = true;
    try {
      applyYtSession(await api.ytVerifySession());
    } catch (e) { showErr(e); } finally { ytVerifying = false; }
  }

  /// L3b: abre la ventana de login integrado de Google (datos aislados). El
  /// progreso llega por `youtube://login`; la ventana se cierra sola al validar.
  async function startYtGoogleLogin() {
    if (ytLoggingIn) return;
    ytLoggingIn = true;
    try {
      ytLoginMsg = "Abriendo la ventana de inicio de sesión…";
      ytDataOffer = false;
      await api.ytGoogleLogin();
    } catch (e) {
      ytLoginMsg = null;
      showErr(e);
    } finally { ytLoggingIn = false; }
  }

  /// L3b: pinta el estado del login integrado. "connected" solo cuando el jar
  /// candidato validó con LL (nunca antes: sin promesas falsas).
  function applyYtLogin(ev: YtLoginEvent) {
    ytAccount = ev.account;
    if (ev.state === "waiting") {
      ytLoginMsg = ev.message;
    } else if (ev.state === "connected") {
      ytLoginMsg = null;
      ytVerifyMsg = null;
      ytExpired = false;
      showToast("Sesión de YouTube conectada");
      loadYtMine();
    } else if (ev.state === "cancelled") {
      ytLoginMsg = null;
    } else {
      // failed | timeout: mensaje honesto, sin tocar el jar ni el marcador.
      ytLoginMsg = ev.message;
    }
  }

  // ---- Fuentes remotas (fase 1) ----

  async function loadSources() {
    try {
      sources = await api.sourceList();
      if (sourceId != null && !sources.some((s) => s.id === sourceId)) sourceId = null;
    } catch (e) { showErr(e); }
  }

  /// G2: abrir una fuente de YouTube YA GUARDADA con el MISMO cargador por
  /// lotes que un enlace pegado: "Cargando… N/50" y luego las filas. Si las
  /// URLs están vigentes en caché, el lote termina al instante (sin yt-dlp).
  async function openYouTubeSource(s: Source) {
    forgetYtView(); // regla 11: cancela la vista/lote anterior
    recoOpen = false;
    sourceId = s.id;
    plistName = null;
    window.clearTimeout(ytBatchTimer);
    ytBatchTimer = 0;
    ytBatchUrl = s.base_url;
    ytSavedOverride = null;
    ytLoading = true; // cabecera "Importando de YouTube…" mientras llega la lista plana
    try {
      applyBatch(await api.ytBatchOpen(s.base_url));
      pollYtBatch();
      await refresh();
    } catch (e) {
      // Sin lote no dejamos la pantalla vacía: se ve la fuente como antes.
      showErr(e);
      await refresh();
    }
    ytLoading = false;
  }

  async function pickSource(v: string) {
    sourceId = v === "local" ? null : Number(v);
    if (sourceId != null) plistName = null; // una vista cada vez
    const s = sourceId != null ? sources.find((x) => x.id === sourceId) : null;
    if (s?.kind === "youtube") {
      await openYouTubeSource(s);
      return;
    }
    forgetYtView();
    refresh().catch(showErr);
  }

  async function saveSource() {
    try {
      // U2: prevención de duplicados (misma clave que usa el simulacro).
      const dup = findDupSource(fKind, fUrl, fUser);
      if (dup) { showToast(`Ya está guardada: «${dup.name}»`); return; }
      const s = await api.sourceAdd(fKind, fName, fUrl, fUser, fSecret);
      fName = fUrl = fUser = fSecret = "";
      await loadSources();
      sourceId = s.id;
      // G2: una fuente YouTube recién guardada entra por el mismo cargador.
      if (s.kind === "youtube") {
        await openYouTubeSource(s);
      } else {
        await refresh();
      }
    } catch (e) { showErr(e); }
  }

  // U2: clave lógica de una fuente para los duplicados. YouTube se identifica
  // por el id de lista (list=), no por la URL cruda: el mismo id con otros
  // parámetros (&si=…) es la misma lista. El resto, por URL y usuario.
  function sourceKey(kind: string, url: string, user = ""): string {
    const u = url.trim().replace(/\/+$/, "");
    if (kind === "youtube") {
      const list = /[?&]list=([^&]+)/.exec(url)?.[1];
      return `youtube:${list ?? u}`;
    }
    if (kind === "subsonic" || kind === "jellyfin") return `${kind}:${u}|${user.trim().toLowerCase()}`;
    return `${kind}:${u}`;
  }

  function findDupSource(kind: string, url: string, user = ""): Source | null {
    const key = sourceKey(kind, url, user);
    return sources.find((s) => sourceKey(s.kind, s.base_url, s.user) === key) ?? null;
  }

  // U2: borrado de fuentes. Subsonic/Jellyfin piden confirmación porque el
  // borrado es definitivo (la contraseña no se guarda). En el resto, la fila
  // desaparece ya y el borrado real llega a los 6 s (o al siguiente
  // arranque): para poder deshacer solo se guarda el id pendiente.
  function askDeleteSource(s: Source) {
    if (s.kind === "subsonic" || s.kind === "jellyfin") { srcConfirmDel = s; return; }
    deleteSourceWithUndo(s);
  }

  function deleteSourceWithUndo(s: Source) {
    if (undoDel != null) commitDelete(undoDel.id); // el anterior se confirma ya
    sources = sources.filter((x) => x.id !== s.id);
    if (sourceId === s.id) { sourceId = null; refresh().catch(showErr); }
    undoDel = { id: s.id, name: s.name };
    try { localStorage.setItem("musica.pendingSourceDel", String(s.id)); } catch { /* sin almacenamiento */ }
    undoTimer = window.setTimeout(() => commitDelete(s.id), 6000);
  }

  async function commitDelete(id: number) {
    if (undoTimer != null) { window.clearTimeout(undoTimer); undoTimer = null; }
    if (undoDel?.id === id) undoDel = null;
    let pend = 0;
    try { pend = Number(localStorage.getItem("musica.pendingSourceDel") ?? "0"); } catch { /* sin almacenamiento */ }
    if (pend === id) { try { localStorage.removeItem("musica.pendingSourceDel"); } catch { /* idem */ } }
    try {
      await api.sourceRemove(id);
      if (sourceId === id) sourceId = null;
      // Con otro borrado pendiente no se recarga: traería su fila de vuelta.
      if (undoDel == null) await loadSources();
    } catch (e) { showErr(e); }
  }

  function undoDeleteSource() {
    if (undoDel == null) return;
    if (undoTimer != null) { window.clearTimeout(undoTimer); undoTimer = null; }
    undoDel = null;
    try { localStorage.removeItem("musica.pendingSourceDel"); } catch { /* sin almacenamiento */ }
    // La fila sigue en la base: recargar la devuelve a las listas.
    loadSources().then(() => refresh().catch(showErr)).catch(showErr);
  }

  function confirmDeleteSource() {
    const s = srcConfirmDel;
    srcConfirmDel = null;
    if (s != null) {
      // La fila se va ya (también si hay otro borrado con Deshacer pendiente).
      sources = sources.filter((x) => x.id !== s.id);
      if (sourceId === s.id) { sourceId = null; refresh().catch(showErr); }
      commitDelete(s.id);
    }
  }

  // U2: renombrar fuente (solo el nombre; comando source_rename).
  function askRenameSource(s: Source) {
    srcRename = { id: s.id, name: s.name };
  }

  async function saveSourceRename(name: string) {
    const r = srcRename;
    if (r == null) return;
    try {
      await api.sourceRename(r.id, name);
      srcRename = null;
      await loadSources();
      showToast("Nombre actualizado");
    } catch (e) { showErr(e); }
  }

  // U2: borrar playlist desde la barra lateral: misma apertura
  // (selectDropTarget) y misma acción (delPlist) que el menú clásico; solo
  // cambia la confirmación, que aquí es un diálogo propio.
  function askDeletePlaylist(name: string) {
    selectDropTarget("playlist:" + name);
    plistDeleteAsk = name;
  }

  async function confirmDeletePlaylist() {
    plistDeleteAsk = null;
    await delPlist();
  }

  // U6a.1: acciones de playlist desde el menú de la barra lateral. El «⋯»
  // clásico actúa sobre la playlist abierta; desde la barra se abre primero la
  // elegida (selectDropTarget fija plistName en el momento) y se reutiliza
  // después la MISMA función de siempre. Sin lógica paralela.
  function sidebarAddQueue(name: string) { selectDropTarget("playlist:" + name); addQueueToPlist(); }
  function sidebarAddCurrent(name: string) { selectDropTarget("playlist:" + name); addCurrentToPlist(); }
  function sidebarAddFolder(name: string) { selectDropTarget("playlist:" + name); addFolderToPlist(); }
  function sidebarAddFiles(name: string) { selectDropTarget("playlist:" + name); addFilesToPlist(); }

  // U6a.1: renombrar playlist con NameDialog (la UI nueva oculta la fila en
  // línea clásica). Al guardar se reutiliza el flujo de siempre: renamePlist
  // deja plistForm="rename" con el destino abierto y savePlist hace el resto.
  function askRenamePlaylist(name: string) {
    selectDropTarget("playlist:" + name);
    plistRename = name;
  }

  async function savePlistRename(newName: string) {
    plistRename = null;
    renamePlist();
    plistDraft = newName;
    await savePlist();
  }

  // U2: el botón clásico "quitar fuente" usa el MISMO flujo que la barra
  // lateral (Deshacer o aviso definitivo según el tipo de fuente).
  function removeSource() {
    const s = sources.find((x) => x.id === sourceId);
    if (s) askDeleteSource(s);
  }

  // ---- Motor de audio y Spotify Connect ----

  async function refreshSpotify() {
    try {
      spotify = await api.spotifyAccount();
    } catch (e) { showErr(e); }
  }

  async function connectSpotify() {
    try {
      // Requiere token PKCE (OAuth de Spotify). libresport verifica Premium
      // antes de inicializar: las cuentas Free se rechazan aquí.
      spotify = await api.spotifyConnect(pkceToken);
      pkceToken = "";
      const st = await api.playbackStatus();
      engine = st.engine;
    } catch (e) { showErr(e); }
  }

  async function disconnectSpotify() {
    try {
      await api.spotifyDisconnect();
      await refreshSpotify();
      nowPlaying = null; // la atribución de Spotify ya no es válida
      lastCoverUri = null;
      const st = await api.playbackStatus();
      engine = st.engine;
      await refresh();
    } catch (e) { showErr(e); }
  }

  async function logoutSpotify() {
    try {
      await api.spotifyLogout();
      await refreshSpotify();
      nowPlaying = null;
      lastCoverUri = null;
    } catch (e) { showErr(e); }
  }

  async function setEngine(kind: "libmpv" | "spotify") {
    try {
      const st: UnifiedStatus = await api.setEngine(kind);
      engine = st.engine;
      if (st.track) nowPlaying = st.track;
    } catch (e) { showErr(e); }
  }

  function onPlaybackState(ev: { payload: PlaybackStateEvent }) {
    if (ev.payload.gen < lastGen) return; // evento viejo
    lastGen = ev.payload.gen;
    engine = ev.payload.engine;
    // El estado también manda: sin esto, una pausa desde MPRIS/móvil no
    // actualizaba el botón ▶/⏸ hasta el siguiente tick/refresh.
    if (ev.payload.state === "playing") paused = false;
    else if (ev.payload.state === "paused" || ev.payload.state === "stopped") paused = true;
  }

  function onTrackChanged(ev: { payload: TrackChangedEvent }) {
    if (ev.payload.gen < lastGen) return; // evento viejo: llega tarde
    lastGen = ev.payload.gen;
    nowPlaying = ev.payload.track;
    cancelSeek(); // un seek pendiente no debe aplicarse a la pista nueva
    retryUri = null; // pista nueva: presupuesto de reintento fresco
    pendingPath = null; // el backend ya dijo qué suena: fin de "cargando"
    // Latencia backend→pintado (meta <200 ms): backend pone ts_ms al emitir.
    const g = ev.payload.gen, ts = ev.payload.ts_ms, tRecv = Date.now();
    requestAnimationFrame(() => {
      console.debug(`[ui-lat] gen=${g} lat=${Date.now() - ts}ms (cola=${tRecv - ts}ms)`);
    });
    const uri = ev.payload.track?.uri ?? null;
    // Si el nuevo track estaba marcado muerto y ahora suena, se rehabilita.
    if (uri && dead[uri] !== undefined) {
      const { [uri]: _, ...rest } = dead;
      dead = rest;
    }
    // Al empezar una canción distinta se abre el modal de portada.
    if (uri && uri !== lastCoverUri) {
      lastCoverUri = uri;
      openCover();
    } else if (!uri) {
      lastCoverUri = null;
    }
  }

  function onRestriction(ev: { payload: RestrictionEvent }) {
    spotNotice = ev.payload;
    window.clearTimeout(spotNoticeTimer);
    spotNoticeTimer = window.setTimeout(() => (spotNotice = null), 12000);
  }

  // Atajos: Espacio play/pause, Ctrl+K buscar, N/P siguiente/anterior, ←/→ seek.
  // Decisión: atajos en frontend porque las teclas multimedia del SO llegan por MPRIS/SMTC.
  function onKey(e: KeyboardEvent) {
    const tag = (e.target as HTMLElement)?.tagName;
    const inInput = tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") { e.preventDefault(); searchEl?.focus(); return; }
    // Escape cierra menús y diálogos (la lámina de portada tiene su propio handler con stopPropagation).
    if (e.key === "Escape") {
      menu = null;
      if (dlConfirm != null) dlConfirm = null;
      // S4/S4.2c: los diálogos del CSV se cierran sin tirar la vista previa.
      if (csvSaveDup != null) { csvSaveDup = null; return; }
      if (csvSaveWarn != null) { csvSaveWarn = null; return; }
      if (csvVarFor != null || csvVarRow != null) { csvVarClose(); return; }
      if (csvSaveOpen) { csvSaveOpen = false; return; }
      if (csvView != null) { csvView = null; csvResetAux(); }
      return;
    }
    if (inInput) return;
    // Con un botón enfocado, Espacio ya lo activa de forma nativa: no duplicar.
    if (e.code === "Space" && tag === "BUTTON") return;
    if (e.code === "Space") { e.preventDefault(); doToggle(); }
    // P4-05: N/P sin modificador (Ctrl/Alt/Meta+N no debe mover la cola).
    else if (!e.ctrlKey && !e.metaKey && !e.altKey && e.key === "n") step(1);
    else if (!e.ctrlKey && !e.metaKey && !e.altKey && e.key === "p") step(-1);
    else if (!e.ctrlKey && !e.metaKey && !e.altKey && e.key === "l") {
      const uri = shownTrack?.uri;
      if (uri) void toggleLike(uri);
    }
    else if (e.key === "ArrowRight") run(api.seek(pos + 5));
    else if (e.key === "ArrowLeft") run(api.seek(Math.max(0, pos - 5)));
  }

  // M2: tema explícito para el menú Ver (mismo efecto que el alternado).
  function setTheme(dark: boolean) {
    const el = document.documentElement;
    el.dataset.theme = dark ? "dark" : "light";
    themeDark = dark;
  }

  function toggleTheme() {
    setTheme(document.documentElement.dataset.theme !== "dark");
  }

  // Toast de confirmación breve (feedback al agregar a playlist).
  let toast = $state<string | null>(null);
  let toastTimer = 0;
  function showToast(msg: string, ms = 2000) {
    toast = msg;
    window.clearTimeout(toastTimer);
    toastTimer = window.setTimeout(() => (toast = null), ms);
  }

  // BUG 4: reordenar con eventos de puntero (el DnD HTML5 falla en webview).
  // Los botones subir/bajar siguen como alternativa.
  let qListEl: HTMLOListElement | null = $state(null);
  let qSuppressClick = false;

  /// Índice de inserción bajo el puntero (0..len; len = al final).
  /// P4-03: la cola solo monta la ventana visible. Se ancla en la primera fila
  /// montada y se calcula con su alto (uniforme): mismo resultado que recorrer
  /// todas las filas y devolver la primera cuyo punto medio queda bajo `y`.
  function insertionIndex(y: number): number | null {
    const ol = qListEl;
    if (!ol) return null;
    const fila = ol.querySelector<HTMLElement>("li[data-i]");
    if (!fila) return 0;
    const r = fila.getBoundingClientRect();
    const h = r.height || 1;
    const j = Number(fila.dataset.i) + Math.round((y - r.top) / h);
    return Math.max(0, Math.min(qItems.length, j));
  }

  async function commitMove(from: number, j: number) {
    // Sin movimiento real o inválido: nada.
    if (j === from || j === from + 1 || j < 0 || j > qItems.length) return;
    // move_item quita e inserta: si baja, el índice cae uno tras quitar.
    const to = j > from ? j - 1 : j;
    try {
      await api.queueMove(from, to);
      await refresh(true);
    } catch (e) { showErr(e); }
  }

  function qPointerDown(e: PointerEvent, i: number) {
    if (e.button !== 0) return;
    const y0 = e.clientY;
    let moved = false;
    const move = (ev: PointerEvent) => {
      if (!moved && Math.abs(ev.clientY - y0) <= 4) return;
      moved = true;
      dropIndex = insertionIndex(ev.clientY);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      const to = dropIndex;
      dropIndex = null;
      if (moved && to != null) {
        qSuppressClick = true; // el clic que sigue al arrastre no reproduce
        // Si el arrastre no genera clic, no puede quedar armado y tragarse el
        // siguiente clic: el clic real llega en la misma tarea del pointerup.
        window.setTimeout(() => (qSuppressClick = false), 0);
        commitMove(i, to);
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  }

  // Clic fuera: cierra el popover de playlists de una fila.
  function onDocClick(e: MouseEvent) {
    const t = e.target as HTMLElement | null;
    if (menu != null && !t?.closest?.(".track-menu, .menu-btn")) menu = null;
  }

  // M4.0: sin menú contextual nativo del webview. UN único oyente en document
  // (aquí, en el montaje de la app) suprime el menú nativo SALVO sobre campos
  // de texto —input, textarea y contenteditable—, que conservan el nativo para
  // copiar/pegar; la subida por ancestros cubre el interior de un editable.
  // Solo escucha «contextmenu»: no toca el teclado ni los atajos, y los menús
  // propios (p. ej. el clic derecho de la barra lateral) siguen igual: su
  // controlador corre antes y ya llama a preventDefault por su cuenta.
  function onContextMenu(e: MouseEvent) {
    for (let n: Element | null = e.target instanceof Element ? e.target : null; n != null; n = n.parentElement) {
      if (n.matches("input, textarea") || (n as HTMLElement).isContentEditable) return;
    }
    e.preventDefault();
  }

  onMount(() => {
    const light = matchMedia("(prefers-color-scheme: light)").matches;
    document.documentElement.dataset.theme = light ? "light" : "dark";
    themeDark = !light;
    refresh().catch(showErr).finally(() => { booted = true; });
    loadQueueState().catch(showErr);
    syncYtLimit(); // una sola consulta al arrancar (normalmente 0)
    loadSources();
    loadPlists();
    refreshSpotify();
    checkYt();
    loadYtAccount();
    // Entorno de descargas (ffmpeg): deshabilita mp3/portada si falta.
    loadDlEnv();
    // Eventos unificados del PlaybackRouter (motor agnóstico).
    const un: UnlistenFn[] = [];
    // Solo cola/biblioteca refetch completo: playback://state ya actualiza
    // pausa en ligero y el tick la posición (2/s). Refrescar 300 filas en cada
    // play/pausa degradaba la UI tras varias canciones.
    // G4: `playback://queue` trae {current,length,rev}. Si `rev` no cambió,
    // solo se mueve el cursor (sin reenviar las filas); el refetch completo
    // queda para cambios estructurales y para la biblioteca.
    listen<QueueEvent>("playback://queue", (ev) => onQueueChanged(ev.payload))
      .then((u) => un.push(u))
      .catch(showErr);
    listen("library://changed", () => refresh().catch(showErr))
      .then((u) => un.push(u))
      .catch(showErr);
    listen<PlaybackStateEvent>("playback://state", onPlaybackState).then((u) => un.push(u)).catch(() => {});
    listen<TrackChangedEvent>("playback://track", onTrackChanged).then((u) => un.push(u)).catch(() => {});
    listen<RestrictionEvent>("spotify://restriction", onRestriction).then((u) => un.push(u)).catch(() => {});
    listen("youtube://expired", onYtExpired).then((u) => un.push(u)).catch(() => {});
    // W3: límite de YouTube (429/not-a-bot): UN aviso por episodio, sin pausar.
    listen("youtube://throttled", () => {
      showToast("YouTube limitó las peticiones: reduciendo la carga unos minutos", 4000);
      syncYtLimit(); // fija el backoff real y arranca la cuenta atrás local
    })
      .then((u) => un.push(u)).catch(() => {});
    // L2: resultado de la verificación de sesión del arranque (segundo plano).
    listen<YtSessionCheck>("youtube://session", (ev) => applyYtSession(ev.payload))
      .then((u) => un.push(u)).catch(() => {});
    // L3b: login integrado de Google (ventana aislada → jar validado con LL).
    listen<YtLoginEvent>("youtube://login", (ev) => applyYtLogin(ev.payload))
      .then((u) => un.push(u)).catch(() => {});
    window.addEventListener("keydown", onKey);
    document.addEventListener("click", onDocClick);
    document.addEventListener("contextmenu", onContextMenu);
    // Paso 1: errores globales al archivo de log (sin secretos: solo mensajes).
    const onWinError = (e: ErrorEvent) => {
      api.reportError(`window.onerror: ${String(e.message ?? e)}`).catch(() => {});
    };
    const onRejection = (e: PromiseRejectionEvent) => {
      api.reportError(`unhandledrejection: ${String(e.reason ?? e)}`).catch(() => {});
    };
    window.addEventListener("error", onWinError);
    window.addEventListener("unhandledrejection", onRejection);
    // Medición UI (punto C): long tasks (>50ms) distinguen si lo que se traba
    // es la interfaz (longtasks crecen) o el audio/transición (no crecen).
    // Sin overhead cuando no hay tareas largas; solo console.debug cada 10.
    let longTasks = 0;
    try {
      const po = new PerformanceObserver((list) => {
        for (const e of list.getEntries()) {
          longTasks++;
          if (longTasks % 10 === 1) {
            console.debug(`[perf-ui] longtasks=${longTasks} ultima=${Math.round(e.duration)}ms`);
          }
        }
      });
      po.observe({ entryTypes: ["longtask"] });
      un.push(() => po.disconnect());
    } catch { /* PerformanceObserver no disponible: sin medición */ }
    // Evita solapar ticks si un IPC (red, auto-avance) tarda más de 500 ms.
    let tickBusy = false;
    const t = window.setInterval(async () => { // tick 500ms: barra de progreso sin IPC pesado
      if (tickBusy) return;
      tickBusy = true;
      try {
        const st = await api.status();
        paused = st.paused; pos = st.pos_secs; dur = st.dur_secs;
        confirmedState = st.confirmed;
        // "Cargando" hasta que mpv confirme (PlaybackRestart); el estado viene
        // de eventos, nunca del clic. Con error final no hay espera: se sale.
        loading = !st.paused && !st.confirmed && !st.error && (nowPlaying != null || cur != null);
        // Error real de mpv: se muestra UNA vez (es persistente hasta el próximo load).
        if (st.error && st.error !== lastPlayerError) {
          lastPlayerError = st.error;
          showErr(st.error);
        } else if (!st.error) {
          lastPlayerError = null;
        }
        // Un reintento con estrategia nueva ante error sin confirmación
        // (nunca mientras la carga avanza: confirmed ya lo cubre).
        if (engine === "libmpv" && st.error && !st.confirmed) {
          const uri = nowPlaying?.uri ?? cur?.path ?? null;
          if (uri && retryUri !== uri) {
            retryUri = uri;
            try { await api.retryCurrent(); } catch (e) { showErr(e); }
          }
        }
        // Auto-avance local al terminar (solo libmpv; en Spotify Spirc avanza).
        // El comando es atómico (consume el latch una vez) e idempotente.
        if (engine === "libmpv" && st.eof) {
          try {
            const rep = await api.trackEnded();
            for (const f of rep.failed) dead[f.path] = f.reason;
            // Un solo aviso: 1 omitida con nombre, varias agregadas en uno.
            if (rep.failed.length > 1) {
              showToast(`Se omitieron ${rep.failed.length} pistas no disponibles`);
            } else if (rep.failed.length > 0) {
              const first = tracks.find((x) => x.path === rep.failed[0].path);
              showToast(`Se omitió: ${first?.title ?? "pista no disponible"}`);
            }
            if (rep.error) showErr(rep.error);
          } catch (e) { showErr(e); }
        }
        // W0 regla 9: continuidad del auto-avance. Escuchando y quedando
        // ≤ 5 pistas del final de lo MOSTRADO (no del total de la lista),
        // se revela el siguiente lote al instante. Antes se comparaba con
        // `total` y el auto-avance nunca cargaba más allá del primer lote.
        if (
          ytBatch?.open === true && ytBatch.more && !ytBatch.loading &&
          !ytMorePending && !st.paused && qCurrent != null &&
          ytBatch.shown - (qCurrent + 1) <= 5
        ) {
          void ytLoadMore();
        }
      } catch {} finally {
        tickBusy = false;
      }
    }, 500);
    return () => { window.removeEventListener("keydown", onKey); document.removeEventListener("click", onDocClick); document.removeEventListener("contextmenu", onContextMenu); window.removeEventListener("error", onWinError); window.removeEventListener("unhandledrejection", onRejection); window.clearInterval(t); stopDlPoll(); cancelSeek(); cancelVolume(); un.forEach((u) => u()); };
  });

  // U2: si la app se cerró durante la ventana de 6 s del borrado con Deshacer,
  // el pendiente se confirma al arrancar (solo se guardó el id).
  onMount(() => {
    let id = 0;
    try { id = Number(localStorage.getItem("musica.pendingSourceDel") ?? "0"); } catch { /* sin almacenamiento */ }
    if (Number.isFinite(id) && id > 0) {
      try { localStorage.removeItem("musica.pendingSourceDel"); } catch { /* idem */ }
      api.sourceRemove(id).then(() => loadSources()).catch(showErr);
    }
  });

  // U4/M3.2: la barra de herramientas se retiró (sus funciones viven en los
  // menús: «Añadir carpeta…» en Archivo, Ajustes en Archivo, tema en Ver).
  // El título/sub de la vista los muestra la cabecera contextual (ContextBar).

  // V1: contexto activo (solo presentación) y textos de su cabecera.
  // Prioridad: import de YouTube → fuente abierta (youtube/radios/servidores)
  // → motor Spotify → biblioteca/playlist. No cambia ninguna función.
  type UiCtx = "musica" | "youtube" | "radios" | "servidores" | "spotify";
  const uiCtx: UiCtx = $derived.by(() => {
    if (recoOpen) return "youtube";
    if (ytImport != null) return "youtube";
    if (sourceId != null) {
      const kind = sources.find((s) => s.id === sourceId)?.kind ?? "";
      if (kind === "youtube") return "youtube";
      if (kind === "radio" || kind === "podcast") return "radios";
      return "servidores";
    }
    if (engine === "spotify") return "spotify";
    return "musica";
  });
  const ctxSrc = $derived(
    sourceId == null ? null : (sources.find((s) => s.id === sourceId) ?? null),
  );
  const ctxTitle = $derived.by(() => {
    if (recoOpen) return "Recomendaciones";
    if (uiCtx === "musica") return plistName ?? "Biblioteca";
    if (uiCtx === "youtube") return ytImport?.title ?? "YouTube Music";
    if (uiCtx === "radios") return ctxSrc?.name ?? "Radios";
    if (uiCtx === "servidores") return ctxSrc?.name ?? "Servidores";
    return "Spotify";
  });
  const ctxSub = $derived.by(() => {
    if (recoOpen) {
      if (recoLoading && !recoFeed) return "Cargando…";
      if (recoFeed != null) {
        const n = recoFeed.items.length;
        return `${n} ${n === 1 ? "playlist" : "playlists"}${recoFeed.community_present ? " · De la comunidad" : ""}`;
      }
      return recoError?.code === "no_session" || recoError?.code === "auth"
        ? "Sin sesión"
        : "YouTube Music";
    }
    if (uiCtx === "musica") {
      return plistName != null
        ? `${tracks.length} ${tracks.length === 1 ? "pista" : "pistas"}`
        : `${tracks.length} / ${total} canciones`;
    }
    if (uiCtx === "youtube") {
      if (ytImport == null) return "Sin vista abierta";
      const n = ytBatch?.shown ?? ytImport.tracks.length;
      const tot = ytBatch?.total ?? ytImport.tracks.length;
      return `${n} de ${tot} pistas`;
    }
    if (uiCtx === "radios") return ctxSrc?.kind === "podcast" ? "Podcast (RSS)" : "Radio (URL directa)";
    if (uiCtx === "servidores") return ctxSrc?.kind === "jellyfin" ? "Jellyfin" : "Subsonic/Navidrome";
    return spotify?.connected
      ? `Conectado${spotify.username ? `: ${spotify.username}` : ""}${spotify.device_name ? ` · ${spotify.device_name}` : ""}`
      : "Sin conectar";
  });

  // M2: estado de la barra de menú. El contador usa EXACTAMENTE el mismo texto
  // que el hint clásico; la sesión de YouTube, los mismos tres estados de la
  // barra lateral; y cada entrada de menú llama a funciones que ya existen
  // (pickFolder, el asistente, pickCsvImport, Ajustes, selectDropTarget,
  // openYtMine, tema y la ventana de atajos): aquí no hay lógica nueva.
  const menuCount = $derived(
    (recoOpen
      ? recoFeed != null
        ? `${recoFeed.items.length} recomendaciones`
        : recoLoading
          ? "Cargando recomendaciones…"
          : "Recomendaciones"
      : ytImport != null
      ? `${tracks.length} de YouTube · ${ytImport.title}`
      : sourceId == null && plistName == null
        ? `${tracks.length} / ${total} canciones`
        : `${tracks.length} resultados`) + (loadingMore ? " · cargando…" : ""),
  );
  const ytSessionOk = $derived(ytAccount?.connected === true);
  const menuSession = $derived(
    "YT · " +
      (ytAccount?.connected ? "Sesión activa" : ytAccount?.unverified ? "Sesión sin verificar" : "Sin sesión"),
  );

  const menuDefs = $derived.by(() => [
    {
      label: "Archivo",
      items: [
        { label: "Añadir carpeta…", icon: "folder", onSelect: pickFolder },
        { label: "Añadir fuente…", icon: "plus", onSelect: () => { addStart = null; addOpen = true; } },
        { label: "Nueva playlist…", icon: "playlist", onSelect: newPlistAssistant },
        { label: "Importar CSV…", icon: "download", onSelect: pickCsvImport },
        { sep: "" },
        { label: "Ajustes…", icon: "settings", onSelect: () => (settingsOpen = true) },
      ],
    },
    {
      label: "Fuente",
      items: [
        { label: "Biblioteca", icon: "disc", checked: plistName == null && sourceId == null, onSelect: () => selectDropTarget("library") },
        ...(sources.length > 0 ? [{ sep: "Fuentes" }] : []),
        ...sources.map((s) => ({ label: s.name, icon: "note", checked: sourceId === s.id, onSelect: () => selectDropTarget("source:" + s.id) })),
        ...(ytMine.length > 0 ? [{ sep: "YouTube Music" }] : []),
        ...ytMine.map((m) => ({ label: m.name, icon: "yt", disabled: !m.url, onSelect: () => openYtMine(m) })),
        { sep: "" },
        { label: "Pegar link de YouTube…", icon: "link", onSelect: () => { addStart = "youtube"; addOpen = true; fKind = "youtube"; } },
        { label: "Conectar Spotify…", icon: "connect", onSelect: () => { settingsFocus = "spotify"; settingsOpen = true; } },
      ],
    },
    {
      label: "Ver",
      items: [
        { label: "Tema claro", icon: "sun", checked: !themeDark, onSelect: () => setTheme(false) },
        { label: "Tema oscuro", icon: "moon", checked: themeDark, onSelect: () => setTheme(true) },
      ],
    },
    {
      label: "Ayuda",
      items: [
        { label: "Acerca de catPet…", icon: "disc", onSelect: () => (aboutOpen = true) },
        { label: "Atajos de teclado…", icon: "list", onSelect: () => (shortcutsOpen = true) },
      ],
    },
  ]);
</script>

<!-- M5: la barra de menú 1-bit (menús Archivo/Fuente/Ver/Ayuda + búsqueda +
     estado) es la única cabecera; la clásica y sus botones (Carpeta, Ajustes,
     tema y el comparador «UI clásica») se retiraron con la interfaz clásica.
     Todo sigue alcanzable: Añadir carpeta/Ajustes en Archivo, tema en Ver. -->
<MenuBar menus={menuDefs}
  bind:searchEl bind:query {onSearchInput} searchDisabled={plistName != null}
  searchTitle={plistName != null ? "Vuelve a Biblioteca para buscar" : "Buscar (Ctrl+K)"}
  countText={menuCount} sessionOk={ytSessionOk} sessionText={menuSession}
  limitSecs={ytLimitSecs} onHome={() => selectDropTarget("library")}>
  {#snippet dl()}
    <DlPanel snap={dlSnap} done={dlDone} folderText={dlDone?.folder ? dlFolderName(dlDone.folder) : ""}
      onCancel={cancelDownloads} onClose={() => (dlDone = null)} />
  {/snippet}
</MenuBar>
{#if err}
  <div class="err" role="alert"><IcoAdapt name="alert" size={16} /> {err} <button onclick={() => (err = "")} aria-label="Descartar" data-tip="Descartar">×</button></div>
{/if}
{#if spotNotice}
  <div class="notice" role="status"><IcoAdapt name="lock" size={16} /> {spotNotice.message}
    {#if spotNotice.code === "skip-budget"}
      <span class="hint">({spotNotice.skips_used}/{spotNotice.skips_max} saltos usados)</span>
    {/if}
    <button onclick={() => (spotNotice = null)} aria-label="Descartar" data-tip="Descartar">×</button>
  </div>
{/if}

<!-- MEJORA 5: barra de fuentes oculta por defecto; un botón la despliega. -->

<div class="shell">
  <!-- U2/U6a.1: abrir/renombrar/borrar y las acciones del «⋯» clásico
       (añadir cola/actual, agregar carpeta/canciones) delegan en las MISMAS
       funciones (selectDropTarget/pickSource/openYtMine; delPlist;
       addQueueToPlist/addCurrentToPlist/addFolderToPlist/addFilesToPlist). -->
  <Sidebar {plists} {sources} {ytMine} {ytAccount} {spotify} {plistName} {sourceId} {recoOpen} {total} sections={uiSections}
    ctxOn
    onOpenLibrary={() => selectDropTarget("library")}
    onOpenPlaylist={(name) => selectDropTarget("playlist:" + name)}
    onOpenSource={(s) => selectDropTarget("source:" + s.id)}
    onOpenYtMine={openYtMine}
    onOpenRecommendations={openRecommendations}
    onPasteLink={() => { addStart = "youtube"; addOpen = true; fKind = "youtube"; }}
    onAdd={() => { addStart = null; addOpen = true; }}
    onRenamePlaylist={askRenamePlaylist}
    onDeletePlaylist={askDeletePlaylist}
    onRenameSource={askRenameSource}
    onDeleteSource={askDeleteSource}
    onAddQueue={sidebarAddQueue}
    onAddCurrent={sidebarAddCurrent}
    onAddFolder={sidebarAddFolder}
    onAddFiles={sidebarAddFiles} />

<main class:queue-min={queueMin}>
  <section class="library" class:welcome={showWelcome}>
    <!-- V1: cabecera contextual; V2: «Música local» re-hogariza aquí sus
         controles (subcaso CSV). M3: los cinco contextos completos, escritos
         en el estilo 1-bit (Window/Btn/Icon16): YouTube (lotes y estado),
         radios/podcasts y servidores (cabecera; su administración vive en la
         barra lateral y los menús) y Spotify (motor y cuenta). Las barras
         clásicas de cada contexto se ocultan solo con el interruptor
         encendido (DOM idéntico apagado). -->
    {#key uiCtx}
      {#if recoOpen}
        <CtxReco title={ctxTitle} subtitle={ctxSub} loading={recoLoading}
          onRefresh={() => void loadReco(true)} onClose={closeReco} />
      {:else if uiCtx === "musica"}
        <CtxMusica title={ctxTitle} subtitle={ctxSub}
          showCsv={plistName != null && plistYtCount > 0}
          running={csvAllRun != null && csvAllRun.plist}
          emp={csvAllEmp} total={csvAllRun?.total ?? 0} done={csvAllDone}
          wait={csvAllWait} cur={csvAllCur} matchable={plistMatchable}
          onMatchAll={csvAllAskPlist} onCancel={csvAllAbort}
          onDownload={askDownloadSavedPlist} />
      {:else if uiCtx === "youtube"}
        <CtxYoutube title={ctxTitle} subtitle={ctxSub}
          shown={ytBatch?.shown ?? ytImport?.tracks.length ?? 0}
          hasImport={ytImport != null}
          loading={ytBatch?.loading ?? false}
          loadingText={`${ytBatch?.processed ?? 0}/${ytBatch?.batchTotal || ytBatch?.batchSize || 0}`}
          warning={ytBatch?.warning ?? null}
          canSave={ytBatch?.playlistId != null && ytBatch.playlistId !== "LM"}
          saved={ytBatch?.saved ?? null}
          saving={ytPlaylistSaving}
          onPlayAll={playYtAll} onSaveSource={saveYtAsSource}
          onToggleSaved={togglePlaylistSaved}
          onDownload={askDownloadPlaylist} onClose={closeYtView} />
      {:else if uiCtx === "radios"}
        <CtxRadios title={ctxTitle} subtitle={ctxSub} />
      {:else if uiCtx === "servidores"}
        <CtxServidores title={ctxTitle} subtitle={ctxSub} />
      {:else}
        <CtxSpotify title={ctxTitle} subtitle={ctxSub}
          connected={spotify?.connected === true}
          onSwitchEngine={() => void setEngine("libmpv")}
          onOpenSettings={() => { settingsFocus = "spotify"; settingsOpen = true; }} />
      {/if}
    {/key}
    {#if recoOpen}
      <RecoView feed={recoFeed} loading={recoLoading} error={recoError} sessionOk={ytSessionOk}
        onOpen={openRecoItem} onRefresh={loadReco} onOpenSettings={openRecoSettings} />
    {:else}
    {#if showWelcome}
      <!-- Portada de estreno: misma imagen que la portada del proyecto.
           Los botones reutilizan las acciones de siempre (carpeta y «+ Añadir»). -->
      <div class="home-cover">
        <div class="home-cover-frame">
          <img class="home-cover-img" src={portada}
            alt="Portada de catPet: mascota geométrica con auriculares y ondas de sonido" />
        </div>
        <p class="home-cover-lead">Añade una carpeta o una fuente para empezar.</p>
        <div class="home-cover-actions">
          <button class="primary" onclick={pickFolder}><IcoAdapt name="plus" size={15} /> Añadir carpeta</button>
          <button onclick={() => { addStart = null; addOpen = true; }}>
            <IcoAdapt name="plus" size={15} /> Añadir fuente…
          </button>
        </div>
      </div>
    {/if}
    {#if ytImport == null && (ytLoading || ytBatch?.open)}
      <!-- M5: con una vista de YouTube abierta, sus controles viven en la
           cabecera CtxYoutube (Reproducir todo, Guardar fuente, Descargar,
           Cerrar). Esta barra solo cubre la importación en curso (ytImport
           aún nulo), que es progreso global y no un control del contexto. -->
      <div class="ytbar">
        <KawaiiAdapt name="ytmusic" size={19} />
        {#if ytLoading}<span class="hint">Importando de YouTube…</span>{/if}
      </div>
    {/if}
    {#if plistName != null && tracks.length === 0}
      <div class="plist-empty">
        <KawaiiAdapt name="surprised" size={64} />
        <p><b>"{plistName}" está vacía.</b></p>
        <p class="hint">Arrastra canciones aquí o agrega una carpeta.</p>
        <div style="display:flex; gap:8px; justify-content:center;">
          <button class="primary" onclick={addFolderToPlist}><IcoAdapt name="plus" size={15} /> Agregar carpeta…</button>
          <button onclick={addFilesToPlist}><IcoAdapt name="plus" size={15} /> Agregar canciones…</button>
        </div>
      </div>
    {/if}
    {#if ytBatch?.open && ytBatch.loading && ytBatch.shown === 0}
      <!-- W4: carga del primer lote: solo contador N/total, sin lista ni botones. -->
      <div class="yt-loading" class:soft={RM}>
        <KawaiiAdapt name="ytmusic" size={42} />
        <span class="hint">Cargando… {ytBatch.processed}/{ytBatch.batchTotal || ytBatch.batchSize}</span>
      </div>
    {:else}
    <VirtualList items={tracks} rowHeight={56} onNearEnd={loadMore}>
      {#snippet row(t, i)}
        {@const tt = t as Track}
        <!-- T6: botón real (teclado nativo) + acciones como hermanas (sin
             botones anidados): ningún clic se pierde y no hay roles falsos. -->
        <div class="rowwrap">
          <button type="button" class="row" class:active={shownTrack?.uri === tt.path} class:pending={pendingPath === tt.path}
            aria-label={tt.title}
            title={pendingPath === tt.path ? "Cargando…" : (dead[tt.path] ?? tt.title)}
            draggable="true"
            ondragstart={(e) => { e.dataTransfer?.setData("text/plain", tt.path); }}
            onclick={() => playRow(i, tt)}>
            <!-- V2: portada 40 px o marcador si no hay arte (carga perezosa). -->
            <span class="art" aria-hidden="true" style="width: 40px; height: 40px; flex-shrink: 0;">
              {#if tt.art_url}
                <div class="art-container" style="width: 40px; height: 40px;">
                  <img use:cropBlackBars={tt.art_url} src={tt.art_url} alt="" loading="lazy" decoding="async" />
                </div>
              {:else}
                <IcoAdapt name="note" size={18} />
              {/if}
            </span>
            <div style="flex:1; min-width:0;">
              <div class="t" class:dead={dead[tt.path] !== undefined} title={dead[tt.path] ?? tt.title}>{tt.title}</div>
              <div class="a">{tt.artist} — {tt.album}</div>
            </div>
            {#if tt.path.startsWith("ytmusic://")}
              {@const sinPrep = unprepared[tt.path] === true && ready[tt.path] !== true && dead[tt.path] === undefined}
              <span class="rdy" class:ok={ready[tt.path] === true} class:unprepared={sinPrep}
                title={ready[tt.path] === true
                  ? "Lista: cambio casi instantáneo"
                  : dead[tt.path] !== undefined
                    ? dead[tt.path]
                    : sinPrep
                      ? "Sin preparar: tardará unos segundos; al hacer clic, prioridad máxima"
                      : "Aún no resuelta: tardará unos segundos"}></span>
            {/if}
            {#if shownTrack?.uri === tt.path}
              <!-- V2: ecualizador simple de la pista activa (solo transform;
                   estático con reduced-motion). -->
              <span class="eq" aria-hidden="true"><i></i><i></i><i></i></span>
            {/if}
            <span class="hint">{fmtSecs(tt.duration_secs)}</span>
          </button>
          <div class="row-actions">
            <button class="rowbtn" aria-label="Reproducir" onclick={(e) => { e.stopPropagation(); playAt(i); }} title="Reproducir"><KawaiiAdapt name="play" size={16} /></button>
            <button class="rowbtn" aria-label="Añadir a la cola" onclick={(e) => { e.stopPropagation(); addOne(tt); }} ondblclick={(e) => e.stopPropagation()} title="Añadir a la cola"><KawaiiAdapt name="plus" size={16} /></button>
            {#if tt.path.startsWith("ytmusic://")}
              <button class="rowbtn like" class:on={liked[tt.path] === true}
                aria-label={liked[tt.path] === true ? "Quitar «Me gusta» de YouTube Music" : "Guardar en «Me gusta» de YouTube Music"}
                aria-pressed={liked[tt.path] === true}
                disabled={likeBusy[tt.path] === true}
                onclick={(e) => { e.stopPropagation(); void toggleLike(tt.path); }}
                ondblclick={(e) => e.stopPropagation()}
                title={likeBusy[tt.path] === true
                  ? "Guardando en YouTube Music…"
                  : liked[tt.path] === true
                    ? "Quitar «Me gusta» de YouTube Music"
                    : "Guardar en «Me gusta» de YouTube Music"}>
                <KawaiiAdapt name={liked[tt.path] === true ? "heart-fill" : "heart"} size={16} />
              </button>
              <button class="rowbtn" aria-label="Descargar audio" onclick={(e) => { e.stopPropagation(); downloadTrack(tt); }} ondblclick={(e) => e.stopPropagation()} title="Descargar audio"><IcoAdapt name="download" size={15} /></button>
            {/if}
            {#if plistName != null && tt.path.startsWith("ytmusic://")}
              <button class="rowbtn" aria-label="Buscar otra versión" onclick={(e) => { e.stopPropagation(); openCsvVariant(tt); }} ondblclick={(e) => e.stopPropagation()} title="Buscar otra versión"><IcoAdapt name="list" size={15} /></button>
            {/if}
            <button class="rowbtn menu-btn" aria-label="Agregar a playlist" onclick={(e) => { e.stopPropagation(); toggleMenu(tt.path, tt, e); }} ondblclick={(e) => e.stopPropagation()} title="Agregar a playlist"><KawaiiAdapt name="playlist" size={16} /></button>
            {#if plistName != null}
              <button class="rowbtn" aria-label="Quitar de la playlist" onclick={() => removePlistTrack(tt.path)} ondblclick={(e) => e.stopPropagation()} title="Quitar de la playlist"><KawaiiAdapt name="remove" size={15} /></button>
            {/if}
          </div>
        </div>
      {/snippet}
    </VirtualList>
    {#if ytBatch?.open && ytBatch.more}
      <!-- W0 regla 7: "Cargar más" al FINAL de la lista (no en cabecera). -->
      <div class="yt-more">
        <button class="small" onclick={ytLoadMore} disabled={ytBatch.loading}
          title={ytBatch.loading ? "Resolviendo el siguiente lote…" : "Traer y preparar el siguiente lote"}>
          {ytBatch.loading
            ? `Cargando… ${ytBatch.processed}/${ytBatch.batchTotal || ytBatch.batchSize}`
            : `Cargar más (quedan ${ytBatch.total - ytBatch.shown})`}
        </button>
      </div>
    {/if}
    {/if}
    {/if}
    {#if menu != null}
      {@const m = menu}
      <!-- U4/M5: popover «Agregar a playlist» (el menú clásico de fila se
           retiró con la interfaz clásica). -->
      <Popover x={m.x} y={m.y} flip={m.flip} vh={m.vh} plists={plists}
        onPick={(name) => addTrackToPlist(name, m.track)}
        onNew={() => addNewPlistFromPopover(m.track)}
        onclose={() => (menu = null)} />
    {/if}
  </section>

  <!-- U1: cola movida a componente (mismo marcado; callbacks al padre). -->
  <Queue items={qItems} current={qCurrent} min={queueMin} dropIndex={dropIndex}
    reduced={RM} bind:listEl={qListEl}
    onToggle={toggleQueue} onPlay={queueRowClick} onGrip={qPointerDown}
    onMove={moveQueue} onRemove={removeOne} onClear={clearQueue} />
</main>
</div>

<!-- U1: reproductor movido a componente (mismo marcado; callbacks al padre). -->
<PlayerBar sounding={sounding} loading={loading} track={shownTrack} engine={engine}
  position={posShown} duration={dur > 0 ? dur : (cur?.duration_secs ?? 0)} fillPct={progPct} volume={vol} volName={volName}
  onStep={step} onToggle={doToggle} onOpenCover={openCover}
  onSeekInput={seekInput}
  onSeekDone={seekDone}
  onVolumeInput={volumeInput}
  onToggleMute={toggleMute}
  canLike={shownTrack?.uri?.startsWith("ytmusic://") ?? false}
  liked={shownTrack?.uri ? liked[shownTrack.uri] === true : false}
  likeBusy={shownTrack?.uri ? likeBusy[shownTrack.uri] === true : false}
  onToggleLike={() => { const uri = shownTrack?.uri; if (uri) void toggleLike(uri); }}
  showCover={showCover} shownTrack={shownTrack} onCloseCover={closeCover} />

{#if settingsOpen}
  <!-- U3: contenido de Ajustes; el estado y los handlers siguen en App.svelte. -->
  <SettingsDialog onclose={() => { settingsOpen = false; settingsFocus = null; if (recoOpen && (recoFeed == null || recoError != null)) void loadReco(false); }} focus={settingsFocus}
    {ytAccount} {ytVerifyMsg} {ytExpired} {ytLoginMsg} {ytLoggingIn} {ytVerifying}
    {ytBrowsers} bind:ytBrowser bind:ytCustomPath bind:ytChannel
    {ytDataOffer} {ytProbeOk} {ytProbeInfo} {showYtMine} {ytMine}
    onConnectYt={connectYt} onDisconnectYt={disconnectYt} onVerifyYt={verifyYtSession}
    onImportYtCookies={importYtCookies} onStartYtGoogleLogin={startYtGoogleLogin}
    onClearYtLoginData={clearYtLoginData} onKeepYtData={() => (ytDataOffer = false)}
    onSaveYtChannel={saveYtChannel}
    onToggleYtMine={() => { showYtMine = !showYtMine; if (showYtMine) loadYtMine(); }}
    onOpenYtMine={openYtMine} onCheckYt={checkYt}
    {dl} {dlEnv} onDlMode={onDlMode} onDlFormat={onDlFormat} onSetDl={setDl} onChangeDlFolder={changeDlFolder}
    {engine} {spotify} onSetEngine={setEngine}
    bind:pkceToken onConnectSpotify={connectSpotify} onDisconnectSpotify={disconnectSpotify} onLogoutSpotify={logoutSpotify}
    {themeDark} onToggleTheme={toggleTheme}
    sections={uiSections} onSectionToggle={setUiSection} />
{/if}

{#if addOpen}
  <!-- U4/U4b: asistente «+ Añadir» (tarjetas + campos); estado y handlers en
       App.svelte. Carpeta de música y Spotify reutilizan pickFolder y Ajustes. -->
  <AddDialog initialKind={addStart} bind:fKind bind:fName bind:fUrl bind:fUser bind:fSecret bind:ytLink
    {ytLoading} ytImportReady={ytImport != null} {ytProbeOk} {ytProbeInfo}
    bind:plistDraft activePlist={plistName}
    onSave={addSaveSource} onImport={addImportYtLink} onSaveYtSource={addSaveYtAsSource}
    onFolder={() => { addOpen = false; pickFolder(); }}
    onOpenSpotify={() => { addOpen = false; settingsFocus = "spotify"; settingsOpen = true; }}
    onCsvImport={() => { addOpen = false; pickCsvImport(); }}
    onSavePlaylist={addSavePlaylist} onAddFolder={addFolderToPlist} onAddFiles={addFilesToPlist}
    onclose={() => (addOpen = false)} />
{/if}

{#if shortcutsOpen}
  <!-- M2: Ayuda › «Atajos de teclado…» (el hint clásico del pie se retira en
       M5). Solo presentación. -->
  <ShortcutsDialog onclose={() => (shortcutsOpen = false)} />
{/if}

{#if aboutOpen}
  <!-- Ayuda › «Acerca de catPet…»: ficha de marca con las mascotas. -->
  <AboutDialog onclose={() => (aboutOpen = false)} />
{/if}

{#if toast != null}
  <div class="toast" role="status">{toast}</div>
{/if}

{#if undoDel != null}
  <div class="toast toast-undo" role="status">«{undoDel.name}» eliminada
    <button class="small" onclick={undoDeleteSource}>Deshacer</button>
  </div>
{/if}

{#if srcRename != null}
  <NameDialog title="Renombrar fuente" value={srcRename.name}
    onsave={saveSourceRename} onclose={() => (srcRename = null)} />
{/if}

{#if plistRename != null}
  <NameDialog title="Renombrar playlist" value={plistRename}
    onsave={savePlistRename} onclose={() => (plistRename = null)} />
{/if}

{#if srcConfirmDel != null}
  <ConfirmDialog title={`Eliminar «${srcConfirmDel.name}»`}
    message={srcConfirmDel.kind === "jellyfin"
      ? "Borrado definitivo: Jellyfin no guarda la contraseña y no se puede restaurar. Tus archivos no se tocan."
      : "Borrado definitivo: Subsonic no guarda la contraseña y no se puede restaurar. Tus archivos no se tocan."}
    onconfirm={confirmDeleteSource} onclose={() => (srcConfirmDel = null)} />
{/if}

{#if plistDeleteAsk != null}
  <ConfirmDialog title={`Eliminar «${plistDeleteAsk}»`}
    message="Se elimina la playlist de la app (su archivo .m3u8). Tus archivos de música no se tocan."
    onconfirm={confirmDeletePlaylist} onclose={() => (plistDeleteAsk = null)} />
{/if}

{#if dlConfirm != null}
  {@const dc = dlConfirm}
  <!-- Confirmación de playlist: nº de pistas + aviso de fallidas. Esc la cierra
       (la × de la ventana y Cancelar, además). -->
  <ObModal title="Descargar playlist" icon="download" ancho={440}
    onclose={() => (dlConfirm = null)}>
    <div class="ob-nota">“{dc.title}”: <b>{dc.total} pistas</b> a la cola de descargas. Las que fallen se contarán como fallidas.</div>
    <div class="ob-acts">
      <Btn primary onclick={confirmDownloadPlaylist}>Descargar</Btn>
      <Btn onclick={() => (dlConfirm = null)}>Cancelar</Btn>
    </div>
  </ObModal>
{/if}

{#if csvView != null}
  {@const cv = csvView}
  <!-- S2/S3: vista previa de un CSV importado + emparejado bajo demanda. Solo
       el ▶ y «Emparejar visibles» tocan la red; nada se persiste (S4). -->
  <ObModal title={`Vista previa · ${cv.file}`} icon="list" ancho={680}
    onclose={() => { csvView = null; csvResetAux(); }}>
    <div class="ob-nota">
      <b>{cv.total}</b> filas{cv.skipped > 0 ? ` · ${cv.skipped} descartadas` : ""}{cv.truncated ? ` · primeras ${cv.rows.length}` : ""}{cv.has_duration ? "" : " · sin columna de duración"}
    </div>
    <div class="ob-csv-tools">
      <span class="hint">Emparejadas {csvMatched}/{cv.rows.length}</span>
      <span class="ob-csv-tools-btns">
        {#if csvAllRun != null && !csvAllRun.plist}
          <!-- S4.2b: progreso del barrido completo (pausa con cuenta atrás). -->
          <span class="hint">Emparejadas {csvAllEmp}/{csvAllRun.total} · Trabajando {csvAllDone}/{csvAllRun.total}</span>
          {#if csvAllWait > 0}
            <span class="hint csvall-wait">YouTube limitó las búsquedas; reintento en {csvAllWait} s</span>
          {:else}
            <span class="hint csvall-cur" title={csvAllCur}>Procesando: {csvAllCur}</span>
          {/if}
          <Btn small onclick={csvAllAbort}>Cancelar</Btn>
        {:else}
          {#if csvMatched > 0}
            <!-- S4: aparece cuando el emparejamiento ya produjo resultados. -->
            <Btn small onclick={csvAskSave} disabled={csvSaving}>Guardar como playlist</Btn>
          {/if}
          <Btn small onclick={csvAllAskPreview} disabled={csvAllRun != null}>Emparejar todas</Btn>
          <Btn small onclick={csvMatchVisible} disabled={csvRange == null || csvAllRun != null}>Emparejar visibles</Btn>
        {/if}
      </span>
    </div>
    <div class="ob-csv-list">
      <VirtualList items={cv.rows} rowHeight={34} onRange={csvOnRange}>
        {#snippet row(r, i)}
          {@const ct = r as CsvTrack}
          {@const st = csvRowState(i)}
          <div class="ob-csv-row" title={ct.title}>
            <span class="ob-csv-n">{i + 1}</span>
            <span class="ob-csv-t">{ct.title}</span>
            <span class="ob-csv-a">{ct.artist}</span>
            <span class="ob-csv-al">{ct.album}</span>
            <span class="ob-csv-d">{ct.duration_s != null ? fmtSecs(ct.duration_s) : "—"}</span>
            <button class="ob-csv-play" title="Reproducir (busca en YouTube si hace falta)"
              disabled={st.status === "loading"} onclick={() => csvPlay(i)}>
              <IcoAdapt name="play" size={13} />
            </button>
            {#if st.status === "no_encontrada" || st.status === "sin_match" || st.status === "dudosa"}
              <button class="ob-csv-var" title="Buscar otra versión (o escribir una búsqueda libre)"
                onclick={() => openCsvVariantRow(i)}><IcoAdapt name="list" size={13} /></button>
            {:else}
              <span></span>
            {/if}
            {#if st.status !== "exacta"}
              <span class="ob-csv-chip" data-state={st.status} title={csvTip(st)}>{csvChip(st)}</span>
            {/if}
          </div>
        {/snippet}
      </VirtualList>
    </div>
    <div class="ob-acts ob-acts-split">
      <span class="hint">El ▶ reproduce una pista; «Guardar como playlist» crea la lista con las emparejadas.</span>
      <Btn onclick={() => { csvView = null; csvResetAux(); }}>Cerrar</Btn>
    </div>
  </ObModal>
{/if}

{#if csvSaveOpen}
  <!-- S4: nombre de la nueva lista (por defecto, el del archivo CSV). -->
  <NameDialog title="Guardar como playlist" label="Nombre" value={csvSuggestName()}
    onsave={(v) => csvSave(v, false)} onclose={() => (csvSaveOpen = false)} />
{/if}

{#if csvSaveWarn != null}
  <!-- S4.2c: si quedan no encontradas, se decide antes de abrir el nombre. -->
  <ObModal title="Guardar como playlist" icon="playlist" ancho={460}
    onclose={() => (csvSaveWarn = null)}>
    <div class="ob-nota">{csvSaveWarnMsg(csvSaveWarn)}</div>
    <div class="ob-acts">
      <Btn primary onclick={() => { csvSaveWarn = null; csvSaveOpen = true; }}>Guardar así</Btn>
      <Btn onclick={() => (csvSaveWarn = null)}>Volver a intentar</Btn>
    </div>
  </ObModal>
{/if}

{#if csvAllAsk != null}
  <!-- S4.2b: confirmación con el total antes de un barrido largo. -->
  <ConfirmDialog title="Emparejar todas"
    message={`Se emparejarán ${csvAllAsk.n} pistas. Puede tardar varios minutos. ¿Continuar?`}
    confirmLabel="Emparejar"
    onconfirm={csvAllAskConfirm} onclose={() => (csvAllAsk = null)} />
{/if}

{#if csvSaveDup != null}
  <!-- S4: mismo nombre y misma huella md5 ⇒ no se creó una segunda lista. -->
  <ObModal title="Ya existe esta lista" icon="playlist" ancho={480}
    onclose={() => (csvSaveDup = null)}>
    <div class="ob-nota">«{csvSaveDup}» ya está guardada con el mismo contenido. Reemplazarla actualiza esa lista; no crea otra.</div>
    <div class="ob-acts">
      <Btn primary disabled={csvSaving} onclick={() => { const n = csvSaveDup; if (n != null) void csvSave(n, true); }}>Reemplazar</Btn>
      <Btn onclick={() => (csvSaveDup = null)}>Cancelar</Btn>
    </div>
  </ObModal>
{/if}

{#if csvVarFor != null || csvVarRow != null}
  {@const vm = csvVarMeta()}
  {#if vm != null}
    <!-- S4/S4.2c: «Buscar otra versión» con búsqueda libre; en la lista
         guardada fija el vídeo del m3u8, en la vista previa solo la fila en
         memoria (se persiste al guardar). Los chips comparan con el ORIGINAL. -->
    <ObModal title="Buscar otra versión" icon="list" ancho={680} onclose={csvVarClose}>
      <div class="ob-nota">{vm.title} — {vm.artist}{vm.album ? ` · ${vm.album}` : ""}</div>
      <div class="ob-cvar-search">
        <input type="search" bind:value={csvVarQuery} placeholder="Buscar en YouTube (título, artista…)"
          onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); void csvVarSearch(); } }} />
        <Btn small onclick={() => void csvVarSearch()}>Buscar</Btn>
      </div>
      {#if csvVarRows == null}
        <div class="ob-cvar-empty">Buscando candidatos en YouTube…</div>
      {:else if csvVarRows.length === 0}
        <div class="ob-cvar-empty">YouTube no devolvió candidatos con esa búsqueda.</div>
      {:else}
        <div class="ob-cvar-list">
          {#each csvVarRows as v (v.id)}
            <div class="ob-cvar-row">
              <span class="ob-cvar-t" title={v.title}>{v.title}</span>
              <span class="ob-cvar-a" title={v.artist}>{v.artist}</span>
              <span class="ob-cvar-d">{fmtSecs(v.duration_secs)}</span>
              <span class="ob-csv-chip" data-state={v.state} title={variantTip(v)}>{variantChip(v)}</span>
              <Btn small onclick={() => csvPickVariant(v)}>Usar esta</Btn>
            </div>
          {/each}
        </div>
      {/if}
      <div class="ob-acts ob-acts-split">
        <span class="hint">{csvVarFor != null
          ? "Solo cambia el vídeo: se conservan título, artista, álbum y duración."
          : "Solo cambia esta fila; la lista se guarda al pulsar «Guardar como playlist»."}</span>
        <Btn onclick={csvVarClose}>Cerrar</Btn>
      </div>
    </ObModal>
  {/if}
{/if}
