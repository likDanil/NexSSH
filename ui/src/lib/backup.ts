// Settings → Backup: everything NexSSH keeps saved into one file, and restored from one
// (desktop/src/backup.rs). Restoring replaces the servers and settings the page runs on, so the
// page loads again afterwards and tells what was restored then.

import { api, errorMessage } from './api';
import { i18n, t, tn } from './i18n.svelte';
import { ALTERNATIVE, app } from './state/app.svelte';
import { sessions } from './state/sessions.svelte';
import { toasts } from './state/toasts.svelte';
import type { BackupRestored } from './types';

/** What was restored, kept over the reload that follows it. */
const RESTORED_KEY = 'nexssh.backupRestored';

/** `nexssh-backup-2026-10-23.json`. */
export function backupFileName(now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `nexssh-backup-${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}.json`;
}

/** Asks where to save a backup and saves it there; with `password` the saved passwords go in,
 * encrypted with it. `false` when cancelled or failed (the user was told why). */
export async function saveBackup(password: string | null): Promise<boolean> {
  try {
    const saved = await api.backupExport(t('backup.saveTitle'), backupFileName(), password);
    if (!saved) return false;
    const path = saved.path;
    let text = t('backup.saved', { path });
    if (password !== null) {
      text = saved.passwords > 0 ? tn('backup.savedPasswords', saved.passwords, { path }) : t('backup.noPasswords', { path });
    }
    toasts.show(text, 'success', 6000);
    return true;
  } catch (e) {
    toasts.error(t('backup.saveFailed', { error: errorMessage(e) }));
    return false;
  }
}

/** Asks for a backup file, then for a go-ahead and its password, and restores it. */
export async function restoreBackup(): Promise<void> {
  let opened;
  try {
    opened = await api.backupOpen(t('backup.openTitle'));
  } catch (e) {
    toasts.error(t('backup.restoreFailed', { error: errorMessage(e) }));
    return;
  }
  if (!opened) return;
  const { path, summary } = opened;
  const date = new Date(summary.created * 1000).toLocaleString(i18n.lang, { dateStyle: 'long', timeStyle: 'short' });
  const message = [
    t('backup.confirmFrom', { date, version: summary.appVersion || '—', servers: tn('backup.servers', summary.servers) }),
    t('backup.confirmReplace'),
  ];
  if (sessions.tabs.length) message.push(tn('backup.confirmTabs', sessions.tabs.length));
  if (!(await app.confirm(t('backup.confirmTitle'), message.join(' '), t('backup.confirmButton'), true))) return;

  if (!summary.passwords) {
    await restore(path, null);
    return;
  }
  // A wrong password changes nothing: it is asked for again until it fits, or the user restores
  // without passwords or gives up.
  for (;;) {
    const answer = await app.askPassword(
      t('backup.passwordTitle'),
      t('backup.passwordAsk'),
      t('backup.confirmButton'),
      t('backup.withoutPasswords'),
    );
    if (answer === null) return;
    const password = answer === ALTERNATIVE ? null : answer;
    if ((await restore(path, password)) || password === null) return;
  }
}

/** Restores the backup and loads the page again; `false` when it failed (the user was told why). */
async function restore(path: string, password: string | null): Promise<boolean> {
  // A change of the settings not written yet would overwrite the restored ones.
  app.cancelSave();
  let restored: BackupRestored;
  try {
    restored = await api.backupRestore(path, password);
  } catch (e) {
    toasts.error(t('backup.restoreFailed', { error: errorMessage(e) }));
    return false;
  }
  try {
    sessionStorage.setItem(RESTORED_KEY, JSON.stringify(restored));
  } catch {
    // Only the message after the reload is lost.
  }
  location.reload();
  return true;
}

/** After the reload that follows a restore: says what was restored. */
export function showRestored() {
  let restored: BackupRestored | null = null;
  try {
    const saved = sessionStorage.getItem(RESTORED_KEY);
    sessionStorage.removeItem(RESTORED_KEY);
    restored = saved ? (JSON.parse(saved) as BackupRestored) : null;
  } catch {
    return;
  }
  if (!restored) return;
  toasts.show(
    restored.passwords > 0 ? tn('backup.restoredPasswords', restored.passwords) : t('backup.restored'),
    'success',
    5000,
  );
  if (restored.passwordsFailed > 0) toasts.error(tn('backup.passwordsFailed', restored.passwordsFailed));
}
