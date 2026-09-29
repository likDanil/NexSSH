<script lang="ts">
  import logo from '../../assets/logo.png';
  import { keys } from '../actions';
  import { api, errorMessage } from '../api';
  import { formatNumber, LANGUAGES, t, tn, type LanguageSetting, type MessageKey } from '../i18n.svelte';
  import { isMac, shortcut } from '../platform';
  import { app, type SettingsSection } from '../state/app.svelte';
  import { CUSTOM_SHELL, missingShellName, shellName, shells } from '../state/shells.svelte';
  import { toasts } from '../state/toasts.svelte';
  import { updates } from '../state/updates.svelte';
  import { THEMES, themeLabel } from '../themes';
  import type { Settings } from '../types';
  import Modal from './Modal.svelte';
  import Select from './Select.svelte';

  const LINE_HEIGHTS = [1, 1.1, 1.2, 1.25, 1.3, 1.4, 1.5];
  const SCROLLBACKS = [1000, 5000, 10000, 50000, 100000];

  let section = $state<SettingsSection>(app.settingsSection);

  const s = $derived(app.settings);
  const windows = $derived(app.info?.os === 'windows');

  // The shells are looked for again: one may have been installed since.
  void shells.load(true);

  const shellOptions: { value: string; label: string }[] = $derived.by(() => {
    const options = [
      { value: '', label: t('settings.shellDefault', { name: shells.default ? shellName(shells.default) : '…' }) },
      ...shells.list.map((shell) => ({ value: shell.id, label: shellName(shell) })),
    ];
    const saved = s.localShell;
    if (saved && saved !== CUSTOM_SHELL && shells.loaded && !shells.byId(saved)) {
      options.push({ value: saved, label: t('settings.shellMissing', { name: missingShellName(saved) }) });
    }
    options.push({ value: CUSTOM_SHELL, label: t('settings.shellCustom') });
    return options;
  });

  const commandExample = $derived(
    windows ? String.raw`"C:\Program Files\Git\bin\bash.exe" --login -i` : '/usr/bin/fish --login',
  );

  async function browseProgram() {
    try {
      const path = await api.pickProgram(t('settings.shellPick'));
      if (path) app.update({ localShellCommand: /\s/.test(path) ? `"${path}"` : path });
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  function setExplorerMenu(enabled: boolean) {
    app.update({ explorerMenu: enabled });
    app.syncExplorerMenu();
  }

  let trusting = $state(false);

  async function trustExplorerMenu() {
    trusting = true;
    try {
      const state = await app.trustExplorerMenu();
      if (state.modern) toasts.show(t('settings.explorerMenuMoved'));
    } catch (e) {
      toasts.error(errorMessage(e));
    } finally {
      trusting = false;
    }
  }

  function fontSize(delta: number) {
    app.update({ fontSize: Math.min(28, Math.max(9, s.fontSize + delta)) });
  }

  async function copyPath() {
    if (!app.info) return;
    await navigator.clipboard.writeText(app.info.dataDir).catch(() => {});
    toasts.show(t('settings.pathCopied'));
  }

  const SECTIONS: { id: SettingsSection; label: MessageKey }[] = [
    { id: 'appearance', label: 'settings.appearance' },
    { id: 'terminal', label: 'settings.terminal' },
    { id: 'keyboard', label: 'settings.keyboard' },
    { id: 'about', label: 'settings.about' },
  ];

  const languages: { id: LanguageSetting; name: string }[] = $derived([
    { id: 'system', name: t('settings.languageSystem') },
    ...LANGUAGES,
  ]);

  const CURSORS: { id: Settings['cursorStyle']; label: MessageKey }[] = [
    { id: 'bar', label: 'settings.cursorBar' },
    { id: 'block', label: 'settings.cursorBlock' },
    { id: 'underline', label: 'settings.cursorUnderline' },
  ];

  const updateStatus = $derived.by(() => {
    const version = updates.info?.version ?? '';
    switch (updates.status) {
      case 'checking':
        return t('update.checking');
      case 'upToDate':
        return t('update.upToDate');
      case 'available':
        return t('update.available', { version });
      case 'downloading':
        return updates.progress == null
          ? t('update.downloadingUnknown')
          : t('update.downloading', { percent: Math.round(updates.progress * 100) });
      case 'ready':
        return t('update.ready', { version });
      case 'installing':
        return t('update.installing');
      default:
        return t(app.settings.autoUpdateCheck ? 'update.idle' : 'update.idleManual');
    }
  });

  const shortcuts: [MessageKey, string][] = $derived([
    ['shortcut.palette', isMac() ? keys.palette() : `${keys.palette()} · ${shortcut('Ctrl', 'Shift', 'K')}`],
    ['shortcut.newSession', keys.newSession()],
    ['shortcut.localTerminal', keys.localTerminal()],
    ['shortcut.closeTab', keys.closeTab()],
    [
      'shortcut.nextPrevTab',
      isMac() ? `${shortcut('Mod', 'Shift', ']')} · ${shortcut('Mod', 'Shift', '[')}` : 'Ctrl+Tab · Ctrl+Shift+Tab',
    ],
    ['shortcut.goToTab', shortcut('Mod', '1…9')],
    ['shortcut.reconnect', keys.reconnect()],
    ['shortcut.files', keys.files()],
    ['shortcut.sidebar', keys.sidebar()],
    ['shortcut.fullscreen', keys.fullscreen()],
    ['shortcut.settings', keys.settings()],
    ['shortcut.copyPaste', isMac() ? '⌘C · ⌘V' : 'Ctrl+Shift+C · Ctrl+Shift+V'],
  ]);
</script>

<Modal title={t('settings.title')} width={600} onclose={() => (app.settingsOpen = false)}>
  <div class="tabs segmented">
    {#each SECTIONS as sec (sec.id)}
      <button class:on={section === sec.id} onclick={() => (section = sec.id)}>{t(sec.label)}</button>
    {/each}
  </div>

  {#if section === 'appearance'}
    <div class="row lang">
      <span>{t('settings.language')}</span>
      <div class="segmented">
        {#each languages as l (l.id)}
          <button class:on={s.language === l.id} onclick={() => app.update({ language: l.id })}>{l.name}</button>
        {/each}
      </div>
    </div>

    <span class="label block">{t('settings.theme')}</span>
    <div class="themes">
      {#each THEMES as th (th.id)}
        <button class="theme" class:on={s.theme === th.id} onclick={() => app.update({ theme: th.id })}>
          <span class="preview" style:background={th.preview[0]}>
            {#if th.id === 'system'}
              <span class="half" style:background="#2a2d31"></span>
            {/if}
            <span class="surface" style:background={th.preview[1]}>
              <span class="line" style:background={th.preview[3]} style:width="38%"></span>
              <span class="line" style:background={th.preview[2]} style:width="62%"></span>
              <span class="line" style:background={th.preview[2]} style:width="48%"></span>
            </span>
          </span>
          <span class="tname">{themeLabel(th.id)}</span>
        </button>
      {/each}
    </div>

    <div class="rows">
      <label class="row">
        <span>{t('settings.font')}</span>
        <input
          class="input"
          value={s.fontFamily}
          placeholder={t('settings.fontPlaceholder')}
          spellcheck="false"
          onchange={(e) => app.update({ fontFamily: e.currentTarget.value })}
        />
      </label>
      <div class="row">
        <span>{t('settings.fontSize')}</span>
        <div class="stepper">
          <button class="btn" onclick={() => fontSize(-1)} aria-label={t('settings.smaller')}>−</button>
          <span class="value">{s.fontSize}</span>
          <button class="btn" onclick={() => fontSize(1)} aria-label={t('settings.larger')}>+</button>
        </div>
      </div>
      <div class="row">
        <span>{t('settings.lineHeight')}</span>
        <Select
          value={s.lineHeight}
          options={LINE_HEIGHTS.map((lh) => ({ value: lh, label: formatNumber(lh) }))}
          onchange={(lineHeight) => app.update({ lineHeight })}
          label={t('settings.lineHeight')}
          width={160}
        />
      </div>
    </div>
  {:else if section === 'terminal'}
    <div class="rows">
      <div class="row">
        <span>{t('settings.cursor')}</span>
        <div class="segmented">
          {#each CURSORS as c (c.id)}
            <button class:on={s.cursorStyle === c.id} onclick={() => app.update({ cursorStyle: c.id })}>{t(c.label)}</button>
          {/each}
        </div>
      </div>
      <label class="row">
        <span>{t('settings.cursorBlink')}</span>
        <input type="checkbox" class="toggle" checked={s.cursorBlink} onchange={(e) => app.update({ cursorBlink: e.currentTarget.checked })} />
      </label>
      <div class="row">
        <span>{t('settings.scrollback')}</span>
        <Select
          value={s.scrollback}
          options={SCROLLBACKS.map((n) => ({ value: n, label: tn('settings.scrollbackLines', n) }))}
          onchange={(scrollback) => app.update({ scrollback })}
          label={t('settings.scrollback')}
          width={160}
        />
      </div>
      <label class="row">
        <span>{t('settings.copyOnSelect')}</span>
        <input type="checkbox" class="toggle" checked={s.copyOnSelect} onchange={(e) => app.update({ copyOnSelect: e.currentTarget.checked })} />
      </label>
      <div class="row">
        <span>{t('settings.rightClick')}</span>
        <div class="segmented">
          <button class:on={!s.rightClickPaste} onclick={() => app.update({ rightClickPaste: false })}>
            {t('settings.rightClickMenu')}
          </button>
          <button class:on={s.rightClickPaste} onclick={() => app.update({ rightClickPaste: true })}>
            {t('settings.rightClickCopyPaste')}
          </button>
        </div>
      </div>
      <label class="row">
        <span>
          {t('settings.gpu')}
          <small>{t('settings.gpuHint')}</small>
        </span>
        <input type="checkbox" class="toggle" checked={s.gpuAcceleration} onchange={(e) => app.update({ gpuAcceleration: e.currentTarget.checked })} />
      </label>
      <div class="row top">
        <span>
          {t('settings.localShell')}
          {#if windows}<small>{t('settings.localShellHint')}</small>{/if}
        </span>
        <Select
          value={s.localShell}
          options={shellOptions}
          onchange={(localShell) => app.update({ localShell })}
          label={t('settings.localShell')}
          width={240}
        />
      </div>
      {#if s.localShell === CUSTOM_SHELL}
        <div class="row top">
          <span>
            {t('settings.shellCommand')}
            <small>{t('settings.shellCommandHint')}</small>
          </span>
          <div class="command">
            <input
              class="input mono"
              value={s.localShellCommand}
              placeholder={commandExample}
              aria-label={t('settings.shellCommand')}
              spellcheck="false"
              autocomplete="off"
              onchange={(e) => app.update({ localShellCommand: e.currentTarget.value.trim() })}
            />
            <button class="btn" onclick={browseProgram}>{t('editor.browse')}</button>
          </div>
        </div>
      {/if}
      {#if windows}
        {@const menu = s.explorerMenu ? app.explorerMenuState : null}
        <div class="row top">
          <span>
            <label for="explorer-menu">{t('settings.explorerMenu')}</label>
            <small>{t('settings.explorerMenuHint')}</small>
            {#if menu?.needsTrust}
              <small>{t('settings.explorerMenuTrustHint')}</small>
              <button class="btn trust" disabled={trusting} onclick={trustExplorerMenu}>
                {t('settings.explorerMenuTrust')}
              </button>
            {:else if menu?.error}
              <small class="warn">{t('settings.explorerMenuFallback', { reason: menu.error })}</small>
            {/if}
          </span>
          <input
            id="explorer-menu"
            type="checkbox"
            class="toggle"
            checked={s.explorerMenu}
            onchange={(e) => setExplorerMenu(e.currentTarget.checked)}
          />
        </div>
      {/if}
    </div>
  {:else if section === 'keyboard'}
    {#if !isMac()}
      <label class="row top">
        <span>
          {t('settings.ctrlK')}
          <small>{t('settings.ctrlKHint')}</small>
        </span>
        <input type="checkbox" class="toggle" checked={s.ctrlKInTerminal} onchange={(e) => app.update({ ctrlKInTerminal: e.currentTarget.checked })} />
      </label>
    {/if}
    <div class="shortcuts">
      {#each shortcuts as [label, keysText] (label)}
        <div class="sc"><span>{t(label)}</span><kbd>{keysText}</kbd></div>
      {/each}
    </div>
  {:else}
    <div class="about">
      <img src={logo} alt="" />
      <div>
        <h3>NexSSH</h3>
        <p>{t('settings.version', { version: app.info?.version ?? '—' })}</p>
      </div>
    </div>
    <div class="rows">
      <div class="row top">
        <span>{t('settings.dataFolder')} <small>{t('settings.dataFolderHint')}</small></span>
        <button class="btn" onclick={copyPath}>{t('settings.copyPath')}</button>
      </div>
      <p class="path mono">{app.info?.dataDir ?? ''}</p>
      {#if updates.supported}
        <div class="row">
          <span>
            {t('update.title')}
            <small>{updateStatus}</small>
          </span>
          {#if updates.status === 'available'}
            <button class="btn" onclick={() => updates.download()}>{t('update.download')}</button>
          {:else if updates.status === 'ready'}
            <button class="btn primary" onclick={() => updates.install()}>{t('update.install')}</button>
          {:else}
            <button
              class="btn"
              disabled={updates.status === 'checking' || updates.status === 'downloading' || updates.status === 'installing'}
              onclick={() => updates.check(true)}
            >
              {t('update.check')}
            </button>
          {/if}
        </div>
        <label class="row">
          <span>{t('update.auto')}</span>
          <input
            type="checkbox"
            class="toggle"
            checked={s.autoUpdateCheck}
            onchange={(e) => app.update({ autoUpdateCheck: e.currentTarget.checked })}
          />
        </label>
      {/if}
      <div class="row">
        <span>{t('settings.keychain')}</span>
        <span class:ok={!app.keychainIssue} class:bad={!!app.keychainIssue}>
          {app.keychainIssue ? t('settings.keychainBad', { reason: app.keychainIssue }) : t('settings.keychainOk')}
        </span>
      </div>
    </div>
    <p class="foot">{t('settings.license')}</p>
  {/if}
</Modal>

<style>
  .tabs {
    margin-bottom: 18px;
  }
  .block {
    display: block;
  }
  .themes {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    gap: 10px;
    margin: 8px 0 20px;
  }
  .theme {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 7px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--text-2);
  }
  .preview {
    position: relative;
    width: 100%;
    aspect-ratio: 1.35;
    border-radius: 9px;
    border: 1px solid var(--border);
    overflow: hidden;
    display: flex;
    align-items: flex-end;
    justify-content: flex-end;
    transition: box-shadow 0.12s;
  }
  .theme.on .preview {
    box-shadow:
      0 0 0 2px var(--bg),
      0 0 0 4px var(--text-2);
  }
  .theme.on .tname {
    color: var(--text);
    font-weight: 500;
  }
  .half {
    position: absolute;
    inset: 0 0 0 50%;
  }
  .surface {
    position: relative;
    width: 72%;
    height: 70%;
    border-radius: 6px 0 0 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 9px 8px;
  }
  .line {
    height: 3px;
    border-radius: 2px;
    opacity: 0.85;
  }
  .rows {
    display: flex;
    flex-direction: column;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    min-height: 44px;
    padding: 6px 0;
    border-bottom: 1px solid var(--border-soft);
  }
  .row.top {
    align-items: flex-start;
  }
  .row.lang {
    margin-bottom: 14px;
  }
  .row > span:first-child {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .row small {
    color: var(--text-2);
    font-size: 11.5px;
    line-height: 1.4;
    max-width: 380px;
  }
  .row small.warn {
    color: var(--amber);
    overflow-wrap: anywhere;
  }
  .row .trust {
    align-self: flex-start;
    margin-top: 6px;
  }
  .row .input {
    max-width: 260px;
  }
  .command {
    display: flex;
    gap: 6px;
    width: 300px;
    flex: none;
  }
  .command .input {
    flex: 1;
    min-width: 0;
    max-width: none;
  }
  .stepper {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .stepper .btn {
    width: 30px;
    padding: 0;
  }
  .value {
    min-width: 24px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
  .toggle {
    appearance: none;
    position: relative;
    flex: none;
    width: 34px;
    height: 20px;
    margin: 0;
    border-radius: 999px;
    background: var(--active);
    transition: background 0.15s var(--ease);
  }
  .toggle::after {
    content: '';
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--bg);
    box-shadow: var(--shadow-sm);
    transition: transform 0.15s var(--ease);
  }
  .toggle:checked {
    background: var(--green);
  }
  .toggle:checked::after {
    transform: translateX(14px);
  }
  .shortcuts {
    display: flex;
    flex-direction: column;
    margin-top: 10px;
  }
  .sc {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    min-height: 34px;
    border-bottom: 1px solid var(--border-soft);
  }
  .sc:last-child {
    border-bottom: 0;
  }
  .sc kbd {
    white-space: nowrap;
  }
  .about {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-bottom: 10px;
  }
  .about img {
    width: 44px;
    filter: var(--logo-filter);
  }
  .about h3 {
    margin: 0;
    font-size: 16px;
  }
  .about p {
    margin: 2px 0 0;
    color: var(--text-2);
  }
  .path {
    margin: 0;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    background: var(--surface);
    border: 1px solid var(--border-soft);
    word-break: break-all;
    -webkit-user-select: text;
    user-select: text;
  }
  .ok {
    color: var(--green);
  }
  .bad {
    color: var(--amber);
    text-align: right;
  }
  .foot {
    margin: 16px 0 0;
    color: var(--text-3);
    font-size: 12px;
  }
</style>
