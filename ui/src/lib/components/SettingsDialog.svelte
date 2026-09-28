<script lang="ts">
  import logo from '../../assets/logo.png';
  import { keys } from '../actions';
  import { isMac, shortcut } from '../platform';
  import { app } from '../state/app.svelte';
  import { toasts } from '../state/toasts.svelte';
  import { THEMES } from '../themes';
  import Modal from './Modal.svelte';

  type Section = 'appearance' | 'terminal' | 'keyboard' | 'about';
  let section = $state<Section>('appearance');

  const s = $derived(app.settings);

  function fontSize(delta: number) {
    app.update({ fontSize: Math.min(28, Math.max(9, s.fontSize + delta)) });
  }

  async function copyPath() {
    if (!app.info) return;
    await navigator.clipboard.writeText(app.info.dataDir).catch(() => {});
    toasts.show('Path copied');
  }

  const shortcuts = $derived([
    ['Command palette', isMac() ? keys.palette() : `${keys.palette()} · ${shortcut('Ctrl', 'Shift', 'K')}`],
    ['New session', keys.newSession()],
    ['Close tab', keys.closeTab()],
    ['Next / previous tab', isMac() ? `${shortcut('Mod', 'Shift', ']')} · ${shortcut('Mod', 'Shift', '[')}` : 'Ctrl+Tab · Ctrl+Shift+Tab'],
    ['Go to tab 1–9', shortcut('Mod', '1…9')],
    ['Reconnect', keys.reconnect()],
    ['Toggle sidebar', keys.sidebar()],
    ['Full screen', keys.fullscreen()],
    ['Settings', keys.settings()],
    ['Copy / paste', isMac() ? '⌘C · ⌘V' : 'Ctrl+Shift+C · Ctrl+Shift+V'],
  ]);
</script>

