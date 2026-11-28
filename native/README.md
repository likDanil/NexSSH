# NexSSH 2.0 — Slint preview

Experimental native client on branch `codex/2.0.0-slint`. This is the first migration
slice, version **2.0.0-alpha.2** (local development), not a replacement for the 1.x release yet.
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
- The terminal font picker enumerates installed monospaced families in the
  background (system and user font folders), with the bundled font always available.
  Legacy names/CSS stacks are preserved; unavailable or proportional fonts render
  with the bundled fallback. Preferences survive restart and remain 1.x-compatible.
- Batched terminal text uses explicit cell bounds and calibrated regular/bold
  advances instead of rounded single-letter measurements. Letter spacing and the
  physical-pixel grid keep colour/style fragments aligned, including fractional DPI;
  Unicode graphemes and wide cells retain their parser positions.
- Segoe UI Variable with separate native weights for body text, names/buttons and
  headings, adjusted to match the visual emphasis of 1.x. Terminal font weights stay independent.
- Windows 11 system-rounded outer corners and a shadow in restored mode; Windows
  keeps maximized and full-screen corners square.
- Server home screen, sidebar search, quick-connect palette and session actions menu.
- Add/edit servers in the six-tab 1.x editor: general fields, sign-in and key picker,
  jump hosts, timeouts, local/remote/SOCKS forwards, login commands and agent access.
  Passwords can be kept, replaced, revealed or forgotten in the OS credential store.
  Server context menus connect, open another session, edit, duplicate, copy the
  address and confirm deletion. Import from `~/.ssh/config` is also available.
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
- Copy actions show a dismissible 1.x-style notification for 3.6 seconds. Repeated
  copies restart its timer; terminal text and tokens use a generic confirmation.
- Shortcuts work in non-Latin layouts through physical key positions, following
  the 1.x keyboard helper. ASCII layouts keep their logical bindings; ordinary
  typing and AltGr retain native handling. Home also has a keyboard focus target.
- SFTP drawer follows `FilesDrawer.svelte` and `ContextMenu.svelte`: compact
  header/path, sortable Name/Size/Modified columns, 28px rows, themed scrollbar,
  300–640px resizing (380px reset) and a date column hidden below 341px. Upload,
  folder/file creation, rename, permissions, path copying, terminal navigation,
  the server's starting folder and confirmed deletion live in compact menus.
  Editing and transfer progress use the 1.x footers. External editors and file
  associations reuse the core's save-back/conflict handling; new files use its
  exclusive-create operation. The server's files folder is applied on first
  opening, including home-relative paths.
- AI-agent MCP server and stdio bridge (`nexssh-slint mcp`), with the 1.x protocol,
  keychain token, per-server access, request dialogs and activity. Disabling the
  server or changing its port also refuses existing HTTP connections.
- Compatible backup export/import, encrypted passwords, a summary and confirmation
  before replacement; About shows data/log locations.
- Signed Windows x64 NSIS updates with progress, confirmation and restart. Preview
  releases use `preview.json`, stable releases retain the 1.x `latest.json` endpoint;
  preview builds graduate to the final stable release. The native release workflow
  packages Slint and reuses the existing update key and Explorer certificate.
- Native tray, close choices and saved tabs. Restored tabs wait for Enter before
  connecting. `--cwd <folder>` opens the selected local shell in that folder.
- The 1.x status bar: server address and uptime, CPU/RAM/root-disk figures, one-minute
  graphs, usage colours and detailed tooltips (load averages, swap, free space and
  minimum/maximum/average). A narrow window retains numbers and hides graphs/uptime.
  It reuses the core's Linux metrics command on a separate SSH channel every two seconds;
  only the active connected SSH tab is polled, with no polling while minimized, in
  the tray or with the bar hidden. Each tab keeps its own bounded history. Right-click
  the bar to hide it; the existing General preference restores it. Local terminals
  keep their shell label, and non-Linux servers show the same unavailable state as 1.x.
