<script lang="ts">
  import {
    closeTab,
    openLocalTerminal,
    openSettings,
    quickConnect,
    toggleFiles,
    toggleFullscreen,
    toggleSidebar,
  } from './lib/actions';
  import CommandPalette from './lib/components/CommandPalette.svelte';
  import ConfirmDialog from './lib/components/ConfirmDialog.svelte';
  import ContextMenu from './lib/components/ContextMenu.svelte';
  import FilesDrawer from './lib/components/FilesDrawer.svelte';
  import ForwardsDialog from './lib/components/ForwardsDialog.svelte';
  import Home from './lib/components/Home.svelte';
  import PermissionsDialog from './lib/components/PermissionsDialog.svelte';
  import ServerEditor from './lib/components/ServerEditor.svelte';
  import SettingsDialog from './lib/components/SettingsDialog.svelte';
  import Sidebar from './lib/components/Sidebar.svelte';
  import TabBar from './lib/components/TabBar.svelte';
  import TerminalPane from './lib/components/TerminalPane.svelte';
  import Toasts from './lib/components/Toasts.svelte';
  import { isMac, shortcutKey } from './lib/platform';
  import AgentRequest from './lib/components/AgentRequest.svelte';
  import { agents } from './lib/state/agents.svelte';
  import { app } from './lib/state/app.svelte';
  import { files } from './lib/state/files.svelte';
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
    const key = shortcutKey(e);
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

    // New local terminal: Ctrl+Shift+` on every system, like VS Code.
    if (e.ctrlKey && shift && !e.altKey && !e.metaKey && e.code === 'Backquote') {
      consume();
      openLocalTerminal();
      return;
    }

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

    // The terminal's font size, like a browser's zoom: Mod+= (or Mod++), Mod+- and Mod+0, also
    // on the number pad. The shell has no use for them (Ctrl+_, its undo, stays with Shift).
    if (key === '=' || key === '+') {
      consume();
      app.zoom(1);
      return;
    }
    if (!shift && (key === '-' || key === '0')) {
      consume();
      app.zoom(key === '-' ? -1 : 0);
      return;
    }

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

    // Files drawer: Mod+Shift+E everywhere.
    if (shift && key === 'e') {
      consume();
      toggleFiles();
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
  {#if !app.sidebarHidden}
    <Sidebar />
  {/if}
  <main>
    <TabBar />
    <div class="surface">
      <div class="terminals">
        {#each sessions.tabs as tab (tab.key)}
          <TerminalPane {tab} active={tab.key === sessions.activeKey} />
        {/each}
        {#if !sessions.active}
          <Home />
        {/if}
      </div>
      {#if files.open && sessions.active?.kind === 'ssh'}
        <FilesDrawer tab={sessions.active} />
      {/if}
    </div>
  </main>
</div>

{#if app.palette.open}<CommandPalette />{/if}
{#if app.editor}<ServerEditor editor={app.editor} />{/if}
{#if app.settingsOpen}<SettingsDialog />{/if}
{#if app.forwardsOpen}<ForwardsDialog />{/if}
{#if app.confirmation}<ConfirmDialog request={app.confirmation} />{/if}
{#if app.permissions}<PermissionsDialog request={app.permissions} />{/if}
{#if agents.request}
  {#key agents.request.id}<AgentRequest request={agents.request} />{/key}
{/if}
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
    display: flex;
    flex: 1;
    min-height: 0;
    margin: 0 6px 6px;
    border-radius: var(--radius);
    border: 1px solid var(--border);
    background: var(--surface);
    overflow: hidden;
  }
  .terminals {
    position: relative;
    flex: 1;
    min-width: 0;
  }
</style>
