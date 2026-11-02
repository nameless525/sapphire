"""Who writes what: the key-pose documents are this tool's, and `player.moves.ron` is tuned in place.

`python3 -B -m unittest discover -s tools/motion -p 'test_*.py'`. Everything happens in a temp folder. Nothing here
compares with what's committed, which tuning moves on purpose, and whose poses and blade socket go through each
platform's libm, so what's generated can't be held byte for byte across OSes.
"""
import contextlib
import io
import os
import sys
import tempfile
import types
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import frames
import moves


def clips():
    """Each move's clip, as `frames.py` wants it: only its length counts."""
    return {d['name']: types.SimpleNamespace(ticks=sum(d['frames'])) for d in frames.MOVES}


class Folder(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.folder = temp.name
        self.stamp = os.path.join(self.folder, 'stamp')
        self.numbers = os.path.join(self.folder, frames.NUMBERS)

    def read(self, path=None):
        with open(path or self.numbers, newline='') as f:
            return f.read()

    def tune(self, text='light_1 hits harder\n'):
        with open(self.numbers, 'a') as f:
            f.write(text)
        return self.read()

    def run_tool(self, *args):
        """`moves.py` on this folder, and what it printed."""
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            moves.main(list(args), folder=self.folder, stamp=self.stamp)
        return out.getvalue()


class WritingNumbers(Folder):
    def test_it_writes_a_file_it_doesnt_find_and_again_changes_nothing(self):
        self.assertTrue(frames.write_numbers(self.folder, clips(), self.stamp))
        written, stamped = self.read(), self.read(self.stamp)
        self.assertIn('startup: 7', written)
        self.assertFalse(frames.write_numbers(self.folder, clips(), self.stamp), 'a clean checkout changes nothing')
        self.assertEqual((self.read(), self.read(self.stamp)), (written, stamped))

    def test_it_refuses_a_file_changed_since_it_last_wrote_it(self):
        frames.write_numbers(self.folder, clips(), self.stamp)
        stamped = self.read(self.stamp)
        tuned = self.tune()
        with self.assertRaises(SystemExit) as refused:
            frames.write_numbers(self.folder, clips(), self.stamp)
        said = str(refused.exception.code)
        self.assertIn('nothing was written', said)
        self.assertIn('--force', said)
        self.assertIn('light_1 hits harder', said, 'it shows what would be lost')
        self.assertEqual((self.read(), self.read(self.stamp)), (tuned, stamped))

    def test_it_refuses_a_file_it_never_wrote(self):
        tuned = self.tune()
        with self.assertRaises(SystemExit):
            frames.write_numbers(self.folder, clips(), self.stamp)
        self.assertEqual(self.read(), tuned)

    def test_force_replaces_an_edited_file_and_makes_it_the_tools_again(self):
        frames.write_numbers(self.folder, clips(), self.stamp)
        written = self.read()
        self.tune()
        self.assertTrue(frames.write_numbers(self.folder, clips(), self.stamp, force=True))
        self.assertEqual(self.read(), written)
        self.assertFalse(frames.write_numbers(self.folder, clips(), self.stamp))

    def test_a_change_of_its_own_numbers_goes_through(self):
        frames.write_numbers(self.folder, clips(), self.stamp)
        before = self.read()
        changed = dict(frames.MOVES[0], cues=[('swing', 7)])
        original = frames.MOVES[0]
        frames.MOVES[0] = changed
        self.addCleanup(frames.MOVES.__setitem__, 0, original)
        self.assertTrue(frames.write_numbers(self.folder, clips(), self.stamp), 'the file was as it left it')
        self.assertNotEqual(self.read(), before)

    def test_a_file_tuned_to_what_it_would_write_is_its_own_again(self):
        # Numbers copied into `frames.py` until the two agree: there's nothing left to lose.
        frames.write_numbers(self.folder, clips(), self.stamp)
        self.tune()
        with open(self.stamp, 'w') as f:
            f.write('someone else\n')
        with open(self.numbers, 'w', newline='') as f:
            f.write(frames.numbers_text(clips()))
        self.assertFalse(frames.write_numbers(self.folder, clips(), self.stamp))
        self.tune()
        with self.assertRaises(SystemExit):
            frames.write_numbers(self.folder, clips(), self.stamp)


class TheTool(Folder):
    def test_write_leaves_the_numbers_alone_and_says_when_they_differ(self):
        tuned = self.tune()
        said = self.run_tool('--write')
        self.assertEqual(self.read(), tuned)
        self.assertFalse(os.path.exists(self.stamp))
        for name in frames.IDS:
            self.assertTrue(os.path.exists(os.path.join(self.folder, name + '.keypose.ron')), name)
        self.assertIn('--write-numbers', said)

    def test_write_numbers_and_force_reach_the_file(self):
        said = self.run_tool('--write-numbers')
        self.assertIn('SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline', said)
        self.assertIn('cargo xtask replay --update', said)
        self.assertNotIn('--write-numbers', self.run_tool('--write'), 'nothing to say: they agree')
        written = self.read()

        self.tune()
        with self.assertRaises(SystemExit):
            self.run_tool('--write-numbers')
        self.run_tool('--write-numbers', '--force')
        self.assertEqual(self.read(), written)

    def test_force_alone_means_nothing(self):
        with self.assertRaises(SystemExit):
            self.run_tool('--force')

    def test_write_says_when_frames_no_longer_fit_a_clip(self):
        too_long = dict(frames.MOVES[0], frames=(7, 4, 24))
        original = frames.MOVES[0]
        frames.MOVES[0] = too_long
        self.addCleanup(frames.MOVES.__setitem__, 0, original)
        said = self.run_tool('--write')
        self.assertIn('frames.py has light_1 at 35 ticks and its clip at 34', said)


if __name__ == '__main__':
    unittest.main()
