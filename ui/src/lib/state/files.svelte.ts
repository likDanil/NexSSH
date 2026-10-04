// The files drawer (SFTP): the folder shown for each tab, and every transfer. Transfers
// keep running when the drawer is closed or another tab is shown.

import { listen } from '@tauri-apps/api/event';
import { api, Channel, errorMessage } from '../api';
import { i18n, t, tn } from '../i18n.svelte';
import type { FilesSort, PickedUpload, SftpEntry, TransferProgress } from '../types';
import { app } from './app.svelte';
import { sessions, type Tab } from './sessions.svelte';
import { toasts } from './toasts.svelte';

export interface FolderView {
  /** Current remote directory ('' until the home directory is known). */
  path: string;
  entries: SftpEntry[];
  loading: boolean;
  error: string | null;
  /** Names of the selected entries. */
  selection: string[];
  /** The entry the keyboard is on. */
  cursor: string | null;
  /** Where a Shift range starts. */
  anchor: string | null;
}

export type TransferStatus = 'running' | 'done' | 'failed' | 'cancelled';

export interface Transfer {
  id: number;
  direction: 'down' | 'up';
  name: string;
  done: number;
  total: number;
  status: TransferStatus;
  /** Bytes per second, smoothed; 0 until measured. */
  rate: number;
  error?: string;
  /** Where a finished download was saved. */
  localPath?: string;
}

/** A file, or a folder with everything in it, to upload; paths are relative to the target folder. */
export interface UploadItem {
  name: string;
  /** Folders to create, parents first. */
  dirs: string[];
  files: { file: File; path: string }[];
}

/** Upload piece size: big enough for throughput, small enough for smooth progress. */
const UPLOAD_CHUNK = 1024 * 1024;
/** Downloads, and uploads, started together (each one is pipelined already). */
const PARALLEL_DOWNLOADS = 3;
const PARALLEL_UPLOADS = 3;
/** Files of a folder dropped on the page sent at the same time (a small file is mostly
 * round trips). */
const PARALLEL_PAGE_FILES = 4;
/** Speed is measured over at least this many milliseconds. */
const RATE_INTERVAL = 500;
/** Deeper folders in a drop are refused (a link loop would never end). */
const MAX_DEPTH = 64;

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

/** `1:05`, `1:02:03` */
export function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const pad = (n: number) => String(n).padStart(2, '0');
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

const collators = new Map<string, Intl.Collator>();

/** Folders first, then by `key`; names compare like people read them ("file2" before "file10"). */
export function sortEntries(entries: SftpEntry[], key: FilesSort, desc: boolean): SftpEntry[] {
  let collator = collators.get(i18n.lang);
  if (!collator) collators.set(i18n.lang, (collator = new Intl.Collator(i18n.lang, { numeric: true })));
  const byName = (a: SftpEntry, b: SftpEntry) => collator.compare(a.name, b.name);
  const sign = desc ? -1 : 1;
  return [...entries].sort((a, b) => {
    const folders = Number(isDirLike(b)) - Number(isDirLike(a));
    if (folders) return folders;
    switch (key) {
      case 'size':
        // Folders have no size of their own: they stay in name order.
        return isDirLike(a) ? byName(a, b) : sign * (a.size - b.size) || byName(a, b);
      case 'modified':
        return sign * ((a.modified ?? 0) - (b.modified ?? 0)) || byName(a, b);
      default:
        return sign * byName(a, b);
    }
  });
}

/** Upload items for plain files (a drop without folder entries). */
export function itemsFromFiles(list: Iterable<File>): UploadItem[] {
  return Array.from(list, (file) => ({ name: file.name, dirs: [], files: [{ file, path: file.name }] }));
}

/** What a drop holds, taken while the drop event runs (it is read afterwards). */
export interface Dropped {
  entries: FileSystemEntry[];
  plain: File[];
  /** The dropped files and folders themselves. */
  files: File[];
}

/** Takes what a drop holds; call it while the drop event runs. */
export function takeDrop(data: DataTransfer): Dropped {
  const entries: FileSystemEntry[] = [];
  const plain: File[] = [];
  for (const item of Array.from(data.items)) {
    if (item.kind !== 'file') continue;
    const entry = item.webkitGetAsEntry?.();
    const file = entry ? null : item.getAsFile();
    if (entry) entries.push(entry);
    else if (file) plain.push(file);
  }
  return { entries, plain, files: Array.from(data.files) };
}

