// Open tabs and their sessions: SSH connections and local terminals, which the backend drives
// the same way. Terminals register themselves here; output from the backend is written
// straight into them (never through reactive state).

import { api, Channel, errorMessage, type LocalTarget, type OpenTarget, type SessionMessage } from '../api';
import { t } from '../i18n.svelte';
import type { ForwardInfo, Prompt, PromptReply, Server, SessionEvent, ShellProfile } from '../types';
import { app } from './app.svelte';
import { destination } from './servers.svelte';
import { CUSTOM_SHELL, commandName, missingShellName, shellName, shells } from './shells.svelte';
import { toasts } from './toasts.svelte';

export type TabStatus = 'connecting' | 'connected' | 'disconnected';

/** An SSH session, or a local terminal (a shell on this computer: "connected" means running). */
export type TabKind = 'ssh' | 'local';

export interface Tab {
  key: string;
  kind: TabKind;
  sessionId: number | null;
  serverId: string | null;
  target: OpenTarget;
  title: string;
  subtitle: string;
  status: TabStatus;
  /** Why the session is disconnected. */
  message: string | null;
  /** The last disconnect was a failure rather than a clean exit. */
  failed: boolean;
  prompts: { id: number; prompt: Prompt }[];
  forwards: ForwardInfo[];
}

/** What the session layer needs from a terminal view. */
export interface TerminalSink {
  write(data: Uint8Array | string): void;
  focus(): void;
  clear(): void;
}

const terminals = new Map<string, TerminalSink>();

/** Serializes writes per tab: keeps keystrokes in order and batches pastes. */
class WriteQueue {
  #pending = '';
  #busy = false;

  push(sessionId: number, data: string) {
    this.#pending += data;
    if (!this.#busy) void this.#drain(sessionId);
  }

  async #drain(sessionId: number) {
    this.#busy = true;
    while (this.#pending) {
      const chunk = this.#pending;
      this.#pending = '';
      try {
        await api.write(sessionId, chunk);
      } catch {
        // The session is gone; its status event explains why.
      }
    }
    this.#busy = false;
  }
}

const DIM = '\x1b[2m';
const RED = '\x1b[31m';
const YELLOW = '\x1b[33m';
const RESET = '\x1b[0m';

let counter = 0;

