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

    def message(self, package, test, executable, kind=("test",), test_profile=None):
        return json.dumps({
            "reason": "compiler-artifact",
            "manifest_path": str(self.workspace / package / "Cargo.toml"),
            "target": {"name": test, "kind": list(kind)},
            # A library built for its unit tests has the test profile and an executable.
            "profile": {"test": kind == ("test",) if test_profile is None else test_profile},
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
            "target/content/desktop/release.smanifest": b"the cook's manifest",
            # The district's folder: the cook's files but its manifest, the district's pack and a manifest of both.
            "target/bench/ci/with-content/desktop/release.smanifest": b"the cook and the district's manifest",
            "target/bench/ci/with-content/desktop/zone_district.spack": b"district pack bytes",
            # Neither the loose cook nor another build's output goes along.
            "target/content/loose/desktop/mesh.smesh": b"loose bytes",
            "target/debug/deps/libsapphire_core-89ab.rlib": b"library bytes",
        }
        for name, content in files.items():
            path = self.workspace / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        for name in ("shared.spack", "zones/ward.spack"):
            path = self.workspace / app_tests_bundle.DISTRICT / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.unlink(missing_ok=True)
            os.link(self.workspace / app_tests_bundle.CONTENT / name, path)
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
        # The district's folder travels as its own two files; the cook is laid out in it again on unpack.
        district = self.consumer / app_tests_bundle.DISTRICT
        self.assertEqual(
            {name for name in actual["files"] if name.startswith(app_tests_bundle.DISTRICT + "/")},
            app_tests_bundle.district_members(),
        )
        self.assertEqual(
            sorted(path.relative_to(district).as_posix() for path in district.rglob("*") if path.is_file()),
            ["release.smanifest", "shared.spack", "zone_district.spack", "zones/ward.spack"],
        )
        self.assertEqual((district / "zones/ward.spack").read_bytes(), b"zone pack bytes")
        self.assertEqual((district / "release.smanifest").read_bytes(), b"the cook and the district's manifest")
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

    def fixture_tests_only(self):
        """The GPU tests' bundle: an integration test and a library's unit tests, with no sandbox or cook."""
        self.fixture()
        (self.workspace / "target/debug/deps/sapphire_render-0a1b2c3d").write_bytes(b"unit test bytes")
        (self.workspace / "crates/sapphire_render").mkdir(parents=True, exist_ok=True)
        (self.consumer / "crates/sapphire_render").mkdir(parents=True, exist_ok=True)
        lines = [
            self.message("apps/sapphire_sandbox", "replays", "target/debug/deps/replays-4567ef01"),
            self.message("crates/sapphire_render", "sapphire_render", "target/debug/deps/sapphire_render-0a1b2c3d",
                         kind=("lib",), test_profile=True),
            self.message("crates/sapphire_core", "sapphire_core", None, kind=("lib",)),
        ]
        self.messages.write_text("\n".join(lines) + "\n")
        return app_tests_bundle.create(self.workspace, self.archive, self.commit, [self.messages], tests_only=True)

    def test_tests_only_carries_unit_and_integration_tests_and_nothing_else(self):
        expected = self.fixture_tests_only()
        self.assertTrue(expected["tests_only"])
        self.assertEqual(
            expected["tests"],
            {
                "replays": {"path": "target/debug/deps/replays-4567ef01", "package": "apps/sapphire_sandbox"},
                "sapphire_render": {"path": "target/debug/deps/sapphire_render-0a1b2c3d", "package": "crates/sapphire_render"},
            },
        )
        self.assertEqual(set(expected["files"]), {test["path"] for test in expected["tests"].values()})
        actual = app_tests_bundle.unpack(self.consumer, self.archive, self.commit)
        self.assertEqual(actual, expected)
        self.assertEqual((self.consumer / "target/debug/deps/sapphire_render-0a1b2c3d").read_bytes(), b"unit test bytes")
        self.assertFalse((self.consumer / "target/debug/sapphire_sandbox").exists())
        self.assertFalse((self.consumer / "target/content").exists())

    def test_tests_only_refuses_the_sandbox_or_the_cook(self):
        for name in ("target/debug/sapphire_sandbox", "target/content/desktop/shared.spack"):
            with self.subTest(name=name):
                self.fixture_tests_only()
                def mutate(entries):
                    metadata, encoded = entries[0]
                    manifest = json.loads(encoded)
                    data = b"extra"
                    mode = 0o755 if name in app_tests_bundle.BINARIES else 0o644
                    manifest["files"][name] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "mode": mode}
                    return [(metadata, json.dumps(manifest).encode())] + entries[1:] + [(tarfile.TarInfo(name), data)]
                self.rewrite(mutate)
                self.assert_refused()

    def test_a_manifest_that_does_not_say_whether_it_is_tests_only_is_refused(self):
        for value in (None, "yes"):
            with self.subTest(value=value):
                self.fixture()
                def change(manifest):
                    if value is None:
                        del manifest["tests_only"]
                    else:
                        manifest["tests_only"] = value
                self.rewrite_manifest(change)
                self.assert_refused()

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

    def test_a_district_in_parts_travels_whole(self):
        part = app_tests_bundle.DISTRICT + "/zone_district.1.spack"
        self.fixture()
        (self.workspace / part).write_bytes(b"the district's second part")
        expected = app_tests_bundle.create(self.workspace, self.archive, self.commit, [self.messages])
        actual = app_tests_bundle.unpack(self.consumer, self.archive, self.commit)
        self.assertEqual(actual, expected)
        self.assertIn(part, actual["files"])
        self.assertEqual((self.consumer / part).read_bytes(), b"the district's second part")
        self.assertFalse(app_tests_bundle.is_district_member(app_tests_bundle.DISTRICT + "/zone_district.0.spack"))

    def test_a_district_folder_that_isnt_the_cook_with_the_district_beside_it_cannot_be_packaged(self):
        district = self.workspace / app_tests_bundle.DISTRICT
        cases = (
            ("no district pack", lambda: (district / "zone_district.spack").unlink(), "must hold zone_district.spack"),
            ("a cook file changed", lambda: self.replace(district / "shared.spack", b"other bytes"), "isn't the desktop cook's"),
            ("a file the cook lacks", lambda: (district / "extra.spack").write_bytes(b"extra"), "isn't the desktop cook's"),
            ("a cook file missing", lambda: (district / "zones/ward.spack").unlink(), "lacks the desktop cook's"),
        )
        for case, spoil, message in cases:
            with self.subTest(case=case):
                self.fixture()
                spoil()
                with self.assertRaisesRegex(ValueError, message):
                    app_tests_bundle.create(self.workspace, self.archive, self.commit, [self.messages])
                (district / "extra.spack").unlink(missing_ok=True)

    def replace(self, path, data):
        path.unlink()
        path.write_bytes(data)

    def test_the_district_folder_takes_only_its_own_files_and_needs_them(self):
        for name in ("target/bench/ci/with-content/desktop/shared.spack",
                     "target/bench/ci/with-content/desktop/other.spack",
                     "target/bench/ci/with-content/mobile/zone_district.spack"):
            with self.subTest(name=name):
                self.fixture()
                def mutate(entries):
                    metadata, encoded = entries[0]
                    manifest = json.loads(encoded)
                    data = b"extra"
                    manifest["files"][name] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "mode": 0o644}
                    return [(metadata, json.dumps(manifest).encode())] + entries[1:] + [(tarfile.TarInfo(name), data)]
                self.rewrite(mutate)
                self.assert_refused()
        for name in sorted(app_tests_bundle.district_members()):
            with self.subTest(missing=name):
                self.fixture()
                def drop(entries):
                    metadata, encoded = entries[0]
                    manifest = json.loads(encoded)
                    del manifest["files"][name]
                    kept = [item for item in entries[1:] if item[0].name != name]
                    return [(metadata, json.dumps(manifest).encode())] + kept
                self.rewrite(drop)
                self.assert_refused()

    def test_tests_only_lays_out_no_district(self):
        self.fixture_tests_only()
        app_tests_bundle.unpack(self.consumer, self.archive, self.commit)
        self.assertFalse((self.consumer / "target/bench").exists())

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
