<script lang="ts">
  // Minimize / maximize / close for the custom title bar on Windows.
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount } from 'svelte';
  import { toggleFullscreen } from '../actions';
  import { t } from '../i18n.svelte';
  import { app } from '../state/app.svelte';
  import Icon from './Icon.svelte';

  const win = getCurrentWindow();
  let maximized = $state(false);

  onMount(() => {
    const refresh = () => win.isMaximized().then((m) => (maximized = m)).catch(() => {});
    refresh();
    const unlisten = win.onResized(refresh);
    return () => void unlisten.then((f) => f());
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
    <button aria-label={t(maximized ? 'window.restore' : 'window.maximize')} onclick={() => win.toggleMaximize()}>
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
  button:hover {
    background: var(--hover);
    color: var(--text);
  }
  .close:hover {
    background: #c42b1c;
    color: #fff;
  }
</style>
