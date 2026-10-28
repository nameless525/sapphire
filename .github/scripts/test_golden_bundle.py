"""Golden fanout must preserve bytes/modes and reject stale or malformed inputs."""

from contextlib import redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import golden_bundle


class GoldenBundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.workspace = self.root / "source"
        self.archive = self.root / "bundle.tar.gz"
        self.destination = self.root / "consumer"
        self.commit = "a" * 40
        self.enterContext(redirect_stdout(io.StringIO()))

    def fixture(self, family="vulkan-lavapipe"):
        suffix = golden_bundle.FAMILIES[family]
        files = {
            f"target/debug/sapphire_sandbox{suffix}": b"sandbox bytes",
            f"target/debug/sapphire_testkit{suffix}": b"testkit bytes",
            "target/content/desktop/shared.spack": b"shared pack bytes",
            "target/content/desktop/zones/forest.spack": b"zone pack bytes",
        }
        if family == "dx12-warp":
            files["target/debug/d3d10warp.dll"] = b"pinned driver bytes"
            files["target/debug/sapphire_sandbox.pdb"] = b"optional symbols"
        for name, content in files.items():
            path = self.workspace / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        return golden_bundle.create(self.workspace, self.archive, self.commit, family)

    def rewrite(self, mutate):
        with tarfile.open(self.archive, "r:gz") as source:
            entries = [(member, source.extractfile(member).read()) for member in source.getmembers()]
        entries = mutate(entries)
        with tarfile.open(self.archive, "w:gz") as target:
            for member, data in entries:
                member.size = len(data)
                target.addfile(member, io.BytesIO(data))

    def assert_refused(self, **kwargs):
        with self.assertRaises((ValueError, KeyError)):
            golden_bundle.unpack(self.archive, self.destination, kwargs.get("commit", self.commit), kwargs.get("family", "vulkan-lavapipe"))
        self.assertFalse(self.destination.exists())

    def test_linux_roundtrip_keeps_bytes_and_executable_permissions(self):
        expected = self.fixture()
        actual = golden_bundle.unpack(self.archive, self.destination, self.commit, "vulkan-lavapipe")
        self.assertEqual(actual, expected)
        for name, record in actual["files"].items():
            path = self.destination / name
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), record["sha256"])
            if os.name != "nt":
                self.assertEqual(path.stat().st_mode & 0o777, record["mode"])
        self.assertFalse((self.destination / "target").exists())

    def test_windows_roundtrip_includes_driver_and_optional_symbols_beside_executables(self):
        self.fixture("dx12-warp")
        golden_bundle.unpack(self.archive, self.destination, self.commit, "dx12-warp")
        self.assertEqual((self.destination / "bin/d3d10warp.dll").read_bytes(), b"pinned driver bytes")
        self.assertTrue((self.destination / "bin/sapphire_sandbox.exe").is_file())
        self.assertTrue((self.destination / "bin/sapphire_sandbox.pdb").is_file())

    def test_other_commit_or_family_never_extracts(self):
        self.fixture()
        self.assert_refused(commit="b" * 40)
        self.assert_refused(family="dx12-warp")

    def test_tampered_payload_is_removed_before_any_execution(self):
        self.fixture()
        self.rewrite(lambda entries: [(member, b"corrupt bytes" if member.name.startswith("bin/") else data) for member, data in entries])
        self.assert_refused()

    def test_same_size_corruption_fails_checksum(self):
        self.fixture()
        self.rewrite(lambda entries: [(member, bytes([data[0] ^ 1]) + data[1:] if member.name.startswith("bin/") else data) for member, data in entries])
        self.assert_refused()

    def test_missing_payload_is_rejected(self):
        self.fixture()
        self.rewrite(lambda entries: [item for item in entries if item[0].name != "content/shared.spack"])
        self.assert_refused()

    def test_duplicate_member_is_rejected(self):
        self.fixture()
        self.rewrite(lambda entries: entries + [entries[-1]])
        self.assert_refused()

    def test_traversal_and_windows_path_names_are_rejected_even_if_in_manifest(self):
        for name in ("../escape", "/escape", "content/../../escape", "content/..\\escape", "content/C:escape"):
            with self.subTest(name=name):
                self.fixture()
                def mutate(entries):
                    metadata, encoded = entries[0]
                    manifest = json.loads(encoded)
                    data = b"escape"
                    manifest["files"][name] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "mode": 0o644}
                    extra = tarfile.TarInfo(name)
                    return [(metadata, json.dumps(manifest).encode())] + entries[1:] + [(extra, data)]
                self.rewrite(mutate)
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

    def test_fresh_destination_is_required(self):
        self.fixture()
        self.destination.mkdir()
        (self.destination / "keep").write_text("keep")
        with self.assertRaisesRegex(ValueError, "fresh"):
            golden_bundle.unpack(self.archive, self.destination, self.commit, "vulkan-lavapipe")
        self.assertEqual((self.destination / "keep").read_text(), "keep")

    def test_missing_driver_or_uncooked_content_cannot_be_packaged(self):
        self.fixture("dx12-warp")
        (self.workspace / "target/debug/d3d10warp.dll").unlink()
        with self.assertRaisesRegex(ValueError, "Missing"):
            golden_bundle.create(self.workspace, self.archive, self.commit, "dx12-warp")
        (self.workspace / "target/content/desktop/shared.spack").unlink()
        with self.assertRaisesRegex(ValueError, "completed desktop cook"):
            golden_bundle.create(self.workspace, self.archive, self.commit, "dx12-warp")

    def test_checkout_identity_is_verified(self):
        with patch.object(golden_bundle.subprocess, "check_output", return_value=self.commit + "\n") as run:
            golden_bundle.verify_checkout(self.commit)
            with self.assertRaisesRegex(ValueError, "does not match"):
                golden_bundle.verify_checkout("b" * 40)
            self.assertEqual(run.call_count, 2)
            with self.assertRaisesRegex(ValueError, "exact Git"):
                golden_bundle.verify_checkout("main")
            self.assertEqual(run.call_count, 2)


if __name__ == "__main__":
    unittest.main()
