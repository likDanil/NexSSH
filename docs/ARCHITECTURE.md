# NexSSH architecture

NexSSH is a desktop SSH client (with local terminals next to the SSH sessions) built from
three layers with one-way dependencies:

```
ui (Svelte + TypeScript + xterm.js)   ← renders, owns no SSH logic
        │  Tauri IPC: commands + one Channel per session
desktop (Tauri 2, Rust)                ← thin shell: window, IPC, settings.json
        │  plain Rust API
core (nexssh-core, Rust)               ← SSH, auth, known_hosts, forwarding, storage, local PTYs
```

`core` has no dependency on Tauri or any GUI toolkit. A CLI or TUI could reuse it as-is:
create a `Core`, open sessions with an `EventSink`, answer `Prompt`s.

## 1. Repository layout

```
NexSSH/
├── Cargo.toml            Rust workspace (core, desktop, explorer), release profile
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
│   │   ├── sftp.rs       files over SFTP: list, create, rename, delete, chmod, transfers
│   │   ├── local.rs      shells on this computer (PowerShell, cmd, WSL, Git Bash, /etc/shells)
│   │   ├── i18n.rs       user-facing messages in every language
│   │   └── session/
│   │       ├── mod.rs    SessionManager, events, prompts
│   │       ├── connect.rs TCP + jump host chains + timeouts
│   │       ├── auth.rs   agent, keys, passwords, keyboard-interactive
│   │       ├── handler.rs russh callbacks (host key, banner, remote forwards)
│   │       ├── shell.rs  session task: PTY, I/O, reconnect loop
│   │       └── local.rs  local terminal task: a program in a pseudo-terminal (ConPTY)
│   └── tests/            integration tests: real OpenSSH servers (sshd.rs, sftp.rs), real shells (local.rs)
├── desktop/              Tauri application ("nexssh" crate)
│   ├── src/lib.rs        builder, window creation
│   ├── src/commands.rs   IPC commands: servers, settings, sessions, window
│   ├── src/sftp.rs       IPC commands of the files drawer, transfers
│   ├── src/local.rs      IPC commands of local terminals, `--cwd <folder>` launches
│   ├── src/explorer.rs   "Open with NexSSH" in Windows Explorer's menu for folders
│   ├── src/links.rs      web links from the terminal, opened in the default browser
│   ├── src/updates.rs    in-app updates (check, download, install)
│   ├── src/sink.rs       EventSink → Tauri Channel
│   ├── src/settings.rs   settings.json (UI-owned schema)
│   ├── tauri.conf.json   bundle config (NSIS installer), updater key, CSP
│   ├── windows/hooks.nsh installer hooks: the uninstaller removes the Explorer menu entry
│   ├── build.rs          embeds the Explorer menu's package (NEXSSH_EXPLORER_PACKAGE)
│   └── capabilities/     window permissions
├── explorer/             "Open with NexSSH" in Windows 11's compact menu (Windows only)
│   ├── src/register.rs   registers the package for the user, removes it
│   ├── src/trust.rs      the computer's trust in the package's certificate (UAC once)
│   ├── command/          the menu entry's COM server (IExplorerCommand, a DLL)
│   ├── package/          the package's manifest (a package with external location) and logo
│   └── tests/package.rs  registers the package and asks its COM server (Windows CI)
├── ui/                   Svelte 5 frontend (Vite)
│   └── src/
│       ├── App.svelte    layout (Sidebar | Tabs + Terminal) and shortcuts
│       ├── app.css       design tokens and themes
│       └── lib/
│           ├── api.ts    the only module calling Tauri
│           ├── types.ts  TypeScript mirror of the Rust types
│           ├── terminal.ts xterm.js wrapper (lazy-loaded chunk), links, pastes
│           ├── actions.ts user actions shared by menus, palette, shortcuts
│           ├── popup.ts  lists that open under a control, rendered on <body>
│           ├── i18n.svelte.ts t(), plurals, language detection
│           ├── locales/  en.ts (reference), ru.ts
│           ├── state/    app, servers, sessions, shells, files, updates, toasts (Svelte runes)
│           └── components/
├── scripts/              test-sshd.sh, icons.py, explorer-package.ps1 (packs and signs the package)
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
| `sftp` | An SFTP client on a channel of the session's connection, opened on first use (`SessionManager::sftp`) and dropped with the connection. Downloads never overwrite: the local name is reserved atomically (`name (1).ext`…), and a failed or cancelled download removes what it wrote. Uploads of local files and folders (`upload_path`) merge into existing folders; the file being written when an upload fails or is cancelled is removed. A recursive `chmod` also gives folders `x` wherever they get `r` (like `chmod -R a+X`), so `644` leaves them openable. Symlinks inside folders are not followed into (they may loop). |
| `local` | Shells for local terminals, found per system: PowerShell 7 (Program Files, else `PATH`), Windows PowerShell, `%ComSpec%`, WSL distributions (from the registry, `HKCU\…\Lxss`; Docker's are skipped), Git Bash (registry `GitForWindows`, then the usual folders); elsewhere the login shell and `/etc/shells`, one entry per real file. The first found is the default. `LocalCommand` is what a local session runs: a shell in a folder, or a command line from the settings (words split at spaces, quotes group, backslashes stay: Windows paths need no escaping). |
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

### Local terminals

A local terminal is a session like an SSH one: `SessionManager::open_local` registers it under
the same ids, it takes the same commands (write, resize, reconnect, disconnect, close) and
reports the same events through the same `EventSink`, so the UI treats both alike. Reconnect
starts the program again in the same tab; disconnect stops it.

* The program runs in a pseudo-terminal from **portable-pty** (ConPTY on Windows). Its API is
  blocking, so a running program has three threads: one reads the output (into a bounded
  queue: a flood of output waits in the terminal, not in memory), one writes the input, one
  waits for the exit. Output is coalesced like an SSH session's.
* The end: the program exited *and* its output ran out. ConPTY keeps the last output until it
  is closed, so the terminal is closed right when the process exits and the output is read to
  its end (2 s at most).
* Closing a tab sends SIGHUP (Windows: terminates the process) and closes the terminal; a
  program still running a second later gets SIGKILL. Input goes through a plain duplicate of
  the terminal's descriptor: portable-pty's own writer sends a newline and end-of-file when
  dropped, which would run whatever was typed at the prompt.
* Programs learn where they run: `TERM=xterm-256color`, `COLORTERM=truecolor`,
  `TERM_PROGRAM=NexSSH`, `TERM_PROGRAM_VERSION` (and `WSLENV` passes them into WSL). WSL is
  taken to the folder with `--cd` (it translates `C:\…` itself; its Linux home otherwise), Git
  Bash gets `CHERE_INVOKING=1` so its login profile keeps the folder, macOS shells run as login
  shells (apps started from the Finder get a minimal `PATH`).
* On Windows the terminal is told it runs on ConPTY (`windowsPty` with the build number), so
  xterm.js adapts its resize and reflow to it.

**Open with NexSSH** (Windows) runs `"…\NexSSH.exe" --cwd <folder>` on a folder, on the empty
space inside one and on a drive. The app sets it up for the current user at start-up and
whenever the setting or the language changes (the label follows the interface language, the
command points at the running copy), and takes it all away when the setting is off; the
uninstaller runs `NexSSH --explorer-cleanup` for that (which also takes back the computer's
trust in the package's certificate, asking for administrator rights once), and removes the
classic entries itself too (`windows/hooks.nsh`).

* Classic entries: the verb `NexSSH` under `HKCU\Software\Classes\Directory\shell`,
  `…\Directory\Background\shell` and `…\Drive\shell` with the command `--cwd "%V"`. A drive's
  root arrives as `C:"` (`"%V"` makes it `"C:\"`, and the backslash escapes the quote); a quote
  cannot be part of a Windows path, so it is read back as `C:\`. On Windows 10 these are the
  entries; Windows 11 shows them only under *Show more options*.
* Windows 11's compact menu lists only commands that packaged apps declare, so NexSSH brings a
  *package with external location* (like VS Code's *Open with Code*): a manifest
  (`explorer/package`) declaring the menu entry for folders and their background and a COM
  server for it, `explorer/command` — an `IExplorerCommand` in a DLL that Explorer loads into a
  COM surrogate. `scripts/explorer-package.ps1` packs the manifest and signs it (the release
  workflow runs it), and `desktop/build.rs` embeds the package, the certificate (without its
  key) and the DLL.
* Windows registers the package only when it trusts the signature. The project has no
  certificate from a public authority (VS Code's package is signed by Microsoft), and its own
  counts only in the computer's *Trusted People* store: neither the user's store nor an
  unsigned package works (Windows refuses unsigned packages that declare an application). Adding
  a certificate there takes administrator rights, so on Windows 11 the settings offer *Move to
  the compact menu*: NexSSH starts itself elevated (`--explorer-trust`, Windows asks the user)
  to trust the certificate, which releases sign with the same key every time
  (`EXPLORER_SIGNING_CERT`). Until then, and when the user says no, the classic entries stay.
* Then, at start-up, the app unpacks the files into `%LOCALAPPDATA%\NexSSH\explorer\<build>`
  (a folder per build, so a DLL that Explorer still holds is never overwritten), registers the
  package for the user with the `PackageManager` API (external location: that folder), without
  administrator rights, and drops the classic entries for folders, which would show up twice
  under *Show more options* (packages cannot add commands to drives, so the drive entry stays).
  Nothing is done again while the registered package is the running build's; another build's
  package and folder are replaced.
* The COM server reads the entry's title and NexSSH's path from `HKCU\Software\NexSSH\ExplorerMenu`
  every time the menu opens (hidden without them, so turning the setting off hides it at
  once). It asks Explorer to start NexSSH, through the desktop's `IShellDispatch2::ShellExecute`:
  a process that the COM server started itself would inherit the package's identity and run in
  its container, where shares like `\\wsl.localhost` are out of reach (and so would the shells
  of its terminals), and Windows would end it when the package is replaced. Only when Explorer
  cannot does it start NexSSH itself; such a NexSSH leaves the package alone
  (`register::runs_in_package`).
* The classic entries are used instead when the user brought back Windows 10's menu with the
  well-known registry tweak, when a build has no package (development builds) and when
  registering it fails; the settings show why then.

A second start hands its `--cwd` over to the running window (single-instance plugin), and the
page opens a local terminal there.

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
├── Sidebar          logo, search, Servers / Quick Connect / Local terminal / Settings, groups,
│                    + Add Server, import
│   └── ServerRow    status dot, name, host
│   └── UpdateCard   new version: Download → progress → Install
├── TabBar           tabs (status dot, or a terminal glyph for local terminals; title, close),
│                    + and its menu (a connection or any local shell), files, ⋯, WindowControls
├── TerminalPane ×N  one xterm per tab, kept alive while hidden (display: none)
│   └── PromptCard   host key / password / passphrase / keyboard-interactive, over the terminal
├── FilesDrawer      SFTP files of the active tab, right of the terminal (which narrows):
│                    sortable list with multi-selection, drops, transfers with speed
├── PermissionsDialog chmod: checkboxes and the octal value, optionally recursive
├── Home             welcome screen or server overview when no tab is active
├── CommandPalette   Ctrl/Cmd+K: servers, tabs, actions, quick connect, local shells
├── ServerEditor     add/edit server (auth, key picker, jump host with its login, forwards)
├── SettingsDialog   themes, font, terminal (and the local terminal's shell, Explorer menu),
│                    keyboard, about
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
  and nested jump hosts are resolved recursively (with loop detection). A saved server signs
  in with its own settings.
* `jumpUser`: login on a jump host typed as an address (a single one, not a chain); its
  password, if remembered, is in the keychain as `jump-password:<id>` of the server that
  uses it. Without them the login comes from `user@` in `jumpHost` (else the local user name)
  and the password is asked on connect.
* `alias`: the `Host` alias when imported from `~/.ssh/config`; re-importing updates
  connection fields but keeps the name, group and history.

`settings.json` belongs to the UI; for local terminals it keeps `localShell` (a shell's id, `''`
for the default, or `custom`), `localShellCommand` (the custom command line) and `explorerMenu`.

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
* **File transfers:** downloads run in the backend straight into the Downloads folder (or a
  folder picked with *Download to…*), with progress on a `Channel` (at most every 100 ms).
  Uploads take two routes:
  * files from the webview (the file dialog, and drops on WebView2 / WKWebView, where
    `webkitGetAsEntry` walks dropped folders) arrive as `File` objects: `sftp_ensure_dir` for
    their folders, then per file `sftp_upload_begin`, 1 MiB pieces as raw IPC bodies
    (`ArrayBuffer`, no JSON or base64) with the transfer id in a header, and
    `sftp_upload_end`;
  * a folder picked in the native dialog (*Upload folder…*: the webview's folder picking
    differs per system), and on Linux whatever is dropped (WebKitGTK shows the page such a drop
    without its files, see below), is read from disk by the backend: `sftp_upload_path`.

  A transfer id chosen by the UI lets `sftp_cancel` stop any of them; a cancelled upload
  deletes its partial file. The UI measures the speed from the progress (over ½ s, smoothed)
  and shows the time left.
* **Dropping files:** the page handles drag & drop itself (tab reordering needs it, and the
  native handler would take every drag from WebView2). WebKitGTK never gives the page the
  files of a drop from outside, so on Linux the native handler stays enabled: it reports the
  paths being dragged in (`files-dragged`), and a drop on the files drawer uploads them. A
  file dropped anywhere else is refused instead of opening in the webview.
* **Start-up:** the window is created in Rust with the saved theme's background colour;
  an inline script applies the last theme before first paint; `xterm.js` is a lazy chunk
  loaded with the first session.

## 6. Why these libraries

| Need | Choice | Why |
| --- | --- | --- |
| SSH | **russh** (ring backend) | Pure Rust, async (Tokio, like Tauri), actively maintained, used in production (e.g. Warpgate). No OpenSSL/libssh2 C toolchain, trivial Windows builds. Supports agent (incl. Pageant and Windows named pipes), certificates, keyboard-interactive, direct-tcpip, remote forwarding. `ring` instead of the default `aws-lc-rs` avoids CMake/NASM on Windows. |
| SFTP | **russh-sftp** | The SFTP client of the russh ecosystem: runs on a channel of the existing connection, pipelines reads and writes (fast on high-latency links). |
| Local PTY | **portable-pty** | WezTerm's pseudo-terminals: ConPTY on Windows (Windows 10 1809+), `openpty` elsewhere; years of use in WezTerm. Writing ConPTY by hand is easy to get subtly wrong (cursor inheritance, draining on close). |
| Updates | **tauri-plugin-updater** | Minisign-verified updates from a static `latest.json`; runs the NSIS installer silently. Built with rustls on `ring` (no OpenSSL, no second crypto backend). |
| Keychain | **keyring-core** + native stores | Windows Credential Manager, macOS Keychain, Secret Service. Linking the stores directly (instead of `keyring`'s all-in-one feature) avoids the zbus async stack. |
| GUI shell | **Tauri 2** | System webview (WebView2 / WKWebView / WebKitGTK): a few MB installer, no bundled Chromium, far lower RAM than Electron. |
| UI | **Svelte 5** | Compiles to small vanilla JS; runes give fine-grained reactivity without a virtual DOM. |
| Terminal | **xterm.js 6** + fit + WebGL + web links | The de-facto web terminal (VS Code). WebGL renderer cuts CPU on heavy output; DOM renderer is the fallback. The web-links addon finds addresses in the output; hyperlinks (OSC 8) are xterm's own. |
| Storage | JSON file | See "Decisions" below. |
| Font | JetBrains Mono (bundled, OFL) | Consistent metrics on every OS; ~100 KB, subsets loaded on demand. |

Deliberately **not** used: icon libraries (hand-drawn inline SVG), UI kits, state libraries,
fuzzy-search libraries (30 lines in `fuzzy.ts`), clipboard/shell plugins (`navigator.clipboard`
with Tauri's clipboard access), logging frameworks. The dialog plugin is used only from Rust,
for native pickers: a key file (a webview file input gives no path), a folder to download
into, a folder to upload.

## 7. Credentials and security

* Passwords and key passphrases are stored **only** in the OS credential store under the
  service `NexSSH` (Windows: target `NexSSH password:<id>`, or `NexSSH jump-password:<id>` for
  a jump host typed as an address; persistence *Local* so they do not roam). NexSSH's files
  never contain secrets.
* If no credential store is usable, the app still works: "remember" is disabled and the
  user is asked on connect.
* Secrets typed during a session stay in memory for reconnects (zeroized on drop);
  private key file contents are zeroized too.
* Host keys: NexSSH writes only its own `known_hosts`; the user's `~/.ssh/known_hosts` is
  read-only. A changed key shows a warning with both fingerprints; cancel is the default.
* Local terminals run programs as the user, like any terminal: the page can start the shells
  found on the computer or the command line of the settings, in a folder it names. The
  Explorer entry is set up for the current user only (HKCU, a package registered for the user).
* Windows 11's menu entry is a package registered for the user and signed with NexSSH's own
  certificate. Only after the user allowed it (UAC) does the computer trust that certificate,
  in *Trusted People*: it then accepts packages signed with its key, which exists only as a
  GitHub Actions secret, like the updater's. The uninstaller takes the trust back (asking for
  administrator rights again). The COM server the package names is the DLL unpacked from the
  running NexSSH into the user's `%LOCALAPPDATA%`.
* The webview runs with a strict CSP (no remote content, no `eval`), and only the window
  permissions needed for the custom title bar are granted. The updater and dialog plugins'
  own commands are not granted either: the UI can only use NexSSH's commands.
* Updates run only if their minisign signature matches the public key built into the app
  (`plugins.updater.pubkey`); the private key exists only as a GitHub Actions secret.
* Links in the terminal open only with Ctrl+click (Cmd+click), and only web ones: the page
  hands the address to `open_link`, which accepts nothing but an `http` or `https` URL with a
  host (no `file:`, no other programs' protocol handlers) and gives the default browser the
  URL as the parser writes it back (ShellExecuteEx on Windows, `xdg-open`, `open`), never
  through a shell. xterm.js itself leaves hyperlinks (OSC 8) to other schemes alone, and a
  hyperlink's hint shows where it leads, which its text need not say.
* Pastes into a terminal leave out control characters but tab and line breaks, as in Windows
  Terminal: an escape sequence from the clipboard could otherwise end a bracketed paste early
  and run the rest, and keys like Ctrl+O run a line without a line break. Text with line breaks
  is shown before it goes in, unless the program takes pastes as text (bracketed paste); the
  settings can make NexSSH always or never ask. Pastes xterm would take by itself (Cmd+V, a
  middle click) go the same way.
* "Show in folder" opens only files this run of the app downloaded itself. The same goes for
  local paths the page hands back: a download goes only to Downloads or a folder the user
  picked in the native dialog, and an upload by path reads only what the user picked or
  dragged in. A compromised page could not make NexSSH write or send other local files.

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
  setting makes it work inside too. macOS uses `⌘`. A key that types a non-Latin character
  (a Cyrillic layout) counts as the US key in its place (`KeyboardEvent.code`), so shortcuts
  do not depend on the layout. The local terminal is `` Ctrl+Shift+` `` everywhere, like VS Code.
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

* **Snippets / history / sync:** new modules in `core` behind their own stores.
* **Split panes:** several `TerminalPane`s per tab; sessions are already independent.
* **Themes:** a theme is a block of CSS variables in `app.css` plus an xterm palette in
  `themes.ts`.
