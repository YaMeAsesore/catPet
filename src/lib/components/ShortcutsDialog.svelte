<!-- M2/M4: Ayuda › «Atajos de teclado…» — la lista del hint clásico (que se
     retiró en M5) en una ventana 1-bit (ObModal: borde 2 px + sombra dura).
     Solo presentación: mismos atajos que el contrato. Se cierra con Esc (como
     los demás diálogos), con la × o con «Cerrar». -->
<script lang="ts">
  import ObModal from "./onebit/ObModal.svelte";
  import Btn from "./onebit/Btn.svelte";

  let { onclose }: { onclose: () => void } = $props();

  const KEYS: [string, string][] = [
    ["Ctrl/Cmd + K", "Buscar en la biblioteca"],
    ["Espacio", "Reproducir / pausar"],
    ["N · P", "Siguiente / pista anterior"],
    ["L", "Me gusta en YouTube Music"],
    ["← · →", "Retroceder / avanzar 5 s"],
    ["Enter", "Guardar en diálogos y quitar de la cola"],
    ["Esc", "Cerrar menús y diálogos"],
  ];
</script>

<ObModal title="Atajos de teclado" ancho={440} focoCerrar onclose={onclose}>
  <dl class="ob-atajos">
    {#each KEYS as [k, txt]}
      <div><dt>{k}</dt><dd>{txt}</dd></div>
    {/each}
  </dl>
  <div class="ob-acts">
    <Btn primary onclick={onclose}>Cerrar</Btn>
  </div>
</ObModal>
