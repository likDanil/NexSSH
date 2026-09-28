import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Tauri serves the built files from disk; the dev server must run on a fixed port.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/desktop/**', '**/core/**', '**/target/**'] },
  },
  build: {
    // WebView2 (Chromium), WKWebView (Safari 16+) and WebKitGTK are all modern.
    target: ['es2022', 'chrome110', 'safari16'],
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: false,
    chunkSizeWarningLimit: 700,
  },
});
