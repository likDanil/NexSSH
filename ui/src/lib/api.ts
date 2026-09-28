// Typed wrappers around Tauri IPC. This is the only module that talks to the backend.

import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  AppInfo,
  Downloaded,
  DownloadProgress,
  ForwardSpec,
  ImportReport,
  KeyInfo,
  PromptReply,
  Server,
  SessionEvent,
  Settings,
  SftpEntry,
  StoreData,
  TransferProgress,
  UpdateInfo,
} from './types';

export type SessionMessage = SessionEvent | ArrayBuffer;

export interface OpenTarget {
  serverId?: string;
  destination?: string;
}

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  appReady: () => invoke<void>('app_ready'),
  /** Language of backend messages (errors, connection progress). */
  setLanguage: (lang: string) => invoke<void>('app_set_language', { lang }),
  setFullscreen: (fullscreen: boolean) => invoke<void>('window_set_fullscreen', { fullscreen }),
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
  /** The system file dialog for a private key; `null` when cancelled. */
  pickKeyFile: (title: string) => invoke<string | null>('pick_key_file', { title }),

  openSession: (target: OpenTarget, cols: number, rows: number, onEvent: Channel<SessionMessage>) =>
    invoke<number>('session_open', { target, cols, rows, onEvent }),
  write: (id: number, data: string) => invoke<void>('session_write', { id, data }),
  writeBinary: (id: number, data: number[]) => invoke<void>('session_write_binary', { id, data }),
  resize: (id: number, cols: number, rows: number) => invoke<void>('session_resize', { id, cols, rows }),
  reconnect: (id: number) => invoke<void>('session_reconnect', { id }),
  disconnect: (id: number) => invoke<void>('session_disconnect', { id }),
  close: (id: number) => invoke<void>('session_close', { id }),
  answer: (id: number, reply: PromptReply) => invoke<void>('prompt_answer', { id, reply }),

  sftpHome: (sessionId: number) => invoke<string>('sftp_home', { sessionId }),
  sftpResolve: (sessionId: number, path: string) => invoke<string>('sftp_resolve', { sessionId, path }),
  sftpList: (sessionId: number, path: string) => invoke<SftpEntry[]>('sftp_list', { sessionId, path }),
  sftpMkdir: (sessionId: number, path: string) => invoke<void>('sftp_mkdir', { sessionId, path }),
  sftpRename: (sessionId: number, from: string, to: string) => invoke<void>('sftp_rename', { sessionId, from, to }),
  sftpRemove: (sessionId: number, path: string) => invoke<void>('sftp_remove', { sessionId, path }),
  /** Saves into the Downloads folder. */
  sftpDownload: (sessionId: number, path: string, transferId: number, onProgress: Channel<TransferProgress>) =>
    invoke<Downloaded>('sftp_download', { sessionId, path, transferId, onProgress }),
  sftpUploadBegin: (sessionId: number, path: string, transferId: number) =>
    invoke<void>('sftp_upload_begin', { sessionId, path, transferId }),
  /** Raw bytes, no JSON encoding. */
  sftpUploadChunk: (transferId: number, data: ArrayBuffer) =>
    invoke<void>('sftp_upload_chunk', data, { headers: { 'x-transfer-id': String(transferId) } }),
  sftpUploadEnd: (transferId: number) => invoke<void>('sftp_upload_end', { transferId }),
  sftpCancel: (transferId: number) => invoke<void>('sftp_cancel', { transferId }),
  revealDownload: (path: string) => invoke<void>('reveal_download', { path }),

  updateCheck: () => invoke<UpdateInfo | null>('update_check'),
  updateDownload: (onProgress: Channel<DownloadProgress>) => invoke<void>('update_download', { onProgress }),
  /** Runs the installer; on Windows the app exits and the new version starts by itself. */
  updateInstall: () => invoke<void>('update_install'),

  addForward: (sessionId: number, spec: ForwardSpec, saveTo?: string) =>
    invoke<void>('forward_add', { sessionId, spec, saveTo: saveTo ?? null }),
  removeForward: (sessionId: number, forwardId: number) =>
    invoke<void>('forward_remove', { sessionId, forwardId }),
};

/** An error as a sentence: backend messages are fragments ("cannot connect to …") so they
 * can be nested; shown on their own they start with a capital letter. */
export function errorMessage(e: unknown): string {
  const text = typeof e === 'string' ? e : e instanceof Error ? e.message : String(e);
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export { Channel };
