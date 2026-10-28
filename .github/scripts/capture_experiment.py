#!/usr/bin/env python3
"""Serial, same-binary capture experiment; this does not replace the golden quality gate."""

import argparse
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import threading
import time

from golden_bundle import digest, verify_checkout

SCENES = (
    "turntable-taa", "turntable-taa-motion", "environment-day",
    "ambient-gtao", "hair-highlights", "effects-blur-lines",
)
MODES = (("off", "off"), ("on", "off"), ("off", "on"), ("on", "on"))


def fixtures(source: Path) -> list[dict]:
    """Read the exact checked-out fixtures; fail if Rust's literal representation changes."""
    text = source.read_text()
    found = {}
    pattern = r'Scene\s*\{\s*name:\s*"([^"]+)",\s*args:\s*&\[([^]]*)\]'
    for name, values in re.findall(pattern, text):
        if name not in SCENES:
            continue
        if name in found:
            raise ValueError(f"Duplicate scene {name}")
        # Current fixture arguments are JSON-compatible Rust string literals. Restrict the
        # grammar so source comments/expressions cannot silently turn into different flags.
        if not re.fullmatch(r'\s*(?:"(?:[^"\\]|\\.)*"\s*,\s*)*', values):
            raise ValueError(f"Scene {name} no longer has literal string arguments")
        found[name] = json.loads("[" + values.rstrip().rstrip(",") + "]")
    if set(found) != set(SCENES):
        raise ValueError("Representative fixtures are missing from the checked-out scene registry")
    return [{"name": name, "args": found[name]} for name in SCENES]


def capture_env(content: Path, cache: Path, pipeline: str, history: str) -> dict:
    env = dict(os.environ)
    env.update(SAPPHIRE_CONTENT_DIR=str(content), SAPPHIRE_GPU_BACKEND="vulkan",
               SAPPHIRE_GPU_DEBUG="none", SAPPHIRE_CAPTURE_HISTORY_ONLY=history)
    env.pop("SAPPHIRE_CAPTURE_PIPELINE_CACHE", None)
    if pipeline == "on":
        env["SAPPHIRE_CAPTURE_PIPELINE_CACHE"] = str(cache)
    return env


def cache_inventory(directory: Path) -> list[dict]:
    return [{"name": str(path.relative_to(directory)), "bytes": path.stat().st_size}
            for path in sorted(directory.rglob("*")) if path.is_file()]


def timed_capture(command: list[str], env: dict, log: Path, timeout: float) -> tuple[int, float]:
    """Time blocking waitpid, avoiding subprocess.run(timeout)'s 50 ms polling tail."""
    with log.open("wb") as stream:
        started = time.perf_counter()
        process = subprocess.Popen(command, env=env, stdout=stream, stderr=subprocess.STDOUT)
        expired = threading.Event()

        def stop() -> None:
            expired.set()
            process.kill()

        watchdog = threading.Timer(timeout, stop)
        watchdog.daemon = True
        try:
            watchdog.start()
            returncode = process.wait()
            elapsed = time.perf_counter() - started
        finally:
            watchdog.cancel()
            if watchdog.ident is not None:
                watchdog.join()
            # Also terminate/reap if wait was interrupted, or starting the watchdog failed.
            if process.poll() is None:
                process.kill()
            process.wait()
        if expired.is_set():
            raise subprocess.TimeoutExpired(command, timeout)
    return returncode, elapsed


def verify_inputs(inputs: Path, commit: str) -> dict:
    manifest = json.loads((inputs / "manifest.json").read_text())
    if manifest["commit"] != commit or manifest["family"] != "vulkan-lavapipe":
        raise ValueError("Experiment inputs do not match the exact source commit/backend")
    for name, record in manifest["files"].items():
        path = inputs / name
        if path.stat().st_size != record["bytes"] or digest(path) != record["sha256"]:
            raise ValueError(f"Input changed after artifact verification: {name}")
    return manifest


