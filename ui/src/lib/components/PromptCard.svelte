<script lang="ts">
  import { t } from '../i18n.svelte';
  import { sessions, type Tab } from '../state/sessions.svelte';
  import type { Prompt, PromptReply } from '../types';
  import Icon from './Icon.svelte';
  import Rich from './Rich.svelte';

  interface Props {
    tab: Tab;
    entry: { id: number; prompt: Prompt };
  }
  let { tab, entry }: Props = $props();

  const p = $derived(entry.prompt);

  let secret = $state('');
  let remember = $state(false);
  let answers = $state<string[]>([]);
  let form = $state<HTMLFormElement>();
  let selected = false;

  // Fresh fields for every new prompt, and focus the first input. A user name starts as the
  // suggested one, to be remembered for its saved server.
  $effect.pre(() => {
    void entry.id;
    const prompt = entry.prompt;
    secret = prompt.kind === 'user' ? prompt.suggestion : '';
    remember = prompt.kind === 'user';
    selected = false;
    answers = prompt.kind === 'keyboardInteractive' ? prompt.prompts.map(() => '') : [];
  });

  $effect(() => {
    void entry.id;
    requestAnimationFrame(() => {
      const target = form?.querySelector<HTMLElement>('input, [data-default]');
      target?.focus();
    });
  });

  function reply(r: PromptReply) {
    sessions.answer(tab, entry.id, r);
    secret = '';
    answers = [];
    requestAnimationFrame(() => sessions.focusActive());
  }

  function cancel() {
    reply({ kind: 'cancel' });
  }

  /** Selects the suggested name when the field first gets focus, so typing replaces it. */
  function selectOnce(e: FocusEvent) {
    if (selected) return;
    selected = true;
    (e.currentTarget as HTMLInputElement).select();
  }

  function replyUser(name: string) {
    if (p.kind !== 'user') return;
    sessions.answerUser(tab, entry.id, p, name, remember);
    secret = '';
    requestAnimationFrame(() => sessions.focusActive());
  }

  function submit(e: SubmitEvent) {
    e.preventDefault();
    switch (p.kind) {
      case 'hostKey':
        if (p.check.status === 'changed') cancel();
        else reply({ kind: 'hostKey', accept: true, remember: true });
        break;
      case 'user':
        if (secret.trim()) replyUser(secret.trim());
        break;
      case 'password':
      case 'passphrase':
        if (secret) reply({ kind: 'secret', value: secret, remember });
        break;
      case 'keyboardInteractive':
        reply({ kind: 'answers', values: [...answers] });
        break;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      cancel();
    }
  }

  function target(host: string, port: number) {
    return port === 22 ? host : `${host}:${port}`;
  }

  const KEY_TYPES: Record<string, string> = {
    'ssh-ed25519': 'ED25519',
    'ssh-rsa': 'RSA',
    'rsa-sha2-256': 'RSA',
    'rsa-sha2-512': 'RSA',
    'ecdsa-sha2-nistp256': 'ECDSA P-256',
    'ecdsa-sha2-nistp384': 'ECDSA P-384',
    'ecdsa-sha2-nistp521': 'ECDSA P-521',
  };

  function keyType(t: string) {
    return KEY_TYPES[t] ?? t.replace(/^ssh-/, '').toUpperCase();
  }
</script>

