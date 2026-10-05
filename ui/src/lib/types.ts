// Mirrors of the Rust types (serde, camelCase). Keep in sync with core/src/model.rs,
// core/src/session/mod.rs and desktop/src/commands.rs.

import type { LanguageSetting } from './i18n.svelte';

export type AuthKind = 'auto' | 'password' | 'key' | 'agent';
/** What AI agents (over MCP) may do on a server: nothing, ask the user first, or anything. */
export type AgentAccess = 'off' | 'ask' | 'allow';
export type ForwardKind = 'local' | 'remote' | 'dynamic';

export interface ForwardSpec {
  kind: ForwardKind;
  bindHost: string;
  bindPort: number;
  targetHost: string;
  targetPort: number;
}

export interface Server {
  id: string;
  name: string;
  host: string;
  port: number;
  user: string;
  group: string;
  auth: AuthKind;
  identityFile?: string;
  /** A saved server's id or name, or `host[:port]` (comma-separated for a chain). */
  jumpHost?: string;
  /** Login on a jump host typed in place; its password is in the OS keychain. */
  jumpUser?: string;
  keepaliveSecs?: number;
  connectTimeoutSecs?: number;
  forwards?: ForwardSpec[];
  alias?: string;
  lastUsed?: number;
  /** Not written when `off`. */
  agents?: AgentAccess;
  /** Typed into the shell after every login, one command per line. */
  startupCommands?: string;
}

export interface StoreData {
  version: number;
  groups: string[];
  servers: Server[];
}

export type SessionStatus = 'connecting' | 'connected' | 'disconnected' | 'closed';

export type HostKeyCheck =
  | { status: 'trusted' }
  | { status: 'unknown' }
  | { status: 'changed'; file: string; line: number; knownFingerprint: string }
  | { status: 'revoked' };

export type Prompt =
  | { kind: 'hostKey'; host: string; port: number; keyType: string; fingerprint: string; check: HostKeyCheck }
  /** The host has no user name: which one to log in as. `serverId`: the saved server it is. */
  | { kind: 'user'; host: string; suggestion: string; serverId?: string }
  | { kind: 'password'; user: string; host: string; canRemember: boolean; error?: string }
  | { kind: 'passphrase'; keyPath: string; canRemember: boolean; error?: string }
  | {
      kind: 'keyboardInteractive';
      host: string;
      name: string;
      instructions: string;
      prompts: { text: string; echo: boolean }[];
    };

export type PromptReply =
  | { kind: 'hostKey'; accept: boolean; remember: boolean }
  | { kind: 'secret'; value: string; remember: boolean }
  | { kind: 'answers'; values: string[] }
  | { kind: 'user'; name: string }
  | { kind: 'cancel' };

export interface ForwardInfo {
  id: number;
  spec: ForwardSpec;
  description: string;
  saved: boolean;
}

export type SessionEvent =
  | { type: 'status'; status: SessionStatus; message?: string; failed: boolean }
  | { type: 'log'; level: 'info' | 'warn' | 'error'; message: string }
  | { type: 'prompt'; id: number; prompt: Prompt }
  | { type: 'promptClosed'; id: number }
  | { type: 'forwards'; forwards: ForwardInfo[] };

/** What a local shell is; mirrors `ShellKind` in core/src/local.rs. */
export type ShellKind = 'pwsh' | 'windowsPowerShell' | 'cmd' | 'wsl' | 'gitBash' | 'unix';

/** A shell found on this computer for local terminals. */
export interface ShellProfile {
  /** `pwsh`, `powershell`, `cmd`, `git-bash`, `wsl:<distribution>` or a Unix shell's path. */
  id: string;
  kind: ShellKind;
  /** English name; for WSL the distribution, for Unix shells the file name. */
  name: string;
  program: string;
  args: string[];
  default: boolean;
}

export interface LocalShells {
  shells: ShellProfile[];
  /** Windows' build number: xterm.js adapts to ConPTY by it. */
  windowsBuild?: number | null;
}

/** Where Windows Explorer shows "Open with NexSSH". */
export interface ExplorerMenuState {
  /** In Windows 11's compact context menu, not only under "Show more options". */
  modern: boolean;
  /** It can go there once the computer trusts NexSSH's certificate, which takes administrator
   * rights once (`api.trustExplorerMenu`). */
  needsTrust: boolean;
  /** Why it is not there, on Windows 11 (details for a bug report). */
  error?: string | null;
}

