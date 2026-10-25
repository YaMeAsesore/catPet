<!-- U4/M4: popover «Agregar a playlist» de la interfaz nueva. Mismo contenido y
     acciones que el menú clásico (addTrackToPlist / newPlistWithTrack llegan
     como callbacks; aquí no se llama a api y el estado `menu` vive en App).
     Esc y clic fuera se gestionan en este componente. M4: iconos del set 1-bit
     (Kawaii1bit), sin mezcla con el set clásico. -->
<script lang="ts">
  import Kawaii from "./onebit/Kawaii1bit.svelte";
  import type { PlaylistInfo } from "../tauri";

  let {
    x,
    y,
    flip,
    vh,
    plists = [],
    onPick,
    onNew,
    onclose,
  }: {
    x: number;
    y: number;
    flip: boolean;
    vh: number;
    plists?: PlaylistInfo[];
    onPick: (name: string) => void;
    onNew: () => void;
    onclose: () => void;
  } = $props();

  let boxEl: HTMLDivElement | null = $state(null);

  // Teclado: el foco entra en la primera opción al abrir y vuelve al botón de
  // origen al cerrar (si sigue en el DOM). Solo presentación: mismo estado.
  $effect(() => {
    const prev = document.activeElement as HTMLElement | null;
    boxEl?.querySelector<HTMLButtonElement>("button")?.focus();
    return () => { prev?.focus?.(); };
  });

  function onWinClick(e: MouseEvent) {
    const t = e.target as HTMLElement | null;
    // Igual que el cierre global del clásico: clic en otro botón de fila lo
    // gestiona ese botón (toggleMenu), no este cierre.
    if (!t?.closest?.(".pop, .menu-btn")) onclose();
  }
  function onWinKey(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
  }
</script>

<svelte:window onclick={onWinClick} onkeydown={onWinKey} />

<div class="track-menu menu pop" role="menu" aria-label="Agregar a playlist" bind:this={boxEl}
  style="left:{x}px; top:{flip ? 'auto' : y + 'px'}; bottom:{flip ? (vh - y) + 'px' : 'auto'};">
  {#if plists.length === 0}
    <div class="menu-hint">Sin playlists todavía</div>
  {/if}
  {#each plists as p (p.name)}
    <button role="menuitem" onclick={() => onPick(p.name)}>
      <Kawaii name="playlist" size={15} /> {p.name} ({p.count})
    </button>
  {/each}
  <button role="menuitem" onclick={onNew}>
    <Kawaii name="plus" size={15} /> Nueva playlist…
  </button>
</div>
