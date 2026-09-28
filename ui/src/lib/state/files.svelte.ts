// The files drawer (SFTP): the folder shown for each tab, and every transfer. Transfers
// keep running when the drawer is closed or another tab is shown.

import { api, Channel, errorMessage } from '../api';
import { i18n, t, tn } from '../i18n.svelte';
import type { SftpEntry, TransferProgress } from '../types';
import { app } from './app.svelte';
import type { Tab } from './sessions.svelte';
import { toasts } from './toasts.svelte';

export interface FolderView {
  /** Current remote directory ('' until the home directory is known). */
  path: string;
  entries: SftpEntry[];
  loading: boolean;
  error: string | null;
  selected: string | null;
}

export type TransferStatus = 'running' | 'done' | 'failed' | 'cancelled';

export interface Transfer {
  id: number;
  direction: 'down' | 'up';
  name: string;
  done: number;
  total: number;
  status: TransferStatus;
  error?: string;
  /** Where a finished download was saved. */
  localPath?: string;
}

/** Upload piece size: big enough for throughput, small enough for smooth progress. */
const UPLOAD_CHUNK = 1024 * 1024;

let nextTransfer = 1;

export function joinPath(dir: string, name: string): string {
  return dir.endsWith('/') ? `${dir}${name}` : `${dir}/${name}`;
}

export function parentPath(path: string): string {
  const trimmed = path.replace(/\/+$/, '');
  const i = trimmed.lastIndexOf('/');
  return i <= 0 ? '/' : trimmed.slice(0, i);
}

export function isDirLike(e: SftpEntry): boolean {
  return e.kind === 'dir' || e.linkToDir;
}

const UNITS = ['size.b', 'size.kb', 'size.mb', 'size.gb', 'size.tb'] as const;

export function formatSize(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  const number = value.toLocaleString(i18n.lang, { maximumFractionDigits: digits });
  return `${number} ${t(UNITS[unit])}`;
}

export function formatDate(seconds?: number): string {
  if (!seconds) return '';
  const date = new Date(seconds * 1000);
  const sameYear = date.getFullYear() === new Date().getFullYear();
  return date.toLocaleString(
    i18n.lang,
    sameYear
      ? { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }
      : { day: 'numeric', month: 'short', year: 'numeric' },
  );
}

class FilesState {
  open = $state(false);
  views = $state<Record<string, FolderView>>({});
  transfers = $state<Transfer[]>([]);

  get running(): number {
    return this.transfers.filter((tr) => tr.status === 'running').length;
  }

  toggle() {
    this.open = !this.open;
  }

  /** Loads the tab's folder the first time the drawer shows it. */
  ensure(tab: Tab) {
    if (this.views[tab.key]) return;
    this.views[tab.key] = { path: '', entries: [], loading: true, error: null, selected: null };
    void this.goHome(tab);
  }

  forget(tabKey: string) {
    delete this.views[tabKey];
  }

  async goHome(tab: Tab) {
    const view = this.views[tab.key];
    if (!view || tab.sessionId == null) return;
    view.loading = true;
    view.error = null;
    try {
      await this.openDir(tab, await api.sftpHome(tab.sessionId));
    } catch (e) {
      view.error = errorMessage(e);
      view.loading = false;
    }
  }

  async openDir(tab: Tab, path: string, select: string | null = null) {
    const view = this.views[tab.key];
    if (!view || tab.sessionId == null) return;
    view.loading = true;
    view.error = null;
    try {
      const entries = await api.sftpList(tab.sessionId, path);
      view.path = path;
      view.entries = entries;
      view.selected = select;
    } catch (e) {
      if (view.path) toasts.error(errorMessage(e));
      else view.error = errorMessage(e);
    } finally {
      view.loading = false;
    }
  }

