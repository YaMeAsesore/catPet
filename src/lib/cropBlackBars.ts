// Acción Svelte reutilizable: recorta barras negras/letterbox en thumbnails YouTube.
// SOLO para i.ytimg.com/vi/ con proporción 4:3 (letterbox) o 16:9 (pillarbox).
// Usa contenedor CSS con --art-zoom (zoom UNIFORME), nunca scale(a,b) con a!=b.
// Se recalcula en cada 'load' y se resetea a 1 al cambiar src (antes de cargar).
// Recibe el src como parámetro para que Svelte llame a update() al cambiar.

export function cropBlackBars(node: HTMLImageElement, src: string) {
  if (!(node instanceof HTMLImageElement)) return;

  const container = node.parentElement;
  if (!container) return;

  let currentSrc = src;

  const calculateZoom = (src: string, naturalWidth: number, naturalHeight: number): number => {
    if (!naturalWidth || !naturalHeight) return 1;

    const ratio = naturalWidth / naturalHeight;
    const isSquare = ratio >= 0.98 && ratio <= 1.02; // ±2% tolerancia

    // Detectar YouTube thumbnail por URL real usada por la app
    const isYtThumb = src.includes('i.ytimg.com/vi/');

    let artZoom = 1;

    if (isYtThumb && !isSquare) {
      // 4:3 con letterbox (ratio ~1.33, ej 480x360 = 1.333)
      if (ratio >= 1.30 && ratio <= 1.36) {
        return 1.333; // 4:3 letterbox → zoom uniforme 1.333
      }
      // 16:9 con pillarbox (ratio ~1.78, ej 640x360 = 1.778, 1280x720 = 1.778)
      // object-fit: cover en contenedor cuadrado YA recorta la franja central → zoom 1
      else if (ratio >= 1.75 && ratio <= 1.80) {
        return 1;
      }
    }
    return 1;
  };

  const applyZoom = (zoom: number) => {
    container.style.setProperty('--art-zoom', String(zoom));
  };

  const resetZoom = () => {
    applyZoom(1);
  };

  const handleLoad = () => {
    const zoom = calculateZoom(node.src, node.naturalWidth, node.naturalHeight);
    applyZoom(zoom);
  };

  // Listener de carga (se vuelve a disparar si cambia src)
  node.addEventListener('load', handleLoad);

  // Calcular inmediato si ya está cargada (mount o src cambiado sin desmontar)
  if (node.complete && node.naturalWidth > 0) {
    handleLoad();
  } else {
    // Reset inmediato al cambiar src (antes de que cargue la nueva)
    resetZoom();
  }

  // Función update para Svelte: se llama cuando cambia el parámetro src
  const update = (newSrc: string) => {
    if (newSrc !== currentSrc) {
      currentSrc = newSrc;
      if (node.complete && node.naturalWidth > 0) {
        handleLoad();
      } else {
        resetZoom();
      }
    }
  };

  return {
    update,
    destroy() {
      node.removeEventListener('load', handleLoad);
      container.style.removeProperty('--art-zoom');
    }
  };
}