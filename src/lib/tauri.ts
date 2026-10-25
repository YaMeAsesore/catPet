// Wrappers tipados sobre los comandos Tauri (MVP + fase 1).
// Decisión: centralizar invoke aquí para que App.svelte no conozca strings IPC.
// OJO: Tauri v2 espera las claves de argumentos en camelCase por defecto
// (el macro usa ArgumentCase::Camel salvo `rename_all` explícito): un param
// Rust `base_url` se invoca como `baseUrl`. Si un comando falla con
// "missing required key Xxx", revisa primero la capitalización aquí.
import { invoke } from "@tauri-apps/api/core";

export interface Track {
  id: number; path: string; title: string; artist: string; album: string;
  genre: string; year: number; track_no: number; duration_secs: number; mtime: number;
  art_url: string; // portada remota o "" (fase 1: solo en memoria)
}
export interface QueueState { items: Track[]; current: number | null; rev?: number; }
export interface PlayerStatus {
  paused: boolean; pos_secs: number; dur_secs: number; volume: number; path: string | null;
  /** Fin de pista latcheado (solo libmpv; lo consume el auto-avance). */
  eof: boolean;
  /** mpv confirmó reproducción (PlaybackRestart) para la carga actual. */
  confirmed: boolean;
  /** Motivo real del último fallo (EndFile ERROR o watchdog agotado). */
  error: string | null;
}
/** Reporte del auto-avance (qué saltó, si paró, aviso único). */
export interface FailedTrack {
  path: string; reason: string;
}
export interface AdvanceReport {
  advanced: boolean; failed: FailedTrack[]; stopped: boolean; error: string | null;
}
export interface Source {
  id: number; kind: string; name: string; base_url: string;
  user: string; secret: string; extra: string;
}

// ---- Eventos IPC del PlaybackRouter (motores libmpv / librespot) ----

export type EngineKind = "libmpv" | "spotify";
export type PlaybackState = "playing" | "paused" | "stopped";

