<!-- M3: contexto «Servidores» (Jellyfin, Subsonic/Navidrome). Igual que
     radios/podcasts, la clásica no tiene controles dentro del contexto: la
     cuenta se administra en la barra lateral (Fuentes: abrir, renombrar,
     quitar) y en Archivo › Añadir fuente…; el motor y las descargas, en
     Ajustes y la barra de menú (desde M1/M2). Esta cabecera aporta el patrón
     de rayas de servidores, el icono de servidor y el subtipo como subtítulo.
     Sin lógica y sin estado. -->
<script lang="ts">
  import ContextBar from "./ContextBar.svelte";

  let { title, subtitle }: { title: string; subtitle: string } = $props();
</script>

<ContextBar ctx="servidores" {title} {subtitle} />
