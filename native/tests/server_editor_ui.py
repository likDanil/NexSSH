"""Server editor regression against Slint MCP; all data and connections are isolated.

Build with SLINT_EMIT_DEBUG_INFO=1 and --features slint/mcp, then pass the binary.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

from settings_ui import Inspector, wait_for


def run(binary):
    with tempfile.TemporaryDirectory(prefix="nexssh-server-ui-") as folder:
        folder = Path(folder)
        legacy = json.dumps({"language": "en", "theme": "black", "autoUpdateCheck": False,
                             "agentsEnabled": False, "explorerMenu": False,
                             "closeAction": "quit", "sidebarAutoHide": False, "restoreTabs": "never"}).encode()
        (folder / "settings.json").write_bytes(legacy)
        original = {"id": "original", "name": "Example server", "host": "server.example",
                    "port": 22, "auth": "auto", "alias": "imported-alias", "lastUsed": 1234}
        (folder / "servers.json").write_text(json.dumps({"version": 1, "servers": [original]}))

        def servers():
            return json.loads((folder / "servers.json").read_text(encoding="utf-8"))["servers"]

        def start():
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            process = subprocess.Popen([str(binary)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                env=dict(os.environ, SLINT_BACKEND="headless", SLINT_MCP_PORT=str(port), NEXSSH_DATA_DIR=str(folder)),
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            try:
                return process, Inspector(port)
            except BaseException:
                process.terminate()
                process.wait(timeout=5)
                raise

        process, ui = start()
        try:
            def edit(label, value):
                handle = wait_for(lambda: ui.find(label, "TextInput"), f"Missing field: {label}")[0]
                ui.call("set_element_value", {"elementHandle": handle, "value": value})

            def menu(name, action):
                ui.press("Server actions: " + name)
                ui.press(action)

            ui.call("click_element", {"elementHandle": ui.find("Example server")[0], "button": "Right"})
            wait_for(lambda: ui.find("Edit…"), "Right click did not open server actions")
            assert not ui.find("Example server", "Tab"), "Right click initiated a connection"
            ui.press("Edit…")
            for label in ["General", "Sign-in", "Connection", "Ports", "Commands", "AI agents"]:
                assert ui.find(label, "Tab"), label
            edit("Port", "70000")
            ui.press("Save")
            assert servers()[0] == original, "Invalid port modified the server"
            edit("Port", "2222")
            edit("Name", "Edited server")
            edit("Host", "changed.example")
            edit("Username", "ops")
            edit("Group", "Production")
            edit("Files folder", "~/www")
            ui.press("Sign-in", "Tab")
            ui.press("Key")
            ui.press("Save")
            assert ui.find("Private key", "TextInput"), "Missing key did not select Login"
            edit("Private key", "~/.ssh/id_ed25519")
            ui.press("Connection", "Tab")
            edit("Jump host", "jump-ops@jump.example:2222")
            wait_for(lambda: ui.find("Jump host login", "TextInput"), "Typed jump host did not expose login")
            edit("Keepalive, s", "0")
            edit("Timeout, s", "42")
            ui.press("Ports", "Tab")
            ui.press("Add forwarding rule")
            edit("Local port 1", "8080")
            edit("Host 1", "localhost")
            edit("Port 1", "80")
            ui.press("Add forwarding rule")
            ui.choose("Port forwarding 2", "Remote")
            edit("Port on the server 2", "0")
            edit("Host 2", "localhost")
            edit("Port 2", "443")
            ui.press("Add forwarding rule")
            ui.choose("Port forwarding 3", "SOCKS")
            edit("Local port 3", "1080")
            ui.press("Commands", "Tab")
            edit("Commands on login", "cd /var/www\n\ntmux new -A -s main")
            ui.press("AI agents", "Tab")
            ui.press("Ask first")
            ui.press("Save")
            wait_for(lambda: servers()[0]["name"] == "Edited server", "Edits were not persisted")
            saved = servers()[0]
            assert saved["id"] == "original" and saved["alias"] == "imported-alias" and saved["lastUsed"] == 1234
            assert (saved["host"], saved["port"], saved["user"], saved["group"]) == ("changed.example", 2222, "ops", "Production")
            assert saved["auth"] == "key" and saved["identityFile"] == "~/.ssh/id_ed25519"
            assert saved["jumpHost"] == "jump.example:2222" and saved["jumpUser"] == "jump-ops"
            assert saved["keepaliveSecs"] == 0 and saved["connectTimeoutSecs"] == 42
            assert saved["sftpRoot"] == "~/www" and saved["agents"] == "ask"
            assert saved["startupCommands"] == "cd /var/www\ntmux new -A -s main"
            assert [f["kind"] for f in saved["forwards"]] == ["local", "remote", "dynamic"]
            menu("Edited server", "Edit…")
            edit("Name", "Discard this change")
            ui.press("Delete")
            ui.press("Cancel")
            assert servers()[0] == saved, "Cancelled deletion changed data"
            ui.press("Cancel")
            menu("Edited server", "Duplicate")
            wait_for(lambda: len(servers()) == 2, "Duplicate did not create a server")
            copy = next(s for s in servers() if s["id"] != "original")
            assert copy["name"] == "Edited server copy" and "alias" not in copy and "lastUsed" not in copy
            menu(copy["name"], "Edit…")
            ui.press("Delete")
            ui.press("Delete")
            wait_for(lambda: len(servers()) == 1, "Confirmed deletion did not remove the copy")
            ui.press("Add Server")
            edit("Host", "127.0.0.1")
            edit("Name", "Loopback test")
            # Closed loopback port: exercise Save & Connect without contacting a real server.
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                edit("Port", str(sock.getsockname()[1]))
            ui.press("Save & Connect")
            wait_for(lambda: ui.find("Loopback test", "Tab"), "Save & Connect did not open a session")
            count = len(ui.find("Loopback test", "Tab"))
            menu("Loopback test", "Go to session")
            assert len(ui.find("Loopback test", "Tab")) == count, "Go to session duplicated the tab"
            menu("Loopback test", "New session")
            wait_for(lambda: len(ui.find("Loopback test", "Tab")) > count, "New session did not open another tab")
            count = len(ui.find("Loopback test", "Tab"))
            menu("Loopback test", "Delete")
            ui.press("Delete")
            wait_for(lambda: len(servers()) == 1, "Deletion failed with open sessions")
            assert len(ui.find("Loopback test", "Tab")) == count, "Deletion closed existing sessions"
            ui.press("Close")
            process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
        assert (folder / "settings.json").read_bytes() == legacy
        process, ui = start()
        try:
            ui.press("Server actions: Edited server")
            ui.press("Edit…")
            assert servers()[0] == saved
            ui.press("Cancel")
            ui.press("Close")
            process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
        print("PASS: six tabs, validation, editing, forwarding, commands, duplication, deletion, sessions and restart")


if __name__ == "__main__":
    default = "target/debug/nexssh-slint" + (".exe" if os.name == "nt" else "")
    run(Path(sys.argv[1] if len(sys.argv) > 1 else default).resolve())
