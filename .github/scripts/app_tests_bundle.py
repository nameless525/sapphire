#!/usr/bin/env python3
"""Package the App tests' build once, then verify the same bytes on each lane's runner and run them there.

The producer builds every test executable the lanes run, the dev sandbox and the live-link tool, and cooks
the desktop content. A lane restores each file where Cargo put it, so the absolute paths the tests were
compiled with (`CARGO_BIN_EXE_*`, `CARGO_MANIFEST_DIR`, `CARGO_TARGET_TMPDIR`) hold on any runner with the
same checkout path, and runs a test executable from its package's directory, as `cargo test` does.
"""

import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tarfile

from golden_bundle import digest, verify_checkout

FORMAT = 1
# What the lanes run beside the tests: the sandbox the tests start (`CARGO_BIN_EXE_sapphire_sandbox`, the
# editor's tests and the live-link smoke) and the live-link tool.
BINARIES = ("target/debug/sapphire_sandbox", "target/debug/sapphire_livelink")
CONTENT = "target/content/desktop"
TESTS = "target/debug/deps"
# Written by `unpack` for `run`; `target/tmp` is `CARGO_TARGET_TMPDIR`, which Cargo makes as it builds.
MANIFEST = "target/app-tests.json"
TMPDIR = "target/tmp"


def safe_relative(name: str) -> bool:
    path = PurePosixPath(name)
    return (
        bool(name)
        and not path.is_absolute()
        and ".." not in path.parts
        and "\\" not in name
        and ":" not in name
        and str(path) == name
    )


def tests_from_messages(workspace: Path, messages: list[Path]) -> dict:
    """Each integration test executable in Cargo's JSON messages, by target name: where it is and its package."""
    tests = {}
    for path in messages:
        for line in path.read_text().splitlines():
            if not line.startswith("{"):
                continue
            message = json.loads(line)
            if message.get("reason") != "compiler-artifact" or message["target"]["kind"] != ["test"]:
                continue
            if not message.get("executable"):
                continue
            entry = {
                "path": Path(message["executable"]).relative_to(workspace).as_posix(),
                "package": Path(message["manifest_path"]).parent.relative_to(workspace).as_posix(),
            }
            name = message["target"]["name"]
            if tests.setdefault(name, entry) != entry:
                raise ValueError(f"Two executables for the test {name}: {tests[name]['path']} and {entry['path']}")
    if not tests:
        raise ValueError("Cargo's messages name no test executable")
    return tests


def create(workspace: Path, archive: Path, commit: str, messages: list[Path]) -> dict:
    tests = tests_from_messages(workspace, messages)
    files = {name: workspace / name for name in BINARIES}
    for test in tests.values():
        files[test["path"]] = workspace / test["path"]
    content = workspace / CONTENT
    if not (content / "shared.spack").is_file():
        raise ValueError("The completed desktop cook must contain shared.spack")
    for path in sorted(content.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"Symbolic links are not app test inputs: {path}")
        if path.is_file():
            files[path.relative_to(workspace).as_posix()] = path
    executables = set(BINARIES) | {test["path"] for test in tests.values()}
    records = {}
    for name, path in files.items():
        if not safe_relative(name) or not path.is_file() or path.is_symlink():
            raise ValueError(f"Missing or invalid app test input: {path}")
        records[name] = {
            "bytes": path.stat().st_size,
            "sha256": digest(path),
            # Artifact transport loses filesystem modes; the inner tar restores these.
            "mode": 0o755 if name in executables else 0o644,
        }
    manifest = {"format": FORMAT, "commit": commit, "tests": tests, "files": records}
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
    print(f"App test bundle: {commit}, {len(tests)} tests, {len(records)} files, {archive.stat().st_size} compressed bytes")
    print(f"Manifest SHA256: {hashlib.sha256(encoded).hexdigest()}")
    for name, test in sorted(tests.items()):
        print(f"Test {name}: {test['path']} from {test['package']}, SHA256 {records[test['path']]['sha256']}")
    return manifest


def allowed(name: str, test_paths: set[str]) -> bool:
    """Only the binaries, the cook and the tests, each where the build put it."""
    if name in BINARIES or name in test_paths:
        return True
    return name.startswith(CONTENT + "/")


