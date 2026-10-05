// User actions shared by the sidebar, menus, the command palette and keyboard shortcuts.

import { api, errorMessage } from './api';
import { t } from './i18n.svelte';
import { shortcut } from './platform';
import { app, type MenuEntry, type SettingsSection } from './state/app.svelte';
import { edits } from './state/edits.svelte';
import { files } from './state/files.svelte';
import { destination, servers } from './state/servers.svelte';
import { sessions, type Tab } from './state/sessions.svelte';
import { CUSTOM_SHELL, commandName, shellName, shells } from './state/shells.svelte';
import { toasts } from './state/toasts.svelte';
import type { Server, ShellProfile } from './types';

export const keys = {
  palette: () => shortcut('Mod', 'K'),
  newSession: () => (isMacLike() ? shortcut('Mod', 'T') : shortcut('Mod', 'Shift', 'T')),
  closeTab: () => (isMacLike() ? shortcut('Mod', 'W') : shortcut('Mod', 'Shift', 'W')),
  settings: () => shortcut('Mod', ','),
  sidebar: () => (isMacLike() ? shortcut('Mod', 'B') : shortcut('Mod', 'Shift', 'B')),
  reconnect: () => (isMacLike() ? shortcut('Mod', 'R') : shortcut('Mod', 'Shift', 'R')),
  fullscreen: () => (isMacLike() ? shortcut('Ctrl', 'Mod', 'F') : 'F11'),
  files: () => shortcut('Mod', 'Shift', 'E'),
  /** Like VS Code's new terminal; Ctrl (not Cmd) on macOS too. */
  localTerminal: () => shortcut('Ctrl', 'Shift', '`'),
  zoomIn: () => shortcut('Mod', '='),
  zoomOut: () => shortcut('Mod', '−'),
  zoomReset: () => shortcut('Mod', '0'),
};

function isMacLike() {
  return document.documentElement.dataset.os === 'macos';
}

export function connect(server: Server, newTab = false) {
  app.menu = null;
  if (newTab) sessions.openServer(server);
  else sessions.focusOrOpen(server);
}

export function quickConnect() {
  app.openPalette('connect');
}

/** A local terminal with `shell`, or with the shell the settings choose. */
export function openLocalTerminal(shell?: ShellProfile) {
  app.menu = null;
  void sessions.openLocal({ shell });
}

/** Opens local terminals in the folders asked for on the command line: `--cwd <folder>`,
 * which Explorer's "Open with NexSSH" runs. */
export async function openRequestedFolders() {
  const folders = await api.launchTake().catch(() => [] as string[]);
  for (const cwd of folders) await sessions.openLocal({ cwd });
}

/** The shell `openLocalTerminal()` starts: the settings' choice if it is installed. */
function chosenShell(): ShellProfile | typeof CUSTOM_SHELL | undefined {
  const setting = app.settings.localShell;
  if (setting === CUSTOM_SHELL && app.settings.localShellCommand.trim()) return CUSTOM_SHELL;
  return (setting && shells.byId(setting)) || shells.default;
}

/** The menu next to the tabs' + button: a connection, or a local terminal with any shell. */
export async function newTabMenu(x: number, y: number) {
  await shells.load();
  const chosen = chosenShell();
  const items: MenuEntry[] = [
    { label: t('tabs.newConnection'), icon: 'zap', hint: keys.newSession(), action: quickConnect },
    'separator',
    ...shells.list.map((shell) => ({
      label: shellName(shell),
      icon: 'terminal',
      hint: shell === chosen ? keys.localTerminal() : undefined,
      action: () => openLocalTerminal(shell),
    })),
  ];
  if (chosen === CUSTOM_SHELL) {
    items.push({
      label: commandName(app.settings.localShellCommand),
      icon: 'terminal',
      hint: keys.localTerminal(),
      action: () => openLocalTerminal(),
    });
  }
  items.push('separator', {
    label: t('session.terminalSettings'),
    icon: 'settings',
    action: () => openSettings('terminal'),
  });
  app.showMenu(x, y, items);
}

export function addServer(preset?: Partial<Server>) {
  app.openEditor(null, preset);
}

export function editServer(server: Server) {
  app.openEditor(server);
}

export async function deleteServer(server: Server) {
  const open = sessions.forServer(server.id).length;
  const ok = await app.confirm(
    t('server.deleteTitle', { name: server.name }),
    t(open ? 'server.deleteMessageOpen' : 'server.deleteMessage'),
    t('common.delete'),
    true,
  );
  if (!ok) return;
  try {
    await servers.remove(server.id);
    toasts.show(t('server.deleted', { name: server.name }));
  } catch (e) {
    toasts.error(errorMessage(e));
  }
}

export async function duplicateServer(server: Server) {
  try {
    await servers.duplicate(server);
  } catch (e) {
    toasts.error(errorMessage(e));
  }
}

export function importSshConfig() {
  void servers.importSshConfig();
}

export function openSettings(section: SettingsSection = 'appearance') {
  app.menu = null;
  app.palette.open = false;
  app.settingsSection = section;
  app.settingsOpen = true;
}

export function showHome() {
  sessions.activeKey = null;
  app.sidebarForHome();
}

export function toggleSidebar() {
  app.setSidebarHidden(!app.sidebarHidden);
}

export function toggleFullscreen() {
  app.toggleFullscreen().catch((e) => toasts.error(errorMessage(e)));
}

export function openForwards() {
  if (sessions.active) app.forwardsOpen = true;
}

/** Shows or hides the files (SFTP) drawer next to the terminal; SSH sessions only. */
export function toggleFiles() {
  app.menu = null;
  if (sessions.active?.kind === 'local') return;
  files.toggle();
}

