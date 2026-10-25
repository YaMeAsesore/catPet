<!-- U1: reproductor inferior (marcado movido ÍNTEGRO desde App.svelte: mismos
     textos, clases, atajos visibles y accesibilidad). Sin lógica propia: la
     búsqueda/volumen se resuelven en el padre vía callbacks (que además
     replican lo que hacían el inline y el bind:value anteriores).
     Piel 1-bit (kawaii monocromo) siempre activa. -->
<script lang="ts">
  import KawaiiAdapt from "./onebit/KawaiiAdapt.svelte";
  import { fmtSecs } from "../tauri";
  import Waves from "../Waves.svelte";
  import { cropBlackBars } from "../cropBlackBars";

  let {
    track = null,
    engine = "libmpv",
    sounding = false,
    loading = false,
    position = 0,
    duration = 0,
    fillPct = 0,
    volume = 80,
    volName = "vol",
    onStep,
    onToggle,
    onOpenCover,
    onSeekInput,
    onSeekDone,
    onVolumeInput,
    onToggleMute,
    showCover = false,
    shownTrack = null,
    onCloseCover,
    canLike = false,
    liked = false,
    likeBusy = false,
    onToggleLike,
  }: {
    track?: { uri: string; title: string; artist: string; album: string; art_url: string } | null;
    engine?: "libmpv" | "spotify";
    sounding?: boolean;
    loading?: boolean;
    position?: number;
    duration?: number;
    fillPct?: number;
    volume?: number;
    volName?: string;
    onStep: (delta: 1 | -1) => void;
    onToggle: () => void;
    onOpenCover: () => void;
    onSeekInput: (v: number) => void;
    onSeekDone: () => void;
    onVolumeInput: (v: number) => void;
    onToggleMute: () => void;
    showCover?: boolean;
    shownTrack?: { uri: string; title: string; artist: string; album: string; art_url: string } | null;
    onCloseCover?: () => void;
    /** Corazón de YouTube Music (solo temas ytmusic://). */
    canLike?: boolean;
    liked?: boolean;
    likeBusy?: boolean;
    onToggleLike?: () => void;
  } = $props();
</script>

<footer class="player" class:playing={sounding}>
  <button onclick={() => onStep(-1)} title="Anterior (P)" data-tip="Anterior (P)"><KawaiiAdapt name="prev" /></button>
  <button class="primary" onclick={onToggle} title="Play/Pause (Espacio)" data-tip="Play/Pause (Espacio)">
    <KawaiiAdapt name={sounding ? "pause" : "play"} />
  </button>
  <button onclick={() => onStep(1)} title="Siguiente (N)" data-tip="Siguiente (N)"><KawaiiAdapt name="next" /></button>
  <!-- Atribución visible unificada: motor agnóstico (libmpv o librespot) -->
  {#if track}
    {#key track.uri}
      <div class="cover-wrapper" style="position: relative;">
        <button class="cover-btn cover-in" onclick={onOpenCover} title="Ver portada" data-tip="Ver portada">
          <div class="art-container" style="width: 44px; height: 44px;">
            {#if track.art_url}
              <img use:cropBlackBars={track.art_url} src={track.art_url} alt="" loading="lazy" />
            {:else}
              <div class="art-container" style="width: 100%; height: 100%; background: var(--paper-2); display: grid; place-items: center;">
                <KawaiiAdapt name="note" size={28} />
              </div>
            {/if}
          </div>
        </button>
        {#if showCover && shownTrack}
          <div class="ob-cover" role="dialog" aria-label="Portada actual" tabindex="-1">
            <button class="icon ob-cover-x" onclick={onCloseCover} aria-label="Cerrar (Esc)" data-tip="Cerrar (Esc)" data-tip-al="fin"><KawaiiAdapt name="remove" size={14} /></button>
            <div class="art-stage" role="button" tabindex="0" onclick={onCloseCover} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onCloseCover?.(); } }}>
              <Waves playing={sounding} estatico />
              {#if shownTrack.art_url}
                <div class="art-container art-container--lamina" style="width: 210px; max-width: 56vw;">
                  <img use:cropBlackBars={shownTrack.art_url} src={shownTrack.art_url} alt="Carátula de {shownTrack.album}" />
                </div>
              {:else}
                <div class="art-container art-container--lamina" style="width: 210px; max-width: 56vw; background: var(--paper-2); display: grid; place-items: center;">
                  <KawaiiAdapt name="sleepy" size={110} />
                </div>
              {/if}
            </div>
            <div class="ob-cover-title">{shownTrack.title}</div>
            <div class="ob-cover-sub">{shownTrack.artist} — {shownTrack.album}</div>
          </div>
        {/if}
      </div>
    {/key}
  {/if}
  <div class="meta">
    <b>{track?.title ?? "Sin reproducción"}</b>
    <span>{
      track
        ? `${track.artist} — ${track.album} · ${engine === "spotify" ? "Spotify Connect" : "libmpv"}`
        : "Añade una carpeta y haz clic"
    }</span>
  </div>
  {#if canLike}
    <button class="icon like" class:on={liked} disabled={likeBusy}
      aria-label={liked ? "Quitar «Me gusta» de YouTube Music" : "Guardar en «Me gusta» de YouTube Music"}
      aria-pressed={liked}
      title={likeBusy
        ? "Guardando en YouTube Music…"
        : liked
          ? "Quitar «Me gusta» de YouTube Music"
          : "Guardar en «Me gusta» de YouTube Music"}
      onclick={onToggleLike}>
      <KawaiiAdapt name={liked ? "heart-fill" : "heart"} size={18} />
    </button>
  {/if}
  <span>{fmtSecs(position)}</span>
  {#if loading}<span class="loading-dots" title="mpv aún no confirma reproducción">Cargando…</span>{/if}
  <input class="progress" type="range" min="0" max={Math.max(duration, 1)} step="1" value={position}
    oninput={(e) => onSeekInput(Number((e.target as HTMLInputElement).value))}
    onchange={onSeekDone} style="flex:1; --fill:{fillPct}%;" title="Posición" />
  <span>{fmtSecs(duration)}</span>
  <div class="vol-wrap">
    <button class="icon" onclick={onToggleMute} title={volume === 0 ? "Activar sonido" : "Silenciar"} data-tip={volume === 0 ? "Activar sonido" : "Silenciar"}>
      {#key volName}<span class="vol-icon"><KawaiiAdapt name={volName} /></span>{/key}
    </button>
    <input class="vol" type="range" min="0" max="100" value={volume} title="Volumen"
      oninput={(e) => onVolumeInput(Number((e.currentTarget as HTMLInputElement).value))}
      style="--fill:{volume}%;" />
  </div>
</footer>
