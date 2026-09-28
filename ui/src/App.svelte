<script lang="ts">
  import { closeTab, openSettings, quickConnect, toggleFullscreen, toggleSidebar } from './lib/actions';
  import CommandPalette from './lib/components/CommandPalette.svelte';
  import ConfirmDialog from './lib/components/ConfirmDialog.svelte';
  import ContextMenu from './lib/components/ContextMenu.svelte';
  import ForwardsDialog from './lib/components/ForwardsDialog.svelte';
  import Home from './lib/components/Home.svelte';
  import ServerEditor from './lib/components/ServerEditor.svelte';
  import SettingsDialog from './lib/components/SettingsDialog.svelte';
  import Sidebar from './lib/components/Sidebar.svelte';
  import TabBar from './lib/components/TabBar.svelte';
  import TerminalPane from './lib/components/TerminalPane.svelte';
  import Toasts from './lib/components/Toasts.svelte';
  import { isMac } from './lib/platform';
  import { app } from './lib/state/app.svelte';
  import { sessions } from './lib/state/sessions.svelte';

  function inTerminal(e: KeyboardEvent): boolean {
    return (e.target as HTMLElement | null)?.classList?.contains('xterm-helper-textarea') ?? false;
  }

  /**
   * App shortcuts, handled in the capture phase before xterm sees the key.
   * Windows/Linux use Ctrl+Shift combinations (like other terminals) so plain Ctrl+<key>
   * keeps working in the shell: Ctrl+W, Ctrl+K, Ctrl+B, Ctrl+R all mean something there.
   */
  function onkeydown(e: KeyboardEvent) {
    const mac = isMac();
    const mod = mac ? e.metaKey : e.ctrlKey;
    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    const shift = e.shiftKey;
    const consume = () => {
      e.preventDefault();
      e.stopPropagation();
    };

    // Command palette.
    const plainCtrlK = !mac && e.ctrlKey && !shift && !e.altKey && key === 'k';
    if (
      (mac && e.metaKey && !shift && key === 'k') ||
      (!mac && e.ctrlKey && shift && key === 'k') ||
      (plainCtrlK && (!inTerminal(e) || app.settings.ctrlKInTerminal))
    ) {
      consume();
      if (app.palette.open) app.closePalette();
      else app.openPalette('commands');
      return;
    }

    if (app.overlayOpen || app.menu) return;

    if (key === 'F11' && !mac) {
      consume();
      toggleFullscreen();
      return;
    }
    if (mac && e.metaKey && e.ctrlKey && key === 'f') {
      consume();
      toggleFullscreen();
      return;
    }
    if (!mod || e.altKey) return;

    // Mod+1..9 selects a tab (9 = last).
    if (!shift && /^[1-9]$/.test(key)) {
      consume();
      sessions.select(key === '9' ? -1 : Number(key) - 1);
      return;
    }
    if (key === 'Tab' && e.ctrlKey) {
      consume();
      sessions.cycle(shift ? -1 : 1);
      return;
    }
    if (!mac && (key === 'PageDown' || key === 'PageUp') && e.ctrlKey) {
      consume();
      sessions.cycle(key === 'PageDown' ? 1 : -1);
      return;
    }
    if (mac && shift && (key === ']' || key === '[' || key === '}' || key === '{')) {
      consume();
      sessions.cycle(key === ']' || key === '}' ? 1 : -1);
      return;
    }
    if (key === ',' && !shift) {
      consume();
      openSettings();
      return;
    }

    // On Windows/Linux the remaining shortcuts need Shift.
    if (!mac && !shift) return;
    switch (key) {
      case 't':
        consume();
        quickConnect();
        break;
      case 'w':
        if (sessions.active) {
          consume();
          closeTab(sessions.active);
        }
        break;
      case 'b':
        consume();
        toggleSidebar();
        break;
      case 'r':
        if (sessions.active?.status === 'disconnected') {
          consume();
          sessions.reconnect(sessions.active);
        }
        break;
    }
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="app">
  {#if !app.settings.sidebarHidden}
    <Sidebar />
  {/if}
  <main>
    <TabBar />
    <div class="surface">
      {#each sessions.tabs as tab (tab.key)}
        <TerminalPane {tab} active={tab.key === sessions.activeKey} />
      {/each}
      {#if !sessions.active}
        <Home />
      {/if}
    </div>
  </main>
</div>

{#if app.palette.open}<CommandPalette />{/if}
{#if app.editor}<ServerEditor editor={app.editor} />{/if}
{#if app.settingsOpen}<SettingsDialog />{/if}
{#if app.forwardsOpen}<ForwardsDialog />{/if}
{#if app.confirmation}<ConfirmDialog request={app.confirmation} />{/if}
<ContextMenu />
<Toasts />

<style>
  .app {
    display: flex;
    height: 100%;
  }
  main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .surface {
    position: relative;
    flex: 1;
    min-height: 0;
    margin: 0 6px 6px 0;
    border-radius: var(--radius);
    border: 1px solid var(--border);
    background: var(--surface);
    overflow: hidden;
  }
  :global(:root[data-os='linux']) .surface,
  :global(.app:not(:has(.sidebar))) .surface {
    margin-left: 6px;
  }
</style>