/** Atribución visible del track actual (siempre presente, motor agnóstico). */
export interface TrackAttribution {
  uri: string; title: string; artist: string; album: string; art_url: string; duration_ms: number;
}
export interface PlaybackStateEvent {
  engine: EngineKind; state: PlaybackState;
  /** Generación de reproducción (ignorar eventos viejos). */
  gen: number;
  /** Instante del backend en ms (medir latencia de pintado). */
  ts_ms: number;
}
export interface TrackChangedEvent {
  engine: EngineKind; track: TrackAttribution | null;
  /** Generación de reproducción (ignorar eventos viejos). */
  gen: number;
  /** Instante del backend en ms (medir latencia de pintado). */
  ts_ms: number;
}
export interface QueueEvent {
  current: number | null; length: number;
  /** Versión estructural de la cola (si no cambia, solo cambió el cursor). */
  rev?: number;
}
export interface UnifiedStatus {
  engine: EngineKind;
  paused: boolean;
  position_ms: number;
  duration_ms: number;
  volume: number;
  track: TrackAttribution | null;
  spotify_ready: boolean;
}
export interface SpotifyAccountInfo {
  connected: boolean;
  username: string | null;
  device_name: string | null;
  persistent_keyring: boolean;
  backend: string | null;
  /** "premium" | "restricted" | null (desconectado). */
  tier: string | null;
  /** librespot no inserta anuncios: siempre false. */
  ads_supported: boolean;
}
/** Evento IPC `spotify://restriction`: un comando violó la política. */
export interface RestrictionEvent {
  tier: string; code: string; message: string;
  skips_used: number; skips_max: number; retry_after_secs: number;
}
export interface PolicyStatus {
  tier: string; ads_supported: boolean;
  skips_used: number; skips_max: number; retry_after_secs: number;
}
export interface PlaylistInfo {
  name: string; count: number;
}
/** S2: fila de una lista importada desde CSV (normalización en Rust). */
export interface CsvTrack { title: string; artist: string; album: string; duration_s: number | null; }
/** S2: vista previa de un CSV local; la búsqueda en YouTube llega en S3. */
export interface CsvPreview {
  file: string; total: number; skipped: number; truncated: boolean; has_duration: boolean;
  rows: CsvTrack[];
}
/** S3: candidato de YouTube devuelto por el emparejador (YtTrack de Rust). */
export interface YtTrack {
  id: string; title: string; artist: string; duration_secs: number; thumbnail: string;
  /** Estado del «Me gusta» de YouTube Music; null/undefined = desconocido. */
  liked?: boolean | null;
}
/** S3: estado de una fila CSV tras emparejar (coincide con MatchState de Rust). */
export type CsvMatchState = "exacta" | "dudosa" | "sin_match" | "no_encontrada";
/** S3: resultado de emparejar una fila (efímero: la UI no persiste nada). */
export interface CsvMatchResult {
  state: CsvMatchState;
  candidate: YtTrack | null;
  motivo: string;
}
/** S4: resultado de guardar la lista como playlist (`duplicate` no escribe). */
export interface SaveOutcome {
  status: "saved" | "duplicate";
  name: string;
}
/** S4: candidato del selector «Buscar otra versión» (YtTrack + veredicto). */
export interface VariantRow extends YtTrack {
  state: CsvMatchState;
  motivo: string;
}
export interface YtImport {
  title: string; tracks: Track[];
}
/** Fila por lote: Track + estado de preparación (W0/W4). */
export interface YtBatchRow extends Track {
  state: "ready" | "dead" | "unprepared";
  motivo: string | null;
  /** Like de YouTube Music en el listado; null = aún desconocido. */
  liked: boolean | null;
}
/** Instantánea del cargador por lotes (se polla máx. ~2 veces por segundo). */
export interface YtBatchSnapshot {
  open: boolean; title: string; total: number; shown: number; batchSize: number;
  loading: boolean; processed: number; batchTotal: number; more: boolean;
  /** Precalentando las primeras pistas del lote (polling ligero de la UI). */
  warming?: boolean;
  /** Id de lista para «Guardar playlist»; null si no se pudo determinar. */
  playlistId?: string | null;
  /** ¿La playlist está guardada en la biblioteca? null = desconocido. */
  saved?: boolean | null;
  rows: YtBatchRow[]; warning: string | null;
}
/** P6: tarjeta de playlist del home personalizado, ya filtrada en Rust. */
export interface RecoItem {
  title: string; author: string; views: string; thumbnail: string; playlist_id: string; section: string;
}
/** P6: resultado de `youtube_recommendations`. */
export interface RecoFeed {
  items: RecoItem[];
  community_present: boolean;
  truncated: boolean;
  warning: string | null;
}
/** P6: error tipado del cliente interno (el mensaje nunca lleva cookies). */
export type RecoErrorCode = "no_session" | "auth" | "config" | "http" | "network" | "parse";
export interface RecoError {
  code: RecoErrorCode;
  message: string;
}
export interface YtMine {
  name: string; url: string; count: number | null;
}
export interface YtAccount {
  connected: boolean; unverified: boolean; browser: string | null;
  /** L4: hay datos aislados de la ventana de login (se ofrecen borrar al desconectar). */
  login_data: boolean;
}
/** Resultado de la verificación de sesión (L2). */
export interface YtSessionCheck {
  status: "valid" | "expired" | "unverified" | "none";
  account: YtAccount;
}
/** L3b: estado del login integrado de Google (evento `youtube://login`). */
export interface YtLoginEvent {
  state: "waiting" | "connected" | "failed" | "cancelled" | "timeout";
  message: string | null;
  account: YtAccount;
}
export interface YtProbe {
  available: boolean; version: string | null; error?: string | null; deno: boolean; node: boolean;
}
export interface BrowserProfile {
  label: string; browser: string; profile: string | null;
}
/** Pista a descargar (id de vídeo plano; nunca URL ni cookies). */
export interface DownloadItem {
  id: string; title: string; artist: string;
}
/** Opciones ya resueltas por la interfaz (carpeta elegida por el usuario). */
export interface DownloadOptions {
  folder: string;
  /** "m4a" (original, sin recodificar) o "mp3" (requiere ffmpeg). */
  format: string;
  embed: boolean;
  numericPrefix?: boolean;
  playlistSubdir?: string | null;
}
export interface DownloadFailure { title: string; motivo: string; }
/** Instantánea de la cola de descargas (poll). */
export interface DownloadSnapshot {
  active: boolean; total: number; finished: number;
  downloaded: number; skipped: number; failed: number;
  current: string | null; summary: string | null;
  folder: string | null; failures: DownloadFailure[]; notice: string | null;
}
/** Entorno de descarga (Ajustes): ffmpeg y cómo instalarlo. */
export interface DownloadEnv { ffmpeg: boolean; ffmpegHint: string; }

