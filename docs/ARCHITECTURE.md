# NexSSH architecture

NexSSH is a desktop SSH client built from three layers with one-way dependencies:

```
ui (Svelte + TypeScript + xterm.js)   ← renders, owns no SSH logic
        │  Tauri IPC: commands + one Channel per session
desktop (Tauri 2, Rust)                ← thin shell: window, IPC, settings.json
        │  plain Rust API
core (nexssh-core, Rust)               ← SSH, auth, known_hosts, forwarding, storage
```

`core` has no dependency on Tauri or any GUI toolkit. A CLI or TUI could reuse it as-is:
create a `Core`, open sessions with an `EventSink`, answer `Prompt`s.

## 1. Repository layout

```
NexSSH/
├── Cargo.toml            Rust workspace (core, desktop), release profile
├── package.json          npm workspace root: Tauri CLI + scripts
├── core/                 nexssh-core
│   ├── src/
│   │   ├── lib.rs        Core facade (store + secrets + known_hosts + sessions)
│   │   ├── model.rs      Server, AuthKind, ForwardSpec, Destination, PtySize
│   │   ├── store.rs      servers.json (atomic writes, import merge)
│   │   ├── secrets.rs    OS keychain via keyring-core
│   │   ├── known_hosts.rs OpenSSH-compatible host key checks
│   │   ├── ssh_config.rs ~/.ssh/config import
│   │   ├── keys.rs       private key discovery/loading (OpenSSH, PEM, PKCS#8, PPK)
│   │   ├── forward.rs    -L / -R / -D (SOCKS5) forwarding
│   │   ├── sftp.rs       files over SFTP: list, mkdir, rename, delete, upload, download
│   │   ├── i18n.rs       user-facing messages in every language
│   │   └── session/
│   │       ├── mod.rs    SessionManager, events, prompts
│   │       ├── connect.rs TCP + jump host chains + timeouts
│   │       ├── auth.rs   agent, keys, passwords, keyboard-interactive
│   │       ├── handler.rs russh callbacks (host key, banner, remote forwards)
│   │       └── shell.rs  session task: PTY, I/O, reconnect loop
│   └── tests/            integration tests against a real OpenSSH server (sshd.rs, sftp.rs)
├── desktop/              Tauri application ("nexssh" crate)
│   ├── src/lib.rs        builder, window creation
│   ├── src/commands.rs   IPC commands: servers, settings, sessions, window
│   ├── src/sftp.rs       IPC commands of the files drawer, transfers
│   ├── src/updates.rs    in-app updates (check, download, install)
│   ├── src/sink.rs       EventSink → Tauri Channel
│   ├── src/settings.rs   settings.json (UI-owned schema)
│   ├── tauri.conf.json   bundle config (NSIS installer), updater key, CSP
│   └── capabilities/     window permissions
├── ui/                   Svelte 5 frontend (Vite)
│   └── src/
│       ├── App.svelte    layout (Sidebar | Tabs + Terminal) and shortcuts
│       ├── app.css       design tokens and themes
│       └── lib/
│           ├── api.ts    the only module calling Tauri
│           ├── types.ts  TypeScript mirror of the Rust types
│           ├── terminal.ts xterm.js wrapper (lazy-loaded chunk)
│           ├── actions.ts user actions shared by menus, palette, shortcuts
│           ├── popup.ts  lists that open under a control, rendered on <body>
│           ├── i18n.svelte.ts t(), plurals, language detection
│           ├── locales/  en.ts (reference), ru.ts
│           ├── state/    app, servers, sessions, files, updates, toasts (Svelte runes)
│           └── components/
├── scripts/              test-sshd.sh, icons.py
└── .github/workflows/    ci.yml, release.yml
```

## 2. Rust modules (core)

| Module | Responsibility |
| --- | --- |
| `model` | Serializable data model shared with the UI (camelCase JSON). Validation (`Server::normalize`), destination parsing (`user@host:port`, IPv6). |
| `store` | `servers.json` + group order. Writes go to a temp file and are renamed over the original; a corrupted file is moved aside, never overwritten. |
| `secrets` | Passwords and passphrases in the OS credential store. Blocking calls run on the blocking pool. |
| `known_hosts` | Reads NexSSH's own `known_hosts` and `~/.ssh/known_hosts` (read-only). Hashed hosts, wildcards, `@revoked`. Unparseable lines are skipped. |
| `ssh_config` | Parser following OpenSSH rules: `Include` (globs), `Host` patterns with negation, first value wins, `%h` `%n` `%p` `%r` `%u` `%d` tokens. |
| `keys` | Finds keys in `~/.ssh`, reads public halves without the passphrase (OpenSSH format or `.pub`). |
| `session` | One Tokio task per session. See below. |
| `forward` | Local listeners (`-L`, SOCKS5 `-D`) and server-side listeners (`-R`) on an authenticated connection. |
| `sftp` | An SFTP client on a channel of the session's connection, opened on first use (`SessionManager::sftp`) and dropped with the connection. Downloads never overwrite: the local name is reserved atomically (`name (1).ext`…), and a failed or cancelled download removes what it wrote. |
| `i18n` | Every user-facing message with all its translations; process-wide language set by the app. |

