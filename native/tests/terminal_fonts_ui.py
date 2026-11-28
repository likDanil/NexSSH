"""Terminal grid/font-picker regression over an isolated loopback SSH fixture.

Requires the optional Slint inspector and test-only paramiko/Pillow dependencies.
Pixel checks compare long, colour-split and bold runs with isolated cells in the
same columns. They catch the old rounded-cell/text-run drift, not font aesthetics.
"""
import argparse
import base64
from io import BytesIO
import json
import math
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request

from PIL import Image
from metrics_ui import Fixture
from settings_ui import Inspector, wait_for


MARKERS = [0, 7, 15, 31, 47, 63]


def payload():
    text = "\x1b[?25l\x1b[2J\x1b[H" + "|" * 64 + "\r\n"
    text += "".join(("\x1b[37m" if i % 2 else "\x1b[36m") + "|" * 8 for i in range(8)) + "\x1b[0m\r\n"
    text += "\x1b[1m" + "|" * 64 + "\x1b[0m"
    for row, weight in [(4, "0"), (5, "1")]:
        for col in MARKERS:
            text += f"\x1b[{row};{col+1}H\x1b[{weight}m|"
    text += ("\x1b[0m\x1b[7;1HBusyBox v1.37.0 built-in shell (ash)\r\n"
             "~ # neo status\r\n\x1b[1m Checking HydraRoute Neo daemon...             \x1b[32malive.\x1b[0m\r\n"
             "~ # neo\r\n\x1b[1m Usage: /opt/bin/neo (start|stop|restart|check|status)\x1b[0m\r\n"
             "~ # UTF-8: Русский 界 e\u0301\r\n")
    return text.encode()


def screenshot(ui):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "take_screenshot", "arguments": {"windowHandle": ui.window}}}
    request = urllib.request.Request(ui.url, json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request) as response:
        for block in json.load(response)["result"]["content"]:
            if block["type"] == "image":
                return base64.b64decode(block["data"])
    raise AssertionError("No screenshot")


def bars(image, x, y, width, height):
    crop = image.crop((x, y, x+width, y+height)).convert("RGB")
    # The isolated profile has a black terminal. Foreground bars, including cyan,
    # exceed this threshold even along antialiased edges.
    columns = [sum(max(crop.getpixel((i,j))) > 90 for j in range(height)) >= 3 for i in range(width)]
    spans = []
    for i, foreground in enumerate(columns):
        if foreground and (i == 0 or not columns[i-1]):
            spans.append([i, i])
        elif foreground:
            spans[-1][1] = i
    return [(a+b)/2 for a,b in spans]


def check_grid(ui, raw, font_size):
    handles = ui.call("query_element_descendants", {"elementHandle": ui.root,
        "queryStack": [{"matchDescendants": True}, {"matchElementId": "AppWindow::terminal"}], "findAll": True})["elementHandles"]
    bounds = ui.call("get_element_properties", {"elementHandle": handles[0]})
    window = ui.call("get_window_properties", {"windowHandle": ui.window})
    scale = window.get("scaleFactor", 1)
    x = round(bounds["absolutePosition"]["x"] * scale)
    y = round(bounds["absolutePosition"]["y"] * scale)
    width = round(bounds["size"]["width"] * scale)
    height = math.ceil(font_size * 1.25 * scale)
    image = Image.open(BytesIO(raw))
    positions = [bars(image, x, y+row*height, width, height) for row in range(5)]
    assert [len(p) for p in positions] == [64, 64, 64, 6, 6], [len(p) for p in positions]
    error = max(abs(positions[row][col] - positions[anchor][i])
                for row, anchor in [(0,3),(1,3),(2,4)] for i,col in enumerate(MARKERS))
    assert error <= 1, f"Text drifts {error}px from terminal columns"
    return error


