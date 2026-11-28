"""Status bar regression with a loopback-only SSH fixture and isolated app data.

Requires the optional Slint MCP build and test-only Python dependency `paramiko`.
Run: python native/tests/metrics_ui.py <binary> [--native] [--artifacts <folder>]
No saved servers, credentials or installed NexSSH processes are used.
"""
import argparse
import base64
from collections import Counter
import ctypes
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request

import paramiko

from settings_ui import Inspector, wait_for


class Fixture:
    def __init__(self, shell_output=b"METRICS_FIXTURE_TERMINAL\r\n"):
        self.shell_output = shell_output
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.port = self.listener.getsockname()[1]
        self.listener.listen()
        self.listener.settimeout(0.2)
        self.key = paramiko.RSAKey.generate(2048)
        self.stop = threading.Event()
        self.transports = []
        self.reads = Counter()
        self.shells = []
        self.execs = []
        self.lock = threading.Lock()
        self.thread = threading.Thread(target=self.accept, daemon=True)
        self.thread.start()

    def count(self, user):
        with self.lock:
            return self.reads[user]

    def accept(self):
        while not self.stop.is_set():
            try:
                connection, _ = self.listener.accept()
            except socket.timeout:
                continue
            transport = paramiko.Transport(connection)
            self.transports.append(transport)
            transport.add_server_key(self.key)
            threading.Thread(target=self.serve, args=(transport,), daemon=True).start()

    def serve(self, transport):
        fixture = self

        class Server(paramiko.ServerInterface):
            user = ""

            def check_auth_none(self, username):
                self.user = username
                return paramiko.AUTH_SUCCESSFUL

            def get_allowed_auths(self, username):
                return "none"

            def check_channel_request(self, kind, channel_id):
                return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

            def check_channel_pty_request(self, *args):
                return True

            def check_channel_window_change_request(self, *args):
                return True

            def check_channel_shell_request(self, channel):
                with fixture.lock:
                    fixture.shells.append((self.user, channel.get_id()))
                channel.sendall(fixture.shell_output)
                return True

            def check_channel_exec_request(self, channel, command):
                with fixture.lock:
                    fixture.reads[self.user] += 1
                    number = fixture.reads[self.user]
                    fixture.execs.append((self.user, channel.get_id(), command))
                if self.user == "denied":
                    return False

                def reply():
                    if self.user == "unsupported":
                        payload = b"not a Linux server\n"
                    else:
                        busy, memory, disk, free = (20, 3, 20, 40) if self.user == "first" else (95, 6, 36, 4)
                        payload = (f"@stat\ncpu {number * busy} 0 0 {number * (100-busy)} 0 0 0 0\n"
                                   f"@mem\nMemTotal: 8388608 kB\nMemAvailable: {(8-memory)*1048576} kB\n"
                                   "SwapTotal: 2097152 kB\nSwapFree: 1048576 kB\n"
                                   "@load\n0.25 0.50 0.75 1/10 42\n@uptime\n183840.5 0\n"
                                   f"@df\nFilesystem 1024-blocks Used Available Capacity Mounted-on\n"
                                   f"/dev/test 67108864 {disk*1048576} {free*1048576} 33% /\n").encode()
                    try:
                        channel.sendall(payload)
                        channel.send_exit_status(0)
                        channel.shutdown_write()
                        channel.close()
                    except (OSError, EOFError):
                        pass

                threading.Thread(target=reply, daemon=True).start()
                return True

        channels = []
        try:
            transport.start_server(server=Server())
            while transport.is_active() and not self.stop.is_set():
                channel = transport.accept(0.2)
                if channel:
                    channels.append(channel)
        except (OSError, EOFError, paramiko.SSHException):
            pass
        finally:
            transport.close()

    def close(self):
        self.stop.set()
        self.thread.join(timeout=2)
        self.listener.close()
        for transport in self.transports:
            transport.close()