export interface KeyInfo {
  path: string;
  keyType: string;
  encrypted: boolean;
  fingerprint?: string;
}

export interface ImportReport {
  path: string;
  found: number;
  summary: { added: number; updated: number; unchanged: number };
  skipped: string[];
}

export type FilesSort = 'name' | 'size' | 'modified';

export type ThemeId = 'system' | 'light' | 'graphite' | 'black' | 'navy';
export type ResolvedTheme = Exclude<ThemeId, 'system'>;

/** UI preferences, stored by the backend in settings.json. */
export interface Settings {
  language: LanguageSetting;
  theme: ThemeId;
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  cursorStyle: 'bar' | 'block' | 'underline';
  cursorBlink: boolean;
  scrollback: number;
  copyOnSelect: boolean;
  rightClickPaste: boolean;
  /** Ask before text with line breaks goes into a terminal: `auto` unless the program takes
   * pastes as text (bracketed paste), so the lines do not run at once. */
  pasteWarning: 'auto' | 'always' | 'never';
  gpuAcceleration: boolean;
  /** Windows/Linux: plain Ctrl+K opens the palette even while typing in a terminal. */
  ctrlKInTerminal: boolean;
  sidebarWidth: number;
  sidebarHidden: boolean;
  /** Hide the sidebar while a session the user opened is shown; it comes back on the home
   * screen. Never saved as `sidebarHidden`. */
  sidebarAutoHide: boolean;
  collapsedGroups: string[];
  /** Files drawer: show dotfiles, its width and sort order (folders always come first). */
  filesShowHidden: boolean;
  filesWidth: number;
  filesSort: FilesSort;
  filesSortDesc: boolean;
  /** Local terminals: '' for the default shell, a shell's id, or 'custom' for `localShellCommand`. */
  localShell: string;
  /** The command line of the custom shell. */
  localShellCommand: string;
  /** Windows: "Open with NexSSH" in the context menu of folders in Explorer. */
  explorerMenu: boolean;
  /** Look for a new version at start-up and every few hours. */
  autoUpdateCheck: boolean;
  /** The version that last ran, to say "updated to …" once after an update. */
  lastVersion: string;
  /** The program remote files are edited in: '' for the first one found, an editor's id
   * (`api.editEditors`), 'system' for the file's default program, or 'custom'. */
  editor: string;
  /** The custom editor's command line; `{file}` stands for the file (else it goes last). */
  editorCommand: string;
  /** Programs for files with some extensions; other files open in `editor`. */
  editorAssociations: EditorAssociation[];
  /** What a double-click on a file in the files drawer does. */
  filesDoubleClick: 'download' | 'edit';
  /** AI agents may connect (MCP on 127.0.0.1:`agentsPort`). */
  agentsEnabled: boolean;
  agentsPort: number;
}

export interface AppInfo {
  version: string;
  os: string;
  dataDir: string;
  settings: Partial<Settings> | null;
  /** This build can update itself. */
  updates: boolean;
}

export type SftpEntryKind = 'dir' | 'file' | 'link' | 'other';

export interface SftpEntry {
  name: string;
  kind: SftpEntryKind;
  /** A symlink to a directory: opens like one. */
  linkToDir: boolean;
  size: number;
  /** Seconds since the epoch. */
  modified?: number;
  /** `rwxr-xr-x` */
  permissions?: string;
  /** The permission bits, e.g. `0o755`. */
  mode?: number;
}

export interface TransferProgress {
  done: number;
  total: number;
}

export interface Downloaded {
  path: string;
  name: string;
}

/** A local file or folder chosen for upload: picked in a dialog, or dropped (Linux). */
export interface PickedUpload {
  path: string;
  name: string;
  folder: boolean;
}

/** Files dropped on the window, passed on by the backend (Linux). */
export interface DroppedFiles {
  items: PickedUpload[];
  /** Where they were dropped, in CSS pixels. */
  x: number;
  y: number;
}

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes?: string;
}

