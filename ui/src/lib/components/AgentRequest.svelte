<script lang="ts">
  import { onMount } from 'svelte';
  import { t, tn, type MessageKey } from '../i18n.svelte';
  import { agents } from '../state/agents.svelte';
  import type { AgentRequest } from '../types';
  import Icon from './Icon.svelte';
  import Modal from './Modal.svelte';

  interface Props {
    request: AgentRequest;
  }
  let { request }: Props = $props();

  let remember = $state(false);
  // Allow is enabled after a moment: a dialog that comes up while the user types in a terminal
  // must not take an Enter or a click meant for something else.
  let ready = $state(false);
  let denyButton: HTMLButtonElement | undefined = $state();

  const titles: Record<string, MessageKey> = {
    run_command: 'agents.ask.run_command',
    write_file: 'agents.ask.write_file',
    upload: 'agents.ask.upload',
  };
  const title = $derived(t(titles[request.tool] ?? 'agents.ask.other', { agent: request.agent }));
  const previewLabel = $derived(t(request.tool === 'run_command' ? 'agents.ask.input' : 'agents.ask.content'));

  onMount(() => {
    // The safe answer has the focus.
    requestAnimationFrame(() => denyButton?.focus());
    const timer = setTimeout(() => (ready = true), 600);
    return () => clearTimeout(timer);
  });

  function answer(allow: boolean) {
    agents.answer(request, allow, allow && remember);
  }
</script>

<Modal {title} subtitle={t('agents.ask.on', { server: request.serverName })} width={560} onclose={() => answer(false)}>
  <div class="body" data-keep-focus>
    <pre class="detail" class:command={request.tool === 'run_command'}>{request.detail}</pre>
    {#if request.preview}
      <span class="label">{previewLabel}</span>
      <pre class="preview">{request.preview}</pre>
    {/if}
    <label class="check">
      <input type="checkbox" bind:checked={remember} />
      {t('agents.ask.remember', { agent: request.agent, server: request.serverName })}
    </label>
  </div>

  {#snippet footer()}
    <span class="who"><Icon name="sparkle" size={14} /> {request.agent}</span>
    {#if agents.waiting > 0}<span class="more">{tn('agents.ask.more', agents.waiting)}</span>{/if}
    <span class="spacer"></span>
    <button class="btn" bind:this={denyButton} onclick={() => answer(false)}>{t('agents.ask.deny')}</button>
    <button class="btn primary" disabled={!ready} onclick={() => answer(true)}>
      {t(request.tool === 'run_command' ? 'agents.ask.run' : 'agents.ask.allow')}
    </button>
  {/snippet}
</Modal>

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  pre {
    margin: 0;
    padding: 9px 11px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--input);
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .detail {
    max-height: 220px;
    overflow: auto;
    color: var(--text);
  }
  .detail.command::before {
    content: '$ ';
    color: var(--text-3);
  }
  .preview {
    max-height: 200px;
    overflow: auto;
    color: var(--text-2);
  }
  .label {
    margin-bottom: -4px;
    color: var(--text-2);
    font-size: 12px;
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    color: var(--text-2);
    font-size: 12.5px;
    line-height: 1.4;
  }
  .check input {
    margin-top: 2px;
  }
  .who {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-2);
    font-weight: 500;
  }
  .more {
    color: var(--text-3);
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
</style>
