<script lang="ts">
  // chmod: nine checkboxes and the same mode as a number, kept in step. Bits above 0o777
  // (setuid, setgid, sticky) are kept unless a four-digit number changes them.
  import { t, type MessageKey } from '../i18n.svelte';
  import type { PermissionsState } from '../state/app.svelte';
  import Modal from './Modal.svelte';

  interface Props {
    request: PermissionsState;
  }
  let { request }: Props = $props();

  const uid = $props.id();

  const WHO: { label: MessageKey; shift: number }[] = [
    { label: 'files.owner', shift: 6 },
    { label: 'files.group', shift: 3 },
    { label: 'files.others', shift: 0 },
  ];
  const WHAT: { label: MessageKey; bit: number }[] = [
    { label: 'files.read', bit: 4 },
    { label: 'files.write', bit: 2 },
    { label: 'files.execute', bit: 1 },
  ];

  // svelte-ignore state_referenced_locally
  let mode = $state(request.mode & 0o7777);
  // svelte-ignore state_referenced_locally
  let text = $state(octal(request.mode & 0o7777));
  let recursive = $state(false);

  const valid = $derived(/^[0-7]{3,4}$/.test(text.trim()));

  function octal(value: number): string {
    return value.toString(8).padStart(value > 0o777 ? 4 : 3, '0');
  }

  function set(mask: number, on: boolean) {
    mode = on ? mode | mask : mode & ~mask;
    text = octal(mode);
  }

  function typed(value: string) {
    text = value;
    if (valid) mode = parseInt(value.trim(), 8);
  }

  /** `rwxr-xr-x`, with setuid, setgid and sticky shown the way `ls` shows them. */
  function symbolic(value: number): string {
    const part = (shift: number, special: number, mark: string) => {
      const x = value & (1 << shift);
      const executable = value & special ? (x ? mark : mark.toUpperCase()) : x ? 'x' : '-';
      return (value & (4 << shift) ? 'r' : '-') + (value & (2 << shift) ? 'w' : '-') + executable;
    };
    return part(6, 0o4000, 's') + part(3, 0o2000, 's') + part(0, 0o1000, 't');
  }

  function submit(e: SubmitEvent) {
    e.preventDefault();
    if (valid) request.resolve({ mode, recursive: request.folders && recursive });
  }
</script>

<Modal title={request.title} width={390} onclose={() => request.resolve(null)}>
  <form onsubmit={submit}>
    <div class="grid" role="group" aria-label={request.title}>
      <span></span>
      {#each WHAT as what (what.bit)}
        <span class="head">{t(what.label)}</span>
      {/each}
      {#each WHO as who (who.shift)}
        <span class="who">{t(who.label)}</span>
        {#each WHAT as what (what.bit)}
          {@const mask = what.bit << who.shift}
          <label class="cell">
            <input
              type="checkbox"
              checked={(mode & mask) !== 0}
              aria-label="{t(who.label)}: {t(what.label)}"
              onchange={(e) => set(mask, e.currentTarget.checked)}
            />
          </label>
        {/each}
      {/each}
    </div>

    <div class="numeric">
      <label for="{uid}-mode">{t('files.numeric')}</label>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        id="{uid}-mode"
        class="input mono"
        class:invalid={!valid}
        value={text}
        maxlength="4"
        inputmode="numeric"
        spellcheck="false"
        autocomplete="off"
        autofocus
        onfocus={(e) => e.currentTarget.select()}
        oninput={(e) => typed(e.currentTarget.value)}
      />
      <span class="mono symbolic">{symbolic(mode)}</span>
    </div>

    {#if request.folders}
      <label class="check">
        <input type="checkbox" bind:checked={recursive} />
        {t('files.recursive')}
      </label>
      {#if recursive}
        <p class="hint">{t('files.recursiveHint')}</p>
      {/if}
    {/if}

    <div class="actions">
      <button type="button" class="btn" onclick={() => request.resolve(null)}>{t('common.cancel')}</button>
      <button type="submit" class="btn primary" disabled={!valid}>{t('files.apply')}</button>
    </div>
  </form>
</Modal>

<style>
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1fr) repeat(3, 78px);
    align-items: center;
    row-gap: 2px;
    margin-bottom: 16px;
  }
  .head {
    color: var(--text-2);
    font-size: 12px;
    text-align: center;
  }
  .who {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .cell {
    display: grid;
    place-items: center;
    height: 30px;
    border-radius: var(--radius-sm);
  }
  .cell:hover {
    background: var(--hover);
  }
  .cell input {
    width: 15px;
    height: 15px;
    margin: 0;
    accent-color: var(--primary);
  }
  .numeric {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 14px;
  }
  .numeric label {
    color: var(--text-2);
  }
  .numeric .input {
    width: 68px;
    text-align: center;
  }
  .numeric .input.invalid {
    border-color: var(--red);
  }
  .symbolic {
    color: var(--text-2);
  }
  .hint {
    margin: 6px 0 0 22px;
    color: var(--text-2);
    font-size: 12px;
    line-height: 1.45;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 18px;
  }
</style>
