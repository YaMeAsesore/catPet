<!-- U4b/U5/M4: asistente «+ Añadir» en el estilo «Mac 1-bit»: ventana
     (ObModal) + tarjetas rectas con borde de 2 px y sombra dura (selección =
     inversión). Carpeta de música usa pickFolder; Spotify abre Ajustes;
     Playlist local reutiliza savePlist (deduplicación « (2)» del backend); las
     acciones «Añadir carpeta/canciones» solo se habilitan tras crear la
     playlist nueva y avisan con «Se añadirá a: …». El resto de tarjetas llevan
     al paso con los campos existentes (mismos campos, validaciones y acciones).
     El estado y los handlers siguen en App.svelte. Esc lo cierra ObModal y el
     foco entra en la primera tarjeta/campo de cada paso (igual que antes). -->
<script lang="ts">
  import { untrack } from "svelte";
  import ObModal from "./onebit/ObModal.svelte";
  import Btn from "./onebit/Btn.svelte";
  import Icon16 from "./onebit/Icon16.svelte";

  let {
    initialKind = null,
    fKind = $bindable("subsonic"),
    fName = $bindable(""),
    fUrl = $bindable(""),
    fUser = $bindable(""),
    fSecret = $bindable(""),
    ytLink = $bindable(""),
    ytLoading = false,
    ytImportReady = false,
    ytProbeOk = null,
    ytProbeInfo = null,
    plistDraft = $bindable(""),
    activePlist = null,
    onSave,
    onImport,
    onSaveYtSource,
    onFolder,
    onOpenSpotify,
    onCsvImport,
    onSavePlaylist,
    onAddFolder,
    onAddFiles,
    onclose,
  }: {
    initialKind?: string | null;
    fKind?: string;
    fName?: string;
    fUrl?: string;
    fUser?: string;
    fSecret?: string;
    ytLink?: string;
    ytLoading?: boolean;
    ytImportReady?: boolean;
    ytProbeOk?: boolean | null;
    ytProbeInfo?: string | null;
    plistDraft?: string;
    activePlist?: string | null;
    onSave: () => void;
    onImport: () => void;
    onSaveYtSource: () => void;
    onFolder: () => void;
    onOpenSpotify: () => void;
    onCsvImport: () => void;
    onSavePlaylist: () => void | Promise<void>;
    onAddFolder: () => void;
    onAddFiles: () => void;
    onclose: () => void;
  } = $props();

  const KIND_LABELS: Record<string, string> = {
    subsonic: "Subsonic/Navidrome",
    jellyfin: "Jellyfin",
    radio: "Radio",
    podcast: "Podcast",
    youtube: "YouTube Music",
  };

  type Step = "cards" | "form" | "playlist";
  // El paso inicial se fija al montar (el diálogo se monta de cero en cada
  // apertura): «Pegar link» entra directo al formulario de YouTube y «Nueva
  // playlist…» del popover de fila entra directo al paso de playlist (U6a.1).
  const initial = untrack(() => initialKind);
  let step = $state<Step>(initial == null ? "cards" : initial === "playlist" ? "playlist" : "form");
  let bodyEl: HTMLDivElement | null = $state(null);
  let firstCardEl: HTMLButtonElement | null = $state(null);
  // U5: las acciones de añadir se habilitan solo cuando la creación de la
  // playlist nueva se completó (activePlist cambia de valor); así el aviso
  // «Se añadirá a: …» nombra siempre una playlist recién creada aquí.
  let created = $state(false);

  async function saveNewPlist() {
    const before = activePlist;
    await onSavePlaylist();
    if (activePlist != null && activePlist !== before) created = true;
  }

  // Foco por paso: tarjetas → primera tarjeta; formulario/playlist → primer
  // campo. Así el teclado recorre todo sin ratón (Esc cierra en todos).
  $effect(() => {
    if (step === "cards") {
      firstCardEl?.focus();
      return;
    }
    const el =
      bodyEl?.querySelector<HTMLElement>("input") ??
      bodyEl?.querySelector<HTMLElement>("select");
    el?.focus();
  });
</script>