  /** Opens a typed path: `~` is the home folder, other relative paths start at the folder shown. */
  async goTo(tab: Tab, typed: string) {
    const view = this.views[tab.key];
    if (!view || tab.sessionId == null) return;
    const relative = !typed.startsWith('/') && !typed.startsWith('~');
    try {
      const path = await api.sftpResolve(tab.sessionId, relative && view.path ? joinPath(view.path, typed) : typed);
      await this.openDir(tab, path);
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  /** Reloads the folder; `select` picks an entry afterwards (default: keep the selection). */
  refresh(tab: Tab, select?: string | null) {
    const view = this.views[tab.key];
    if (!view) return;
    if (view.path) void this.openDir(tab, view.path, select === undefined ? view.selected : select);
    else void this.goHome(tab);
  }

  up(tab: Tab) {
    const view = this.views[tab.key];
    if (!view?.path || view.path === '/') return;
    const from = view.path.replace(/\/+$/, '').split('/').pop() ?? null;
    void this.openDir(tab, parentPath(view.path), from);
  }

  async mkdir(tab: Tab) {
    const view = this.views[tab.key];
    if (!view?.path || tab.sessionId == null) return;
    const name = await app.prompt(t('files.newFolderTitle'), '', '', t('files.create'));
    if (!name) return;
    try {
      await api.sftpMkdir(tab.sessionId, joinPath(view.path, name));
      this.refresh(tab, name);
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  async rename(tab: Tab, entry: SftpEntry) {
    const view = this.views[tab.key];
    if (!view?.path || tab.sessionId == null) return;
    const name = await app.prompt(t('files.renameTitle'), '', entry.name, t('files.renameAction'), !isDirLike(entry));
    if (!name || name === entry.name) return;
    try {
      await api.sftpRename(tab.sessionId, joinPath(view.path, entry.name), joinPath(view.path, name));
      this.refresh(tab, name);
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  async remove(tab: Tab, entry: SftpEntry) {
    const view = this.views[tab.key];
    if (!view?.path || tab.sessionId == null) return;
    const ok = await app.confirm(
      t('files.deleteTitle', { name: entry.name }),
      t(entry.kind === 'dir' ? 'files.deleteFolderMessage' : 'files.deleteFileMessage'),
      t('common.delete'),
      true,
    );
    if (!ok) return;
    try {
      await api.sftpRemove(tab.sessionId, joinPath(view.path, entry.name));
      this.refresh(tab, null);
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  #add(direction: Transfer['direction'], name: string, total: number): Transfer {
    this.transfers.unshift({ id: nextTransfer++, direction, name, done: 0, total, status: 'running' });
    // The proxied object, so later updates are reactive.
    return this.transfers[0];
  }

  /** Downloads a file or folder into the Downloads folder. */
  async download(tab: Tab, entry: SftpEntry) {
    const view = this.views[tab.key];
    if (!view?.path || tab.sessionId == null) return;
    const tr = this.#add('down', entry.name, isDirLike(entry) ? 0 : entry.size);
    const channel = new Channel<TransferProgress>();
    channel.onmessage = (p) => {
      tr.done = p.done;
      tr.total = p.total;
    };
    try {
      const saved = await api.sftpDownload(tab.sessionId, joinPath(view.path, entry.name), tr.id, channel);
      tr.status = 'done';
      tr.done = tr.total;
      tr.name = saved.name;
      tr.localPath = saved.path;
      toasts.show(t('files.downloaded', { name: saved.name }), 'success', 6000, {
        label: t('files.reveal'),
        run: () => this.reveal(tr),
      });
    } catch (e) {
      if (tr.status !== 'running') return;
      tr.status = 'failed';
      tr.error = errorMessage(e);
      toasts.error(tr.error);
    }
  }

  /** Uploads files into the folder shown for `tab`, asking before replacing any. */
  async upload(tab: Tab, files: File[]) {
    const view = this.views[tab.key];
    if (!view?.path || tab.sessionId == null || !files.length) return;
    const dir = view.path;
    const sessionId = tab.sessionId;
    const existing = new Set(view.entries.map((e) => e.name));
    const conflicts = files.filter((f) => existing.has(f.name)).length;
    if (conflicts) {
      const ok = await app.confirm(
        tn('files.replaceTitle', conflicts),
        tn('files.replaceMessage', conflicts, { dir }),
        t('files.replace'),
        true,
      );
      if (!ok) return;
    }
    let uploaded = 0;
    for (const file of files) {
      const tr = this.#add('up', file.name, file.size);
      try {
        await api.sftpUploadBegin(sessionId, joinPath(dir, file.name), tr.id);
        for (let offset = 0; offset < file.size && tr.status === 'running'; offset += UPLOAD_CHUNK) {
          const chunk = await file.slice(offset, offset + UPLOAD_CHUNK).arrayBuffer();
          await api.sftpUploadChunk(tr.id, chunk);
          tr.done = offset + chunk.byteLength;
        }
        if (tr.status !== 'running') continue;
        await api.sftpUploadEnd(tr.id);
        tr.status = 'done';
        tr.done = tr.total;
        uploaded++;
      } catch (e) {
        if (tr.status !== 'running') continue;
        tr.status = 'failed';
        tr.error = errorMessage(e);
        toasts.error(`${file.name}: ${tr.error}`);
      }
    }
    if (uploaded) {
      toasts.show(tn('files.uploaded', uploaded), 'success');
      if (this.views[tab.key]?.path === dir) this.refresh(tab);
    }
  }

  cancel(tr: Transfer) {
    if (tr.status !== 'running') return;
    tr.status = 'cancelled';
    void api.sftpCancel(tr.id).catch(() => {});
  }

  reveal(tr: Transfer) {
    if (tr.localPath) api.revealDownload(tr.localPath).catch((e) => toasts.error(errorMessage(e)));
  }

  clearFinished() {
    this.transfers = this.transfers.filter((tr) => tr.status === 'running');
  }
}

export const files = new FilesState();
