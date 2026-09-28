// User actions shared by the sidebar, menus, the command palette and keyboard shortcuts.

import { errorMessage } from './api';
import { shortcut } from './platform';
import { app, type MenuEntry } from './state/app.svelte';
import { destination, servers } from './state/servers.svelte';
import { sessions, type Tab } from './state/sessions.svelte';
import { toasts } from './state/toasts.svelte';
import type { Server } from './types';

export const keys = {
  palette: () => shortcut('Mod', 'K'),
  newSession: () => (isMacLike() ? shortcut('Mod', 'T') : shortcut('Mod', 'Shift', 'T')),
  closeTab: () => (isMacLike() ? shortcut('Mod', 'W') : shortcut('Mod', 'Shift', 'W')),
  settings: () => shortcut('Mod', ','),
  sidebar: () => (isMacLike() ? shortcut('Mod', 'B') : shortcut('Mod', 'Shift', 'B')),
  reconnect: () => (isMacLike() ? shortcut('Mod', 'R') : shortcut('Mod', 'Shift', 'R')),
  fullscreen: () => (isMacLike() ? shortcut('Ctrl', 'Mod', 'F') : 'F11'),
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

export function addServer(preset?: Partial<Server>) {
  app.openEditor(null, preset);
}

export function editServer(server: Server) {
  app.openEditor(server);
}

export async function deleteServer(server: Server) {
  const open = sessions.forServer(server.id).length;
  const ok = await app.confirm(
    `Delete ${server.name}?`,
    `The server and its saved password will be removed.${open ? ' Open sessions stay connected.' : ''}`,
    'Delete',
    true,
  );
  if (!ok) return;
  try {
    await servers.remove(server.id);
    toasts.show(`Deleted ${server.name}`);
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

export function openSettings() {
  app.menu = null;
  app.palette.open = false;
  app.settingsOpen = true;
}

export function showHome() {
  sessions.activeKey = null;
}

export function toggleSidebar() {
  app.update({ sidebarHidden: !app.settings.sidebarHidden });
}

export function toggleFullscreen() {
  app.toggleFullscreen().catch((e) => toasts.error(errorMessage(e)));
}

export function openForwards() {
  if (sessions.active) app.forwardsOpen = true;
}

async function copyText(text: string) {
  await navigator.clipboard.writeText(text).catch(() => {});
  toasts.show(`Copied ${text}`);
}

export function serverMenu(server: Server, x: number, y: number) {
  const open = sessions.forServer(server.id).length > 0;
  const items: MenuEntry[] = [
    { label: open ? 'Go to session' : 'Connect', icon: 'terminal', action: () => connect(server) },
    { label: 'New session', icon: 'plus', action: () => connect(server, true) },
    'separator',
    { label: 'Edit…', icon: 'edit', action: () => editServer(server) },
    { label: 'Duplicate', icon: 'duplicate', action: () => duplicateServer(server) },
    { label: 'Copy address', icon: 'copy', action: () => copyText(destination(server)) },
    'separator',
    { label: 'Delete', icon: 'trash', danger: true, action: () => deleteServer(server) },
  ];
  app.showMenu(x, y, items);
}

export async function closeTab(tab: Tab) {
  sessions.close(tab.key);
  requestAnimationFrame(() => sessions.focusActive());
}

/** The ⋯ menu of a session and the tab context menu. */
export function sessionMenu(tab: Tab, x: number, y: number) {
  const server = tab.serverId ? servers.byId.get(tab.serverId) : undefined;
  const connected = tab.status === 'connected';
  const items: MenuEntry[] = [
    tab.status === 'disconnected'
      ? { label: 'Reconnect', icon: 'refresh', hint: keys.reconnect(), action: () => sessions.reconnect(tab) }
      : { label: 'Disconnect', icon: 'power', action: () => sessions.disconnect(tab) },
    { label: 'Duplicate session', icon: 'duplicate', action: () => sessions.duplicate(tab) },
    { label: 'Port forwarding…', icon: 'forward', disabled: !connected, action: () => openForwards() },
    'separator',
    { label: 'Clear terminal', icon: 'eraser', action: () => sessions.clear(tab) },
    { label: 'Full screen', icon: 'fullscreen', hint: keys.fullscreen(), action: toggleFullscreen },
  ];
  if (server) {
    items.push({ label: 'Connection settings…', icon: 'edit', action: () => editServer(server) });
  } else {
    items.push({
      label: 'Save as server…',
      icon: 'server',
      action: () => addServer(presetFromDestination(tab.subtitle)),
    });
  }
  items.push('separator', { label: 'Close tab', icon: 'x', hint: keys.closeTab(), action: () => closeTab(tab) });
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