- One GUI per data directory, including simultaneous launches and differently named
  copies of the executable. Launching again reveals the existing window, restoring
  it from minimization or the tray without changing its tabs or open dialogs.
  Explorer's `--cwd` requests (also `--cwd=...`, multiple folders and relative paths)
  are forwarded to that GUI. OS locks release automatically after exit or a crash.
  Installer helpers, update verification and the MCP stdio bridge remain independent.
  Separate `NEXSSH_DATA_DIR` profiles can run side by side; older alpha.1 binaries
  do not participate in the new instance protocol.
- Session cleanup on exit, including local shell processes.
- Hidden/minimized windows retain the PTY size and existing output; terminal frame
  construction waits until the window is visible, while protocol replies still run.

## Migration plan / limitations

1. **Terminal compatibility:** real-world vim, htop, tmux and ConPTY coverage;
   mouse reporting, IME, regex/whole-word search, links, resizing/reflow and rendering performance.
   Selection is cleared when output changes or the viewport scrolls.
2. **Server management:** group management and remaining contextual actions.
   Some native notices and hints still need full localization.
3. **Files and tools:** bulk selection, drag-and-drop transfers, sudo editing,
   forwarding controls. SSH/SFTP/editor interaction still needs live-server QA.
4. **Desktop integration:** window state and drag-and-drop.
   The installer uses the existing NexSSH identity and the same Explorer package
   handling as 1.x. Debug builds and instances with `NEXSSH_DATA_DIR` leave the
   Explorer menu alone unless `NEXSSH_EXPLORER_MENU` is set.
5. **Measure:** compare release builds for startup time, idle CPU and memory with
   identical session counts before deciding whether to replace the existing client.

Neither full xterm.js compatibility nor a memory/performance improvement is claimed
by this prototype. SSH and platform-specific behavior need testing on all supported OSes.

See [the 2.x release guide](../docs/releases-2.x.md) for local packaging, the two update
channels, signing, CI migration tests and publication. Installer integration tests run
on disposable Windows runners; they must not replace a user's installed NexSSH during QA.

## Structure

- `ui/app.slint`: native window layout, ported from the original Svelte components.
- `ui/controls.slint`: controls ported from the shared CSS.
- `ui/typography.slint`: common UI font family and visual emphasis roles.
- `build.rs`: generates Slint theme colors and SVG assets directly from `ui/src/app.css`
  and `ui/src/lib/components/Icon.svelte`, plus terminal palettes and theme previews
  from `ui/src/lib/themes.ts`. These 1.x files remain the source of truth.
- `build/settings.rs`: reads original defaults, translations and keyword patterns.
- `src/settings.rs`, `settings_schema.rs`: complete preference storage and UI rows.
- `ui/server-editor.slint`, `server-menu.slint`, `src/server_editor.rs`: server forms,
  context actions, validation and asynchronous keychain/file-dialog work.
- `src/services.rs`: file dialogs, backups and metadata off the UI thread.
- `src/updates.rs`: preview/stable discovery, signed downloads and NSIS installation.
- `../explorer/menu.rs`: Explorer integration and installer commands shared with 1.x.
- `src/files.rs`, `editor.rs`: SFTP and external editors; `src/agents/`: native MCP host.
  Editor and MCP implementations are ported from their `desktop/src` counterparts;
  their protocol, access policy and editor resolution tests are retained.
- `src/app.rs`: UI-thread controller, session lifecycle and event bridge.
- `src/terminal.rs`: bounded terminal parser, ANSI colors, input encoding and terminal replies.
- `src/keyboard.rs`: native shortcut normalization and paired key releases.
- `src/metrics.rs`, `ui/status-bar.slint`: asynchronous metrics, bounded history,
  translated values, graphs, tooltips and the responsive footer ported from 1.x.
- `src/instance.rs`: profile lock, authenticated local launch forwarding and foreground handoff.
- `src/prompts.rs`: SSH prompts and responses.
- `../core`: shared SSH, local PTY and storage implementation.