/** What a terminal shows, as text (see `TermView.snapshot`). */
export interface TerminalSnapshot {
  /** The lines, without colours; a long line the terminal wrapped is one line again. */
  text: string;
  cols: number;
  rows: number;
  /** From 1, on the screen. */
  cursorRow: number;
  cursorCol: number;
  /** A full-screen program (vim, htop, less) has the screen. */
  alternate: boolean;
  /** Lines in the scrollback above the screen. */
  above: number;
  /** The first row given (from 1), and the last one with something on it: where the output
   * ends (0 when nothing has been printed). */
  from: number;
  total: number;
}

/** A terminal tab read for an agent; mirrors `Screen` in desktop/src/agents/mod.rs. */
export interface AgentScreen extends TerminalSnapshot {
  title: string;
  status: string;
  /** Which of the server's tabs (from 1), and how many it has. */
  tab: number;
  tabs: number;
}

/** The backend asks the page to read a terminal tab of a server for an agent. */
export interface AgentScreenRequest {
  id: number;
  serverId: string;
  /** From 1; the active tab of the server, else its last, when not given. */
  tab: number | null;
  /** The last lines, scrollback included, instead of the screen. */
  lines: number | null;
}

/** A remote file edited in a program of this computer; mirrors core/src/edit.rs. */
export type EditState = 'synced' | 'uploading' | 'conflict' | 'denied' | 'waiting' | 'failed';

export interface EditInfo {
  id: number;
  sessionId: number;
  remotePath: string;
  localPath: string;
  name: string;
  state: EditState;
  /** Unix time in milliseconds of the last save that reached the server. */
  savedAt?: number;
  /** Read and written with sudo. */
  sudo: boolean;
  /** `denied` because sudo wants a password. */
  needsPassword: boolean;
  error?: string;
}

/** Why a file could not be opened for editing. */
export type EditRefusal =
  | { kind: 'denied'; message: string }
  | { kind: 'password'; wrong: boolean; message?: string }
  | { kind: 'failed'; message: string };

export interface EditorInfo {
  id: string;
  name: string;
  program: string;
}

/** A rectangle in the window's pixels (physical, not CSS). */
export interface PixelRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** The maximize button, while Windows 11's snap layouts take the mouse over it
 * (desktop/src/snap.rs). */
export type SnapButtonState = 'hover' | 'press' | 'none';

/** A program for files with some extensions (`Settings.editorAssociations`); the backend
 * reads it in desktop/src/edit.rs. */
export interface EditorAssociation {
  /** Lowercase, without the dot: `['yml', 'yaml']`, `['tar.gz']`. */
  extensions: string[];
  /** Like `Settings.editor`, but never '': an editor's id, 'system' or 'custom'. */
  editor: string;
  /** For 'custom': the command line; `{file}` stands for the file (else it goes last). */
  command: string;
}

/** A question from the backend: may an agent do this? Mirrors desktop/src/agents/mod.rs. */
export interface AgentRequest {
  id: number;
  agent: string;
  serverId: string;
  serverName: string;
  tool: 'run_command' | 'write_file' | 'upload' | string;
  /** The command, or the path. */
  detail: string;
  /** A command's input, or the beginning of a file's new content. */
  preview?: string;
}

/** An agent needs a session of a server that has none: the page opens a tab. */
export interface AgentOpen {
  id: number;
  agent: string;
  serverId: string;
}

export type AgentActivityStatus = 'waiting' | 'running' | 'done' | 'failed' | 'denied';

export interface AgentActivity {
  id: number;
  /** Unix time in milliseconds. */
  at: number;
  agent: string;
  serverId: string;
  serverName: string;
  tool: string;
  detail: string;
  status: AgentActivityStatus;
  sessionId?: number;
  exitCode?: number;
  durationMs?: number;
  error?: string;
}

export interface AgentsStatus {
  enabled: boolean;
  running: boolean;
  port: number;
  url: string;
  error?: string;
  /** NexSSH's program, for agents' configurations (`NexSSH mcp`). */
  exe: string;
  /** The token is in the keychain (the bridge can read it). */
  tokenKept: boolean;
  agents: { name: string; lastSeen: number }[];
  requests: AgentRequest[];
  opens: AgentOpen[];
  /** Newest first. */
  activity: AgentActivity[];
}

export interface DownloadProgress {
  downloaded: number;
  total?: number;
}
