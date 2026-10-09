// Closing NexSSH, and its icon in the tray. The backend leaves every close to the page (the
// window's close button, Alt+F4, the taskbar, "Quit" in the tray), since only the page knows the
// settings and the tabs: the window hides into the tray with the sessions still connected, or
// NexSSH quits, keeping the tabs for the next start if it should, or the user is asked first.
// The page also gives the tray its menu, in the interface language (desktop/src/tray.rs).

import { connect, openLocalTerminal } from '../actions';
import { api } from '../api';
import { t, tn } from '../i18n.svelte';
import type { CloseRequest, TrayAction, TraySpec } from '../types';
import { agents } from './agents.svelte';
import { app } from './app.svelte';
import { restore } from './restore.svelte';
import { servers } from './servers.svelte';
import { sessions } from './sessions.svelte';

class TrayState {
  /** A question about closing is on screen. */
  #asking = false;
  /** The menu last sent, to send only changes. */
  #sent = '';

  /** This system shows NexSSH's icon in the tray. */
  get supported(): boolean {
    return !!app.info?.tray;
  }

  async start() {
    await api.onCloseRequest((request) => void this.#closeRequested(request));
    await api.onTrayAction((action) => this.#act(action));
    if (!this.supported) return;
    $effect.root(() => {
      $effect(() => this.#send(this.#spec()));
    });
  }

  /** The window's close button: as the settings say (without a tray NexSSH quits). */
  async close() {
    if (this.#asking) return;
    const action = this.supported ? app.settings.closeAction : 'quit';
    if (action === 'tray') return this.minimize();
    const tabs = restore.askable;
    if (action === 'quit' && !tabs) return this.#exit(app.settings.restoreTabs === 'always');
    await this.#ask(action === 'ask', tabs);
  }

  /** Quits NexSSH; asks first whether to open the tabs again next time, if the settings say so. */
  async quit() {
    if (this.#asking) {
      void api.windowShow().catch(() => {});
      return;
    }
    const tabs = restore.askable;
    if (!tabs) return this.#exit(app.settings.restoreTabs === 'always');
    // The question needs the window, which may be in the tray.
    void api.windowShow().catch(() => {});
    await this.#ask(false, tabs);
  }

  /** Hides the window into the tray; the sessions stay connected. */
  minimize() {
    app.menu = null;
    void api.windowHide().catch(() => {});
  }

  async #closeRequested({ id, kind }: CloseRequest) {
    // At once: the backend quits by itself when the page does not take the request.
    void api.closeTaken(id).catch(() => {});
    if (kind === 'quit') await this.quit();
    else await this.close();
  }

  /** `tray`: offer to hide the window into the tray; `tabs`: offer to open them next time. */
  async #ask(tray: boolean, tabs: number) {
    this.#asking = true;
    try {
      const answer = await app.askClose(tray, tabs);
      if (!answer) return;
      if (answer.remember) {
        if (tray) app.update({ closeAction: answer.action });
        if (tabs && answer.action === 'quit') restore.setMode(answer.reopen ? 'always' : 'never');
      }
      if (answer.action === 'tray') this.minimize();
      else await this.#exit(tabs ? answer.reopen : app.settings.restoreTabs === 'always');
    } finally {
      this.#asking = false;
    }
  }

  /** Quits, after keeping the open tabs for the next start, or forgetting them. */
  async #exit(reopen: boolean) {
    try {
      await restore.beforeQuit(reopen);
    } catch (e) {
      console.error('tabs for the next start', e);
    }
    await api.appExit();
  }

  #act(action: TrayAction) {
    if (action.action === 'local') {
      openLocalTerminal();
      return;
    }
    const server = servers.byId.get(action.serverId);
    if (server) connect(server);
  }

  /** The menu: what is open and what AI agents do, then the window, the servers, a local
   * terminal and quitting. */
  #spec(): TraySpec {
    const status = [sessions.tabs.length ? tn('tray.tabs', sessions.tabs.length) : t('tray.noTabs')];
    const agentsLine = this.#agentsLine();
    if (agentsLine) status.push(agentsLine);
    const groups = servers.groups;
    return {
      always: app.settings.closeAction === 'tray',
      tooltip: `NexSSH — ${status.join(' · ')}`,
      status,
      open: t('tray.open'),
      connect: t('tray.connect'),
      noServers: t('tray.noServers'),
      local: t('tray.localTerminal'),
      quit: t('tray.quit'),
      groups: groups.map((group) => ({
        // Servers without a group: in place, or under "Other" next to groups, as in the sidebar.
        name: group.name || (groups.length > 1 ? t('group.other') : ''),
        servers: group.servers.map((server) => ({ id: server.id, name: server.name })),
      })),
    };
  }

  /** AI agents at work (or waiting for an answer), else that they may connect. */
  #agentsLine(): string | null {
    const status = agents.status;
    if (!app.settings.agentsEnabled || !status?.running) return null;
    const busy = new Set(
      status.activity.filter((a) => a.status === 'running' || a.status === 'waiting').map((a) => a.agent),
    );
    return busy.size ? t('tray.agentsBusy', { names: [...busy].join(', ') }) : t('tray.agentsOn');
  }

  #send(spec: TraySpec) {
    const json = JSON.stringify(spec);
    if (json === this.#sent) return;
    this.#sent = json;
    void api.traySet(spec).catch((e) => console.error('tray', e));
  }
}

export const tray = new TrayState();