Layout measurements are ported from `App.svelte`, `Sidebar.svelte`, `TabBar.svelte`,
`ServerRow.svelte`, `ServerEditor.svelte`, `Home.svelte`, `CommandPalette.svelte` and `Modal.svelte`, rather
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

The optional UI regression checks start and close their own headless instances with
temporary data. They verify settings controls, dropdown callbacks, language changes,
persistence after restart and preservation of the original settings file. The server
check also covers all six editor tabs, validation, duplication, deletion and session actions;
its SSH connection attempts use a closed loopback port:

```sh
cargo build -p nexssh-slint --features slint/mcp
python native/tests/settings_ui.py
python native/tests/server_editor_ui.py
python native/tests/single_instance.py target/debug/nexssh-slint.exe
# Windows native-window checks (also requires the optional inspector build):
python native/tests/single_instance.py target/debug/nexssh-slint.exe --native
```

The single-instance check covers concurrent starts, preserving UI/tab state, a renamed
executable, forwarded Unicode folders, independent profiles and crash recovery. Its
Windows native pass also checks minimized/tray restoration and foreground activation.
It creates its own temporary profiles and never closes an existing user's NexSSH.

The optional metrics regression runs its own loopback SSH server with a test-only
Python dependency. It checks actual SSH exec channels, independent tab data, colours,
tooltips, languages, hide/resume, unsupported servers and refused commands. The native
Windows pass also checks pausing while minimized and the narrow footer layout:

```powershell
$env:SLINT_EMIT_DEBUG_INFO = '1'
cargo build -p nexssh-slint --features slint/mcp
python -m venv target/native-metrics-tests
./target/native-metrics-tests/Scripts/python.exe -m pip install paramiko==5.1.0
./target/native-metrics-tests/Scripts/python.exe native/tests/metrics_ui.py target/debug/nexssh-slint.exe
./target/native-metrics-tests/Scripts/python.exe native/tests/metrics_ui.py target/debug/nexssh-slint.exe --native
```

The file-drawer regression uses the same test dependency and an isolated loopback
SFTP filesystem. It checks row geometry, menus, sorting, folder navigation,
creation/rename/deletion confirmation, permissions validation, inline errors,
languages and resizing. It never uses saved connections or remote user files:

```powershell
./target/native-metrics-tests/Scripts/python.exe native/tests/files_ui.py target/debug/nexssh-slint.exe
./target/native-metrics-tests/Scripts/python.exe native/tests/files_ui.py target/debug/nexssh-slint.exe --native
```

The optional font regression additionally needs Pillow. It compares pixels of
long, colour-split and bold runs with isolated glyphs in the same terminal columns,
then checks the installed-font picker, persistence/restart and both UI languages:

```powershell
./target/native-metrics-tests/Scripts/python.exe -m pip install Pillow==13.0.0
./target/native-metrics-tests/Scripts/python.exe native/tests/terminal_fonts_ui.py target/debug/nexssh-slint.exe --artifacts target/native-font-tests
# Fractional Windows DPI with the native renderer:
$env:SLINT_SCALE_FACTOR = '1.25'
./target/native-metrics-tests/Scripts/python.exe native/tests/terminal_fonts_ui.py target/debug/nexssh-slint.exe --native --artifacts target/native-font-tests-dpi
Remove-Item Env:SLINT_SCALE_FACTOR
```

For repeatable renderer memory measurements, use the isolated loopback fixture:

```powershell
$env:SLINT_EMIT_DEBUG_INFO = '1'
cargo build -p nexssh-slint --features slint/mcp
uv run --with paramiko --with psutil native/tests/render_memory.py target/debug/nexssh-slint.exe --backend winit-femtovg --output target/render-memory.json
```

The probe samples the home screen, a coloured terminal, 400 updates at 20 Hz,
and idle memory after those updates. It reports resident/private memory, process
CPU time and thread count. Compare the same build profile, window scale and
renderer on the same machine; inspector builds are not release-memory estimates.
GPU drivers/context allocations remain even when terminal items are reused.
Terminal frames update existing model rows and retain unchanged text items;
clearing the screen or closing the last tab releases the terminal model.
