#!/usr/bin/env python3
"""Package golden inputs once, then verify the same bytes on each capture runner."""

import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile

FAMILIES = {"vulkan-lavapipe": "", "dx12-warp": ".exe"}
FORMAT = 1


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def safe_name(name: str) -> bool:
    path = PurePosixPath(name)
    return (
        bool(name)
        and not path.is_absolute()
        and ".." not in path.parts
        and "\\" not in name
        and ":" not in name
        and str(path) == name
        and len(path.parts) > 1
        and path.parts[0] in {"bin", "content"}
    )


def create(workspace: Path, archive: Path, commit: str, family: str) -> dict:
    suffix = FAMILIES[family]
    files = {}
    for executable in ("sapphire_sandbox", "sapphire_testkit"):
        name = executable + suffix
        files[f"bin/{name}"] = workspace / "target" / "debug" / name
    if family == "dx12-warp":
        files["bin/d3d10warp.dll"] = workspace / "target/debug/d3d10warp.dll"
        symbols = workspace / "target/debug/sapphire_sandbox.pdb"
        if symbols.exists():
            files["bin/sapphire_sandbox.pdb"] = symbols
    content = workspace / "target/content/desktop"
    if not (content / "shared.spack").is_file():
        raise ValueError("The completed desktop cook must contain shared.spack")
    for path in sorted(content.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"Symbolic links are not golden inputs: {path}")
        if path.is_file():
            files[f"content/{path.relative_to(content).as_posix()}"] = path
    records = {}
    for name, path in files.items():
        if not safe_name(name) or not path.is_file() or path.is_symlink():
            raise ValueError(f"Missing or invalid golden input: {path}")
        records[name] = {
            "bytes": path.stat().st_size,
            "sha256": digest(path),
            # Artifact transport loses filesystem modes; the inner tar restores these.
            "mode": 0o755 if name in {f"bin/sapphire_sandbox{suffix}", f"bin/sapphire_testkit{suffix}"} else 0o644,
        }
    manifest = {"format": FORMAT, "commit": commit, "family": family, "files": records}
    encoded = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()
    archive.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, "w:gz", compresslevel=1) as target:
        metadata = tarfile.TarInfo("manifest.json")
        metadata.size = len(encoded)
        metadata.mode = 0o644
        target.addfile(metadata, io.BytesIO(encoded))
        for name, path in files.items():
            entry = tarfile.TarInfo(name)
            entry.size = records[name]["bytes"]
            entry.mode = records[name]["mode"]
            with path.open("rb") as source:
                target.addfile(entry, source)
    print(f"Golden bundle: {family}, {commit}, {len(records)} files, {archive.stat().st_size} compressed bytes")
    print(f"Manifest SHA256: {hashlib.sha256(encoded).hexdigest()}")
    for name, record in records.items():
        print(f"Input {name}: {record['bytes']} bytes, SHA256 {record['sha256']}")
    return manifest


def unpack(archive: Path, destination: Path, commit: str, family: str) -> dict:
    if destination.exists():
        raise ValueError("Golden input destination must be fresh; refusing to mix runs")
    with tarfile.open(archive, "r:gz") as source:
        members = source.getmembers()
        if len({member.name for member in members}) != len(members):
            raise ValueError("Duplicate archive members")
        metadata = source.getmember("manifest.json")
        if not metadata.isfile() or metadata.size > 4 * 1024 * 1024:
            raise ValueError("Invalid golden manifest")
        with source.extractfile(metadata) as stream:
            encoded = stream.read()
            manifest = json.loads(encoded)
        if (manifest.get("format"), manifest.get("commit"), manifest.get("family")) != (FORMAT, commit, family):
            raise ValueError("Golden inputs belong to a different commit, family or format")
        records = manifest.get("files")
        if not isinstance(records, dict) or set(records) != {m.name for m in members if m.name != "manifest.json"}:
            raise ValueError("Golden manifest and archive inventory differ")
        suffix = FAMILIES[family]
        required = {f"bin/sapphire_sandbox{suffix}", f"bin/sapphire_testkit{suffix}", "content/shared.spack"}
        if family == "dx12-warp":
            required.add("bin/d3d10warp.dll")
        allowed_bins = required | {"bin/sapphire_sandbox.pdb"}
        if not required.issubset(records):
            raise ValueError("Required golden inputs are missing")
        folded = [name.casefold() for name in records]
        if len(folded) != len(set(folded)):
            raise ValueError("Case-colliding golden inputs")
        for member in members:
            if member.name == "manifest.json":
                continue
            record = records[member.name]
            executable = member.name in {f"bin/sapphire_sandbox{suffix}", f"bin/sapphire_testkit{suffix}"}
            if (
                not isinstance(record, dict)
                or not safe_name(member.name)
                or not member.isfile()
                or member.name.startswith("bin/") and member.name not in allowed_bins
                or member.size != record.get("bytes")
                or record.get("mode") != (0o755 if executable else 0o644)
                or not re.fullmatch("[a-f0-9]{64}", record.get("sha256", ""))
            ):
                raise ValueError(f"Invalid golden input: {member.name}")
        destination.mkdir(parents=True)
        try:
            for member in members:
                if member.name == "manifest.json":
                    continue
                path = destination / member.name
                path.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as src, path.open("xb") as dst:
                    shutil.copyfileobj(src, dst)
                record = records[member.name]
                if digest(path) != record["sha256"]:
                    raise ValueError(f"Golden input checksum mismatch: {member.name}")
                path.chmod(record["mode"])
            (destination / "manifest.json").write_bytes(encoded)
        except BaseException:
            shutil.rmtree(destination)
            raise
    print(f"Verified golden inputs: {family}, {commit}, {len(records)} files")
    print(f"Manifest SHA256: {hashlib.sha256(encoded).hexdigest()}")
    return manifest


def verify_checkout(commit: str) -> None:
    if not re.fullmatch("[a-f0-9]{40}", commit):
        raise ValueError("An exact Git commit is required")
    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    if actual != commit:
        raise ValueError(f"Checkout {actual} does not match requested golden commit {commit}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("create", "unpack"):
        command = commands.add_parser(name)
        command.add_argument("--archive", required=True, type=Path)
        command.add_argument("--commit", required=True)
        command.add_argument("--family", choices=FAMILIES, required=True)
        if name == "unpack":
            command.add_argument("--destination", required=True, type=Path)
    args = parser.parse_args()
    verify_checkout(args.commit)
    if args.command == "create":
        create(Path.cwd(), args.archive, args.commit, args.family)
    else:
        unpack(args.archive, args.destination, args.commit, args.family)


if __name__ == "__main__":
    main()
