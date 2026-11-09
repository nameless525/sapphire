"""Pose the Lancer's clips (M3 C2). Run with --write to update its key-pose documents.

First drafts nobody has judged. Its locomotion and guard are KayKit's, retargeted by the pack's
sidecars; these are its attacks, its hop back and its reactions. Each attack's wind-up, active and
recovery are the move's frame data to come (part 2's `lancer.moves.ron`), so a clip's length is the
move's: thrust 16 + 5 + 20, sweep 22 + 8 + 26, vault 30 + 6 + 34 ticks.

The spear is skinned to `handslot.r`: `grip` puts that slot, `aim` points the spear from it toward the
tip (1.33 m out), and `hold` puts the left hand on the shaft that far ahead of the right.
"""
import sys
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

from figure_pose import Clip, main  # noqa: E402
from pose import norm  # noqa: E402

RIG_ID = '41e229ad-28a3-41f9-aeb8-ba214e06be6a'

# The stance every attack starts and ends in: the left foot forward, the body turned to the right
# behind the spear, its point low toward the foe, the left hand forward on the shaft.
STANCE = dict(
    hips=(0.0, -0.06, 0.0),
    pelvis=(0.0, -22.0, 0.0), spine=(4.0, -4.0, 0.0), chest=(6.0, -8.0, 0.0), head=(-6.0, 32.0, 0.0),
    left_foot=(0.1, 0.17, 0.0, -10.0, 0.0), right_foot=(-0.15, -0.15, 0.0, -45.0, 0.0),
    knees=(0.1, 0.15),
    grip=(-0.08, 0.95, 0.06), aim=norm((0.02, -0.14, 1.0)), roll=(0.0, 1.0, 0.0),
    elbow=(-0.6, -0.7, -0.3), hold=0.22, elbow_l=(0.4, -1.0, 0.1),
)


def at(tick, *bases, **fields):
    key = {}
    for base in bases:
        key.update(base)
    key.update(fields)
    key['tick'] = tick
    return key


def thrust():
    return Clip('thrust', 'A quick thrust: the spear drawn back, a step in on the left foot and a straight stab.', 41, [
        at(0, STANCE),
        at(12, ease='Out', hips=(0.0, -0.08, -0.04), pelvis=(0.0, -32.0, 0.0), chest=(6.0, -18.0, 0.0),
           head=(-6.0, 44.0, 0.0), grip=(-0.16, 0.98, -0.18), aim=norm((0.04, -0.06, 1.0))),
        at(16, grip=(-0.17, 0.99, -0.22)),
        at(19, ease='In', dense=4, root=(0.0, 0.22), left_foot=(0.1, 0.55, 0.0, -10.0, 0.0),
           right_foot=(-0.15, -0.04, 0.0, -45.0, 0.0), hips=(0.0, -0.13, 0.1), pelvis=(0.0, -8.0, 0.0), spine=(8.0, 0.0, 0.0), chest=(8.0, 2.0, 0.0),
           head=(-8.0, 10.0, 0.0), grip=(-0.05, 1.0, 0.32), aim=norm((0.0, -0.04, 1.0)), hold=0.1),
        at(21, grip=(-0.04, 1.0, 0.35)),
        at(30, root=(0.0, 0.3), right_foot=(-0.15, 0.1, 0.0, -45.0, 0.0), hips=(0.0, -0.08, 0.04),
           pelvis=(0.0, -18.0, 0.0), spine=(4.0, -4.0, 0.0), chest=(6.0, -6.0, 0.0), head=(-6.0, 28.0, 0.0),
           grip=(-0.1, 0.94, 0.12), aim=norm((0.02, -0.12, 1.0)), hold=0.28),
        at(41, STANCE, root=(0.0, 0.3), left_foot=(0.1, 0.47, 0.0, -10.0, 0.0),
           right_foot=(-0.15, 0.15, 0.0, -45.0, 0.0)),
    ])


