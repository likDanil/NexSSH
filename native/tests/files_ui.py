"""FilesDrawer.svelte visual/interaction regression over isolated loopback SFTP.

Use the optional Slint MCP build and test-only paramiko dependency (see README).
All filesystem mutations are confined to TemporaryDirectory, never saved servers.
"""
import argparse
import base64
import errno
import json
import os
from pathlib import Path
import posixpath
import stat
import subprocess
import tempfile
import time
import urllib.request

import paramiko

from metrics_ui import Fixture
from settings_ui import Inspector, wait_for


class Filesystem(paramiko.SFTPServerInterface):
    def __init__(self, server, root):
        super().__init__(server)
        self.root = root
        self.modes = {}

    def canonicalize(self, path):
        if not path.startswith("/"):
            path = "/home/test/" + path
        return posixpath.normpath(path)

    def local(self, path):
        remote = self.canonicalize(path)
        if remote == "/home/test/current":
            remote = "/home/test/docs"
        result = (self.root / remote.lstrip("/")).resolve()
        if not result.is_relative_to(self.root.resolve()):
            raise PermissionError(errno.EACCES, "Outside fixture")
        return result

    def attrs(self, path):
        attributes = paramiko.SFTPAttributes.from_stat(self.local(path).stat())
        if path in self.modes:
            attributes.st_mode = stat.S_IFMT(attributes.st_mode) | self.modes[path]
        return attributes

    def list_folder(self, path):
        try:
            entries = []
            for child in self.local(path).iterdir():
                attributes = self.attrs(posixpath.join(path, child.name))
                attributes.filename = child.name
                entries.append(attributes)
            if self.canonicalize(path) == "/home/test":
                attributes = self.attrs("/home/test/docs")
                attributes.filename = "current"
                attributes.st_mode = stat.S_IFLNK | 0o777
                entries.append(attributes)
            return entries
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def stat(self, path):
        try:
            return self.attrs(path)
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def lstat(self, path):
        attributes = self.stat(path)
        if self.canonicalize(path) == "/home/test/current" and isinstance(attributes, paramiko.SFTPAttributes):
            attributes.st_mode = stat.S_IFLNK | 0o777
        return attributes

    def readlink(self, path):
        return "docs"

    def open(self, path, flags, attributes):
        try:
            descriptor = os.open(self.local(path), flags, 0o644)
            mode = "r+b" if flags & os.O_RDWR else "wb" if flags & os.O_WRONLY else "rb"
            file = os.fdopen(descriptor, mode)
            handle = paramiko.SFTPHandle(flags)
            handle.readfile = file
            handle.writefile = file
            return handle
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def mkdir(self, path, attributes):
        return self.operation(lambda: self.local(path).mkdir())

    def remove(self, path):
        return self.operation(lambda: self.local(path).unlink())

    def rmdir(self, path):
        return self.operation(lambda: self.local(path).rmdir())

    def rename(self, old, new):
        return self.operation(lambda: self.local(old).rename(self.local(new)))

    def chattr(self, path, attributes):
        if attributes.st_mode is not None:
            self.modes[path] = attributes.st_mode & 0o7777
        return paramiko.SFTP_OK

    @staticmethod
    def operation(callback):
        try:
            callback()
            return paramiko.SFTP_OK
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)


class Server(Fixture):
    def __init__(self, root):
        self.root = root
        super().__init__()

    def serve(self, transport):
        transport.set_subsystem_handler("sftp", paramiko.SFTPServer, Filesystem, root=self.root)
        super().serve(transport)


