// Open tabs and their sessions: SSH connections and local terminals, which the backend drives
// the same way. Terminals register themselves here; output from the backend is written
// straight into them (never through reactive state).

import { api, Channel, errorMessage, type LocalTarget, type OpenTarget, type SessionMessage } from '../api';
import { t } from '../i18n.svelte';
import type {
  AgentOpen,
  AgentScreen,
  ForwardInfo,
  Prompt,
  PromptReply,
  SavedTabs,
  Server,
  SessionEvent,
  ShellProfile,
  TerminalSnapshot,
} from '../types';
import { app } from './app.svelte';
import { destination, servers } from './servers.svelte';
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
  /** The user name typed for a host without one (a `user` prompt), until the session connects;
   * `server`: the saved server to remember it for. */
  typedUser: { name: string; host: string; server: string | null } | null;
  /** AI agents' requests waiting for this tab to connect (see state/agents). */
  agentOpens: number[];
  /** Opened again from the last run (state/restore): it connects when the user says so (Enter),
   * so no password is asked for out of the blue. */
  waiting: boolean;
}

/** What the session layer needs from a terminal view. */
export interface TerminalSink {
  write(data: Uint8Array | string): void;
  focus(): void;
  clear(): void;
  snapshot(lines: number | null): TerminalSnapshot;
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
  /** Each terminal's size, for sessions opened after it is on screen. */
  #sizes = new Map<string, { cols: number; rows: number }>();

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

  /**
   * An AI agent needs a session of a saved server: a tab of it that is connecting or can be
   * reconnected, else a new one. It opens behind the tab the user is in (keystrokes keep going
   * there) and comes forward when it needs the user: a password, a host key. The backend learns
   * that it connected by itself; a failure is reported.
   */
  openForAgent(open: AgentOpen) {
    const server = servers.byId.get(open.serverId);
    if (!server) {
      void api.agentsOpenFailed(open.id, t('agents.serverGone')).catch(() => {});
      return;
    }
    const mine = this.forServer(server.id);
    let tab = mine.find((t) => t.status === 'connecting') ?? mine.find((t) => t.status === 'disconnected');
    if (tab?.status === 'disconnected') this.reconnect(tab);
    if (!tab) {
      const key = this.#open('ssh', { serverId: server.id }, server.name, destination(server), server.id, true);
      tab = this.get(key);
    }
    tab?.agentOpens.push(open.id);
  }

  /**
   * What a terminal tab of a saved server shows, for an agent: tab number `tab` of the server
   * (from 1, in tab bar order), else the one the user is in, else the last; the screen, or the
   * last `lines` lines. Why not, when it cannot be read.
   */
  screen(serverId: string, tab: number | null, lines: number | null): { screen: AgentScreen } | { error: string } {
    const mine = this.tabs.filter((t) => t.kind === 'ssh' && t.serverId === serverId);
    if (!mine.length) return { error: 'noTab' };
    const pick = tab ? mine[tab - 1] : (mine.find((t) => t.key === this.activeKey) ?? mine[mine.length - 1]);
    if (!pick) return { error: 'noSuchTab' };
    const terminal = terminals.get(pick.key);
    if (!terminal) return { error: 'notReady' };
    return {
      screen: {
        ...terminal.snapshot(lines),
        title: pick.title,
        status: pick.status,
        tab: mine.indexOf(pick) + 1,
        tabs: mine.length,
      },
    };
  }

