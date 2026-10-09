import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { openRequestedFolders } from './lib/actions';
import { api } from './lib/api';
import { agents } from './lib/state/agents.svelte';
import { edits } from './lib/state/edits.svelte';
import { app } from './lib/state/app.svelte';
import { restore } from './lib/state/restore.svelte';
import { servers } from './lib/state/servers.svelte';
import { shells } from './lib/state/shells.svelte';
import { tray } from './lib/state/tray.svelte';
import { updates } from './lib/state/updates.svelte';

// Sessions of a previous page instance (e.g. after a reload) cannot be reattached.
void api.appReady().catch(() => {});

await Promise.all([app.init(), servers.load()]);

mount(App, { target: document.getElementById('app')! });
// The tabs of the last run, if the settings say so; before the folders asked for below.
restore.start();
void tray.start();
updates.start();
void shells.load();
void agents.start();
void edits.start();

// "Open with NexSSH" in Explorer (`NexSSH --cwd <folder>`): the folders of this start, and of
// later ones, which hand them over to this window.
void api.onLaunch(() => void openRequestedFolders());
void openRequestedFolders();

// The default context menu of the webview ("Reload", "Inspect") is not useful here;
// text fields keep theirs.
document.addEventListener('contextmenu', (e) => {
  const t = e.target as HTMLElement;
  if (!t.closest('input, textarea')) e.preventDefault();
});

// A file or link dropped where nothing takes it would open in the webview in place of the app.
for (const type of ['dragover', 'drop'] as const) {
  window.addEventListener(type, (e) => {
    const types = e.dataTransfer?.types ?? [];
    if (e.defaultPrevented || !(types.includes('Files') || types.includes('text/uri-list'))) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'none';
  });
}
