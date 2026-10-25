<!-- U2/M4: confirmación genérica en el estilo «Mac 1-bit» (ventana ObModal +
     Btn; role=alertdialog). Esc cierra; el foco va a Cancelar para que la
     acción destructiva no quede a un Enter de distancia. El padre decide qué
     hacer al confirmar. -->
<script lang="ts">
  import ObModal from "./onebit/ObModal.svelte";
  import Btn from "./onebit/Btn.svelte";

  let {
    title,
    message = "",
    confirmLabel = "Eliminar",
    onconfirm,
    onclose,
  }: {
    title: string;
    message?: string;
    confirmLabel?: string;
    onconfirm: () => void;
    onclose: () => void;
  } = $props();

  let cancelEl: HTMLButtonElement | null = $state(null);
  $effect(() => { cancelEl?.focus(); });
</script>

<ObModal {title} role="alertdialog" ancho={420} onclose={onclose}>
  {#if message}
    <div class="ob-nota">{message}</div>
  {/if}
  <div class="ob-acts">
    <Btn primary onclick={onconfirm}>{confirmLabel}</Btn>
    <Btn bind:el={cancelEl} onclick={onclose}>Cancelar</Btn>
  </div>
</ObModal>
