<!-- P6: vista de recomendaciones. Tarjetas 1-bit (radio 0, sin sombras),
     sin portadas remotas: el «arte» es un bloque de rayas + icono para que la
     vista siga siendo monocroma y no dependa de la red. El clic delega en
     App.svelte, que reutiliza importYtLink/youtube_batch_open. -->
<script lang="ts">
  import Btn from "./onebit/Btn.svelte";
  import Icon16 from "./onebit/Icon16.svelte";
  import Kawaii from "./onebit/Kawaii1bit.svelte";
  import type { RecoError, RecoFeed, RecoItem } from "../tauri";

  let {
    feed = null,
    loading = false,
    error = null,
    sessionOk = true,
    onOpen,
    onRefresh,
    onOpenSettings,
  }: {
    feed?: RecoFeed | null;
    loading?: boolean;
    error?: RecoError | null;
    sessionOk?: boolean;
    onOpen: (item: RecoItem) => void;
    onRefresh: (force?: boolean) => void;
    onOpenSettings: () => void;
  } = $props();

  let needsSettings = $derived(
    !sessionOk || error?.code === "no_session" || error?.code === "auth",
  );
  let hasCards = $derived((feed?.items.length ?? 0) > 0);
  let showCommunityWarning = $derived(
    hasCards && !loading && feed?.community_present === false,
  );

  // P7-1b: la portada puede no existir (miniatura ausente) o no llegar (error
  // de red). En ambos casos se muestra un respaldo visible: la inicial del
  // título sobre las rayas. Antes de ocultar la imagen se reintenta la carga
  // una vez, por si el fallo fue transitorio.
  function inicialTitulo(title: string): string {
    const t = (title ?? "").trim();
    const ch = [...t].find((c) => /\p{L}|\p{N}/u.test(c));
    return (ch ?? "♪").toUpperCase();
  }

  // P7-1c: `loading="lazy"` dejaba sin pedir las portadas fuera del viewport
  // hasta hacer scroll. Ahora se piden todas al montar, pero como máximo
  // MAX_CARGA a la vez para no saturar la red; el resto espera turno.
  const MAX_CARGA = 6;
  let cola: HTMLImageElement[] = [];
  let activas = 0;

  function bombeaPortadas(): void {
    while (activas < MAX_CARGA && cola.length > 0) {
      const img = cola.shift();
      if (img == null || !img.isConnected) continue;
      const url = img.dataset.url ?? "";
      if (url === "") continue;
      img.dataset.enCurso = "1";
      activas += 1;
      img.src = url;
      // Red de seguridad: si la petición no termina, se libera el turno para
      // que la cola no se quede parada.
      setTimeout(() => {
        if (img.dataset.enCurso === "1") {
          delete img.dataset.enCurso;
          img.dataset.timeout = "1";
          liberaPortada();
          falloPortada(img);
        }
      }, 12000);
    }
  }

  function liberaPortada(): void {
    activas = Math.max(0, activas - 1);
    bombeaPortadas();
  }

  function sueltaPortada(img: HTMLImageElement): boolean {
    if (img.dataset.enCurso !== "1") return false; // ya liberada (timeout)
    delete img.dataset.enCurso;
    liberaPortada();
    return true;
  }

  function falloPortada(img: HTMLImageElement): void {
    const cover = img.closest(".reco-cover") as HTMLElement | null;
    img.style.display = "none";
    if (cover) {
      cover.dataset.portada = "fallo";
      cover.querySelector(".reco-inicial")?.classList.add("reco-inicial-visible");
    }
  }

  function portadaCargada(img: HTMLImageElement): void {
    if (!sueltaPortada(img)) return;
    const cover = img.closest(".reco-cover") as HTMLElement | null;
    if (cover) cover.dataset.portada = "ok";
  }

  function reintentaPortada(img: HTMLImageElement): void {
    if (!sueltaPortada(img)) return;
    if (img.dataset.reintento !== "1") {
      // Primer fallo: un reintento (puede haber sido la red). Vuelve a la cola
      // para no saltarse el tope de concurrencia.
      img.dataset.reintento = "1";
      setTimeout(() => {
        if (!img.isConnected) return;
        img.style.display = "";
        cola.push(img);
        bombeaPortadas();
      }, 700);
      return;
    }
    // Segundo fallo: se oculta la imagen y queda el respaldo visible.
    falloPortada(img);
  }

  // Acción: la imagen se monta sin src; la cola decide cuándo pedirla.
  function encolaPortada(img: HTMLImageElement): { destroy: () => void } {
    cola.push(img);
    bombeaPortadas();
    return {
      destroy() {
        const i = cola.indexOf(img);
        if (i >= 0) cola.splice(i, 1);
      },
    };
  }
