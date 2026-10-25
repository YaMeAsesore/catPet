<!-- M1: cabecera contextual en el estilo «Mac 1-bit». M3: es una `Window`
     (borde de 2 px, sombra dura, franja de rayas del contexto y chip con el
     icono 16×16 original): el contexto se distingue por icono + rayas + texto,
     nunca por color. V1: detección de contexto en App.svelte. V2: los controles
     re-hogarizados de cada contexto llegan por el snippet opcional `actions`
     (una acción principal + secundarias); aquí no hay lógica: el estado y los
     handlers viven en App.svelte o en el componente Ctx* de cada contexto. -->
<script lang="ts">
  import type { Snippet } from "svelte";
  import Window from "../onebit/Window.svelte";

  let { ctx, title, subtitle, actions }: {
    ctx: "musica" | "youtube" | "radios" | "servidores" | "spotify";
    title: string;
    subtitle: string;
    actions?: Snippet;
  } = $props();

  // Icono del chip por contexto (nombres del set 1-bit; ninguno lleva color).
  const ICONO = {
    musica: "disc",
    youtube: "yt",
    radios: "radio",
    servidores: "server",
    spotify: "note",
  } as const;
</script>

<Window {title} sub={subtitle} pattern={ctx} icon={ICONO[ctx]} label="Contexto: {title}" extraClass="ctxbar c-{ctx}">
  {#snippet actions()}{@render actions?.()}{/snippet}
</Window>