async function copyText(text: string) {
  await navigator.clipboard.writeText(text).catch(() => {});
  toasts.show(t('common.copied', { text }));
}

export function serverMenu(server: Server, x: number, y: number) {
  const open = sessions.forServer(server.id).length > 0;
  const items: MenuEntry[] = [
    { label: t(open ? 'server.goToSession' : 'server.connect'), icon: 'terminal', action: () => connect(server) },
    { label: t('server.newSession'), icon: 'plus', action: () => connect(server, true) },
    'separator',
    { label: t('server.edit'), icon: 'edit', action: () => editServer(server) },
    { label: t('server.duplicate'), icon: 'duplicate', action: () => duplicateServer(server) },
    { label: t('server.copyAddress'), icon: 'copy', action: () => copyText(destination(server)) },
    'separator',
    { label: t('common.delete'), icon: 'trash', danger: true, action: () => deleteServer(server) },
  ];
  app.showMenu(x, y, items);
}

export async function closeTab(tab: Tab) {
  // Edited files of the session are no longer sent once it closes.
  const unsent = edits.unsent(tab.sessionId);
  if (unsent.length) {
    const names = unsent.map((e) => e.name).join(', ');
    const ok = await app.confirm(t('edit.closeTitle'), t('edit.closeUnsent', { names }), t('session.closeTab'), true);
    if (!ok) return;
  }
  sessions.close(tab.key);
  requestAnimationFrame(() => sessions.focusActive());
}

/** The ⋯ menu of a local terminal and its tab's context menu. */
function localMenu(tab: Tab, x: number, y: number) {
  app.showMenu(x, y, [
    tab.status === 'disconnected'
      ? { label: t('session.restart'), icon: 'refresh', hint: keys.reconnect(), action: () => sessions.reconnect(tab) }
      : { label: t('session.stop'), icon: 'power', action: () => sessions.disconnect(tab) },
    { label: t('session.duplicate'), icon: 'duplicate', action: () => sessions.duplicate(tab) },
    'separator',
    { label: t('session.clear'), icon: 'eraser', action: () => sessions.clear(tab) },
    { label: t('session.fullscreen'), icon: 'fullscreen', hint: keys.fullscreen(), action: toggleFullscreen },
    { label: t('session.terminalSettings'), icon: 'settings', action: () => openSettings('terminal') },
    'separator',
    { label: t('session.closeTab'), icon: 'x', hint: keys.closeTab(), action: () => closeTab(tab) },
  ]);
}

/** The ⋯ menu of a session and the tab context menu. */
export function sessionMenu(tab: Tab, x: number, y: number) {
  if (tab.kind === 'local') return localMenu(tab, x, y);
  const server = tab.serverId ? servers.byId.get(tab.serverId) : undefined;
  const connected = tab.status === 'connected';
  const items: MenuEntry[] = [
    tab.status === 'disconnected'
      ? { label: t('session.reconnect'), icon: 'refresh', hint: keys.reconnect(), action: () => sessions.reconnect(tab) }
      : { label: t('session.disconnect'), icon: 'power', action: () => sessions.disconnect(tab) },
    { label: t('session.duplicate'), icon: 'duplicate', action: () => sessions.duplicate(tab) },
    { label: t('session.files'), icon: 'folder', hint: keys.files(), action: toggleFiles },
    { label: t('session.forwarding'), icon: 'forward', disabled: !connected, action: () => openForwards() },
    'separator',
    { label: t('session.clear'), icon: 'eraser', action: () => sessions.clear(tab) },
    { label: t('session.fullscreen'), icon: 'fullscreen', hint: keys.fullscreen(), action: toggleFullscreen },
  ];
  if (server) {
    items.push({ label: t('session.connectionSettings'), icon: 'edit', action: () => editServer(server) });
  } else {
    items.push({
      label: t('session.saveAsServer'),
      icon: 'server',
      action: () => addServer(presetFromDestination(tab.subtitle)),
    });
  }
  items.push('separator', {
    label: t('session.closeTab'),
    icon: 'x',
    hint: keys.closeTab(),
    action: () => closeTab(tab),
  });
  app.showMenu(x, y, items);
}

/** Pre-fills the server editor from `user@host:port`. */
export function presetFromDestination(input: string): Partial<Server> {
  let rest = input.trim().replace(/^ssh:\/\//, '');
  let user = '';
  const at = rest.lastIndexOf('@');
  if (at > 0) {
    user = rest.slice(0, at);
    rest = rest.slice(at + 1);
  }
  let host = rest;
  let port = 22;
  const v6 = rest.match(/^\[([^\]]+)\](?::(\d+))?$/);
  if (v6) {
    host = v6[1];
    port = v6[2] ? Number(v6[2]) : 22;
  } else if ((rest.match(/:/g) ?? []).length === 1) {
    const [h, p] = rest.split(':');
    host = h;
    port = Number(p) || 22;
  }
  return { user, host, port, name: host };
}

/** Looks like something ssh could connect to (`user@host`, `host:port`, a hostname or IP). */
export function looksLikeDestination(q: string): boolean {
  const s = q.trim();
  if (!s || /\s/.test(s)) return false;
  return (
    /@/.test(s) || // user@host
    /:\d+$/.test(s) || // host:port
    /^[\w-]+(\.[\w-]+)+$/.test(s) || // example.com, 10.0.0.1
    /^\[?[0-9a-f]*:[0-9a-f:]+\]?$/i.test(s) // IPv6 (needs a colon; "dead" is not an address)
  );
}
