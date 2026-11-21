"""Release metadata shared by CI and local dry runs; never publishes anything."""
import argparse
import base64
import datetime
import json
from pathlib import Path
import re
import tomllib
from urllib.parse import quote

VERSION = re.compile(r"2\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(alpha|beta|rc)\.([1-9]\d*))?\Z")


def plan(version, tag=None):
    if not VERSION.fullmatch(version):
        raise ValueError(f"Unsupported native release version: {version}")
    if tag and tag != f"v{version}":
        raise ValueError(f"Tag {tag} differs from native/Cargo.toml ({version})")
    preview = "-" in version
    return {"version": version, "tag": f"v{version}",
            "channel": "preview" if preview else "stable",
            "manifest": "preview.json" if preview else "latest.json"}


def manifest(directory, version, repository):
    metadata = plan(version)
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("Invalid GitHub repository")
    directory = Path(directory)
    installers = list(directory.glob("*-setup.exe"))
    if len(installers) != 1:
        raise ValueError("Expected exactly one installer")
    installer = installers[0]
    expected = f"NexSSH_{version}_x64-setup.exe"
    if installer.name != expected:
        raise ValueError(f"Expected {expected}")
    signature = installer.with_suffix(".exe.sig").read_text().strip()
    # Cryptographic verification is performed by the built client's --verify-update command.
    decoded = base64.b64decode(signature, validate=True).decode()
    trusted = next((line for line in decoded.splitlines() if line.startswith("trusted comment: ")), "")
    if f"version:{version}" not in trusted.removeprefix("trusted comment: ").split("\t"):
        raise ValueError("Installer signature does not bind the release version")
    artifact = {"signature": signature,
                "url": f"https://github.com/{repository}/releases/download/{metadata['tag']}/{quote(installer.name)}"}
    value = {"version": version, "notes": f"NexSSH {version}",
             "pub_date": datetime.datetime.now(datetime.timezone.utc).isoformat(),
             "platforms": {"windows-x86_64": artifact, "windows-x86_64-nsis": artifact}}
    other = directory / ("latest.json" if metadata["channel"] == "preview" else "preview.json")
    if other.exists():
        raise ValueError(f"Mixed update channels in {directory}")
    destination = directory / metadata["manifest"]
    destination.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["plan", "manifest"])
    parser.add_argument("--version")
    parser.add_argument("--tag")
    parser.add_argument("--directory", default="target/native-dist")
    parser.add_argument("--repository", default="likDanil/NexSSH")
    args = parser.parse_args()
    version = args.version or tomllib.loads(Path("native/Cargo.toml").read_text())["package"]["version"]
    metadata = plan(version, args.tag)
    if args.command == "plan":
        for key, value in metadata.items():
            print(f"{key}={value}")
    else:
        print(manifest(args.directory, version, args.repository))


if __name__ == "__main__":
    main()
