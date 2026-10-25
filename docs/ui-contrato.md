# Contrato congelado de UI (fase U0)

Generado en la rama `feat/ui-rediseno` (base `f304f28`, 2026-10-15) ANTES de
tocar nada. Es el patrón anti-regresión del rediseño: en U6 se repiten los
mismos greps y **cada entrada sigue referenciada; ningún conteo baja**. Si algo
desaparece, la fase es un fallo (aunque "se vea bien").

Los conteos cubren `src/App.svelte` (2049 líneas). Los componentes nuevos
(Sidebar, PlayerBar, Queue, SettingsDialog, AddDialog, TrackTable, Popover,
DlPanel) son presentación: reciben props/callbacks; **las llamadas `api.*`,
los `listen(...)` y los atajos permanecen en App.svelte**.

## (a) Llamadas `api.*` usadas por App.svelte — 59 nombres, 82 referencias

Verificación U6:

```bash
grep -o "api\.[a-zA-Z]*" src/App.svelte | sort | uniq -c
```
Cada nombre debe seguir apareciendo con conteo **≥** al de la tabla.

| api.* | n | comando invoke (tauri.ts) |
|---|---|---|
| playlistAdd | 6 | playlist_add |
| volume | 4 | player_volume |
| ytAccount | 3 | youtube_account |
| seek | 3 | player_seek |
| playIndex | 3 | player_play_index |
| ytBatchOpen | 2 | youtube_batch_open |
| status | 2 | player_status |
| sourceAdd | 2 | source_add |
| search | 2 | library_search |
| reportError | 2 | report_error |
| queueSet | 2 | queue_set |
| queueMove | 2 | queue_move |
| queueList | 2 | queue_list |
| playbackStatus | 2 | playback_status |
| ytVerifySession | 1 | youtube_verify_session |
| ytSetChannel | 1 | youtube_set_channel |
| ytProbe | 1 | youtube_probe |
| ytMyPlaylists | 1 | youtube_my_playlists |
| ytImportCookies | 1 | youtube_import_cookies |
| ytGoogleLogin | 1 | youtube_google_login |
| ytDisconnect | 1 | youtube_disconnect |
| ytDead | 1 | youtube_dead |
| ytConnect | 1 | youtube_connect |
| ytClearLoginData | 1 | youtube_clear_login_data |
| ytCached | 1 | youtube_cached |
| ytBrowsers | 1 | youtube_browsers |
| ytBatchProgress | 1 | youtube_batch_progress |
| ytBatchMore | 1 | youtube_batch_more |
| ytBatchCancel | 1 | youtube_batch_cancel |
| trackEnded | 1 | player_track_ended |
| toggle | 1 | player_toggle |
| stats | 1 | library_stats |
| spotifyLogout | 1 | spotify_logout |
| spotifyDisconnect | 1 | spotify_disconnect |
| spotifyConnect | 1 | spotify_connect |
| spotifyAccount | 1 | spotify_account |
| sourceRemove | 1 | source_remove |
| sourceList | 1 | source_list |
| setEngine | 1 | playback_set_engine |
| scan | 1 | scan_folder |
| retryCurrent | 1 | player_retry_current |
| remoteSearch | 1 | remote_search |
| queueRemove | 1 | queue_remove |
| queueClear | 1 | queue_clear |
| queueAdd | 1 | queue_add |
| prev | 1 | player_prev |
| playlistTracks | 1 | playlist_tracks |
| playlistRename | 1 | playlist_rename |
| playlistRemove | 1 | playlist_remove |
| playlistList | 1 | playlist_list |
| playlistDelete | 1 | playlist_delete |
| playlistCreate | 1 | playlist_create |
| playlistAddFolder | 1 | playlist_add_folder |
| next | 1 | player_next |
| downloadStartPlaylist | 1 | download_start_playlist |
| downloadStart | 1 | download_start |
| downloadProgress | 1 | download_progress |
| downloadEnv | 1 | download_env |
| downloadCancel | 1 | download_cancel |

Wrappers de `tauri.ts` que App.svelte **no** usa hoy (no cuentan para el
contrato; no se borran de tauri.ts): `ytImport`, `ytWarm`, `spotifyLoad`,
`spotifySearch`, `policyStatus`, `play`, `pause`, `shuffle`, `repeat`.

## (b) Eventos `listen(...)` — 9

```bash
grep -n "listen" src/App.svelte
```
Cada literal de evento debe seguir apareciendo ≥1 vez (como llamada real).

| evento | handler (App.svelte U0) |
|---|---|
| queue://changed | onQueueChanged (:289) |
| library://changed | refresh (:233) |
| playback://state | onPlaybackState (:1181) |
| playback://track | onTrackChanged (:1191) |
| spotify://restriction | onRestriction (:1217) |
| youtube://expired | onYtExpired (:1000) |
| youtube://throttled | toast 4 s (:1361) |
| youtube://session | applyYtSession (:1007) |
| youtube://login | applyYtLogin (:1044) |

## (c) Atajos de teclado (onKey :1225 + Enter)

| tecla | acción | nota |
|---|---|---|
| Ctrl/Cmd+K | foco al buscador | preventDefault |
| Escape | cierra portada, menú de pista, menú de playlist, desplegable de biblioteca, confirmación de descarga | los diálogos NUEVOS (Ajustes, +Añadir, popovers) gestionarán su propio Esc en su componente: `onKey` no se toca |
| Espacio | play/pause | no en inputs; no cuando el foco es un BUTTON (nativo) |
| N | siguiente | `e.key === "n"` |
| P | anterior | `e.key === "p"` |
| ← / → | ±5 s | `api.seek` |
| Enter | guardar nombre de playlist (input :1757) y quitar de la cola (:1917) | |

## (d) Diálogos del sistema (plugin-dialog, 6 usos)

Ninguno se elimina; cambian de sitio en el flujo, no de comportamiento.

| uso | línea U0 |
|---|---|
| Carpeta de música (scan) | :332 |
| Carpeta para playlist | :574 |
| Canciones para playlist | :586 |
| Carpeta de una descarga | :641 |
| Carpeta por defecto de descargas | :649 |
| cookies.txt de YouTube | :949 |

## (e) Claves de localStorage — NO renombrar (ajustes de usuario)

- `musica.queueMin` (cola minimizada)
- `musica.dl.mode`, `musica.dl.folder`, `musica.dl.format`, `musica.dl.embed`, `musica.dl.prefix`, `musica.dl.v` (ajustes de descargas)

Claves NUEVAS permitidas (esta fase las declara antes de usarlas):
- `musica.ui2` — interruptor temporal de la interfaz nueva (se retira en U6).
- `musica.ui.sections` — "Mostrar secciones" (U4); por defecto todas visibles.

## (f) Reglas que no pueden romperse

1. La búsqueda sigue deshabilitada con una playlist local abierta (comportamiento actual).
2. Las acciones por fila siguen siendo botones HERMANOS de la fila (patrón T6: sin botones anidados ni clics perdidos).
3. `VirtualList` sigue siendo la única lista de pistas (windowinng; no se pinta la lista completa).
4. El arrastre de la cola (punteros, no HTML5 DnD) se conserva tal cual.
5. Los textos de estado de sesión se mantienen ("Sesión caducada: reconecta o importa cookies.txt", "No se pudo verificar ahora", etc.).
