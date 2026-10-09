// Tabs of the last run: the tabs open when NexSSH quits come back at the next start, as the
// settings say (`restoreTabs`): ask when quitting, always, or never. Local terminals start again;
// SSH tabs wait for Enter, so no password is asked for out of the blue. With `always` the list
// follows the tabs as they change, so it also survives the computer shutting down with NexSSH in
// the tray.

import { t, tn } from '../i18n.svelte';
import type { SavedTab, SavedTabs, Settings } from '../types';
import { app } from './app.svelte';
import { sessions } from './sessions.svelte';
import { toasts } from './toasts.svelte';

/** The tabs open now, as kept for the next start; `null` when there are none. */
function current(): SavedTabs | null {
  if (!sessions.tabs.length) return null;
  return {
    tabs: sessions.tabs.map((tab) => ({
      kind: tab.kind,
      target: $state.snapshot(tab.target),
      title: tab.title,
      subtitle: tab.subtitle,
    })),
    active: sessions.tabs.findIndex((tab) => tab.key === sessions.activeKey),
  };
}

function validTab(tab: unknown): tab is SavedTab {
  if (!tab || typeof tab !== 'object') return false;
  const { kind, target, title, subtitle } = tab as Partial<SavedTab>;
  if (typeof title !== 'string' || typeof subtitle !== 'string' || !target || typeof target !== 'object') return false;
  if (kind === 'local') return !!target.local && typeof target.local === 'object';
  return kind === 'ssh' && (typeof target.serverId === 'string' || typeof target.destination === 'string');
}

/** The list from settings.json, which may have been edited by hand: the tabs that make sense. */
function read(saved: unknown): SavedTabs | null {
  if (!saved || typeof saved !== 'object') return null;
  const { tabs, active } = saved as Partial<SavedTabs>;
  const valid = Array.isArray(tabs) ? tabs.filter(validTab) : [];
  if (!valid.length) return null;
  return { tabs: valid, active: typeof active === 'number' && valid.length === tabs?.length ? active : 0 };
}

class RestoreState {
  /** The list last written, to write only changes. */
  #written = '';

  /** At start-up: opens the tabs of the last run again if the settings say so; with `always` the
   * list keeps in step with the tabs from then on. */
  start() {
    this.#written = JSON.stringify(app.settings.lastTabs ?? null);
    const saved = read(app.settings.lastTabs);
    if (saved && app.settings.restoreTabs !== 'never') this.#reopen(saved);
    if (app.settings.restoreTabs !== 'always') {
      // Kept when quitting, for this start only.
      if (app.settings.lastTabs) this.#write(null);
    }
    $effect.root(() => {
      $effect(() => {
        if (app.settings.restoreTabs === 'always') this.#write(current());
      });
    });
  }

  /** Open tabs to ask about when quitting: the settings leave it to the user. */
  get askable(): number {
    return app.settings.restoreTabs === 'ask' ? sessions.tabs.length : 0;
  }

  /** Changes the setting; the list starts from the tabs open now, or is forgotten. */
  setMode(mode: Settings['restoreTabs']) {
    app.update({ restoreTabs: mode });
    this.#write(mode === 'always' ? current() : null);
  }

  /** Before NexSSH quits (or restarts for an update): keeps the open tabs for the next start, or
   * forgets them, and writes the settings at once. */
  async beforeQuit(reopen: boolean) {
    this.#write(reopen ? current() : null);
    await app.flush();
  }

  #write(list: SavedTabs | null) {
    const json = JSON.stringify(list);
    if (json === this.#written) return;
    this.#written = json;
    app.update({ lastTabs: list });
  }

  #reopen(saved: SavedTabs) {
    const { opened, waiting } = sessions.reopen(saved);
    if (!opened) return;
    const connectAll = waiting ? { label: t('restore.connectAll'), run: () => sessions.connectWaiting() } : undefined;
    toasts.show(tn('restore.reopened', opened), 'info', 9000, connectAll);
  }
}

export const restore = new RestoreState();
