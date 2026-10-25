<!-- U3/M4: contenido de Ajustes en el estilo «Mac 1-bit» (Window + Btn + filas
     y controles rectos; sin colores: todo es tinta sobre papel). Mismos textos,
     mismos botones y mismos handlers que la versión previa; el estado sigue en
     App.svelte y llega como props/callbacks (aquí no se llama a api ni se
     cambia lógica). Esc lo gestiona ObModal, así que onKey de App.svelte no se
     toca. M4: la navegación de la izquierda solo desplaza a cada sección —no
     oculta ninguna—; antes las seis secciones eran un único scroll. -->
<script lang="ts">
  import ObModal from "./onebit/ObModal.svelte";
  import Btn from "./onebit/Btn.svelte";
  import Icon16 from "./onebit/Icon16.svelte";
  import type { BrowserProfile, DownloadEnv, SpotifyAccountInfo, YtAccount, YtMine } from "../tauri";
  import type { DlSettings } from "../dlsettings";

  let {
    onclose,
    // Sesión de YouTube
    ytAccount = null,
    ytVerifyMsg = null,
    ytExpired = false,
    ytLoginMsg = null,
    ytLoggingIn = false,
    ytVerifying = false,
    ytBrowsers = [],
    ytBrowser = $bindable(""),
    ytCustomPath = $bindable(""),
    ytChannel = $bindable(""),
    ytDataOffer = false,
    ytProbeOk = null,
    ytProbeInfo = null,
    showYtMine = false,
    ytMine = [],
    onConnectYt,
    onDisconnectYt,
    onVerifyYt,
    onImportYtCookies,
    onStartYtGoogleLogin,
    onClearYtLoginData,
    onKeepYtData,
    onSaveYtChannel,
    onToggleYtMine,
    onOpenYtMine,
    onCheckYt,
    // Descargas
    dl,
    dlEnv = null,
    onDlMode,
    onDlFormat,
    onSetDl,
    onChangeDlFolder,
    // Audio
    engine = "libmpv",
    spotify = null,
    onSetEngine,
    // Spotify
    pkceToken = $bindable(""),
    onConnectSpotify,
    onDisconnectSpotify,
    onLogoutSpotify,
    // Apariencia
    themeDark = false,
    onToggleTheme,
    sections,
    onSectionToggle,
    focus = null,
  }: {
    onclose: () => void;
    ytAccount?: YtAccount | null;
    ytVerifyMsg?: string | null;
    ytExpired?: boolean;
    ytLoginMsg?: string | null;
    ytLoggingIn?: boolean;
    ytVerifying?: boolean;
    ytBrowsers?: BrowserProfile[];
    ytBrowser?: string;
    ytCustomPath?: string;
    ytChannel?: string;
    ytDataOffer?: boolean;
    ytProbeOk?: boolean | null;
    ytProbeInfo?: string | null;
    showYtMine?: boolean;
    ytMine?: YtMine[];
    onConnectYt: () => void;
    onDisconnectYt: () => void;
    onVerifyYt: () => void;
    onImportYtCookies: () => void;
    onStartYtGoogleLogin: () => void;
    onClearYtLoginData: () => void;
    onKeepYtData: () => void;
    onSaveYtChannel: () => void;
    onToggleYtMine: () => void;
    onOpenYtMine: (m: YtMine) => void;
    onCheckYt: () => void;
    dl: DlSettings;
    dlEnv?: DownloadEnv | null;
    onDlMode: (e: Event) => void;
    onDlFormat: (e: Event) => void;
    onSetDl: (patch: Partial<DlSettings>) => void;
    onChangeDlFolder: () => void;
    engine?: "libmpv" | "spotify";
    spotify?: SpotifyAccountInfo | null;
    onSetEngine: (kind: "libmpv" | "spotify") => void;
    pkceToken?: string;
    onConnectSpotify: () => void;
    onDisconnectSpotify: () => void;
    onLogoutSpotify: () => void;
    themeDark?: boolean;
    onToggleTheme: () => void;
    sections: { musica: boolean; youtube: boolean; radios: boolean; servers: boolean; spotify: boolean };
    onSectionToggle: (k: "musica" | "youtube" | "radios" | "servers" | "spotify", v: boolean) => void;
    focus?: string | null;
  } = $props();

  const RM =
    typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  const SECTION_LABELS: { key: "musica" | "youtube" | "radios" | "servers" | "spotify"; label: string }[] = [
    { key: "musica", label: "Mi música" },
    { key: "youtube", label: "YouTube" },
    { key: "radios", label: "Radios" },
    { key: "servers", label: "Servidores" },
    { key: "spotify", label: "Spotify" },
  ];

  // Navegación lateral: mismos títulos que los <h3> de cada sección.
  const SECCIONES = [
    { id: "yt", label: "Sesión de YouTube" },
    { id: "dl", label: "Descargas" },
    { id: "audio", label: "Audio" },
    { id: "spotify", label: "Spotify" },
    { id: "apariencia", label: "Apariencia" },
    { id: "avanzado", label: "Avanzado" },
  ] as const;

  let actual = $state<string>("yt");
  let paneEl: HTMLElement | null = $state(null);
  let spotifySecEl: HTMLElement | null = $state(null);

  /// Desplaza el panel a la sección elegida (no se oculta ninguna).
  function irA(id: string) {
    actual = id;
    const i = SECCIONES.findIndex((s) => s.id === id);
    const secs = paneEl?.querySelectorAll<HTMLElement>(".ob-sec");
    secs?.[i]?.scrollIntoView({ block: "start", behavior: RM ? "auto" : "smooth" });
  }

  // U4b: al abrirse desde la tarjeta Spotify del asistente, se desplaza a esa
  // sección (el resto de aperturas no cambia).
  $effect(() => {
    if (focus === "spotify") {
      actual = "spotify";
      spotifySecEl?.scrollIntoView({ block: "start", behavior: RM ? "auto" : "smooth" });
    }
  });