<Modal title="Settings" width={600} onclose={() => (app.settingsOpen = false)}>
  <div class="tabs segmented">
    <button class:on={section === 'appearance'} onclick={() => (section = 'appearance')}>Appearance</button>
    <button class:on={section === 'terminal'} onclick={() => (section = 'terminal')}>Terminal</button>
    <button class:on={section === 'keyboard'} onclick={() => (section = 'keyboard')}>Keyboard</button>
    <button class:on={section === 'about'} onclick={() => (section = 'about')}>About</button>
  </div>

  {#if section === 'appearance'}
    <span class="label block">Theme</span>
    <div class="themes">
      {#each THEMES as t (t.id)}
        <button class="theme" class:on={s.theme === t.id} onclick={() => app.update({ theme: t.id })}>
          <span class="preview" style:background={t.preview[0]}>
            {#if t.id === 'system'}
              <span class="half" style:background="#2a2d31"></span>
            {/if}
            <span class="surface" style:background={t.preview[1]}>
              <span class="line" style:background={t.preview[3]} style:width="38%"></span>
              <span class="line" style:background={t.preview[2]} style:width="62%"></span>
              <span class="line" style:background={t.preview[2]} style:width="48%"></span>
            </span>
          </span>
          <span class="tname">{t.label}</span>
        </button>
      {/each}
    </div>

    <div class="rows">
      <label class="row">
        <span>Font</span>
        <input
          class="input"
          value={s.fontFamily}
          placeholder="JetBrains Mono (built in)"
          spellcheck="false"
          onchange={(e) => app.update({ fontFamily: e.currentTarget.value })}
        />
      </label>
      <div class="row">
        <span>Font size</span>
        <div class="stepper">
          <button class="btn" onclick={() => fontSize(-1)} aria-label="Smaller">−</button>
          <span class="value">{s.fontSize}</span>
          <button class="btn" onclick={() => fontSize(1)} aria-label="Larger">+</button>
        </div>
      </div>
      <label class="row">
        <span>Line height</span>
        <select class="input narrow" value={String(s.lineHeight)} onchange={(e) => app.update({ lineHeight: Number(e.currentTarget.value) })}>
          {#each [1, 1.1, 1.2, 1.25, 1.3, 1.4, 1.5] as lh (lh)}<option value={String(lh)}>{lh}</option>{/each}
        </select>
      </label>
    </div>
  {:else if section === 'terminal'}
    <div class="rows">
      <div class="row">
        <span>Cursor</span>
        <div class="segmented">
          {#each ['bar', 'block', 'underline'] as const as c (c)}
            <button class:on={s.cursorStyle === c} onclick={() => app.update({ cursorStyle: c })}>{c[0].toUpperCase() + c.slice(1)}</button>
          {/each}
        </div>
      </div>
      <label class="row">
        <span>Blinking cursor</span>
        <input type="checkbox" class="toggle" checked={s.cursorBlink} onchange={(e) => app.update({ cursorBlink: e.currentTarget.checked })} />
      </label>
      <label class="row">
        <span>Scrollback</span>
        <select class="input narrow" value={String(s.scrollback)} onchange={(e) => app.update({ scrollback: Number(e.currentTarget.value) })}>
          {#each [1000, 5000, 10000, 50000, 100000] as n (n)}<option value={String(n)}>{n.toLocaleString()} lines</option>{/each}
        </select>
      </label>
      <label class="row">
        <span>Copy on select</span>
        <input type="checkbox" class="toggle" checked={s.copyOnSelect} onchange={(e) => app.update({ copyOnSelect: e.currentTarget.checked })} />
      </label>
      <div class="row">
        <span>Right click</span>
        <div class="segmented">
          <button class:on={!s.rightClickPaste} onclick={() => app.update({ rightClickPaste: false })}>Menu</button>
          <button class:on={s.rightClickPaste} onclick={() => app.update({ rightClickPaste: true })}>Copy / Paste</button>
        </div>
      </div>
      <label class="row">
        <span>
          GPU rendering
          <small>WebGL renderer: lower CPU usage on heavy output.</small>
        </span>
        <input type="checkbox" class="toggle" checked={s.gpuAcceleration} onchange={(e) => app.update({ gpuAcceleration: e.currentTarget.checked })} />
      </label>
    </div>
  {:else if section === 'keyboard'}
    {#if !isMac()}
      <label class="row top">
        <span>
          Ctrl+K opens the palette inside the terminal
          <small>Off by default: in the shell Ctrl+K deletes to the end of the line (and cuts in nano). Ctrl+Shift+K always opens the palette.</small>
        </span>
        <input type="checkbox" class="toggle" checked={s.ctrlKInTerminal} onchange={(e) => app.update({ ctrlKInTerminal: e.currentTarget.checked })} />
      </label>
    {/if}
    <div class="shortcuts">
      {#each shortcuts as [label, keysText] (label)}
        <div class="sc"><span>{label}</span><kbd>{keysText}</kbd></div>
      {/each}
    </div>
  {:else}
    <div class="about">
      <img src={logo} alt="" />
      <div>
        <h3>NexSSH</h3>
        <p>Version {app.info?.version ?? '—'} · a fast, minimal SSH client</p>
      </div>
    </div>
    <div class="rows">
      <div class="row top">
        <span>Data folder <small>servers.json, settings.json and known_hosts (no secrets).</small></span>
        <button class="btn" onclick={copyPath}>Copy path</button>
      </div>
      <p class="path mono">{app.info?.dataDir ?? ''}</p>
      <div class="row">
        <span>System keychain</span>
        <span class:ok={!app.keychainIssue} class:bad={!!app.keychainIssue}>
          {app.keychainIssue ? `Unavailable — ${app.keychainIssue}` : 'Available'}
        </span>
      </div>
    </div>
    <p class="foot">Open source under the MIT license · github.com/likDanil/NexSSH</p>
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
  .row .input {
    max-width: 260px;
  }
  .narrow {
    width: 160px;
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
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 2px 18px;
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