</script>

<section class="reco" aria-label="Recomendaciones de YouTube Music" aria-busy={loading}>
  {#if loading && !feed}
    <div class="reco-state" role="status">
      <Kawaii name="ytmusic" size={42} />
      <b>Buscando recomendaciones…</b>
      <p class="hint">Leyendo tu home de YouTube Music.</p>
    </div>
  {:else if error && !hasCards}
    <div class="reco-state" role="alert">
      <Kawaii name={needsSettings ? "ytmusic" : "surprised"} size={42} />
      <b>{needsSettings ? "Sin sesión de YouTube Music" : "No se pudieron cargar"}</b>
      <p class="hint">
        {needsSettings
          ? "Conecta tu cuenta para ver las recomendaciones de tu home."
          : error.message}
      </p>
      {#if needsSettings}
        <Btn primary onclick={onOpenSettings} title="Abrir Ajustes en la sección de YouTube">
          Abrir Ajustes
        </Btn>
      {:else}
        <Btn primary onclick={() => onRefresh(true)} title="Volver a intentarlo">Reintentar</Btn>
      {/if}
    </div>
  {:else if !hasCards}
    <div class="reco-state" role="status">
      <Kawaii name="sleepy" size={42} />
      <b>No hay playlists para mostrar</b>
      <p class="hint">YouTube Music no devolvió tarjetas estables en esta carga.</p>
      <Btn onclick={() => onRefresh(true)} title="Volver a pedir el home">Actualizar</Btn>
    </div>
  {:else if feed != null && feed.items.length > 0}
    {@const data = feed}
    <div class="reco-top">
      <p>Playlists de tu home de YouTube Music.</p>
      <span class="reco-count">
        {data.items.length} {data.items.length === 1 ? "playlist" : "playlists"}
      </span>
    </div>

    {#if error}
      <p class="reco-error" role="alert">
        {error.message} Se muestran las últimas recomendaciones cargadas.
      </p>
    {/if}

    {#if data.warning}
      <p class="reco-warn" role="status">{data.warning}</p>
    {/if}

    {#if showCommunityWarning}
      <p class="reco-warn" role="status">
        No apareció «De la comunidad» en esta carga; se muestran las recomendaciones disponibles.
      </p>
    {/if}

    {#if data.truncated}
      <p class="reco-hint">Se muestran las primeras playlists estables del home.</p>
    {/if}

    <div class="reco-grid">
      {#each data.items as item, i (item.playlist_id)}
        <button
          class="reco-card"
          type="button"
          onclick={() => onOpen(item)}
          title="Abrir la playlist «{item.title}»"
          aria-label="Abrir {item.title}{item.author ? `, de ${item.author}` : ''}"
        >
          <span class="reco-cover" aria-hidden="true" data-portada={item.thumbnail ? "pendiente" : "sin"}>
            <span class="reco-stripes"></span>
            <span class="reco-inicial" class:reco-inicial-visible={!item.thumbnail}
              >{inicialTitulo(item.title)}</span
            >
            {#if item.thumbnail}
              <img
                class="reco-img"
                data-url={item.thumbnail}
                alt=""
                loading="eager"
                decoding="async"
                use:encolaPortada
                onload={(event) => portadaCargada(event.currentTarget as HTMLImageElement)}
                onerror={(event) =>
                  reintentaPortada(event.currentTarget as HTMLImageElement)}
              />
            {/if}
            <span class="reco-index">{String(i + 1).padStart(2, "0")}</span>
            <Icon16 name="playlist" size={30} />
          </span>
          <span class="reco-body">
            {#if item.section}
              <span class="reco-section">{item.section}</span>
            {/if}
            <span class="reco-title">{item.title}</span>
            <span class="reco-author">{item.author || "YouTube Music"}</span>
            {#if item.views}
              <span class="reco-views">{item.views}</span>
            {/if}
          </span>
          <span class="reco-open">
            <span>Abrir playlist</span>
            <Icon16 name="next" size={14} />
          </span>
        </button>
      {/each}
    </div>
  {/if}
</section>

<style>
  .reco {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 10px 14px 24px;
    color: var(--ink);
  }
  .reco-top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    margin: 0 0 10px;
    font-family: var(--font-pixel);
  }
  .reco-top p { margin: 0; font-size: 15px; font-weight: 700; }
  .reco-count { color: var(--ink-soft); font-size: 13px; white-space: nowrap; }
  .reco-error,
  .reco-warn,
  .reco-hint {
    margin: 0 0 10px;
    padding: 6px 9px;
    border: 2px solid var(--ink);
    background: var(--paper-2);
    color: var(--ink);
    font-size: 12.5px;
    line-height: 1.35;
  }
  .reco-hint { border-style: dashed; background: transparent; color: var(--ink-soft); }
  .reco-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
    gap: 12px;
    align-items: stretch;
  }
  .reco-card {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 0;
    min-width: 0;
    padding: 0;
    border: 2px solid var(--ink);
    border-radius: 0;
    background: var(--paper);
    color: var(--ink);
    box-shadow: none;
    text-align: left;
    font: inherit;
    overflow: hidden;
  }
  .reco-card:active { transform: none; }
  .reco-card:focus-visible { outline-offset: 3px; }
  .reco-cover {
    position: relative;
    display: grid;
    place-items: center;
    aspect-ratio: 16 / 7;
    min-height: 84px;
    overflow: hidden;
    border-bottom: 2px solid var(--ink);
    background: var(--paper-2);
    color: var(--ink);
  }
  .reco-stripes {
    position: absolute;
    inset: 0;
    background-image: repeating-linear-gradient(
      135deg,
      var(--ink) 0 1px,
      transparent 1px 7px
    );
    opacity: 0.35;
  }
  .reco-img {
    position: absolute;
    inset: 0;
    z-index: 1;
    width: 100%;
    height: 100%;
    object-fit: cover;
    border-radius: 0;
  }
  /* P7-1b: respaldo visible (inicial del título) cuando no hay portada o
     cuando la carga falló tras el reintento. */
  .reco-inicial {
    position: absolute;
    inset: 0;
    z-index: 1;
    display: grid;
    place-items: center;
    font-family: var(--font-pixel);
    font-size: 38px;
    font-weight: 700;
    line-height: 1;
    color: var(--ink);
    opacity: 0;
    pointer-events: none;
  }
  .reco-inicial.reco-inicial-visible { opacity: 1; }
  .reco-index {
    position: absolute;
    z-index: 2;
    top: 5px;
    left: 7px;
    font-family: var(--font-pixel);
    font-size: 13px;
    font-weight: 700;
  }
  .reco-cover :global(svg) { position: relative; z-index: 2; }
  .reco-card:hover .reco-cover,
  .reco-card:focus-visible .reco-cover {
    background: var(--paper);
    color: var(--ink);
  }
  .reco-body {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
    padding: 9px 10px 10px;
  }
  .reco-section {
    align-self: flex-start;
    max-width: 100%;
    padding: 1px 5px;
    border: 1px solid currentColor;
    font-family: var(--font-pixel);
    font-size: 11px;
    line-height: 1.25;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .reco-title {
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
    font-family: var(--font-pixel);
    font-size: 17px;
    font-weight: 700;
    line-height: 1.18;
    overflow-wrap: anywhere;
  }
  .reco-author,
  .reco-views {
    min-width: 0;
    color: inherit;
    font-size: 12px;
    line-height: 1.3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .reco-views { color: var(--ink-soft); }
  .reco-card:hover .reco-views { color: inherit; }
  .reco-open {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-top: auto;
    padding: 5px 10px;
    border-top: 2px solid currentColor;
    font-family: var(--font-pixel);
    font-size: 12px;
    font-weight: 700;
    text-transform: uppercase;
  }
  .reco-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-height: 240px;
    padding: 28px 16px;
    text-align: center;
    color: var(--ink);
  }
  .reco-state b { font-family: var(--font-pixel); font-size: 17px; }
  .reco-state .hint { padding: 0; max-width: 42ch; color: var(--ink-soft); }
  @media (max-width: 760px) {
    .reco-grid { grid-template-columns: repeat(auto-fill, minmax(210px, 1fr)); }
  }
</style>