</script>

<ObModal title="Ajustes" ancho={760} focoCerrar onclose={onclose}>
  <div class="ob-set">
    <nav class="ob-set-nav" aria-label="Secciones de Ajustes">
      {#each SECCIONES as s (s.id)}
        <button class:on={actual === s.id} onclick={() => irA(s.id)}>{s.label}</button>
      {/each}
    </nav>
    <div class="ob-set-pane" bind:this={paneEl}>

      <section class="ob-sec">
        <h3>Sesión de YouTube</h3>
        {#if ytProbeOk === false && !ytProbeInfo}
          <span class="hint">yt-dlp no instalado: <b>sudo pacman -S yt-dlp</b> (solo streaming, no descarga nada)</span>
        {:else if ytProbeInfo}
          <span class="hint">{ytProbeInfo}</span>
        {/if}
        {#if ytAccount?.connected}
          <span>Sesión YouTube: <b>{ytAccount.browser ?? "navegador"}</b></span>
          <span class="hint">La sesión también se usa en las descargas: evita el aviso de YouTube «confirma que no eres un bot».</span>
          <span class="hint">Para cambiar de cuenta: pulsa «Cerrar sesión», reinicia la aplicación e inicia sesión con la nueva cuenta. Si añades otra cuenta sin cerrar sesión antes, se usará la primera.</span>
          <div class="ob-row">
            <Btn onclick={onVerifyYt} disabled={ytVerifying} title="Comprueba ahora que la sesión sigue viva">Verificar sesión</Btn>
            <Btn onclick={onDisconnectYt} title="Borra cookies y metadatos"><Icon16 name="logout" size={15} /> Cerrar sesión</Btn>
          </div>
          <div class="ob-row">
            <input placeholder="URL de tu canal (opcional, para tus playlists)" bind:value={ytChannel} style="min-width:220px;" />
            <Btn onclick={onSaveYtChannel} title="Guardar canal">Guardar canal</Btn>
            <Btn onclick={onToggleYtMine} title="Ver mis playlists">
              <Icon16 name="playlist" size={15} /> Mis playlists
            </Btn>
          </div>
        {:else}
          {#if ytAccount?.unverified}
            <span class="hint">Sesión sin verificar.</span>
            <Btn onclick={onVerifyYt} disabled={ytVerifying} title="Comprueba ahora que la sesión sigue viva">Verificar sesión</Btn>
          {/if}
          {#if ytBrowsers.length > 0}
            <span class="select-wrap">
              <select bind:value={ytBrowser} title="Navegador y perfil con la sesión de YouTube">
                {#each ytBrowsers as b (b.label)}
                  <option value={b.label}>{b.label}</option>
                {/each}
              </select>
            </span>
          {:else}
            <span class="hint">No se encontró ningún navegador con cookies.</span>
          {/if}
          <input placeholder="Ruta del perfil (opcional: Flatpak/Snap/otro)" bind:value={ytCustomPath} style="min-width:200px;" />
          <div class="ob-row">
            <Btn primary onclick={onConnectYt} title="Exporta cookies del navegador (sin contraseña; menos estable que el inicio integrado o cookies.txt)">
              <Icon16 name="connect" size={15} /> Conectar cuenta
            </Btn>
            <Btn onclick={onImportYtCookies} title="Usa un cookies.txt exportado del navegador">
              Importar cookies.txt
            </Btn>
            <Btn onclick={onStartYtGoogleLogin} disabled={ytLoggingIn} title="Ventana de Google con datos aislados (sin contraseñas); se cierra sola al validar la sesión">
              <Icon16 name="connect" size={15} /> Iniciar sesión con Google
            </Btn>
          </div>
          {#if ytDataOffer}
            <span class="hint">Quedan cookies y datos de la ventana de inicio de sesión integrado. ¿Borrar solo ese directorio?</span>
            <div class="ob-row">
              <Btn onclick={onClearYtLoginData} title="Borra solo el directorio de datos de la ventana de inicio de sesión integrado">Borrar datos de la ventana</Btn>
              <Btn onclick={onKeepYtData}>Conservar</Btn>
            </div>
          {/if}
          <span class="hint">Recomendado: «Iniciar sesión con Google» o «Importar cookies.txt» (exporta desde una ventana privada de music.youtube.com) — más estables que leer las cookies del navegador, que falla con el navegador abierto o el llavero bloqueado. Sin contraseña, nunca.</span>
        {/if}
        {#if ytVerifyMsg}
          <span class="hint">{ytVerifyMsg}</span>
        {/if}
        {#if ytLoginMsg}
          <span class="hint">{ytLoginMsg}</span>
        {/if}
        {#if ytExpired}
          <span class="hint">Sesión expirada. <Btn onclick={onConnectYt}>Reconectar</Btn></span>
        {/if}
        {#if showYtMine && ytAccount?.connected}
          <div class="ob-mine">
            {#if ytMine.length === 0}
              <span class="hint">Sin playlists visibles (¿canal configurado?). Pega cualquier link con list=…</span>
            {/if}
            {#each ytMine as m (m.url || m.name)}
              <Btn onclick={() => onOpenYtMine(m)} disabled={!m.url} title={m.url || m.name}>
                <Icon16 name="list" size={14} /> {m.name}
              </Btn>
            {/each}
          </div>
        {/if}
      </section>

      <section class="ob-sec">
        <h3>Descargas</h3>
        <span class="hint">Descargas de YouTube Music (yt-dlp, sin cuenta ni cookies)</span>
        <span class="select-wrap">
          <select value={dl.mode} onchange={onDlMode} title="Dónde guardar las descargas">
            <option value="default">Carpeta por defecto</option>
            <option value="ask">Preguntar cada vez</option>
          </select>
        </span>
        <div class="ob-row">
          <span class="hint" title={dl.folder}>Carpeta: <b>{dl.folder || "(sin elegir: se preguntará)"}</b></span>
          <Btn onclick={onChangeDlFolder} title="Elegir la carpeta por defecto con el selector del sistema">Cambiar</Btn>
        </div>
        <span class="select-wrap">
          <select value={dl.format} onchange={onDlFormat}
            title="Formato de audio (sin ffmpeg solo cabe guardar el original)">
            <option value="original" disabled={dlEnv != null && !dlEnv.ffmpeg}>Original (sin recodificar; suele ser webm/opus)</option>
            <option value="m4a" disabled={dlEnv != null && !dlEnv.ffmpeg}>m4a (AAC; requiere ffmpeg)</option>
            <option value="mp3" disabled={dlEnv != null && !dlEnv.ffmpeg}>mp3 + portada (requiere ffmpeg)</option>
          </select>
        </span>
        <label class="ob-check hint">
          <input type="checkbox" checked={dl.embed} disabled={dlEnv == null || !dlEnv.ffmpeg}
            title={dl.format === "original"
              ? "En modo Original solo se incrustan etiquetas (la portada necesita m4a o mp3)"
              : "Incrustar portada y etiquetas en el archivo"}
            onchange={(e) => onSetDl({ embed: (e.currentTarget as HTMLInputElement).checked })} />
          Portada y etiquetas
        </label>
        <label class="ob-check hint">
          <input type="checkbox" checked={dl.prefix}
            onchange={(e) => onSetDl({ prefix: (e.currentTarget as HTMLInputElement).checked })} />
          Prefijo "01 - " en playlists
        </label>
        {#if dl.format === "m4a"}
          <span class="hint" title="Datos medidos con yt-dlp 2026.08.19 + ffmpeg">
            Códec real del m4a: AAC. Si la fuente ya es m4a (~128 kb/s) solo cambia el contenedor (remux, sin recodificar);
            si la fuente es opus, se recodifica a AAC y el archivo puede pesar más.
          </span>
        {/if}
        {#if dlEnv != null && !dlEnv.ffmpeg}
          <span class="hint ob-alerta" title={dlEnv.ffmpegHint}>ffmpeg no instalado: m4a, mp3 y portada deshabilitados; se guardará el original. {dlEnv.ffmpegHint}</span>
        {/if}
      </section>

      <section class="ob-sec">
        <h3>Audio</h3>
        <div class="ob-row">
          <span class="ob-engine" title="Motor de audio activo">
            <Icon16 name={engine === "spotify" ? "note" : "disc"} size={14} />
            {engine === "spotify" ? "Spotify Connect" : "libmpv"}
          </span>
          {#if spotify?.connected}
            <Btn onclick={() => onSetEngine(engine === "spotify" ? "libmpv" : "spotify")} title="Cambiar de motor sin perder la cola">
              {engine === "spotify" ? "→ libmpv" : "→ Spotify"}
            </Btn>
          {/if}
        </div>
        <span class="hint">Motor activo. Con Spotify conectado (ver «Spotify») puedes alternar libmpv ↔ Spotify Connect sin perder la cola.</span>
      </section>

      <section class="ob-sec" bind:this={spotifySecEl}>
        <h3>Spotify</h3>
        {#if spotify?.connected}
          <span>Conectado como <b>{spotify.username ?? "usuario"}</b> · dispositivo "{spotify.device_name}" · plan {spotify.tier ?? "?"} {spotify.persistent_keyring ? "" : " · keyring solo en memoria (sin Secret Service)"}</span>
          <span class="hint">Sin anuncios en este motor (librespot no los gestiona): las cuentas gratuitas no son compatibles, se requiere Premium.</span>
          <div class="ob-row">
            <Btn primary onclick={() => onSetEngine("spotify")}>Activar Spotify</Btn>
            <Btn onclick={onDisconnectSpotify}>Desconectar</Btn>
            <Btn onclick={onLogoutSpotify} title="Borra credenciales del keyring">Cerrar sesión</Btn>
          </div>
        {:else}
          <div class="ob-row">
            <input type="password" placeholder="Token PKCE de Spotify (sesión OAuth)" bind:value={pkceToken} />
            <Btn primary onclick={onConnectSpotify} disabled={pkceToken.length === 0}>Conectar (verifica Premium)</Btn>
          </div>
          <span class="hint">librespot solo funciona con cuentas Premium; las Free se rechazan antes de inicializar el motor.</span>
        {/if}
      </section>

      <section class="ob-sec">
        <h3>Apariencia</h3>
        <div class="ob-row">
          <Btn onclick={onToggleTheme} title="Tema claro/oscuro">
            <Icon16 name={themeDark ? "sun" : "moon"} size={15} /> Tema: {themeDark ? "oscuro" : "claro"}
          </Btn>
        </div>
        <span class="hint">Mostrar secciones (barra lateral)</span>
        {#each SECTION_LABELS as s (s.key)}
          <label class="ob-check">
            <input type="checkbox" checked={sections[s.key]}
              onchange={(e) => onSectionToggle(s.key, (e.currentTarget as HTMLInputElement).checked)} />
            {s.label}
          </label>
        {/each}
      </section>

      <section class="ob-sec">
        <h3>Avanzado</h3>
        <span class="hint">Diagnóstico</span>
        {#if ytProbeOk === false && !ytProbeInfo}
          <span class="hint">yt-dlp: no instalado (sudo pacman -S yt-dlp)</span>
        {:else if ytProbeInfo}
          <span class="hint">{ytProbeInfo}</span>
        {:else}
          <span class="hint">yt-dlp: sin comprobar todavía</span>
        {/if}
        {#if dlEnv != null}
          <span class="hint">
            ffmpeg: {dlEnv.ffmpeg ? "instalado (m4a, mp3 y portada disponibles)" : `no instalado — ${dlEnv.ffmpegHint}`}
          </span>
        {/if}
        <div class="ob-row">
          <Btn onclick={onCheckYt} title="Vuelve a comprobar yt-dlp y su runtime">Comprobar yt-dlp</Btn>
        </div>
        <span class="hint">El registro de errores de la app se guarda en musica.log dentro de su carpeta de datos.</span>
      </section>

    </div>
  </div>
</ObModal>