/** Upload items from a drop, read in the page. */
export function readDrop({ entries, plain }: Dropped): Promise<UploadItem[]> {
  return readEntries(entries).then((items) => [...items, ...itemsFromFiles(plain)]);
}

/** WebView2's way to hand files to the app (Windows). */
interface WebviewBridge {
  postMessageWithAdditionalObjects?: (message: unknown, objects: ArrayLike<unknown>) => void;
}

/** How long to wait for the app to tell the paths of dropped files. */
const DROP_ANSWER_TIMEOUT = 3000;
let nextDrop = 1;
const dropAnswers = new Map<number, (items: PickedUpload[]) => void>();
let dropListener: Promise<unknown> | null = null;

/** Windows: the dropped files as the app sees them (see `take_page_drops` in sftp.rs), so they
 * are uploaded from disk, which is much faster than sending them from the page; `null` where
 * that is not possible, or not for all of them. */
export async function pathsOfDrop({ files }: Dropped): Promise<PickedUpload[] | null> {
  const webview = (window as { chrome?: { webview?: WebviewBridge } }).chrome?.webview;
  if (!webview?.postMessageWithAdditionalObjects || !files.length) return null;
  const id = nextDrop++;
  const answer = new Promise<PickedUpload[]>((resolve) => dropAnswers.set(id, resolve));
  dropListener ??= listen<{ id: number; items: PickedUpload[] }>('files-picked', ({ payload }) =>
    dropAnswers.get(payload.id)?.(payload.items),
  );
  await dropListener;
  webview.postMessageWithAdditionalObjects(`nexssh-drop:${id}`, files);
  const timeout = new Promise<null>((resolve) => setTimeout(() => resolve(null), DROP_ANSWER_TIMEOUT));
  const items = await Promise.race([answer, timeout]);
  dropAnswers.delete(id);
  return items?.length === files.length ? items : null;
}

async function readEntries(entries: FileSystemEntry[]): Promise<UploadItem[]> {
  const items: UploadItem[] = [];
  for (const entry of entries) {
    const item: UploadItem = { name: entry.name, dirs: [], files: [] };
    await collect(entry, '', item, 0);
    items.push(item);
  }
  return items;
}

async function collect(entry: FileSystemEntry, parent: string, item: UploadItem, depth: number) {
  const path = parent ? `${parent}/${entry.name}` : entry.name;
  if (entry.isFile) {
    const file = await new Promise<File>((resolve, reject) => (entry as FileSystemFileEntry).file(resolve, reject));
    item.files.push({ file, path });
  } else if (entry.isDirectory) {
    if (depth >= MAX_DEPTH) throw new Error(t('files.tooDeep', { path }));
    item.dirs.push(path);
    const reader = (entry as FileSystemDirectoryEntry).createReader();
    // A folder comes in batches, until an empty one.
    for (;;) {
      const batch = await new Promise<FileSystemEntry[]>((resolve, reject) => reader.readEntries(resolve, reject));
      if (!batch.length) break;
      for (const child of batch) await collect(child, path, item, depth + 1);
    }
  }
}

