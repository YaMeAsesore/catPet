<!-- Marca de Música: vinilo + play (imágenes 1 y 2).
     Se incrusta en línea para heredar el color del tema (tinta/papel). A tamaño
     pequeño usa una reducción de 3 aros con trazo grueso; a partir de 40 px, el
     vinilo completo. -->
<script lang="ts">
  import logoSvg from "../../assets/brand/logo.svg?raw";
  import logoTinySvg from "../../assets/brand/logo-small.svg?raw";

  let {
    size = 24,
    strokeWidth = 0,
    title = "Música",
  }: { size?: number; strokeWidth?: number; title?: string } = $props();

  // A tamaño pequeño el trazo original (13/512) desaparece y las ondas se
  // emborronan: se usa la cabeza sólida. A partir de 40 px cabe el emblema.
  const tiny = $derived(size < 40);
  // A tamaño medio se engrosa un poco para conservar ~1,4 px en pantalla.
  const sw = $derived(strokeWidth > 0 ? strokeWidth : Math.max(13, 720 / size));
</script>

<span
  class="brand-mark"
  style={`width:${size}px;height:${size}px;--brand-sw:${sw}`}
  role={title ? "img" : undefined}
  aria-hidden={title ? undefined : "true"}
  aria-label={title || undefined}
>
  {#if tiny}{@html logoTinySvg}{:else}{@html logoSvg}{/if}
</span>

<style>
  .brand-mark {
    display: inline-flex;
    flex: none;
    color: inherit;
    vertical-align: middle;
  }
  .brand-mark :global(svg) {
    width: 100%;
    height: 100%;
    display: block;
    stroke-width: var(--brand-sw);
  }
</style>
