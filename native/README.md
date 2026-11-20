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

- Native dark window, server list and search.
- Quick SSH connection (`user@host:port`, including bracketed IPv6).
- Multiple local and SSH sessions, switching/closing tabs and reconnecting.
- Username, password, private-key passphrase, keyboard-interactive/2FA and host-key dialogs.
- Native terminal state via `vt100`: ANSI colors, truecolor, cursor positioning,
  alternate screen, Unicode cells and PTY resizing.
- 5,000 lines of scrollback: mouse wheel or Shift+PageUp / Shift+PageDown.
- Copy visible screen (button or Ctrl+Shift+C), paste (button or Ctrl+Shift+V),
  bracketed paste and confirmation before inserting multiple lines.
- Session cleanup on exit, including local shell processes.

## Migration plan / limitations

1. **Terminal compatibility:** real-world vim, htop, tmux and ConPTY coverage;
   mouse reporting, text selection, IME, search, links, resizing/reflow and rendering performance.
   Current copy copies the entire visible screen; mouse selection is not implemented.
2. **Server management:** create/edit/delete, group management, SSH config import,
   shell selection, settings, translations and themes. The preview currently uses Russian
   UI labels and the default local shell.
3. **Files and tools:** SFTP UI, transfers, forwarding controls, metrics and the AI-agent bridge.
4. **Desktop integration:** tray, session restoration, window state, drag-and-drop,
   Explorer integration, signed updates and packaging.
5. **Measure:** compare release builds for startup time, idle CPU and memory with
   identical session counts before deciding whether to replace the existing client.

Neither full xterm.js compatibility nor a memory/performance improvement is claimed
by this prototype. SSH and platform-specific behavior need testing on all supported OSes.

## Structure

- `ui/app.slint`: native window and components.
- `src/app.rs`: UI-thread controller, session lifecycle and event bridge.
- `src/terminal.rs`: bounded terminal parser, ANSI colors, input encoding and terminal replies.
- `src/prompts.rs`: SSH prompts and responses.
- `../core`: shared SSH, local PTY and storage implementation.

For UI inspection in development, Slint provides an optional local MCP server:

```powershell
$env:SLINT_EMIT_DEBUG_INFO = '1'
$env:SLINT_MCP_PORT = '9127'
cargo run -p nexssh-slint --features slint/mcp
```

Do not enable that feature in release builds. See the
[Slint testing documentation](https://docs.rs/i-slint-backend-testing/1.19.0/i_slint_backend_testing/).
