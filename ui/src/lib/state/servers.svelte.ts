// Saved servers (loaded from and written through the backend store).

import { api, errorMessage } from '../api';
import type { ImportReport, Server, StoreData } from '../types';
import { toasts } from './toasts.svelte';

export interface ServerGroup {
  name: string;
  servers: Server[];
}

export const SUGGESTED_GROUPS = ['Production', 'Development', 'Personal'];

export function destination(s: Pick<Server, 'user' | 'host' | 'port'>): string {
  const host = s.host.includes(':') ? `[${s.host}]` : s.host;
  const base = s.user ? `${s.user}@${host}` : host;
  return s.port && s.port !== 22 ? `${base}:${s.port}` : base;
}

class ServersState {
  data = $state<StoreData>({ version: 1, groups: [], servers: [] });
  loaded = $state(false);

  byId: Map<string, Server> = $derived(new Map(this.data.servers.map((s) => [s.id, s])));

  /** Servers grouped in sidebar order; ungrouped servers come last under "". */
  groups: ServerGroup[] = $derived.by(() => {
    const map = new Map<string, Server[]>();
    for (const g of this.data.groups) map.set(g, []);
    for (const s of this.data.servers) {
      const key = s.group || '';
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(s);
    }
    const out: ServerGroup[] = [];
    for (const [name, servers] of map) {
      if (name === '') continue;
      out.push({ name, servers });
    }
    const ungrouped = map.get('');
    if (ungrouped?.length) out.push({ name: '', servers: ungrouped });
    return out;
  });

  groupNames: string[] = $derived(this.data.groups);

  recent: Server[] = $derived(
    [...this.data.servers]
      .filter((s) => s.lastUsed)
      .sort((a, b) => (b.lastUsed ?? 0) - (a.lastUsed ?? 0))
      .slice(0, 6),
  );

  async load() {
    try {
      this.data = await api.servers();
    } catch (e) {
      toasts.error(`Could not load servers: ${errorMessage(e)}`);
    } finally {
      this.loaded = true;
    }
  }

  async save(server: Server, password?: string, clearPassword = false): Promise<Server> {
    const { server: saved, warning } = await api.saveServer(server, password, clearPassword);
    await this.load();
    if (warning) toasts.show(warning, 'error', 7000);
    return saved;
  }

  async remove(id: string) {
    await api.deleteServer(id);
    await this.load();
  }

  async duplicate(server: Server) {
    const copy: Server = { ...$state.snapshot(server), id: '', name: `${server.name} copy`, alias: undefined, lastUsed: undefined };
    await this.save(copy);
  }

  async renameGroup(from: string, to: string) {
    this.data = await api.renameGroup(from, to);
  }

  async deleteGroup(name: string) {
    this.data = await api.deleteGroup(name);
  }

  async importSshConfig(): Promise<ImportReport | null> {
    try {
      const report = await api.importSshConfig();
      await this.load();
      const { added, updated } = report.summary;
      if (report.found === 0) {
        toasts.show(`No hosts found in ${report.path}`);
      } else if (added + updated === 0) {
        toasts.show(`Everything from ${report.path} is already here`);
      } else {
        const parts = [];
        if (added) parts.push(`${added} added`);
        if (updated) parts.push(`${updated} updated`);
        toasts.show(`Imported from ${report.path}: ${parts.join(', ')}`, 'success');
      }
      if (report.skipped.length) toasts.show(`Skipped ${report.skipped.join('; ')}`);
      return report;
    } catch (e) {
      toasts.error(`Import failed: ${errorMessage(e)}`);
      return null;
    }
  }
}

export const servers = new ServersState();
