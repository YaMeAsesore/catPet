<!-- U2/M4: diálogo de texto para renombrar en el estilo «Mac 1-bit» (ventana
     ObModal + Btn; sin radios ni colores). Esc lo cierra ObModal, Enter guarda;
     el campo recibe el foco con el texto preseleccionado (igual que antes). -->
<script lang="ts">
  import { untrack } from "svelte";
  import ObModal from "./onebit/ObModal.svelte";
  import Btn from "./onebit/Btn.svelte";

  let {
    title,
    value = "",
    label = "Nombre",
    onsave,
    onclose,
  }: {
    title: string;
    value?: string;
    label?: string;
    onsave: (v: string) => void;
    onclose: () => void;
  } = $props();

  // untrack: el borrador parte del valor inicial; los cambios del padre no
  // pisan lo que el usuario escribe mientras el diálogo está abierto.
  let draft = $state(untrack(() => value));
  let inputEl: HTMLInputElement | null = $state(null);
  $effect(() => { inputEl?.focus(); inputEl?.select(); });

  function save() {
    const v = draft.trim();
    if (!v) return;
    onsave(v);
  }
</script>

<ObModal {title} ancho={380} onclose={onclose}>
  <input class="ob-input" placeholder={label} aria-label={label} bind:value={draft}
    bind:this={inputEl} onkeydown={(e) => { if (e.key === "Enter") save(); }} />
  <div class="ob-acts">
    <Btn primary onclick={save} disabled={draft.trim().length === 0}>Guardar</Btn>
    <Btn onclick={onclose}>Cancelar</Btn>
  </div>
</ObModal>
