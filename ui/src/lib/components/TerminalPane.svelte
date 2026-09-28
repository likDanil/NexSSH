<script lang="ts">
  import { onMount } from 'svelte';
  import { closeTab, sessionMenu } from '../actions';
  import { app, type MenuEntry } from '../state/app.svelte';
  import { sessions, type Tab } from '../state/sessions.svelte';
  import type { TermSettings, TermView } from '../terminal';
  import { TERMINAL_THEMES } from '../themes';
  import Icon from './Icon.svelte';
  import PromptCard from './PromptCard.svelte';

  interface Props {
    tab: Tab;
    active: boolean;
  }
  let { tab, active }: Props = $props();

  let host: HTMLDivElement;
  // xterm objects must not become reactive proxies.
  let view: TermView | null = null;
  let loaded = $state(false);
  let frame = 0;

  const termSettings: TermSettings = $derived({
    fontFamily: app.settings.fontFamily,
    fontSize: app.settings.fontSize,
    lineHeight: app.settings.lineHeight,
    cursorStyle: app.settings.cursorStyle,
    cursorBlink: app.settings.cursorBlink,
    scrollback: app.settings.scrollback,
    copyOnSelect: app.settings.copyOnSelect,
    gpuAcceleration: app.settings.gpuAcceleration,
  });
  const theme = $derived(TERMINAL_THEMES[app.theme]);

  function fitSoon() {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      if (active && view && host.clientWidth > 0) view.fit();
    });
  }

  onMount(() => {
    let disposed = false;
    let observer: ResizeObserver | undefined;
    void (async () => {
      const mod = await import('../terminal');
      await mod.ensureFont(app.settings.fontSize);
      if (disposed) return;
      const v = new mod.TermView(host, termSettings, theme, {
        onData: (data) => sessions.input(tab.key, data),
        onBinary: (data) => sessions.inputBinary(tab.key, data),
        onResize: (cols, rows) => sessions.resize(tab.key, cols, rows),
        onContextMenu: (e, tv) => contextMenu(e, tv),
      });
      view = v;
      v.fit();
      loaded = true;
      observer = new ResizeObserver(fitSoon);
      observer.observe(host);
      if (active && !app.overlayOpen) v.focus();
      void sessions.start(tab.key, v, v.term.cols, v.term.rows);
    })();
    return () => {
      disposed = true;
      cancelAnimationFrame(frame);
      observer?.disconnect();
      sessions.detach(tab.key);
      view?.dispose();
      view = null;
    };
  });

  // Re-fit and focus when the tab becomes visible.
  $effect(() => {
    if (active && loaded) {
      fitSoon();
      if (!app.overlayOpen && !tab.prompts.length) requestAnimationFrame(() => view?.focus());
    }
  });

  // Live settings/theme changes.
  $effect(() => {
    const s = termSettings;
    const t = theme;
    if (loaded && view) {
      view.apply(s, t);
      fitSoon();
    }
  });

  function contextMenu(e: MouseEvent, v: TermView) {
    if (app.settings.rightClickPaste) {
      if (v.term.hasSelection()) {
        void v.copy().then(() => v.term.clearSelection());
      } else {
        void v.paste();
      }
      return;
    }
    const items: MenuEntry[] = [
      { label: 'Copy', icon: 'copy', disabled: !v.term.hasSelection(), action: () => void v.copy() },
      { label: 'Paste', icon: 'paste', action: () => void v.paste().then(() => v.focus()) },
      { label: 'Select all', action: () => v.term.selectAll() },
      'separator',
      { label: 'Clear terminal', icon: 'eraser', action: () => v.clear() },
    ];
    items.push('separator', {
      label: 'Session…',
      icon: 'more',
      action: () => sessionMenu(tab, e.clientX, e.clientY),
    });
    app.showMenu(e.clientX, e.clientY, items);
  }
</script>

<div class="pane" class:active>
  <div class="host" bind:this={host}></div>

  {#if tab.status === 'connecting' && !tab.prompts.length}
    <div class="progress" aria-hidden="true"></div>
  {/if}

  {#if tab.prompts.length}
    {#key tab.prompts[0].id}
      <PromptCard {tab} entry={tab.prompts[0]} />
    {/key}
  {/if}

  {#if tab.status === 'disconnected'}
    <div class="banner" class:failed={tab.failed}>
      <span class="msg" title={tab.message ?? ''}>{tab.message}</span>
      <button class="btn" onclick={() => sessions.reconnect(tab)}>
        <Icon name="refresh" size={14} /> Reconnect <kbd>Enter</kbd>
      </button>
      <button class="icon-btn small" title="Close tab" aria-label="Close tab" onclick={() => closeTab(tab)}>
        <Icon name="x" size={13} />
      </button>
    </div>
  {/if}
</div>

<style>
  .pane {
    position: absolute;
    inset: 0;
    display: none;
  }
  .pane.active {
    display: block;
  }
  .host {
    position: absolute;
    inset: 12px 6px 8px 16px;
  }
  .host :global(.xterm) {
    height: 100%;
  }
  .host :global(.xterm-viewport) {
    background-color: transparent !important;
  }
  .progress {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 2px;
    overflow: hidden;
    border-radius: var(--radius) var(--radius) 0 0;
  }
  .progress::after {
    content: '';
    position: absolute;
    inset: 0;
    width: 30%;
    background: linear-gradient(90deg, transparent, var(--amber), transparent);
    animation: slide 1.3s var(--ease) infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
  .banner {
    position: absolute;
    left: 50%;
    bottom: 18px;
    z-index: 4;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: calc(100% - 40px);
    padding: 6px 6px 6px 14px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--bg);
    box-shadow: var(--shadow);
    animation: rise 0.18s var(--ease);
  }
  .banner.failed {
    border-color: color-mix(in srgb, var(--red) 35%, var(--border));
  }
  .msg {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text-2);
  }
  .failed .msg {
    color: var(--red);
  }
  .banner kbd {
    margin-left: 2px;
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translate(-50%, 6px);
    }
  }
</style>