def sweep():
    return Clip('sweep', 'A low sweep: wound to the right, the spear swung flat round the front at the knees, and back.', 56, [
        at(0, STANCE),
        at(14, ease='Out', hips=(0.0, -0.12, 0.0), pelvis=(0.0, -42.0, 0.0), spine=(6.0, -10.0, 0.0),
           chest=(6.0, -20.0, 0.0), head=(-6.0, 60.0, 0.0), left_foot=(0.18, 0.16, 0.0, 0.0, 0.0),
           right_foot=(-0.22, -0.12, 0.0, -50.0, 0.0), grip=(-0.28, 0.88, -0.1), aim=norm((-0.9, -0.18, -0.4)),
           elbow=(-0.3, -1.0, -0.3), hold=None, reach=(0.36, 1.0, 0.16), elbow_l=(0.6, -0.8, 0.0)),
        at(22, grip=(-0.3, 0.87, -0.12), aim=norm((-0.88, -0.22, -0.48))),
        at(24, ease='In', dense=8, pelvis=(0.0, -24.0, 0.0), chest=(6.0, -10.0, 0.0), head=(-6.0, 34.0, 0.0),
           grip=(-0.24, 0.84, 0.12), aim=norm((-0.7, -0.26, 0.66))),
        at(26, pelvis=(0.0, -4.0, 0.0), chest=(6.0, 4.0, 0.0), head=(-6.0, 4.0, 0.0),
           grip=(-0.08, 0.82, 0.24), aim=norm((-0.05, -0.28, 1.0))),
        at(28, pelvis=(0.0, 16.0, 0.0), chest=(6.0, 16.0, 0.0), head=(-6.0, -24.0, 0.0),
           grip=(0.06, 0.83, 0.2), aim=norm((0.62, -0.26, 0.74))),
        at(30, ease='Out', pelvis=(0.0, 28.0, 0.0), chest=(6.0, 24.0, 0.0), head=(-6.0, -40.0, 0.0),
           grip=(0.12, 0.85, 0.12), aim=norm((0.96, -0.22, 0.2)), reach=(0.4, 0.95, -0.1)),
        at(40, pelvis=(0.0, 10.0, 0.0), chest=(6.0, 8.0, 0.0), head=(-6.0, -10.0, 0.0),
           grip=(0.0, 0.88, 0.14), aim=norm((0.5, -0.2, 0.84)), reach=(0.06, 0.92, 0.3)),
        at(48, STANCE),
        at(56, STANCE),
    ])


def vault():
    leap = 3.4
    return Clip('vault', 'A leaping stab: a crouch with the spear drawn high, a long jump in, and the point driven down on landing.', 70, [
        at(0, STANCE),
        at(18, ease='Out', hips=(0.0, -0.17, -0.06), pelvis=(10.0, -20.0, 0.0), spine=(10.0, -6.0, 0.0),
           chest=(8.0, -8.0, 0.0), head=(-14.0, 30.0, 0.0), grip=(-0.16, 1.12, -0.16), aim=norm((0.0, 0.25, 1.0)),
           elbow=(-0.6, -0.4, -0.6), hold=0.3),
        at(24, ease='In', hips=(0.0, -0.2, -0.04)),
        at(27, root=(0.0, 0.5), hips=(0.0, 0.02, 0.06), right_foot=(-0.15, 0.2, 0.12, -30.0, 30.0),
           left_foot=(0.1, 0.5, 0.06, -10.0, 20.0), grip=(-0.12, 1.24, 0.0), aim=norm((0.0, 0.1, 1.0))),
        at(30, dense=6, root=(0.0, 1.7), hips=(0.0, 0.06, 0.0), left_foot=(0.1, 1.86, 0.32, -10.0, 0.0),
           right_foot=(-0.15, 1.5, 0.3, -30.0, 0.0), pelvis=(-6.0, -12.0, 0.0), chest=(-4.0, -6.0, 0.0),
           head=(-4.0, 20.0, 0.0), grip=(-0.08, 1.36, 0.1), aim=norm((0.0, -0.4, 1.0))),
        at(34, ease='In', root=(0.0, leap), hips=(0.0, -0.18, 0.14), pelvis=(14.0, -14.0, 0.0),
           spine=(12.0, 0.0, 0.0), chest=(10.0, 0.0, 0.0), head=(-18.0, 16.0, 0.0),
           left_foot=(0.12, leap + 0.24, 0.0, -10.0, 0.0), right_foot=(-0.16, leap - 0.2, 0.0, -40.0, 0.0),
           grip=(-0.06, 0.8, 0.36), aim=norm((0.0, -0.8, 0.6)), hold=0.1),
        at(36, grip=(-0.06, 0.77, 0.38)),
        at(52, hips=(0.0, -0.1, 0.04), pelvis=(4.0, -20.0, 0.0), spine=(6.0, -4.0, 0.0), chest=(6.0, -6.0, 0.0),
           head=(-8.0, 28.0, 0.0), grip=(-0.1, 0.9, 0.12), aim=norm((0.02, -0.25, 1.0)), hold=0.28),
        at(70, STANCE, root=(0.0, leap), left_foot=(0.12, leap + 0.24, 0.0, -10.0, 0.0),
           right_foot=(-0.16, leap - 0.12, 0.0, -45.0, 0.0)),
    ])


