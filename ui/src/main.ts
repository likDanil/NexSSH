import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { api } from './lib/api';
import { app } from './lib/state/app.svelte';
import { servers } from './lib/state/servers.svelte';

// Sessions of a previous page instance (e.g. after a reload) cannot be reattached.
void api.appReady().catch(() => {});

await Promise.all([app.init(), servers.load()]);

mount(App, { target: document.getElementById('app')! });

// The default context menu of the webview ("Reload", "Inspect") is not useful here;
// text fields keep theirs.
document.addEventListener('contextmenu', (e) => {
  const t = e.target as HTMLElement;
  if (!t.closest('input, textarea')) e.preventDefault();
});