def run(binary, artifacts, native=False):
    with tempfile.TemporaryDirectory(prefix="nexssh-files-ui-") as directory:
        directory = Path(directory)
        remote = directory / "remote"
        home = remote / "home/test"
        (home / "docs").mkdir(parents=True)
        (home / "logs").mkdir()
        (home / "docs/notes.txt").write_text("unchanged fixture", encoding="utf-8")
        (home / "readme.txt").write_text("fixture README", encoding="utf-8")
        (home / "app.log").write_bytes(b"x" * 4096)
        (home / ".env").write_text("FAKE_FIXTURE=true")
        (home / "Очень длинное название файла для проверки.txt").write_text("unicode fixture", encoding="utf-8")
        for number in range(25):
            (home / f"report-{number:02}.txt").write_text(str(number))
        fixture = Server(remote)
        profile = directory / "profile"
        profile.mkdir()
        (profile / "settings.json").write_text(json.dumps({"language": "en", "theme": "black",
            "agentsEnabled": False, "autoUpdateCheck": False, "explorerMenu": False,
            "closeAction": "quit", "restoreTabs": "never", "sidebarAutoHide": False,
            "filesOpen": True, "filesWidth": 380}))
        (profile / "servers.json").write_text(json.dumps({"version": 1, "servers": [{
            "id": "fixture", "name": "SFTP fixture", "host": "127.0.0.1", "port": fixture.port,
            "user": "first", "auth": "auto"}]}))
        import socket
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        env = dict(os.environ, NEXSSH_DATA_DIR=str(profile), SLINT_BACKEND="headless", SLINT_MCP_PORT=str(port))
        if native:
            env.pop("SLINT_BACKEND", None)
        with (directory / "app.log").open("w", encoding="utf-8") as log:
            process = subprocess.Popen([str(binary)], env=env, stdin=subprocess.DEVNULL,
                stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            try:
                ui = Inspector(port)
                ui.press("SFTP fixture")
                ui.press("Доверять ключу")
                wait_for(lambda: ui.find("readme.txt", "ListItem"), "SFTP listing did not arrive", seconds=15)

                def prop(handle):
                    return ui.call("get_element_properties", {"elementHandle": handle})

                def double_click(handle):
                    ui.call("click_element", {"elementHandle": handle, "action":"DoubleClick", "button":"Left"})

                def row(name):
                    return wait_for(lambda: ui.find(name, "ListItem"), f"Missing row: {name}")[0]

                def menu(name, choice):
                    ui.call("click_element", {"elementHandle": row(name), "button": "Right"})
                    ui.press(choice)

                def set_field(label, value):
                    handle = wait_for(lambda: ui.find(label, "TextInput"), f"Missing field: {label}")[0]
                    ui.call("set_element_value", {"elementHandle": handle, "value": value})
                    return handle

                def snap(name):
                    if artifacts is None:
                        return
                    artifacts.mkdir(parents=True, exist_ok=True)
                    payload = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                        "name": "take_screenshot", "arguments": {"windowHandle": ui.window}}}
                    request = urllib.request.Request(ui.url, json.dumps(payload).encode(), headers={"Content-Type": "application/json"})
                    with urllib.request.urlopen(request) as response:
                        content = json.load(response)["result"]["content"]
                    for block in content:
                        if block["type"] == "image":
                            (artifacts / f"{name}.png").write_bytes(base64.b64decode(block["data"]))

                assert ui.find("Name") and ui.find("Size") and ui.find("Modified")
                assert not ui.find(".env", "ListItem")
                double_click(row("docs"))
                row("notes.txt")
                ui.call("click_element", {"elementHandle": ui.find("Files", "Groupbox")[0], "button":"Right"})
                ui.press("New file")
                set_field("Name", "new.txt")
                ui.press("Create")
                wait_for(lambda: (home / "docs/new.txt").is_file(), "New file was not created")
                row("new.txt")
                ui.call("click_element", {"elementHandle": ui.find("Files", "Groupbox")[0], "button":"Right"})
                ui.press("New file")
                set_field("Name", "notes.txt")
                ui.press("Create")
                wait_for(lambda: ui.find("Закрыть уведомление"), "Creating an existing file did not report an error")
                assert (home / "docs/notes.txt").read_text() == "unchanged fixture", "New file overwrote existing contents"
                ui.press("Закрыть уведомление")
                ui.press("Up one level")
                row("readme.txt")
                snap("files-en")
                assert prop(row("docs"))["size"]["height"] == 28
                assert abs(prop(row("logs"))["absolutePosition"]["y"] - prop(row("docs"))["absolutePosition"]["y"]) == 28
                snap("files-en")
                ui.call("click_element", {"elementHandle": row("readme.txt"), "button": "Right"})
                snap("file-menu")
                assert ui.find("Copy path") and ui.find("Permissions…")
                ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\x1b"})

                ui.press("Size")
                wait_for(lambda: json.loads((profile / "native-settings.json").read_text())["filesSort"] == "size", "Sort was not saved")
                ui.press("Upload")
                assert ui.find("Upload files…") and ui.find("Upload folder…")
                snap("upload-menu")
                ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\x1b"})

                ui.press("New folder")
                set_field("Name", "new directory")
                ui.press("Create")
                wait_for(lambda: (home / "new directory").is_dir(), "New folder was not created")
                row("new directory")
                menu("new directory", "Rename…")
                set_field("Name", "renamed")
                ui.press("Rename")
                wait_for(lambda: (home / "renamed").is_dir(), "Rename failed")
                row("renamed")
                menu("renamed", "Delete")
                ui.press("Cancel")
                assert (home / "renamed").exists(), "Cancel deleted the folder"
                menu("renamed", "Delete")
                ui.press("Delete")
                wait_for(lambda: not (home / "renamed").exists(), "Confirmed deletion failed")
                wait_for(lambda: not ui.find("renamed", "ListItem"), "Listing did not finish after delete")

                double_click(row("docs"))
                row("notes.txt")
                ui.press("Up one level")
                row("readme.txt")
                menu("readme.txt", "Permissions…")
                set_field("Numeric value", "888")
                ui.press("Apply")
                assert ui.find("Numeric value", "TextInput"), "Invalid mode closed the form"
                set_field("Numeric value", "640")
                ui.press("Apply")
                row("readme.txt")

                # Failed paths stay inline with Retry, without a request loop.
                path = set_field("Path", "/missing")
                ui.call("click_element", {"elementHandle": path})
                ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\n"})
                wait_for(lambda: ui.find("Retry"), "Listing error has no inline retry")
                snap("files-error")
                ui.press("Retry")
                row("readme.txt")
                ui.press("Settings")
                ui.press("Русский")
                ui.call("dispatch_key_event", {"windowHandle": ui.window, "text": "\x1b"})
                assert ui.find("Имя") and ui.find("Размер") and ui.find("Изменён")
                snap("files-ru")

                # The 1.x resize limits and breakpoint for dates.
                handles = ui.call("query_element_descendants", {"elementHandle": ui.root,
                    "queryStack": [{"matchDescendants": True}, {"matchElementId": "FilesDrawer::resizer"}], "findAll": True})["elementHandles"]
                handle = handles[0]
                bounds = prop(handle)
                ui.call("drag_element", {"elementHandle": handle, "target": {
                    "x": bounds["absolutePosition"]["x"] + 83, "y": bounds["absolutePosition"]["y"] + 20}})
                wait_for(lambda: not ui.find("Изменён"), "Narrow drawer did not hide date column")
                snap("files-narrow")
                ui.call("click_element", {"elementHandle": handle, "action": "DoubleClick"})
                wait_for(lambda: ui.find("Изменён"), "Double-click did not restore 380px width")
                assert (home / "readme.txt").read_text() == "fixture README"
                ui.press("Закрыть")
                assert process.wait(timeout=8) == 0
                print("PASS: loopback SFTP, compact header/table, row height, sorting, menus, localized dates, folder navigation, exclusive file creation, rename/delete confirmation, permission validation, retry, resize/breakpoint")
            except BaseException:
                if 'ui' in locals():
                    entries = ui.call("query_element_descendants", {"elementHandle": ui.root, "queryStack": [{"matchDescendants": True}, {"matchElementAccessibleRole": "ListItem"}], "findAll": True}).get("elementHandles", [])
                    names = list(dict.fromkeys(ui.call("get_element_properties", {"elementHandle": h}).get("accessibleLabel", "") for h in entries))
                    print("Visible rows:", names)
                log.flush()
                print((directory / "app.log").read_text(encoding="utf-8", errors="replace")[-5000:])
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                fixture.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=lambda p: Path(p).resolve())
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--native", action="store_true")
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
        run(options.binary, options.artifacts, options.native)
    finally:
        if foreground:
            user.SetForegroundWindow(foreground)