def backstep():
    back = -2.0
    return Clip('backstep', 'A hop back out of reach, the spear kept between it and the foe.', 32, [
        at(0, STANCE),
        at(6, ease='Out', hips=(0.0, -0.13, -0.02)),
        at(10, ease='In', root=(0.0, -0.25), hips=(0.0, -0.02, -0.06), left_foot=(0.1, 0.05, 0.08, -10.0, -20.0),
           right_foot=(-0.15, -0.4, 0.1, -45.0, 0.0)),
        at(16, root=(0.0, -1.3), hips=(0.0, 0.04, -0.04), left_foot=(0.1, -1.05, 0.2, -10.0, 0.0),
           right_foot=(-0.15, -1.55, 0.2, -45.0, 0.0), grip=(-0.12, 1.0, 0.1), aim=norm((0.02, -0.05, 1.0))),
        at(21, ease='In', root=(0.0, back), hips=(0.0, -0.14, -0.02), left_foot=(0.1, back + 0.17, 0.0, -10.0, 0.0),
           right_foot=(-0.15, back - 0.15, 0.0, -45.0, 0.0)),
        at(32, STANCE, root=(0.0, back), left_foot=(0.1, back + 0.17, 0.0, -10.0, 0.0),
           right_foot=(-0.15, back - 0.15, 0.0, -45.0, 0.0)),
    ])


def stagger_light():
    return Clip('stagger_light', 'A flinch: the head snaps back and the spear jerks up, then the stance again.', 24, [
        at(0, STANCE),
        at(5, ease='Out', dense=3, hips=(0.0, -0.05, -0.06), spine=(-6.0, 0.0, 0.0), chest=(-12.0, 4.0, 4.0),
           head=(-20.0, 26.0, 0.0), grip=(-0.12, 1.0, 0.0), aim=norm((0.0, 0.3, 1.0)), hold=0.16),
        at(24, STANCE),
    ])


def stagger_heavy():
    return Clip('stagger_heavy', 'Knocked reeling: two stumbling steps back, the spear thrown wide, then a crouch to recover.', 44, [
        at(0, STANCE),
        at(6, ease='Out', dense=4, root=(0.0, -0.12), hips=(0.0, -0.04, -0.1), pelvis=(-6.0, -14.0, 0.0),
           left_foot=(0.1, 0.06, 0.04, -10.0, 0.0),
           spine=(-12.0, 0.0, 4.0), chest=(-18.0, 8.0, 6.0), head=(-24.0, 20.0, 0.0),
           grip=(-0.28, 1.06, -0.1), aim=norm((-0.5, 0.5, 0.7)), hold=None, reach=(0.42, 1.0, 0.12)),
        at(16, root=(0.0, -0.3), right_foot=(-0.16, -0.5, 0.0, -45.0, 0.0), hips=(0.0, -0.08, -0.08),
           left_foot=(0.1, 0.0, 0.06, -10.0, 0.0)),
        at(24, root=(0.0, -0.4), left_foot=(0.1, -0.12, 0.0, -10.0, 0.0), hips=(0.0, -0.14, -0.02),
           pelvis=(6.0, -20.0, 0.0), spine=(8.0, -4.0, 0.0), chest=(6.0, -6.0, 0.0), head=(-10.0, 30.0, 0.0),
           grip=(-0.14, 0.92, 0.04), aim=norm((0.0, -0.1, 1.0)), hold=0.3, reach=None),
        at(44, STANCE, root=(0.0, -0.4), left_foot=(0.1, -0.12, 0.0, -10.0, 0.0),
           right_foot=(-0.16, -0.5, 0.0, -45.0, 0.0)),
    ])


# Lying on its back where a fall leaves it: the hips at the ground, legs bent, the spear by its side.
LYING = dict(
    hips=(0.0, -0.66, -0.55), pelvis=(-82.0, 0.0, 0.0), spine=(-4.0, 0.0, 0.0), chest=(-4.0, 0.0, 0.0),
    head=(18.0, 10.0, 0.0), knees=(0.0, 0.0),
    grip=(-0.36, 0.09, -0.62), aim=norm((0.1, 0.0, 1.0)), roll=(0.0, 1.0, 0.0), elbow=(-1.0, 0.0, 0.0),
    hold=None, reach=(0.36, 0.09, -0.62), elbow_l=(1.0, 0.0, 0.0),
)
# Where a lying figure's feet are, from its root: its legs out straight from the hips, toes up.
LYING_FEET = 0.12


def lying_feet(root_z):
    return dict(left_foot=(0.12, root_z + LYING_FEET, 0.0, 6.0, -70.0),
                right_foot=(-0.12, root_z + LYING_FEET, 0.0, -6.0, -70.0))


