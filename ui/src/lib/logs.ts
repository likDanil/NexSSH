// The app's log (desktop/src/logger.rs): kept in the data folder, saved from Settings → About into
// one file to send along with a report of a problem. Errors of the page's own code go there too.

import { api, errorMessage } from './api';
import { t } from './i18n.svelte';
import { toasts } from './state/toasts.svelte';

/** Page errors written to the log per page load: an error that repeats every frame stops here. */
const MAX_PAGE_ERRORS = 50;

/** `nexssh-log-2026-10-25-1403.log`. */
export function logFileName(now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  return `nexssh-log-${date}-${pad(now.getHours())}${pad(now.getMinutes())}.log`;
}

/** Asks where to save the log and saves it there. */
export async function saveLogs(): Promise<void> {
  try {
    const saved = await api.logsExport(t('logs.saveTitle'), logFileName());
    if (saved) toasts.show(t('logs.saved', { path: saved.path }), 'success', 6000);
  } catch (e) {
    toasts.error(t('logs.saveFailed', { error: errorMessage(e) }));
  }
}

export async function revealLogs(): Promise<void> {
  try {
    await api.logsReveal();
  } catch (e) {
    toasts.error(errorMessage(e));
  }
}

/** Errors the page's code did not handle go into the log too. */
export function catchPageErrors() {
  let left = MAX_PAGE_ERRORS;
  const write = (message: string) => {
    if (left-- > 0) void api.logPage('error', message).catch(() => {});
  };
  const describe = (error: unknown) =>
    error instanceof Error ? (error.stack ?? `${error.name}: ${error.message}`) : String(error);
  window.addEventListener('error', (e) => {
    write(e.error ? describe(e.error) : `${e.message} (${e.filename}:${e.lineno}:${e.colno})`);
  });
  window.addEventListener('unhandledrejection', (e) => write(`unhandled rejection: ${describe(e.reason)}`));
}