  /** Tells the backend that the tab its agents wait for will not connect. */
  #failAgentOpens(tab: Tab, message: string) {
    for (const id of tab.agentOpens) void api.agentsOpenFailed(id, message).catch(() => {});
    tab.agentOpens = [];
  }

  /** Focuses an existing tab of the server, or opens a new session. */
  focusOrOpen(server: Server) {
    const existing = this.forServer(server.id);
    const pick = existing.find((t) => t.key === this.activeKey) ?? existing[existing.length - 1];
    if (pick) {
      this.activeKey = pick.key;
      app.sidebarForSession();
    } else {
      this.openServer(server);
    }
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

  /**
   * Opens the tabs of the last run again (see state/restore), after the ones open now: local
   * terminals start, SSH tabs wait for Enter. Tabs of saved servers deleted since are left out.
   * The tab that was in front comes to the front again. Says how many tabs came back, and how
   * many of them wait.
   */
  reopen(saved: SavedTabs): { opened: number; waiting: number } {
    const keys: (string | null)[] = [];
    let waiting = 0;
    for (const s of saved.tabs) {
      const serverId = s.target.serverId ?? null;
      const server = serverId ? servers.byId.get(serverId) : undefined;
      if (serverId && !server) {
        keys.push(null);
        continue;
      }
      // A saved server may have been renamed since.
      const title = server ? server.name : s.title;
      const subtitle = server ? destination(server) : s.subtitle;
      const wait = s.kind === 'ssh';
      if (wait) waiting++;
      keys.push(this.#open(s.kind, s.target, title, subtitle, serverId, true, wait));
    }
    const opened = keys.filter((k) => k !== null);
    if (saved.active >= 0 && opened.length) {
      this.activeKey = keys[saved.active] ?? opened[0];
      app.sidebarForSession();
    }
    return { opened: opened.length, waiting };
  }

  /** Connects the tabs of the last run that are still waiting. */
  connectWaiting() {
    for (const tab of this.tabs) if (tab.waiting) this.#begin(tab);
  }

  /** Adds a tab and returns its key; `behind`: the active tab stays active (if there is one);
   * `waiting`: it connects when the user says so (a tab of the last run). */
  #open(
    kind: TabKind,
    target: OpenTarget,
    title: string,
    subtitle: string,
    serverId: string | null,
    behind = false,
    waiting = false,
  ): string {
    const tab: Tab = {
      key: `t${++counter}`,
      kind,
      sessionId: null,
      serverId,
      target,
      title,
      subtitle,
      status: waiting ? 'disconnected' : 'connecting',
      message: waiting ? t('restore.banner') : null,
      failed: false,
      prompts: [],
      forwards: [],
      typedUser: null,
      agentOpens: [],
      waiting,
    };
    this.tabs.push(tab);
    if (!behind || this.activeKey === null) this.activeKey = tab.key;
    // Tabs agents open are not the user's doing: the layout stays.
    if (!behind) app.sidebarForSession();
    return tab.key;
  }

  /** Called by a terminal view once it is mounted and sized: starts the session (a tab of the
   * last run waits for the user instead). */
  async start(key: string, terminal: TerminalSink, cols: number, rows: number) {
    terminals.set(key, terminal);
    this.#sizes.set(key, { cols, rows });
    const tab = this.get(key);
    if (!tab) return;
    if (tab.waiting) {
      terminal.write(`${DIM}${t('restore.pressEnter')}${RESET}\r\n`);
      return;
    }
    await this.#connect(key, terminal, cols, rows);
  }

  /** Opens the session of a tab that has none: a tab of the last run, or one whose session could
   * not be opened. */
  #begin(tab: Tab) {
    tab.waiting = false;
    tab.status = 'connecting';
    tab.message = null;
    tab.failed = false;
    const terminal = terminals.get(tab.key);
    const size = this.#sizes.get(tab.key);
    // Not on screen yet: start() opens it.
    if (!terminal || !size) return;
    terminal.write('\r\n');
    void this.#connect(tab.key, terminal, size.cols, size.rows);
  }

  async #connect(key: string, terminal: TerminalSink, cols: number, rows: number) {
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
        this.#failAgentOpens(current, current.message);
      }
      terminal.write(`${RED}${errorMessage(e)}${RESET}\r\n`);
    }
  }

  /** Called when a terminal view is destroyed. */
  detach(key: string) {
    terminals.delete(key);
    this.#sizes.delete(key);
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
          tab.agentOpens = [];
          this.#keepTypedUser(tab);
        } else if (ev.status === 'disconnected') {
          tab.status = 'disconnected';
          tab.message = sentence(ev.message ?? t('session.disconnected'));
          tab.failed = ev.failed;
          tab.prompts = [];
          term?.write(`\r\n${DIM}── ${tab.message} ──${RESET}\r\n`);
          this.#failAgentOpens(tab, tab.message);
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
        // A tab opened behind for an agent needs the user now, also with the window in the tray.
        if (tab.agentOpens.length) {
          this.activeKey = tab.key;
          void api.windowAttention().catch(() => {});
        }
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
   * local terminal's shell again, or connects a tab of the last run). */
  input(key: string, data: string) {
    const tab = this.get(key);
    if (!tab) return;
    if (tab.status === 'disconnected') {
      if (data === '\r') this.reconnect(tab);
      return;
    }
    if (tab.sessionId == null || tab.status !== 'connected') return;
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
    if (this.#sizes.has(key)) this.#sizes.set(key, { cols, rows });
    const tab = this.get(key);
    if (tab?.sessionId == null) return;
    void api.resize(tab.sessionId, cols, rows).catch(() => {});
  }

  reconnect(tab: Tab) {
    if (tab.sessionId == null) {
      if (tab.status === 'disconnected') this.#begin(tab);
      return;
    }
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

  /** Answers a `user` prompt with `name`; `remember`: also for the saved server it is about,
   * once logging in with it worked. */
  answerUser(tab: Tab, promptId: number, prompt: Extract<Prompt, { kind: 'user' }>, name: string, remember: boolean) {
    tab.typedUser = { name, host: prompt.host, server: remember ? (prompt.serverId ?? null) : null };
    this.answer(tab, promptId, { kind: 'user', name });
  }

  /** After logging in with a typed user name: the address typed for the tab gets it, so that
   * duplicating the tab or saving it as a server keeps it, and so does the saved server it was
   * to be remembered for. */
  #keepTypedUser(tab: Tab) {
    const typed = tab.typedUser;
    if (!typed) return;
    tab.typedUser = null;
    const typedHost = tab.target.destination?.replace(/:22$/, '');
    if (typedHost && typedHost === typed.host) {
      tab.target.destination = `${typed.name}@${tab.target.destination}`;
      tab.subtitle = tab.target.destination;
    }
    const server = typed.server ? servers.byId.get(typed.server) : undefined;
    if (server && !server.user) {
      servers
        .save({ ...$state.snapshot(server), user: typed.name })
        .then((saved) => {
          for (const other of this.forServer(saved.id)) other.subtitle = destination(saved);
        })
        .catch((e) => toasts.error(errorMessage(e)));
    }
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
    this.#failAgentOpens(this.tabs[index], t('agents.tabClosed'));
    this.tabs.splice(index, 1);
    this.#queues.delete(key);
    this.#sizes.delete(key);
    if (this.activeKey === key) {
      this.activeKey = this.tabs[index]?.key ?? this.tabs[index - 1]?.key ?? null;
    }
    if (!this.tabs.length) app.sidebarForHome();
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
