#!/usr/bin/env python3
"""Native Windows actual-CLI path fixture; --inventory is portable and read-only."""
import argparse
import hashlib
import json
import ntpath
import os
from pathlib import Path, PureWindowsPath
import platform
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

HERE = Path(__file__).resolve().parent


def inventory():
    cases = []
    for tool in ("replay", "golden"):
        for shape in ("ordinary", "missing-tail", "drive-relative", "root-relative", "verbatim",
                      "junction-parent-ordinary"):
            cases.append(dict(tool=tool, role="primary", shape=shape, expect="success"))
        roles = ("replays",) if tool == "replay" else ("content", "goldens", "sandbox")
        for role in roles:
            for shape in ("equal", "ancestor", "descendant"):
                cases.append(dict(tool=tool, role=role, shape=shape, expect="refusal"))
                if role == "sandbox" and shape == "descendant":
                    cases[-1]["native_resolution_error_allowed"] = True
        for shape in ("drive-relative-overlap", "root-relative-overlap", "verbatim-overlap",
                      "junction-overlap", "junction-missing-tail-overlap"):
            cases.append(dict(tool=tool, role="primary", shape=shape, expect="refusal"))
        cases.append(dict(tool=tool, role="primary", shape="dangling-junction", expect="io-refusal"))
        cases.append(dict(tool=tool, role="primary", shape="junction-parent-verbatim",
                          expect="conditional-native-overlap"))
    return cases


def output_path(case, root, spec):
    primary = case / "inputs" / ("replays" if spec["tool"] == "replay" else "content")
    roles = dict(replays=primary, content=case / "inputs/content",
                 goldens=case / "inputs/goldens", sandbox=case / "inputs/bin/sandbox.exe")
    source = primary if spec["role"] == "primary" else roles[spec["role"]]
    shape = spec["shape"]
    if shape == "dangling-junction":
        return str(case / "junction" / "new/deep")
    if shape in ("equal", "ancestor", "descendant"):
        return str({"equal": source, "ancestor": source.parent,
                    "descendant": source / "child"}[shape])
    if shape.startswith("junction-"):
        if shape == "junction-parent-verbatim":
            return "\\\\?\\" + str(case / "junction") + "\\..\\" + primary.name
        suffix = {"junction-overlap": "", "junction-missing-tail-overlap": "new/deep",
                  "junction-parent-ordinary": "../ordinary-output"}[shape]
        # Keep the unnormalized spelling: Path.resolve() would erase the case under test.
        return str(case / "junction") + ("\\" + suffix.replace("/", "\\") if suffix else "")
    target = primary / "new/deep" if shape.endswith("overlap") else case / "output"
    if shape == "missing-tail":
        target = target / "new/deep"
    text = str(target)
    if shape.startswith("drive-relative"):
        # Child cwd is root, on this same drive. No ambient per-drive cwd is used.
        return ntpath.splitdrive(text)[0] + ntpath.relpath(text, str(root))
    if shape.startswith("root-relative"):
        return ntpath.splitdrive(text)[1]
    if shape.startswith("verbatim"):
        return "\\\\?\\" + text
    return text


def snapshot(tree, allow_junction=False):
    # Never recurse through reparse points, including junctions on older Python versions.
    result = {}

    def visit(directory):
        for entry in sorted(directory.iterdir()):
            key = entry.relative_to(tree).as_posix()
            reparse = entry.is_symlink() or getattr(entry.lstat(), "st_file_attributes", 0) & 0x400
            if reparse:
                if not allow_junction:
                    raise RuntimeError(f"unexpected reparse point in protected tree: {entry}")
                result[key] = "reparse:" + os.readlink(entry)
            elif entry.is_dir():
                result[key] = "directory"
                visit(entry)
            else:
                result[key] = hashlib.sha256(entry.read_bytes()).hexdigest()
    visit(tree)
    return result


