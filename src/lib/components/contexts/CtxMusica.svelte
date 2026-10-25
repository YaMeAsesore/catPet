<!-- V2/M3: acciones del contexto «Música local» re-hogarizadas en la cabecera,
     escritas directamente en el estilo «Mac 1-bit» (la ventana la pone
     ContextBar; botones Btn y iconos Icon16, sin matices de color). Único
     botón con acento = acción principal («Emparejar todas», con TODOS sus
     estados: progreso, backoff y cancelación); las secundarias viven en el
     menú «⋯» («Descargar playlist»). El subcaso CSV conserva todos sus
     controles y textos exactos de la barra clásica, que sigue intacta para el
     interruptor apagado. Sin lógica: estado y handlers vienen de App.svelte. -->
<script lang="ts">
  import ContextBar from "./ContextBar.svelte";
  import Btn from "../onebit/Btn.svelte";
  import Icon16 from "../onebit/Icon16.svelte";

  let {
    title,
    subtitle,
    showCsv = false,
    running = false,
    emp = 0,
    total = 0,
    done = 0,
    wait = 0,
    cur = "",
    matchable = 0,
    onMatchAll,
    onCancel,
    onDownload,
  }: {
    title: string;
    subtitle: string;
    showCsv?: boolean;
    running?: boolean;
    emp?: number;
    total?: number;
    done?: number;
    wait?: number;
    cur?: string;
    matchable?: number;
    onMatchAll?: () => void;
    onCancel?: () => void;
    onDownload?: () => void;
  } = $props();

  // Menú «⋯» propio: cierra con clic fuera o Esc (los diálogos/menús nuevos
  // gestionan su propio Esc; onKey de App.svelte no se toca). Con el menú
  // cerrado no hay oyentes: las teclas quedan para la app. Esc además suelta
  // el foco (como en la barra de menú) para que Espacio/N/P vuelvan a la app.
  let moreOpen = $state(false);
</script>

<svelte:window
  onclick={() => (moreOpen = false)}
  onkeydown={(e) => {
    if (moreOpen && e.key === "Escape") {
      moreOpen = false;
      (document.activeElement as HTMLElement | null)?.blur?.();
    }
  }} />

<ContextBar ctx="musica" {title} subtitle={subtitle}>
  {#snippet actions()}
    {#if showCsv}
      {#if running}
        <span class="hint">Emparejadas {emp}/{total} · Trabajando {done}/{total}</span>
        {#if wait > 0}
          <span class="hint csvall-wait">YouTube limitó las búsquedas; reintento en {wait} s</span>
        {:else}
          <span class="hint csvall-cur" title={cur}>Procesando: {cur}</span>
        {/if}
        <Btn small onclick={onCancel}>Cancelar</Btn>
      {:else if matchable === 0}
        <span class="hint ctx-hint">Todas las pistas tienen match. Usa «Buscar otra versión» por pista si quieres cambiarlas.</span>
        <Btn small disabled>Emparejar todas</Btn>
      {:else}
        <span class="hint">{matchable} {matchable === 1 ? "pista sin match" : "pistas sin match"}</span>
        <Btn primary small onclick={onMatchAll}
          title="Busca reemplazo para las pistas marcadas como no disponibles (se conservan título, artista, álbum y duración)">Emparejar todas</Btn>
      {/if}
      <div class="ctx-more">
        <Btn small icon aria-haspopup="menu" aria-expanded={moreOpen} data-tip="Más acciones" data-tip-al="fin" data-tip-pos="abajo" aria-label="Más acciones"
          onclick={(e) => { e.stopPropagation(); moreOpen = !moreOpen; }}>
          <Icon16 name="dots" size={15} />
        </Btn>
        {#if moreOpen}
          <div class="menu ctx-menu" role="menu" aria-label="Más acciones">
            <button role="menuitem" onclick={() => { moreOpen = false; onDownload?.(); }}
              title="Descargar TODA la lista abierta (no solo las pistas visibles)"><Icon16 name="download" size={14} /> Descargar playlist</button>
          </div>
        {/if}
      </div>
    {/if}
  {/snippet}
</ContextBar>
