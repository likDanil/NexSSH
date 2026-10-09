<script lang="ts">
  // Closing NexSSH, when the settings leave it to the user (state/tray): into the tray or quit,
  // and whether the open tabs come back next time.
  import { t, tn } from '../i18n.svelte';
  import type { CloseQuestion } from '../state/app.svelte';
  import Modal from './Modal.svelte';

  interface Props {
    request: CloseQuestion;
  }
  let { request }: Props = $props();

  let reopen = $state(false);
  let remember = $state(false);

  function answer(action: 'tray' | 'quit') {
    request.resolve({ action, reopen, remember });
  }

  function submit(e: Event) {
    e.preventDefault();
    // The tray when it is offered: nothing is lost there.
    answer(request.tray ? 'tray' : 'quit');
  }
</script>

<Modal title={t('close.title')} width={440} onclose={() => request.resolve(null)}>
  <form onsubmit={submit}>
    {#if request.tray}
      <p>{t('close.trayHint')}</p>
    {:else if request.tabs}
      <p>{tn('close.tabsOpen', request.tabs)}</p>
    {/if}
    {#if request.tabs}
      <label class="check"><input type="checkbox" bind:checked={reopen} /> {tn('close.reopen', request.tabs)}</label>
    {/if}
    <label class="check"><input type="checkbox" bind:checked={remember} /> {t('close.remember')}</label>
    {#if remember}<p class="hint">{t('close.rememberHint')}</p>{/if}
    <div class="actions">
      <button type="button" class="btn" onclick={() => request.resolve(null)}>{t('common.cancel')}</button>
      {#if request.tray}
        <button type="button" class="btn" onclick={() => answer('quit')}>{t('close.quit')}</button>
      {/if}
      <!-- svelte-ignore a11y_autofocus -->
      <button type="submit" class="btn primary" autofocus>
        {t(request.tray ? 'close.toTray' : 'close.quit')}
      </button>
    </div>
  </form>
</Modal>

<style>
  p {
    margin: 0 0 14px;
    color: var(--text-2);
    line-height: 1.5;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 10px;
    color: var(--text-2);
  }
  .hint {
    margin: -4px 0 10px 24px;
    font-size: 12px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
</style>