export const api = {
  scan: (folder: string) => invoke<{ added: number; kept: number; removed: number; total: number }>("scan_folder", { folder }),
  search: (query: string, limit = 300, offset = 0) =>
    invoke<Track[]>("library_search", { query, limit, offset }),
  stats: () => invoke<number>("library_stats"),
  sourceList: () => invoke<Source[]>("source_list"),
  sourceAdd: (kind: string, name: string, baseUrl: string, user: string, secret: string) =>
    invoke<Source>("source_add", { kind, name, baseUrl, user, secret }),
  sourceRemove: (id: number) => invoke<void>("source_remove", { id }),
  // U2: renombrar fuente (solo el nombre; ni URL ni credenciales).
  sourceRename: (id: number, name: string) => invoke<void>("source_rename", { id, name }),
  remoteSearch: (sourceId: number, query: string) =>
    invoke<Track[]>("remote_search", { sourceId, query }),
  queueSet: (tracks: Track[], start: number) => invoke<void>("queue_set", { tracks, start }),
  queueAdd: (track: Track) => invoke<void>("queue_add", { track }),
  queueAppend: (tracks: Track[]) => invoke<void>("queue_append", { tracks }),
  queueMove: (from: number, to: number) => invoke<void>("queue_move", { from, to }),
  queueRemove: (index: number) => invoke<void>("queue_remove", { index }),
  queueClear: () => invoke<void>("queue_clear"),
  queueList: () => invoke<QueueState>("queue_list"),
  playIndex: (index: number) => invoke<void>("player_play_index", { index }),
  toggle: () => invoke<boolean>("player_toggle"),
  next: () => invoke<void>("player_next"),
  prev: () => invoke<void>("player_prev"),
  seek: (secs: number) => invoke<void>("player_seek", { secs }),
  volume: (volume: number) => invoke<void>("player_volume", { volume }),
  status: () => invoke<PlayerStatus>("player_status"),
  trackEnded: () => invoke<AdvanceReport>("player_track_ended"),
  retryCurrent: () => invoke<boolean>("player_retry_current"),

  // ---- PlaybackRouter: estado unificado + control de motor ----
  playbackStatus: () => invoke<UnifiedStatus>("playback_status"),
  setEngine: (engine: "libmpv" | "spotify") => invoke<UnifiedStatus>("playback_set_engine", { engine }),
  play: () => invoke<void>("player_play"),
  pause: () => invoke<void>("player_pause"),
  shuffle: (on: boolean) => invoke<void>("playback_shuffle", { on }),
  repeat: (mode: "off" | "context" | "track") => invoke<void>("playback_repeat", { mode }),

  // ---- Spotify (librespot) ----
  spotifyConnect: (pkceToken: string, deviceName?: string) =>
    invoke<SpotifyAccountInfo>("spotify_connect", {
      pkceToken,
      deviceName: deviceName ?? null,
      backend: null,
    }),
  spotifyDisconnect: () => invoke<void>("spotify_disconnect"),
  spotifyLogout: () => invoke<void>("spotify_logout"),
  spotifyAccount: () => invoke<SpotifyAccountInfo>("spotify_account"),
  spotifyLoad: (uri: string, startPlaying = true) => invoke<void>("spotify_load", { uri, startPlaying }),
  spotifySearch: (query: string, limit = 20) => invoke<Track[]>("spotify_search", { query, limit }),
  policyStatus: () => invoke<PolicyStatus>("spotify_policy_status"),

  // ---- Playlists locales (carpeta propia + manifiesto m3u8) ----
  playlistList: () => invoke<PlaylistInfo[]>("playlist_list"),
  playlistCreate: (name: string) => invoke<PlaylistInfo>("playlist_create", { name }),
  playlistRename: (oldName: string, newName: string) =>
    invoke<PlaylistInfo>("playlist_rename", { oldName, newName }),
  playlistDelete: (name: string) => invoke<void>("playlist_delete", { name }),
  playlistTracks: (name: string) => invoke<Track[]>("playlist_tracks", { name }),
  playlistAdd: (name: string, track: Track) => invoke<boolean>("playlist_add", { name, track }),
  playlistRemove: (name: string, path: string) => invoke<boolean>("playlist_remove", { name, path }),
  // S4: fija otra versión de una pista ytmusic:// (misma posición y metadatos).
  playlistReplaceTrack: (name: string, oldPath: string, track: Track) =>
    invoke<boolean>("playlist_replace_track", { name, oldPath, track }),
  playlistAddFolder: (name: string, folder: string) =>
    invoke<number>("playlist_add_folder", { name, folder }),

  // ---- Importación de listas desde CSV (S2) ----
  csvPreview: (path: string) => invoke<CsvPreview>("csv_preview", { path }),
  // S3: emparejado bajo demanda (efímero hasta que S4 guarda la lista).
  csvMatch: (title: string, artist: string, album: string, durationS: number | null, warm = false) =>
    invoke<CsvMatchResult>("csv_match", { title, artist, album, durationS, warm }),
  // S4: persistencia en playlist local (m3u8 con ytmusic:// y metadatos
  // originales del CSV) y candidatos frescos para «Buscar otra versión».
  csvSavePlaylist: (name: string, tracks: Track[], replace = false) =>
    invoke<SaveOutcome>("csv_save_playlist", { name, tracks, replace }),
  csvVariants: (title: string, artist: string, album: string, durationS: number, query?: string | null) =>
    invoke<VariantRow[]>("csv_variants", { title, artist, album, durationS, query: query ?? null }),

  // ---- YouTube Music (yt-dlp, solo streaming) ----
  ytProbe: () => invoke<YtProbe>("youtube_probe"),
  ytImport: (url: string, start = 1, end = 100) =>
    invoke<YtImport>("youtube_import", { url, start, end }),
  ytAccount: () => invoke<YtAccount>("youtube_account"),
  ytVerifySession: () => invoke<YtSessionCheck>("youtube_verify_session"),
  ytConnect: (browser: string, profile?: string | null) =>
    invoke<YtAccount>("youtube_connect", { browser, profile: profile ?? null }),
  ytBrowsers: () => invoke<BrowserProfile[]>("youtube_browsers"),
  ytImportCookies: (path: string) => invoke<YtAccount>("youtube_import_cookies", { path }),
  ytGoogleLogin: () => invoke<void>("youtube_google_login"),
  ytClearLoginData: () => invoke<void>("youtube_clear_login_data"),
  ytCached: (paths: string[]) => invoke<boolean[]>("youtube_cached", { paths }),
  ytDead: (paths: string[]) => invoke<(string | null)[]>("youtube_dead", { paths }),
  // S4.2b: segundos que quedan del backoff de límite de YouTube (0 = libre).
  ytThrottleSecs: () => invoke<number>("youtube_throttle_secs"),
  ytWarm: (paths: string[]) => invoke<void>("youtube_warm", { paths }).catch(() => {}),
  // W0: vista por lotes (sin filas hasta que su lote esté procesado).
  ytBatchOpen: (url: string) => invoke<YtBatchSnapshot>("youtube_batch_open", { url }),
  ytBatchProgress: () => invoke<YtBatchSnapshot>("youtube_batch_progress"),
  ytBatchMore: () => invoke<YtBatchSnapshot>("youtube_batch_more"),
  ytBatchCancel: () => invoke<void>("youtube_batch_cancel"),
  ytLikeStatus: (videoId: string) => invoke<boolean | null>("youtube_like_status", { videoId }),
  ytSetLike: (videoId: string, liked: boolean) =>
    invoke<void>("youtube_set_like", { videoId, liked }),
  ytSetPlaylistLike: (playlistId: string, saved: boolean) =>
    invoke<void>("youtube_set_playlist_like", { playlistId, saved }),
  // P6: home personalizado; `force` salta la caché en memoria de 10 min.
  ytRecommendations: (force = false) =>
    invoke<RecoFeed>("youtube_recommendations", { force }),
  reportError: (message: string) => invoke<void>("report_error", { message }),
  ytDisconnect: () => invoke<void>("youtube_disconnect"),
  ytMyPlaylists: () => invoke<YtMine[]>("youtube_my_playlists"),
  ytSetChannel: (url: string) => invoke<void>("youtube_set_channel", { url }),
  downloadStart: (items: DownloadItem[], opts: DownloadOptions) =>
    invoke<DownloadSnapshot>("download_start", { items, opts }),
  downloadStartPlaylist: (url: string, opts: DownloadOptions, items?: DownloadItem[] | null, title?: string | null) =>
    invoke<DownloadSnapshot>("download_start_playlist", { url, opts, items: items ?? null, title: title ?? null }),
  downloadCancel: () => invoke<DownloadSnapshot>("download_cancel"),
  downloadProgress: () => invoke<DownloadSnapshot>("download_progress"),
  downloadEnv: () => invoke<DownloadEnv>("download_env"),
};

export function fmtSecs(s: number): string {
  if (!isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  return `${m}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
}
