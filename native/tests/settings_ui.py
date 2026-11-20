"""Settings regression test against Slint's optional MCP inspector, using isolated data.

Build first: cargo build -p nexssh-slint --features slint/mcp
Run: python native/tests/settings_ui.py [path/to/nexssh-slint.exe]
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request


def wait_for(check, message, seconds=8):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except (OSError, ValueError, KeyError):
            pass
        time.sleep(0.04)
    raise AssertionError(message)


class Inspector:
    def __init__(self, port):
        self.url = f"http://127.0.0.1:{port}/mcp"
        windows = wait_for(lambda: self.call("list_windows", {})["windowHandles"],
                           "Slint MCP unavailable; build with --features slint/mcp")
        self.window=windows[0]
        self.root = self.call("get_window_properties", {"windowHandle": self.window})["rootElementHandle"]

    def call(self, name, arguments):
        body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": name, "arguments": arguments}}
        request = urllib.request.Request(self.url, json.dumps(body).encode(),
                                         headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=3) as response:
            result = json.load(response)["result"]
        assert not result.get("isError"), result
        content = result.get("content", [])
        return json.loads(content[0]["text"]) if content else None

    def find(self, label, role="Button"):
        return self.call("query_element_descendants", {
            "elementHandle": self.root, "queryStack": [{"matchDescendants": True},
                {"matchElementAccessibleRole": role}, {"matchElementAccessibleLabel": label}],
            "findAll": True}).get("elementHandles", [])

    def press(self, label, role="Button", action="Default_"):
        element = wait_for(lambda: self.find(label, role), f"Missing {role}: {label}")[0]
        self.call("invoke_accessibility_action", {"elementHandle": element, "action": action})
        time.sleep(0.05)

    def choose(self, label, option):
        self.press(label, "Combobox", "Expand")
        self.press(option)


def run(binary):
    with tempfile.TemporaryDirectory(prefix="nexssh-settings-ui-") as directory:
        directory = Path(directory)
        legacy = json.dumps({"language": "ru", "theme": "light", "autoUpdateCheck": False,
                             "agentsEnabled": False, "explorerMenu": False,
                             "closeAction": "quit", "restoreTabs": "never",
                             "futureField": {"preserved": True}}).encode()
        (directory / "settings.json").write_bytes(legacy)
        def saved():
            return json.loads((directory / "native-settings.json").read_text(encoding="utf-8"))

        def start():
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            environment = dict(os.environ, SLINT_BACKEND="headless", SLINT_MCP_PORT=str(port),
                               NEXSSH_DATA_DIR=str(directory))
            process = subprocess.Popen([str(binary)], env=environment, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            try:
                return process, Inspector(port)
            except BaseException:
                process.terminate()
                process.wait(timeout=5)
                raise

        process, ui = start()
        try:
            ui.press("Настройки")
            ui.press("English")
            wait_for(lambda: saved()["language"] == "en", "Language choice was not saved")
            ui.press("Terminal", "Tab")
            ui.press("Block")
            wait_for(lambda: saved()["cursorStyle"] == "block", "Cursor choice was not saved")
            ui.press("Blinking cursor", "Switch")
            ui.press("General", "Tab")
            for _ in range(4):ui.press("Larger")
            wait_for(lambda: saved()["fontSize"] == 17 and saved()["cursorBlink"], "Changes were not persisted")
            ui.press("Terminal","Tab")
            swatch=ui.find("Error")[0]
            scrollbar=ui.find("Settings","Slider")[0]
            swatch_bounds=ui.call("get_element_properties",{"elementHandle":swatch})
            scrollbar_bounds=ui.call("get_element_properties",{"elementHandle":scrollbar})
            assert scrollbar_bounds["absolutePosition"]["x"]>swatch_bounds["absolutePosition"]["x"]+swatch_bounds["size"]["width"],"Settings scrollbar overlaps controls"
            ui.call("invoke_accessibility_action",{"elementHandle":scrollbar,"action":"Increment"})
            moved=ui.call("get_element_properties",{"elementHandle":scrollbar})
            assert float(moved["accessibleValue"])>float(scrollbar_bounds["accessibleValue"]),"Settings scrollbar does not scroll"
            ui.call("invoke_accessibility_action",{"elementHandle":scrollbar,"action":"Decrement"})
            ui.press("Error")
            color=wait_for(lambda:ui.find("Hex","TextInput"),"Colour picker did not open")[0]
            ui.call("set_element_value",{"elementHandle":color,"value":"8686fe"})
            wait_for(lambda:saved()["highlightColors"]["error"]=="#8686fe","Colour edit was not applied")
            red=ui.find("R","TextInput")[0]
            ui.call("set_element_value",{"elementHandle":red,"value":"100"})
            wait_for(lambda:saved()["highlightColors"]["error"]=="#6486fe","RGB channel edit was not applied")
            ui.call("set_element_value",{"elementHandle":color,"value":"8686fe"})
            wait_for(lambda:saved()["highlightColors"]["error"]=="#8686fe","Hex edit did not restore the colour")
            ui.press("Save the color")
            wait_for(lambda:"#8686fe" in saved()["savedColors"],"Saved colour was not persisted")
            ui.call("dispatch_key_event",{"windowHandle":ui.window,"text":"\x1b"})
            assert ui.find("General","Tab"),"Closing a colour popup must keep Settings open"
            for label in ["Files", "Keyboard", "AI agents", "Backup", "About"]:
                ui.press(label, "Tab")
            ui.call("dispatch_key_event",{"windowHandle":ui.window,"text":"\x1b"})
            ui.press("Hide sidebar")
            reveal=wait_for(lambda:ui.find("Show sidebar"),"Missing button to reopen sidebar")[0]
            properties=ui.call("get_element_properties",{"elementHandle":reveal})
            assert properties["absolutePosition"]["x"]<30 and properties["computedOpacity"]==1
            ui.press("Show sidebar")
            ui.press("Close")
            process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
        assert (directory / "settings.json").read_bytes() == legacy
        assert saved()["futureField"] == {"preserved": True}
        process, ui = start()
        try:
            ui.press("Settings")
            assert saved()["cursorStyle"] == "block" and saved()["fontSize"] == 17
            ui.call("dispatch_key_event",{"windowHandle":ui.window,"text":"\x1b"})
            ui.press("Close")
            process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
        print("PASS: settings, colours, sidebar reveal, seven sections, persistence and restart; legacy data preserved")


if __name__ == "__main__":
    default="target/debug/nexssh-slint"+(".exe" if os.name=="nt" else "")
    run(Path(sys.argv[1] if len(sys.argv) > 1 else default).resolve())
