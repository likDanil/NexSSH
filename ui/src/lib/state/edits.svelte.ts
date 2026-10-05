// Remote files edited in a program of this computer (desktop/src/edit.rs, core/src/edit.rs):
// the list the backend sends on every change, opening files (with sudo when the server
// refuses), and the questions saves raise: a file changed on the server, a file only sudo
// may write.

import { api, errorMessage } from '../api';
import { t } from '../i18n.svelte';
import type { EditInfo, EditorInfo, EditRefusal } from '../types';
import { app } from './app.svelte';
import { files, parentPath } from './files.svelte';
import type { Tab } from './sessions.svelte';
import { toasts } from './toasts.svelte';

function refusal(e: unknown): EditRefusal | null {
  return e && typeof e === 'object' && 'kind' in e ? (e as EditRefusal) : null;
}

function failure(e: unknown): string {
  const r = refusal(e);
  return r && 'message' in r && r.message ? errorMessage(r.message) : errorMessage(e);
}

class EditsState {
  list = $state<EditInfo[]>([]);
  editors = $state<EditorInfo[]>([]);
  /** Questions on screen or answered, by edit and state: each is asked once. */
  #asked = new Set<string>();

  async start() {
    await api.onEdits((list) => this.#update(list));
    this.#update(await api.editList().catch(() => []));
    void this.loadEditors();
  }

  async loadEditors() {
    this.editors = await api.editEditors().catch(() => []);
  }

  /** The editor files open in, as the settings choose it. */
  editorName(): string {
    const { editor } = app.settings;
    if (editor === 'custom') return t('edit.customEditor');
    if (editor === 'system') return t('edit.defaultProgram');
    const found = this.editors.find((e) => e.id === editor) ?? this.editors[0];
    return found?.name ?? t('edit.defaultProgram');
  }

  forSession(sessionId: number | null): EditInfo[] {
    return sessionId == null ? [] : this.list.filter((e) => e.sessionId === sessionId);
  }

  /** Files of the session with saves that did not reach the server. */
  unsent(sessionId: number | null): EditInfo[] {
    return this.forSession(sessionId).filter((e) => e.state !== 'synced');
  }

  #update(list: EditInfo[]) {
    const before = new Map(this.list.map((e) => [e.id, e]));
    this.list = list;
    for (const edit of list) {
      const old = before.get(edit.id);
      if (edit.savedAt && old && edit.savedAt !== old.savedAt) {
        toasts.show(t('edit.saved', { name: edit.name }), 'success', 2200);
        // Its size and time of change in the list.
        files.refreshFolder(edit.sessionId, parentPath(edit.remotePath));
      }
      if (edit.state === 'conflict' || edit.state === 'denied') void this.#raise(edit);
      else this.#settled(edit.id);
    }
  }

  #settled(id: number) {
    for (const key of [...this.#asked]) if (key.startsWith(`${id}:`)) this.#asked.delete(key);
  }

  async #raise(edit: EditInfo) {
    const key = `${edit.id}:${edit.state}${edit.needsPassword ? ':password' : ''}`;
    if (this.#asked.has(key)) return;
    this.#asked.add(key);
    await this.resolve(edit);
  }

  /** Asks what to do about a file that changed on the server, or that sudo must write. */
  async resolve(edit: EditInfo) {
    if (edit.state === 'conflict') {
      const choice = await app.choose(
        t('edit.conflictTitle', { name: edit.name }),
        t('edit.conflictMessage', { path: edit.remotePath }),
        t('edit.overwrite'),
        t('edit.takeTheirs'),
        true,
      );
      if (choice) await api.editResolve(edit.id, choice === 'confirm' ? 'overwrite' : 'reload').catch(toastError);
    } else if (edit.state === 'denied' && edit.needsPassword) {
      await this.#sudoPassword(edit);
    } else if (edit.state === 'denied') {
      const ok = await app.confirm(
        t('edit.deniedTitle', { name: edit.name }),
        t('edit.deniedMessage', { path: edit.remotePath }),
        t('edit.useSudo'),
      );
      if (!ok) return;
      const after = await api.editSudo(edit.id).catch(toastError);
      if (after?.state === 'denied' && after.needsPassword) await this.#sudoPassword(after);
    }
  }

  async #sudoPassword(edit: EditInfo) {
    const message = edit.error ? errorMessage(edit.error) : t('edit.passwordMessage', { name: edit.name });
    const password = await app.askPassword(t('edit.passwordTitle'), message, t('edit.save'));
    if (password === null) return;
    const after = await api.editSudo(edit.id, password).catch(toastError);
    if (after?.state === 'denied' && after.needsPassword) await this.#sudoPassword(after);
  }

  /** Opens a remote file of the tab's session in the editor. */
  async open(tab: Tab, path: string) {
    if (tab.sessionId == null) return;
    try {
      await api.editOpen(tab.sessionId, path);
    } catch (e) {
      const r = refusal(e);
      if (r?.kind !== 'denied') {
        toasts.error(failure(e));
        return;
      }
      const name = path.split('/').pop() || path;
      const ok = await app.confirm(t('edit.readDeniedTitle', { name }), t('edit.readDeniedMessage'), t('edit.openWithSudo'));
      if (ok) await this.#openSudo(tab, path);
    }
  }

  async #openSudo(tab: Tab, path: string, password?: string) {
    if (tab.sessionId == null) return;
    try {
      await api.editOpen(tab.sessionId, path, true, password);
    } catch (e) {
      const r = refusal(e);
      if (r?.kind !== 'password') {
        toasts.error(failure(e));
        return;
      }
      const name = path.split('/').pop() || path;
      const message = r.wrong
        ? errorMessage(r.message ?? t('edit.wrongPassword'))
        : t('edit.passwordMessage', { name });
      const typed = await app.askPassword(t('edit.passwordTitle'), message, t('edit.open'));
      if (typed !== null) await this.#openSudo(tab, path, typed);
    }
  }

  /** Opens the file's copy in the editor again. */
  show(edit: EditInfo) {
    api.editOpen(edit.sessionId, edit.remotePath, edit.sudo).catch((e) => toasts.error(failure(e)));
  }

  reveal(edit: EditInfo) {
    api.editReveal(edit.id).catch(toastError);
  }

  /** Stops editing; a save that did not reach the server is lost, so the user is asked. */
  async stop(edit: EditInfo) {
    if (edit.state !== 'synced') {
      const ok = await app.confirm(
        t('edit.stopTitle', { name: edit.name }),
        t('edit.stopUnsent'),
        t('edit.stop'),
        true,
      );
      if (!ok) return;
    }
    api.editStop(edit.id).catch(toastError);
  }
}

function toastError(e: unknown): null {
  toasts.error(errorMessage(e));
  return null;
}

export const edits = new EditsState();
