"""The generated figures' clips: what's committed is what the posing makes, and a hand lets go of the
weapon as a blend starts."""
import importlib
import re
import unittest

import figure_pose as fp

# Every figure `tools/figures/` specifies, by its posing module here, which shares its name.
FIGURES = [(importlib.import_module(spec.name.removesuffix('.figure.ron')), spec.name.removesuffix('.figure.ron'))
           for spec in sorted((fp.REPO / 'tools/figures').glob('*.figure.ron'))]
NUMBER = re.compile(r'-?\d+(?:\.\d+)?')
# Each number is written to 4 decimals, so another machine's libm can round one the other way.
TOLERANCE = 2e-4
# An angle on the wrap reads -180 on one machine and 180 on another (Linux's libm put the boss's
# forearm on the other side), the same turn, so numbers are compared as turns.
FULL_TURN = 360.0
# How far a hand or foot may fall short of its target: a first draft's, read against the figures'
# reach (the Lancer's arm is 0.47 m, the boss's 1.06 m).
MAX_MISS = 0.06


class FigureClipTests(unittest.TestCase):
    def test_the_committed_clips_are_what_the_posing_makes(self):
        self.assertTrue(FIGURES)
        for module, name in FIGURES:
            folder = fp.REPO / 'content/combat' / name
            rig = fp.Rig(folder / f'{name}.glb')
            for clip in module.CLIPS:
                samples = fp.sample(rig, clip)
                path = folder / f'{clip.name}.keypose.ron'
                made, committed = fp.ron(f'{name}.py', clip, samples), path.read_text()
                self.assertEqual(NUMBER.sub('#', made), NUMBER.sub('#', committed),
                                 f'{path.name}: run tools/motion/{name}.py --write')
                for a, b in zip(NUMBER.findall(made), NUMBER.findall(committed)):
                    apart = abs(float(a) - float(b)) % FULL_TURN
                    self.assertLessEqual(min(apart, FULL_TURN - apart), TOLERANCE, path.name)
                meta = path.with_suffix('.ron.meta')
                self.assertEqual(fp.meta(module.RIG_ID, name, clip.name), meta.read_text(), meta.name)

    def test_no_limb_falls_far_short_and_no_sole_goes_under_the_ground(self):
        for module, name in FIGURES:
            rig = fp.Rig(fp.REPO / 'content/combat' / name / f'{name}.glb')
            for clip in module.CLIPS:
                for tick, _, pose, miss in fp.sample(rig, clip):
                    self.assertLess(miss, MAX_MISS, f'{clip.name} tick {tick}')
                    self.assertGreater(min(fp.soles(rig, pose)), -0.015, f'{clip.name} tick {tick}')

    def test_a_hand_lets_go_as_a_blend_starts_and_takes_hold_as_it_ends(self):
        holding = dict(fp.DEFAULTS, hold=0.3, reach=None, tick=0)
        free = dict(fp.DEFAULTS, hold=None, reach=(0.4, 1.0, 0.1), tick=10)
        self.assertEqual(fp.blend(holding, free, 0.0)['hold'], 0.3)
        let_go = fp.blend(holding, free, 0.1)
        self.assertEqual((let_go['hold'], let_go['reach']), (None, (0.4, 1.0, 0.1)))
        taking = fp.blend(free, holding, 0.9)
        self.assertEqual((taking['hold'], taking['reach']), (None, (0.4, 1.0, 0.1)))
        self.assertEqual(fp.blend(free, holding, 1.0)['hold'], 0.3)


if __name__ == '__main__':
    unittest.main()
