<!-- M3: contexto «YouTube» (import por link, fuente guardada o YouTube
     Music): cabecera «Mac 1-bit» con la ventana de ContextBar (rayas +
     icono yt + título + «N de M pistas»). Re-hogariza EXACTAMENTE los
     controles de la barra clásica de YouTube: acción principal «Reproducir
     todo» (solo con pistas mostradas), aviso ⚠ del lote, y las secundarias
     en «⋯» («Guardar fuente», «Descargar playlist»); «Cerrar» queda visible.
     LOS LOTES: la carga del primer lote sigue en el área de la lista
     (.yt-loading, contador N/M) y «Cargar más» al FINAL de la lista (regla
     W0-7): aquí solo se refleja el progreso del lote en curso. Los puntos de
     estado por pista (.rdy) viven en la fila compartida. Sin lógica: estado y
     handlers vienen de App.svelte. -->
<script lang="ts">
  import ContextBar from "./ContextBar.svelte";
  import Btn from "../onebit/Btn.svelte";
  import Icon16 from "../onebit/Icon16.svelte";
  import KawaiiAdapt from "../onebit/KawaiiAdapt.svelte";

  let {
    title,
    subtitle,
    shown = 0,
    hasImport = false,
    loading = false,
    loadingText = "",
    warning = null,
    canSave = false,
    saved = null,
    saving = false,
    onPlayAll,
    onSaveSource,
    onDownload,
    onClose,
    onToggleSaved,
  }: {
    title: string;
    subtitle: string;
    shown?: number;
    hasImport?: boolean;
    loading?: boolean;
    loadingText?: string;
    warning?: string | null;
    canSave?: boolean;
    saved?: boolean | null;
    saving?: boolean;
    onPlayAll?: () => void;
    onSaveSource?: () => void;
    onDownload?: () => void;
    onClose?: () => void;
    onToggleSaved?: () => void;
  } = $props();

  // Menú «⋯» propio: cierra con clic fuera o Esc; con el menú cerrado no hay
  // oyentes (Espacio/N/P/flechas/Ctrl+K siguen siendo de la app). Esc suelta
  // además el foco para que Espacio/N/P vuelvan a la app.
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

<ContextBar ctx="youtube" {title} subtitle={subtitle}>
  {#snippet actions()}
    {#if loading}
      <span class="hint" title="Traer y preparar el siguiente lote">Preparando lote… {loadingText}</span>
    {/if}
    {#if warning}
      <span class="ctx-warn" title={warning}>⚠ {warning}</span>
    {/if}
    {#if shown > 0}
      <Btn primary small onclick={onPlayAll} title="Mandar a la cola y reproducir">Reproducir todo</Btn>
    {/if}
    {#if canSave}
      <Btn small disabled={saving} onclick={onToggleSaved}
        title={saved === true
          ? "Quitar esta playlist de tu biblioteca de YouTube Music"
          : "Guardar esta playlist en tu biblioteca de YouTube Music"}>
        <KawaiiAdapt name={saved === true ? "heart-fill" : "heart"} size={15} />
        {saving ? "Guardando…" : saved === true ? "Guardada" : "Guardar"}
      </Btn>
    {/if}
    {#if hasImport}
      <div class="ctx-more">
        <Btn small icon aria-haspopup="menu" aria-expanded={moreOpen} data-tip="Más acciones" data-tip-al="fin" data-tip-pos="abajo" aria-label="Más acciones"
          onclick={(e) => { e.stopPropagation(); moreOpen = !moreOpen; }}>
          <Icon16 name="dots" size={15} />
        </Btn>
        {#if moreOpen}
          <div class="menu ctx-menu" role="menu" aria-label="Más acciones">
            <button role="menuitem" onclick={() => { moreOpen = false; onSaveSource?.(); }}
              title="Guardar como fuente"><Icon16 name="playlist" size={14} /> Guardar fuente</button>
            <button role="menuitem" onclick={() => { moreOpen = false; onDownload?.(); }}
              title="Descargar TODA la playlist abierta (no solo las pistas visibles)"><Icon16 name="download" size={14} /> Descargar playlist</button>
          </div>
        {/if}
      </div>
      <Btn small onclick={onClose} title="Cerrar vista">Cerrar</Btn>
    {/if}
  {/snippet}
</ContextBar>
