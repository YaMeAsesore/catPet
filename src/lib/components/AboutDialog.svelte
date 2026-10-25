<!-- «Acerca de Música»: ficha de la app con las dos mascotas de la marca.
     Usa las imágenes 3 y 4 vectorizadas (gato y personaje con auriculares) y
     el logo vinilo-play. Solo presentación; se cierra con Esc, × o «Cerrar». -->
<script lang="ts">
  import ObModal from "./onebit/ObModal.svelte";
  import Btn from "./onebit/Btn.svelte";
  import BrandMark from "./BrandMark.svelte";
  import gato from "../../assets/brand/mascota-gato.svg?raw";
  import persona from "../../assets/brand/mascota-persona.svg?raw";

  let { onclose }: { onclose: () => void } = $props();
</script>

<ObModal title="Acerca de Música" icon="disc" ancho={620} onclose={onclose}>
  <div class="about-head">
    <BrandMark size={54} title="Música" />
    <div class="about-id">
      <div class="about-name">Música <span class="about-ver">0.1.0</span></div>
      <div class="about-sub">Reproductor local, sin anuncios ni tracking.</div>
    </div>
  </div>

  <p class="about-txt">
    Biblioteca local con búsqueda instantánea, cola, MPRIS/SMTC y YouTube Music.
    Hecho con Tauri, Rust, Svelte y libmpv.
  </p>

  <div class="about-art">
    <figure class="about-panel dark">
      <span class="about-fig" aria-hidden="true">{@html gato}</span>
      <figcaption>Gato · biblioteca local</figcaption>
    </figure>
    <figure class="about-panel light">
      <span class="about-fig" aria-hidden="true">{@html persona}</span>
      <figcaption>Personaje · escucha</figcaption>
    </figure>
  </div>

  <div class="ob-acts">
    <Btn primary onclick={onclose}>Cerrar</Btn>
  </div>
</ObModal>

<style>
  .about-head { display: flex; align-items: center; gap: 14px; margin-bottom: 10px; }
  .about-id { min-width: 0; }
  .about-name { font-family: var(--font-pixel); font-size: 24px; font-weight: 700; }
  .about-ver { font-family: var(--font-text); font-size: 12px; font-weight: 400; color: var(--muted); margin-left: 5px; }
  .about-sub { color: var(--muted); font-size: 13px; }
  .about-txt { margin: 0 0 14px; font-size: 13px; line-height: 1.5; }
  .about-art { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  .about-panel {
    margin: 0; border: 2px solid var(--ink); box-shadow: var(--shadow-hard);
    display: flex; flex-direction: column; align-items: center; justify-content: center;
    gap: 6px; padding: 12px 10px 8px; min-height: 188px;
  }
  .about-panel.dark { background: var(--ink); color: var(--paper); }
  .about-panel.light { background: var(--paper-2); color: var(--ink); }
  .about-fig { display: block; width: 138px; height: 138px; }
  .about-fig :global(svg) { width: 100%; height: 100%; display: block; stroke-width: 6; }
  .about-panel figcaption { font-family: var(--font-pixel); font-size: 12px; letter-spacing: 0.02em; }
  @media (max-width: 560px) { .about-art { grid-template-columns: 1fr; } }
</style>
