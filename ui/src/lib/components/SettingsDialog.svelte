<script lang="ts">
  import logo from '../../assets/logo.png';
  import { keys } from '../actions';
  import { api, errorMessage } from '../api';
  import { formatNumber, i18n, LANGUAGES, t, timeAgo, tn, type LanguageSetting, type MessageKey } from '../i18n.svelte';
  import { isMac, shortcut } from '../platform';
  import { agents } from '../state/agents.svelte';
  import { edits } from '../state/edits.svelte';
  import { app, clampFontSize, type SettingsSection } from '../state/app.svelte';
  import { servers } from '../state/servers.svelte';
  import { sessions } from '../state/sessions.svelte';
  import { CUSTOM_SHELL, missingShellName, shellName, shells } from '../state/shells.svelte';
  import { toasts } from '../state/toasts.svelte';
  import { updates } from '../state/updates.svelte';
  import { THEMES, themeLabel } from '../themes';
  import type { AgentActivity, Server, Settings } from '../types';
  import Icon from './Icon.svelte';
  import Modal from './Modal.svelte';
  import Select from './Select.svelte';

  const LINE_HEIGHTS = [1, 1.1, 1.2, 1.25, 1.3, 1.4, 1.5];
  const SCROLLBACKS = [1000, 5000, 10000, 50000, 100000];

  let section = $state<SettingsSection>(app.settingsSection);

  const s = $derived(app.settings);
  const windows = $derived(app.info?.os === 'windows');

  // The shells and editors are looked for again: one may have been installed since.
  void shells.load(true);
  void edits.loadEditors();

  const editorOptions: { value: string; label: string }[] = $derived.by(() => {
    const first = edits.editors[0]?.name ?? t('settings.editorSystem');
    const options = [
      { value: '', label: t('settings.editorAuto', { name: first }) },
      ...edits.editors.map((e) => ({ value: e.id, label: e.name })),
    ];
    const saved = s.editor;
    if (saved && saved !== 'system' && saved !== 'custom' && !edits.editors.some((e) => e.id === saved)) {
      options.push({ value: saved, label: t('settings.editorMissing', { name: saved }) });
    }
    options.push({ value: 'system', label: t('settings.editorSystem') }, { value: 'custom', label: t('settings.editorCustom') });
    return options;
  });

  const editorExample = $derived(
    windows ? String.raw`"C:\Program Files\Notepad++\notepad++.exe" -multiInst {file}` : 'subl {file}',
  );

  async function browseEditor() {
    try {
      const path = await api.pickProgram(t('settings.editorPick'));
      if (path) app.update({ editorCommand: /\s/.test(path) ? `"${path}"` : path });
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

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

  /** Turned on with a session open, the sidebar steps aside right away. */
  function setSidebarAutoHide(on: boolean) {
    app.update({ sidebarAutoHide: on });
    if (on && sessions.active) app.sidebarForSession();
  }

  function fontSize(delta: number) {
    app.update({ fontSize: clampFontSize(s.fontSize + delta) });
  }

  async function copyPath() {
    if (!app.info) return;
    await navigator.clipboard.writeText(app.info.dataDir).catch(() => {});
    toasts.show(t('settings.pathCopied'));
  }

  const SECTIONS: { id: SettingsSection; label: MessageKey }[] = [
    { id: 'appearance', label: 'settings.appearance' },
    { id: 'terminal', label: 'settings.terminal' },
    { id: 'files', label: 'settings.files' },
    { id: 'keyboard', label: 'settings.keyboard' },
    { id: 'agents', label: 'settings.agents' },
    { id: 'about', label: 'settings.about' },
  ];

  // ---- AI agents --------------------------------------------------------------------

  type Client = 'claude' | 'codex' | 'json' | 'http';
  const CLIENTS: { id: Client; label: string; hint: MessageKey }[] = [
    { id: 'claude', label: 'Claude Code', hint: 'agents.hintClaude' },
    { id: 'codex', label: 'Codex', hint: 'agents.hintCodex' },
    { id: 'json', label: 'JSON', hint: 'agents.hintJson' },
    { id: 'http', label: 'HTTP', hint: 'agents.hintHttp' },
  ];
  let client = $state<Client>('claude');
  let token = $state<string | null>(null);
  let tokenShown = $state(false);
  let copied = $state(false);
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  const agentStatus = $derived(agents.status);
  const sharedServers = $derived(servers.data.servers.filter((sv) => sv.agents && sv.agents !== 'off'));

  /** How an agent connects to NexSSH: the bridge (`NexSSH mcp`, no token) or HTTP. */
  function snippet(kind: Client, secret: string): string {
    const exe = agentStatus?.exe || 'NexSSH';
    const url = agentStatus?.url ?? `http://127.0.0.1:${s.agentsPort}/mcp`;
    switch (kind) {
      case 'claude':
        return `claude mcp add --scope user nexssh "${exe}" mcp`;
      case 'codex':
        // A JSON string is a valid TOML basic string.
        return `[mcp_servers.nexssh]\ncommand = ${JSON.stringify(exe)}\nargs = ["mcp"]`;
      case 'json':
        return JSON.stringify({ mcpServers: { nexssh: { command: exe, args: ['mcp'] } } }, null, 2);
      case 'http':
        return JSON.stringify(
          { mcpServers: { nexssh: { type: 'http', url, headers: { Authorization: `Bearer ${secret}` } } } },
          null,
          2,
        );
    }
  }

  const shownSnippet = $derived(snippet(client, tokenShown && token ? token : '••••••••'));

  async function ensureToken(): Promise<string> {
    if (!token) token = await api.agentsToken();
    return token;
  }

  async function copySnippet() {
    try {
      const text = snippet(client, client === 'http' ? await ensureToken() : '');
      await navigator.clipboard.writeText(text);
      copied = true;
      clearTimeout(copiedTimer);
      copiedTimer = setTimeout(() => (copied = false), 1500);
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  async function toggleToken() {
    try {
      if (!tokenShown) await ensureToken();
      tokenShown = !tokenShown;
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  async function renewToken() {
    if (!(await app.confirm(t('agents.newTokenTitle'), t('agents.newTokenConfirm'), t('agents.newToken')))) return;
    try {
      token = await api.agentsNewToken();
      toasts.show(t('agents.newTokenDone'), 'success');
    } catch (e) {
      toasts.error(errorMessage(e));
    }
  }

  function setPort(value: string) {
    const port = Number(value.trim());
    if (Number.isInteger(port) && port >= 1 && port <= 65535) app.update({ agentsPort: port });
  }

  function editShared(server: Server) {
    app.settingsOpen = false;
    app.openEditor(server);
  }

  const ACTIVITY_ICONS: Record<AgentActivity['status'], string> = {
    waiting: 'more',
    running: 'sparkle',
    done: 'check',
    failed: 'alert',
    denied: 'x',
  };

  function activityMeta(a: AgentActivity): string {
    const parts = [a.agent, a.serverName];
    if (a.status === 'done' && a.exitCode != null) parts.push(t('agents.exit', { code: a.exitCode }));
    else parts.push(t(`agents.status.${a.status}` as MessageKey));
    if (a.durationMs != null) parts.push(duration(a.durationMs));
    parts.push(new Date(a.at).toLocaleTimeString(i18n.lang, { hour: '2-digit', minute: '2-digit' }));
    return parts.join(' · ');
  }

  function duration(ms: number): string {
    if (ms >= 60_000) {
      const secs = Math.round(ms / 1000);
      return `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
    }
    const digits = ms < 10_000 ? 1 : 0;
    return new Intl.NumberFormat(i18n.lang, {
      style: 'unit',
      unit: 'second',
      unitDisplay: 'short',
      maximumFractionDigits: digits,
      minimumFractionDigits: digits,
    }).format(ms / 1000);
  }

  const TOOLS: Record<string, MessageKey> = {
    run_command: 'agents.tool.run_command',
    read_file: 'agents.tool.read_file',
    write_file: 'agents.tool.write_file',
    list_directory: 'agents.tool.list_directory',
    terminal_read: 'agents.tool.terminal_read',
    upload: 'agents.tool.upload',
    download: 'agents.tool.download',
  };

  function toolLabel(tool: string): string {
    return TOOLS[tool] ? t(TOOLS[tool]) : tool;
  }

  const languages: { id: LanguageSetting; name: string }[] = $derived([
    { id: 'system', name: t('settings.languageSystem') },
    ...LANGUAGES,
  ]);

  const PASTE_WARNINGS: { id: Settings['pasteWarning']; label: MessageKey }[] = [
    { id: 'auto', label: 'settings.pasteAuto' },
    { id: 'always', label: 'settings.pasteAlways' },
    { id: 'never', label: 'settings.pasteNever' },
  ];
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
    ['shortcut.openLink', t('shortcut.click', { key: shortcut('Mod') })],
    ['shortcut.fontSize', `${keys.zoomIn()} · ${keys.zoomOut()} · ${keys.zoomReset()}`],
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
      <label class="row top">
        <span>
          {t('settings.sidebarAutoHide')}
          <small>{t('settings.sidebarAutoHideHint', { key: keys.sidebar() })}</small>
        </span>
        <input
          type="checkbox"
          class="toggle"
          checked={s.sidebarAutoHide}
          onchange={(e) => setSidebarAutoHide(e.currentTarget.checked)}
        />
      </label>
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
      <div class="row top">
        <span>
          {t('settings.pasteWarning')}
          <small>{t('settings.pasteWarningHint')}</small>
        </span>
        <div class="segmented">
          {#each PASTE_WARNINGS as p (p.id)}
            <button class:on={s.pasteWarning === p.id} onclick={() => app.update({ pasteWarning: p.id })}>{t(p.label)}</button>
          {/each}
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
  {:else if section === 'files'}
    <div class="rows">
      <div class="row top">
        <span>
          {t('settings.editor')}
          <small>{t('settings.editorHint')}</small>
        </span>
        <Select
          value={s.editor}
          options={editorOptions}
          onchange={(editor) => app.update({ editor })}
          label={t('settings.editor')}
          width={240}
        />
      </div>
      {#if s.editor === 'custom'}
        <div class="row top">
          <span>
            {t('settings.editorCommand')}
            <small>{t('settings.editorCommandHint')}</small>
          </span>
          <div class="command">
            <input
              class="input mono"
              value={s.editorCommand}
              placeholder={editorExample}
              aria-label={t('settings.editorCommand')}
              spellcheck="false"
              autocomplete="off"
              onchange={(e) => app.update({ editorCommand: e.currentTarget.value.trim() })}
            />
            <button class="btn" onclick={browseEditor}>{t('editor.browse')}</button>
          </div>
        </div>
      {/if}
      <div class="row">
        <span>{t('settings.doubleClick')}</span>
        <div class="segmented">
          <button class:on={s.filesDoubleClick === 'download'} onclick={() => app.update({ filesDoubleClick: 'download' })}>
            {t('settings.doubleClickDownload')}
          </button>
          <button class:on={s.filesDoubleClick === 'edit'} onclick={() => app.update({ filesDoubleClick: 'edit' })}>
            {t('settings.doubleClickEdit')}
          </button>
        </div>
      </div>
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
  {:else if section === 'agents'}
    <div class="rows">
      <div class="row top">
        <span>
          <label for="agents-enabled">{t('agents.enable')}</label>
          <small>{t('agents.enableHint')}</small>
          {#if s.agentsEnabled}
            {#if agentStatus?.error}
              <small class="warn">{t('agents.failed', { reason: agentStatus.error })}</small>
            {:else if agentStatus?.running}
              <small class="ok mono">{t('agents.running', { url: agentStatus.url })}</small>
            {:else}
              <small>{t('agents.starting')}</small>
            {/if}
          {/if}
        </span>
        <input
          id="agents-enabled"
          type="checkbox"
          class="toggle"
          checked={s.agentsEnabled}
          onchange={(e) => app.update({ agentsEnabled: e.currentTarget.checked })}
        />
      </div>
    </div>

    {#if s.agentsEnabled}
      <div class="connect">
        <span class="label">{t('agents.connect')}</span>
        <div class="segmented">
          {#each CLIENTS as c (c.id)}
            <button class:on={client === c.id} onclick={() => (client = c.id)}>{c.label}</button>
          {/each}
        </div>
      </div>
      <div class="snippet">
        <pre class="mono">{shownSnippet}</pre>
        <button class="btn copy" onclick={copySnippet}>
          <Icon name={copied ? 'check' : 'copy'} size={14} />
          {t(copied ? 'agents.copied' : 'agents.copy')}
        </button>
      </div>
      <p class="note">{t(CLIENTS.find((c) => c.id === client)?.hint ?? 'agents.hintJson')}</p>
      {#if client === 'http'}
        <div class="token">
          <button class="btn" onclick={toggleToken}>{t(tokenShown ? 'agents.hideToken' : 'agents.showToken')}</button>
          <button class="btn" onclick={renewToken}>{t('agents.newToken')}</button>
        </div>
      {/if}
      {#if agentStatus?.running && !agentStatus.tokenKept}
        <p class="note warn">{t('agents.tokenNotKept')}</p>
      {/if}

      <div class="rows">
        <div class="row">
          <span>{t('agents.port')} <small>{t('agents.portHint')}</small></span>
          <input
            class="input port"
            inputmode="numeric"
            value={s.agentsPort}
            aria-label={t('agents.port')}
            onchange={(e) => setPort(e.currentTarget.value)}
          />
        </div>
        <div class="row top">
          <span>{t('agents.shared')}</span>
          {#if sharedServers.length}
            <span class="chips">
              {#each sharedServers as server (server.id)}
                <button class="chip" onclick={() => editShared(server)}>
                  {server.name}
                  <small>{t(server.agents === 'allow' ? 'agents.accessAllow' : 'agents.accessAsk')}</small>
                </button>
              {/each}
            </span>
          {:else}
            <small class="empty">{t('agents.sharedNone')}</small>
          {/if}
        </div>
        {#if agentStatus?.agents.length}
          <div class="row top">
            <span>{t('agents.recent')}</span>
            <span class="chips">
              {#each agentStatus.agents as seen (seen.name)}
                <span class="chip static">{seen.name} <small>{timeAgo(seen.lastSeen / 1000)}</small></span>
              {/each}
            </span>
          </div>
        {/if}
      </div>

      <div class="activity-head">
        <span class="label">{t('agents.activity')}</span>
        {#if agentStatus?.activity.length}
          <button class="btn ghost small" onclick={() => void api.agentsClearActivity()}>{t('agents.clear')}</button>
        {/if}
      </div>
      {#if agentStatus?.activity.length}
        <ul class="activity">
          {#each agentStatus.activity as a (a.id)}
            <li class={a.status}>
              <span class="icon"><Icon name={ACTIVITY_ICONS[a.status]} size={14} stroke={1.9} /></span>
              <span class="what">
                <span class="summary"><span class="tool">{toolLabel(a.tool)}</span><code>{a.detail}</code></span>
                <small>{activityMeta(a)}</small>
                {#if a.error}<small class="error">{a.error}</small>{/if}
              </span>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="note">{t('agents.activityEmpty')}</p>
      {/if}
    {/if}
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
  .row small.ok {
    color: var(--green);
    overflow-wrap: anywhere;
  }
  .connect {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin: 16px 0 8px;
  }
  .snippet {
    position: relative;
  }
  .snippet pre {
    margin: 0;
    padding: 10px 12px;
    padding-right: 116px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-soft);
    background: var(--surface);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    -webkit-user-select: text;
    user-select: text;
  }
  .snippet .copy {
    position: absolute;
    top: 6px;
    right: 6px;
    height: 28px;
  }
  .note {
    margin: 8px 0 0;
    color: var(--text-2);
    font-size: 12px;
    line-height: 1.45;
  }
  .note.warn {
    color: var(--amber);
  }
  .token {
    display: flex;
    gap: 6px;
    margin-top: 8px;
  }
  .connect + .snippet + .note + .rows,
  .token + .rows,
  .note + .rows {
    margin-top: 10px;
  }
  .input.port {
    width: 90px;
    text-align: right;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 6px;
    max-width: 380px;
  }
  .chip {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
    padding: 3px 9px;
    border: 1px solid var(--border-soft);
    border-radius: 999px;
    background: var(--surface);
    color: var(--text);
    font-size: 12.5px;
  }
  button.chip:hover {
    background: var(--hover);
  }
  .chip small {
    color: var(--text-2);
    font-size: 11px;
  }
  .empty {
    max-width: 320px;
    color: var(--text-2);
    font-size: 12px;
    text-align: right;
  }
  .activity-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 16px 0 4px;
    min-height: 28px;
  }
  .activity-head .btn.small {
    height: 26px;
    padding: 0 8px;
    color: var(--text-2);
  }
  .activity {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .activity li {
    display: flex;
    gap: 10px;
    padding: 7px 0;
    border-bottom: 1px solid var(--border-soft);
  }
  .activity li:last-child {
    border-bottom: 0;
  }
  .activity .icon {
    flex: none;
    display: grid;
    padding-top: 2px;
    color: var(--text-2);
  }
  .activity .done .icon {
    color: var(--green);
  }
  .activity .failed .icon {
    color: var(--red);
  }
  .activity .denied .icon {
    color: var(--amber);
  }
  .activity .running .icon {
    color: var(--text);
    animation: pulse 1.4s var(--ease) infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .what {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .what .summary {
    display: flex;
    gap: 8px;
    min-width: 0;
  }
  .what .tool {
    flex: none;
    color: var(--text-2);
  }
  .what code {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .what small {
    color: var(--text-3);
    font-size: 11.5px;
  }
  .what small.error {
    color: var(--red);
    overflow-wrap: anywhere;
  }
</style>
