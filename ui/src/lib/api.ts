// Typed wrappers around Tauri IPC. This is the only module that talks to the backend.

import { Channel, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  AppInfo,
  Downloaded,
  DownloadProgress,
  ExplorerMenuState,
  ForwardSpec,
  ImportReport,
  KeyInfo,
  LocalShells,
  PickedUpload,
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

/** Passwords saved with a server, in the OS keychain: a new one, or forget the stored one. */
export interface ServerSecrets {
  password?: string;
  clearPassword?: boolean;
  /** For its jump host, when that is typed in place rather than a saved server. */
  jumpPassword?: string;
  clearJumpPassword?: boolean;
}

/** A local terminal: a shell of `localShells`, or a command line, started in `cwd`. */
export interface LocalTarget {
  /** The default shell when neither this nor `command` is set. */
  profile?: string;
  command?: string;
  /** The home folder when not set. */
  cwd?: string;
}

export interface OpenTarget {
  serverId?: string;
  destination?: string;
  local?: LocalTarget;
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
  saveServer: (server: Server, secrets: ServerSecrets = {}) =>
    invoke<{ server: Server; warning?: string }>('server_save', {
      server,
      password: secrets.password || null,
      clearPassword: !!secrets.clearPassword,
      jumpPassword: secrets.jumpPassword || null,
      clearJumpPassword: !!secrets.clearJumpPassword,
    }),
  deleteServer: (id: string) => invoke<void>('server_delete', { id }),
  /** Whether a password is remembered for the server, or with `jump` for its jump host. */
  hasPassword: (id: string, jump = false) => invoke<boolean>('server_has_password', { id, jump }),
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

  localShells: () => invoke<LocalShells>('local_shells'),
  /** A local terminal; afterwards it is a session like an SSH one (write, resize, close…). */
  openLocal: (target: LocalTarget, cols: number, rows: number, onEvent: Channel<SessionMessage>) =>
    invoke<number>('local_open', { target, cols, rows, onEvent }),
  /** The system file dialog for the program of a custom shell; `null` when cancelled. */
  pickProgram: (title: string) => invoke<string | null>('pick_program', { title }),
  /** Windows: adds or removes "Open with NexSSH" in Explorer, labelled in the backend's language
   * (in Windows 11's compact menu where it can). */
  setExplorerMenu: (enabled: boolean) => invoke<ExplorerMenuState>('explorer_menu', { enabled }),
  /** Folders to open local terminals in, asked for with `--cwd` (e.g. from Explorer). */
  launchTake: () => invoke<string[]>('launch_take'),
  /** NexSSH was started again with `--cwd`: `launchTake` has new folders. */
  onLaunch: (handler: () => void) => listen('launch', handler),

  sftpHome: (sessionId: number) => invoke<string>('sftp_home', { sessionId }),
  sftpResolve: (sessionId: number, path: string) => invoke<string>('sftp_resolve', { sessionId, path }),
  sftpList: (sessionId: number, path: string) => invoke<SftpEntry[]>('sftp_list', { sessionId, path }),
  sftpMkdir: (sessionId: number, path: string) => invoke<void>('sftp_mkdir', { sessionId, path }),
  /** Creates the folder unless it exists (folder uploads merge into existing ones). */
  sftpEnsureDir: (sessionId: number, path: string) => invoke<void>('sftp_ensure_dir', { sessionId, path }),
  /** An empty file; never replaces one. */
  sftpNewFile: (sessionId: number, path: string) => invoke<void>('sftp_new_file', { sessionId, path }),
  sftpChmod: (sessionId: number, path: string, mode: number, recursive: boolean) =>
    invoke<void>('sftp_chmod', { sessionId, path, mode, recursive }),
  sftpRename: (sessionId: number, from: string, to: string) => invoke<void>('sftp_rename', { sessionId, from, to }),
  sftpRemove: (sessionId: number, path: string) => invoke<void>('sftp_remove', { sessionId, path }),
  /** The system folder dialog for "Download to…"; `null` when cancelled. */
  sftpPickDestination: (title: string) => invoke<string | null>('sftp_pick_destination', { title }),
  /** Saves into `dest` (a folder from `sftpPickDestination`), or into Downloads when `null`. */
  sftpDownload: (
    sessionId: number,
    path: string,
    dest: string | null,
    transferId: number,
    onProgress: Channel<TransferProgress>,
  ) => invoke<Downloaded>('sftp_download', { sessionId, path, dest, transferId, onProgress }),
  /** The system folder dialog for an upload; `null` when cancelled. */
  sftpPickUpload: (title: string) => invoke<PickedUpload | null>('sftp_pick_upload', { title }),
  /** Uploads a folder from `sftpPickUpload` into `dir`; returns the remote path. */
  sftpUploadPath: (
    sessionId: number,
    path: string,
    dir: string,
    transferId: number,
    onProgress: Channel<TransferProgress>,
  ) => invoke<string>('sftp_upload_path', { sessionId, path, dir, transferId, onProgress }),
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