def png_bytes():
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = (b"\0" + bytes((48, 96, 144, 255)) * 64) * 64
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 64, 64, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def public_evidence(receipt, root):
    """Redact machine paths without walking the fixture tree or exporting its files."""
    payload = json.dumps(receipt, indent=2)
    for path, replacement in (
        (root, "<fixture>"),
        (os.environ.get("GITHUB_WORKSPACE"), "<workspace>"),
        (HERE.parents[2], "<workspace>"),
        (os.environ.get("RUNNER_TEMP"), "<runner-temp>"),
        (Path.home(), "<home>"),
    ):
        if path:
            spelling = str(path)
            for variant in {spelling, spelling.replace("\\", "/")}:
                escaped = json.dumps(variant)[1:-1]
                payload = re.sub(re.escape(escaped), replacement, payload, flags=re.IGNORECASE)
    return payload + "\n"


def conditional_refusal(completed, tool, out, checksum, alias_proved):
    """Require a normal, output-specific refusal from the actual CLI."""
    expected_exit = 1 if tool == "replay" else 2
    assert completed.returncode == expected_exit, f"expected exit {expected_exit}"
    assert b"panicked" not in completed.stderr, "CLI panicked"
    diagnostics = completed.stderr.decode("utf-8", errors="replace").splitlines()
    if any(line.startswith(f"error: output {out} overlaps input ") for line in diagnostics):
        return "overlap-refusal"
    assert not alias_proved, "native IO proved the alias; expected an overlap refusal"
    contexts = [f"resolving {out}:"]
    if tool == "replay":
        checksum_out = PureWindowsPath(out) / checksum
        contexts.extend((f"resolving {checksum_out}:", f"creating {out}:", f"writing {checksum_out}:"))
    else:
        contexts.extend((f"failed to empty {out}:", f"failed to create {out}:"))
    prefixes = tuple(f"error: {context} " for context in contexts)
    assert any(line.startswith(prefixes) for line in diagnostics), "missing output-path IO refusal"
    return "native-io-refusal"


