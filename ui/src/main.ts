import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { api } from './lib/api';
import { app } from './lib/state/app.svelte';
import { servers } from './lib/state/servers.svelte';
import { updates } from './lib/state/updates.svelte';

// Sessions of a previous page instance (e.g. after a reload) cannot be reattached.
void api.appReady().catch(() => {});

await Promise.all([app.init(), servers.load()]);

mount(App, { target: document.getElementById('app')! });
updates.start();

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
