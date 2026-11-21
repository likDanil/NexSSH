"""Signed updater UI regression. Requires an MCP build with the fixture public key.

The test never accepts installation. All HTTP traffic and app data are local fixtures.
See docs/releases-2.x.md for the build command.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
from settings_ui import Inspector, wait_for


def run(binary):
    fixtures = Path(__file__).parent / "fixtures/updater"
    payload = (fixtures / "payload.txt").read_bytes()
    signature = (fixtures / "payload.txt.sig").read_text().strip()
    state = {"mode": "valid"}

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == "/update.exe":
                data = b"tampered" if state["mode"] == "tampered" else payload
            else:
                value = {"version": "2.0.0-alpha.3" if state["mode"] == "replay" else "2.0.0-alpha.2",
                         "platforms": {"windows-x86_64-nsis": {
                             "url": f"http://127.0.0.1:{server.server_port}/update.exe", "signature": signature}}}
                data = json.dumps(value).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        for mode in ["valid", "automatic", "tampered", "replay"]:
            state["mode"] = mode
            with tempfile.TemporaryDirectory(prefix="nexssh-updater-ui-") as folder:
                folder = Path(folder)
                (folder / "settings.json").write_text(json.dumps({
                    "language": "ru", "theme": "black", "autoUpdateCheck": mode == "automatic",
                    "agentsEnabled": False, "explorerMenu": False, "closeAction": "quit", "restoreTabs": "never"}))
                with socket.socket() as sock:
                    sock.bind(("127.0.0.1", 0))
                    port = sock.getsockname()[1]
                environment = dict(os.environ, SLINT_BACKEND="headless", SLINT_MCP_PORT=str(port),
                    NEXSSH_DATA_DIR=str(folder), NEXSSH_UPDATE_URL=f"http://127.0.0.1:{server.server_port}/manifest.json")
                process = subprocess.Popen([str(binary)], env=environment, stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
                try:
                    ui = Inspector(port)
                    if mode == "automatic":
                        wait_for(lambda: ui.find("Скачать"), "Automatic check did not show an update on the main screen")
                        ui.press("Скачать")
                        wait_for(lambda: ui.find("Установить"), "Main-screen download did not become ready")
                    ui.press("Настройки")
                    ui.press("О программе", "Tab")
                    if mode != "automatic":
                        ui.press("Проверить обновления")
                        wait_for(lambda: ui.find("Скачать"), "New signed release was not offered")
                        ui.press("Скачать")
                    if mode in ["valid", "automatic"]:
                        wait_for(lambda: ui.find("Установить"), "Verified update did not become ready")
                        ui.press("Установить")
                        ui.press("Отмена")
                        assert process.poll() is None, "Cancelling installation closed the application"
                        assert ui.find("Установить"), "Cancelling discarded the verified update"
                    else:
                        wait_for(lambda: ui.find("Проверить обновления"), "Invalid update did not return to a retryable state")
                        assert not ui.find("Установить"), "An unverified update can be installed"
                    # Close our own headless app through its normal shutdown path.
                    ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\x1b"})
                    ui.press("Закрыть")
                    process.wait(timeout=5)
                    assert process.returncode == 0
                finally:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)
        print("PASS: signed download, install cancellation, tamper/replay rejection and retry state")
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    run(Path(sys.argv[1]).resolve())