<div class="scrim">
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <form class="card" class:warn={p.kind === 'hostKey' && p.check.status === 'changed'} bind:this={form} onsubmit={submit} {onkeydown}>
    {#if p.kind === 'hostKey'}
      {#if p.check.status === 'changed'}
        <div class="head danger"><Icon name="alert" size={18} /><h3>{t('prompt.changed.title')}</h3></div>
        <p>
          <Rich
            key="prompt.changed.body"
            params={{ host: target(p.host, p.port), file: p.check.file, line: p.check.line }}
            tags={{ host: 'b', file: 'code' }}
          />
        </p>
        <div class="fp"><span>{t('prompt.changed.recorded')}</span><code>{p.check.knownFingerprint}</code></div>
        <div class="fp">
          <span>{t('prompt.changed.presented', { type: keyType(p.keyType) })}</span><code>{p.fingerprint}</code>
        </div>
        <div class="buttons">
          <button type="button" class="btn danger" onclick={() => reply({ kind: 'hostKey', accept: true, remember: true })}>
            {t('prompt.changed.accept')}
          </button>
          <button type="submit" class="btn primary" data-default>{t('common.cancel')}</button>
        </div>
      {:else}
        <div class="head"><Icon name="shield" size={18} /><h3>{t('prompt.newHost.title')}</h3></div>
        <p>
          <Rich key="prompt.newHost.body" params={{ host: target(p.host, p.port) }} tags={{ host: 'b' }} />
        </p>
        <div class="fp"><span>{t('prompt.newHost.key', { type: keyType(p.keyType) })}</span><code>{p.fingerprint}</code></div>
        <div class="buttons">
          <button type="button" class="btn ghost" onclick={cancel}>{t('common.cancel')}</button>
          <button type="button" class="btn" onclick={() => reply({ kind: 'hostKey', accept: true, remember: false })}>
            {t('prompt.newHost.once')}
          </button>
          <button type="submit" class="btn primary" data-default>{t('prompt.newHost.trust')}</button>
        </div>
      {/if}
    {:else if p.kind === 'user'}
      <div class="head"><Icon name="user" size={18} /><h3>{t('prompt.user.title')}</h3></div>
      <p><Rich key="prompt.user.body" params={{ host: p.host }} tags={{ host: 'b' }} /></p>
      <input
        class="input"
        bind:value={secret}
        placeholder={t('prompt.user.placeholder')}
        autocomplete="username"
        spellcheck="false"
        onfocus={selectOnce}
      />
      {#if p.serverId}
        <label class="check remember">
          <input type="checkbox" bind:checked={remember} />
          {t('prompt.user.remember')}
        </label>
      {:else}
        <p class="hint">{t('prompt.user.hint', { host: p.host })}</p>
      {/if}
      <div class="buttons">
        <button type="button" class="btn ghost" onclick={cancel}>{t('common.cancel')}</button>
        <button type="submit" class="btn primary" disabled={!secret.trim()}>{t('common.continue')}</button>
      </div>
    {:else if p.kind === 'password' || p.kind === 'passphrase'}
      <div class="head">
        <Icon name={p.kind === 'password' ? 'lock' : 'key'} size={18} />
        <h3>{t(p.kind === 'password' ? 'prompt.password.title' : 'prompt.passphrase.title')}</h3>
      </div>
      <p class="sub">{p.kind === 'password' ? `${p.user}@${p.host}` : p.keyPath}</p>
      <input
        class="input"
        class:invalid={!!p.error}
        type="password"
        bind:value={secret}
        placeholder={t(p.kind === 'password' ? 'prompt.password.placeholder' : 'prompt.passphrase.placeholder')}
        autocomplete="off"
        spellcheck="false"
      />
      {#if p.error}<p class="error">{p.error}</p>{/if}
      {#if p.canRemember}
        <label class="check remember">
          <input type="checkbox" bind:checked={remember} />
          {t('prompt.remember')}
        </label>
      {/if}
      <div class="buttons">
        <button type="button" class="btn ghost" onclick={cancel}>{t('common.cancel')}</button>
        <button type="submit" class="btn primary" disabled={!secret}>
          {t(p.kind === 'password' ? 'prompt.connect' : 'prompt.unlock')}
        </button>
      </div>
    {:else if p.kind === 'keyboardInteractive'}
      <div class="head"><Icon name="shield" size={18} /><h3>{p.name || t('prompt.verification')}</h3></div>
      <p class="sub">{p.host}</p>
      {#if p.instructions}<p class="pre">{p.instructions}</p>{/if}
      {#each p.prompts as q, i (i)}
        <label class="field">
          <span>{q.text.trim()}</span>
          <input
            class="input"
            type={q.echo ? 'text' : 'password'}
            bind:value={answers[i]}
            autocomplete={q.echo ? 'off' : 'one-time-code'}
            spellcheck="false"
          />
        </label>
      {/each}
      <div class="buttons">
        <button type="button" class="btn ghost" onclick={cancel}>{t('common.cancel')}</button>
        <button type="submit" class="btn primary">{t('common.continue')}</button>
      </div>
    {/if}
  </form>
</div>

<style>
  .scrim {
    position: absolute;
    inset: 0;
    z-index: 5;
    display: grid;
    place-items: center;
    padding: 20px;
    background: color-mix(in srgb, var(--surface) 72%, transparent);
    backdrop-filter: blur(1.5px);
    animation: fade 0.12s var(--ease);
  }
  .card {
    width: 420px;
    max-width: 100%;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 18px 18px 16px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--bg);
    box-shadow: var(--shadow);
    animation: rise 0.16s var(--ease);
  }
  .card.warn {
    border-color: color-mix(in srgb, var(--red) 45%, var(--border));
  }
  .head {
    display: flex;
    align-items: center;
    gap: 9px;
    color: var(--text-2);
  }
  .head.danger {
    color: var(--red);
  }
  h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
  }
  .danger h3 {
    color: var(--red);
  }
  p {
    margin: 0;
    line-height: 1.5;
    color: var(--text-2);
  }
  p :global(b) {
    color: var(--text);
    font-weight: 600;
  }
  .sub {
    margin-top: -4px;
    font-family: var(--font-mono);
    font-size: 12px;
    word-break: break-all;
  }
  .pre {
    white-space: pre-wrap;
  }
  code,
  p :global(code) {
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  .fp {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 9px 11px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-soft);
    background: var(--surface);
  }
  .fp span {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.03em;
    color: var(--text-2);
    text-transform: uppercase;
  }
  .fp code {
    word-break: break-all;
    color: var(--text);
    -webkit-user-select: text;
    user-select: text;
  }
  .error {
    margin-top: -4px;
    color: var(--red);
    font-size: 12px;
  }
  .remember {
    color: var(--text-2);
  }
  .hint {
    font-size: 12px;
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 6px;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
