"""Pose a zone's ambient cast's own clips (M5 C10). Run with --write to update their key-pose documents.

First drafts nobody has judged. The cast (`tools/figures/ambient_*.figure.ron`) stands on the Lancer's
joints exactly and plays on its skeleton, so these are posed on the Lancer's rest and its walk is the
Lancer's `Walking_B`, retargeted by the pack's sidecars. `content/ambient/ambient.animgraph.ron` blends
`stand` with that walk and plays `shift` and `look_around` as gestures, each from `stand`'s first pose
and back to it, so a gesture starts and ends where standing is.

Unarmed: the right hand's slot is put where a hanging hand's fingers are (`grip`, its fist's axis still
forward), and the left hand's wrist by `reach`.
"""
import sys
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

from figure_pose import Clip, main  # noqa: E402

RIG_ID = '41e229ad-28a3-41f9-aeb8-ba214e06be6a'
FOLDER = 'content/ambient'
MODEL = 'content/combat/lancer/lancer.glb'

# Standing at ease: feet a little apart and turned out, knees soft, arms hanging with the elbows a
# little bent and back.
STAND = dict(
    hips=(0.0, -0.025, 0.0),
    pelvis=(0.0, 0.0, 0.0), spine=(2.0, 0.0, 0.0), chest=(2.0, 0.0, 0.0), head=(-2.0, 0.0, 0.0),
    left_foot=(0.11, 0.0, 0.0, 8.0, 0.0), right_foot=(-0.11, -0.02, 0.0, -8.0, 0.0),
    knees=(0.05, 0.05),
    grip=(-0.2, 0.67, 0.04), aim=(0.0, 0.0, 1.0), roll=(-1.0, 0.0, 0.0), elbow=(-0.4, -0.2, -1.0),
    reach=(0.2, 0.76, 0.03), elbow_l=(0.4, -0.2, -1.0),
)


def at(tick, *bases, **fields):
    key = {}
    for base in bases:
        key.update(base)
    key.update(fields)
    key['tick'] = tick
    return key


def stand():
    return Clip('stand', 'Standing at ease, breathing: the chest rises and the weight drifts a little, a loop.', 120, [
        at(0, STAND),
        at(60, hips=(0.008, -0.022, 0.0), pelvis=(0.0, 0.0, -1.0), spine=(1.0, 0.0, 0.0), chest=(3.5, 0.0, 0.0),
           head=(-3.0, 2.0, 0.0), grip=(-0.2, 0.675, 0.04), reach=(0.2, 0.765, 0.03)),
        at(120, STAND),
    ], step=4)


def shift():
    return Clip('shift', 'The weight moved onto the left leg, the right knee eased, a breath held there and back.', 90, [
        at(0, STAND),
        at(30, hips=(0.035, -0.035, 0.0), pelvis=(0.0, 4.0, -4.0), spine=(2.0, -2.0, 2.0), chest=(2.0, -2.0, 2.0),
           head=(-2.0, 4.0, -1.0), knees=(0.05, 0.2), grip=(-0.17, 0.66, 0.05), reach=(0.24, 0.75, 0.03)),
        at(60, hips=(0.03, -0.035, 0.0), head=(-3.0, 6.0, -1.0)),
        at(90, STAND),
    ], step=3)


def look_around():
    return Clip('look_around', 'A look to the left down the street, then to the right, and ahead again.', 120, [
        at(0, STAND),
        at(28, pelvis=(0.0, 6.0, 0.0), spine=(2.0, 4.0, 0.0), chest=(2.0, 8.0, 0.0), head=(-4.0, 30.0, 0.0)),
        at(48, head=(-6.0, 34.0, 0.0)),
        at(78, pelvis=(0.0, -5.0, 0.0), spine=(2.0, -4.0, 0.0), chest=(2.0, -7.0, 0.0), head=(-2.0, -28.0, 0.0)),
        at(96, head=(-3.0, -30.0, 0.0)),
        at(120, STAND),
    ], step=4)


CLIPS = [stand(), shift(), look_around()]

if __name__ == '__main__':
    main('ambient.py', 'ambient', RIG_ID, CLIPS, __doc__.splitlines()[0], folder=FOLDER, model=MODEL)