def unpack(workspace: Path, archive: Path, commit: str) -> dict:
    target = workspace / "target"
    if target.exists():
        raise ValueError("App test inputs need a fresh checkout with no target/; refusing to mix builds")
    with tarfile.open(archive, "r:gz") as source:
        members = source.getmembers()
        if len({member.name for member in members}) != len(members):
            raise ValueError("Duplicate archive members")
        metadata = source.getmember("manifest.json")
        if not metadata.isfile() or metadata.size > 4 * 1024 * 1024:
            raise ValueError("Invalid app test manifest")
        with source.extractfile(metadata) as stream:
            encoded = stream.read()
            manifest = json.loads(encoded)
        if (manifest.get("format"), manifest.get("commit")) != (FORMAT, commit):
            raise ValueError("App test inputs belong to a different commit or format")
        records = manifest.get("files")
        tests = manifest.get("tests")
        if not isinstance(records, dict) or set(records) != {m.name for m in members if m.name != "manifest.json"}:
            raise ValueError("App test manifest and archive inventory differ")
        if not isinstance(tests, dict) or not tests:
            raise ValueError("The app test manifest names no test")
        test_paths = set()
        for name, test in tests.items():
            if (
                not isinstance(test, dict)
                or not re.fullmatch("[a-z0-9_]+", name)
                or not isinstance(test.get("path"), str)
                or PurePosixPath(test["path"]).parent != PurePosixPath(TESTS)
                or not safe_relative(test["path"])
                or not isinstance(test.get("package"), str)
                or not safe_relative(test["package"])
                or not (workspace / test["package"]).is_dir()
            ):
                raise ValueError(f"Invalid app test entry: {name}")
            test_paths.add(test["path"])
        executables = set(BINARIES) | test_paths
        required = executables | {CONTENT + "/shared.spack"}
        if not required.issubset(records):
            raise ValueError("Required app test inputs are missing")
        folded = [name.casefold() for name in records]
        if len(folded) != len(set(folded)):
            raise ValueError("Case-colliding app test inputs")
        for member in members:
            if member.name == "manifest.json":
                continue
            record = records[member.name]
            if (
                not isinstance(record, dict)
                or not safe_relative(member.name)
                or not allowed(member.name, test_paths)
                or not member.isfile()
                or member.size != record.get("bytes")
                or record.get("mode") != (0o755 if member.name in executables else 0o644)
                or not re.fullmatch("[a-f0-9]{64}", record.get("sha256", ""))
            ):
                raise ValueError(f"Invalid app test input: {member.name}")
        try:
            for member in members:
                if member.name == "manifest.json":
                    continue
                path = workspace / member.name
                path.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as src, path.open("xb") as dst:
                    shutil.copyfileobj(src, dst)
                record = records[member.name]
                if digest(path) != record["sha256"]:
                    raise ValueError(f"App test input checksum mismatch: {member.name}")
                path.chmod(record["mode"])
            (workspace / TMPDIR).mkdir(parents=True)
            (workspace / MANIFEST).write_bytes(encoded)
        except BaseException:
            shutil.rmtree(target)
            raise
    print(f"Verified app test inputs: {commit}, {len(tests)} tests, {len(records)} files")
    print(f"Manifest SHA256: {hashlib.sha256(encoded).hexdigest()}")
    return manifest


def run(workspace: Path, name: str, arguments: list[str]) -> int:
    """Runs the unpacked test `name` from its package's directory, as `cargo test` does, and returns its status."""
    tests = json.loads((workspace / MANIFEST).read_text())["tests"]
    if name not in tests:
        raise ValueError(f"No app test named {name}; the inputs hold {', '.join(sorted(tests))}")
    test = tests[name]
    print(f"Running {test['path']} from {test['package']} {' '.join(arguments)}", flush=True)
    status = subprocess.run([workspace / test["path"], *arguments], cwd=workspace / test["package"]).returncode
    # A test killed by a signal fails as a shell reports it.
    return 128 - status if status < 0 else status


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create_command = commands.add_parser("create")
    create_command.add_argument("--archive", required=True, type=Path)
    create_command.add_argument("--commit", required=True)
    create_command.add_argument("--messages", required=True, type=Path, nargs="+")
    unpack_command = commands.add_parser("unpack")
    unpack_command.add_argument("--archive", required=True, type=Path)
    unpack_command.add_argument("--commit", required=True)
    run_command = commands.add_parser("run")
    run_command.add_argument("test")
    run_command.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    workspace = Path.cwd()
    if args.command == "run":
        arguments = args.arguments[1:] if args.arguments[:1] == ["--"] else args.arguments
        return run(workspace, args.test, arguments)
    verify_checkout(args.commit)
    if args.command == "create":
        create(workspace, args.archive, args.commit, args.messages)
    else:
        unpack(workspace, args.archive, args.commit)
    return 0


if __name__ == "__main__":
    sys.exit(main())
