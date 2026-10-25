<!-- M1: componente base «ventana» 1-bit: borde fino (2 px), franja de título
     con el patrón de rayas del contexto (opcional) y sombra dura (sin
     desenfoque). Presentación pura: no gestiona estado ni lógica. Los tokens
     (--ink/--paper/--stripe-*) viven en styles.css bajo body.onebit.
     M3: acepta icono (chip 16×16), subtítulo y clase extra para que TODAS las
     cabeceras de contexto (ContextBar) sean la misma ventana; sin icono/sub el
     aspecto es idéntico al de M1. -->
<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon16 from "./Icon16.svelte";

  // M4: props para reutilizarla como ventana de diálogo (role/aria-modal y
  // estilo puntual, p. ej. el ancho); con los valores por defecto el aspecto y
  // el marcado son idénticos a los de M1/M3 (ContextBar).
  let { title, pattern = null, icon = null, sub = null, label = null, actions, children, extraClass = "", role = null, ariaModal = false, style = null }: {
    title: string;
    pattern?: "musica" | "youtube" | "radios" | "servidores" | "spotify" | null;
    icon?: string | null;
    sub?: string | null;
    label?: string | null;
    actions?: Snippet;
    children?: Snippet;
    extraClass?: string;
    role?: "dialog" | "alertdialog" | null;
    ariaModal?: boolean;
    style?: string | null;
  } = $props();
</script>

<section class="ob-win {extraClass}" data-pattern={pattern ?? undefined} aria-label={label ?? undefined}
  role={role ?? undefined} aria-modal={ariaModal ? "true" : undefined} {style}>
  <div class="ob-winhead">
    <div class="ob-stripe"></div>
    <div class="ob-bar">
      {#if icon}<span class="ob-chip" aria-hidden="true"><Icon16 name={icon} size={20} /></span>{/if}
      <span class="ob-bar-txt">
        <span class="ob-bar-title txt" title={title}>{title}</span>
        {#if sub}<span class="ob-bar-sub txt" title={sub}>{sub}</span>{/if}
      </span>
      {#if actions}<span class="ob-bar-acts">{@render actions()}</span>{/if}
    </div>
  </div>
  {#if children}<div class="ob-body">{@render children()}</div>{/if}
</section>

<style>
  .ob-win {
    border: 2px solid var(--ink);
    background: var(--paper);
    box-shadow: var(--shadow-hard);
    color: var(--ink);
  }
  .ob-winhead { border-bottom: 2px solid var(--ink); }
  .ob-stripe { height: 8px; background-image: var(--ob-stripes, none); }
  .ob-bar { display: flex; align-items: center; gap: 8px; padding: 4px 8px; }
  .ob-win[data-pattern="musica"] { --ob-stripes: var(--stripe-musica); }
  .ob-win[data-pattern="youtube"] { --ob-stripes: var(--stripe-youtube); }
  .ob-win[data-pattern="radios"] { --ob-stripes: var(--stripe-radios); }
  .ob-win[data-pattern="servidores"] { --ob-stripes: var(--stripe-servidores); }
  .ob-win[data-pattern="spotify"] { --ob-stripes: var(--stripe-spotify); }
  /* Chip del icono 16×16: el contexto se distingue por icono + rayas + texto,
     nunca por color (M1/M3). */
  .ob-chip {
    width: 32px; height: 32px; flex: none;
    display: flex; align-items: center; justify-content: center;
    background: var(--paper); border: 2px solid var(--ink); color: var(--ink);
  }
  .ob-bar-txt { min-width: 0; flex: 1; display: flex; flex-direction: column; }
  .ob-bar-title {
    font-family: var(--font-pixel);
    font-weight: 700; font-size: 14px;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .ob-bar-sub {
    font-size: 12.5px; color: var(--muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .ob-bar-acts {
    margin-left: auto; display: flex; align-items: center;
    flex-wrap: wrap; justify-content: flex-end; gap: 6px;
  }
  .ob-body { padding: 8px; }
</style>
