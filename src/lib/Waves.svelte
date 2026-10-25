<!-- Ondas concéntricas ondulantes alrededor de la portada.
  NOTA HONESTA: libmpv no expone datos de audio (sin FFT en su API cliente),
  así que NO reaccionan al audio real: es una simulación con ruido suave
  (suma de senos inconmensurables + envolvente de pulso). Un solo canvas,
  transform/opacity en CSS, rAF con DPR capado; en pausa se congela y atenúa
  (sin CPU) y con prefers-reduced-motion dibuja un fotograma estático. -->
<script lang="ts">
  import { onMount } from "svelte";

  let {
    playing = true,
    size = 276,
    estatico = false,
  }: { playing?: boolean; size?: number; estatico?: boolean } = $props();

  let canvas: HTMLCanvasElement | null = $state(null);
  let raf = 0;
  let t = Math.random() * 100; // fase inicial aleatoria: nunca dos iguales

  const RM =
    typeof matchMedia !== "undefined" &&
    matchMedia("(prefers-reduced-motion: reduce)").matches;

  function palette(el: HTMLElement): [string, string, string] {
    // Leer del estilo computado del elemento (canvas), que hereda de body.onebit en 1-bit.
    // En modo clásico, hereda de :root. Fallbacks clásicos solo si no hay tokens.
    const cs = getComputedStyle(el);
    const g = (n: string, fb: string) => (cs.getPropertyValue(n).trim() || fb);
    return [g("--accent", "#A6562C"), g("--accent-ink", "#7E421F"), g("--muted", "#6B5B45")];
  }

  interface Ring {
    frac: number;
    amp: number;
    wob: number;
    speed: number;
    alpha: number;
  }
  const RINGS: Ring[] = [
    { frac: 0.405, amp: 9, wob: 3, speed: 1.0, alpha: 0.85 },
    { frac: 0.452, amp: 7, wob: 4, speed: -0.7, alpha: 0.6 },
    { frac: 0.493, amp: 6, wob: 5, speed: 0.45, alpha: 0.5 },
  ];
  const SEG = 140;

  function draw(ctx: CanvasRenderingContext2D, colors: [string, string, string], dim: boolean) {
    const w = size;
    const c = w / 2;
    // Envolvente de "ritmo": pulso ~112 BPM + oleaje lento (simulado).
    const env = dim
      ? 0.35
      : 0.72 + 0.28 * (0.5 + 0.5 * Math.sin(t * 2 * Math.PI * 1.87)) * (0.6 + 0.4 * Math.sin(t * 2 * Math.PI * 0.63 + 1.3));
    ctx.clearRect(0, 0, w, w);
    ctx.lineWidth = 2;
    ctx.lineJoin = "round";
    RINGS.forEach((rg, ri) => {
      ctx.beginPath();
      for (let i = 0; i <= SEG; i++) {
        const th = (i / SEG) * Math.PI * 2;
        const rr =
          rg.frac * w +
          (Math.sin(th * rg.wob + t * (1.7 * rg.speed)) * 0.5 +
            Math.sin(th * (rg.wob + 2) - t * (1.1 * rg.speed)) * 0.3 +
            Math.sin(th * (rg.wob - 1) + t * (0.6 * rg.speed)) * 0.2) *
          rg.amp *
          env;
        const x = c + Math.cos(th) * rr;
        const y = c + Math.sin(th) * rr;
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.closePath();
      ctx.globalAlpha = rg.alpha * (dim ? 0.4 : 1);
      ctx.strokeStyle = colors[ri % colors.length];
      ctx.stroke();
    });
    ctx.globalAlpha = 1;
  }

  function loop(ctx: CanvasRenderingContext2D, colors: [string, string, string]) {
    t += 1 / 60;
    draw(ctx, colors, false);
    raf = requestAnimationFrame(() => loop(ctx, colors));
  }

  function stop() {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
  }

  onMount(() => {
    const el = canvas;
    if (!el) return;
    const ctx = el.getContext("2d");
    if (!ctx) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    el.width = Math.round(size * dpr);
    el.height = Math.round(size * dpr);
    ctx.scale(dpr, dpr);
    const colors = palette(el);
    // M3.2: en "Mac 1-bit" no hay bucle: un solo fotograma estático en tinta
    // (los aros dejan de girar/pulsar y de gastar CPU; el contraste es tinta
    // sobre papel, así que tampoco hay color). Con pausa queda más tenue.
    if (estatico) {
      draw(ctx, colors, !playing);
      // P1b: medir color real de los aros (no la esquina transparente).
      // Recorre el canvas y reporta el color más frecuente con alfa > 0.
      if (estatico && document.body.classList.contains('onebit')) {
        const imgData = ctx.getImageData(0, 0, size, size);
        const data = imgData.data;
        const colorCounts = new Map<string, number>();
        for (let i = 0; i < data.length; i += 4) {
          const a = data[i + 3];
          if (a > 0) {
            const key = `${data[i]},${data[i+1]},${data[i+2]}`;
            colorCounts.set(key, (colorCounts.get(key) || 0) + 1);
          }
        }
        let topColor: [number, number, number] | null = null;
        let topCount = 0;
        for (const [key, count] of colorCounts) {
          if (count > topCount) {
            topCount = count;
            topColor = key.split(',').map(Number) as [number, number, number];
          }
        }
        if (topColor) {
          const [r, g, b] = topColor;
          console.debug('[waves] color real de aros (1-bit estático):', { r, g, b, count: topCount, hex: '#' + [r, g, b].map(v => v.toString(16).padStart(2, '0')).join('') });
          // Verificar que no es naranja clásico
          const isOrange = r > 150 && g < 100 && b < 50;
          if (isOrange) console.error('[waves] ALERTA: color naranja detectado en 1-bit!');
        }
      }
      return;
    }
    if (RM) {
      draw(ctx, colors, true); // un fotograma estático tenue
      return;
    }
    if (playing) loop(ctx, colors);
    else draw(ctx, colors, true);
    return () => stop();
  });

  // Al pausar/reanudar: arranca o congela el bucle (sin CPU en pausa).
  $effect(() => {
    const el = canvas;
    if (!el || RM || estatico) return;
    const ctx = el.getContext("2d");
    if (!ctx) return;
    const colors = palette(el);
    if (playing) {
      if (!raf) loop(ctx, colors);
    } else {
      stop();
      draw(ctx, colors, true);
    }
  });
</script>

<canvas
  bind:this={canvas}
  class="waves-canvas"
  class:dim={!playing}
  style="width:100%;height:100%;"
  aria-hidden="true"
></canvas>