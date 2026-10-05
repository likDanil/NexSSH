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

  let selected = false;
  let checked = $state(false);

  /** Selects the text when the field first gets focus, so typing replaces it. */
  function selectText(e: FocusEvent) {
    if (selected) return;
    selected = true;
    const input = e.currentTarget as HTMLInputElement;
    const dot = request.fileName ? input.value.lastIndexOf('.') : -1;
    input.setSelectionRange(0, dot > 0 ? dot : input.value.length);
  }

  function submit(e?: Event) {
    e?.preventDefault();
    if (request.input !== undefined) {
      const v = request.password ? value : value.trim();
      if (!v) return;
      request.resolve(v);
    } else {
      request.resolve(checked ? 'checked' : '');
    }
  }
</script>

<Modal title={request.title} width={request.preview === undefined ? 400 : 520} onclose={() => request.resolve(null)}>
  <form onsubmit={submit}>
    {#if request.message}<p>{request.message}</p>{/if}
    {#if request.preview !== undefined}<pre class="preview">{request.preview}</pre>{/if}
    {#if request.checkbox}
      <label class="check"><input type="checkbox" bind:checked /> {request.checkbox}</label>
    {/if}
    {#if request.input !== undefined && request.password}
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input" type="password" bind:value autofocus autocomplete="off" />
    {:else if request.input !== undefined}
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input" bind:value autofocus spellcheck="false" onfocus={selectText} />
    {/if}
    <div class="actions">
      <button type="button" class="btn" onclick={() => request.resolve(null)}>{t('common.cancel')}</button>
      {#if request.alternative}
        <button type="button" class="btn" onclick={() => request.resolve('alternative')}>{request.alternative}</button>
      {/if}
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
  .preview {
    max-height: 190px;
    overflow: auto;
    margin: 0 0 14px;
    padding: 8px 10px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--input);
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.45;
    white-space: pre;
    user-select: text;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 14px;
    color: var(--text-2);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