def run(binary, native=False, artifacts=None):
    fixture = Fixture()
    process = None
    foreground = None
    if native:
        from ctypes import wintypes
        user = ctypes.WinDLL("user32", use_last_error=True)
        user.GetForegroundWindow.restype = wintypes.HWND
        user.SetForegroundWindow.argtypes = [wintypes.HWND]
        foreground = user.GetForegroundWindow()
    try:
        with tempfile.TemporaryDirectory(prefix="nexssh-metrics-ui-") as folder:
            folder = Path(folder)
            (folder / "settings.json").write_text(json.dumps({
                "language": "en", "theme": "black", "autoUpdateCheck": False,
                "agentsEnabled": False, "explorerMenu": False, "closeAction": "quit",
                "sidebarAutoHide": False, "restoreTabs": "never", "statusBar": True}))
            servers = [{"id": name, "name": name, "user": user, "host": "127.0.0.1",
                        "port": fixture.port, "auth": "auto"} for name, user in
                       [("Stats One", "first"), ("Stats Two", "second"),
                        ("No proc", "unsupported"), ("Exec denied", "denied")]]
            (folder / "servers.json").write_text(json.dumps({"version": 1, "servers": servers}))
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            env = dict(os.environ, NEXSSH_DATA_DIR=str(folder), SLINT_MCP_PORT=str(port))
            if native:
                env.pop("SLINT_BACKEND", None)
            else:
                env["SLINT_BACKEND"] = "headless"
            with (folder / "stderr.log").open("w", encoding="utf-8") as log:
                process = subprocess.Popen([str(binary)], env=env, stdin=subprocess.DEVNULL,
                    stdout=log, stderr=log,
                    creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
                try:
                    ui = Inspector(port)

                    def connect(name):
                        ui.press(name)
                        wait_for(lambda: ui.find("Доверять ключу"), "Host-key confirmation missing")
                        ui.press("Доверять ключу")
                        wait_for(lambda: fixture.count(next(s["user"] for s in servers if s["name"] == name)) > 0,
                                 "Metrics were not read on the connected SSH session")

                    def reading(label):
                        return ui.find(label, "Groupbox")

                    def saved():
                        return json.loads((folder / "native-settings.json").read_text(encoding="utf-8"))

                    def snapshot(name):
                        if artifacts is None:
                            return
                        artifacts.mkdir(parents=True, exist_ok=True)
                        body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                "params": {"name": "take_screenshot", "arguments": {"windowHandle": ui.window}}}
                        request = urllib.request.Request(ui.url, json.dumps(body).encode(),
                            headers={"Content-Type": "application/json"})
                        with urllib.request.urlopen(request) as response:
                            content = json.load(response)["result"]["content"]
                        for block in content:
                            if block["type"] == "image":
                                (artifacts / f"{name}.png").write_bytes(base64.b64decode(block["data"]))

                    connect("Stats One")
                    wait_for(lambda: reading("CPU: 20%"), "CPU delta was not shown")
                    assert reading("RAM: 3.0 / 8.0 GB")
                    assert reading("Disk: 20 / 64 GB")
                    assert ui.find("up 2 d 3 h", "Text")
                    assert ui.find(f"first@127.0.0.1:{fixture.port}", "Text")
                    for name in ["CPU: 20%", "RAM: 3.0 / 8.0 GB", "Disk: 20 / 64 GB"]:
                        group = reading(name)[0]
                        images = ui.call("query_element_descendants", {"elementHandle": group,
                            "queryStack": [{"matchDescendants": True}, {"matchElementTypeName": "Image"}],
                            "findAll": True}).get("elementHandles", [])
                        if images:
                            image = ui.call("get_element_properties", {"elementHandle": images[0]})
                            label = ui.call("get_element_properties", {"elementHandle": ui.find(name.split(":")[0], "Text")[0]})
                            assert label["absolutePosition"]["x"] + label["size"]["width"] <= image["absolutePosition"]["x"], "Metric label overlaps graph"
                    snapshot("status-en")
                    cpu = reading("CPU: 20%")[0]
                    ui.call("hover_element", {"elementHandle": cpu})
                    wait_for(lambda: ui.find("Processor: 20% busy\nLoad average: 0.25  0.5  0.75\nLast minute: 20% to 20%, 20% on average", "Text"),
                             "CPU tooltip and minute statistics missing")
                    snapshot("tooltip-en")

                    connect("Stats Two")
                    wait_for(lambda: reading("CPU: 95%"), "New tab reused another server's CPU")
                    assert reading("RAM: 6.0 / 8.0 GB")
                    assert not ui.find("Processor: 20% busy\nLoad average: 0.25  0.5  0.75\nLast minute: 20% to 20%, 20% on average", "Text"), "Old tab's tooltip remained visible"
                    snapshot("status-high")
                    ui.press("Stats One", "Tab")
                    wait_for(lambda: reading("CPU: 20%"), "Returning to a tab lost its cached reading")

                    # The context menu uses the same persisted preference as Settings.
                    ui.call("click_element", {"elementHandle": reading("CPU: 20%")[0], "button": "Right"})
                    ui.press("Hide status bar")
                    wait_for(lambda: not saved()["statusBar"], "Hiding the bar was not persisted")
                    time.sleep(0.2)
                    count = fixture.count("first")
                    time.sleep(2.4)
                    assert fixture.count("first") == count, "Hidden status bar continued polling"
                    ui.press("Settings")
                    ui.press("Status bar", "Switch")
                    ui.press("Русский")
                    ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\x1b"})
                    wait_for(lambda: fixture.count("first") > count, "Re-enabling did not resume polling")
                    wait_for(lambda: reading("ЦП: 20\u00a0%"), "Cached metrics did not change language")
                    assert reading("Память: 3,0 / 8,0 ГБ")
                    snapshot("status-ru")
                    ui.press("Настройки")
                    ui.press("English")
                    ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\x1b"})

                    connect("No proc")
                    wait_for(lambda: ui.find("No load figures", "Text"), "Unsupported server state missing")
                    count = fixture.count("unsupported")
                    time.sleep(2.4)
                    assert fixture.count("unsupported") == count, "Unsupported server was polled every two seconds"
                    assert not reading("CPU: 20%")
                    connect("Exec denied")
                    count = fixture.count("denied")
                    time.sleep(2.4)
                    assert fixture.count("denied") == count, "Refused exec did not back off"
                    assert not reading("CPU: 20%")
                    ui.press("Stats One", "Tab")
                    wait_for(lambda: reading("CPU: 20%"), "Metrics did not resume")

                    if native:
                        check_native_window(process, ui, fixture, snapshot, binary, env)
                    with fixture.lock:
                        assert len(fixture.shells) == 4, "Metrics opened additional terminals"
                        assert all(b"/proc/stat" in command for _, _, command in fixture.execs)
                        assert all((user, channel) not in fixture.shells for user, channel, _ in fixture.execs)
                    ui.press("Close")
                    assert process.wait(timeout=8) == 0
                    print("PASS: real SSH channels, CPU/RAM/disk, graphs/tooltips, tab isolation, "
                          "language, hide/resume, unsupported servers and error backoff")
                except BaseException:
                    log.flush()
                    print((folder / "stderr.log").read_text(encoding="utf-8", errors="replace")[-5000:])
                    raise
                finally:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)
    finally:
        fixture.close()
        if native and foreground:
            user.SetForegroundWindow(foreground)


