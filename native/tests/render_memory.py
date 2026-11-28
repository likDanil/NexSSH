"""Repeatable native renderer memory/CPU probe using an isolated loopback SSH fixture.

Build with SLINT_EMIT_DEBUG_INFO=1 and --features slint/mcp.
Test-only dependencies: paramiko, psutil.
Example: uv run --with paramiko --with psutil native/tests/render_memory.py <exe>
Use --backend winit-femtovg, or --backend winit-femtovg-wgpu --wgpu-backend dx12
with a build that enables slint/renderer-femtovg-wgpu, for comparisons.
No working-set trimming, screenshots during sampling, or real SSH profiles are used.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

import psutil

from metrics_ui import Fixture
from settings_ui import Inspector, wait_for


def screen(number):
    # Most text is unchanged, as with a terminal dashboard. Every tenth update
    # also changes run count, exercising insertion/removal of rendered items.
    rows = []
    for row in range(36):
        parts = [f"\x1b[{31 + col % 6}m{col:02d} abc" for col in range(12)]
        rows.append(f"\x1b[{row+1};1H" + " ".join(parts))
    rows.append(f"\x1b[1;1H\x1b[0m{number:06d}")
    rows.append("\x1b[38;1H\x1b[2K" + ("short" if number % 20 < 10 else "longer \x1b[32mcoloured"))
    return ("\x1b[?25l" + "".join(rows)).encode()


def run(args):
    fixture = Fixture(shell_output=screen(0))
    result = {"binary": str(args.binary), "backend": args.backend,
              "wgpu_backend": args.wgpu_backend, "samples": []}
    try:
        with tempfile.TemporaryDirectory(prefix="nexssh-render-memory-") as folder:
            folder = Path(folder)
            (folder / "settings.json").write_text(json.dumps({
                "language": "en", "theme": "black", "gpuAcceleration": True,
                "agentsEnabled": False, "autoUpdateCheck": False, "explorerMenu": False,
                "restoreTabs": "never", "closeAction": "quit", "filesOpen": False,
                "statusBar": False, "highlightKeywords": False, "scrollback": 100,
            }), encoding="utf-8")
            (folder / "servers.json").write_text(json.dumps({"version": 1, "servers": [{
                "id": "fixture", "name": "Render fixture", "host": "127.0.0.1",
                "port": fixture.port, "user": "first", "auth": "auto",
            }]}), encoding="utf-8")
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            env = dict(os.environ, NEXSSH_DATA_DIR=str(folder), SLINT_MCP_PORT=str(port))
            env.pop("SLINT_BACKEND", None)
            env.pop("WGPU_BACKEND", None)
            if args.backend:
                env["SLINT_BACKEND"] = args.backend
            if args.wgpu_backend:
                env["WGPU_BACKEND"] = args.wgpu_backend
            with (folder / "stderr.log").open("w", encoding="utf-8") as log:
                process = subprocess.Popen([str(args.binary)], env=env,
                    stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                    creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
                try:
                    observed = psutil.Process(process.pid)

                    def sample(stage):
                        memory = observed.memory_info()
                        cpu = observed.cpu_times()
                        value = {"stage": stage, "rss_mib": round(memory.rss / 2**20, 2),
                                 "private_mib": round(getattr(memory, "private", memory.vms) / 2**20, 2),
                                 "cpu_seconds": round(cpu.user + cpu.system, 3),
                                 "threads": observed.num_threads()}
                        result["samples"].append(value)
                        print(json.dumps(value), flush=True)

                    ui = Inspector(port)
                    time.sleep(3)
                    sample("home")
                    ui.press("Render fixture")
                    ui.press("Доверять ключу")
                    wait_for(lambda: fixture.shells, "SSH shell did not open")
                    channel_id = fixture.shells[0][1]
                    channel = fixture.transports[0]._channels.get(channel_id)
                    assert channel is not None
                    time.sleep(2)
                    sample("terminal")
                    started = time.monotonic()
                    for number in range(args.frames):
                        channel.sendall(screen(number))
                        time.sleep(max(0, started + (number + 1) / 20 - time.monotonic()))
                    sample("after_updates")
                    time.sleep(3)
                    sample("settled")
                    result["graphics_modules"] = sorted({Path(item.path).name for item in
                        observed.memory_maps() if Path(item.path).name.lower().startswith(
                        ("nvoglv", "nvwgf", "d3d12", "vulkan", "atio", "igxel"))})
                    ui.press("Close")
                    assert process.wait(timeout=8) == 0
                except BaseException:
                    log.flush()
                    print((folder / "stderr.log").read_text(encoding="utf-8", errors="replace")[-4000:])
                    raise
                finally:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)
    finally:
        fixture.close()
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=lambda value: Path(value).resolve())
    parser.add_argument("--backend")
    parser.add_argument("--wgpu-backend")
    parser.add_argument("--frames", type=int, default=400)
    parser.add_argument("--output", type=Path)
    run(parser.parse_args())