### Session lifecycle

```
            ┌────────────── Reconnect ◄──────────────┐
            ▼                                         │
open ─► Connecting ─► (prompts) ─► Connected ─► Disconnected ─► Close ─► Closed
            │             ▲                        ▲
            └─ Disconnect ┘ (abort)                └─ shell exit / network loss / Disconnect
```

* The session task survives disconnects, so reconnecting keeps the tab, the scrollback
  and secrets typed during the session (kept in memory, zeroized on drop).
* Output is coalesced: a lone chunk (keystroke echo) is delivered immediately, bursts
  are batched every 8 ms or 64 KB, which keeps IPC traffic and CPU low under `cat bigfile`.
* Writes go through a separate task so a full remote window (a huge paste) can never
  block reading output.
* Timeouts (`connect_timeout`) apply to DNS, TCP, handshake and authentication, but time
  spent waiting for the user (host key, password, 2FA) does not count.

### Authentication order

`Auto` behaves like the `ssh` command:

1. a password remembered for this server (so unrelated keys don't use up `MaxAuthTries`);
2. SSH agent identities (OpenSSH agent / `SSH_AUTH_SOCK` on Unix; the Windows OpenSSH
   agent pipe, `SSH_AUTH_SOCK` pipes, or Pageant on Windows);
3. the configured key file, then `~/.ssh/id_ed25519`, `id_ecdsa`, `id_rsa`;
4. keyboard-interactive (a single hidden "password" prompt uses the password dialog,
   anything else — OTP codes — is shown verbatim), then password.

Encrypted keys are offered by their public half first; the passphrase is asked only if the
server accepts the key (like OpenSSH). If the user cancels, an invalid signature is sent so
the server rejects that attempt and authentication continues with the next method.

## 3. Svelte components

```
App.svelte
├── Sidebar          logo, search, Servers / Quick Connect / Settings, groups, + Add Server, import
│   └── ServerRow    status dot, name, host
│   └── UpdateCard   new version: Download → progress → Install
├── TabBar           tabs (status, title, close), +, files, ⋯ session menu, WindowControls (Windows)
├── TerminalPane ×N  one xterm per tab, kept alive while hidden (display: none)
│   └── PromptCard   host key / password / passphrase / keyboard-interactive, over the terminal
├── FilesDrawer      SFTP files of the active tab, right of the terminal (which narrows), transfers
├── Home             welcome screen or server overview when no tab is active
├── CommandPalette   Ctrl/Cmd+K: servers, tabs, actions, quick connect
├── ServerEditor     add/edit server (auth, key picker, jump host, forwards)
├── SettingsDialog   themes, font, terminal, keyboard, about
├── ForwardsDialog   active forwards of the session, add -L / -R / SOCKS
├── ConfirmDialog, ContextMenu, Toasts
├── Select, Suggest  drop-down list / text field with suggestions in the app's style: the
│                  popups of a native <select> or <datalist> cannot be themed
└── Rich           a translated message with styled placeholders (<b>, <code>, <kbd>)
```

State lives in small rune-based stores (`lib/state/*.svelte.ts`). xterm instances are
kept outside reactive state; backend output is written straight into them.

## 4. Data model

`servers.json` (no secrets):

```json
{
  "version": 1,
  "groups": ["Production", "Development"],
  "servers": [
    {
      "id": "0fb2a40f8144a9da",
      "name": "web-01",
      "host": "192.168.1.10",
      "port": 22,
      "user": "deploy",
      "group": "Production",
      "auth": "auto",
      "identityFile": "~/.ssh/id_ed25519",
      "jumpHost": "bastion",
      "keepaliveSecs": 30,
      "connectTimeoutSecs": 15,
      "forwards": [{ "kind": "local", "bindHost": "127.0.0.1", "bindPort": 5433, "targetHost": "db", "targetPort": 5432 }],
      "alias": "web-01",
      "lastUsed": 1790566803
    }
  ]
}
```

* `auth`: `auto | password | key | agent`.
* `jumpHost`: id or name of a saved server, or `[user@]host[:port]`; comma-separated chains
  and nested jump hosts are resolved recursively (with loop detection).
* `alias`: the `Host` alias when imported from `~/.ssh/config`; re-importing updates
  connection fields but keeps the name, group and history.

A session (runtime only) is identified by a numeric id and reports:

```ts
type SessionEvent =
  | { type: 'status'; status: 'connecting' | 'connected' | 'disconnected' | 'closed'; message?: string; failed: boolean }
  | { type: 'log'; level: 'info' | 'warn' | 'error'; message: string }
  | { type: 'prompt'; id: number; prompt: Prompt }
  | { type: 'promptClosed'; id: number }
  | { type: 'forwards'; forwards: ForwardInfo[] };
// plus raw terminal output (ArrayBuffer)
```

## 5. Rust ↔ Tauri ↔ Svelte

* **Commands** (`desktop/src/commands.rs`) cover servers, settings, sessions, prompts and
  forwards. `ui/src/lib/api.ts` is their typed mirror; no other UI module calls Tauri.
* **Streaming:** `session_open` receives a `Channel`. Terminal output is sent as
  `InvokeResponseBody::Raw` — an `ArrayBuffer` in JavaScript, no JSON encoding — and
  written into xterm as `Uint8Array` (xterm decodes UTF-8 across chunk boundaries).
  Status events and prompts are small JSON messages on the same channel, so ordering
  between output and status is preserved.
* **Input:** `session_write` per keystroke through a per-tab queue: one request in flight,
  later keystrokes and pastes are batched, order is guaranteed.
* **Prompts:** the core emits `Prompt { id, … }` and awaits a oneshot; the UI answers with
  `prompt_answer(id, reply)`. Abandoned prompts emit `promptClosed`.
* **File transfers:** downloads run in the backend straight into the Downloads folder, with
  progress on a `Channel` (at most every 100 ms). Uploads come from the webview's `File`
  objects: `sftp_upload_begin`, then 1 MiB pieces as raw IPC bodies (`ArrayBuffer`, no JSON or
  base64) with the transfer id in a header, then `sftp_upload_end`. A transfer id chosen by the
  UI lets `sftp_cancel` stop either kind; a cancelled upload deletes its partial file.
* **Start-up:** the window is created in Rust with the saved theme's background colour;
  an inline script applies the last theme before first paint; `xterm.js` is a lazy chunk
  loaded with the first session.

## 6. Why these libraries

| Need | Choice | Why |
| --- | --- | --- |
| SSH | **russh** (ring backend) | Pure Rust, async (Tokio, like Tauri), actively maintained, used in production (e.g. Warpgate). No OpenSSL/libssh2 C toolchain, trivial Windows builds. Supports agent (incl. Pageant and Windows named pipes), certificates, keyboard-interactive, direct-tcpip, remote forwarding. `ring` instead of the default `aws-lc-rs` avoids CMake/NASM on Windows. |
| SFTP | **russh-sftp** | The SFTP client of the russh ecosystem: runs on a channel of the existing connection, pipelines reads and writes (fast on high-latency links). |
| Updates | **tauri-plugin-updater** | Minisign-verified updates from a static `latest.json`; runs the NSIS installer silently. Built with rustls on `ring` (no OpenSSL, no second crypto backend). |
| Keychain | **keyring-core** + native stores | Windows Credential Manager, macOS Keychain, Secret Service. Linking the stores directly (instead of `keyring`'s all-in-one feature) avoids the zbus async stack. |
| GUI shell | **Tauri 2** | System webview (WebView2 / WKWebView / WebKitGTK): a few MB installer, no bundled Chromium, far lower RAM than Electron. |
| UI | **Svelte 5** | Compiles to small vanilla JS; runes give fine-grained reactivity without a virtual DOM. |
| Terminal | **xterm.js 6** + fit + WebGL | The de-facto web terminal (VS Code). WebGL renderer cuts CPU on heavy output; DOM renderer is the fallback. |
| Storage | JSON file | See "Decisions" below. |
| Font | JetBrains Mono (bundled, OFL) | Consistent metrics on every OS; ~100 KB, subsets loaded on demand. |

Deliberately **not** used: icon libraries (hand-drawn inline SVG), UI kits, state libraries,
fuzzy-search libraries (30 lines in `fuzzy.ts`), clipboard/shell plugins (`navigator.clipboard`
with Tauri's clipboard access), logging frameworks. The dialog plugin is used only from Rust,
for the native file picker that chooses a key file (a webview file input gives no path).

## 7. Credentials and security

* Passwords and key passphrases are stored **only** in the OS credential store under the
  service `NexSSH` (Windows: target `NexSSH password:<id>`, persistence *Local* so they do
  not roam). NexSSH's files never contain secrets.
* If no credential store is usable, the app still works: "remember" is disabled and the
  user is asked on connect.
* Secrets typed during a session stay in memory for reconnects (zeroized on drop);
  private key file contents are zeroized too.
* Host keys: NexSSH writes only its own `known_hosts`; the user's `~/.ssh/known_hosts` is
  read-only. A changed key shows a warning with both fingerprints; cancel is the default.
* The webview runs with a strict CSP (no remote content, no `eval`), and only the window
  permissions needed for the custom title bar are granted. The updater and dialog plugins'
  own commands are not granted either: the UI can only use NexSSH's commands.
* Updates run only if their minisign signature matches the public key built into the app
  (`plugins.updater.pubkey`); the private key exists only as a GitHub Actions secret.
* "Show in folder" opens only files this run of the app downloaded itself.

## Languages

The interface is available in English and Russian; the default follows the system
(`navigator.languages`, i.e. the Windows display language in WebView2).

* **UI texts** live in `ui/src/lib/locales/*.ts`. English is the reference; a catalog is typed
  as `Catalog`, so a missing or misspelled key fails `svelte-check`. `t()` reads reactive
  state: switching the language re-renders the open window instantly, no reload.
  Plurals use `Intl.PluralRules`, dates and numbers `Intl` formatters — no i18n library.
* **Backend texts** (connection progress in the terminal, errors, prompt hints) are produced in
  Rust by `core::i18n`. The UI tells the backend which language to use (`app_set_language`)
  at start-up and on every change. Keeping them in the core means a CLI would get them too.
* The UI never parses backend texts: facts it needs travel as data (e.g. `failed` on a
  `disconnected` status tells a clean exit from a lost connection).

## Decisions worth knowing

* **JSON instead of SQLite for the server list.** The list is small and read once at
  start-up; `serde_json` is already in the dependency tree, so this costs nothing, keeps
  the file human-readable and diff-friendly (dotfiles, backups). All access goes through
  `ServerStore`, so moving to SQLite later (history, snippets, sync) is a local change.
* **Session = task, not connection.** Reconnect keeps the terminal and cached secrets.
* **Keyboard shortcuts:** on Windows/Linux app shortcuts use `Ctrl+Shift+…` because
  `Ctrl+K`, `Ctrl+W`, `Ctrl+R`, `Ctrl+B` all mean something in a shell (kill line, delete
  word, reverse search, tmux). `Ctrl+K` still opens the palette outside the terminal, and a
  setting makes it work inside too. macOS uses `⌘`.
* **Custom title bar on Windows** (tabs live in the title bar, like Windows Terminal),
  overlay title bar on macOS, native decorations on Linux.
* **Full screen goes through Rust** (`window_set_fullscreen`). On Windows an undecorated
  window that is maximized keeps the maximized client area (the work area, taskbar
  excluded) even in full screen, which left a strip at the bottom. The command un-maximizes
  before entering full screen and maximizes again on leaving it.
* **Updates are Windows-only** (the only published installer), in two explicit steps:
  *Download* in the background, then *Install*, which closes the sessions, runs the installer
  silently (`installMode: quiet`: no windows, no UAC prompt since it installs per user) and
  restarts NexSSH. A check runs 5 s after start-up and every 6 hours (can be turned off).
* **One running copy.** Starting NexSSH again (say, the desktop shortcut clicked twice) brings
  the running window to the front, restored if minimized, instead of opening a second copy;
  two copies would also overwrite each other's `servers.json` and `settings.json`. The
  single-instance plugin (a named mutex on Windows, D-Bus on Linux) is registered first, so
  the second process exits before it creates anything.
* **One SFTP channel per connection,** opened lazily: sessions that never open the files
  drawer cost nothing, and there is no second login or prompt.

## Extending

* **Folder uploads:** walk dropped folders (`webkitGetAsEntry`) and reuse the upload commands.
* **Snippets / history / sync:** new modules in `core` behind their own stores.
* **Split panes:** several `TerminalPane`s per tab; sessions are already independent.
* **Themes:** a theme is a block of CSS variables in `app.css` plus an xterm palette in
  `themes.ts`.
