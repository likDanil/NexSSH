<script lang="ts">
  // The search bar over a terminal (Ctrl+Shift+F, Cmd+F on macOS): finds text in the output and
  // the scrollback, newest first, and highlights every match.
  import { onMount } from 'svelte';
  import { formatNumber, t, tn, type MessageKey } from '../i18n.svelte';
  import { app } from '../state/app.svelte';
  import { sessions, type Tab } from '../state/sessions.svelte';
  import { FIND_LIMIT, type FindOptions, type FindResults, type TermView } from '../terminal';
  import { FIND_COLORS } from '../themes';
  import Icon from './Icon.svelte';

  interface Props {
    tab: Tab;
    view: TermView;
  }
  let { tab, view }: Props = $props();

  let input: HTMLInputElement;
  let text = $state('');
  let options = $state<FindOptions>({ caseSensitive: false, wholeWord: false, regex: false });
  let results = $state<FindResults | null>(null);
  let invalid = $state(false);

  const TOGGLES: { option: keyof FindOptions; label: string; title: MessageKey; key: string }[] = [
    { option: 'caseSensitive', label: 'Aa', title: 'find.caseSensitive', key: 'c' },
    { option: 'wholeWord', label: 'ab', title: 'find.wholeWord', key: 'w' },
    { option: 'regex', label: '.*', title: 'find.regex', key: 'r' },
  ];

  const status = $derived.by(() => {
    if (!text) return '';
    if (invalid) return t('find.invalid');
    if (!results) return '';
    if (!results.count) return t('find.none');
    const count = results.count >= FIND_LIMIT ? `${formatNumber(FIND_LIMIT)}+` : formatNumber(results.count);
    if (!results.index) return tn('find.matches', results.count, { count });
    return t('find.position', { index: formatNumber(results.index), count });
  });

  /** Upwards (`back`) is towards older output; a new search starts at the newest. */
  function find(back = true, stay = false) {
    invalid = false;
    try {
      view.find(text, options, back, FIND_COLORS[app.theme], stay);
    } catch {
      // An unfinished regex, like `(`.
      invalid = true;
      results = null;
    }
  }

  function toggle(option: keyof FindOptions) {
    options[option] = !options[option];
    find(true, true);
    input.focus();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      sessions.endFind(tab);
    } else if (e.key === 'Enter' || e.key === 'F3') {
      e.preventDefault();
      find(!e.shiftKey);
    } else if (e.altKey && !e.ctrlKey && !e.metaKey) {
      // Alt+C, Alt+W, Alt+R as in VS Code, by the key's place (any layout).
      const toggled = TOGGLES.find((x) => e.code === `Key${x.key.toUpperCase()}`);
      if (toggled) {
        e.preventDefault();
        toggle(toggled.option);
      }
    }
  }

  onMount(() => {
    view.onFindResults((r) => (results = r));
    // What was selected on one row is what is looked for.
    text = view.selectedLine();
    if (text) find();
    return () => {
      view.onFindResults(null);
      view.endFind();
    };
  });

  // Ctrl+Shift+F again while the bar is shown: back into the field.
  $effect(() => {
    void tab.findFocus;
    input.focus();
    input.select();
  });
</script>

<div class="find-bar" data-keep-focus role="search">
  <Icon name="search" size={14} />
  <input
    bind:this={input}
    bind:value={text}
    class:invalid
    type="text"
    spellcheck="false"
    autocomplete="off"
    placeholder={t('find.placeholder')}
    aria-label={t('find.placeholder')}
    oninput={() => find()}
    {onkeydown}
  />
  <span class="status" class:none={invalid || results?.count === 0} aria-live="polite">{status}</span>
  {#each TOGGLES as x (x.option)}
    <button
      class="icon-btn small toggle"
      class:on={options[x.option]}
      title={`${t(x.title)} (Alt+${x.key.toUpperCase()})`}
      aria-label={t(x.title)}
      aria-pressed={options[x.option]}
      onclick={() => toggle(x.option)}
    >
      {x.label}
    </button>
  {/each}
  <span class="sep"></span>
  <button
    class="icon-btn small"
    title={`${t('find.previous')} (Enter)`}
    aria-label={t('find.previous')}
    disabled={!text || invalid}
    onclick={() => find(true)}
  >
    <Icon name="arrowUp" size={14} />
  </button>
  <button
    class="icon-btn small"
    title={`${t('find.next')} (Shift+Enter)`}
    aria-label={t('find.next')}
    disabled={!text || invalid}
    onclick={() => find(false)}
  >
    <Icon name="arrowDown" size={14} />
  </button>
  <button class="icon-btn small" title={`${t('find.close')} (Esc)`} aria-label={t('find.close')} onclick={() => sessions.endFind(tab)}>
    <Icon name="x" size={13} />
  </button>
</div>

<style>
  .find-bar {
    position: absolute;
    top: 8px;
    right: 24px;
    z-index: 6;
    display: flex;
    align-items: center;
    gap: 2px;
    width: min(440px, calc(100% - 48px));
    padding: 4px 4px 4px 10px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: var(--bg);
    box-shadow: var(--shadow);
    color: var(--text-2);
    animation: drop 0.14s var(--ease);
  }
  input {
    flex: 1;
    min-width: 60px;
    height: 24px;
    margin-left: 6px;
    padding: 0;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }
  input.invalid {
    color: var(--red);
  }
  .status {
    flex-shrink: 0;
    padding: 0 6px;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .status.none {
    color: var(--red);
  }
  .toggle {
    width: 26px;
    font-family: var(--font-mono);
    font-size: 11.5px;
    font-weight: 600;
  }
  .sep {
    width: 1px;
    height: 16px;
    margin: 0 3px;
    background: var(--border);
  }
  @keyframes drop {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
  }
</style>
