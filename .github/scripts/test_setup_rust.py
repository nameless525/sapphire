"""Safety and target-coverage checks; all rustup calls are mocked."""

from contextlib import redirect_stdout
import io
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import setup_rust


class SetupRustTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel = "1.99.0"\ncomponents = ["rustfmt", "clippy"]\n'
            'targets = ["aarch64-apple-ios", "aarch64-apple-ios-sim", '
            '"aarch64-linux-android", "x86_64-linux-android"]\n'
        )
        self.env = {
            "GITHUB_ACTIONS": "true",
            "SAPPHIRE_RUNNER_ENVIRONMENT": "github-hosted",
            "GITHUB_ENV": str(self.root / "job.env"),
            "RUNNER_OS": "Linux",
        }
        self.host = "x86_64-unknown-linux-gnu"
        self.installed = [f"1.99.0-{self.host}", f"stable-{self.host}", f"1.99.1-{self.host}"]
        self.run = self.enterContext(patch.object(setup_rust.subprocess, "run"))
        self.output = self.enterContext(
            patch.object(setup_rust.subprocess, "check_output", side_effect=self.rustup_output)
        )
        self.enterContext(redirect_stdout(io.StringIO()))

    def rustup_output(self, command, **kwargs):
        if command == ["rustup", "run", "1.99.0", "rustc", "-vV"]:
            return f"release: 1.99.0\nhost: {self.host}\n"
        if command == ["rustup", "toolchain", "list", "--quiet"]:
            return "\n".join(self.installed) + "\n"
        self.fail(f"Unexpected command: {command}")

    def invoke(self):
        setup_rust.setup(self.root, self.env)

    def test_local_and_self_hosted_calls_cannot_mutate_any_toolchain(self):
        for updates in (
            {"GITHUB_ACTIONS": "false"},
            {"GITHUB_ACTIONS": ""},
            {"SAPPHIRE_RUNNER_ENVIRONMENT": "self-hosted"},
            {"SAPPHIRE_RUNNER_ENVIRONMENT": ""},
            {"GITHUB_ENV": ""},
        ):
            with self.subTest(updates=updates), self.assertRaises(RuntimeError):
                setup_rust.setup(self.root, self.env | updates)
        self.run.assert_not_called()
        self.output.assert_not_called()
        self.assertFalse(Path(self.env["GITHUB_ENV"]).exists())

    def test_host_only_install_and_cache_inventory_keep_exact_pin(self):
        self.invoke()
        install, remove = self.run.call_args_list
        self.assertEqual(install.args[0], ["rustup", "toolchain", "install", "1.99.0", "--profile", "minimal"])
        self.assertEqual(remove.args[0], ["rustup", "toolchain", "uninstall", *self.installed[1:]])
        for call in self.run.call_args_list + self.output.call_args_list:
            self.assertEqual(call.kwargs["env"]["RUSTUP_TOOLCHAIN"], "1.99.0")
        self.assertEqual(Path(self.env["GITHUB_ENV"]).read_text(), "RUSTUP_TOOLCHAIN=1.99.0\n")
        self.assertNotIn("RUSTUP_TOOLCHAIN", self.env)

    def test_mobile_jobs_retain_every_target_they_build(self):
        for group, runner_os, targets in (
            ("ios", "macOS", ["aarch64-apple-ios", "aarch64-apple-ios-sim"]),
            ("android", "Linux", ["aarch64-linux-android"]),
        ):
            with self.subTest(group=group):
                self.run.reset_mock()
                self.env.update(SAPPHIRE_RUST_GROUP=group, RUNNER_OS=runner_os)
                self.invoke()
                command = self.run.call_args_list[0].args[0]
                self.assertEqual(command[6:], [value for target in targets for value in ("--target", target)])

    def test_lint_uses_the_manifest_components(self):
        self.env["SAPPHIRE_RUST_LINT"] = "true"
        self.invoke()
        self.assertEqual(self.run.call_args_list[0].args[0][6:], ["--component", "rustfmt", "--component", "clippy"])

    def test_no_uninstall_if_pinned_inventory_is_missing(self):
        self.installed = [f"stable-{self.host}"]
        with self.assertRaisesRegex(RuntimeError, "refusing to remove"):
            self.invoke()
        self.assertEqual(self.run.call_count, 1)
        self.assertFalse(Path(self.env["GITHUB_ENV"]).exists())

    def test_already_normalized_inventory_needs_no_uninstall(self):
        self.installed = [self.installed[0]]
        self.invoke()
        self.assertEqual(self.run.call_count, 1)

    def test_failed_install_does_not_remove_anything_or_export_override(self):
        self.run.side_effect = subprocess.CalledProcessError(1, "rustup")
        with self.assertRaises(subprocess.CalledProcessError):
            self.invoke()
        self.assertEqual(self.run.call_count, 1)
        self.output.assert_not_called()
        self.assertFalse(Path(self.env["GITHUB_ENV"]).exists())

    def test_invalid_configuration_fails_before_rustup(self):
        for updates in (
            {"SAPPHIRE_RUST_GROUP": "unknown"},
            {"SAPPHIRE_RUST_GROUP": "ios", "RUNNER_OS": "Linux"},
            {"SAPPHIRE_RUST_LINT": "yes"},
        ):
            with self.subTest(updates=updates), self.assertRaises(ValueError):
                setup_rust.setup(self.root, self.env | updates)
        self.run.assert_not_called()
        self.output.assert_not_called()

    def test_missing_manifest_target_fails_before_rustup(self):
        path = self.root / "rust-toolchain.toml"
        path.write_text(path.read_text().replace('"aarch64-apple-ios-sim", ', ""))
        self.env.update(SAPPHIRE_RUST_GROUP="ios", RUNNER_OS="macOS")
        with self.assertRaisesRegex(ValueError, "must be declared"):
            self.invoke()
        self.run.assert_not_called()

    def test_moving_channel_cannot_be_installed(self):
        path = self.root / "rust-toolchain.toml"
        path.write_text(path.read_text().replace('"1.99.0"', '"stable"'))
        with self.assertRaisesRegex(ValueError, "exact Rust version"):
            self.invoke()
        self.run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
