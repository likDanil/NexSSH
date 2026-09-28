<script lang="ts">
  import { t } from '../i18n.svelte';
  import type { ConfirmState } from '../state/app.svelte';
  import Modal from './Modal.svelte';

  interface Props {
    request: ConfirmState;
  }
  let { request }: Props = $props();

  // svelte-ignore state_referenced_locally
  let value = $state(request.input ?? '');

  function submit(e?: Event) {
    e?.preventDefault();
    if (request.input !== undefined) {
      const v = value.trim();
      if (!v) return;
      request.resolve(v);
    } else {
      request.resolve('');
    }
  }
</script>

<Modal title={request.title} width={400} onclose={() => request.resolve(null)}>
  <form onsubmit={submit}>
    {#if request.message}<p>{request.message}</p>{/if}
    {#if request.input !== undefined}
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input" bind:value autofocus spellcheck="false" />
    {/if}
    <div class="actions">
      <button type="button" class="btn" onclick={() => request.resolve(null)}>{t('common.cancel')}</button>
      <!-- svelte-ignore a11y_autofocus -->
      <button
        type="submit"
        class="btn"
        class:primary={!request.danger}
        class:danger={request.danger}
        autofocus={request.input === undefined}
      >
        {request.confirmLabel}
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
  .input {
    margin-bottom: 14px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
