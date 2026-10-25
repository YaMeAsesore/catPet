<!-- M4: ventana modal 1-bit reutilizable — scrim + `Window` (borde 2 px,
     sombra dura, franja y chip opcionales) + «×» de cierre en la barra + Esc.
     Solo presentación: el estado y los handlers siguen en el padre; aquí no se
     llama a api. La entrada/salida usa fade en el scrim y scale en la ventana
     (duración 0 con prefers-reduced-motion). `role` permite alertdialog y
     `focoCerrar` reproduce el foco inicial del × que tenía Ajustes. -->
<script lang="ts">
  import { fade, scale } from "svelte/transition";
  import type { Snippet } from "svelte";
  import Window from "./Window.svelte";
  import Icon16 from "./Icon16.svelte";

  let {
    title,
    label = null,
    icon = null,
    sub = null,
    ancho = 420,
    role = "dialog",
    focoCerrar = false,
    onclose,
    actions,
    children,
  }: {
    title: string;
    label?: string | null;
    icon?: string | null;
    sub?: string | null;
    /** Ancho base de la ventana en px (nunca supera 94vw). */
    ancho?: number;
    role?: "dialog" | "alertdialog";
    focoCerrar?: boolean;
    onclose: () => void;
    actions?: Snippet;
    children?: Snippet;
  } = $props();

  const RM =
    typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  let xEl: HTMLButtonElement | null = $state(null);
  $effect(() => { if (focoCerrar) xEl?.focus(); });

  // Esc cierra, como en los diálogos de U2/U3 (sin tocar onKey de App.svelte).
  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="ob-scrim" transition:fade={{ duration: RM ? 0 : 200 }}>
  <!-- Svelte 5 no admite `transition:` en componentes: el scale va en un
       envoltorio de una sola pieza que ajusta al tamaño de la ventana. -->
  <div class="ob-dlg-anim" transition:scale={{ duration: RM ? 0 : 200, start: 0.97 }}>
    <Window {title} {icon} {sub} label={label ?? title} {role} ariaModal extraClass="ob-dlg"
      style={`--ob-dlg-w:${ancho}px`}>
      {#snippet actions()}
        {@render actions?.()}
        <button class="ob-x" bind:this={xEl} onclick={onclose} title="Cerrar (Esc)" aria-label="Cerrar (Esc)">
          <Icon16 name="remove" size={14} />
        </button>
      {/snippet}
      {@render children?.()}
    </Window>
  </div>
</div>