def summarize(records: list[dict]) -> dict:
    measured = [record for record in records if not record["discarded_warmup"]]
    modes = {}
    for pipeline, history in MODES:
        key = f"cache-{pipeline}_history-{history}"
        runs = [record for record in measured if record["mode"] == key]
        durations = [record["capture_seconds"] for record in runs]
        modes[key] = {
            "round_seconds": durations,
            "median_seconds": statistics.median(durations) if durations else None,
            "min_seconds": min(durations) if durations else None,
            "max_seconds": max(durations) if durations else None,
            "all_png_bytes_identical": all(scene["matches_reference"] for run in runs for scene in run["scenes"]),
            "cache_bytes_by_round": [sum(item["bytes"] for item in run["cache_files"]) for run in runs],
        }
    baseline = modes["cache-off_history-off"]["median_seconds"]
    for mode in modes.values():
        value = mode["median_seconds"]
        mode["median_reduction_percent"] = 100 * (1 - value / baseline) if baseline and value else None
    return modes


def experiment(inputs: Path, output: Path, commit: str, rounds: int = 3, timeout: int = 180) -> dict:
    if output.exists():
        raise ValueError("Experiment output must be fresh")
    manifest = verify_inputs(inputs, commit)
    cohort = fixtures(Path("tools/sapphire_testkit/src/scenes.rs"))
    output.mkdir(parents=True)
    binary = inputs / "bin/sapphire_sandbox"
    report = {
        "commit": commit, "input_manifest": manifest,
        "manifest_sha256": digest(inputs / "manifest.json"),
        "source_fixtures_sha256": digest(Path("tools/sapphire_testkit/src/scenes.rs")),
        "fixtures": cohort, "measured_rounds": rounds, "sequences": [],
        "method": "One physical runner and executable; fresh process per scene; one discarded 4-mode warmup round; alternating mode order; fresh explicit cache per mode/round shared only within its serial scene cohort; normal Mesa disk cache untouched. PNG byte parity across every warmup and measured capture. No full-suite gain extrapolation.",
    }
    references = {}
    try:
        for round_index in range(rounds + 1):
            order = MODES if round_index % 2 == 0 else tuple(reversed(MODES))
            for pipeline, history in order:
                key = f"cache-{pipeline}_history-{history}"
                folder = output / f"round-{round_index}" / key
                folder.mkdir(parents=True)
                cache = folder / "pipeline-cache"
                env = capture_env(inputs / "content", cache, pipeline, history)
                sequence = {"round": round_index, "discarded_warmup": round_index == 0,
                            "mode": key, "scenes": [], "capture_seconds": 0.0, "cache_files": []}
                report["sequences"].append(sequence)
                for scene in cohort:
                    png = folder / f"{scene['name']}.png"
                    log = folder / f"{scene['name']}.log"
                    command = [str(binary), "--capture", str(png), "--size", "640x360", *scene["args"]]
                    returncode, elapsed = timed_capture(command, env, log, timeout)
                    if returncode != 0 or not png.is_file():
                        raise RuntimeError(f"Capture failed ({returncode}): {log}")
                    checksum = digest(png)
                    references.setdefault(scene["name"], checksum)
                    item = {"name": scene["name"], "seconds": elapsed, "png_sha256": checksum,
                            "matches_reference": checksum == references[scene["name"]],
                            "cache_bytes_after": sum(x["bytes"] for x in cache_inventory(cache))}
                    sequence["scenes"].append(item)
                    sequence["capture_seconds"] += elapsed
                    print(f"round={round_index} {key} {scene['name']}: {elapsed:.3f}s parity={item['matches_reference']}", flush=True)
                sequence["cache_files"] = cache_inventory(cache)
                report["summary"] = summarize(report["sequences"])
                (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        if any(not scene["matches_reference"] for run in report["sequences"] for scene in run["scenes"]):
            raise RuntimeError("PNG byte parity failed; inspect results.json and preserved captures")
        report["complete"] = True
    except BaseException as error:
        report["complete"] = False
        report["error"] = str(error)
        raise
    finally:
        report["summary"] = summarize(report["sequences"])
        (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report["summary"], indent=2))
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--sha", required=True)
    args = parser.parse_args()
    verify_checkout(args.sha)
    experiment(args.inputs.resolve(), args.output.resolve(), args.sha)


if __name__ == "__main__":
    main()
