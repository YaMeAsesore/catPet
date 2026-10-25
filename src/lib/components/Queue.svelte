<!-- U1: cola de reproducción (marcado movido ÍNTEGRO desde App.svelte:
     mismos textos, clases y accesibilidad). Sin lógica propia: recibe datos y
     callbacks del padre. El arrastre (punteros) y el flag anti-clic siguen en
     App.svelte: aquí solo se llaman.
     Piel 1-bit (iconos 16×16 y kawaii monocromo) siempre activa. -->
<script lang="ts">
  import IcoAdapt from "./onebit/IcoAdapt.svelte";
  import KawaiiAdapt from "./onebit/KawaiiAdapt.svelte";
  import { flip } from "svelte/animate";
  import type { Track } from "../tauri";

  let {
    items = [],
    current = null,
    min = false,
    dropIndex = null,
    reduced = false,
    listEl = $bindable<HTMLOListElement | null>(null),
    onToggle,
    onPlay,
    onGrip,
    onMove,
    onRemove,
    onClear,
  }: {
    items?: Track[];
    current?: number | null;
    min?: boolean;
    dropIndex?: number | null;
    reduced?: boolean;
    listEl?: HTMLOListElement | null;
    onToggle: () => void;
    onPlay: (i: number) => void;
    onGrip: (e: PointerEvent, i: number) => void;
    onMove: (i: number, dir: -1 | 1) => void;
    onRemove: (e: MouseEvent | KeyboardEvent, i: number) => void;
    onClear: () => void;
  } = $props();

  // P4-03: la cola monta solo la ventana visible (misma técnica que
  // VirtualList: dos huecos con el alto de las filas no montadas). Con 20 000
  // pistas el DOM pasa de 20 000 <li> a ~20; el arrastre no cambia porque App
  // calcula el índice con la primera fila montada y su alto uniforme.
  let scrollTop = $state(0);
  let clientH = $state(0);
  let rowH = $state(33);
  const total = $derived(items.length);
  // El hueco superior nunca puede pasar del total: al vaciar o acortar la cola
  // con scroll abajo, un `desde` mayor que `total` dejaba altura fantasma.
  const desde = $derived(Math.min(total, Math.max(0, Math.floor(scrollTop / rowH) - 6)));
  const hasta = $derived(Math.min(total, desde + Math.ceil(clientH / rowH) + 12));
  const visibles = $derived(items.slice(desde, hasta).map((t, k) => ({ t, i: desde + k })));

  // Alto real de fila (uniforme por CSS): se mide de la primera fila montada.
  $effect(() => {
    if (items.length === 0) return;
    const li = listEl?.querySelector<HTMLElement>("li[data-i]");
    if (!li) return;
    const h = li.getBoundingClientRect().height;
    if (h > 0 && Math.abs(h - rowH) > 0.5) rowH = h;
  });
</script>

<aside class="queue">
  <!-- MEJORA 6: cabecera siempre visible con el toggle minimizar/expandir. -->
  <div class="queue-head">
    <button class="icon small" onclick={onToggle}
      title={min ? "Expandir cola" : "Minimizar cola"} aria-expanded={!min}>
      <IcoAdapt name={min ? "chevL" : "chevR"} size={16} />
    </button>
    <span class="hint"><KawaiiAdapt name="queue" size={17} /> Cola ({items.length}) — arrastra para reordenar</span>
  </div>
  <div class="queue-body" inert={min}>
  {#if items.length === 0}
    <div class="queue-empty">
      <KawaiiAdapt name="happy" size={48} />
      <span>Cola vacía: haz clic en una canción</span>
    </div>
  {/if}
  <ol bind:this={listEl} bind:clientHeight={clientH}
    role="list" aria-label="Cola de reproducción"
    onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}>
    {#if desde > 0}
      <li class="q-gap" style={`height:${desde * rowH}px`} aria-hidden="true"></li>
    {/if}
    {#each visibles as { t, i } (t.path + i)}
      <li
        data-i={i}
        aria-setsize={total}
        aria-posinset={i + 1}
        class:cur={i === current}
        class:drop-target={dropIndex === i}
        class:drop-end={dropIndex === items.length && i === items.length - 1}
        animate:flip={{ duration: reduced ? 0 : 200 }}
        title="{t.title} — {t.artist}"
      >
        <span class="qnum">{i + 1}</span>
        <button type="button" class="qgrip" title="Arrastrar para reordenar" aria-label="Arrastrar para reordenar"
          onpointerdown={(e) => onGrip(e, i)}><IcoAdapt name="grip" size={13} /></button>
        <!-- T6: botón real para reproducir (los botones subir/bajan ya
             daban alternativa de teclado al arrastre). -->
        <button type="button" class="qt qt-btn" title="{t.title} — {t.artist}"
          onclick={() => onPlay(i)}>{t.title}</button>
        <span class="qacts">
          <button onclick={(e) => { e.stopPropagation(); onMove(i, -1); }} title="Subir" disabled={i === 0}><IcoAdapt name="chevU" size={13} /></button>
          <button onclick={(e) => { e.stopPropagation(); onMove(i, 1); }} title="Bajar" disabled={i === items.length - 1}><IcoAdapt name="chevD" size={13} /></button>
          <button onclick={(e) => onRemove(e, i)} onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); onRemove(e, i); } }} title="Quitar de la cola"><KawaiiAdapt name="remove" size={15} /></button>
        </span>
      </li>
    {/each}
    {#if hasta < total}
      <li class="q-gap" style={`height:${(total - hasta) * rowH}px`} aria-hidden="true"></li>
    {/if}
  </ol>
  <div style="padding:8px; display:flex; gap:8px;">
    <button onclick={onClear}>Limpiar</button>
  </div>
  </div><!-- /.queue-body -->
</aside>