"""Ensure experiment controls and parity checks cannot silently compare different inputs."""

from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import tempfile
import subprocess
import sys
import time
import unittest
from unittest.mock import patch

import capture_experiment as experiment
import golden_bundle


class CaptureExperimentTests(unittest.TestCase):
    def test_checked_out_fixture_registry_is_used_without_modified_flags(self):
        fixtures = experiment.fixtures(Path("tools/sapphire_testkit/src/scenes.rs"))
        self.assertEqual([row["name"] for row in fixtures], list(experiment.SCENES))
        self.assertIn("--capture-frames", fixtures[0]["args"])
        self.assertIn("32", fixtures[0]["args"])

    def test_off_control_removes_inherited_cache_and_does_not_disable_mesa(self):
        with patch.dict(os.environ, {"SAPPHIRE_CAPTURE_PIPELINE_CACHE": "/inherited", "MESA_SHADER_CACHE_DIR": "/normal-mesa"}):
            env = experiment.capture_env(Path("content"), "off")
        self.assertNotIn("SAPPHIRE_CAPTURE_PIPELINE_CACHE", env)
        self.assertEqual(env["MESA_SHADER_CACHE_DIR"], "/normal-mesa")
        self.assertEqual(env["SAPPHIRE_CAPTURE_HISTORY_ONLY"], "off")

    def test_two_modes_share_binary_and_detect_byte_mismatch(self):
        with tempfile.TemporaryDirectory() as directory, redirect_stdout(io.StringIO()):
            root = Path(directory)
            inputs = root / "inputs"
            binary = inputs / "bin/sapphire_sandbox"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"one exact binary")
            content = inputs / "content/shared.spack"
            content.parent.mkdir()
            content.write_bytes(b"one exact content")
            manifest = {"commit": "a" * 40, "family": "vulkan-lavapipe", "files": {
                "bin/sapphire_sandbox": {"bytes": binary.stat().st_size, "sha256": golden_bundle.digest(binary)},
                "content/shared.spack": {"bytes": content.stat().st_size, "sha256": golden_bundle.digest(content)},
            }}
            (inputs / "manifest.json").write_text(json.dumps(manifest))
            calls = []

            def fake_capture(command, env, log, timeout):
                calls.append((command, env))
                Path(command[2]).write_bytes(b"identical PNG bytes")
                return 0, 0.125

            with patch.object(experiment, "timed_capture", fake_capture):
                report = experiment.experiment(inputs, root / "output", "a" * 40)
            self.assertTrue(report["complete"])
            self.assertEqual(len(calls), 6 * 2 * 4)
            self.assertEqual({command[0] for command, _ in calls}, {str(binary)})
            self.assertTrue(all("SAPPHIRE_CAPTURE_PIPELINE_CACHE" not in env for _, env in calls))
            self.assertEqual({env["SAPPHIRE_CAPTURE_HISTORY_ONLY"] for _, env in calls}, {"off", "on"})
            for summary in report["summary"].values():
                self.assertEqual(len(summary["round_seconds"]), 3)
                self.assertTrue(summary["all_png_bytes_identical"])
            self.assertEqual([row["mode"] for row in report["sequences"][:2]], list(report["summary"]))
            self.assertEqual([row["mode"] for row in report["sequences"]][2:4], list(reversed(report["summary"])))

            def mismatch(command, env, log, timeout):
                result = fake_capture(command, env, log, timeout)
                if env["SAPPHIRE_CAPTURE_HISTORY_ONLY"] == "on":
                    Path(command[2]).write_bytes(b"regressed PNG bytes")
                return result

            with patch.object(experiment, "timed_capture", mismatch):
                with self.assertRaisesRegex(RuntimeError, "PNG byte parity"):
                    experiment.experiment(inputs, root / "mismatch", "a" * 40, rounds=1)
            partial = json.loads((root / "mismatch/results.json").read_text())
            self.assertFalse(partial["complete"])
            self.assertFalse(partial["summary"]["history-on"]["all_png_bytes_identical"])

    def test_blocking_capture_keeps_both_output_streams_and_exit_status(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "capture.log"
            code = 'import sys; print("stdout", flush=True); print("stderr", file=sys.stderr, flush=True); sys.exit(7)'
            status, elapsed = experiment.timed_capture([sys.executable, "-c", code], dict(os.environ), log, 5)
            self.assertEqual(status, 7)
            self.assertGreater(elapsed, 0)
            self.assertEqual(log.read_text(), "stdout\nstderr\n")

    def test_watchdog_kills_and_reaps_child_and_preserves_output(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "capture.log"
            children = []
            popen = subprocess.Popen

            def start(*args, **kwargs):
                child = popen(*args, **kwargs)
                children.append(child)
                return child

            before = time.perf_counter()
            with patch.object(experiment.subprocess, "Popen", start):
                with self.assertRaises(subprocess.TimeoutExpired):
                    experiment.timed_capture(
                        [sys.executable, "-c", 'import time; print("started", flush=True); time.sleep(30)'],
                        dict(os.environ), log, 2,
                    )
            self.assertLess(time.perf_counter() - before, 10)
            self.assertIsNotNone(children[0].returncode)
            self.assertNotEqual(children[0].returncode, 0)
            self.assertEqual(log.read_text(), "started\n")


if __name__ == "__main__":
    unittest.main()
