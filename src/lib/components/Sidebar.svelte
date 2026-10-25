<!-- U2/U6a.1: barra lateral funcional. Abrir delega en las funciones de la app
     (selectDropTarget/pickSource/openYtMine vía callbacks): sin lógica
     paralela. Renombrar y borrar también se delegan al padre, igual que las
     acciones del «⋯» clásico de playlist (abrir, añadir cola/actual, agregar
     carpeta/canciones, renombrar, borrar). Aquí solo hay presentación y el
     menú ⋯ / clic derecho (Esc y clic fuera se cierran en este componente).
     La Biblioteca no tiene acciones: no se puede borrar.
     M2: piel 1-bit para la interfaz nueva (iconos 16×16 y kawaii monocromo,
     mismos nombres que el set clásico; la búsqueda vive en la barra de menú). -->
<script lang="ts">
  import Icon from "./onebit/Icon16.svelte";
  import Kawaii from "./onebit/Kawaii1bit.svelte";
  import BrandMark from "./BrandMark.svelte";
  import type { PlaylistInfo, Source, YtMine, YtAccount, SpotifyAccountInfo } from "../tauri";

  // U3: secciones visibles de la barra lateral («Mostrar secciones» de Ajustes).
  const ALL_SECTIONS = { musica: true, youtube: true, radios: true, servers: true, spotify: true };

  let {
    plists = [],
    sources = [],
    ytMine = [],
    ytAccount = null,
    spotify = null,
    plistName = null,
    sourceId = null,
    recoOpen = false,
    total = 0,
    sections = ALL_SECTIONS,
    ctxOn = false,
    onOpenLibrary,
    onOpenPlaylist,
    onOpenSource,
    onOpenYtMine,
    onOpenRecommendations,
    onPasteLink,
    onAdd,
    onRenamePlaylist,
    onDeletePlaylist,
    onRenameSource,
    onDeleteSource,
    onAddQueue,
    onAddCurrent,
    onAddFolder,
    onAddFiles,
  }: {
    plists?: PlaylistInfo[];
    sources?: Source[];
    ytMine?: YtMine[];
    ytAccount?: YtAccount | null;
    spotify?: SpotifyAccountInfo | null;
    plistName?: string | null;
    sourceId?: number | null;
    /** P6: la vista de recomendaciones está abierta (activo en la barra). */
    recoOpen?: boolean;
    total?: number;
    sections?: { musica: boolean; youtube: boolean; radios: boolean; servers: boolean; spotify: boolean };
    /** V1: con la interfaz contextual encendida, el activo y los puntos de
        sesión llevan data-ctx para el matiz del contexto (solo color). */
    ctxOn?: boolean;
    onOpenLibrary: () => void;
    onOpenPlaylist: (name: string) => void;
    onOpenSource: (s: Source) => void;
    onOpenYtMine: (m: YtMine) => void;
    onOpenRecommendations: () => void;
    onPasteLink: () => void;
    onAdd: () => void;
    onRenamePlaylist: (name: string) => void;
    onDeletePlaylist: (name: string) => void;
    onRenameSource: (s: Source) => void;
    onDeleteSource: (s: Source) => void;
    onAddQueue: (name: string) => void;
    onAddCurrent: (name: string) => void;
    onAddFolder: (name: string) => void;
    onAddFiles: (name: string) => void;
  } = $props();

  // Plegado manual (además del automático por ancho, hecho con CSS).
  let folded = $state(false);
  const ytSources = $derived(sources.filter((s) => s.kind === "youtube"));
  const radios = $derived(sources.filter((s) => s.kind === "radio" || s.kind === "podcast"));
  const servers = $derived(sources.filter((s) => !["youtube", "radio", "podcast"].includes(s.kind)));
  const ytState = $derived(
    ytAccount?.connected ? "Sesión activa" : ytAccount?.unverified ? "Sesión sin verificar" : "Sin sesión",
  );

  // U2: menú de acciones (⋯ o clic derecho). Solo presentación local.
  type SbMenu =
    | { kind: "plist"; name: string; x: number; y: number; flip: boolean; vh: number }
    | { kind: "source"; src: Source; x: number; y: number; flip: boolean; vh: number };
  let menu = $state<SbMenu | null>(null);

  function openMenuAt(
    x: number, y: number,
    m: { kind: "plist"; name: string } | { kind: "source"; src: Source },
  ) {
    const vh = window.innerHeight;
    // U6a.1: siete acciones para playlists; tres para fuentes.
    const estH = m.kind === "plist" ? 300 : 148;
    const base = { x: Math.max(8, Math.min(x, window.innerWidth - 216)), y, flip: y + estH > vh - 10, vh };
    menu = m.kind === "plist"
      ? { kind: "plist", name: m.name, ...base }
      : { kind: "source", src: m.src, ...base };
  }

  function menuFromBtn(
    e: MouseEvent,
    m: { kind: "plist"; name: string } | { kind: "source"; src: Source },
  ) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    openMenuAt(r.left, r.bottom + 4, m);
  }

  /// Cierra el menú y ejecuta la acción elegida.
  function act(fn: (m: SbMenu) => void) {
    const m = menu;
    menu = null;
    if (m) fn(m);
  }

  function onWinClick() { menu = null; }
  function onWinKey(e: KeyboardEvent) { if (e.key === "Escape") menu = null; }