def run(args):
    if os.name != "nt":
        raise RuntimeError("--execute requires native Windows; no emulation pass is accepted")
    if sys.flags.optimize:
        raise RuntimeError("run without -O: this fixture requires its assertions")
    replay, golden = (Path(value).resolve(strict=True) for value in (args.replay, args.testkit))
    for executable in (replay, golden):
        if not executable.is_file() or executable.suffix.lower() != ".exe":
            raise RuntimeError(f"expected admitted native .exe: {executable}")
    root = Path(tempfile.mkdtemp(prefix="sapphire_windows_paths_"))
    print(f"Disposable fixture and evidence: {root}", flush=True)
    receipt = dict(status="running", actual_cli_cases_verified=False,
                   windows_verified=False, root=str(root),
                   platform=platform.platform(), python=sys.version, commands=[], cases=[], gaps=[],
                   scope="real CLI path/write/prune; deterministic fake capture, no GPU/backend validation")

    if args.evidence:
        receipt["checkout_sha"] = os.environ.get("GITHUB_SHA")

    def save():
        (root / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
        if args.evidence:
            args.evidence.write_text(public_evidence(receipt, root), encoding="utf-8")

    def save_output(index, label, row, stdout, stderr):
        for stream, data in (("stdout", stdout), ("stderr", stderr)):
            data = data or b""
            (root / f"{index:03}-{label}.{stream}").write_bytes(data)
            if args.evidence:
                row[stream] = data[:32768].decode("utf-8", errors="replace")
                if len(data) > 32768:
                    row[f"{stream}_truncated"] = True

    def call(label, command, expected=0, env=None):
        index = len(receipt["commands"])
        row = dict(label=label, argv=[str(x) for x in command], cwd=str(root))
        receipt["commands"].append(row)
        save()
        child = subprocess.Popen(row["argv"], cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            stdout, stderr = child.communicate(timeout=60)
        except subprocess.TimeoutExpired as error:
            # Native Windows tree termination: golden may have its fake capture child alive.
            try:
                cleanup = subprocess.run(["taskkill.exe", "/PID", str(child.pid), "/T", "/F"],
                                         capture_output=True, timeout=10)
                row["timeout_tree_cleanup_exit"] = cleanup.returncode
            except (OSError, subprocess.TimeoutExpired) as cleanup_error:
                row["timeout_tree_cleanup_error"] = str(cleanup_error)
            finally:
                if child.poll() is None:
                    child.kill()
                try:
                    stdout, stderr = child.communicate(timeout=10)
                except subprocess.TimeoutExpired:
                    stdout, stderr = error.stdout, error.stderr
                    row["timeout_tree_cleanup_incomplete"] = True
                save_output(index, label, row, stdout, stderr)
            row["status"] = "timeout"
            save()
            raise
        completed = subprocess.CompletedProcess(row["argv"], child.returncode, stdout, stderr)
        row["exit_code"] = completed.returncode
        save_output(index, label, row, stdout, stderr)
        save()
        if expected == 0 and completed.returncode != 0:
            raise RuntimeError(f"{label}: expected success, got {completed.returncode}; inspect raw logs")
        return completed

    def junction(label, link, target):
        junction_env = os.environ.copy()
        junction_env.update(SAPPHIRE_FIXTURE_LINK=str(link), SAPPHIRE_FIXTURE_TARGET=str(target))
        receipt.setdefault("junctions", []).append(dict(link=str(link), target=str(target)))
        # Fixed PowerShell code; paths arrive only through environment variables, never shell code.
        code = ("$ErrorActionPreference='Stop'; New-Item -ItemType Junction "
                "-Path $env:SAPPHIRE_FIXTURE_LINK -Target $env:SAPPHIRE_FIXTURE_TARGET | Out-Null")
        call(label, ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", code], env=junction_env)

    try:
        drive, tail = ntpath.splitdrive(str(root))
        if len(drive) != 2 or drive[1] != ":" or not tail.startswith("\\"):
            raise RuntimeError("fixture requires local drive-letter TEMP; UNC TEMP is a named unsupported setup")
        receipt["executables"] = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (replay, golden)}
        receipt["fixture_sources"] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                                      for p in (HERE / "run.py", HERE / "sandbox.rs")}
        call("rustc-version", [args.rustc, "--version", "--verbose"])
        fake = root / "capture-producer.exe"
        call("build-fake", [args.rustc, "--edition=2024", str(HERE / "sandbox.rs"), "-o", fake])
        seed = root / "seed"
        call("record-seed", [replay, "record", "path-fixture", "--dir", seed / "replays", "--out",
                             seed / "out", "--scenario", "melee", "--script", "idle", "--ticks", "2"])
        seed_files = sorted((seed / "replays").glob("checksums.*.txt"))
        if len(seed_files) != 1 or not (seed / "replays/path-fixture.sreplay").is_file():
            raise RuntimeError("record did not produce one architecture checksum and a replay")
        checksum = seed_files[0].name
        env = os.environ.copy()
        env["CI"] = "1"  # Missing golden must fail, even on a developer machine.
        for index, spec in enumerate(inventory()):
            case = root / "cases" / f"{index:02}-{spec['tool']}-{spec['role']}-{spec['shape']}"
            inputs = case / "inputs"
            inputs.mkdir(parents=True)
            if spec["tool"] == "replay":
                shutil.copytree(seed / "replays", inputs / "replays")
                primary = inputs / "replays"
            else:
                for directory in ("content", "goldens", "bin"):
                    (inputs / directory).mkdir()
                (inputs / "content/canary.txt").write_bytes(b"protected content\n")
                (inputs / "content/capture.png").write_bytes(png_bytes())
                (inputs / "goldens/turntable.fixture-path-only.png").write_bytes(png_bytes())
                shutil.copyfile(fake, inputs / "bin/sandbox.exe")
                primary = inputs / "content"
            out = output_path(case, root, spec)
            actual_out = (case / "ordinary-output" if spec["shape"] == "junction-parent-ordinary"
                          else case / "output")
            if spec["shape"] == "missing-tail":
                actual_out = actual_out / "new/deep"
            if spec["shape"].startswith("junction-"):
                junction(f"junction-{index}", case / "junction", primary)
            elif spec["shape"] == "dangling-junction":
                target = case / "removed-target"
                target.mkdir()
                junction(f"junction-{index}", case / "junction", target)
                target.rmdir()
            # Existing disjoint outputs exercise golden's actual recursive prune.
            if spec["expect"] == "success" and spec["shape"] != "missing-tail":
                actual_out.mkdir(parents=True, exist_ok=True)
                (actual_out / "stale-canary.txt").write_bytes(b"disposable output\n")
            before = snapshot(inputs)
            case_before = snapshot(case, allow_junction=True)
            alias_probe = None
            if spec["expect"] == "conditional-native-overlap":
                name = "path-fixture.sreplay" if spec["tool"] == "replay" else "canary.txt"
                reference = (primary / name).read_bytes()
                # Test the actual verbatim spelling without normalizing away its parent component.
                try:
                    actual = (Path(out) / name).read_bytes()
                except OSError as error:
                    alias_probe = dict(status="unavailable-native-form", error=str(error),
                                       winerror=getattr(error, "winerror", None))
                    receipt["gaps"].append(dict(spec, out=out, native_alias_probe=alias_probe))
                else:
                    assert actual == reference, f"native alias resolved to different bytes: {index}"
                    alias_probe = dict(status="proved")
            command = ([replay, "check", "--dir", inputs / "replays", "--out", out]
                       if spec["tool"] == "replay" else
                       [golden, "golden", "--sandbox", inputs / "bin/sandbox.exe", "--content",
                        inputs / "content", "--goldens", inputs / "goldens", "--out", out,
                        "--scene", "turntable", "--scene-timeout", "10"])
            row = dict(spec, out=out, argv=[str(x) for x in command], status="running",
                       protected_before=before, case_before=case_before)
            if alias_probe is not None:
                row["native_alias_probe"] = alias_probe
            receipt["cases"].append(row)
            save()  # Preserve canaries even if the following operation unexpectedly destroys them.
            completed = call(f"case-{index}", command, expected=None, env=env)
            row.update(exit_code=completed.returncode, protected_after=snapshot(inputs))
            save()
            assert row["protected_after"] == before, f"input canary changed: {index}"
            if spec["expect"] != "success":
                if alias_probe is not None:
                    row["cli_refusal"] = conditional_refusal(
                        completed, spec["tool"], out, checksum, alias_probe["status"] == "proved",
                    )
                else:
                    assert completed.returncode != 0, f"overlap accepted: {index}"
                    assert b"panicked" not in completed.stderr, f"panic: {index}"
                    if spec["expect"] == "refusal" and not spec.get("native_resolution_error_allowed"):
                        assert b"overlaps input" in completed.stderr, f"not an overlap refusal: {index}"
                    else:
                        assert b"error:" in completed.stderr, f"missing native failure diagnostic: {index}"
                assert snapshot(case, allow_junction=True) == case_before, f"refused operation wrote into case: {index}"
            else:
                assert completed.returncode == 0, f"supported disjoint output failed: {index}"
                if spec["tool"] == "replay":
                    assert (actual_out / checksum).read_bytes() == (inputs / "replays" / checksum).read_bytes()
                else:
                    assert not (actual_out / "stale-canary.txt").exists(), "output was not pruned"
                    assert (actual_out / "turntable.png").read_bytes() == png_bytes()
                    timings = json.loads((actual_out / "timings.json").read_text())
                    assert len(timings) == 1 and timings[0]["status"] == "ok", timings
            row["status"] = "passed"
            save()
        assert receipt["executables"] == {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (replay, golden)}
        receipt.update(status="passed", actual_cli_cases_verified=True,
                       windows_verified=not receipt["gaps"])
        if receipt["gaps"]:
            print("All CLI cases passed. Some native verbatim alias forms were unavailable.", flush=True)
    except BaseException as error:
        receipt.update(status="failed", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        receipt["executables_after"] = {}
        for executable in (replay, golden):
            try:
                identity = hashlib.sha256(executable.read_bytes()).hexdigest()
            except OSError as error:
                identity = f"unreadable: {error}"
            receipt["executables_after"][str(executable)] = identity
        save()
        print(f"Retained receipt/logs/fixtures (no automatic deletion): {root}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--inventory", action="store_true")
    mode.add_argument("--execute", action="store_true")
    parser.add_argument("--replay")
    parser.add_argument("--testkit")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--evidence", type=Path, help="write a path-redacted JSON receipt with bounded logs")
    args = parser.parse_args()
    if args.inventory:
        root = PureWindowsPath(r"C:\owned\fixture")
        rows = [dict(spec, out=output_path(root / "cases" / str(i), root, spec))
                for i, spec in enumerate(inventory())]
        print(json.dumps(dict(native_windows_executed=False, cases=rows), indent=2))
    elif not args.replay or not args.testkit:
        parser.error("--execute requires --replay and --testkit admitted native executables")
    else:
        run(args)


if __name__ == "__main__":
    main()
