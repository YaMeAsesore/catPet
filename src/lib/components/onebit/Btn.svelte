<!-- M1: botón base 1-bit (envoltorio fino del <button> global): variantes
     normal/primary/…; sin lógica. El comportamiento (hover invertido, foco
     con anillo discontinuo y esquinas rectas) viene de styles.css bajo
     body.onebit. M3: variante `icon` (cuadrada, solo icono) para los «⋯» de
     las cabeceras de contexto. -->
<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";

  let { primary = false, small = false, icon = false, title = undefined, disabled = false, onclick, children, el = $bindable(null), ...rest }:
    HTMLButtonAttributes & {
      primary?: boolean;
      small?: boolean;
      icon?: boolean;
      disabled?: boolean;
      children?: Snippet;
      /** M4: el <button> real, para devolver el foco (diálogos de confirmación). */
      el?: HTMLButtonElement | null;
    } = $props();
</script>

<button class:primary class:small class:icon {title} {disabled} {onclick} type="button" bind:this={el} {...rest}>
  {@render children?.()}
</button>
