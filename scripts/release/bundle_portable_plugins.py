#!/usr/bin/env python3
"""Add verified Windows plugins and offline update packages to a portable ZIP."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import tomllib
import zipfile
from pathlib import Path, PurePosixPath

from verify_native_package import archive_entry_bytes, verify_portable_archive


PROJECT_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_SELECTION = Path(__file__).with_name("windows-bundled-plugins.json")


def validate_payload(payload: Path, selection: Path, version: str, target: str) -> dict:
    packages = payload / "resources/plugin-packages"
    index = json.loads((packages / "index.json").read_text())
    if index["hostVersion"] != version or index["target"] != target:
        raise RuntimeError("plugin bundle and application target/version differ")
    selected_ids = {plugin["id"] for plugin in json.loads(selection.read_text())["plugins"]}
    records = index["plugins"]
    if len(records) != len(selected_ids) or {record["id"] for record in records} != selected_ids:
        raise RuntimeError("plugin bundle differs from the selected plugin IDs")
    for record in records:
        filename = record["file"]
        if Path(filename).name != filename or record["target"] not in (target, "any"):
            raise RuntimeError("invalid offline plugin package target or filename")
        package = packages / filename
        content = package.read_bytes()
        if len(content) != record["size"] or f"sha256:{hashlib.sha256(content).hexdigest()}" != record["checksum"]:
            raise RuntimeError(f"offline plugin package checksum differs: {record['id']}")
        installed = payload / "data/plugins" / record["id"]
        manifest = json.loads((installed / "plugin.json").read_text())
        if manifest["id"] != record["id"] or manifest["version"] != record["version"]:
            raise RuntimeError("installed plugin identity differs from the bundle index")
        with zipfile.ZipFile(package) as archive:
            files = [entry for entry in archive.infolist() if not entry.is_dir()]
            manifests = [entry.filename for entry in files if PurePosixPath(entry.filename).name == "plugin.json"]
            if len(manifests) != 1:
                raise RuntimeError("plugin archive must have one manifest")
            prefix = str(PurePosixPath(manifests[0]).parent)
            prefix = "" if prefix == "." else f"{prefix}/"
            expected_files = set()
            for entry in files:
                if not entry.filename.startswith(prefix):
                    raise RuntimeError("plugin archive has entries outside its root")
                relative = entry.filename.removeprefix(prefix)
                parts = PurePosixPath(relative).parts
                if not parts or ".." in parts or relative.startswith("/") or "\\" in relative or ":" in relative:
                    raise RuntimeError("invalid plugin archive path")
                expected_files.add(relative)
                if (installed / relative).read_bytes() != archive.read(entry):
                    raise RuntimeError(f"installed plugin file differs from its verified archive: {record['id']}")
            actual_files = {path.relative_to(installed).as_posix() for path in installed.rglob("*") if path.is_file()}
            if actual_files != expected_files:
                raise RuntimeError(f"installed plugin files differ from the archive: {record['id']}")
    installed_ids = {path.name for path in (payload / "data/plugins").iterdir()}
    if installed_ids != selected_ids:
        raise RuntimeError("installed plugin directories differ from the selection")
    return index


def bundle_payload(source: Path, payload: Path, output: Path) -> None:
    with zipfile.ZipFile(source) as archive:
        names = archive.namelist()
        roots = {PurePosixPath(name).parts[0] for name in names}
        if len(roots) != 1 or len(names) != len(set(names)):
            raise RuntimeError("portable archive must have one root and unique entries")
        root = roots.pop()
        if any(name.startswith(f"{root}/data/") and not name.endswith("/") for name in names):
            raise RuntimeError("source portable archive contains existing user data")
    additions = []
    for path in sorted(payload.rglob("*")):
        if path.is_symlink():
            raise RuntimeError("plugin payload contains a symbolic link")
        if not path.is_file():
            continue
        relative = path.relative_to(payload).as_posix()
        if not (
            relative.startswith("data/plugins/")
            or relative.startswith("data/plugin-catalog-histories/")
            or relative == "data/plugin-catalog-cache.json"
            or relative.startswith("resources/plugin-packages/")
        ):
            raise RuntimeError(f"unapproved plugin payload path: {relative}")
        destination = f"{root}/{relative}"
        if destination in names:
            raise RuntimeError(f"plugin payload would overwrite a package entry: {relative}")
        additions.append((path, destination))
    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(dir=output.parent, suffix=".zip")
    os.close(descriptor)
    temporary_path = Path(temporary)
    try:
        shutil.copyfile(source, temporary_path)
        # Appending preserves the already-built executable and its runtime payloads.
        with zipfile.ZipFile(temporary_path, "a", compression=zipfile.ZIP_DEFLATED) as archive:
            for path, destination in additions:
                archive.write(path, destination)
        os.replace(temporary_path, output)
    finally:
        temporary_path.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--target", choices=("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"), required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--selection", type=Path, default=DEFAULT_SELECTION)
    parser.add_argument("--payload", type=Path, help="Reuse an already verified package_plugins output directory")
    args = parser.parse_args()
    source = args.archive.resolve()
    output = args.output.resolve()
    if output.exists() or source == output:
        raise RuntimeError("output must be a new archive")
    workspace = tomllib.loads((PROJECT_ROOT / "Cargo.toml").read_text())
    version = workspace["workspace"]["package"]["version"]
    verify_portable_archive(source, args.target, version)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output.parent, prefix="plugin-bundle-") as directory:
        payload = args.payload.resolve() if args.payload else Path(directory) / "payload"
        if args.payload is None:
            subprocess.run([
                "cargo", "run", "--locked", "-p", "oxideterm-plugin-registry",
                "--example", "package_plugins", "--", args.target,
                str(args.selection.resolve()), str(payload),
            ], cwd=PROJECT_ROOT, check=True)
        validate_payload(payload, args.selection, version, args.target)
        bundle_payload(source, payload, output)
    verify_portable_archive(output, args.target, version)
    index = json.loads(archive_entry_bytes(output, "resources/plugin-packages/index.json"))
    print(f"Created {output.name} with {len(index['plugins'])} plugins for {args.target}")


if __name__ == "__main__":
    main()
