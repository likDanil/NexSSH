import base64
import tempfile
import unittest
from pathlib import Path
from native_release import manifest, plan


class NativeReleaseTests(unittest.TestCase):
    def test_preview_never_writes_stable_feed(self):
        self.assertEqual(plan("2.0.0-alpha.1")["manifest"], "preview.json")
        self.assertEqual(plan("2.0.0")["manifest"], "latest.json")
        for version in ["1.5.2", "2.0.0+test", "2.0.0-alpha.0", "2.0.0-alpha.01"]:
            with self.assertRaises(ValueError):
                plan(version)
        with self.assertRaises(ValueError):
            plan("2.0.0-alpha.1", "v2.0.0")

    def test_manifest_and_signed_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            version = "2.0.0-alpha.2"
            exe = directory / f"NexSSH_{version}_x64-setup.exe"
            exe.write_bytes(b"test installer")
            signature = exe.with_suffix(".exe.sig")
            signature.write_text(base64.b64encode(b"trusted comment: version:2.0.0-alpha.2\n").decode())
            self.assertEqual(manifest(directory, version, "likDanil/NexSSH").name, "preview.json")
            self.assertFalse((directory / "latest.json").exists())
            signature.write_text(base64.b64encode(b"trusted comment: version:2.0.0-alpha.1\n").decode())
            with self.assertRaises(ValueError):
                manifest(directory, version, "likDanil/NexSSH")


if __name__ == "__main__":
    unittest.main()