def run(binary, artifacts, baseline=False, native=False, gpu=False):
    fixture = Fixture(shell_output=payload())
    with tempfile.TemporaryDirectory(prefix="nexssh-fonts-ui-") as folder:
        folder = Path(folder)
        (folder / "settings.json").write_text(json.dumps({"language":"en", "theme":"black",
            "agentsEnabled":False, "explorerMenu":False, "autoUpdateCheck":False, "closeAction":"quit",
            "restoreTabs":"never", "sidebarAutoHide":False, "highlightKeywords":False,
            "fontFamily":"", "fontSize":13, "lineHeight":1.25, "filesOpen":False,
            "gpuAcceleration":gpu}))
        (folder / "servers.json").write_text(json.dumps({"version":1, "servers":[{
            "id":"fixture", "name":"Font fixture", "host":"127.0.0.1", "port":fixture.port,
            "user":"first", "auth":"auto"}]}))
        with socket.socket() as sock:
            sock.bind(("127.0.0.1",0))
            port = sock.getsockname()[1]
        env = dict(os.environ, NEXSSH_DATA_DIR=str(folder), SLINT_MCP_PORT=str(port))
        if native:
            env.pop("SLINT_BACKEND", None)
        else:
            env["SLINT_BACKEND"] = "headless"
        with (folder / "stderr.log").open("w", encoding="utf-8") as log:
            process = subprocess.Popen([str(binary)], env=env, stdin=subprocess.DEVNULL,
                stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            try:
                ui = Inspector(port)
                ui.press("Font fixture")
                ui.press("Доверять ключу")
                wait_for(lambda: fixture.shells, "SSH shell did not open")
                time.sleep(0.4)
                artifacts.mkdir(parents=True, exist_ok=True)

                def capture(name, size=13):
                    raw = screenshot(ui)
                    (artifacts / f"{name}.png").write_bytes(raw)
                    if not baseline:
                        return check_grid(ui, raw, size)

                def close_settings():
                    # Click the backdrop outside the centered dialog.
                    ui.call("click_element", {"elementHandle":ui.find("Servers")[0]})
                    wait_for(lambda: not ui.find("Font", "Combobox"), "Settings did not close")

                capture("bundled-13")
                if baseline:
                    print("Captured baseline terminal")
                    return
                ui.press("Settings")
                ui.press("Font", "Combobox", "Expand")
                wait_for(lambda: ui.find("Consolas"), "Installed font list is missing Consolas", seconds=20)
                (artifacts / "font-picker.png").write_bytes(screenshot(ui))
                assert ui.find("Courier New") and ui.find("JetBrains Mono (built in)")
                assert not ui.find("Arial"), "Proportional UI font offered for terminal"
                ui.press("Consolas")
                saved = lambda: json.loads((folder / "native-settings.json").read_text())
                wait_for(lambda: saved()["fontFamily"] == "Consolas", "Font choice was not persisted")
                close_settings()
                capture("consolas-13")
                ui.press("Settings")
                ui.choose("Font", "Courier New")
                for _ in range(4): ui.press("Larger")
                wait_for(lambda: saved()["fontSize"] == 17, "Font size was not persisted")
                close_settings()
                capture("courier-17", 17)
                ui.press("Settings")
                ui.choose("Font", "JetBrains Mono (built in)")
                for _ in range(8): ui.press("Smaller")
                close_settings()
                capture("bundled-9", 9)
                ui.press("Settings")
                ui.press("Русский")
                ui.press("Шрифт", "Combobox", "Expand")
                assert ui.find("JetBrains Mono (встроенный)") and ui.find("Consolas")
                (artifacts / "font-picker-ru.png").write_bytes(screenshot(ui))
                ui.press("Consolas")
                ui.press("Закрыть")
                assert process.wait(timeout=8) == 0
                process = subprocess.Popen([str(binary)], env=env, stdin=subprocess.DEVNULL,
                    stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
                ui = Inspector(port)
                ui.press("Настройки")
                font = ui.find("Шрифт", "Combobox")[0]
                assert ui.call("get_element_properties", {"elementHandle":font})["accessibleValue"] == "Consolas"
                ui.press("Закрыть")
                assert process.wait(timeout=8) == 0
                print("PASS: installed monospace picker, persistence, localization; regular/bold/colour runs align with isolated cells at 9/13/17px")
            except BaseException:
                log.flush()
                print((folder / "stderr.log").read_text(encoding="utf-8", errors="replace")[-2500:])
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                fixture.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=lambda p: Path(p).resolve())
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--baseline", action="store_true")
    parser.add_argument("--native", action="store_true")
    parser.add_argument("--gpu", action="store_true")
    options = parser.parse_args()
    foreground = None
    if options.native and os.name == "nt":
        import ctypes
        from ctypes import wintypes
        user = ctypes.WinDLL("user32", use_last_error=True)
        user.GetForegroundWindow.restype = wintypes.HWND
        user.SetForegroundWindow.argtypes = [wintypes.HWND]
        foreground = user.GetForegroundWindow()
    try:
        run(options.binary, options.artifacts, options.baseline, options.native, options.gpu)
    finally:
        if foreground:
            user.SetForegroundWindow(foreground)