def check_native_window(process, ui, fixture, snapshot, binary, env):
    from ctypes import wintypes
    user = ctypes.WinDLL("user32", use_last_error=True)
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    user.IsIconic.argtypes = [wintypes.HWND]
    user.IsWindowVisible.argtypes = [wintypes.HWND]
    user.MoveWindow.argtypes = [wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.BOOL]
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
    handles = []

    @callback_type
    def collect(hwnd, _):
        pid = wintypes.DWORD()
        user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        title = ctypes.create_unicode_buffer(256)
        user.GetWindowTextW(hwnd, title, 256)
        if pid.value == process.pid and title.value == "NexSSH 2.0":
            handles.append(hwnd)
        return True

    user.EnumWindows(collect, 0)
    assert len(handles) == 1
    hwnd = handles[0]
    assert ui.find("METRICS_FIXTURE_TERMINAL", "Text"), "Fixture terminal text missing before minimize"
    user.ShowWindow(hwnd, 6)
    wait_for(lambda: user.IsIconic(hwnd), "Window did not minimize")
    time.sleep(0.2)
    count = fixture.count("first")
    time.sleep(2.4)
    assert fixture.count("first") == count, "Minimized window kept polling"
    user.ShowWindow(hwnd, 9)
    wait_for(lambda: fixture.count("first") > count, "Restore did not resume polling")
    wait_for(lambda: ui.find("METRICS_FIXTURE_TERMINAL", "Text"), "Minimize truncated existing terminal output")
    user.MoveWindow(hwnd, 50, 50, 780, 650, True)
    time.sleep(0.2)
    snapshot("status-narrow")
    groups = [ui.call("get_element_properties", {"elementHandle": ui.find(label, "Groupbox")[0]})
              for label in ["CPU: 20%", "RAM: 3.0 / 8.0 GB", "Disk: 20 / 64 GB"]]
    for left, right in zip(groups, groups[1:]):
        assert left["absolutePosition"]["x"] + left["size"]["width"] <= right["absolutePosition"]["x"], "Metrics overlap in a narrow window"
    bar = ui.call("get_element_properties", {"elementHandle": ui.find("Status bar", "Groupbox")[0]})
    assert groups[-1]["absolutePosition"]["x"] + groups[-1]["size"]["width"] <= bar["absolutePosition"]["x"] + bar["size"]["width"], "Metrics extend beyond the footer"

    def close_preference(label):
        ui.press("Settings")
        for _ in range(20):
            if ui.find(label):
                ui.press(label)
                ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\x1b"})
                return
            ui.press("Settings", "Slider", "Increment")
        raise AssertionError("Close preference not found")

    close_preference("To tray")
    ui.press("Close")
    wait_for(lambda: not user.IsWindowVisible(hwnd), "App did not hide in tray")
    time.sleep(0.2)
    count = fixture.count("first")
    time.sleep(2.4)
    assert fixture.count("first") == count, "Tray window continued polling"
    result = subprocess.run([str(binary)], env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10,
        creationflags=subprocess.CREATE_NO_WINDOW)
    assert result.returncode == 0
    wait_for(lambda: fixture.count("first") > count, "Opening from tray did not resume polling")
    wait_for(lambda: ui.find("METRICS_FIXTURE_TERMINAL", "Text"), "Tray restore truncated terminal output")
    close_preference("Quit")
    print("PASS: native minimize/tray pause and resume; narrow layout")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=lambda p: Path(p).resolve())
    parser.add_argument("--native", action="store_true")
    parser.add_argument("--artifacts", type=Path)
    args = parser.parse_args()
    run(args.binary, args.native, args.artifacts)
