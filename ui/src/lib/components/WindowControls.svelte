<script lang="ts">
  // Minimize / maximize / close for the custom title bar on Windows.
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount } from 'svelte';
  import { toggleFullscreen } from '../actions';
  import { api } from '../api';
  import { t } from '../i18n.svelte';
  import { app } from '../state/app.svelte';
  import type { SnapButtonState } from '../types';
  import Icon from './Icon.svelte';

  const win = getCurrentWindow();
  let maximized = $state(false);
  let maxButton = $state<HTMLButtonElement | null>(null);
  /** Windows 11 shows its snap layouts on the maximize button through a window of the backend
   * that lies over it (desktop/src/snap.rs) and takes the mouse there: it says how the button
   * should look, and clicks it. */
  let snap = $state<SnapButtonState>('none');

  onMount(() => {
    const refresh = () => win.isMaximized().then((m) => (maximized = m)).catch(() => {});
    refresh();
    const unlisten = [win.onResized(refresh), api.onSnapButton((state) => (snap = state))];
    return () => unlisten.forEach((u) => void u.then((f) => f()));
  });

  // The backend hears where the button is whenever that may have changed: the window's size,
  // its scale, full screen (no maximize button then).
  $effect(() => {
    const button = maxButton;
    let frame = 0;
    const report = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const box = button?.getBoundingClientRect();
        const scale = devicePixelRatio;
        const rect = box?.width
          ? {
              x: Math.round(box.left * scale),
              y: Math.round(box.top * scale),
              width: Math.round(box.right * scale) - Math.round(box.left * scale),
              height: Math.round(box.bottom * scale) - Math.round(box.top * scale),
            }
          : null;
        api.windowSnapButton(rect).catch(() => {});
      });
    };
    report();
    if (!button) return;
    const observer = new ResizeObserver(report);
    observer.observe(button);
    window.addEventListener('resize', report);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener('resize', report);
      snap = 'none';
    };
  });
</script>

<div class="controls">
  <button aria-label={t('window.minimize')} onclick={() => win.minimize()}>
    <Icon name="winMin" size={16} stroke={1.2} />
  </button>
  {#if app.fullscreen}
    <button aria-label={t('window.exitFullscreen')} title={t('window.exitFullscreen')} onclick={toggleFullscreen}>
      <Icon name="winRestore" size={16} stroke={1.2} />
    </button>
  {:else}
    <button
      bind:this={maxButton}
      class:hover={snap === 'hover'}
      class:press={snap === 'press'}
      aria-label={t(maximized ? 'window.restore' : 'window.maximize')}
      onclick={() => win.toggleMaximize()}
    >
      <Icon name={maximized ? 'winRestore' : 'winMax'} size={16} stroke={1.2} />
    </button>
  {/if}
  <button class="close" aria-label={t('window.close')} onclick={() => win.close()}>
    <Icon name="winClose" size={16} stroke={1.2} />
  </button>
</div>

<style>
  .controls {
    display: flex;
    align-self: stretch;
    margin-left: 6px;
  }
  button {
    display: grid;
    place-items: center;
    width: 46px;
    height: 100%;
    border: 0;
    background: transparent;
    color: var(--text-2);
    transition: background 0.1s;
  }
  button:hover,
  button.hover {
    background: var(--hover);
    color: var(--text);
  }
  button.press {
    background: var(--active);
    color: var(--text);
  }
  .close:hover {
    background: #c42b1c;
    color: #fff;
  }
</style>
