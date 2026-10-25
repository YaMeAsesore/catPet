<!-- Adaptador kawaii 1-bit para componentes compartidos (cola, reproductor, popover).
   Usa siempre Kawaii1bit (pixel-art monocroma). El set clásico se retiró en M5. -->
<script lang="ts">
  import Kawaii1bit from "./Kawaii1bit.svelte";

  let { name, size = 20 }: { name: string; size?: number } = $props();
</script>

<Kawaii1bit {name} {size} />