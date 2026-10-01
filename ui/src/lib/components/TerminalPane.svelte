<script lang="ts">
  import { onMount } from 'svelte';
  import { closeTab, sessionMenu } from '../actions';
  import { api, errorMessage } from '../api';
  import { t } from '../i18n.svelte';
  import { shortcut } from '../platform';
  import { app, type MenuEntry } from '../state/app.svelte';
  import { sessions, type Tab } from '../state/sessions.svelte';
  import { shells } from '../state/shells.svelte';
  import { toasts } from '../state/toasts.svelte';
  import type { HoveredLink, TermSettings, TermView } from '../terminal';
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
  // The link under the mouse, shown with how to open it.
  let link = $state<HoveredLink | null>(null);
  let linkTimer: ReturnType<typeof setTimeout> | undefined;

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
      // A local terminal on Windows runs in ConPTY, which xterm.js adapts to by the build.
      if (tab.kind === 'local' && app.info?.os === 'windows') await shells.load();
      if (disposed) return;
      const conpty =
        tab.kind === 'local' && app.info?.os === 'windows'
          ? { backend: 'conpty' as const, buildNumber: shells.windowsBuild ?? undefined }
          : undefined;
      const v = new mod.TermView(
        host,
        termSettings,
        theme,
        {
          onData: (data) => sessions.input(tab.key, data),
          onBinary: (data) => sessions.inputBinary(tab.key, data),
          onResize: (cols, rows) => sessions.resize(tab.key, cols, rows),
          onContextMenu: (e, tv) => contextMenu(e, tv),
          onLinkOpen: openLink,
          onLinkHover: hoverLink,
          onZoom: (step) => app.zoom(step),
          allowPaste: (text, bracketed) => app.allowPaste(text, bracketed),
        },
        conpty,
      );
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
      clearTimeout(linkTimer);
      observer?.disconnect();
      sessions.detach(tab.key);
      view?.dispose();
      view = null;
    };
  });

  // Re-fit and focus when the tab becomes visible, or a dialog closes; a dialog opened from
  // the files drawer gives the focus back to the drawer instead.
  $effect(() => {
    if (active && loaded) {
      fitSoon();
      if (!app.overlayOpen && !tab.prompts.length) {
        requestAnimationFrame(() => {
          if (!document.activeElement?.closest('[data-keep-focus]')) view?.focus();
        });
      }
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

  function hoverLink(next: HoveredLink | null) {
    clearTimeout(linkTimer);
    if (!next) {
      link = null;
      return;
    }
    // When the mouse rests on it, not while it passes by.
    linkTimer = setTimeout(() => (link = next), 400);
  }

  function openLink(uri: string) {
    hoverLink(null);
    api.openLink(uri).catch((e) => toasts.error(errorMessage(e)));
  }

  function copyLink(uri: string) {
    navigator.clipboard
      .writeText(uri)
      .then(() => toasts.show(t('terminal.linkCopied')))
      .catch(() => {});
  }

  /** Next to the mouse, on the side of it with room. */
  function tipPosition(l: HoveredLink): string {
    const x = l.x > window.innerWidth * 0.6 ? `right: ${window.innerWidth - l.x + 8}px` : `left: ${l.x + 12}px`;
    const y = l.y > window.innerHeight - 90 ? `bottom: ${window.innerHeight - l.y + 10}px` : `top: ${l.y + 18}px`;
    return `${x}; ${y}`;
  }

  function contextMenu(e: MouseEvent, v: TermView) {
    const uri = v.link;
    hoverLink(null);
    if (app.settings.rightClickPaste) {
      if (v.term.hasSelection()) {
        void v.copy().then(() => v.term.clearSelection());
      } else {
        void v.paste();
      }
      return;
    }
    const items: MenuEntry[] = [];
    if (uri) {
      items.push(
        { label: t('terminal.openLink'), icon: 'globe', action: () => openLink(uri) },
        { label: t('terminal.copyLink'), icon: 'link', action: () => copyLink(uri) },
        'separator',
      );
    }
    items.push(
      { label: t('terminal.copy'), icon: 'copy', disabled: !v.term.hasSelection(), action: () => void v.copy() },
      { label: t('terminal.paste'), icon: 'paste', action: () => void v.paste().then(() => v.focus()) },
      { label: t('terminal.selectAll'), action: () => v.term.selectAll() },
      'separator',
      { label: t('session.clear'), icon: 'eraser', action: () => v.clear() },
    );
    items.push('separator', {
      label: t('session.menu'),
      icon: 'more',
      action: () => sessionMenu(tab, e.clientX, e.clientY),
    });
    app.showMenu(e.clientX, e.clientY, items);
  }
</script>

<div class="pane" class:active>
  <div class="host" bind:this={host}></div>

  {#if active && app.zoomBadge !== null}
    <div class="zoom-badge" aria-live="polite">{t('terminal.fontSize', { size: app.zoomBadge })}</div>
  {/if}

  {#if link && active}
    <div class="link-tip" style={tipPosition(link)} role="tooltip">
      {#if link.hyperlink}<span class="uri">{link.uri}</span>{/if}
      <span>{t('terminal.linkHint', { key: shortcut('Mod') })}</span>
    </div>
  {/if}

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
        <Icon name="refresh" size={14} />
        {t(tab.kind === 'local' ? 'session.restart' : 'session.reconnect')} <kbd>Enter</kbd>
      </button>
      <button class="icon-btn small" title={t('session.closeTab')} aria-label={t('session.closeTab')} onclick={() => closeTab(tab)}>
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
  /* A rounded bar like the app's other scrollbars, slimmer than the strip that takes the mouse
     (xterm's 14 px, which the fit addon keeps free); a little wider under the mouse. The colors
     are the theme's: xterm sets them with `background`, which would paint under the border too,
     hence a selector that outweighs its own. */
  .host :global(.xterm .xterm-scrollable-element > .scrollbar > .slider) {
    box-sizing: border-box;
    border: 3px solid transparent;
    border-inline-width: 4px;
    border-radius: 7px;
    background-clip: padding-box;
    transition: border-inline-width 0.12s var(--ease);
  }
  .host :global(.xterm .xterm-scrollable-element > .scrollbar > .slider:hover),
  .host :global(.xterm .xterm-scrollable-element > .scrollbar > .slider.active) {
    border-inline-width: 3px;
  }
  .zoom-badge {
    position: absolute;
    top: 12px;
    right: 18px;
    z-index: 5;
    padding: 4px 12px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--bg);
    box-shadow: var(--shadow);
    color: var(--text-2);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    pointer-events: none;
  }
  .link-tip {
    position: fixed;
    z-index: 30;
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-width: min(560px, calc(100vw - 24px));
    padding: 5px 9px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg);
    box-shadow: var(--shadow);
    color: var(--text-2);
    font-size: 12px;
    pointer-events: none;
  }
  .link-tip .uri {
    color: var(--text);
    overflow-wrap: anywhere;
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
