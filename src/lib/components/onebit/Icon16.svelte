<!-- M1: iconos originales 16×16 «Mac 1-bit» — dibujados para este proyecto
     (sin trazos de terceros ni marcas); heredan el color con currentColor y se
     alinean a la rejilla con shape-rendering: crispEdges. Nombres compatibles
     con el set anterior (Icon.svelte) más los nuevos: playlist, queue,
     volume, mute, remove/x, connect, logout, settings, yt, check y search.
     Alias: plus = add, x = remove (históricos). -->
<script lang="ts">
  let { name, size = 16 }: { name: string; size?: number } = $props();
</script>

<svg
  width={size} height={size} viewBox="0 0 16 16" fill="none"
  shape-rendering="crispEdges" aria-hidden="true" focusable="false"
>
  {#if name === "play"}
    <path d="M5 3l8 5-8 5z" fill="currentColor" />
  {:else if name === "pause"}
    <rect x="4" y="3" width="3" height="10" fill="currentColor" />
    <rect x="9" y="3" width="3" height="10" fill="currentColor" />
  {:else if name === "next"}
    <path d="M4 3l6 5-6 5z" fill="currentColor" />
    <rect x="11" y="3" width="2" height="10" fill="currentColor" />
  {:else if name === "prev"}
    <path d="M12 3l-6 5 6 5z" fill="currentColor" />
    <rect x="3" y="3" width="2" height="10" fill="currentColor" />
  {:else if name === "plus" || name === "add"}
    <rect x="7" y="3" width="2" height="10" fill="currentColor" />
    <rect x="3" y="7" width="10" height="2" fill="currentColor" />
  {:else if name === "trash"}
    <rect x="6" y="2" width="4" height="2" fill="currentColor" />
    <rect x="3" y="4" width="10" height="2" fill="currentColor" />
    <path d="M4 7h8l-1 7H5z" fill="currentColor" />
  {:else if name === "folder"}
    <path d="M2 4h5l2 2h5v8H2z" fill="currentColor" />
  {:else if name === "disc"}
    <path
      fill-rule="evenodd"
      d="M6 2h4v1h2v1h1v2h1v4h-1v2h-1v1h-2v1H6v-1H4v-1H3V9H2V5h1V4h1V3h2zM7 6h2v1h1v2h-1v1H7V9H6V7h1z"
      fill="currentColor"
    />
  {:else if name === "note"}
    <rect x="7" y="2" width="2" height="8" fill="currentColor" />
    <path d="M9 2h4v5h-2V4H9z" fill="currentColor" />
    <rect x="4" y="10" width="5" height="4" fill="currentColor" />
  {:else if name === "radio"}
    <rect x="3" y="2" width="2" height="2" fill="currentColor" />
    <rect x="2" y="5" width="12" height="9" stroke="currentColor" stroke-width="2" />
    <rect x="4" y="8" width="4" height="3" fill="currentColor" />
    <rect x="10" y="8" width="2" height="2" fill="currentColor" />
  {:else if name === "server"}
    <rect x="2" y="3" width="12" height="4" stroke="currentColor" stroke-width="2" />
    <rect x="2" y="9" width="12" height="4" stroke="currentColor" stroke-width="2" />
    <rect x="4" y="4" width="2" height="2" fill="currentColor" />
    <rect x="4" y="10" width="2" height="2" fill="currentColor" />
  {:else if name === "mic"}
    <rect x="6" y="2" width="4" height="7" stroke="currentColor" stroke-width="2" />
    <rect x="7" y="9" width="2" height="3" fill="currentColor" />
    <rect x="5" y="12" width="6" height="2" fill="currentColor" />
  {:else if name === "sliders"}
    <rect x="3" y="3" width="10" height="1" fill="currentColor" />
    <rect x="5" y="2" width="2" height="3" fill="currentColor" />
    <rect x="3" y="8" width="10" height="1" fill="currentColor" />
    <rect x="9" y="7" width="2" height="3" fill="currentColor" />
    <rect x="3" y="13" width="10" height="1" fill="currentColor" />
    <rect x="4" y="12" width="2" height="3" fill="currentColor" />
  {:else if name === "sun"}
    <rect x="6" y="6" width="4" height="4" fill="currentColor" />
    <rect x="7" y="2" width="2" height="2" fill="currentColor" />
    <rect x="7" y="12" width="2" height="2" fill="currentColor" />
    <rect x="2" y="7" width="2" height="2" fill="currentColor" />
    <rect x="12" y="7" width="2" height="2" fill="currentColor" />
    <rect x="3" y="3" width="2" height="2" fill="currentColor" />
    <rect x="11" y="3" width="2" height="2" fill="currentColor" />
    <rect x="3" y="11" width="2" height="2" fill="currentColor" />
    <rect x="11" y="11" width="2" height="2" fill="currentColor" />
  {:else if name === "moon"}
    <path
      fill-rule="evenodd"
      d="M7 1h4v1h2v1h1v2h1v6h-1v2h-1v1h-2v1H7v-1H5v-1H4V9H3V5h1V3h1V2h2zM8 3h3v1h1v3h-1v2h-1v1h-2V9h1V7h1V5H8z"
      fill="currentColor"
    />
  {:else if name === "alert"}
    <path
      fill-rule="evenodd"
      d="M8 2l6 12H2zM7 6h2v4H7zM7 11h2v2H7z"
      fill="currentColor"
    />
  {:else if name === "lock"}
    <path d="M6 3h4v1h1v3h-2V5H7v2H5V4h1z" fill="currentColor" />
    <rect x="4" y="7" width="8" height="6" fill="currentColor" />
  {:else if name === "link"}
    <path d="M6 4H4v8h2M10 4h2v8h-2M4 8h8" stroke="currentColor" stroke-width="2" />
  {:else if name === "list"}
    <rect x="2" y="3" width="2" height="2" fill="currentColor" />
    <rect x="6" y="3" width="8" height="2" fill="currentColor" />
    <rect x="2" y="8" width="2" height="2" fill="currentColor" />
    <rect x="6" y="8" width="8" height="2" fill="currentColor" />
    <rect x="2" y="13" width="2" height="2" fill="currentColor" />
    <rect x="6" y="13" width="8" height="2" fill="currentColor" />
  {:else if name === "download"}
    <rect x="7" y="2" width="2" height="7" fill="currentColor" />
    <path d="M4 7l4 4 4-4z" fill="currentColor" />
    <rect x="3" y="13" width="10" height="2" fill="currentColor" />
  {:else if name === "dots"}
    <rect x="2" y="7" width="2" height="2" fill="currentColor" />
    <rect x="7" y="7" width="2" height="2" fill="currentColor" />
    <rect x="12" y="7" width="2" height="2" fill="currentColor" />
  {:else if name === "grip"}
    <rect x="4" y="3" width="2" height="2" fill="currentColor" />
    <rect x="10" y="3" width="2" height="2" fill="currentColor" />
    <rect x="4" y="7" width="2" height="2" fill="currentColor" />
    <rect x="10" y="7" width="2" height="2" fill="currentColor" />
    <rect x="4" y="11" width="2" height="2" fill="currentColor" />
    <rect x="10" y="11" width="2" height="2" fill="currentColor" />
  {:else if name === "pencil"}
    <path d="M2 14l1-4 7-7 3 3-7 7z" fill="currentColor" />
  {:else if name === "chevD"}
    <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="2" />
  {:else if name === "chevL"}
    <path d="M10 4L6 8l4 4" stroke="currentColor" stroke-width="2" />
  {:else if name === "chevR"}
    <path d="M6 4l4 4-4 4" stroke="currentColor" stroke-width="2" />
  {:else if name === "chevU"}
    <path d="M4 10l4-4 4 4" stroke="currentColor" stroke-width="2" />
  {:else if name === "search"}
    <circle cx="7" cy="7" r="4.5" stroke="currentColor" stroke-width="2" />
    <path d="M10.5 10.5L14 14" stroke="currentColor" stroke-width="2" />
  {:else if name === "playlist"}
    <rect x="2" y="3" width="2" height="2" fill="currentColor" />
    <rect x="6" y="3" width="8" height="2" fill="currentColor" />
    <rect x="2" y="8" width="2" height="2" fill="currentColor" />
    <rect x="6" y="8" width="8" height="2" fill="currentColor" />
    <path d="M10 10l5 2.5-5 2.5z" fill="currentColor" />
  {:else if name === "queue"}
    <rect x="2" y="3" width="12" height="2" fill="currentColor" />
    <rect x="2" y="7" width="12" height="2" fill="currentColor" />
    <rect x="2" y="11" width="7" height="2" fill="currentColor" />
    <path d="M10 10l5 2.5-5 2.5z" fill="currentColor" />
  {:else if name === "volume"}
    <path d="M3 6h3l4-3v10l-4-3H3z" fill="currentColor" />
    <path d="M11 5c1.5 1.5 1.5 4.5 0 6" stroke="currentColor" stroke-width="1.5" />
    <path d="M13 3.5c2.5 2.5 2.5 6.5 0 9" stroke="currentColor" stroke-width="1.5" />
  {:else if name === "mute"}
    <path d="M3 6h3l4-3v10l-4-3H3z" fill="currentColor" />
    <path d="M11 6l4 4M15 6l-4 4" stroke="currentColor" stroke-width="2" />
  {:else if name === "remove" || name === "x"}
    <path d="M3 3l10 10M13 3L3 13" stroke="currentColor" stroke-width="2" />
  {:else if name === "connect"}
    <rect x="1" y="7" width="3" height="2" fill="currentColor" />
    <rect x="4" y="5" width="6" height="6" fill="currentColor" />
    <rect x="10" y="6" width="3" height="1" fill="currentColor" />
    <rect x="10" y="9" width="3" height="1" fill="currentColor" />
  {:else if name === "logout"}
    <path d="M3 2h5v12H3" stroke="currentColor" stroke-width="2" />
    <rect x="9" y="7" width="3" height="2" fill="currentColor" />
    <path d="M12 5l3 3-3 3z" fill="currentColor" />
  {:else if name === "settings"}
    <rect x="6" y="6" width="4" height="4" fill="currentColor" />
    <rect x="7" y="2" width="2" height="3" fill="currentColor" />
    <rect x="7" y="11" width="2" height="3" fill="currentColor" />
    <rect x="2" y="7" width="3" height="2" fill="currentColor" />
    <rect x="11" y="7" width="3" height="2" fill="currentColor" />
    <rect x="3" y="3" width="2" height="2" fill="currentColor" />
    <rect x="11" y="3" width="2" height="2" fill="currentColor" />
    <rect x="3" y="11" width="2" height="2" fill="currentColor" />
    <rect x="11" y="11" width="2" height="2" fill="currentColor" />
  {:else if name === "yt"}
    <rect x="1" y="3" width="14" height="10" stroke="currentColor" stroke-width="2" />
    <path d="M6 6l4 2-4 2z" fill="currentColor" />
  {:else if name === "check"}
    <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" />
  {/if}
</svg>
