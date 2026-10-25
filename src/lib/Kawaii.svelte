<!-- Set kawaii reemplazable por archivos (BUG 2: el glob dinámico no garantizaba
  render en el primer frame del dev-server; los imports estáticos sí).
  Cada icono vive en ../assets/icons/kawaii/<nombre>.svg: para cambiar el set
  basta sustituir el CONTENIDO de los archivos (mismo nombre), sin tocar código.
  Dibujos propios: sin dependencias ni licencias. -->
<script lang="ts">
  import svgAdd from "../assets/icons/kawaii/add.svg?raw";
  import svgHappy from "../assets/icons/kawaii/happy.svg?raw";
  import svgHeart from "../assets/icons/kawaii/heart.svg?raw";
  import svgHeartFill from "../assets/icons/kawaii/heart-fill.svg?raw";
  import svgMenu from "../assets/icons/kawaii/menu.svg?raw";
  import svgMute from "../assets/icons/kawaii/mute.svg?raw";
  import svgNext from "../assets/icons/kawaii/next.svg?raw";
  import svgPause from "../assets/icons/kawaii/pause.svg?raw";
  import svgPlay from "../assets/icons/kawaii/play.svg?raw";
  import svgPlaylist from "../assets/icons/kawaii/playlist.svg?raw";
  import svgPrev from "../assets/icons/kawaii/prev.svg?raw";
  import svgQueue from "../assets/icons/kawaii/queue.svg?raw";
  import svgRemove from "../assets/icons/kawaii/remove.svg?raw";
  import svgSettings from "../assets/icons/kawaii/settings.svg?raw";
  import svgSleepy from "../assets/icons/kawaii/sleepy.svg?raw";
  import svgSurprised from "../assets/icons/kawaii/surprised.svg?raw";
  import svgMoon from "../assets/icons/kawaii/theme-moon.svg?raw";
  import svgSun from "../assets/icons/kawaii/theme-sun.svg?raw";
  import svgVolume from "../assets/icons/kawaii/volume.svg?raw";
  import svgYt from "../assets/icons/kawaii/ytmusic.svg?raw";
  import svgConnect from "../assets/icons/kawaii/connect.svg?raw";
  import svgLogout from "../assets/icons/kawaii/logout.svg?raw";

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
  // BUG 4: un nombre sin clave dejaba el botón vacío EN SILENCIO (pasó con
  // "vol"/"plus"). Aviso en consola de desarrollo para no repetirlo.
  $effect(() => {
    if (!raw && typeof console !== "undefined") {
      console.error(`[kawaii] icono desconocido: "${name}" (revisa assets/icons/kawaii)`);
    }
  });
  const sized = $derived(
    raw.replace("<svg", `<svg width="${size}" height="${size}"`),
  );
</script>

{@html sized}
