// Typed wrappers around Tauri IPC. This is the only module that talks to the backend.

import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  AppInfo,
  ForwardSpec,
  ImportReport,
  KeyInfo,
  PromptReply,
  Server,
  SessionEvent,
  Settings,
  StoreData,
} from './types';

export type SessionMessage = SessionEvent | ArrayBuffer;

export interface OpenTarget {
  serverId?: string;
  destination?: string;
}

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  appReady: () => invoke<void>('app_ready'),
  keychainStatus: () => invoke<string | null>('keychain_status'),
  saveSettings: (settings: Settings) => invoke<void>('settings_set', { settings }),

  servers: () => invoke<StoreData>('servers_list'),
  saveServer: (server: Server, password?: string, clearPassword = false) =>
    invoke<{ server: Server; warning?: string }>('server_save', { server, password: password || null, clearPassword }),
  deleteServer: (id: string) => invoke<void>('server_delete', { id }),
  hasPassword: (id: string) => invoke<boolean>('server_has_password', { id }),
  setGroups: (groups: string[]) => invoke<StoreData>('groups_set', { groups }),
  renameGroup: (from: string, to: string) => invoke<StoreData>('group_rename', { from, to }),
  deleteGroup: (name: string) => invoke<StoreData>('group_delete', { name }),
  importSshConfig: (path?: string) => invoke<ImportReport>('ssh_config_import', { path: path || null }),
  keys: () => invoke<KeyInfo[]>('keys_list'),

  openSession: (target: OpenTarget, cols: number, rows: number, onEvent: Channel<SessionMessage>) =>
    invoke<number>('session_open', { target, cols, rows, onEvent }),
  write: (id: number, data: string) => invoke<void>('session_write', { id, data }),
  writeBinary: (id: number, data: number[]) => invoke<void>('session_write_binary', { id, data }),
  resize: (id: number, cols: number, rows: number) => invoke<void>('session_resize', { id, cols, rows }),
  reconnect: (id: number) => invoke<void>('session_reconnect', { id }),
  disconnect: (id: number) => invoke<void>('session_disconnect', { id }),
  close: (id: number) => invoke<void>('session_close', { id }),
  answer: (id: number, reply: PromptReply) => invoke<void>('prompt_answer', { id, reply }),

  addForward: (sessionId: number, spec: ForwardSpec, saveTo?: string) =>
    invoke<void>('forward_add', { sessionId, spec, saveTo: saveTo ?? null }),
  removeForward: (sessionId: number, forwardId: number) =>
    invoke<void>('forward_remove', { sessionId, forwardId }),
};

export function errorMessage(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

export { Channel };
