// Open tabs and their SSH sessions. Terminals register themselves here; output from the
// backend is written straight into them (never through reactive state).

import { api, Channel, errorMessage, type OpenTarget, type SessionMessage } from '../api';
import type { ForwardInfo, Prompt, PromptReply, Server, SessionEvent } from '../types';
import { destination } from './servers.svelte';

export type TabStatus = 'connecting' | 'connected' | 'disconnected';

export interface Tab {
  key: string;
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
    this.#open({ serverId: server.id }, server.name, destination(server), server.id);
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
    this.#open({ destination: d }, host || d, d, null);
  }

  duplicate(tab: Tab) {
    this.#open({ ...tab.target }, tab.title, tab.subtitle, tab.serverId);
  }

  #open(target: OpenTarget, title: string, subtitle: string, serverId: string | null) {
    const tab: Tab = {
      key: `t${++counter}`,
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

  /** Called by a terminal view once it is mounted and sized: starts the SSH session. */
  async start(key: string, terminal: TerminalSink, cols: number, rows: number) {
    terminals.set(key, terminal);
    const tab = this.get(key);
    if (!tab) return;
    const channel = new Channel<SessionMessage>();
    channel.onmessage = (msg) => this.#onMessage(key, msg);
    try {
      const id = await api.openSession($state.snapshot(tab.target), cols, rows, channel);
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
          tab.message = ev.message ?? 'Disconnected';
          tab.failed = !/^Session ended/.test(tab.message);
          tab.prompts = [];
          term?.write(`\r\n${DIM}── ${tab.message} ──${RESET}\r\n`);
        } else if (ev.status === 'closed') {
          this.#remove(tab.key);
        }
        break;
      case 'log': {
        const color = ev.level === 'error' ? RED : ev.level === 'warn' ? YELLOW : DIM;
        // Errors also end up in the status message; only echo infos and warnings.
        if (ev.level !== 'error') term?.write(`${color}${ev.message}${RESET}\r\n`);
        break;
      }
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

  /** Keyboard input from the terminal. Enter reconnects a disconnected session. */
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
