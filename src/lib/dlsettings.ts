// Ajustes de descarga.
//
// Decisión: se guardan en `localStorage`, el MISMO mecanismo que ya usaba la
// app para su único ajuste de UI persistente (`musica.queueMin`, App.svelte).
// No existe fichero de configuración ni tabla de ajustes en el proyecto, y no
// se crea otro mecanismo nuevo: mismas claves, misma lectura protegida.
//
// Quien resuelve la carpeta es la interfaz (ella abre el selector del sistema
// con tauri-plugin-dialog); el backend solo recibe la ruta ya decidida.

export type DlMode = "default" | "ask";

/** "original" = sin recodificar; "m4a"/"mp3" necesitan ffmpeg (B1). */
export type DlFormat = "original" | "m4a" | "mp3";

export interface DlSettings {
  /** "default": carpeta elegida una vez; "ask": selector en cada descarga. */
  mode: DlMode;
  /** Carpeta por defecto (vacía = sin elegir → se comporta como "ask"). */
  folder: string;
  /** Formato de salida (ver DlFormat); sin ffmpeg solo aplica "original". */
  format: DlFormat;
  /** Incrustar portada y etiquetas (mp3/m4a con ffmpeg; en original, etiquetas). */
  embed: boolean;
  /** Prefijo numérico "01 - " en descargas de playlist. */
  prefix: boolean;
}

export const DL_DEFAULTS: DlSettings = {
  mode: "default",
  folder: "",
  format: "original",
  embed: false,
  prefix: false,
};

const KEY = {
  mode: "musica.dl.mode",
  folder: "musica.dl.folder",
  format: "musica.dl.format",
  embed: "musica.dl.embed",
  prefix: "musica.dl.prefix",
  /** Versión del esquema: pre-B1 la clave "m4a" significaba "Original". */
  ver: "musica.dl.v",
} as const;

function store(): Storage | null {
  try {
    return typeof localStorage !== "undefined" ? localStorage : null;
  } catch {
    return null;
  }
}

export function loadDlSettings(): DlSettings {
  const s = store();
  if (!s) return { ...DL_DEFAULTS };
  try {
    const mode = s.getItem(KEY.mode);
    const raw = s.getItem(KEY.format);
    // Migración pre-B1: la clave "m4a" guardada significaba "Original sin
    // recodificar" (así se etiquetaba en D4). Sin la marca de versión, "m4a"
    // legacy se traduce a "original" para no cambiar a escondidas el comportamiento.
    const v2 = s.getItem(KEY.ver) === "2";
    let format: DlFormat = "original";
    if (raw === "mp3") format = "mp3";
    else if (raw === "m4a") format = v2 ? "m4a" : "original";
    else if (raw === "original") format = "original";
    return {
      mode: mode === "ask" ? "ask" : "default",
      folder: s.getItem(KEY.folder) ?? "",
      format,
      embed: s.getItem(KEY.embed) === "1",
      prefix: s.getItem(KEY.prefix) === "1",
    };
  } catch {
    return { ...DL_DEFAULTS };
  }
}

export function saveDlSettings(v: DlSettings): void {
  const s = store();
  if (!s) return;
  try {
    s.setItem(KEY.mode, v.mode);
    s.setItem(KEY.folder, v.folder);
    s.setItem(KEY.format, v.format);
    s.setItem(KEY.embed, v.embed ? "1" : "0");
    s.setItem(KEY.prefix, v.prefix ? "1" : "0");
    s.setItem(KEY.ver, "2");
  } catch {
    // Sin almacenamiento disponible: el ajuste no persiste, nada se rompe.
  }
}

/** Carpeta que usaría la próxima descarga; null = hay que preguntar. */
export function effectiveFolder(v: DlSettings): string | null {
  const f = v.folder.trim();
  return v.mode === "default" && f.length > 0 ? v.folder : null;
}
