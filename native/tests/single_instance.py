"""Real-process single-instance regression, with isolated profiles and owned PIDs only.

Build with SLINT_EMIT_DEBUG_INFO=1 and --features slint/mcp.
Run: python native/tests/single_instance.py <binary> [--native]
The Windows --native pass additionally checks minimize, tray and activation.
"""
import ctypes
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time

from settings_ui import Inspector, wait_for


def run(binary, native=False):
    if native and os.name != "nt":
        raise ValueError("--native requires Windows")
    processes = []
    user = None
    foreground = None
    if native:
        from ctypes import wintypes
        user = ctypes.WinDLL("user32", use_last_error=True)
        user.GetForegroundWindow.restype = wintypes.HWND
        user.SetForegroundWindow.argtypes = [wintypes.HWND]
        user.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
        user.IsIconic.argtypes = [wintypes.HWND]
        user.IsZoomed.argtypes = [wintypes.HWND]
        user.IsWindowVisible.argtypes = [wintypes.HWND]
        user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
        user.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
        callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        user.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
        foreground = user.GetForegroundWindow()

        def windows(pid):
            found = []
            @callback_type
            def collect(hwnd, _):
                owner = wintypes.DWORD()
                user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
                if owner.value == pid:
                    title = ctypes.create_unicode_buffer(256)
                    user.GetWindowTextW(hwnd, title, 256)
                    if title.value == "NexSSH 2.0":
                        found.append(hwnd)
                return True
            user.EnumWindows(collect, 0)
            return found

    with tempfile.TemporaryDirectory(prefix="nexssh-instance-") as directory:
        directory = Path(directory)
        logs = []
        settings = {"language": "en", "theme": "black", "agentsEnabled": False,
                    "explorerMenu": False, "autoUpdateCheck": False,
                    "closeAction": "quit", "restoreTabs": "always",
                    "localShell": "custom",
                    "localShellCommand": '"cmd.exe" /d /q' if os.name == "nt" else "/bin/sh"}

        def profile(name):
            folder = directory / name
            folder.mkdir()
            (folder / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
            return folder

        def environment(folder):
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            env = dict(os.environ, NEXSSH_DATA_DIR=str(folder), SLINT_MCP_PORT=str(port))
            if native:
                env.pop("SLINT_BACKEND", None)
            else:
                env["SLINT_BACKEND"] = "headless"
            return env, port

        def spawn(env, args=(), executable=binary, cwd=None):
            log = (directory / f"process-{len(processes)}.log").open("w", encoding="utf-8")
            logs.append(log)
            startup = None
            if os.name == "nt":
                startup = subprocess.STARTUPINFO()
                startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
                startup.wShowWindow = subprocess.SW_HIDE
            process = subprocess.Popen([str(executable), *args], cwd=cwd, env=env,
                stdin=subprocess.DEVNULL, stdout=log, stderr=log, startupinfo=startup,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            processes.append(process)
            return process

        def endpoint(folder):
            return json.loads((folder / ".native-instance.json").read_text(encoding="utf-8"))

        def saved(folder):
            return json.loads((folder / "native-settings.json").read_text(encoding="utf-8"))

        def forward(env, args=(), **kwargs):
            other = spawn(env, args, **kwargs)
            assert other.wait(timeout=10) == 0, f"Secondary failed: {other.returncode}"

        def close(process, ui):
            ui.press("Close")
            assert process.wait(timeout=8) == 0

        def close_choice(ui, label):
            ui.press("Settings")
            for _ in range(20):
                if ui.find(label):
                    ui.press(label)
                    return
                ui.press("Settings", "Slider", "Increment")
            raise AssertionError(f"Missing close preference: {label}")

        try:
            folder = profile("shared")
            env, port = environment(folder)
            burst = [spawn(env) for _ in range(6)]
            owner = wait_for(lambda: endpoint(folder)["pid"], "No primary endpoint")
            primary = next(p for p in burst if p.pid == owner)
            ui = Inspector(port)
            for process in burst:
                if process is not primary:
                    assert process.wait(timeout=10) == 0
            assert primary.poll() is None
            if native:
                hwnd = wait_for(lambda: windows(owner), "No native window")[0]
                assert len(windows(owner)) == 1

            # Revealing an existing GUI must preserve the settings overlay and edits.
            ui.press("Settings")
            ui.press("Larger")
            wait_for(lambda: saved(folder)["fontSize"] == 14, "Settings not saved")
            forward(env)
            assert ui.find("General", "Tab"), "Second launch dismissed the settings overlay"
            assert saved(folder)["fontSize"] == 14
            ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\x1b"})

            # Same profile through a differently named executable still forwards.
            copy = directory / binary.name
            shutil.copy2(binary, copy)
            forward(env, executable=copy)
            assert endpoint(folder)["pid"] == owner

            # Resolve relative paths at the sender, support Unicode and multiple folders.
            first = directory / "Папка с пробелами"
            second = directory / "mcp"
            first.mkdir()
            second.mkdir()
            forward(env, ["--cwd=" + first.name, "--cwd", "mcp"], cwd=directory)
            expected = [str(first), str(second)]
            wait_for(lambda: [tab["target"]["local"]["cwd"]
                             for tab in saved(folder)["lastTabs"]["tabs"]] == expected,
                     "Forwarded folders did not open in the original GUI")
            before = saved(folder)["lastTabs"]
            forward(env)
            time.sleep(0.2)
            assert saved(folder)["lastTabs"] == before, "Bare launch changed existing tabs"

            if native:
                user.ShowWindow(hwnd, 6)  # SW_MINIMIZE
                wait_for(lambda: user.IsIconic(hwnd), "Window did not minimize")
                helper = spawn(env, ["--verify-update", "missing.json", "missing.exe"])
                assert helper.wait(timeout=3) == 1
                assert user.IsIconic(hwnd), "Installer helper incorrectly activated the GUI"
                forward(env)
                wait_for(lambda: not user.IsIconic(hwnd) and user.IsWindowVisible(hwnd),
                         "Second launch did not restore a minimized window")
                wait_for(lambda: user.GetForegroundWindow() == hwnd,
                         "Existing window was not brought to the foreground")
                user.ShowWindow(hwnd, 3)  # SW_MAXIMIZE
                wait_for(lambda: user.IsZoomed(hwnd), "Window did not maximize")
                forward(env)
                time.sleep(0.1)
                assert user.IsZoomed(hwnd), "Second launch discarded maximized state"
                user.ShowWindow(hwnd, 6)
                wait_for(lambda: user.IsIconic(hwnd), "Maximized window did not minimize")
                forward(env)
                wait_for(lambda: not user.IsIconic(hwnd) and user.IsZoomed(hwnd),
                         "Minimized maximized window did not retain its state")
                close_choice(ui, "To tray")
                wait_for(lambda: saved(folder)["closeAction"] == "tray", "Tray choice not applied")
                ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\x1b"})
                ui.press("Close")
                wait_for(lambda: not user.IsWindowVisible(hwnd), "Window did not hide in tray")
                assert primary.poll() is None
                forward(env)
                wait_for(lambda: user.IsWindowVisible(hwnd), "Second launch did not reveal tray window")
                assert len(windows(owner)) == 1
                close_choice(ui, "Quit")
                wait_for(lambda: saved(folder)["closeAction"] == "quit", "Quit choice not applied")
                ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\x1b"})

            # Another data directory is independent (isolated/portable profiles).
            other_folder = profile("separate")
            other_env, other_port = environment(other_folder)
            separate = spawn(other_env)
            separate_ui = Inspector(other_port)
            assert endpoint(other_folder)["pid"] == separate.pid != owner
            close(separate, separate_ui)
            close(primary, ui)
            assert not (folder / ".native-instance.json").exists()

            # An OS lock is released even after a crash, without deleting the lock file.
            crash_folder = profile("crash")
            crash_env, crash_port = environment(crash_folder)
            crashed = spawn(crash_env)
            Inspector(crash_port)
            crashed.kill()  # Only our sessionless, isolated child process.
            crashed.wait(timeout=5)
            assert (crash_folder / ".native-instance.json").exists()
            restart_env, restart_port = environment(crash_folder)
            restarted = spawn(restart_env)
            restart_ui = Inspector(restart_port)
            assert endpoint(crash_folder)["pid"] == restarted.pid
            forward(restart_env)
            close(restarted, restart_ui)
            print("PASS: simultaneous launches, existing GUI/state, renamed exe, Unicode cwd, "
                  "separate profiles, clean exit and crash recovery" +
                  (", minimized/maximized restore, foreground activation, helper bypass and tray" if native else ""))
        except BaseException:
            for log in logs:
                log.flush()
                content = Path(log.name).read_text(encoding="utf-8", errors="replace")
                if content:
                    print(Path(log.name).name + ":\n" + content[-3000:], file=sys.stderr)
            raise
        finally:
            for process in processes:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
            for log in logs:
                log.close()
            if native and foreground:
                user.SetForegroundWindow(foreground)


if __name__ == "__main__":
    run(Path(sys.argv[1]).resolve(), "--native" in sys.argv[2:])
