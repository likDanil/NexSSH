# NexSSH 2.0 — Slint preview

Experimental native client on branch `codex/2.0.0-slint`. This is the first migration
slice, version **2.0.0-alpha.1**, not a replacement for the 1.x release yet.
It renders with Slint/FemtoVG and uses `nexssh-core` directly, without Tauri or a WebView.

## Run

Requires Rust **1.95+** and the platform's native Rust build toolchain. On Windows,
install Visual Studio Build Tools with the C++ workload. Node.js is optional.

```sh
cargo run -p nexssh-slint
cargo test -p nexssh-slint
cargo build -p nexssh-slint --release
```

The matching npm shortcuts are `npm run native:dev`, `native:check`, `native:test`,
and `native:build`. The Windows binary is `target/release/nexssh-slint.exe`.
The existing `npm run dev` and `npm run build` still build the 1.x Tauri client.

The preview reads the same saved servers and credential store as 1.x. It does not
migrate the data format. SSH connections still apply the core's normal behavior,
including saved startup commands, automatic forwards and host-key decisions.
For a separate sandbox, set `NEXSSH_DATA_DIR` before running:

```powershell
$env:NEXSSH_DATA_DIR = Join-Path $env:TEMP 'nexssh-2-preview'
cargo run -p nexssh-slint
```

## Implemented

- 1.x window layout: 240px resizable sidebar, 40px custom title bar, compact tabs,
  collapsible server groups, rounded terminal surface and session status bar.
- Light, Graphite, Black, Navy and System themes; bundled JetBrains Mono.
- Server home screen, sidebar search, quick-connect palette and session actions menu.
- Add a server and import `~/.ssh/config`.
- All seven settings sections, with defaults and Russian/English messages generated from
  the original TypeScript sources. Preferences inherit `settings.json` key by key and
  save to `native-settings.json`; unknown preferences survive. Corrupt native settings
  are preserved before recovery.
- Fonts, themes, cursor style/blink, status bar, sidebar hiding, default/custom shell,
  paste policy, copy on selection, right-click behaviour and keyword colours.
- Settings retain the 1.x segmented choices, font stepper, theme previews, About,
  backup and agent layouts. Colour swatches open an HSV picker with Hex, RGB, HSL,
  presets and saved colours. The themed scrollbar has a dedicated gutter outside
  the controls; the hidden-sidebar button stays at the left edge of the title bar.
- CPU or GPU renderer (after restart); configurable history for new tabs.
- Quick SSH connection (`user@host:port`, including bracketed IPv6).
- Multiple local and SSH sessions, switching/closing tabs and reconnecting.
- Username, password, private-key passphrase, keyboard-interactive/2FA and host-key dialogs.
- Native terminal state via `vt100`: ANSI colors, truecolor, cursor positioning,
  alternate screen, Unicode cells and PTY resizing.
- 1,000–100,000 lines of scrollback: mouse wheel or Shift+PageUp / Shift+PageDown.
- Mouse selection, Unicode copying, literal search through history (Ctrl+Shift+F),
  paste (Ctrl+Shift+V), bracketed paste and configurable multiline confirmation.
- SFTP drawer: browse, sort, hidden files, upload/download files and folders,
  transfer cancellation; external editors and file associations use the existing
  core's save-back and conflict detection.
- AI-agent MCP server and stdio bridge (`nexssh-slint mcp`), with the 1.x protocol,
  keychain token, per-server access, request dialogs and activity. Disabling the
  server or changing its port also refuses existing HTTP connections.
- Compatible backup export/import, encrypted passwords, a summary and confirmation
  before replacement; About shows data/log locations and checks for 2.x releases.
- Native tray, close choices and saved tabs. Restored tabs wait for Enter before
  connecting. `--cwd <folder>` opens the selected local shell in that folder.
- Session cleanup on exit, including local shell processes.

## Migration plan / limitations

1. **Terminal compatibility:** real-world vim, htop, tmux and ConPTY coverage;
   mouse reporting, IME, regex/whole-word search, links, resizing/reflow and rendering performance.
   Selection is cleared when output changes or the viewport scrolls.
2. **Server management:** full edit/delete dialogs, group management and remaining
   contextual actions. Some native notices and hints still need full localization.
3. **Files and tools:** complete file context menus, permissions, sudo editing,
   forwarding controls and metrics. SSH/SFTP/editor interaction still needs live-server QA.
4. **Desktop integration:** window state, single-instance routing, drag-and-drop,
   signed updates and packaging. The preview checks for 2.x releases but does not
   install updates. Explorer uses a separate classic NexSSH 2.0 entry; debug builds
   leave the registry alone unless `NEXSSH_EXPLORER_MENU` is set.
5. **Measure:** compare release builds for startup time, idle CPU and memory with
   identical session counts before deciding whether to replace the existing client.

Neither full xterm.js compatibility nor a memory/performance improvement is claimed
by this prototype. SSH and platform-specific behavior need testing on all supported OSes.

## Structure

- `ui/app.slint`: native window layout, ported from the original Svelte components.
- `ui/controls.slint`: controls ported from the shared CSS.
- `build.rs`: generates Slint theme colors and SVG assets directly from `ui/src/app.css`
  and `ui/src/lib/components/Icon.svelte`, plus terminal palettes and theme previews
  from `ui/src/lib/themes.ts`. These 1.x files remain the source of truth.
- `build/settings.rs`: reads original defaults, translations and keyword patterns.
- `src/settings.rs`, `settings_schema.rs`: complete preference storage and UI rows.
- `src/services.rs`: file dialogs, backups, metadata and update checks off the UI thread.
- `src/files.rs`, `editor.rs`: SFTP and external editors; `src/agents/`: native MCP host.
  Editor and MCP implementations are ported from their `desktop/src` counterparts;
  their protocol, access policy and editor resolution tests are retained.
- `src/app.rs`: UI-thread controller, session lifecycle and event bridge.
- `src/terminal.rs`: bounded terminal parser, ANSI colors, input encoding and terminal replies.
- `src/prompts.rs`: SSH prompts and responses.
- `../core`: shared SSH, local PTY and storage implementation.

Layout measurements are ported from `App.svelte`, `Sidebar.svelte`, `TabBar.svelte`,
`ServerRow.svelte`, `Home.svelte`, `CommandPalette.svelte` and `Modal.svelte`, rather
than estimated from screenshots. Native text rasterization, shadows and window-system
integration can still differ from WebView rendering; full pixel parity is not yet claimed.

For UI inspection in development, Slint provides an optional local MCP server:

```powershell
$env:SLINT_EMIT_DEBUG_INFO = '1'
$env:SLINT_MCP_PORT = '9127'
cargo run -p nexssh-slint --features slint/mcp
```

Do not enable that feature in release builds. See the
[Slint testing documentation](https://docs.rs/i-slint-backend-testing/1.19.0/i_slint_backend_testing/).

The optional UI regression check starts and closes its own headless instances with
temporary data. It verifies settings controls, dropdown callbacks, language changes,
persistence after restart and preservation of the original settings file:

```sh
cargo build -p nexssh-slint --features slint/mcp
python native/tests/settings_ui.py
```
