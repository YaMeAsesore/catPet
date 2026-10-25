<!-- U4/M4: panel flotante de descargas con chip (interfaz nueva), en el estilo
     «Mac 1-bit»: borde de 2 px, esquinas rectas y sin colores (avisos en
     negrita, no en color). Mismos datos y acciones que el panel clásico:
     cancelar el lote y cerrar el resumen; el estado (dlSnap/dlDone) vive en
     App.svelte y llega como props. El chip vive en la barra de menú (derecha)
     y el panel se despliega debajo; iconos 16×16 del set 1-bit. -->
<script lang="ts">
  import Icon16 from "./onebit/Icon16.svelte";
  import type { DownloadSnapshot } from "../tauri";

  let {
    snap = null,
    done = null,
    folderText = "",
    onCancel,
    onClose,
  }: {
    snap?: DownloadSnapshot | null;
    done?: DownloadSnapshot | null;
    folderText?: string;
    onCancel: () => void;
    onClose: () => void;
  } = $props();

  // Chip o panel: el panel se abre solo al empezar un lote y el chip permite
  // plegarlo sin perder el progreso.
  let open = $state(true);
  let wasActive = $state(false);
  $effect(() => {
    const active = snap?.active === true;
    if (active && !wasActive) open = true;
    wasActive = active;
  });
</script>

{#if snap != null && snap.active}
  {#if open}
    <div class="ob-dlpanel" role="status">
      <div class="ob-dl-head">
        <Icon16 name="download" size={15} />
        <b>Descargando</b> <span class="hint">{snap.finished}/{snap.total}</span>
        <button class="icon small ob-dl-x" onclick={() => (open = false)} title="Minimizar (chip)">—</button>
        <button class="icon small ob-dl-x" onclick={onCancel} title="Cancelar descargas (no deja archivos .part)"><Icon16 name="remove" size={13} /></button>
      </div>
      {#if snap.current}<div class="ob-dl-now" title={snap.current}>{snap.current}</div>{/if}
      <div class="ob-dl-sub">{snap.downloaded} descargadas{snap.failed > 0 ? ` · ${snap.failed} fallidas` : ""}</div>
      {#if snap.notice}<div class="ob-dl-sub ob-dl-warn" title={snap.notice}>⚠ {snap.notice}</div>{/if}
    </div>
  {:else}
    <button class="ob-dlchip" onclick={() => (open = true)} title="Ver descargas en curso">
      <Icon16 name="download" size={14} /> {snap.finished}/{snap.total}
      {#if snap.failed > 0}<span class="ob-dl-warn">· {snap.failed} fallidas</span>{/if}
    </button>
  {/if}
{:else if done != null}
  {#if open}
    <div class="ob-dlpanel" role="status">
      <div class="ob-dl-head">
        <Icon16 name="download" size={15} />
        <b>Descargas</b>
        <button class="icon small ob-dl-x" onclick={() => (open = false)} title="Minimizar (chip)">—</button>
        <button class="icon small ob-dl-x" onclick={onClose} title="Cerrar resumen"><Icon16 name="remove" size={13} /></button>
      </div>
      <div class="ob-dl-now">{done.summary}</div>
      {#if done.folder}
        <div class="ob-dl-sub" title={done.folder}>Carpeta: {folderText}</div>
      {/if}
      {#if done.failures.length > 0}
        <ul class="ob-dl-fails">
          {#each done.failures.slice(0, 4) as f}
            <li title={f.motivo}>{f.title} — {f.motivo}</li>
          {/each}
        </ul>
      {/if}
      <div class="ob-dl-foot">
        <button class="small" onclick={onClose}>Cerrar</button>
      </div>
    </div>
  {:else}
    <button class="ob-dlchip" onclick={() => (open = true)} title="Ver el resumen de descargas">
      <Icon16 name="download" size={14} /> Descargas
      {#if done.failed > 0}<span class="ob-dl-warn">· {done.failed} fallidas</span>{/if}
    </button>
  {/if}
{/if}
