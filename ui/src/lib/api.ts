// Typed wrappers around Tauri IPC. This is the only module that talks to the backend.

import { Channel, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  AgentScreen,
  AgentScreenRequest,
  AgentsStatus,
  AppInfo,
  EditInfo,
  EditorInfo,
  Downloaded,
  DownloadProgress,
  ExplorerMenuState,
  ForwardSpec,
  ImportReport,
  KeyInfo,
  LocalShells,
  PickedUpload,
  PixelRect,
  PromptReply,
  Server,
  SessionEvent,
  Settings,
  SftpEntry,
  SnapButtonState,
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

  /** Opens a web link from the terminal in the default browser (http and https only). */
  openLink: (url: string) => invoke<void>('open_link', { url }),

  localShells: () => invoke<LocalShells>('local_shells'),
  /** A local terminal; afterwards it is a session like an SSH one (write, resize, close…). */
  openLocal: (target: LocalTarget, cols: number, rows: number, onEvent: Channel<SessionMessage>) =>
    invoke<number>('local_open', { target, cols, rows, onEvent }),
  /** The system file dialog for the program of a custom shell; `null` when cancelled. */
  pickProgram: (title: string) => invoke<string | null>('pick_program', { title }),
  /** Windows: adds or removes "Open with NexSSH" in Explorer, labelled in the backend's language
   * (in Windows 11's compact menu where it can). */
  setExplorerMenu: (enabled: boolean) => invoke<ExplorerMenuState>('explorer_menu', { enabled }),
  /** Windows 11: moves it into Explorer's compact menu; Windows asks the user for administrator
   * rights (unchanged when they say no). */
  trustExplorerMenu: () => invoke<ExplorerMenuState>('explorer_menu_trust'),
  /** Windows 11: where the title bar's maximize button is, in the window's pixels, for the
   * snap layouts there (`null`: no such button). */
  windowSnapButton: (rect: PixelRect | null) => invoke<void>('window_snap_button', { rect }),
  /** How the maximize button should look while the snap layouts' window takes the mouse. */
  onSnapButton: (handler: (state: SnapButtonState) => void) =>
    listen<SnapButtonState>('snap-button', (e) => handler(e.payload)),
  /** Folders to open local terminals in, asked for with `--cwd` (e.g. from Explorer). */
  launchTake: () => invoke<string[]>('launch_take'),
  /** NexSSH was started again with `--cwd`: `launchTake` has new folders. */
  onLaunch: (handler: () => void) => listen('launch', handler),

  sftpHome: (sessionId: number) => invoke<string>('sftp_home', { sessionId }),
  sftpResolve: (sessionId: number, path: string) => invoke<string>('sftp_resolve', { sessionId, path }),
  sftpList: (sessionId: number, path: string) => invoke<SftpEntry[]>('sftp_list', { sessionId, path }),
  sftpMkdir: (sessionId: number, path: string) => invoke<void>('sftp_mkdir', { sessionId, path }),
  /** Creates the folders that do not exist (folder uploads merge into existing ones), parents
   * first. */
  sftpEnsureDirs: (sessionId: number, paths: string[]) => invoke<void>('sftp_ensure_dirs', { sessionId, paths }),
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
  /** The system file dialog for an upload (any number of files); empty when cancelled. */
  sftpPickFiles: (title: string) => invoke<PickedUpload[]>('sftp_pick_files', { title }),
  /** Uploads a file or folder from `sftpPickUpload`/`sftpPickFiles` (or dropped on the window)
   * into `dir`; returns the remote path. */
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

  /** Remote files edited in a program of this computer. */
  editEditors: () => invoke<EditorInfo[]>('edit_editors'),
  /** Opens a remote file in the editor of the settings (with sudo: `password`, or one typed
   * earlier, or none); rejects with an `EditRefusal`. A file edited already opens again. */
  editOpen: (sessionId: number, path: string, sudo = false, password?: string) =>
    invoke<EditInfo>('edit_open', { sessionId, path, sudo, password: password ?? null }),
  editList: () => invoke<EditInfo[]>('edit_list'),
  onEdits: (handler: (list: EditInfo[]) => void) => listen<EditInfo[]>('edits', (e) => handler(e.payload)),
  editResolve: (id: number, resolution: 'overwrite' | 'reload') =>
    invoke<EditInfo | null>('edit_resolve', { id, resolution }),
  /** Writes the file with sudo from now on (`password`: sudo's) and sends it. */
  editSudo: (id: number, password?: string) => invoke<EditInfo | null>('edit_sudo', { id, password: password ?? null }),
  editStop: (id: number) => invoke<void>('edit_stop', { id }),
  editReveal: (id: number) => invoke<void>('edit_reveal', { id }),

  /** AI agents (MCP): everything the page shows about them, sent again on every change. */
  agentsStatus: () => invoke<AgentsStatus>('agents_status'),
  onAgents: (handler: (status: AgentsStatus) => void) =>
    listen<AgentsStatus>('agents', (e) => handler(e.payload)),
  /** The bearer token for agents that connect over HTTP. */
  agentsToken: () => invoke<string>('agents_token'),
  /** A new token; agents holding the old one are refused. */
  agentsNewToken: () => invoke<string>('agents_new_token'),
  /** The user's answer to a request; `remember`: don't ask this agent about this server again
   * until NexSSH quits. */
  agentsAnswer: (id: number, allow: boolean, remember: boolean) =>
    invoke<void>('agents_answer', { id, allow, remember }),
  /** The tab opened for an agent did not connect. */
  agentsOpenFailed: (id: number, message: string) => invoke<void>('agents_open_failed', { id, message }),
  agentsClearActivity: () => invoke<void>('agents_clear_activity'),
  /** An agent wants to see a terminal tab of a server (`terminal_read`). */
  onAgentsRead: (handler: (request: AgentScreenRequest) => void) =>
    listen<AgentScreenRequest>('agents-read', (e) => handler(e.payload)),
  /** The answer: what the tab shows, or why it cannot be read (`noTab`, `noSuchTab`, `notReady`). */
  agentsScreen: (id: number, screen: AgentScreen | null, error: string | null) =>
    invoke<void>('agents_screen', { id, screen, error }),

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