/** Core messages are lowercase fragments ("cannot connect to …"); show them as sentences. */
function sentence(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** The last part of a path, for a tab's title (a drive's root stays `C:\`). */
function folderName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '');
  if (/^[A-Za-z]:$/.test(trimmed)) return `${trimmed}\\`;
  return trimmed.split(/[\\/]/).pop() || path;
}

class SessionsState {
  tabs = $state<Tab[]>([]);
  activeKey = $state<string | null>(null);
  active: Tab | null = $derived(this.tabs.find((t) => t.key === this.activeKey) ?? null);

  #queues = new Map<string, WriteQueue>();

  get(key: string): Tab | undefined {
    return this.tabs.find((t) => t.key === key);
  }

  forServer(serverId: string): Tab[] {
    return this.tabs.filter((t) => t.serverId === serverId);
  }

  /** Connection state of a saved server, for sidebar indicators. */
  serverStatus(serverId: string): TabStatus | 'idle' {
    let best: TabStatus | 'idle' = 'idle';
    for (const t of this.tabs) {
      if (t.serverId !== serverId) continue;
      if (t.status === 'connected') return 'connected';
      if (t.status === 'connecting') best = 'connecting';
      else if (best === 'idle') best = 'disconnected';
    }
    return best;
  }

  openServer(server: Server) {
    this.#open('ssh', { serverId: server.id }, server.name, destination(server), server.id);
  }

  /** Focuses an existing tab of the server, or opens a new session. */
  focusOrOpen(server: Server) {
    const existing = this.forServer(server.id);
    const pick = existing.find((t) => t.key === this.activeKey) ?? existing[existing.length - 1];
    if (pick) this.activeKey = pick.key;
    else this.openServer(server);
  }

  openDestination(dest: string) {
    const d = dest.trim();
    // The tab shows just the host; the full destination is in the tooltip.
    const host = d.replace(/^ssh:\/\//, '').replace(/^.*@/, '').replace(/^\[([^\]]+)\].*$/, '$1').replace(/:\d+$/, '');
    this.#open('ssh', { destination: d }, host || d, d, null);
  }

  /**
   * Opens a local terminal: `shell`, else the one the settings choose; in `cwd`, else in the
   * home folder. Opened in a folder, the tab is named after it.
   */
  async openLocal(options: { shell?: ShellProfile; cwd?: string } = {}) {
    await shells.load();
    const setting = app.settings.localShell;
    const custom = app.settings.localShellCommand.trim();
    let target: LocalTarget;
    let name: string;
    if (options.shell) {
      target = { profile: options.shell.id };
      name = shellName(options.shell);
    } else if (setting === CUSTOM_SHELL && custom) {
      target = { command: custom };
      name = commandName(custom);
    } else {
      const chosen = setting && setting !== CUSTOM_SHELL ? shells.byId(setting) : undefined;
      // Without a list (it could not be read) the backend starts its default shell.
      const shell = chosen ?? shells.default;
      if (!chosen && shell && setting && setting !== CUSTOM_SHELL) {
        toasts.error(t('local.missing', { name: missingShellName(setting), fallback: shellName(shell) }));
      }
      target = shell ? { profile: shell.id } : {};
      name = shell ? shellName(shell) : t('nav.localTerminal');
    }
    if (options.cwd) target.cwd = options.cwd;
    const title = options.cwd ? folderName(options.cwd) : name;
    const subtitle = options.cwd ? `${name} · ${options.cwd}` : name;
    this.#open('local', { local: target }, title, subtitle, null);
  }

  duplicate(tab: Tab) {
    this.#open(tab.kind, $state.snapshot(tab.target), tab.title, tab.subtitle, tab.serverId);
  }

  #open(kind: TabKind, target: OpenTarget, title: string, subtitle: string, serverId: string | null) {
    const tab: Tab = {
      key: `t${++counter}`,
      kind,
      sessionId: null,
      serverId,
      target,
      title,
      subtitle,
      status: 'connecting',
      message: null,
      failed: false,
      prompts: [],
      forwards: [],
    };
    this.tabs.push(tab);
    this.activeKey = tab.key;
  }

  /** Called by a terminal view once it is mounted and sized: starts the session. */
  async start(key: string, terminal: TerminalSink, cols: number, rows: number) {
    terminals.set(key, terminal);
    const tab = this.get(key);
    if (!tab) return;
    const channel = new Channel<SessionMessage>();
    channel.onmessage = (msg) => this.#onMessage(key, msg);
    try {
      const target = $state.snapshot(tab.target);
      const id = target.local
        ? await api.openLocal(target.local, cols, rows, channel)
        : await api.openSession(target, cols, rows, channel);
      const current = this.get(key);
      if (!current) {
        void api.close(id);
        return;
      }
      current.sessionId = id;
    } catch (e) {
      const current = this.get(key);
      if (current) {
        current.status = 'disconnected';
        current.failed = true;
        current.message = errorMessage(e);
      }
      terminal.write(`${RED}${errorMessage(e)}${RESET}\r\n`);
    }
  }

  /** Called when a terminal view is destroyed. */
  detach(key: string) {
    terminals.delete(key);
  }

  #onMessage(key: string, msg: SessionMessage) {
    const term = terminals.get(key);
    if (msg instanceof ArrayBuffer) {
      term?.write(new Uint8Array(msg));
      return;
    }
    const tab = this.get(key);
    if (!tab) return;
    this.#onEvent(tab, msg, term);
  }

  #onEvent(tab: Tab, ev: SessionEvent, term: TerminalSink | undefined) {
    switch (ev.type) {
      case 'status':
        if (ev.status === 'connecting') {
          tab.status = 'connecting';
          tab.message = null;
        } else if (ev.status === 'connected') {
          tab.status = 'connected';
          tab.message = null;
          tab.failed = false;
        } else if (ev.status === 'disconnected') {
          tab.status = 'disconnected';
          tab.message = sentence(ev.message ?? t('session.disconnected'));
          tab.failed = ev.failed;
          tab.prompts = [];
          term?.write(`\r\n${DIM}── ${tab.message} ──${RESET}\r\n`);
        } else if (ev.status === 'closed') {
          this.#remove(tab.key);
        }
        break;
      case 'log':
        // Errors also end up in the status message. While connecting, progress goes to the
        // terminal like `ssh -v` lite; once the shell runs, writing into it would garble
        // the user's command line, so only warnings surface (as toasts).
        if (ev.level === 'error') break;
        if (tab.status === 'connecting') {
          term?.write(`${ev.level === 'warn' ? YELLOW : DIM}${ev.message}${RESET}\r\n`);
        } else if (ev.level === 'warn') {
          toasts.show(`${tab.title}: ${ev.message}`, 'error');
        }
        break;
      case 'prompt':
        tab.prompts.push({ id: ev.id, prompt: ev.prompt });
        break;
      case 'promptClosed':
        tab.prompts = tab.prompts.filter((p) => p.id !== ev.id);
        break;
      case 'forwards':
        tab.forwards = ev.forwards;
        break;
    }
  }

  /** Keyboard input from the terminal. Enter reconnects a disconnected session (or starts a
   * local terminal's shell again). */
  input(key: string, data: string) {
    const tab = this.get(key);
    if (!tab || tab.sessionId == null) return;
    if (tab.status === 'disconnected') {
      if (data === '\r') this.reconnect(tab);
      return;
    }
    if (tab.status !== 'connected') return;
    let queue = this.#queues.get(key);
    if (!queue) this.#queues.set(key, (queue = new WriteQueue()));
    queue.push(tab.sessionId, data);
  }

  inputBinary(key: string, data: string) {
    const tab = this.get(key);
    if (!tab || tab.sessionId == null || tab.status !== 'connected') return;
    const bytes = Array.from(data, (c) => c.charCodeAt(0) & 0xff);
    void api.writeBinary(tab.sessionId, bytes).catch(() => {});
  }

  resize(key: string, cols: number, rows: number) {
    const tab = this.get(key);
    if (tab?.sessionId == null) return;
    void api.resize(tab.sessionId, cols, rows).catch(() => {});
  }

  reconnect(tab: Tab) {
    if (tab.sessionId == null) return;
    tab.status = 'connecting';
    tab.message = null;
    terminals.get(tab.key)?.write('\r\n');
    void api.reconnect(tab.sessionId).catch(() => {});
  }

  /** Drops the connection but keeps the tab (reconnect with Enter). */
  disconnect(tab: Tab) {
    if (tab.sessionId == null || tab.status === 'disconnected') return;
    void api.disconnect(tab.sessionId).catch(() => {});
  }

  answer(tab: Tab, promptId: number, reply: PromptReply) {
    tab.prompts = tab.prompts.filter((p) => p.id !== promptId);
    void api.answer(promptId, reply).catch(() => {});
  }

  close(key: string) {
    const tab = this.get(key);
    if (!tab) return;
    if (tab.sessionId != null) void api.close(tab.sessionId).catch(() => {});
    this.#remove(key);
  }

  #remove(key: string) {
    const index = this.tabs.findIndex((t) => t.key === key);
    if (index === -1) return;
    this.tabs.splice(index, 1);
    this.#queues.delete(key);
    if (this.activeKey === key) {
      this.activeKey = this.tabs[index]?.key ?? this.tabs[index - 1]?.key ?? null;
    }
  }

  clear(tab: Tab) {
    terminals.get(tab.key)?.clear();
  }

  focusActive() {
    if (this.activeKey) terminals.get(this.activeKey)?.focus();
  }

  cycle(step: number) {
    if (!this.tabs.length) return;
    const index = this.tabs.findIndex((t) => t.key === this.activeKey);
    const next = (index + step + this.tabs.length) % this.tabs.length;
    this.activeKey = this.tabs[next].key;
  }

  select(index: number) {
    const tab = index === -1 ? this.tabs[this.tabs.length - 1] : this.tabs[index];
    if (tab) this.activeKey = tab.key;
  }

  move(from: number, to: number) {
    if (from === to || from < 0 || to < 0 || from >= this.tabs.length || to >= this.tabs.length) return;
    const [tab] = this.tabs.splice(from, 1);
    this.tabs.splice(to, 0, tab);
  }
}

export const sessions = new SessionsState();
