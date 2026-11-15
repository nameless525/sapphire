"""The GPU tests' shards must run every listed test exactly once between them, and say so when one doesn't run."""

from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import unittest
import zlib

import gpu_test_shards
from app_tests_bundle import MANIFEST

# Windows' limit on a process's command line, and a test executable's path as deep as a runner puts it.
WINDOWS_COMMAND_LINE = 32767
EXECUTABLE = "D:\\a\\sapphire\\sapphire\\target\\debug\\deps\\gpu-5dabef7bd7bad868.exe"


def suite(count, length=60, seed=7):
    """`count` distinct names shaped like the suite's, `module::a_long_descriptive_name`."""
    generator = random.Random(seed)
    names = set()
    while len(names) < count:
        words = "_".join(generator.choice(("lines", "shadow", "every", "tier", "holds", "ramp", "edge")) for _ in range(12))
        names.add(f"family_{generator.randrange(40)}::{words}"[:length] + f"_{len(names)}")
    return sorted(names)


class FakeExecutables:
    """Test executables as a dictionary: each lists its names and runs those it is given, as libtest does."""

    def __init__(self, workspace, listed, exit_status=None, skip=None, crash=None):
        self.workspace = workspace
        self.listed = listed
        self.exit_status = exit_status or {}
        self.skip = skip or set()
        self.crash = crash or set()
        self.ran = []

    def __call__(self, command, cwd, echo):
        target = Path(command[0]).name
        names = self.listed[target]
        assert cwd == self.workspace / f"crates/{target}", cwd
        if command[1:] == gpu_test_shards.LIST:
            return 0, "".join(f"{name}: test\n" for name in names) + "a_bench: bench\n"
        self.assertRunCommand(command)
        given = command[1 + len(gpu_test_shards.RUN):]
        if target in self.crash:
            return 101, f"running {len(given)} tests\n"
        ran = [name for name in given if name in names and name not in self.skip]
        self.ran.extend((target, name) for name in ran)
        lines = [f"test {name} ... ok\n" for name in ran]
        lines.append(f"test result: ok. {len(ran)} passed; 0 failed; 0 ignored; 0 measured; {len(names) - len(ran)} filtered out; finished in 0.1s\n")
        return self.exit_status.get(target, 0), "".join(lines)

    @staticmethod
    def assertRunCommand(command):
        assert command[1:1 + len(gpu_test_shards.RUN)] == gpu_test_shards.RUN, command


class GpuTestShardsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.workspace = Path(self.temp.name)
        self.output = self.enterContext(redirect_stdout(io.StringIO()))

    def bundle(self, targets):
        """A manifest as `unpack` writes it, with each target its own package under crates/."""
        tests = {target: {"path": f"target/debug/deps/{target}", "package": f"crates/{target}"} for target in targets}
        for test in tests.values():
            (self.workspace / test["package"]).mkdir(parents=True, exist_ok=True)
        (self.workspace / MANIFEST).parent.mkdir(parents=True, exist_ok=True)
        (self.workspace / MANIFEST).write_text(json.dumps({"tests": tests}))

    def test_the_shard_is_crc32_of_the_name_so_it_never_depends_on_order_or_on_other_tests(self):
        names = suite(400)
        self.assertEqual(gpu_test_shards.shard_of("outline::lines_ride_out_a_render_scale_step", 4),
                         zlib.crc32(b"outline::lines_ride_out_a_render_scale_step") % 4 + 1)
        before = {name: gpu_test_shards.shard_of(name, 4) for name in names}
        grown = names + suite(200, seed=8)
        random.Random(1).shuffle(grown)
        after = {name: gpu_test_shards.shard_of(name, 4) for name in grown}
        self.assertEqual({name: after[name] for name in names}, before)

    def test_every_name_lands_in_exactly_one_shard(self):
        names = suite(600)
        for shards in range(1, 9):
            with self.subTest(shards=shards):
                held = [{name for name in names if gpu_test_shards.shard_of(name, shards) == shard}
                        for shard in range(1, shards + 1)]
                self.assertEqual(set().union(*held), set(names))
                self.assertEqual(sum(len(h) for h in held), len(names))
                self.assertEqual(gpu_test_shards.shard_counts({"gpu": names}, shards), [len(h) for h in held])
                # Four shards of 600 hold 150 each on average; the hash keeps each well away from empty.
                if shards > 1:
                    self.assertTrue(all(len(h) > len(names) / shards / 2 for h in held), [len(h) for h in held])

    def test_chunks_keep_each_command_line_under_half_of_windows_limit_at_twice_the_suite(self):
        # Twice the 483 tests of 2026-11-15, each as long as the longest then (116), all in one shard.
        names = suite(966, length=116)
        chunked = list(gpu_test_shards.chunks(names))
        self.assertGreater(len(chunked), 1)
        self.assertEqual([name for chunk in chunked for name in chunk], names)
        for chunk in chunked:
            line = subprocess.list2cmdline([EXECUTABLE, *gpu_test_shards.RUN, *chunk])
            self.assertLess(len(line), WINDOWS_COMMAND_LINE // 2)
        self.assertEqual(list(gpu_test_shards.chunks([])), [])
        with self.assertRaisesRegex(ValueError, "longer"):
            list(gpu_test_shards.chunks(["x" * 100], limit=50))

    def test_the_list_holds_tests_and_nothing_else(self):
        output = "render::a: test\nrender::b: test\nsome_bench: bench\n\n2 tests, 1 benchmark\nnoise from start-up\n"
        self.assertEqual(gpu_test_shards.names_from_list(output), ["render::a", "render::b"])

    def test_the_shards_run_every_listed_test_once_and_a_shared_name_in_both_executables(self):
        listed = {
            "sapphire_render": suite(150, seed=1) + ["gpu_tests::shared_name"],
            "gpu": suite(300, seed=2) + ["gpu_tests::shared_name"],
            "sapphire_rhi": suite(9, seed=3),
        }
        self.bundle(listed)
        packages = [f"crates/{target}" for target in listed]
        ran = []
        for shard in (1, 2, 3):
            fake = FakeExecutables(self.workspace, listed)
            self.assertEqual(gpu_test_shards.run_shard(self.workspace, packages, shard, 3, fake), 0)
            ran.extend(fake.ran)
        expected = sorted((target, name) for target, names in listed.items() for name in names)
        self.assertEqual(sorted(ran), expected)
        total = len({name for names in listed.values() for name in names})
        self.assertIn(f"of {total} tests (1/3 ", self.output.getvalue())

    def test_a_package_not_named_is_not_run(self):
        listed = {"gpu": suite(20, seed=1), "sapphire_stage": suite(20, seed=2)}
        self.bundle(listed)
        fake = FakeExecutables(self.workspace, listed)
        for shard in (1, 2):
            gpu_test_shards.run_shard(self.workspace, ["crates/gpu"], shard, 2, fake)
        self.assertEqual({target for target, _ in fake.ran}, {"gpu"})

    def test_a_failure_a_crash_or_a_test_not_run_fails_the_shard_and_the_rest_still_runs(self):
        listed = {"a": suite(40, seed=1), "b": suite(40, seed=2), "c": suite(40, seed=3)}
        self.bundle(listed)
        packages = ["crates/a", "crates/b", "crates/c"]
        missing = next(name for name in listed["a"] if gpu_test_shards.shard_of(name, 2) == 1)
        for fault in ({"exit_status": {"a": 101}}, {"crash": {"a"}}, {"skip": {missing}}):
            with self.subTest(fault=fault):
                fake = FakeExecutables(self.workspace, listed, **fault)
                self.assertEqual(gpu_test_shards.run_shard(self.workspace, packages, 1, 2, fake), 1)
                self.assertEqual({target for target, _ in fake.ran} & {"b", "c"}, {"b", "c"})

    def test_a_package_with_no_executable_or_a_list_that_fails_is_an_error(self):
        listed = {"gpu": suite(10)}
        self.bundle(listed)
        with self.assertRaisesRegex(ValueError, "no test executable from crates/gpu_typo"):
            gpu_test_shards.run_shard(self.workspace, ["crates/gpu", "crates/gpu_typo"], 1, 2, FakeExecutables(self.workspace, listed))
        with self.assertRaisesRegex(ValueError, "No shard 3 of 2"):
            gpu_test_shards.run_shard(self.workspace, ["crates/gpu"], 3, 2, FakeExecutables(self.workspace, listed))
        with self.assertRaisesRegex(ValueError, "Listing gpu's tests exited with 1"):
            gpu_test_shards.run_shard(self.workspace, ["crates/gpu"], 1, 2, lambda command, cwd, echo: (1, "error\n"))

    @unittest.skipIf(os.name == "nt", "a script stands in for a test executable")
    def test_launch_runs_in_the_directory_and_returns_the_status_and_output(self):
        script = self.workspace / "fake_test"
        script.write_text(f"#!{sys.executable}\nimport os, sys\nprint(os.getcwd()); print(' '.join(sys.argv[1:]))\nsys.exit(3)\n")
        script.chmod(0o755)
        status, output = gpu_test_shards.launch([str(script), "--exact", "a::b"], self.workspace, True)
        self.assertEqual(status, 3)
        cwd, arguments = output.splitlines()
        self.assertEqual(Path(cwd).resolve(), self.workspace.resolve())
        self.assertEqual(arguments, "--exact a::b")


if __name__ == "__main__":
    unittest.main()