/** Runs `task` for every item, at most `limit` at a time. */
async function eachLimited<T>(items: T[], limit: number, task: (item: T) => Promise<void>) {
  let next = 0;
  const worker = async () => {
    while (next < items.length) await task(items[next++]);
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
}

/** A path as one shell word: `'it'\''s'`. */
function shellQuote(path: string): string {
  return `'${path.replaceAll("'", `'\\''`)}'`;
}

/** One toast for the failures of a batch. */
function reportFailures(errors: string[]) {
  if (errors.length === 1) toasts.error(errors[0]);
  else if (errors.length) toasts.error(tn('files.failedMany', errors.length, { error: errors[0] }));
}

class FilesState {
  open = $state(false);
  views = $state<Record<string, FolderView>>({});
  transfers = $state<Transfer[]>([]);

  /** The latest listing asked for per tab: older answers are dropped. */
  #loads = new Map<string, number>();
  /** Last speed measurement per running transfer. */
  #samples = new Map<number, { time: number; done: number }>();
  /** Uploads of a transfer's files on their way from the page: cancelled with it. */
  #parts = new Map<number, Set<number>>();

  get running(): number {
    return this.transfers.filter((tr) => tr.status === 'running').length;
  }

  toggle() {
    this.open = !this.open;
  }

  /** Loads the tab's folder the first time the drawer shows it. */
  ensure(tab: Tab) {
    if (this.views[tab.key]) return;
    this.views[tab.key] = {
      path: '',
      entries: [],
      loading: true,
      error: null,
      selection: [],
      cursor: null,
      anchor: null,
    };
    void this.goHome(tab);
  }

  forget(tabKey: string) {
    delete this.views[tabKey];
    this.#loads.delete(tabKey);
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

  /** Shows `path`. `select` names the entries to select; without it, a reload of the same
   * folder keeps the selection. */
  async openDir(tab: Tab, path: string, select?: string[]) {
    const view = this.views[tab.key];
    if (!view || tab.sessionId == null) return;
    const load = (this.#loads.get(tab.key) ?? 0) + 1;
    this.#loads.set(tab.key, load);
    view.loading = true;
    view.error = null;
    try {
      const entries = await api.sftpList(tab.sessionId, path);
      if (this.#loads.get(tab.key) !== load) return;
      const names = new Set(entries.map((e) => e.name));
      const keep = select === undefined && path === view.path;
      const exists = (name: string | null) => name !== null && names.has(name);
      view.selection = (keep ? view.selection : (select ?? [])).filter((name) => names.has(name));
      view.cursor = keep && exists(view.cursor) ? view.cursor : (view.selection[0] ?? null);
      view.anchor = keep && exists(view.anchor) ? view.anchor : view.cursor;
      view.path = path;
      view.entries = entries;
    } catch (e) {
      if (this.#loads.get(tab.key) !== load) return;
      if (view.path) toasts.error(errorMessage(e));
      else view.error = errorMessage(e);
    } finally {
      if (this.#loads.get(tab.key) === load) view.loading = false;
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

  /** Reloads the folder; `select` picks entries afterwards (default: keep the selection). */
  refresh(tab: Tab, select?: string[]) {
    const view = this.views[tab.key];
    if (!view) return;
    if (view.path) void this.openDir(tab, view.path, select);
    else void this.goHome(tab);
  }

  /** Reloads the tab's folder if it still shows `dir`. */
  #refreshIn(tab: Tab, dir: string, select?: string[]) {
    if (this.views[tab.key]?.path === dir) this.refresh(tab, select);
  }

  up(tab: Tab) {
    const view = this.views[tab.key];
    if (!view?.path || view.path === '/') return;
    const from = view.path.replace(/\/+$/, '').split('/').pop();
    void this.openDir(tab, parentPath(view.path), from ? [from] : []);
  }

  async mkdir(tab: Tab) {
    const view = this.views[tab.key];
    if (!view?.path || tab.sessionId == null) return;
    const dir = view.path;
    const name = await app.prompt(t('files.newFolderTitle'), '', '', t('files.create'));
    if (!name) return;
    try {
      await api.sftpMkdir(tab.sessionId, joinPath(dir, name));
      this.#refreshIn(tab, dir, [name]);
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  /** Creates an empty file. */
  async newFile(tab: Tab) {
    const view = this.views[tab.key];
    if (!view?.path || tab.sessionId == null) return;
    const dir = view.path;
    const name = await app.prompt(t('files.newFileTitle'), '', '', t('files.create'));
    if (!name) return;
    try {
      await api.sftpNewFile(tab.sessionId, joinPath(dir, name));
      this.#refreshIn(tab, dir, [name]);
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  async rename(tab: Tab, entry: SftpEntry) {
    const view = this.views[tab.key];
    if (!view?.path || tab.sessionId == null) return;
    const dir = view.path;
    const name = await app.prompt(t('files.renameTitle'), '', entry.name, t('files.renameAction'), !isDirLike(entry));
    if (!name || name === entry.name) return;
    try {
      await api.sftpRename(tab.sessionId, joinPath(dir, entry.name), joinPath(dir, name));
      this.#refreshIn(tab, dir, [name]);
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  /** Deletes the entries (folders with everything inside) after one confirmation. */
  async remove(tab: Tab, entries: SftpEntry[]) {
    const view = this.views[tab.key];
    const sessionId = tab.sessionId;
    if (!view?.path || sessionId == null || !entries.length) return;
    const dir = view.path;
    const one = entries.length === 1 ? entries[0] : null;
    const ok = await app.confirm(
      one ? t('files.deleteTitle', { name: one.name }) : tn('files.deleteManyTitle', entries.length),
      one
        ? t(one.kind === 'dir' ? 'files.deleteFolderMessage' : 'files.deleteFileMessage')
        : t('files.deleteManyMessage'),
      t('common.delete'),
      true,
    );
    if (!ok) return;
    const errors: string[] = [];
    for (const entry of entries) {
      try {
        await api.sftpRemove(sessionId, joinPath(dir, entry.name));
      } catch (e) {
        errors.push(errorMessage(e));
      }
    }
    reportFailures(errors);
    this.#refreshIn(tab, dir, []);
  }

  /** Changes permissions: asks for the mode, then applies it to every entry. */
  async chmod(tab: Tab, entries: SftpEntry[]) {
    const view = this.views[tab.key];
    const sessionId = tab.sessionId;
    if (!view?.path || sessionId == null || !entries.length) return;
    const dir = view.path;
    const first = entries[0];
    const choice = await app.askPermissions(
      entries.length === 1 ? t('files.permissionsTitle', { name: first.name }) : tn('files.permissionsMany', entries.length),
      first.mode ?? (isDirLike(first) ? 0o755 : 0o644),
      entries.some((e) => e.kind === 'dir'),
    );
    if (!choice) return;
    const errors: string[] = [];
    for (const entry of entries) {
      try {
        await api.sftpChmod(sessionId, joinPath(dir, entry.name), choice.mode, choice.recursive && entry.kind === 'dir');
      } catch (e) {
        errors.push(errorMessage(e));
      }
    }
    if (errors.length) reportFailures(errors);
    else toasts.show(t('files.permissionsChanged'), 'success');
    this.#refreshIn(tab, dir);
  }

  /** Makes the tab's shell go to `path` by typing `cd` into it. */
  openInTerminal(tab: Tab, path: string) {
    // A control character (a line break above all) would run the rest as a command.
    if (/[\x00-\x1f\x7f]/.test(path)) {
      toasts.error(t('files.cdUnsafe'));
      return;
    }
    sessions.input(tab.key, `cd ${shellQuote(path)}\r`);
    sessions.focusActive();
  }

  #add(direction: Transfer['direction'], name: string, total: number): Transfer {
    const id = nextTransfer++;
    this.transfers.unshift({ id, direction, name, done: 0, total, status: 'running', rate: 0 });
    this.#samples.set(id, { time: performance.now(), done: 0 });
    // The proxied object, so later updates are reactive.
    return this.transfers[0];
  }

  /** Records progress; the speed is measured over half a second or more, then smoothed. */
  #progress(tr: Transfer, done: number, total = tr.total) {
    tr.done = done;
    tr.total = total;
    const sample = this.#samples.get(tr.id);
    const now = performance.now();
    if (!sample || now - sample.time < RATE_INTERVAL) return;
    const rate = ((done - sample.done) * 1000) / (now - sample.time);
    tr.rate = tr.rate ? tr.rate * 0.6 + rate * 0.4 : rate;
    this.#samples.set(tr.id, { time: now, done });
  }

  #finish(tr: Transfer) {
    tr.status = 'done';
    tr.done = tr.total;
    this.#samples.delete(tr.id);
  }

  #fail(tr: Transfer, error: string) {
    tr.status = 'failed';
    tr.error = error;
    this.#samples.delete(tr.id);
  }

  /** Downloads files and folders into `dest`, or into Downloads. */
  async download(tab: Tab, entries: SftpEntry[], dest: string | null = null) {
    const view = this.views[tab.key];
    const sessionId = tab.sessionId;
    if (!view?.path || sessionId == null || !entries.length) return;
    const dir = view.path;
    const saved: Transfer[] = [];
    const errors: string[] = [];
    await eachLimited(entries, PARALLEL_DOWNLOADS, async (entry) => {
      const tr = this.#add('down', entry.name, isDirLike(entry) ? 0 : entry.size);
      const channel = new Channel<TransferProgress>();
      channel.onmessage = (p) => {
        if (tr.status === 'running') this.#progress(tr, p.done, p.total);
      };
      try {
        const result = await api.sftpDownload(sessionId, joinPath(dir, entry.name), dest, tr.id, channel);
        tr.name = result.name;
        tr.localPath = result.path;
        this.#finish(tr);
        saved.push(tr);
      } catch (e) {
        if (tr.status !== 'running') return;
        this.#fail(tr, errorMessage(e));
        errors.push(tr.error ?? '');
      }
    });
    reportFailures(errors);
    if (!saved.length) return;
    const first = saved[0];
    toasts.show(
      saved.length === 1 ? t('files.downloaded', { name: first.name }) : tn('files.downloadedMany', saved.length),
      'success',
      6000,
      { label: t('files.reveal'), run: () => this.reveal(first) },
    );
  }

  /** Asks for a local folder, then downloads there. */
  async downloadTo(tab: Tab, entries: SftpEntry[]) {
    if (!entries.length) return;
    let dest: string | null;
    try {
      dest = await api.sftpPickDestination(t('files.downloadToTitle'));
    } catch (e) {
      toasts.error(errorMessage(e));
      return;
    }
    if (dest) await this.download(tab, entries, dest);
  }

  /** Uploads files and folders into `dir` (default: the folder shown), asking before
   * replacing anything. Folders merge into existing ones. */
  async upload(tab: Tab, items: UploadItem[], dir?: string) {
    const view = this.views[tab.key];
    const sessionId = tab.sessionId;
    if (!view?.path || sessionId == null || !items.length) return;
    const target = dir ?? view.path;
    const names = items.map((item) => ({ name: item.name, folder: item.dirs.length > 0 }));
    if (!(await this.#confirmReplace(tab, sessionId, target, names))) return;
    const uploaded: string[] = [];
    const errors: string[] = [];
    await eachLimited(items, PARALLEL_UPLOADS, async (item) => {
      const error = await this.#uploadItem(sessionId, target, item);
      if (error === null) uploaded.push(item.name);
      else if (error) errors.push(error);
    });
    reportFailures(errors);
    this.#uploaded(tab, target, uploaded);
  }

  /** Asks for local files, then uploads them into `dir` (default: the folder shown). */
  async uploadFiles(tab: Tab, dir?: string) {
    if (!this.views[tab.key]?.path || tab.sessionId == null) return;
    let picked: PickedUpload[];
    try {
      picked = await api.sftpPickFiles(t('files.uploadFilesTitle'));
    } catch (e) {
      toasts.error(errorMessage(e));
      return;
    }
    if (picked.length) await this.uploadPicked(tab, picked, dir);
  }

  /** Asks for a local folder, then uploads it into `dir` (default: the folder shown). */
  async uploadFolder(tab: Tab, dir?: string) {
    if (!this.views[tab.key]?.path || tab.sessionId == null) return;
    let picked: PickedUpload | null;
    try {
      picked = await api.sftpPickUpload(t('files.uploadFolderTitle'));
    } catch (e) {
      toasts.error(errorMessage(e));
      return;
    }
    if (picked) await this.uploadPicked(tab, [picked], dir);
  }

  /** Uploads local files and folders picked in a dialog or dropped on the window into `dir`
   * (default: the folder shown); the backend reads them from disk. */
  async uploadPicked(tab: Tab, items: PickedUpload[], dir?: string) {
    const view = this.views[tab.key];
    const sessionId = tab.sessionId;
    if (!view?.path || sessionId == null || !items.length) return;
    const target = dir ?? view.path;
    if (!(await this.#confirmReplace(tab, sessionId, target, items))) return;
    const uploaded: string[] = [];
    const errors: string[] = [];
    await eachLimited(items, PARALLEL_UPLOADS, async (item) => {
      const tr = this.#add('up', item.name, 0);
      const channel = new Channel<TransferProgress>();
      channel.onmessage = (p) => {
        if (tr.status === 'running') this.#progress(tr, p.done, p.total);
      };
      try {
        await api.sftpUploadPath(sessionId, item.path, target, tr.id, channel);
        this.#finish(tr);
        uploaded.push(item.name);
      } catch (e) {
        if (tr.status !== 'running') return;
        this.#fail(tr, errorMessage(e));
        errors.push(tr.error ?? '');
      }
    });
    reportFailures(errors);
    this.#uploaded(tab, target, uploaded);
  }

  /** Asks before replacing what `dir` already has; `true` to go ahead. */
  async #confirmReplace(
    tab: Tab,
    sessionId: number,
    dir: string,
    items: { name: string; folder: boolean }[],
  ): Promise<boolean> {
    const view = this.views[tab.key];
    let existing: SftpEntry[];
    try {
      existing = view?.path === dir ? view.entries : await api.sftpList(sessionId, dir);
    } catch (e) {
      toasts.error(errorMessage(e));
      return false;
    }
    const byName = new Map(existing.map((e) => [e.name, e]));
    const conflicts = items.filter((item) => byName.has(item.name));
    if (!conflicts.length) return true;
    const shown = conflicts.slice(0, 3).map((item) => item.name);
    const names = shown.join(', ') + (conflicts.length > shown.length ? ', …' : '');
    const merge = conflicts.some((item) => {
      const there = byName.get(item.name);
      return item.folder && there !== undefined && isDirLike(there);
    });
    return app.confirm(
      tn('files.replaceTitle', conflicts.length),
      tn('files.replaceMessage', conflicts.length, { dir, names }) + (merge ? ` ${t('files.mergeNote')}` : ''),
      t('files.replace'),
      true,
    );
  }

  /** Reports finished uploads and shows them: selected in the folder they went to, or in
   * a fresh listing of its parent. */
  #uploaded(tab: Tab, dir: string, names: string[]) {
    if (!names.length) return;
    toasts.show(
      names.length === 1 ? t('files.uploadedOne', { name: names[0] }) : tn('files.uploaded', names.length),
      'success',
    );
    if (this.views[tab.key]?.path === dir) this.refresh(tab, names);
    else this.#refreshIn(tab, parentPath(dir));
  }

  /** Uploads one file or folder from the page as one transfer, several files at a time:
   * `null` when done, else the error ('' when cancelled). */
  async #uploadItem(sessionId: number, dir: string, item: UploadItem): Promise<string | null> {
    const tr = this.#add('up', item.name, item.files.reduce((sum, f) => sum + f.file.size, 0));
    const parts = new Set<number>();
    this.#parts.set(tr.id, parts);
    /** Bytes sent per file. */
    const sent = new Map<string, number>();
    const report = () => this.#progress(tr, [...sent.values()].reduce((sum, n) => sum + n, 0));
    try {
      if (item.dirs.length) await api.sftpEnsureDirs(sessionId, item.dirs.map((sub) => joinPath(dir, sub)));
      await eachLimited(item.files, PARALLEL_PAGE_FILES, async ({ file, path }) => {
        if (tr.status !== 'running') return;
        const id = nextTransfer++;
        parts.add(id);
        await api.sftpUploadBegin(sessionId, joinPath(dir, path), id);
        // The next piece is read while this one is on its way.
        const piece = (offset: number) => file.slice(offset, offset + UPLOAD_CHUNK).arrayBuffer();
        let next = piece(0);
        for (let offset = 0; offset < file.size && tr.status === 'running'; offset += UPLOAD_CHUNK) {
          const chunk = await next;
          if (offset + UPLOAD_CHUNK < file.size) next = piece(offset + UPLOAD_CHUNK);
          await api.sftpUploadChunk(id, chunk);
          sent.set(path, offset + chunk.byteLength);
          report();
        }
        if (tr.status !== 'running') {
          // Stopped while a piece was on its way: that put the upload back, drop it again.
          void api.sftpCancel(id).catch(() => {});
          return;
        }
        await api.sftpUploadEnd(id);
        parts.delete(id);
      });
      if (tr.status !== 'running') return '';
      this.#finish(tr);
      return null;
    } catch (e) {
      if (tr.status !== 'running') return '';
      // The other files stop too.
      this.#fail(tr, errorMessage(e));
      for (const id of parts) void api.sftpCancel(id).catch(() => {});
      return tr.error ?? '';
    } finally {
      this.#parts.delete(tr.id);
    }
  }

  cancel(tr: Transfer) {
    if (tr.status !== 'running') return;
    tr.status = 'cancelled';
    this.#samples.delete(tr.id);
    void api.sftpCancel(tr.id).catch(() => {});
    for (const id of this.#parts.get(tr.id) ?? []) void api.sftpCancel(id).catch(() => {});
  }

  reveal(tr: Transfer) {
    if (tr.localPath) api.revealDownload(tr.localPath).catch((e) => toasts.error(errorMessage(e)));
  }

  clearFinished() {
    this.transfers = this.transfers.filter((tr) => tr.status === 'running');
  }
}

export const files = new FilesState();
