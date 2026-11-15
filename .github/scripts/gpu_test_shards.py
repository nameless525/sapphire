#!/usr/bin/env python3
"""Run one shard of the `#[ignore]`d GPU tests, from the executables `app_tests_bundle.py unpack` put back.

Each test goes to a shard by a stable hash of its name, `zlib.crc32(name) % shards`, so adding a test never
moves another and no list order matters. A shard lists the ignored tests of each executable of the packages
it is given, prints how many each shard holds of the total, then runs each executable from its package's
directory (as `cargo test` does) on its own share with `--exact`, one test at a time. It runs every
executable whatever an earlier one did, and fails if any run fails or doesn't run exactly the names it was
given, so no test can fall between shards unseen. A name in two executables runs in both, in one shard.
"""

import argparse
from collections.abc import Callable, Iterator
import json
from pathlib import Path
import re
import subprocess
import sys
import zlib

from app_tests_bundle import MANIFEST

# Windows passes at most 32,767 characters to a process. Names go in chunks of at most a quarter of that,
# so the executable's path and the flags fit around them. A shard's names came to 7,400 to 8,300 characters
# in four shards of the 483 tests of main's run on 2026-11-15, so a suite twice the size runs in two or three
# chunks in its largest executable.
CHUNK_CHARACTERS = 8192
LIST = ["--ignored", "--list", "--format", "terse"]
RUN = ["--ignored", "--test-threads=1", "--exact"]
RESULT = re.compile(r"test result: \w+\. (\d+) passed; (\d+) failed; \d+ ignored; \d+ measured; \d+ filtered out")

# Starts a command in a directory and returns its exit status and its output, as text.
Launch = Callable[[list[str], Path, bool], tuple[int, str]]


def shard_of(name: str, shards: int) -> int:
    """The shard, from 1, that runs the test `name`."""
    return zlib.crc32(name.encode()) % shards + 1


def names_from_list(output: str) -> list[str]:
    """The tests in a test executable's `--list --format terse` output; benchmarks and other lines aren't."""
    return [line.removesuffix(": test") for line in output.splitlines() if line.endswith(": test")]


def chunks(names: list[str], limit: int = CHUNK_CHARACTERS) -> Iterator[list[str]]:
    """`names` in order, in runs whose names and the spaces between them take at most `limit` characters."""
    chunk: list[str] = []
    size = 0
    for name in names:
        cost = len(name) + 1
        if cost > limit:
            raise ValueError(f"The test name {name} is longer than a command line's share")
        if chunk and size + cost > limit:
            yield chunk
            chunk, size = [], 0
        chunk.append(name)
        size += cost
    if chunk:
        yield chunk


def shard_counts(listed: dict[str, list[str]], shards: int) -> list[int]:
    """How many distinct test names each shard holds, failing if the shards don't partition them."""
    names = {name for tests in listed.values() for name in tests}
    counts = [0] * shards
    for name in names:
        counts[shard_of(name, shards) - 1] += 1
    if sum(counts) != len(names):
        raise ValueError(f"The shards hold {sum(counts)} tests of {len(names)}")
    return counts


def launch(command: list[str], cwd: Path, echo: bool) -> tuple[int, str]:
    """Runs `command` in `cwd`, printing its output as it comes if `echo`, and returns its status and output."""
    with subprocess.Popen(
        command, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace"
    ) as process:
        lines = []
        for line in process.stdout:
            lines.append(line)
            if echo:
                print(line, end="", flush=True)
    return process.returncode, "".join(lines)


def run_shard(workspace: Path, packages: list[str], shard: int, shards: int, run: Launch = launch) -> int:
    """Runs shard `shard` of `shards` of the bundle's tests in `packages`; returns the step's exit status."""
    if not 1 <= shard <= shards:
        raise ValueError(f"No shard {shard} of {shards}")
    tests = json.loads((workspace / MANIFEST).read_text())["tests"]
    selected = {target: test for target, test in sorted(tests.items()) if test["package"] in packages}
    for package in packages:
        if not any(test["package"] == package for test in selected.values()):
            raise ValueError(f"The bundle has no test executable from {package}")
    listed = {}
    for target, test in selected.items():
        status, output = run([str(workspace / test["path"]), *LIST], workspace / test["package"], False)
        if status != 0:
            print(output, end="")
            raise ValueError(f"Listing {target}'s tests exited with {status}")
        listed[target] = names_from_list(output)
    counts = shard_counts(listed, shards)
    spread = ", ".join(f"{index}/{shards} {count}" for index, count in enumerate(counts, 1))
    print(f"Shard {shard}/{shards} of {' '.join(packages)}: {counts[shard - 1]} of {sum(counts)} tests ({spread})")
    problems = []
    for target, test in selected.items():
        mine = [name for name in listed[target] if shard_of(name, shards) == shard]
        print(f"{target}: {len(mine)} of {len(listed[target])} tests", flush=True)
        for chunk in chunks(mine):
            status, output = run([str(workspace / test["path"]), *RUN, *chunk], workspace / test["package"], True)
            results = RESULT.findall(output)
            ran = sum(int(passed) + int(failed) for passed, failed in results)
            if status != 0:
                problems.append(f"{target} exited with {status}")
            if len(results) != 1 or ran != len(chunk):
                problems.append(f"{target} ran {ran} of the {len(chunk)} tests it was given")
    for problem in problems:
        print(f"Failed: {problem}")
    return 1 if problems else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shard", required=True, type=int, help="this shard, from 1")
    parser.add_argument("--shards", required=True, type=int)
    parser.add_argument("packages", nargs="+", help="the packages' directories, as the bundle names them")
    args = parser.parse_args()
    # The tests' output is echoed as it comes; a Windows runner's console code page can't print all of it.
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    return run_shard(Path.cwd(), args.packages, args.shard, args.shards)


if __name__ == "__main__":
    sys.exit(main())
