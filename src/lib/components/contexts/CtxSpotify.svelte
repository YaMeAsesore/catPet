<!-- M3: contexto «Spotify» (motor librespot activo). Re-hogariza los dos
     controles del contexto que en la clásica viven en la barra de fuentes:
     el cambio de motor («→ libmpv», mismo handler setEngine que Ajustes ›
     Audio) y la entrada a la cuenta. Con la cuenta conectada el botón cambia
     de motor; sin conectar, la acción principal abre Ajustes › Spotify (donde
     viven token PKCE, conectar, desconectar y cerrar sesión desde M2). El
     aviso de restricción del motor (spotify://restriction) sigue en la barra
     de avisos compartida, ya en tinta/papel. Sin lógica: handlers de App. -->
<script lang="ts">
  import ContextBar from "./ContextBar.svelte";
  import Btn from "../onebit/Btn.svelte";
  import Icon16 from "../onebit/Icon16.svelte";

  let {
    title,
    subtitle,
    connected = false,
    onSwitchEngine,
    onOpenSettings,
  }: {
    title: string;
    subtitle: string;
    connected?: boolean;
    onSwitchEngine?: () => void;
    onOpenSettings?: () => void;
  } = $props();
</script>

<ContextBar ctx="spotify" {title} subtitle={subtitle}>
  {#snippet actions()}
    {#if connected}
      <Btn small onclick={onSwitchEngine} title="Cambiar de motor sin perder la cola"><Icon16 name="disc" size={14} /> → libmpv</Btn>
    {:else}
      <Btn primary small onclick={onOpenSettings} title="Conectar la cuenta en Ajustes › Spotify">Conectar Spotify…</Btn>
    {/if}
    <Btn small onclick={onOpenSettings} title="Cuenta, dispositivo y clave en Ajustes › Spotify"><Icon16 name="sliders" size={14} /> Ajustes de Spotify…</Btn>
  {/snippet}
</ContextBar>
