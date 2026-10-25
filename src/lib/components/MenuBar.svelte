<!-- M2: barra de menú de la interfaz nueva «Mac 1-bit». Menús Archivo /
     Fuente / Ver / Ayuda con teclado completo: ↓↑ abren y recorren (con salto
     circular), Enter/Espacio activan (activación nativa del botón), Esc cierra
     y suelta el foco, ←/→ cambian de menú e Inicio/Fin van a los extremos.
     Con los menús CERRADOS no queda ningún oyente ni se consume tecla alguna:
     Ctrl+K, Espacio, N, P y las flechas siguen llegando intactas a la app
     (los oyentes de ratón/Escape solo existen mientras hay un menú abierto).
     A la derecha vive el estado: contador, sesión de YouTube, aviso de límite
     y el chip de descargas (snippet). Presentación pura: cada entrada llama a
     una función que pasa la app; aquí no hay lógica de negocio. -->
<script module lang="ts">
  export type MenuEntry = {
    label?: string;
    icon?: string;
    checked?: boolean;
    disabled?: boolean;
    onSelect?: () => void;
    /** Separador: texto opcional; sin texto es una línea. */
    sep?: string;
  };
  export type Menu = { label: string; items: MenuEntry[] };
</script>

<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon16 from "./onebit/Icon16.svelte";
  import BrandMark from "./BrandMark.svelte";

  let {
    menus = [],
    searchEl = $bindable<HTMLInputElement | null>(null),
    query = $bindable(""),
    onSearchInput,
    searchDisabled = false,
    searchTitle = "Buscar (Ctrl+K)",
    countText = "",
    sessionOk = false,
    sessionText = "",
    limitSecs = 0,
    dl,
    onHome,
  }: {
    menus?: Menu[];
    searchEl?: HTMLInputElement | null;
    query?: string;
    onSearchInput: (e: Event) => void;
    searchDisabled?: boolean;
    searchTitle?: string;
    countText?: string;
    sessionOk?: boolean;
    sessionText?: string;
    limitSecs?: number;
    dl?: Snippet;
    onHome?: () => void;
  } = $props();

  let open = $state<number | null>(null);
  let barEl: HTMLDivElement | null = $state(null);
  let panelEl: HTMLDivElement | null = $state(null);

  function panelButtons(): HTMLButtonElement[] {
    if (!panelEl) return [];
    return Array.from(panelEl.querySelectorAll<HTMLButtonElement>("button:not([disabled])"));
  }

  function focusEdge(which: "first" | "last") {
    const bs = panelButtons();
    if (bs.length === 0) return;
    (which === "first" ? bs[0] : bs[bs.length - 1]).focus();
  }

  function closeMenus() {
    open = null;
  }

  /// Esc: cierra y suelta el foco (si estaba en la barra). Así Espacio, N, P,
  /// Ctrl+K y las flechas vuelven a ser de la app: ningún botón del menú las
  /// retiene tras cerrarlo.
  function closeAndRelease() {
    closeMenus();
    const a = document.activeElement as HTMLElement | null;
    if (a && barEl?.contains(a)) a.blur();
  }

  function onBtnClick(e: MouseEvent, i: number) {
    if (open === i) {
      closeMenus();
      return;
    }
    open = i;
    // Enter/Espacio nativos (detail 0) o clic de teclado: entra al primer ítem.
    if (e.detail === 0) requestAnimationFrame(() => focusEdge("first"));
  }

  function onBtnKey(e: KeyboardEvent, i: number) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      e.stopPropagation();
      if (open !== i) open = i;
      requestAnimationFrame(() => focusEdge(e.key === "ArrowDown" ? "first" : "last"));
    } else if (e.key === "Escape" && open === i) {
      e.preventDefault();
      closeAndRelease();
    }
  }

  function onPanelKey(e: KeyboardEvent) {
    const bs = panelButtons();
    const idx = bs.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      e.stopPropagation();
      if (bs.length === 0) return;
      const d = e.key === "ArrowDown" ? 1 : -1;
      const next = idx < 0 ? (d === 1 ? 0 : bs.length - 1) : (idx + d + bs.length) % bs.length;
      bs[next].focus();
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      e.stopPropagation();
      focusEdge(e.key === "Home" ? "first" : "last");
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeAndRelease();
    } else if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && open != null && menus.length > 1) {
      e.preventDefault();
      e.stopPropagation();
      const d = e.key === "ArrowRight" ? 1 : -1;
      open = (open + d + menus.length) % menus.length;
      requestAnimationFrame(() => focusEdge("first"));
    }
    // Resto de teclas (Ctrl+K, Espacio, N, P, Tab…): no se tocan; si el foco
    // sale de la barra, focusout cierra el menú.
  }

  function pick(it: MenuEntry) {
    if (it.disabled || !it.onSelect) return;
    closeMenus();
    it.onSelect();
  }

  function onHover(i: number) {
    // Convención de barra de menú: con uno abierto, pasar por otro lo cambia.
    if (open != null && open !== i) open = i;
  }

  function onFocusOut(e: FocusEvent) {
    if (open == null) return;
    const to = e.relatedTarget as Node | null;
    if (!to || !barEl?.contains(to)) closeMenus();
  }

  // Mientras hay un menú abierto: clic fuera y Escape (el foco puede estar
  // fuera si se abrió con el ratón) cierran. Al cerrar se retira TODO: con los
  // menús cerrados no queda ningún oyente que pueda capturar teclas.
  $effect(() => {
    if (open == null) return;
    const onDoc = (e: MouseEvent) => {
      const t = e.target as Node | null;
      if (t && barEl?.contains(t)) return;
      closeMenus();
    };
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeAndRelease();
    };
    document.addEventListener("mousedown", onDoc, true);
    window.addEventListener("keydown", onEsc);
    return () => {
      document.removeEventListener("mousedown", onDoc, true);
      window.removeEventListener("keydown", onEsc);
    };
  });
