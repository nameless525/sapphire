"""A zone's ambient cast's clips (M5 C10): what's committed is what `ambient.py` makes, every limb reaches
its target, and each gesture starts and ends where standing is."""
import re
import unittest

import ambient
import figure_pose as fp

# As `test_figure_pose.py` compares a figure's clips: 4 decimals written, so another machine's libm can
# round one the other way, and an angle on the wrap compared as a turn.
NUMBER = re.compile(r'-?\d+(?:\.\d+)?')
TOLERANCE = 2e-4
FULL_TURN = 360.0


class AmbientClipTests(unittest.TestCase):
    def setUp(self):
        self.rig = fp.Rig(fp.REPO / ambient.MODEL)

    def test_the_committed_clips_are_what_the_posing_makes(self):
        folder = fp.REPO / ambient.FOLDER
        for clip in ambient.CLIPS:
            path = folder / f'{clip.name}.keypose.ron'
            made, committed = fp.ron('ambient.py', clip, fp.sample(self.rig, clip)), path.read_text()
            self.assertEqual(NUMBER.sub('#', made), NUMBER.sub('#', committed), f'{path.name}: run tools/motion/ambient.py --write')
            for a, b in zip(NUMBER.findall(made), NUMBER.findall(committed)):
                apart = abs(float(a) - float(b)) % FULL_TURN
                self.assertLessEqual(min(apart, FULL_TURN - apart), TOLERANCE, path.name)
            meta = path.with_suffix('.ron.meta')
            self.assertEqual(fp.meta(ambient.RIG_ID, 'ambient', clip.name), meta.read_text(), meta.name)

    def test_every_limb_reaches_and_no_sole_goes_under_the_ground(self):
        for clip in ambient.CLIPS:
            for tick, _, pose, miss in fp.sample(self.rig, clip):
                self.assertLess(miss, 0.01, f'{clip.name} tick {tick}')
                self.assertGreater(min(fp.soles(self.rig, pose)), -0.015, f'{clip.name} tick {tick}')

    def test_every_clip_starts_and_ends_standing(self):
        standing = fp.sample(self.rig, ambient.CLIPS[0])[0][2].turns
        for clip in ambient.CLIPS:
            samples = fp.sample(self.rig, clip)
            self.assertEqual(samples[-1][0], clip.ticks, clip.name)
            for _, _, pose, _ in (samples[0], samples[-1]):
                for bone, turn in pose.turns.items():
                    for a, b in zip(turn, standing[bone]):
                        self.assertAlmostEqual(a, b, places=6, msg=f'{clip.name} {bone}')


if __name__ == '__main__':
    unittest.main()
