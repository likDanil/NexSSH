<script lang="ts">
  // Shown in the sidebar only while a new version waits: Download → progress → Install.
  import { t } from '../i18n.svelte';
  import { updates } from '../state/updates.svelte';
  import Icon from './Icon.svelte';

  const version = $derived(updates.info?.version ?? '');
  const percent = $derived(updates.progress == null ? null : Math.round(updates.progress * 100));
  const title = $derived.by(() => {
    switch (updates.status) {
      case 'downloading':
        return percent == null ? t('update.downloadingUnknown') : t('update.downloading', { percent });
      case 'ready':
        return t('update.cardReady');
      case 'installing':
        return t('update.installing');
      default:
        return t('update.cardNew');
    }
  });
</script>

{#if updates.pending}
  <div class="update" title={updates.info?.notes ?? ''}>
    <span class="icon"><Icon name="update" size={15} /></span>
    <span class="text">
      <span class="label">{title}</span>
      {#if updates.status === 'downloading'}
        <span class="bar" class:indeterminate={percent == null}>
          <span style:width="{percent ?? 30}%"></span>
        </span>
      {:else}
        <span class="sub">{t('update.version', { version })}</span>
      {/if}
    </span>
    {#if updates.status === 'available'}
      <button class="btn small" onclick={() => updates.download()}>{t('update.download')}</button>
    {:else if updates.status === 'ready'}
      <button class="btn small primary" onclick={() => updates.install()}>{t('update.install')}</button>
    {/if}
  </div>
{/if}

<style>
  .update {
    display: flex;
    align-items: center;
    gap: 9px;
    margin: 0 10px 8px;
    padding: 8px 8px 8px 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-soft);
    background: var(--surface);
    animation: appear 0.18s var(--ease);
  }
  .icon {
    display: grid;
    color: var(--green);
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .label {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: 12.5px;
    color: var(--text);
  }
  .sub {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: 11.5px;
    color: var(--text-2);
  }
  .bar {
    position: relative;
    height: 3px;
    border-radius: 2px;
    background: var(--border-soft);
    overflow: hidden;
  }
  .bar span {
    position: absolute;
    inset: 0 auto 0 0;
    border-radius: 2px;
    background: var(--green);
    transition: width 0.15s linear;
  }
  .bar.indeterminate span {
    animation: slide 1.2s var(--ease) infinite;
  }
  .btn.small {
    height: 26px;
    padding: 0 10px;
    font-size: 12px;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
  @keyframes appear {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
  }
</style>
