// In-app updates: check, download with progress, install. On Windows the installer runs
// silently and starts the new version by itself (see desktop/src/updates.rs).

import { api, Channel, errorMessage } from '../api';
import { t } from '../i18n.svelte';
import type { DownloadProgress, UpdateInfo } from '../types';
import { app } from './app.svelte';
import { restore } from './restore.svelte';
import { sessions } from './sessions.svelte';
import { toasts } from './toasts.svelte';

export type UpdateStatus = 'idle' | 'checking' | 'upToDate' | 'available' | 'downloading' | 'ready' | 'installing';

const FIRST_CHECK_MS = 5_000;
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;

class UpdatesState {
  status = $state<UpdateStatus>('idle');
  info = $state<UpdateInfo | null>(null);
  /** Download progress from 0 to 1; `null` while the size is unknown. */
  progress = $state<number | null>(0);

  #timer: ReturnType<typeof setTimeout> | undefined;

  get supported(): boolean {
    return !!app.info?.updates;
  }

  /** An update waits for the user (shown in the sidebar). */
  get pending(): boolean {
    return ['available', 'downloading', 'ready', 'installing'].includes(this.status) && !!this.info;
  }

  /** Called once the window is up: says "updated" after an update and starts the checks. */
  start() {
    const version = app.info?.version;
    if (version && app.settings.lastVersion !== version) {
      if (app.settings.lastVersion) toasts.show(t('update.updated', { version }), 'success', 6000);
      app.update({ lastVersion: version });
    }
    if (this.supported) this.#schedule(FIRST_CHECK_MS);
  }

  #schedule(ms: number) {
    clearTimeout(this.#timer);
    this.#timer = setTimeout(() => {
      if (app.settings.autoUpdateCheck) void this.check(false);
      this.#schedule(CHECK_EVERY_MS);
    }, ms);
  }

  /** `manual` checks report their result; background ones stay quiet (e.g. offline). */
  async check(manual: boolean) {
    const busy = this.status === 'checking' || this.status === 'downloading' || this.status === 'installing';
    if (!this.supported || busy || (this.status === 'ready' && !manual)) return;
    const before = this.status;
    this.status = 'checking';
    try {
      const info = await api.updateCheck();
      if (!info) {
        this.info = null;
        this.status = 'upToDate';
        if (manual) toasts.show(t('update.upToDate'));
        return;
      }
      const alreadyDownloaded = before === 'ready' && this.info?.version === info.version;
      this.info = info;
      this.status = alreadyDownloaded ? 'ready' : 'available';
    } catch (e) {
      this.status = before;
      if (manual) toasts.error(errorMessage(e));
      else console.warn('update check failed', e);
    }
  }

  async download() {
    if (this.status !== 'available') return;
    this.status = 'downloading';
    this.progress = 0;
    const channel = new Channel<DownloadProgress>();
    channel.onmessage = (p) => {
      this.progress = p.total ? Math.min(1, p.downloaded / p.total) : null;
    };
    try {
      await api.updateDownload(channel);
      this.status = 'ready';
    } catch (e) {
      this.status = 'available';
      toasts.error(errorMessage(e));
    }
  }

  /** Installs without any installer window; asks first only if sessions are connected. The tabs
   * open again after the restart, unless the settings never reopen them. */
  async install() {
    if (this.status !== 'ready') return;
    const reopen = app.settings.restoreTabs !== 'never';
    const connected = sessions.tabs.some((tab) => tab.status === 'connected');
    if (connected) {
      const message = t(reopen ? 'update.confirmReopen' : 'update.confirmMessage');
      const ok = await app.confirm(t('update.confirmTitle'), message, t('update.install'));
      if (!ok) return;
    }
    this.status = 'installing';
    try {
      await restore.beforeQuit(reopen);
      await api.updateInstall();
    } catch (e) {
      this.status = 'ready';
      toasts.error(errorMessage(e));
      // NexSSH runs on: the tabs are kept for a restart only when the settings always keep them.
      if (app.settings.restoreTabs === 'ask') void restore.beforeQuit(false).catch(() => {});
    }
  }
}

export const updates = new UpdatesState();
