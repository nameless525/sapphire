"""Holds the synthetic capture: the committed BVH is what `capture.py` writes, and its feet, read back from
the file's own numbers, slide where it says and nowhere else."""
import unittest

import capture


def _rotation(axis, degrees):
    """The matrix of a turn about one axis, rows."""
    radians = degrees * (capture.PI / 180.0)
    s, c = capture._sin(radians), capture._cos(radians)
    if axis == 'X':
        return ((1, 0, 0), (0, c, -s), (0, s, c))
    if axis == 'Y':
        return ((c, 0, s), (0, 1, 0), (-s, 0, c))
    return ((c, -s, 0), (s, c, 0), (0, 0, 1))


def _mul(a, b):
    return tuple(tuple(sum(a[i][k] * b[k][j] for k in range(3)) for j in range(3)) for i in range(3))


def _apply(m, v):
    return tuple(sum(m[i][k] * v[k] for k in range(3)) for i in range(3))


def _ankles(text):
    """Each frame's left and right ankle, meters, by the file's hierarchy and channel order."""
    lines = text.split('\n')
    frames = [[float(value) for value in line.split()] for line in lines[lines.index('MOTION') + 3:] if line]
    offsets = {name: offset for name, _, offset in capture.JOINTS}
    parents = {name: parent for name, parent, _ in capture.JOINTS}
    order = [name for name, _, _ in capture.JOINTS]
    out = []
    for row in frames:
        model = {}
        at = 3
        for name in order:
            z, x, y = row[at:at + 3]
            at += 3
            local = _mul(_mul(_rotation('Z', z), _rotation('X', x)), _rotation('Y', y))
            if parents[name] is None:
                model[name] = (tuple(v / 100.0 for v in row[:3]), local)
            else:
                place, turned = model[parents[name]]
                moved = _apply(turned, offsets[name])
                model[name] = (tuple(p + m for p, m in zip(place, moved)), _mul(turned, local))
        out.append((model['LeftFoot'][0], model['RightFoot'][0]))
    return out


class CaptureTests(unittest.TestCase):
    def test_the_committed_file_is_what_the_generator_writes(self):
        with open(capture.OUTPUT, encoding='utf-8', newline='') as f:
            committed = f.read()
        self.assertEqual(committed, capture.bvh(), 'run `python3 tools/motion/capture.py --write`')

    def test_every_leg_reaches_and_only_the_injection_slides(self):
        self.assertEqual(capture.check(), [])

    def test_the_files_planted_feet_slide_only_on_the_injected_frames(self):
        ankles = _ankles(capture.bvh())
        self.assertEqual(len(ankles), capture.FRAMES)
        for side, index in (('Left', 0), ('Right', 1)):
            anchor = None
            for frame, both in enumerate(ankles):
                ankle = both[index]
                target, planted = capture.ankle_at(frame, side)
                # The legs land the ankle where it was asked for, to the noise.
                self.assertLess(max(abs(a - b) for a, b in zip(ankle, target)), 0.002, (side, frame))
                if not planted:
                    anchor = None
                    continue
                anchor = anchor or ankle
                moved = abs(ankle[2] - anchor[2])
                if side == 'Left' and capture.INJECTED[0] < frame <= capture.INJECTED[1]:
                    self.assertGreater(moved, 0.0, frame)
                elif side == 'Left' and capture.INJECTED[1] < frame <= 47:
                    self.assertAlmostEqual(moved, capture.SLIDE, delta=0.003)
                else:
                    self.assertLess(moved, 0.003, (side, frame))


if __name__ == '__main__':
    unittest.main()
