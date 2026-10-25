<!-- P6: cabecera del contexto «Recomendaciones». Reutiliza el contexto visual
     youtube (rayas + chip yt) para no inventar un sexto color/patrón. Solo
     presenta acciones; el estado y la carga viven en App.svelte. -->
<script lang="ts">
  import ContextBar from "./ContextBar.svelte";
  import Btn from "../onebit/Btn.svelte";

  let {
    title,
    subtitle,
    loading = false,
    onRefresh,
    onClose,
  }: {
    title: string;
    subtitle: string;
    loading?: boolean;
    onRefresh?: () => void;
    onClose?: () => void;
  } = $props();
</script>

<ContextBar ctx="youtube" {title} {subtitle}>
  {#snippet actions()}
    {#if loading}
      <span class="hint">Actualizando…</span>
    {/if}
    <Btn small onclick={onRefresh} disabled={loading} title="Volver a pedir el home (ignora la caché de 10 min)">
      Actualizar
    </Btn>
    <Btn small onclick={onClose} title="Cerrar vista">Cerrar</Btn>
  {/snippet}
</ContextBar>
