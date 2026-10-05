// AI agents (MCP): the backend's status, its questions for the user and the tabs it asks for.
// The backend sends its whole status on every change; this keeps the latest one and acts on
// what is new in it (a page that loads late misses nothing).

import { api } from '../api';
import type { AgentActivity, AgentRequest, AgentsStatus } from '../types';
import { app } from './app.svelte';
import { sessions } from './sessions.svelte';

class AgentsState {
  status = $state<AgentsStatus | null>(null);
  /** The question on screen: the oldest one. */
  request: AgentRequest | null = $derived(this.status?.requests[0] ?? null);
  /** Questions after it. */
  waiting = $derived(Math.max(0, (this.status?.requests.length ?? 0) - 1));

  #opened = new Set<number>();

  async start() {
    await api.onAgents((status) => this.#update(status));
    // An agent reads a terminal tab (`terminal_read`): the page has the terminals.
    await api.onAgentsRead(({ id, serverId, tab, lines }) => {
      const result = sessions.screen(serverId, tab, lines);
      const screen = 'screen' in result ? result.screen : null;
      const error = 'error' in result ? result.error : null;
      void api.agentsScreen(id, screen, error).catch(() => {});
    });
    try {
      this.#update(await api.agentsStatus());
    } catch (e) {
      console.error('agents', e);
    }
  }

  #update(status: AgentsStatus) {
    this.status = status;
    app.agentAsking = status.requests.length > 0;
    for (const open of status.opens) {
      if (this.#opened.has(open.id)) continue;
      this.#opened.add(open.id);
      sessions.openForAgent(open);
    }
  }

  /** What an agent is doing on this session right now. */
  busy(sessionId: number | null): AgentActivity | undefined {
    if (sessionId == null) return undefined;
    return this.status?.activity.find((a) => a.status === 'running' && a.sessionId === sessionId);
  }

  answer(request: AgentRequest, allow: boolean, remember: boolean) {
    // Gone from the screen at once; the backend's next status agrees.
    if (this.status) this.status.requests = this.status.requests.filter((r) => r.id !== request.id);
    app.agentAsking = !!this.status?.requests.length;
    void api.agentsAnswer(request.id, allow, remember).catch(() => {});
  }
}

export const agents = new AgentsState();