</script>

<div class="mb">
  <!-- Marca de la app: abre la biblioteca (misma acción que Fuente › Biblioteca). -->
  <button type="button" class="mb-brand" title="Música — Biblioteca" aria-label="Música — ir a la biblioteca"
    onclick={() => onHome?.()}>
    <BrandMark size={22} title="" />
    <span class="mb-brand-name">Música</span>
  </button>

  <div class="mb-menus" role="menubar" aria-label="Menú principal" bind:this={barEl} onfocusout={onFocusOut}>
    {#each menus as m, i (m.label)}
      <div class="mb-item">
        <button type="button" class="mb-btn" class:open={open === i}
          onmouseenter={() => onHover(i)}
          aria-haspopup="menu" aria-expanded={open === i}
          onclick={(e) => onBtnClick(e, i)} onkeydown={(e) => onBtnKey(e, i)}>{m.label}</button>
        {#if open === i}
          <div class="mb-panel" role="menu" aria-label={m.label} tabindex="-1" bind:this={panelEl} onkeydown={onPanelKey}>
            {#each m.items as it, k (k)}
              {#if it.sep !== undefined}
                <div class="mb-sep" role="separator">{it.sep}</div>
              {:else}
                <button type="button" class="mb-entry"
                  role={it.checked !== undefined ? "menuitemcheckbox" : "menuitem"}
                  aria-checked={it.checked !== undefined ? it.checked : undefined}
                  disabled={it.disabled === true} onclick={() => pick(it)}>
                  <span class="mb-check">
                    {#if it.checked !== undefined}
                      {#if it.checked}<Icon16 name="check" size={13} />{/if}
                    {:else if it.icon}
                      <Icon16 name={it.icon} size={14} />
                    {/if}
                  </span>
                  <span class="mb-lbl" title={it.label}>{it.label}</span>
                </button>
              {/if}
            {/each}
          </div>
        {/if}
      </div>
    {/each}
  </div>

  <div class="mb-search">
    <Icon16 name="search" size={14} />
    <input type="search" bind:this={searchEl} bind:value={query} oninput={onSearchInput}
      placeholder="Buscar…" disabled={searchDisabled} title={searchTitle} aria-label="Buscar" />
  </div>

  <div class="mb-status">
    {#if countText}
      <span class="mb-count" title={countText}>{countText}</span>
    {/if}
    <span class="mb-session" title={`Sesión de YouTube — ${sessionText}`}>
      <span class="mb-dot" class:ok={sessionOk}></span>
      <span class="mb-session-txt">{sessionText}</span>
    </span>
    {#if limitSecs > 0}
      <span class="mb-limit" title="YouTube está limitando las peticiones; se reanuda solo">límite YT · {limitSecs} s</span>
    {/if}
    {#if dl}<span class="mb-dl">{@render dl()}</span>{/if}
  </div>
</div>
