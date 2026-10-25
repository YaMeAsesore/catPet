<!-- M1: set kawaii 1-bit — mismos nombres y API que Kawaii.svelte, pero los
     dibujos viven en ../../../assets/icons/kawaii1bit/<nombre>.svg, redibujados
     como pixel-art monocroma original (currentColor) para el estilo «Mac
     1-bit». Sustituir el CONTENIDO de un archivo cambia el dibujo sin tocar
     código. Dibujos propios: sin dependencias ni licencias. -->
<script lang="ts">
  import svgAdd from "../../../assets/icons/kawaii1bit/add.svg?raw";
  import svgHappy from "../../../assets/icons/kawaii1bit/happy.svg?raw";
  import svgHeart from "../../../assets/icons/kawaii1bit/heart.svg?raw";
  import svgHeartFill from "../../../assets/icons/kawaii1bit/heart-fill.svg?raw";
  import svgMenu from "../../../assets/icons/kawaii1bit/menu.svg?raw";
  import svgMute from "../../../assets/icons/kawaii1bit/mute.svg?raw";
  import svgNext from "../../../assets/icons/kawaii1bit/next.svg?raw";
  import svgNote from "../../../assets/icons/kawaii1bit/note.svg?raw";
  import svgPause from "../../../assets/icons/kawaii1bit/pause.svg?raw";
  import svgPlay from "../../../assets/icons/kawaii1bit/play.svg?raw";
  import svgPlaylist from "../../../assets/icons/kawaii1bit/playlist.svg?raw";
  import svgPrev from "../../../assets/icons/kawaii1bit/prev.svg?raw";
  import svgQueue from "../../../assets/icons/kawaii1bit/queue.svg?raw";
  import svgRemove from "../../../assets/icons/kawaii1bit/remove.svg?raw";
  import svgSettings from "../../../assets/icons/kawaii1bit/settings.svg?raw";
  import svgSleepy from "../../../assets/icons/kawaii1bit/sleepy.svg?raw";
  import svgSurprised from "../../../assets/icons/kawaii1bit/surprised.svg?raw";
  import svgMoon from "../../../assets/icons/kawaii1bit/theme-moon.svg?raw";
  import svgSun from "../../../assets/icons/kawaii1bit/theme-sun.svg?raw";
  import svgVolume from "../../../assets/icons/kawaii1bit/volume.svg?raw";
  import svgYt from "../../../assets/icons/kawaii1bit/ytmusic.svg?raw";
  import svgConnect from "../../../assets/icons/kawaii1bit/connect.svg?raw";
  import svgLogout from "../../../assets/icons/kawaii1bit/logout.svg?raw";

  const SET: Record<string, string> = {
    add: svgAdd,
    // Alias: los usos históricos piden "plus"/"vol" (botones vacíos si fallan).
    plus: svgAdd,
    vol: svgVolume,
    happy: svgHappy,
    heart: svgHeart,
    "heart-fill": svgHeartFill,
    menu: svgMenu,
    mute: svgMute,
    next: svgNext,
    note: svgNote,
    pause: svgPause,
    play: svgPlay,
    playlist: svgPlaylist,
    prev: svgPrev,
    queue: svgQueue,
    remove: svgRemove,
    settings: svgSettings,
    sleepy: svgSleepy,
    surprised: svgSurprised,
    "theme-moon": svgMoon,
    "theme-sun": svgSun,
    volume: svgVolume,
    ytmusic: svgYt,
    connect: svgConnect,
    logout: svgLogout,
  };

  let { name, size = 20 }: { name: string; size?: number } = $props();

  const raw = $derived(SET[name] ?? "");
  // Un nombre sin clave dejaría el botón vacío en silencio: aviso en consola.
  $effect(() => {
    if (!raw && typeof console !== "undefined") {
      console.error(`[kawaii1bit] icono desconocido: "${name}" (revisa assets/icons/kawaii1bit)`);
    }
  });
  const sized = $derived(
    raw.replace("<svg", `<svg width="${size}" height="${size}"`),
  );
</script>

{@html sized}
