<!-- Adaptador de iconos 1-bit para componentes compartidos.
   Usa siempre Icon16 (set 1-bit). El set clásico (Icon) se retiró en M5. -->
<script lang="ts">
  import Icon16 from "./Icon16.svelte";

  let { name, size = 16 }: { name: string; size?: number } = $props();
</script>

<Icon16 {name} {size} />