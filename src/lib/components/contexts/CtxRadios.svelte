<!-- M3: contexto «Radios y podcasts» (fuente kind radio/podcast). En la
     interfaz clásica este contexto NO tiene controles propios: la emisora o
     el feed se eligen, añaden, renombran y quitan desde la barra lateral
     (secciones Fuentes, con «⋯» por fila) y los menús Archivo/Fuente desde
     M1/M2; el motor y las descargas, desde Ajustes y la barra de menú. Aquí
     solo vive la cabecera: ventana 1-bit con el patrón de rayas de radios, el
     icono de radio y el tipo de fuente (radio directa / RSS) como subtítulo.
     Sin lógica y sin estado. -->
<script lang="ts">
  import ContextBar from "./ContextBar.svelte";

  let { title, subtitle }: { title: string; subtitle: string } = $props();
</script>

<ContextBar ctx="radios" {title} {subtitle} />
