// Mirrors of the Rust types (serde, camelCase). Keep in sync with core/src/model.rs,
// core/src/session/mod.rs and desktop/src/commands.rs.

import type { LanguageSetting } from './i18n.svelte';

export type AuthKind = 'auto' | 'password' | 'key' | 'agent';
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

export interface DownloadProgress {
  downloaded: number;
  total?: number;
}