<ObModal title="Añadir" ancho={560} onclose={onclose}>
  <div bind:this={bodyEl}>
    {#if step === "cards"}
      <section class="ob-sec">
        <h3>¿Qué quieres añadir?</h3>
        <div class="ob-add-cards">
          <button class="ob-add-card" bind:this={firstCardEl} onclick={onFolder}>
            <Icon16 name="folder" size={18} /><span>Carpeta de música</span>
          </button>
          <button class="ob-add-card" onclick={() => (step = "playlist")}>
            <Icon16 name="list" size={18} /><span>Playlist local</span>
          </button>
          <button class="ob-add-card" onclick={onCsvImport}>
            <Icon16 name="download" size={18} /><span>Importar desde CSV</span>
          </button>
          <button class="ob-add-card" onclick={() => { fKind = "youtube"; step = "form"; }}>
            <Icon16 name="play" size={18} /><span>YouTube Music</span>
          </button>
          <button class="ob-add-card" onclick={() => { fKind = "radio"; step = "form"; }}>
            <Icon16 name="radio" size={18} /><span>Radio</span>
          </button>
          <button class="ob-add-card" onclick={() => { fKind = "podcast"; step = "form"; }}>
            <Icon16 name="mic" size={18} /><span>Podcast</span>
          </button>
          <button class="ob-add-card" onclick={() => { fKind = "subsonic"; step = "form"; }}>
            <Icon16 name="server" size={18} /><span>Subsonic/Navidrome</span>
          </button>
          <button class="ob-add-card" onclick={() => { fKind = "jellyfin"; step = "form"; }}>
            <Icon16 name="connect" size={18} /><span>Jellyfin</span>
          </button>
          <button class="ob-add-card" onclick={onOpenSpotify}>
            <Icon16 name="note" size={18} /><span>Spotify</span>
          </button>
        </div>
        <div class="ob-acts">
          <Btn onclick={onclose}>Cancelar</Btn>
        </div>
      </section>
    {:else if step === "playlist"}
      <section class="ob-sec">
        <h3>Playlist local</h3>
        <div class="ob-row">
          <input placeholder="Nombre de la playlist" bind:value={plistDraft}
            onkeydown={(e) => { if (e.key === "Enter") saveNewPlist(); }} />
          <Btn primary onclick={saveNewPlist} disabled={plistDraft.trim().length === 0}>Guardar</Btn>
        </div>
        {#if created && activePlist != null}
          <span class="hint">Se añadirá a: <b>{activePlist}</b></span>
          <div class="ob-row">
            <Btn primary onclick={onAddFolder}><Icon16 name="plus" size={14} /> Añadir carpeta…</Btn>
            <Btn onclick={onAddFiles}><Icon16 name="plus" size={14} /> Añadir canciones…</Btn>
          </div>
        {:else}
          <span class="hint">Crea la playlist y se habilitarán «Añadir carpeta» y «Añadir canciones».</span>
        {/if}
        <div class="ob-acts">
          <Btn onclick={() => (step = "cards")}>← Tipos</Btn>
          <Btn onclick={onclose}>Cancelar</Btn>
        </div>
      </section>
    {:else}
      <section class="ob-sec">
        <h3>{KIND_LABELS[fKind] ?? "Fuente"}</h3>
        <select bind:value={fKind} title="Tipo">
          <option value="subsonic">Subsonic/Navidrome</option>
          <option value="jellyfin">Jellyfin</option>
          <option value="radio">Radio (stream o Radio Garden)</option>
          <option value="podcast">Podcast (RSS)</option>
          <option value="youtube">YouTube Music</option>
        </select>
        {#if fKind === "youtube"}
          <input placeholder="Pegar link de playlist o video" bind:value={ytLink} />
          <div class="ob-row">
            <Btn primary onclick={onImport} disabled={ytLink.trim().length === 0 || ytLoading}>
              {ytLoading ? "Importando…" : "Importar"}
            </Btn>
            <Btn onclick={onSaveYtSource} disabled={!ytImportReady} title="Guardar la importación actual como fuente">Guardar como fuente</Btn>
          </div>
          {#if ytProbeOk === false && !ytProbeInfo}
            <span class="hint">yt-dlp no instalado: <b>sudo pacman -S yt-dlp</b> (solo streaming, no descarga nada)</span>
          {:else if ytProbeInfo}
            <span class="hint">{ytProbeInfo}</span>
          {/if}
          <span class="hint">La sesión de YouTube se gestiona en Ajustes (engranaje de la cabecera).</span>
        {:else}
          <input placeholder={fKind === "radio" ? "Nombre (opcional)" : "Nombre"} bind:value={fName} />
          <input placeholder={fKind === "radio" ? "URL del stream o de Radio Garden" : "URL base o feed"} bind:value={fUrl} />
          {#if fKind === "radio"}
            <span class="hint">Acepta un stream directo (.mp3, .aac, .ogg, .m3u8), una emisora de radio.garden (<code>/listen/...</code>) o páginas compatibles como Streema y LiveOnlineRadio. El nombre es opcional.</span>
          {/if}
          {#if fKind === "subsonic" || fKind === "jellyfin"}
            <input placeholder="Usuario" bind:value={fUser} />
            <input type="password" placeholder="Contraseña (no se guarda)" bind:value={fSecret} />
          {/if}
          <div class="ob-row">
            <Btn primary onclick={onSave}
              disabled={fUrl.trim().length === 0 || (fKind !== "radio" && fName.trim().length === 0)}>Probar y guardar</Btn>
          </div>
        {/if}
        <div class="ob-acts">
          <Btn onclick={() => (step = "cards")}>← Tipos</Btn>
          <Btn onclick={onclose}>Cancelar</Btn>
        </div>
      </section>
    {/if}
  </div>
</ObModal>
