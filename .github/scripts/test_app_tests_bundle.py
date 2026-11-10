"""The App tests' fanout must put each file back where Cargo built it, and refuse stale or malformed inputs."""

from contextlib import redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest

import app_tests_bundle


class AppTestsBundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.workspace = self.root / "producer"
        self.consumer = self.root / "consumer"
        self.archive = self.root / "bundle.tar.gz"
        self.messages = self.root / "messages.jsonl"
        self.commit = "a" * 40
        self.enterContext(redirect_stdout(io.StringIO()))

    def message(self, package, test, executable, kind=("test",)):
        return json.dumps({
            "reason": "compiler-artifact",
            "manifest_path": str(self.workspace / package / "Cargo.toml"),
            "target": {"name": test, "kind": list(kind)},
            "executable": str(self.workspace / executable) if executable else None,
        })

    def fixture(self, extra_messages=()):
        files = {
            "target/debug/sapphire_sandbox": b"sandbox bytes",
            "target/debug/sapphire_livelink": b"live-link bytes",
            "target/debug/deps/wins_the_ward-0123abcd": b"walk test bytes",
            "target/debug/deps/replays-4567ef01": b"replays test bytes",
            "target/content/desktop/shared.spack": b"shared pack bytes",
            "target/content/desktop/zones/ward.spack": b"zone pack bytes",
            # Neither the loose cook nor another build's output goes along.
            "target/content/loose/desktop/mesh.smesh": b"loose bytes",
            "target/debug/deps/libsapphire_core-89ab.rlib": b"library bytes",
        }
        for name, content in files.items():
            path = self.workspace / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        for package in ("game/game_app", "apps/sapphire_sandbox"):
            (self.workspace / package).mkdir(parents=True, exist_ok=True)
            (self.consumer / package).mkdir(parents=True, exist_ok=True)
        lines = [
            "not a message: Cargo's stdout carries only JSON, but a stray line must not matter",
            self.message("game/game_app", "wins_the_ward", "target/debug/deps/wins_the_ward-0123abcd"),
            self.message("apps/sapphire_sandbox", "replays", "target/debug/deps/replays-4567ef01"),
            # The same test built again by a later command, fresh: the same executable.
            self.message("apps/sapphire_sandbox", "replays", "target/debug/deps/replays-4567ef01"),
            self.message("crates/sapphire_core", "sapphire_core", None, kind=("lib",)),
            *extra_messages,
        ]
        self.messages.write_text("\n".join(lines) + "\n")
        return app_tests_bundle.create(self.workspace, self.archive, self.commit, [self.messages])

    def rewrite(self, mutate):
        with tarfile.open(self.archive, "r:gz") as source:
            entries = [(member, source.extractfile(member).read()) for member in source.getmembers()]
        entries = mutate(entries)
        with tarfile.open(self.archive, "w:gz") as target:
            for member, data in entries:
                member.size = len(data)
                target.addfile(member, io.BytesIO(data))

    def rewrite_manifest(self, change):
        def mutate(entries):
            metadata, encoded = entries[0]
            manifest = json.loads(encoded)
            change(manifest)
            return [(metadata, json.dumps(manifest).encode())] + entries[1:]
        self.rewrite(mutate)

    def assert_refused(self, commit=None):
        with self.assertRaises((ValueError, KeyError)):
            app_tests_bundle.unpack(self.consumer, self.archive, commit or self.commit)
        self.assertFalse((self.consumer / "target").exists())

    def test_roundtrip_puts_each_file_where_cargo_built_it(self):
        expected = self.fixture()
        self.assertEqual(
            expected["tests"],
            {
                "wins_the_ward": {"path": "target/debug/deps/wins_the_ward-0123abcd", "package": "game/game_app"},
                "replays": {"path": "target/debug/deps/replays-4567ef01", "package": "apps/sapphire_sandbox"},
            },
        )
        actual = app_tests_bundle.unpack(self.consumer, self.archive, self.commit)
        self.assertEqual(actual, expected)
        for name, record in actual["files"].items():
            path = self.consumer / name
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), record["sha256"])
            if os.name != "nt":
                self.assertEqual(path.stat().st_mode & 0o777, record["mode"])
        self.assertEqual(actual["files"]["target/debug/sapphire_sandbox"]["mode"], 0o755)
        self.assertEqual(actual["files"]["target/debug/deps/replays-4567ef01"]["mode"], 0o755)
        self.assertEqual(actual["files"]["target/content/desktop/shared.spack"]["mode"], 0o644)
        self.assertFalse((self.consumer / "target/content/loose").exists())
        self.assertFalse((self.consumer / "target/debug/deps/libsapphire_core-89ab.rlib").exists())
        self.assertTrue((self.consumer / app_tests_bundle.TMPDIR).is_dir())

    @unittest.skipIf(os.name == "nt", "the lanes run on Linux")
    def test_run_starts_the_test_in_its_package_with_its_arguments(self):
        self.fixture()
        app_tests_bundle.unpack(self.consumer, self.archive, self.commit)
        script = self.consumer / "target/debug/deps/replays-4567ef01"
        out = self.root / "ran.json"
        script.write_text(
            f"#!{sys.executable}\nimport json, os, sys\n"
            f"json.dump([os.getcwd(), sys.argv[1:]], open({str(out)!r}, 'w'))\nsys.exit(3)\n"
        )
        status = app_tests_bundle.run(self.consumer, "replays", ["--ignored", "--exact", "a_test"])
        self.assertEqual(status, 3)
        cwd, arguments = json.loads(out.read_text())
        self.assertEqual(Path(cwd).resolve(), (self.consumer / "apps/sapphire_sandbox").resolve())
        self.assertEqual(arguments, ["--ignored", "--exact", "a_test"])
        with self.assertRaisesRegex(ValueError, "No app test named"):
            app_tests_bundle.run(self.consumer, "wins_the_wards", [])

    def test_two_executables_for_one_test_cannot_be_packaged(self):
        other = self.message("game/game_app", "wins_the_ward", "target/debug/deps/wins_the_ward-feedface")
        with self.assertRaisesRegex(ValueError, "Two executables"):
            self.fixture([other])

    def test_no_tests_or_an_uncooked_tree_cannot_be_packaged(self):
        self.fixture()
        self.messages.write_text(self.message("crates/sapphire_core", "sapphire_core", None, kind=("lib",)) + "\n")
        with self.assertRaisesRegex(ValueError, "no test executable"):
            app_tests_bundle.create(self.workspace, self.archive, self.commit, [self.messages])
        self.fixture()
        (self.workspace / "target/content/desktop/shared.spack").unlink()
        with self.assertRaisesRegex(ValueError, "completed desktop cook"):
            app_tests_bundle.create(self.workspace, self.archive, self.commit, [self.messages])
        (self.workspace / "target/content/desktop/shared.spack").write_bytes(b"shared pack bytes")
        (self.workspace / "target/debug/sapphire_livelink").unlink()
        with self.assertRaisesRegex(ValueError, "Missing"):
            app_tests_bundle.create(self.workspace, self.archive, self.commit, [self.messages])

    def test_other_commit_never_extracts(self):
        self.fixture()
        self.assert_refused(commit="b" * 40)

    def test_a_used_checkout_is_refused(self):
        self.fixture()
        (self.consumer / "target").mkdir()
        (self.consumer / "target/keep").write_text("keep")
        with self.assertRaisesRegex(ValueError, "fresh"):
            app_tests_bundle.unpack(self.consumer, self.archive, self.commit)
        self.assertEqual((self.consumer / "target/keep").read_text(), "keep")

    def test_same_size_corruption_fails_checksum(self):
        self.fixture()
        self.rewrite(lambda entries: [(member, bytes([data[0] ^ 1]) + data[1:] if "deps/" in member.name else data) for member, data in entries])
        self.assert_refused()

    def test_missing_payload_is_rejected(self):
        self.fixture()
        self.rewrite(lambda entries: [item for item in entries if item[0].name != "target/debug/sapphire_sandbox"])
        self.assert_refused()

    def test_duplicate_member_is_rejected(self):
        self.fixture()
        self.rewrite(lambda entries: entries + [entries[-1]])
        self.assert_refused()

    def test_a_test_outside_the_build_or_its_package_outside_the_checkout_is_rejected(self):
        for path, package in (
            ("target/debug/sapphire_sandbox", "game/game_app"),
            ("target/debug/deps/../../escape", "game/game_app"),
            ("target/debug/deps/wins_the_ward-0123abcd", "../outside"),
            ("target/debug/deps/wins_the_ward-0123abcd", "crates/not_checked_out"),
        ):
            with self.subTest(path=path, package=package):
                self.fixture()
                def change(manifest):
                    manifest["tests"]["wins_the_ward"] = {"path": path, "package": package}
                self.rewrite_manifest(change)
                self.assert_refused()

    def test_files_outside_the_inputs_are_rejected_even_if_in_manifest(self):
        for name in ("../escape", "/escape", "target/content/desktop/../../escape", "target/debug/other_tool",
                     "target/content/desktop\\escape", "target/content/C:escape", "src/main.rs"):
            with self.subTest(name=name):
                self.fixture()
                def mutate(entries):
                    metadata, encoded = entries[0]
                    manifest = json.loads(encoded)
                    data = b"escape"
                    manifest["files"][name] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "mode": 0o644}
                    return [(metadata, json.dumps(manifest).encode())] + entries[1:] + [(tarfile.TarInfo(name), data)]
                self.rewrite(mutate)
                self.assert_refused()

    def test_an_executable_must_stay_executable_and_content_must_not_be(self):
        for name, mode in (("target/debug/deps/replays-4567ef01", 0o644), ("target/content/desktop/shared.spack", 0o755)):
            with self.subTest(name=name):
                self.fixture()
                def change(manifest):
                    manifest["files"][name]["mode"] = mode
                self.rewrite_manifest(change)
                self.assert_refused()

    def test_links_are_rejected(self):
        self.fixture()
        def mutate(entries):
            member, data = entries[-1]
            member.type = tarfile.SYMTYPE
            member.linkname = "../../outside"
            return entries
        self.rewrite(mutate)
        self.assert_refused()


if __name__ == "__main__":
    unittest.main()
