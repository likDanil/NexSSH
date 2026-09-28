<script lang="ts">
  import { app, type MenuItem } from '../state/app.svelte';
  import Icon from './Icon.svelte';

  let el = $state<HTMLDivElement>();
  let pos = $state({ x: 0, y: 0 });
  let focused = $state(-1);

  const items = $derived((app.menu?.items ?? []).map((e, i) => ({ e, i })));
  const actionable = $derived(
    items.filter((x): x is { e: MenuItem; i: number } => x.e !== 'separator' && !x.e.disabled).map((x) => x.i),
  );

  // Keep the menu inside the window.
  $effect(() => {
    const menu = app.menu;
    if (!menu || !el) return;
    const rect = el.getBoundingClientRect();
    pos = {
      x: Math.min(menu.x, window.innerWidth - rect.width - 6),
      y: menu.y + rect.height > window.innerHeight - 6 ? Math.max(6, menu.y - rect.height) : menu.y,
    };
    focused = -1;
    el.focus();
  });

  function close() {
    app.menu = null;
  }

  function run(item: MenuItem) {
    close();
    item.action();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (!actionable.length) return;
      const at = actionable.indexOf(focused);
      const next = e.key === 'ArrowDown' ? at + 1 : at - 1;
      focused = actionable[(next + actionable.length) % actionable.length];
    } else if (e.key === 'Enter' && focused >= 0) {
      e.preventDefault();
      const entry = app.menu?.items[focused];
      if (entry && entry !== 'separator') run(entry);
    }
  }
</script>

<svelte:window onblur={close} onresize={close} />

{#if app.menu}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="scrim" onmousedown={close} oncontextmenu={(e) => (e.preventDefault(), close())}></div>
  <div
    class="menu"
    role="menu"
    tabindex="-1"
    bind:this={el}
    style:left="{pos.x}px"
    style:top="{pos.y}px"
    {onkeydown}
  >
    {#each items as { e, i } (i)}
      {#if e === 'separator'}
        <div class="sep"></div>
      {:else}
        <button
          role="menuitem"
          class:danger={e.danger}
          class:focused={focused === i}
          disabled={e.disabled}
          onmouseenter={() => (focused = i)}
          onclick={() => run(e)}
        >
          <span class="icon">{#if e.icon}<Icon name={e.icon} size={15} />{/if}</span>
          <span class="text">{e.label}</span>
          {#if e.hint}<span class="hint">{e.hint}</span>{/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
  }
  .menu {
    position: fixed;
    z-index: 61;
    min-width: 200px;
    max-width: 320px;
    padding: 5px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: var(--elevated);
    box-shadow: var(--shadow);
    animation: pop 0.1s var(--ease);
  }
  button {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    height: 30px;
    padding: 0 9px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    text-align: left;
  }
  button.focused {
    background: var(--active);
  }
  button:disabled {
    opacity: 0.45;
  }
  .icon {
    display: grid;
    place-items: center;
    width: 16px;
    color: var(--text-2);
  }
  .text {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hint {
    color: var(--text-3);
    font-size: 11.5px;
  }
  .danger,
  .danger .icon {
    color: var(--red);
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--border-soft);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(0.98);
    }
  }
</style>
