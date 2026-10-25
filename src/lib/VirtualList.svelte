<script lang="ts">
  // Lista virtualizada mínima: solo monta las filas visibles + overscan.
  // Decisión: windowing manual porque 50k nodos DOM congelan el scroll; aquí ~30 nodos.
  import type { Snippet } from "svelte";
  let {
    items = [],
    rowHeight = 56,
    overscan = 8,
    row,
    onNearEnd,
    onRange,
  }: {
    items: any[];
    rowHeight?: number;
    overscan?: number;
    row: Snippet<[any, number]>;
    onNearEnd?: () => void; // scroll infinito: el padre pagina con OFFSET
    onRange?: (start: number, end: number) => void; // rango visible (solo al cambiar)
  } = $props();

  let scrollTop = $state(0);
  let height = $state(600);

  const total = $derived(items.length);
  const start = $derived(Math.max(0, Math.floor(scrollTop / rowHeight) - overscan));
  const visible = $derived(Math.ceil(height / rowHeight) + overscan * 2);
  const end = $derived(Math.min(total, start + visible));
  const slice = $derived(
    items.slice(start, end).map((it, k) => ({ it, i: start + k }))
  );

  // Avisa cuando faltan ~12 filas para el final (el padre decide si hay más páginas).
  $effect(() => {
    if (total > 0 && total * rowHeight - (scrollTop + height) < rowHeight * 12) {
      onNearEnd?.();
    }
  });

  // S3: comunica el rango visible SOLO cuando cambia (comparado con el último
  // emitido). Sin prop no hace nada; el scroll dentro de las mismas filas no
  // vuelve a avisar.
  let lastRange = "";
  $effect(() => {
    const key = `${start}:${end}`;
    if (onRange != null && key !== lastRange) {
      lastRange = key;
      onRange(start, end);
    }
  });
</script>

<div
  class="virt"
  bind:clientHeight={height}
  onscroll={(e) => (scrollTop = (e.currentTarget as HTMLDivElement).scrollTop)}
>
  <div style={`height:${total * rowHeight}px; position:relative;`}>
    {#each slice as { it, i } (i)}
      <div style={`position:absolute; top:${i * rowHeight}px; left:0; right:0; height:${rowHeight}px;`}>
        {@render row(it, i)}
      </div>
    {/each}
  </div>
</div>
