import json
import hashlib
from pathlib import Path
import sys
import tempfile
import unittest
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))

from bundle_portable_plugins import bundle_payload, validate_payload


class PortablePluginBundleTests(unittest.TestCase):
    def test_payload_rejects_modified_installed_files_and_offline_archives(self):
        for modified, expected_error in [
            ("installed", "installed plugin file differs"),
            ("archive", "offline plugin package checksum differs"),
        ]:
            with self.subTest(modified=modified), tempfile.TemporaryDirectory() as directory:
                payload = Path(directory)
                installed = payload / "data/plugins/com.example.language"
                packages = payload / "resources/plugin-packages"
                installed.mkdir(parents=True)
                packages.mkdir(parents=True)
                manifest = b'{"id":"com.example.language","version":"0.1.0"}'
                (installed / "plugin.json").write_bytes(manifest)
                (installed / "parser.wasm").write_bytes(b"original parser fixture")
                package = packages / "language.zip"
                with zipfile.ZipFile(package, "w") as archive:
                    archive.writestr("plugin.json", manifest)
                    archive.writestr("parser.wasm", b"original parser fixture")
                content = package.read_bytes()
                (packages / "index.json").write_text(json.dumps({
                    "hostVersion": "2.2.2", "target": "x86_64-pc-windows-msvc",
                    "plugins": [{
                        "id": "com.example.language", "version": "0.1.0", "target": "any",
                        "file": "language.zip", "size": len(content),
                        "checksum": f"sha256:{hashlib.sha256(content).hexdigest()}",
                    }],
                }))
                selection = payload / "selection.json"
                selection.write_text('{"plugins":[{"id":"com.example.language"}]}')
                if modified == "installed":
                    (installed / "parser.wasm").write_bytes(b"changed parser fixture")
                else:
                    package.write_bytes(content + b"changed archive fixture")
                with self.assertRaisesRegex(RuntimeError, expected_error):
                    validate_payload(payload, selection, "2.2.2", "x86_64-pc-windows-msvc")

    def test_bundle_keeps_app_and_update_ownership_and_adds_installed_and_offline_packages(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            source = base / "source.zip"
            manifest = b'{"managedEntries":["oxideterm-native.exe","resources","tools"]}'
            original = {
                "OxideTerm/oxideterm-native.exe": b"original Windows executable",
                "OxideTerm/portable-update.json": manifest,
                "OxideTerm/data/plugins/": b"",
            }
            with zipfile.ZipFile(source, "w") as archive:
                for name, value in original.items():
                    archive.writestr(name, value)
            payload = base / "payload"
            files = {
                "data/plugins/com.example.language/plugin.json": b'{"id":"com.example.language","version":"0.1.0"}',
                "resources/plugin-packages/language.zip": b"verified offline plugin package",
                "resources/plugin-packages/index.json": b'{"plugins":[{"id":"com.example.language","version":"0.1.0"}]}',
            }
            for name, value in files.items():
                path = payload / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(value)
            output = base / "bundled.zip"
            bundle_payload(source, payload, output)
            with zipfile.ZipFile(output) as archive:
                self.assertEqual(set(archive.namelist()), set(original) | {f"OxideTerm/{name}" for name in files})
                for name, value in original.items():
                    self.assertEqual(archive.read(name), value, name)
                for name, value in files.items():
                    self.assertEqual(archive.read(f"OxideTerm/{name}"), value, name)
                self.assertEqual(json.loads(archive.read("OxideTerm/portable-update.json")), {
                    "managedEntries": ["oxideterm-native.exe", "resources", "tools"],
                })

    def test_rejects_profile_data_in_source_or_staged_payload_before_writing_output(self):
        for location in ("source", "payload"):
            with self.subTest(location=location), tempfile.TemporaryDirectory() as directory:
                base = Path(directory)
                source = base / "source.zip"
                with zipfile.ZipFile(source, "w") as archive:
                    archive.writestr("OxideTerm/oxideterm-native.exe", b"app")
                    if location == "source":
                        archive.writestr("OxideTerm/data/connections.json", b"private connection fixture")
                payload = base / "payload"
                payload.mkdir()
                if location == "payload":
                    path = payload / "data/plugin-config.json"
                    path.parent.mkdir()
                    path.write_bytes(b"private plugin settings fixture")
                output = base / "bundled.zip"
                with self.assertRaisesRegex(RuntimeError, "existing user data|unapproved plugin payload"):
                    bundle_payload(source, payload, output)
                self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