def knockdown():
    return Clip('knockdown', 'Thrown down on its back, and lying there.', 60, [
        at(0, STANCE),
        at(8, ease='Out', root=(0.0, -0.2), hips=(0.0, -0.02, -0.12), pelvis=(-16.0, -10.0, 0.0),
           spine=(-14.0, 0.0, 0.0), chest=(-18.0, 4.0, 0.0), head=(-20.0, 16.0, 0.0),
           left_foot=(0.1, 0.05, 0.1, -10.0, 0.0), grip=(-0.3, 1.1, -0.1), aim=norm((-0.3, 0.6, 0.7)),
           hold=None, reach=(0.42, 1.1, 0.0)),
        at(18, ease='In', root=(0.0, -0.6), hips=(0.0, -0.42, -0.4), pelvis=(-55.0, 0.0, 0.0),
           left_foot=(0.12, -0.3, 0.16, 0.0, 30.0), right_foot=(-0.14, -0.5, 0.06, -20.0, 20.0),
           grip=(-0.38, 0.72, -0.56), reach=(0.38, 0.76, -0.56)),
        at(24, LYING, lying_feet(-0.8), ease='Out', root=(0.0, -0.8)),
        at(60, ease='Smooth', head=(14.0, 10.0, 0.0)),
    ])


def recover():
    start = dict(LYING, root=(0.0, 0.0), **lying_feet(0.0))
    return Clip('recover', 'Up from its back: rolls to a knee, plants the spear, and stands into the stance.', 50, [
        at(0, start, head=(14.0, 10.0, 0.0)),
        at(14, hips=(0.0, -0.5, -0.12), pelvis=(-30.0, -20.0, 0.0), spine=(14.0, 0.0, 0.0), chest=(12.0, 0.0, 0.0),
           head=(-4.0, 20.0, 0.0), left_foot=(0.14, 0.32, 0.0, -10.0, 0.0), right_foot=(-0.16, -0.1, 0.09, -45.0, 60.0),
           grip=(-0.22, 0.5, 0.16), aim=norm((0.0, 0.7, 0.7)), roll=(0.0, 0.0, -1.0), elbow=(-0.6, -0.6, -0.4),
           hold=None, reach=(0.26, 0.52, 0.16)),
        at(28, hips=(0.0, -0.3, 0.0), pelvis=(10.0, -20.0, 0.0), spine=(10.0, -4.0, 0.0), chest=(8.0, -6.0, 0.0),
           head=(-10.0, 30.0, 0.0), right_foot=(-0.15, -0.15, 0.0, -45.0, 0.0),
           grip=(-0.14, 0.82, 0.08), aim=norm((0.02, -0.05, 1.0)), roll=(0.0, 1.0, 0.0), hold=0.3, reach=None),
        at(50, STANCE, left_foot=(0.14, 0.32, 0.0, -10.0, 0.0)),
    ])


def death():
    return Clip('death', 'Struck down: buckles to its knees, the spear falling from its hand, and topples onto its back.', 80, [
        at(0, STANCE),
        at(10, ease='Out', hips=(0.0, -0.06, -0.08), spine=(-10.0, 0.0, 0.0), chest=(-16.0, 6.0, 0.0),
           head=(-24.0, 20.0, 0.0), grip=(-0.22, 1.02, -0.04), aim=norm((-0.2, 0.4, 0.9)),
           hold=None, reach=(0.3, 1.0, 0.22)),
        at(28, ease='In', hips=(0.0, -0.4, -0.04), pelvis=(10.0, -10.0, 0.0), spine=(14.0, 0.0, 0.0),
           chest=(16.0, 0.0, 0.0), head=(30.0, 10.0, 0.0), left_foot=(0.1, 0.17, 0.0, -10.0, 0.0),
           right_foot=(-0.15, -0.3, 0.1, -45.0, 70.0),
           grip=(-0.34, 0.45, 0.0), aim=norm((-0.3, 0.2, 1.0)), elbow=(-0.3, -1.0, 0.0), hold=None,
           reach=(0.36, 0.5, 0.05)),
        at(40, hips=(0.0, -0.44, -0.08)),
        at(56, LYING, lying_feet(-0.1), ease='In', root=(0.0, -0.1)),
        at(80, ease='Out', head=(10.0, 20.0, 0.0)),
    ])


CLIPS = [thrust(), sweep(), vault(), backstep(), stagger_light(), stagger_heavy(), knockdown(), recover(), death()]

if __name__ == '__main__':
    # The spear's hit path runs along it from its head's middle, 1.25 m out from the fist.
    main('lancer.py', 'lancer', RIG_ID, CLIPS, __doc__.splitlines()[0], weapon=('spear', 1.25))
