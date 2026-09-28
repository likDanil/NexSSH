<script lang="ts">
  import { t } from '../i18n.svelte';
  import { toasts } from '../state/toasts.svelte';
  import Icon from './Icon.svelte';
</script>

<div class="toasts" aria-live="polite">
  {#each toasts.items as toast (toast.id)}
    <div class="toast {toast.kind}">
      {#if toast.kind === 'success'}<span class="mark ok"><Icon name="check" size={14} /></span>{/if}
      {#if toast.kind === 'error'}<span class="mark err"><Icon name="alert" size={14} /></span>{/if}
      <span class="text">{toast.text}</span>
      {#if toast.action}
        <button
          class="action"
          onclick={() => {
            toast.action?.run();
            toasts.dismiss(toast.id);
          }}>{toast.action.label}</button
        >
      {/if}
      <button class="icon-btn small" aria-label={t('common.dismiss')} onclick={() => toasts.dismiss(toast.id)}>
        <Icon name="x" size={13} />
      </button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 16px;
    bottom: 16px;
    z-index: 70;
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-end;
    pointer-events: none;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: 9px;
    max-width: 420px;
    padding: 8px 8px 8px 12px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: var(--elevated);
    box-shadow: var(--shadow);
    animation: slide 0.18s var(--ease);
  }
  .text {
    flex: 1;
    line-height: 1.35;
    -webkit-user-select: text;
    user-select: text;
  }
  .action {
    flex: none;
    height: 24px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--radius-sm);
    background: var(--hover);
    color: var(--text);
    font-weight: 500;
    white-space: nowrap;
  }
  .action:hover {
    background: var(--active);
  }
  .mark {
    display: grid;
    place-items: center;
  }
  .ok {
    color: var(--green);
  }
  .err {
    color: var(--red);
  }
  @keyframes slide {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