</script>

<svelte:window onclick={onWinClick} onkeydown={onWinKey} />

<aside class="sidebar" class:folded>
  <div class="sb-head">
    <span class="sb-logo"><BrandMark size={folded ? 20 : 24} title="" /></span>
    <span class="sb-title txt">catPet</span>
    <button class="icon small sb-fold" onclick={() => (folded = !folded)}
      title={folded ? "Desplegar barra lateral" : "Plegar barra lateral"} aria-expanded={!folded}>
      <Icon name={folded ? "chevR" : "chevL"} size={15} />
    </button>
  </div>
  <div class="sb-scroll">
    {#if sections.musica}
    <div class="sb-sec txt">Mi música</div>
    <div class="sb-row">
      <button class="sb-item" class:active={plistName == null && sourceId == null && !recoOpen}
        data-ctx={ctxOn ? "musica" : undefined}
        onclick={onOpenLibrary} title="Biblioteca">
        <Icon name="disc" size={15} /><span class="lbl">Biblioteca</span><span class="count txt">{total}</span>
      </button>
    </div>
    {#each plists as p (p.name)}
      <div class="sb-row">
        <button class="sb-item" class:active={plistName === p.name} onclick={() => onOpenPlaylist(p.name)}
          data-ctx={ctxOn ? "musica" : undefined}
          oncontextmenu={(e) => { e.preventDefault(); openMenuAt(e.clientX, e.clientY, { kind: "plist", name: p.name }); }}
          title={p.name}>
          <Kawaii name="playlist" size={16} /><span class="lbl">{p.name}</span><span class="count txt">{p.count}</span>
        </button>
        <button class="icon small sb-more" title="Acciones de «{p.name}»" aria-haspopup="menu"
          aria-expanded={menu?.kind === "plist" && menu.name === p.name}
          onclick={(e) => { e.stopPropagation(); menuFromBtn(e, { kind: "plist", name: p.name }); }}>
          <Icon name="dots" size={15} />
        </button>
      </div>
    {/each}
    {#if plists.length === 0}
      <div class="sb-empty txt">Aún no tienes playlists.</div>
    {/if}
    {#if plists.length === 0 && total === 0}
      <div class="sb-empty txt">Añade una carpeta con «+ Añadir» para empezar.</div>
    {/if}
    {/if}

    {#if sections.youtube}
    <div class="sb-sec txt">YouTube</div>
    <div class="sb-status" data-ctx={ctxOn ? "youtube" : undefined}>
      <span class="sb-dot" class:ok={ytAccount?.connected === true}></span>
      <span class="txt">{ytState}</span>
    </div>
    <div class="sb-row">
      <button class="sb-item" onclick={onPasteLink} title="Pegar un link de YouTube Music">
        <Icon name="link" size={15} /><span class="lbl">Pegar link</span>
      </button>
    </div>
    <div class="sb-row">
      <button class="sb-item" class:active={recoOpen} data-ctx={ctxOn ? "youtube" : undefined}
        onclick={onOpenRecommendations} title="Recomendaciones de YouTube Music">
        <Icon name="playlist" size={15} /><span class="lbl">Recomendaciones</span>
      </button>
    </div>
    {#each ytMine as m (m.url || m.name)}
      <div class="sb-row">
        <button class="sb-item" disabled={!m.url} onclick={() => onOpenYtMine(m)} title={m.name}>
          <Icon name="list" size={15} /><span class="lbl">{m.name}</span>
          {#if m.count != null}<span class="count txt">{m.count}</span>{/if}
        </button>
      </div>
    {/each}
    {#if ytSources.length > 0}
      <div class="sb-sec txt">Guardadas</div>
      {#each ytSources as s (s.id)}
        <div class="sb-row">
          <button class="sb-item" class:active={sourceId === s.id} onclick={() => onOpenSource(s)}
            data-ctx={ctxOn ? "youtube" : undefined}
            oncontextmenu={(e) => { e.preventDefault(); openMenuAt(e.clientX, e.clientY, { kind: "source", src: s }); }}
            title={s.name}>
            <Icon name="list" size={15} /><span class="lbl">{s.name}</span>
          </button>
          <button class="icon small sb-more" title="Acciones de «{s.name}»" aria-haspopup="menu"
            aria-expanded={menu?.kind === "source" && menu.src.id === s.id}
            onclick={(e) => { e.stopPropagation(); menuFromBtn(e, { kind: "source", src: s }); }}>
            <Icon name="dots" size={15} />
          </button>
        </div>
      {/each}
    {/if}
    {/if}

    {#if sections.radios}
    <div class="sb-sec txt">Radios</div>
    {#each radios as s (s.id)}
      <div class="sb-row">
        <button class="sb-item" class:active={sourceId === s.id} onclick={() => onOpenSource(s)}
          data-ctx={ctxOn ? "radios" : undefined}
          oncontextmenu={(e) => { e.preventDefault(); openMenuAt(e.clientX, e.clientY, { kind: "source", src: s }); }}
          title={s.name}>
          <Icon name="note" size={15} /><span class="lbl">{s.name}</span>
        </button>
        <button class="icon small sb-more" title="Acciones de «{s.name}»" aria-haspopup="menu"
          aria-expanded={menu?.kind === "source" && menu.src.id === s.id}
          onclick={(e) => { e.stopPropagation(); menuFromBtn(e, { kind: "source", src: s }); }}>
          <Icon name="dots" size={15} />
        </button>
      </div>
    {/each}
    {#if radios.length === 0}
      <div class="sb-empty txt">Sin radios ni podcasts.</div>
    {/if}
    {/if}

    {#if sections.servers && servers.length > 0}
      <div class="sb-sec txt">Servidores</div>
      {#each servers as s (s.id)}
        <div class="sb-row">
          <button class="sb-item" class:active={sourceId === s.id} onclick={() => onOpenSource(s)}
            data-ctx={ctxOn ? "servidores" : undefined}
            oncontextmenu={(e) => { e.preventDefault(); openMenuAt(e.clientX, e.clientY, { kind: "source", src: s }); }}
            title={s.name}>
            <Kawaii name="connect" size={16} /><span class="lbl">{s.name}</span>
          </button>
          <button class="icon small sb-more" title="Acciones de «{s.name}»" aria-haspopup="menu"
            aria-expanded={menu?.kind === "source" && menu.src.id === s.id}
            onclick={(e) => { e.stopPropagation(); menuFromBtn(e, { kind: "source", src: s }); }}>
            <Icon name="dots" size={15} />
          </button>
        </div>
      {/each}
    {/if}

    {#if sections.spotify}
    <div class="sb-sec txt">Spotify</div>
    <div class="sb-status" data-ctx={ctxOn ? "spotify" : undefined}>
      <span class="sb-dot" class:ok={spotify?.connected === true}></span>
      <span class="txt">{spotify?.connected ? `Conectado${spotify.username ? `: ${spotify.username}` : ""}` : "Sin conectar"}</span>
    </div>
    {/if}
  </div>
  <button class="sb-add" onclick={onAdd} title="Añadir fuente (Subsonic, Jellyfin, radio, podcast, YouTube)">
    <Icon name="plus" size={15} /><span class="lbl">Añadir</span>
  </button>
</aside>

{#if menu != null}
  {@const m = menu}
  <div class="menu sb-menu" role="menu" aria-label="Acciones"
    style="left:{m.x}px; top:{m.flip ? 'auto' : m.y + 'px'}; bottom:{m.flip ? (m.vh - m.y) + 'px' : 'auto'};">
    <button role="menuitem"
      onclick={() => act((mm) => { if (mm.kind === "plist") onOpenPlaylist(mm.name); else onOpenSource(mm.src); })}>
      <Kawaii name="play" size={15} /> Abrir
    </button>
    {#if m.kind === "plist"}
      <button role="menuitem"
        onclick={() => act((mm) => { if (mm.kind === "plist") onAddQueue(mm.name); })}>
        <Icon name="plus" size={15} /> Añadir cola
      </button>
      <button role="menuitem"
        onclick={() => act((mm) => { if (mm.kind === "plist") onAddCurrent(mm.name); })}>
        <Icon name="plus" size={15} /> Añadir actual
      </button>
      <button role="menuitem"
        onclick={() => act((mm) => { if (mm.kind === "plist") onAddFolder(mm.name); })}>
        <Icon name="plus" size={15} /> Agregar carpeta…
      </button>
      <button role="menuitem"
        onclick={() => act((mm) => { if (mm.kind === "plist") onAddFiles(mm.name); })}>
        <Icon name="plus" size={15} /> Agregar canciones…
      </button>
    {/if}
    <button role="menuitem"
      onclick={() => act((mm) => { if (mm.kind === "plist") onRenamePlaylist(mm.name); else onRenameSource(mm.src); })}>
      <Icon name="pencil" size={15} /> Renombrar…
    </button>
    <button role="menuitem"
      onclick={() => act((mm) => { if (mm.kind === "plist") onDeletePlaylist(mm.name); else onDeleteSource(mm.src); })}>
      <Icon name="trash" size={15} /> Borrar
    </button>
  </div>
{/if}
