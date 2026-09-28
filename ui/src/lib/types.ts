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
  jumpHost?: string;
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
  gpuAcceleration: boolean;
  /** Windows/Linux: plain Ctrl+K opens the palette even while typing in a terminal. */
  ctrlKInTerminal: boolean;
  sidebarWidth: number;
  sidebarHidden: boolean;
  collapsedGroups: string[];
}

export interface AppInfo {
  version: string;
  os: string;
  dataDir: string;
  settings: Partial<Settings> | null;
}
