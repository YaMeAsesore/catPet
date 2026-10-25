<!-- Set de iconos propio estilo kawaii (trazo redondeado coherente).
  Dibujados a mano para este proyecto: sin dependencias ni licencias de terceros.
  Heredan el color del tema vía currentColor; hover/activo viajan con el botón. -->
<script lang="ts">
  let { name, size = 18 }: { name: string; size?: number } = $props();
</script>

<svg
  width={size} height={size} viewBox="0 0 24 24" fill="none"
  stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"
  aria-hidden="true" focusable="false"
>
  {#if name === "play"}
    <path d="M8 5.2v13.6L18.2 12z" fill="currentColor" stroke="none" />
  {:else if name === "pause"}
    <rect x="6.4" y="5" width="4.2" height="14" rx="2" fill="currentColor" stroke="none" />
    <rect x="13.4" y="5" width="4.2" height="14" rx="2" fill="currentColor" stroke="none" />
  {:else if name === "next"}
    <path d="M6 5.5v13l7.5-6.5z" fill="currentColor" stroke="none" />
    <path d="M17 5.5v13" />
  {:else if name === "prev"}
    <path d="M18 5.5v13l-7.5-6.5z" fill="currentColor" stroke="none" />
    <path d="M7 5.5v13" />
  {:else if name === "plus"}
    <path d="M12 5v14M5 12h14" />
  {:else if name === "trash"}
    <path d="M4 7h16M10 5h4M6.5 7l1 13.5h9l1-13.5M10 11v6M14 11v6" />
  {:else if name === "pencil"}
    <path d="M4 20l1-4L16.5 4.5a2.1 2.1 0 013 3L8 19z" />
    <path d="M14.5 6.5l3 3" />
  {:else if name === "vol-mute"}
    <path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5H4z" />
    <path d="M16 9.5l5 5M21 9.5l-5 5" />
  {:else if name === "vol-low"}
    <path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5H4z" />
    <path d="M15.5 9.5a4 4 0 010 5" />
  {:else if name === "vol-mid"}
    <path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5H4z" />
    <path d="M15.5 9.5a4 4 0 010 5M18 7a8 8 0 010 10" />
  {:else if name === "vol-high"}
    <path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5H4z" />
    <path d="M15.5 9.5a4 4 0 010 5M18 7a8 8 0 010 10M20.5 4.5a12 12 0 010 15" />
  {:else if name === "sun"}
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2.5V5M12 19v2.5M2.5 12H5M19 12h2.5M4.9 4.9l1.8 1.8M17.3 17.3l1.8 1.8M19.1 4.9l-1.8 1.8M6.7 17.3l-1.8 1.8" />
  {:else if name === "moon"}
    <path d="M20 13.5A8 8 0 1110.5 4a6.8 6.8 0 009.5 9.5z" />
  {:else if name === "note"}
    <path d="M9.5 17.5V6l10-2.2V15" />
    <circle cx="6.8" cy="17.5" r="2.7" />
    <circle cx="16.8" cy="15" r="2.7" />
  {:else if name === "disc"}
    <circle cx="12" cy="12" r="8.5" />
    <circle cx="12" cy="12" r="2" fill="currentColor" stroke="none" />
  {:else if name === "alert"}
    <path d="M12 3.5L2.5 20h19z" />
    <path d="M12 9.5V14M12 17h.01" />
  {:else if name === "lock"}
    <rect x="5" y="10.5" width="14" height="9.5" rx="2.5" />
    <path d="M8 10.5V8a4 4 0 018 0v2.5" />
  {:else if name === "sliders"}
    <path d="M4 7h16M4 12h16M4 17h16" />
    <circle cx="15" cy="7" r="2.2" fill="currentColor" stroke="none" />
    <circle cx="9" cy="12" r="2.2" fill="currentColor" stroke="none" />
    <circle cx="16" cy="17" r="2.2" fill="currentColor" stroke="none" />
  {:else if name === "chevL"}
    <path d="M14.5 5.5L8 12l6.5 6.5" />
  {:else if name === "chevR"}
    <path d="M9.5 5.5L16 12l-6.5 6.5" />
  {:else if name === "chevU"}
    <path d="M5.5 14.5L12 8l6.5 6.5" />
  {:else if name === "chevD"}
    <path d="M5.5 9.5L12 16l6.5-6.5" />
  {:else if name === "grip"}
    <circle cx="9" cy="6" r="1.3" fill="currentColor" stroke="none" />
    <circle cx="15" cy="6" r="1.3" fill="currentColor" stroke="none" />
    <circle cx="9" cy="12" r="1.3" fill="currentColor" stroke="none" />
    <circle cx="15" cy="12" r="1.3" fill="currentColor" stroke="none" />
    <circle cx="9" cy="18" r="1.3" fill="currentColor" stroke="none" />
    <circle cx="15" cy="18" r="1.3" fill="currentColor" stroke="none" />
  {:else if name === "dots"}
    <circle cx="5" cy="12" r="1.6" fill="currentColor" stroke="none" />
    <circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none" />
    <circle cx="19" cy="12" r="1.6" fill="currentColor" stroke="none" />
  {:else if name === "download"}
    <path d="M12 4.5v9.5M8 10.5l4 4 4-4" />
    <path d="M5.5 18.5h13" />
  {:else if name === "list"}
    <path d="M9.5 6.5h10M9.5 12h10M9.5 17.5h10" />
    <circle cx="5.2" cy="6.5" r="1.3" fill="currentColor" stroke="none" />
    <circle cx="5.2" cy="12" r="1.3" fill="currentColor" stroke="none" />
    <circle cx="5.2" cy="17.5" r="1.3" fill="currentColor" stroke="none" />
  {:else if name === "link"}
    <path d="M10.4 13.6a4.3 4.3 0 006.1 0l2.9-2.9a4.3 4.3 0 00-6.1-6.1l-1.4 1.4" />
    <path d="M13.6 10.4a4.3 4.3 0 00-6.1 0l-2.9 2.9a4.3 4.3 0 006.1 6.1l1.4-1.4" />
  {:else if name === "folder"}
    <path d="M4 7.5a2.2 2.2 0 012.2-2.2h3.4l1.9 2h6.3a2.2 2.2 0 012.2 2.2v6.8a2.2 2.2 0 01-2.2 2.2H6.2A2.2 2.2 0 014 16.3z" />
  {:else if name === "radio"}
    <circle cx="12" cy="12" r="1.7" fill="currentColor" stroke="none" />
    <path d="M8.6 15.4a4.8 4.8 0 010-6.8M15.4 8.6a4.8 4.8 0 010 6.8M6.2 17.8a8.2 8.2 0 010-11.6M17.8 6.2a8.2 8.2 0 010 11.6" />
  {:else if name === "mic"}
    <rect x="9.3" y="3.8" width="5.4" height="9.8" rx="2.7" />
    <path d="M6.4 11.2a5.6 5.6 0 0011.2 0M12 16.8v3.4" />
  {:else if name === "server"}
    <rect x="4" y="4.6" width="16" height="6" rx="2" />
    <rect x="4" y="13.4" width="16" height="6" rx="2" />
    <circle cx="7.8" cy="7.6" r="1" fill="currentColor" stroke="none" />
    <circle cx="7.8" cy="16.4" r="1" fill="currentColor" stroke="none" />
  {:else}
    <circle cx="12" cy="12" r="8" />
  {/if}
</svg>
