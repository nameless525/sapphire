"""The player's moves: poses by target, written out as key-pose documents. See the README.

usage: moves.py [--write] [--write-numbers [--force]] [-v] [<move> ...]

With no flag it writes nothing, and prints what `Move.check` finds in each move, or in those named.
`--write` writes the key-pose documents. `--write-numbers` writes `player.moves.ron` from `frames.py`, and
refuses to replace a file changed since it last did, unless `--force` says to.
"""
import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import frames
from pose import *
from rig import REPO

FOLDER = os.path.join(REPO, 'content/combat/player')
# The digest of what `--write-numbers` last wrote, kept out of `content/`, which is cooked.
STAMP = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'player.moves.written')

# The combat idle (`content/lookdev/character/idle_combat.keypose.ron`) at tick 0: where every move starts
# and most settle. Its arm holds the blade out in front, at `rh=(-24, -46, 0.35)`, `blade=(-26, 30)`.
IDLE_HIPS = (0.012, -0.056, -0.012)
IDLE_BODY = dict(Hips=(4, -20, 0), Spine=(2, 6, 0), Chest=(2, 6, 0), UpperChest=(0, 3, 0))
IDLE_RAW = dict(
    Neck=(0, 0, 0), Head=(-1, 6, 1),
    LeftUpperLeg=(-32, 12, 6), LeftLowerLeg=(44, 0, 0), LeftFoot=(-16, 10, -3), LeftToes=(0, 0, 0),
    RightUpperLeg=(-14, -8, -7), RightLowerLeg=(38, 0, 0), RightFoot=(-24, 22, 3), RightToes=(0, 0, 0),
    LeftShoulder=(0, 0, 0), LeftUpperArm=(0, -34, -62), LeftLowerArm=(0, -62, 0), LeftHand=(8, 0, -6),
    RightShoulder=(0, 0, 0), RightUpperArm=(25, 17.4, 46.5), RightLowerArm=(0, 72.6, 0), RightHand=(-81.3, -0.2, 1.9),
)
IDLE_L = (0.17, 0.17, 2)
IDLE_R = (-0.08, -0.11, -9)


def idle_feet(root_z=0.0, root_x=0.0):
    return dict(L=Foot(IDLE_L[0] + root_x, IDLE_L[1] + root_z, yaw=IDLE_L[2]),
                R=Foot(IDLE_R[0] + root_x, IDLE_R[1] + root_z, yaw=IDLE_R[2]))


# The right hand closed round a grip, the left relaxed: set once, in a document's first key.
FIST = dict(
    RightThumbProximal=(0, 26, 14), RightThumbDistal=(0, 30, 0),
    RightIndexProximal=(0, 0, 58), RightIndexIntermediate=(0, 0, 66), RightIndexDistal=(0, 0, 36),
    RightMiddleProximal=(0, 0, 64), RightMiddleIntermediate=(0, 0, 70), RightMiddleDistal=(0, 0, 38),
    RightRingProximal=(0, 0, 68), RightRingIntermediate=(0, 0, 72), RightRingDistal=(0, 0, 38),
    RightLittleProximal=(0, 0, 72), RightLittleIntermediate=(0, 0, 74), RightLittleDistal=(0, 0, 38),
)
LEFT_OPEN = dict(
    LeftThumbProximal=(0, -12, -6), LeftThumbDistal=(0, -14, 0),
    LeftIndexProximal=(0, 0, -30), LeftIndexIntermediate=(0, 0, -36), LeftIndexDistal=(0, 0, -18),
    LeftMiddleProximal=(0, 0, -36), LeftMiddleIntermediate=(0, 0, -40), LeftMiddleDistal=(0, 0, -20),
    LeftRingProximal=(0, 0, -40), LeftRingIntermediate=(0, 0, -44), LeftRingDistal=(0, 0, -22),
    LeftLittleProximal=(0, 0, -44), LeftLittleIntermediate=(0, 0, -46), LeftLittleDistal=(0, 0, -22),
)
LEFT_FIST = {k: tuple(-v if isinstance(v, (int, float)) else v for v in values)
             for k, values in {b.replace('Right', 'Left'): vs for b, vs in FIST.items()}.items()}
LEFT_FLAT = dict(
    LeftThumbProximal=(0, -6, -2), LeftThumbDistal=(0, -6, 0),
    LeftIndexProximal=(0, 0, -4), LeftIndexIntermediate=(0, 0, -6), LeftIndexDistal=(0, 0, -4),
    LeftMiddleProximal=(0, 0, -6), LeftMiddleIntermediate=(0, 0, -8), LeftMiddleDistal=(0, 0, -4),
    LeftRingProximal=(0, 0, -8), LeftRingIntermediate=(0, 0, -10), LeftRingDistal=(0, 0, -6),
    LeftLittleProximal=(0, 0, -10), LeftLittleIntermediate=(0, 0, -12), LeftLittleDistal=(0, 0, -6),
)
HANDS = {**FIST, **LEFT_OPEN}

HEADER = """Turns are degrees in the character's rest space, about each bone's own joint, X then Y then Z.
The character faces +Z, with its left arm along +X at rest. Ticks are 60 a second. `root` is where
the move has carried the character, `hips` moves the hips from rest. The right hand holds the sword,
leaving the fist on the thumb side. Frame data is in player.moves.ron. Written by tools/motion/moves.py,
so edit a move there."""


def start(move, say):
    return move.key(0, say, hips=IDLE_HIPS, body=IDLE_BODY, raw=IDLE_RAW, fingers=HANDS, **idle_feet())


def finish(move, tick, say, root_z, root_x=0.0, ease='Smooth', fingers=None):
    return move.key(tick, say, ease=ease, root=(root_x, 0, root_z), hips=IDLE_HIPS, body=IDLE_BODY, raw=IDLE_RAW,
                    fingers=fingers, **idle_feet(root_z, root_x))


# What the end of one hit leaves the body doing: the next hit starts from it.
AFTER_1 = dict(
    hips=(-0.015, -0.14, 0.02),
    body=dict(Hips=(7, 26, -2), Spine=(5, 10, -3), Chest=(10, 10, -2), UpperChest=(6, 4, 0)),
    look=(6, -3),
    rh=(46, -24, 0.38), elbow_r=(-0.3, -0.8, -0.5), blade=(92, -10),
    lh=(110, -50, 0.38), elbow_l=(0.7, -0.4, -0.6),
)
AFTER_1_FEET = dict(L=(0.17, -0.40, 12, 30), R=(-0.13, 0.08, -16, 0))

AFTER_2 = dict(
    hips=(0.015, -0.135, -0.01),
    body=dict(Hips=(8, -24, 2), Spine=(4, -12, 3), Chest=(2, -10, 2), UpperChest=(0, -6, 0)),
    look=(-6, 2),
    rh=(-62, 40, 0.38), elbow_r=(-0.7, -0.6, -0.4), blade=(-88, 70),
    lh=(55, -26, 0.34), elbow_l=(0.8, -0.5, -0.2),
)
AFTER_2_FEET = dict(L=(0.19, 0.02, 14, 0), R=(-0.13, -0.46, -14, 26))


def light_1():
    m = Move('light_1', 34, """The first cut of the string (M2 O3): a forehand slash from high on the right down across to the
low left, on a step of the right foot. Hit on ticks 7 to 10. It leaves the blade low on the left and
the right foot forward, where `light_2` starts; left alone, the back foot steps through to ready.

""" + HEADER)
    start(m, "Ready, as the combat idle holds the character.")
    m.key(3, "It moves at once: the weight sinks onto the left foot and the shoulders start round to the right.",
          ease='Out', hips=(0.03, -0.10, -0.02),
          body=dict(Hips=(5, -30, 0), Spine=(4, -8, 3), Chest=(4, -8, 2), UpperChest=(2, -4, 0)),
          R=Foot(-0.08, -0.11, yaw=-12, pitch=18, planted=False),
          look=(-12, -2),
          rh=(-58, 4, 0.35), elbow_r=(-0.8, -0.5, -0.3), blade=(-92, 44),
          lh=(35, -28, 0.32), elbow_l=(0.8, -0.6, -0.2))
    m.key(7, "The top of the wind-up: coiled to the right, low, blade up and back over the right shoulder.",
          root=(0, 0, 0.06), hips=(0.045, -0.125, -0.03),
          body=dict(Hips=(4, -42, 2), Spine=(2, -16, 4), Chest=(0, -16, 3), UpperChest=(-2, -8, 0)),
          R=Foot(-0.09, 0.02, yaw=-14, pitch=10, lift=0.04),
          look=(-22, -2),
          rh=(-122, 34, 0.40), elbow_r=(-0.9, -0.4, -0.3), blade=(-150, 56),
          lh=(28, -14, 0.40), elbow_l=(0.8, -0.6, -0.1))
    m.key(8, "The cut starts in the hips: they turn and drive forward while the arm stays back.",
          ease='In', root=(0, 0, 0.14), hips=(0.03, -0.12, -0.01),
          body=dict(Hips=(7, -24, 1), Spine=(3, -8, 2), Chest=(2, -14, 2), UpperChest=(0, -8, 0)),
          L=Foot(0.17, 0.17, yaw=4, pitch=8),
          R=Foot(-0.1, 0.26, yaw=-14, pitch=-4, lift=0.07),
          look=(-16, -2),
          rh=(-116, 30, 0.41), elbow_r=(-0.9, -0.4, -0.3), blade=(-146, 50),
          lh=(40, -24, 0.38), elbow_l=(0.8, -0.6, -0.2))
    m.key(9, "Then the chest, and the arm comes after it, the wrist still behind.",
          ease='Linear', root=(0, 0, 0.3), hips=(0.02, -0.12, 0.0),
          body=dict(Hips=(6, -6, 0), Spine=(5, 0, -1), Chest=(7, -4, -2), UpperChest=(4, -3, 0)),
          L=Foot(0.17, 0.17, yaw=6, pitch=16),
          R=Foot(-0.11, 0.52, yaw=-14, pitch=-14, lift=0.09),
          look=(-8, -4),
          rh=(-62, 14, 0.42), elbow_r=(-0.8, -0.5, -0.4), blade=(-78, 30),
          lh=(60, -38, 0.38), elbow_l=(0.8, -0.5, -0.4))
    m.key(10, "Through the front, the blade at its fastest.",
          ease='Linear', root=(0, 0, 0.44), hips=(0.005, -0.13, 0.01),
          body=dict(Hips=(7, 8, -1), Spine=(5, 6, -2), Chest=(9, 4, -2), UpperChest=(5, 2, 0)),
          L=Foot(0.17, 0.17, yaw=8, pitch=25),
          R=Foot(-0.12, 0.68, yaw=-15, pitch=-8, lift=0.04),
          look=(-2, -5),
          rh=(-18, 0, 0.43), elbow_r=(-0.6, -0.7, -0.4), blade=(-14, 6),
          lh=(82, -46, 0.40), elbow_l=(0.8, -0.4, -0.5))
    m.key(11, "The end of the cut: the right foot lands, the blade across the front and falling to the left.",
          ease='Linear', root=(0, 0, 0.56), hips=(-0.01, -0.14, 0.02),
          body=dict(Hips=(8, 18, -2), Spine=(6, 12, -4), Chest=(12, 12, -3), UpperChest=(7, 6, 0)),
          L=Foot(0.17, 0.17, yaw=10, pitch=34),
          R=Foot(-0.13, 0.78, yaw=-16),
          look=(4, -6),
          rh=(26, -14, 0.43), elbow_r=(-0.3, -0.8, -0.5), blade=(66, -16),
          lh=(100, -52, 0.40), elbow_l=(0.8, -0.3, -0.5))
    m.key(15, "Past the mark: the hips sink and the chest keeps turning, the blade out behind on the left.",
          ease=('Overshoot', 0.12), root=(0, 0, 0.66), hips=(-0.02, -0.17, 0.03),
          body=dict(Hips=(9, 30, -3), Spine=(7, 16, -5), Chest=(15, 14, -4), UpperChest=(9, 8, 0)),
          L=Foot(0.17, 0.26, yaw=12, pitch=42),
          look=(8, -6),
          rh=(56, -26, 0.42), elbow_r=(-0.2, -0.8, -0.6), blade=(104, -26),
          lh=(125, -48, 0.42), elbow_l=(0.7, -0.3, -0.6))
    m.key(22, "Settled and open: blade low on the left, right foot forward, ready to be cancelled.",
          ease='Out', root=(0, 0, 0.7),
          L=Foot(0.17, 0.30, yaw=12, pitch=30), **AFTER_1)
    m.key(26, "Held a moment longer.", ease='Linear',
          body=dict(Hips=(7, 24, -2), Spine=(5, 10, -3), Chest=(10, 8, -2), UpperChest=(5, 4, 0)))
    m.key(30, "Left alone, the back foot comes through, the blade coming back to the middle.",
          root=(0, 0, 0.78), hips=(0.01, -0.09, 0.0),
          body=dict(Hips=(6, 0, -1), Spine=(4, 6, -1), Chest=(4, 6, -1), UpperChest=(2, 2, 0)),
          L=Foot(0.17, 0.74, yaw=6, pitch=10, lift=0.05),
          look=(4, -2),
          rh=(-20, -36, 0.36), elbow_r=(-0.5, -0.8, -0.4), blade=(10, 30),
          lh=(40, -40, 0.32), elbow_l=(0.7, -0.6, -0.3))
    finish(m, 34, "Ready again.", 0.89, ease='Out')
    return m


def light_2():
    m = Move('light_2', 36, """The second cut (M2 O3): a backhand rising from low on the left to high on the right, on a step of
the left foot. Hit on ticks 7 to 10. It starts where `light_1` settles and leaves the blade high on
the right, with the left foot forward, where `light_3` starts.

""" + HEADER)
    lf, rf = AFTER_1_FEET['L'], AFTER_1_FEET['R']
    m.key(0, "Where the first cut settles.", fingers=HANDS,
          L=Foot(lf[0], lf[1], yaw=lf[2], pitch=lf[3]), R=Foot(rf[0], rf[1], yaw=rf[2], pitch=rf[3]), **AFTER_1)
    m.key(3, "At once: the weight drops onto the right foot, the blade pulling further back and low.",
          ease='Out', hips=(-0.03, -0.165, 0.0),
          body=dict(Hips=(8, 34, -3), Spine=(7, 12, -4), Chest=(11, 12, -3), UpperChest=(6, 6, 0)),
          L=Foot(lf[0], lf[1], yaw=lf[2], pitch=46, planted=False),
          look=(10, -3),
          rh=(60, -32, 0.40), elbow_r=(-0.3, -0.8, -0.5), blade=(128, -24),
          lh=(118, -44, 0.40), elbow_l=(0.7, -0.3, -0.6))
    m.key(7, "The bottom of the wind-up: coiled to the left, the back foot on its way.",
          root=(0, 0, 0.06), hips=(-0.04, -0.17, 0.02),
          body=dict(Hips=(8, 42, -4), Spine=(7, 14, -4), Chest=(11, 14, -3), UpperChest=(5, 8, 0)),
          L=Foot(0.18, -0.10, yaw=8, pitch=26, lift=0.05),
          look=(14, -2),
          rh=(74, -34, 0.42), elbow_r=(-0.2, -0.8, -0.5), blade=(150, -20),
          lh=(128, -38, 0.42), elbow_l=(0.6, -0.4, -0.7))
    m.key(8, "The hips swing back through first, the arm still low on the left.",
          ease='In', root=(0, 0, 0.13), hips=(-0.03, -0.155, 0.01),
          body=dict(Hips=(7, 24, -3), Spine=(5, 8, -3), Chest=(10, 10, -2), UpperChest=(4, 6, 0)),
          L=Foot(0.18, 0.08, yaw=8, pitch=8, lift=0.08),
          R=Foot(rf[0], rf[1], yaw=rf[2], pitch=6),
          look=(8, -1),
          rh=(62, -36, 0.42), elbow_r=(-0.2, -0.8, -0.5), blade=(128, -22),
          lh=(118, -42, 0.42), elbow_l=(0.6, -0.4, -0.7))
    m.key(9, "The cut, rising: the blade comes under and up across the front.",
          ease='Linear', root=(0, 0, 0.26), hips=(-0.015, -0.14, 0.0),
          body=dict(Hips=(6, 6, -1), Spine=(4, 0, 0), Chest=(6, 2, 0), UpperChest=(3, 2, 0)),
          L=Foot(0.18, 0.28, yaw=6, pitch=-10, lift=0.08),
          R=Foot(rf[0], rf[1], yaw=rf[2], pitch=14),
          look=(4, 0),
          rh=(18, -32, 0.43), elbow_r=(-0.4, -0.8, -0.4), blade=(32, -10),
          lh=(100, -40, 0.40), elbow_l=(0.7, -0.4, -0.6))
    m.key(10, "Through the front, the blade at its fastest.",
          ease='Linear', root=(0, 0, 0.38), hips=(0.0, -0.145, 0.0),
          body=dict(Hips=(6, -8, 1), Spine=(4, -6, 1), Chest=(5, -6, 1), UpperChest=(3, -3, 0)),
          L=Foot(0.19, 0.48, yaw=10, pitch=-6, lift=0.04),
          R=Foot(rf[0], rf[1], yaw=rf[2], pitch=24),
          look=(-1, 1),
          rh=(-24, -6, 0.43), elbow_r=(-0.6, -0.7, -0.3), blade=(-34, 20),
          lh=(84, -40, 0.38), elbow_l=(0.8, -0.4, -0.4))
    m.key(11, "The end of the cut: the left foot lands and the blade climbs past the right shoulder.",
          ease='Linear', root=(0, 0, 0.5), hips=(0.015, -0.155, 0.0),
          body=dict(Hips=(7, -22, 2), Spine=(5, -14, 3), Chest=(6, -12, 3), UpperChest=(3, -6, 0)),
          L=Foot(0.19, 0.62, yaw=14),
          R=Foot(rf[0], rf[1], yaw=rf[2], pitch=32),
          look=(-6, 2),
          rh=(-62, 30, 0.43), elbow_r=(-0.8, -0.5, -0.3), blade=(-82, 56),
          lh=(66, -40, 0.38), elbow_l=(0.8, -0.5, -0.2))
    m.key(15, "Past the mark, the chest still turning and the arm thrown high.",
          ease=('Overshoot', 0.12), root=(0, 0, 0.58), hips=(0.02, -0.18, -0.01),
          body=dict(Hips=(7, -32, 3), Spine=(5, -16, 4), Chest=(4, -14, 3), UpperChest=(2, -8, 0)),
          R=Foot(rf[0], 0.14, yaw=rf[2], pitch=40),
          look=(-8, 4),
          rh=(-78, 44, 0.43), elbow_r=(-0.8, -0.4, -0.4), blade=(-108, 72),
          lh=(50, -32, 0.38), elbow_l=(0.8, -0.5, -0.1))
    m.key(22, "Settled and open: blade high on the right, left foot forward.",
          ease='Out', root=(0, 0, 0.6), R=Foot(rf[0], 0.14, yaw=-14, pitch=26), **AFTER_2)
    m.key(27, "Held a moment longer.", ease='Linear',
          body=dict(Hips=(8, -22, 2), Spine=(4, -12, 3), Chest=(2, -10, 2), UpperChest=(0, -6, 0)))
    m.key(32, "Left alone, the back foot comes up, the body draws back over it and the blade comes down.",
          root=(0, 0, 0.5), hips=(0.01, -0.09, -0.01),
          R=Foot(-0.09, 0.32, yaw=-10, pitch=10, lift=0.03),
          body=dict(Hips=(6, -22, 1), Spine=(3, 0, 1), Chest=(3, 2, 1), UpperChest=(1, 2, 0)),
          look=(2, 0),
          rh=(-30, -30, 0.36), elbow_r=(-0.5, -0.8, -0.4), blade=(5, 40),
          lh=(30, -42, 0.32), elbow_l=(0.7, -0.6, -0.3))
    finish(m, 36, "Ready again.", 0.45, ease='Out')
    return m


def light_3():
    m = Move('light_3', 52, """The finisher (M2 O3): a two-handed chop on a long bound of the left foot, the heavy end of the
string. Hit on ticks 16 to 19. It starts where `light_2` settles, gathers low, rises and arches with
the blade over its head, and lands with the tip by the ground, holding there a beat.

""" + HEADER)
    lf, rf = AFTER_2_FEET['L'], AFTER_2_FEET['R']
    m.key(0, "Where the second cut settles.", fingers=HANDS,
          L=Foot(lf[0], lf[1], yaw=lf[2], pitch=lf[3]), R=Foot(rf[0], rf[1], yaw=rf[2], pitch=rf[3]), **AFTER_2)
    m.key(4, "Gathering: everything drops, the left hand comes over to the grip.",
          ease='Out', root=(0, 0, -0.02), hips=(0.0, -0.21, -0.03),
          body=dict(Hips=(8, -24, 2), Spine=(8, -6, 2), Chest=(12, -6, 1), UpperChest=(8, -2, 0)),
          look=(-2, 6),
          rh=(-40, 26, 0.38), elbow_r=(-0.8, -0.5, -0.3), blade=(-112, 58),
          lh=(-8, 4, 0.36), elbow_l=(0.7, -0.6, -0.2))
    m.key(9, "Rising: the hands go overhead, the back arches.",
          root=(0, 0, -0.03), hips=(0.0, -0.08, -0.05),
          body=dict(Hips=(2, -22, 1), Spine=(-6, -4, 1), Chest=(-8, -4, 1), UpperChest=(-8, -2, 0)),
          L=Foot(lf[0], lf[1], yaw=lf[2], pitch=30, planted=False),
          R=Foot(rf[0], rf[1], yaw=rf[2], pitch=40),
          look=(-2, 14), fingers=LEFT_FIST,
          rh=(-14, 80, 0.40), elbow_r=(-0.8, 0.2, -0.5), blade=(180, 32), two_hands=True, elbow_l=(0.8, 0.2, -0.4),
          shoulder_r=(0, 0, -12), shoulder_l=(0, 0, 12))
    m.key(13, "The top: up on the toes and pushing off, drawn as far back as it goes.",
          ease=('Curve', 0.2, 0.0, 0.5, 1.0), root=(0, 0, 0.06), hips=(0.0, -0.03, -0.07),
          body=dict(Hips=(-4, -22, 1), Spine=(-9, -2, 1), Chest=(-11, -2, 1), UpperChest=(-10, 0, 0)),
          L=Foot(lf[0], 0.30, yaw=lf[2], pitch=-6, lift=0.14),
          R=Foot(rf[0], rf[1] + 0.12, yaw=rf[2], pitch=52),
          look=(-2, 18),
          rh=(-8, 84, 0.42), elbow_r=(-0.8, 0.2, -0.6), blade=(180, 14), two_hands=True, elbow_l=(0.8, 0.2, -0.5))
    m.key(16, "The chop, in the air: the body folds forward over the blade coming down.",
          ease='Linear', root=(0, 0, 0.62), hips=(0.0, -0.04, 0.0),
          body=dict(Hips=(7, -12, 1), Spine=(5, 0, 0), Chest=(11, 0, 0), UpperChest=(7, 0, 0)),
          L=Foot(0.18, 1.1, yaw=8, pitch=-22, lift=0.14),
          R=Foot(-0.12, 0.22, yaw=-14, pitch=56, lift=0.12),
          look=(0, 6),
          rh=(-2, 46, 0.43), elbow_r=(-0.8, -0.3, -0.5), blade=(0, 84), two_hands=True, elbow_l=(0.8, -0.3, -0.5),
          shoulder_r=(0, 0, 0), shoulder_l=(0, 0, 0))
    m.key(19, "Landing on the left foot, deep, the blade through and falling toward the ground.",
          ease='Linear', root=(0, 0, 1.3), hips=(0.0, -0.31, 0.02),
          body=dict(Hips=(12, -8, 0), Spine=(8, 0, 0), Chest=(22, 0, 0), UpperChest=(14, 0, 0)),
          L=Foot(0.18, 1.57, yaw=10),
          R=Foot(-0.13, 0.66, yaw=-16, pitch=50, lift=0.02),
          look=(0, -8),
          rh=(2, -42, 0.43), elbow_r=(-0.7, -0.6, -0.3), blade=(0, -18), two_hands=True, elbow_l=(0.7, -0.6, -0.3))
    m.key(23, "Past the mark: it sinks a little further, the tip nearly on the ground.",
          ease=('Overshoot', 0.15), root=(0, 0, 1.36), hips=(0.0, -0.355, 0.03),
          body=dict(Hips=(14, -8, 0), Spine=(9, 0, 0), Chest=(24, 0, 0), UpperChest=(16, 0, 0)),
          R=Foot(-0.13, 0.72, yaw=-16, pitch=46),
          look=(0, -12),
          rh=(2, -50, 0.43), elbow_r=(-0.7, -0.6, -0.3), blade=(0, -24), two_hands=True, elbow_l=(0.7, -0.6, -0.3))
    m.key(32, "Holding the landing a beat, breathing.",
          ease='Out', hips=(0.0, -0.33, 0.02),
          body=dict(Hips=(12, -8, 0), Spine=(8, 0, 0), Chest=(22, 0, 0), UpperChest=(14, 0, 0)),
          look=(0, -4),
          rh=(2, -46, 0.42), elbow_r=(-0.7, -0.6, -0.3), blade=(0, -22), two_hands=True, elbow_l=(0.7, -0.6, -0.3))
    m.key(38, "The back foot lifts to come through.",
          hips=(0.005, -0.25, 0.01),
          R=Foot(-0.12, 0.92, yaw=-14, pitch=34, lift=0.04),
          body=dict(Hips=(10, -12, 0), Spine=(6, 1, 0), Chest=(16, 2, 0), UpperChest=(9, 1, 0)))
    m.key(44, "Rising, the back foot coming through, the left hand letting go.",
          root=(0, 0, 1.4), hips=(0.01, -0.12, 0.0), fingers=LEFT_OPEN,
          L=Foot(0.18, 1.57, yaw=6),
          R=Foot(-0.09, 1.18, yaw=-10, pitch=8, lift=0.03),
          body=dict(Hips=(8, -16, 0), Spine=(6, 2, 0), Chest=(4, 4, 0), UpperChest=(2, 2, 0)),
          look=(0, 0),
          rh=(-16, -36, 0.36), elbow_r=(-0.5, -0.8, -0.4), blade=(4, 34),
          lh=(30, -40, 0.32), elbow_l=(0.7, -0.6, -0.3))
    finish(m, 52, "Ready again.", 1.4, ease='Out')
    return m


def dash_attack():
    m = Move('dash_attack', 34, """The dash attack (M2 O3): out of a dodge's end or from ready, thrown forward low with the blade
trailing, and cut across on the way past. Hit on ticks 5 to 9. Warp bends it toward a target.

""" + HEADER)
    start(m, "Ready.")
    m.key(3, "Thrown forward at once: the body pitches over, the blade and left arm left behind.", root=(0, 0, 0.42), hips=(0.03, -0.15, 0.06),
          body=dict(Hips=(8, -30, 2), Spine=(12, -6, 2), Chest=(22, -6, 2), UpperChest=(12, -4, 0)),
          L=Foot(0.17, 0.72, yaw=4, pitch=-6, lift=0.12),
          R=Foot(-0.08, 0.0, yaw=-9, pitch=32),
          look=(-6, 6),
          rh=(-100, -22, 0.40), elbow_r=(-0.8, -0.4, -0.4), blade=(-122, 24),
          lh=(120, -40, 0.40), elbow_l=(0.6, -0.1, -0.8))
    m.key(5, "In the air, stretched out, the blade at its furthest back.",
          ease='Linear', root=(0, 0, 1.0), hips=(0.03, -0.14, 0.09),
          body=dict(Hips=(8, -34, 2), Spine=(14, -8, 2), Chest=(25, -8, 2), UpperChest=(14, -4, 0)),
          L=Foot(0.17, 1.42, yaw=4, pitch=-10, lift=0.2),
          R=Foot(-0.08, 0.46, yaw=-9, pitch=64, lift=0.1),
          look=(-8, 10),
          rh=(-140, -12, 0.42), elbow_r=(-0.8, -0.4, -0.4), blade=(-170, 14),
          lh=(155, -18, 0.40), elbow_l=(0.6, 0.0, -0.8))
    m.key(7, "The cut: the hips whip round and the arm comes through with the blade flat.",
          ease='Linear', root=(0, 0, 1.72), hips=(0.01, -0.17, 0.05),
          body=dict(Hips=(8, -6, 0), Spine=(12, 4, -2), Chest=(20, 4, -2), UpperChest=(12, 2, 0)),
          L=Foot(0.17, 2.1, yaw=6, pitch=-14, lift=0.1),
          R=Foot(-0.1, 1.2, yaw=-9, pitch=60, lift=0.14),
          look=(-2, 4),
          rh=(-64, -6, 0.43), elbow_r=(-0.8, -0.4, -0.4), blade=(-62, 4), edge=leading_edge(-62, 45),
          lh=(150, -34, 0.42), elbow_l=(0.5, -0.3, -0.8))
    m.key(8, "Flat across the front.",
          ease='Linear', root=(0, 0, 1.96), hips=(0.0, -0.2, 0.05),
          body=dict(Hips=(8, 6, -1), Spine=(11, 8, -3), Chest=(19, 8, -3), UpperChest=(11, 4, 0)),
          L=Foot(0.18, 2.34, yaw=7, pitch=-10, lift=0.06),
          R=Foot(-0.11, 1.46, yaw=-11, pitch=58, lift=0.1),
          look=(0, 2),
          rh=(-18, -10, 0.43), elbow_r=(-0.7, -0.6, -0.4), blade=(-14, 0), edge=leading_edge(-14, 45),
          lh=(135, -42, 0.42), elbow_l=(0.6, -0.3, -0.7))
    m.key(9, "Past the middle, the blade dropping a little as the body comes down.",
          ease='Linear', root=(0, 0, 2.17), hips=(-0.01, -0.225, 0.045),
          body=dict(Hips=(8, 18, -2), Spine=(11, 11, -4), Chest=(18, 11, -3), UpperChest=(9, 6, 0)),
          L=Foot(0.19, 2.52, yaw=9, pitch=-4, lift=0.02),
          R=Foot(-0.12, 1.68, yaw=-13, pitch=57, lift=0.06),
          look=(4, 0),
          rh=(22, -14, 0.43), elbow_r=(-0.4, -0.7, -0.5), blade=(40, -4), edge=leading_edge(40, 45),
          lh=(126, -46, 0.42), elbow_l=(0.7, -0.3, -0.6))
    m.key(10, "The left foot lands and skids, the blade across and out to the left.",
          ease='Linear', root=(0, 0, 2.36), hips=(-0.02, -0.25, 0.04),
          body=dict(Hips=(8, 28, -3), Spine=(11, 14, -5), Chest=(18, 14, -4), UpperChest=(9, 8, 0)),
          L=Foot(0.19, 2.66, yaw=10, planted=False),
          R=Foot(-0.13, 1.88, yaw=-14, pitch=56, lift=0.03),
          look=(8, -2),
          rh=(56, -18, 0.43), elbow_r=(-0.2, -0.8, -0.6), blade=(92, -8), edge=leading_edge(92, 45),
          lh=(120, -50, 0.42), elbow_l=(0.7, -0.3, -0.6))
    m.key(14, "The skid stops, past the mark: sunk low, the chest still turning, the back foot dragging up.",
          ease=('Overshoot', 0.1), root=(0, 0, 2.6), hips=(-0.03, -0.28, 0.02),
          body=dict(Hips=(8, 36, -4), Spine=(9, 15, -5), Chest=(17, 16, -4), UpperChest=(10, 8, 0)),
          L=Foot(0.19, 2.8, yaw=12),
          R=Foot(-0.13, 2.08, yaw=-14, pitch=48),
          look=(10, -4),
          rh=(78, -22, 0.43), elbow_r=(-0.1, -0.8, -0.6), blade=(124, -22),
          lh=(125, -50, 0.42), elbow_l=(0.7, -0.3, -0.6))
    m.key(22, "Settled, low and open.",
          ease='Out', root=(0, 0, 2.64), hips=(-0.02, -0.2, 0.01),
          body=dict(Hips=(7, 26, -3), Spine=(8, 10, -3), Chest=(11, 10, -2), UpperChest=(5, 4, 0)),
          R=Foot(-0.13, 2.2, yaw=-14, pitch=30),
          look=(6, -2),
          rh=(56, -24, 0.38), elbow_r=(-0.3, -0.8, -0.5), blade=(96, -10),
          lh=(110, -50, 0.38), elbow_l=(0.7, -0.4, -0.6))
    m.key(29, "Rising, the back foot coming up, the blade back to the middle.",
          hips=(0.0, -0.1, 0.0), root=(0, 0, 2.64),
          L=Foot(0.18, 2.81, yaw=6),
          R=Foot(-0.09, 2.45, yaw=-10, pitch=10, lift=0.03),
          body=dict(Hips=(7, -6, 0), Spine=(4, 4, 0), Chest=(4, 4, 0), UpperChest=(2, 2, 0)),
          look=(2, 0),
          rh=(-12, -36, 0.36), elbow_r=(-0.5, -0.8, -0.4), blade=(4, 34),
          lh=(36, -40, 0.32), elbow_l=(0.7, -0.6, -0.3))
    finish(m, 34, "Ready again.", 2.64, ease='Out')
    return m


def dodge():
    m = Move('dodge', 24, """The dodge (M2 O3): a low leap straight back, keeping its eyes on the enemy, 2.4 m in 14 ticks. The
camera mostly sees the character's back, so it reads by dropping: the head falls away, the feet split
wide and the arms and blade are left out in front, pulled along behind the motion.

""" + HEADER)
    start(m, "Ready.")
    m.key(2, "At once: it drops and pushes off the front foot, the back foot already reaching.",
          ease='Out', root=(0, 0, -0.26), hips=(-0.02, -0.15, 0.03),
          body=dict(Hips=(10, -22, 3), Spine=(6, 4, 2), Chest=(12, 4, 2), UpperChest=(6, 2, 0)),
          L=Foot(0.17, 0.17, yaw=6, pitch=28),
          R=Foot(-0.12, -0.5, yaw=-14, pitch=-6, lift=0.06),
          look=(-4, 6),
          rh=(-40, -40, 0.34), elbow_r=(-0.8, -0.5, -0.3), blade=(-30, 14),
          lh=(26, -38, 0.34), elbow_l=(0.8, -0.5, -0.3))
    m.key(6, "In the air: flat and low, feet split, arms and blade left behind out front.",
          ease='Linear', root=(0, 0, -1.2), hips=(-0.05, -0.2, 0.0),
          body=dict(Hips=(12, -18, 8), Spine=(8, 6, 5), Chest=(20, 6, 5), UpperChest=(12, 2, 0)),
          L=Foot(0.22, -0.62, yaw=8, pitch=24, lift=0.08),
          R=Foot(-0.22, -1.62, yaw=-16, pitch=-14, lift=0.12),
          look=(-2, 14),
          rh=(-36, -44, 0.43), elbow_r=(-0.9, -0.3, -0.3), blade=(-22, -22),
          lh=(62, -6, 0.43), elbow_l=(0.8, -0.5, -0.3))
    m.key(10, "The back foot catches the ground and skids, the body coming down onto it.",
          ease='Linear', root=(0, 0, -2.0), hips=(-0.06, -0.27, -0.03),
          body=dict(Hips=(12, -14, 9), Spine=(8, 6, 6), Chest=(20, 6, 5), UpperChest=(14, 2, 0)),
          L=Foot(0.24, -1.38, yaw=10, pitch=24, lift=0.08),
          R=Foot(-0.22, -2.34, yaw=-20, pitch=6, planted=False),
          look=(-2, 16),
          rh=(-42, -46, 0.43), elbow_r=(-0.9, -0.3, -0.3), blade=(-26, -24),
          lh=(66, -10, 0.43), elbow_l=(0.8, -0.5, -0.3))
    m.key(12, "The front foot reaches for the ground.",
          ease='Linear', root=(0, 0, -2.27), hips=(-0.05, -0.3, -0.03),
          L=Foot(0.23, -1.8, yaw=10, pitch=16, lift=0.03))
    m.key(14, "Down, past the mark: low and wide, the front foot landing, blade out in guard.",
          ease=('Overshoot', 0.06), root=(0, 0, -2.38), hips=(-0.03, -0.31, -0.03),
          body=dict(Hips=(8, -16, 4), Spine=(6, 6, 3), Chest=(18, 6, 2), UpperChest=(14, 2, 0)),
          L=Foot(0.22, -2.08, yaw=10, pitch=12),
          R=Foot(-0.22, -2.58, yaw=-20),
          look=(-2, 10),
          rh=(-44, -34, 0.40), elbow_r=(-0.9, -0.3, -0.3), blade=(-24, -4),
          lh=(58, -26, 0.40), elbow_l=(0.8, -0.5, -0.3))
    m.key(19, "Coming back up, the feet drawing in.",
          ease='Out', root=(0, 0, -2.4), hips=(-0.01, -0.13, -0.01),
          L=Foot(0.19, -2.23, yaw=6),
          R=Foot(-0.12, -2.5, yaw=-12),
          body=dict(Hips=(8, -20, 1), Spine=(4, 6, 1), Chest=(3, 6, 1), UpperChest=(1, 3, 0)),
          look=(0, 4),
          rh=(-22, -34, 0.36), elbow_r=(-0.5, -0.8, -0.4), blade=(4, 34),
          lh=(32, -40, 0.33), elbow_l=(0.7, -0.6, -0.3))
    finish(m, 24, "Ready again.", -2.4, ease='Out')
    return m


def skill():
    m = Move('skill', 52, """The skill (M2 O3): a coiled drop with the blade held low behind, then a whole turn with the arm out,
which cuts on ticks 12 to 15, and a rising cut coming out of the turn on ticks 21 to 25, off the
ground at its top. A flick of the blade ends it. A key names at most a little over a quarter turn of
root, since bones blend the short way.

""" + HEADER)
    start(m, "Ready.")
    m.key(2, "The blade swings out to the right on its way back, clear of the leg.",
          ease='Out', root=(0, 0, 0.02), hips=(0.015, -0.12, -0.01),
          body=dict(Hips=(6, -32, 1), Spine=(5, -4, 1), Chest=(5, -4, 1), UpperChest=(2, -2, 0)),
          L=Foot(0.18, 0.28, yaw=3, pitch=4, lift=0.02, planted=False),
          look=(-12, -2),
          rh=(-80, -8, 0.38), elbow_r=(-0.8, -0.5, -0.3), blade=(-108, 18),
          lh=(14, -16, 0.36), elbow_l=(0.8, -0.5, -0.2))
    m.key(4, "The drop: down at once and turned away, the left foot sliding forward, the left hand out to aim.",
          root=(0, 0, 0.05), hips=(0.02, -0.2, -0.01),
          body=dict(Hips=(8, -46, 2), Spine=(8, -12, 2), Chest=(8, -12, 2), UpperChest=(4, -6, 0)),
          L=Foot(0.2, 0.46, yaw=4, planted=False),
          R=Foot(-0.08, -0.11, yaw=-30, pitch=10, planted=False),
          look=(-24, -2),
          rh=(-136, -34, 0.40), elbow_r=(-0.8, -0.4, -0.5), blade=(-168, -8),
          lh=(8, 0, 0.40), elbow_l=(0.8, -0.5, -0.2), fingers=LEFT_FLAT)
    m.key(8, "Coiled: lower, the blade hidden behind, still creeping tighter.",
          root=(0, 0, 0.08), hips=(0.03, -0.26, -0.03),
          body=dict(Hips=(8, -52, 3), Spine=(10, -14, 3), Chest=(10, -14, 2), UpperChest=(5, -8, 0)),
          L=Foot(0.21, 0.56, yaw=4),
          R=Foot(-0.1, -0.18, yaw=-34, pitch=14),
          look=(-24, -2),
          rh=(-150, -30, 0.42), elbow_r=(-0.8, -0.4, -0.6), blade=(-176, -14),
          lh=(6, 2, 0.43), elbow_l=(0.8, -0.5, -0.2))
    m.key(12, "The last of it, tighter still.",
          ease='In', root=(0, 0, 0.14), hips=(0.03, -0.28, -0.02),
          body=dict(Hips=(8, -56, 3), Spine=(11, -15, 3), Chest=(10, -15, 2), UpperChest=(5, -8, 0)),
          look=(-22, -2),
          rh=(-156, -28, 0.43), elbow_r=(-0.8, -0.4, -0.6), blade=(-178, -14))
    m.key(13, "Unwinding starts in the hips.",
          ease='In', root=(0.04, 0, 0.24), root_turn=22, hips=(0.02, -0.25, 0.0),
          body=dict(Hips=(7, -40, 2), Spine=(10, -10, 2), Chest=(9, -12, 2), UpperChest=(5, -6, 0)),
          L=Foot(0.21, 0.56, yaw=26, planted=False),
          R=Foot(-0.16, -0.22, yaw=-28, pitch=20, lift=0.02, model=True),
          rh=(-136, -20, 0.43), elbow_r=(-0.8, -0.4, -0.6), blade=(-158, -10))
    m.key(14, "The whole body turns left over the planted foot, the arm flung out.",
          ease='Linear', root=(0.1, 0, 0.4), root_turn=70, hips=(0.0, -0.2, 0.02),
          body=dict(Hips=(6, -16, 0), Spine=(8, 0, 0), Chest=(8, 0, 0), UpperChest=(4, 0, 0)),
          L=Foot(0.21, 0.56, yaw=74, planted=False),
          R=Foot(-0.3, -0.26, yaw=-20, pitch=30, lift=0.06, model=True),
          look=(-10, 0),
          rh=(-78, -6, 0.43), elbow_r=(-0.8, -0.5, -0.4), blade=(-82, 6),
          lh=(90, -30, 0.36), elbow_l=(0.7, -0.3, -0.6), fingers=LEFT_OPEN)
    m.key(16, "Through the front, the arm straight out.",
          ease='Linear', root=(0.14, 0, 0.56), root_turn=150, hips=(0.0, -0.17, 0.02),
          body=dict(Hips=(8, 0, 0), Spine=(6, 2, 0), Chest=(4, 2, 0), UpperChest=(2, 0, 0)),
          L=Foot(0.21, 0.56, yaw=154, planted=False),
          R=Foot(-0.32, -0.3, yaw=-20, pitch=20, lift=0.1, model=True),
          look=(-16, 0),
          rh=(-70, -8, 0.43), elbow_r=(-0.8, -0.5, -0.4), blade=(-72, 4),
          lh=(120, -30, 0.36), elbow_l=(0.6, -0.2, -0.7))
    m.key(19, "Round the back, keeping its speed.",
          ease='Linear', root=(0.14, 0, 0.62), root_turn=240, hips=(0.0, -0.18, 0.0),
          L=Foot(0.21, 0.56, yaw=244, planted=False),
          R=Foot(-0.3, -0.34, yaw=-20, pitch=16, lift=0.1, model=True),
          rh=(-64, -24, 0.43), elbow_r=(-0.8, -0.5, -0.4), blade=(-60, -18),
          lh=(130, -24, 0.36), elbow_l=(0.6, -0.2, -0.7))
    m.key(21, "Dropping under for the second cut as the turn comes home.",
          ease='Linear', root=(0.1, 0, 0.68), root_turn=300, hips=(0.0, -0.24, 0.0),
          body=dict(Hips=(7, -6, 0), Spine=(9, 0, 0), Chest=(8, 0, 0), UpperChest=(5, 0, 0)),
          L=Foot(0.21, 0.56, yaw=304, planted=False),
          R=Foot(-0.24, -0.34, yaw=-20, pitch=16, lift=0.05, model=True),
          look=(-6, 0),
          rh=(-54, -46, 0.43), elbow_r=(-0.8, -0.5, -0.3), blade=(-40, -34),
          lh=(110, -30, 0.36), elbow_l=(0.7, -0.2, -0.6))
    m.key(23, "Rising, driving up off the planted foot.",
          ease='Linear', root=(0.04, 0, 0.82), root_turn=340, hips=(0.0, -0.1, 0.0),
          body=dict(Hips=(2, 8, 0), Spine=(-2, 6, 0), Chest=(-4, 6, 0), UpperChest=(-4, 2, 0)),
          L=Foot(0.17, 0.9, yaw=344, pitch=30, planted=False),
          R=Foot(-0.2, -0.24, yaw=-14, pitch=30, lift=0.08, model=True),
          look=(-2, 8),
          rh=(-14, 8, 0.43), elbow_r=(-0.8, -0.3, -0.5), blade=(-8, 36),
          lh=(90, -36, 0.36), elbow_l=(0.7, -0.3, -0.6))
    m.key(26, "The top: off the ground, stretched, the blade straight up over the left shoulder.",
          ease=('Overshoot', 0.08), root=(0, 0, 1.02), root_turn=360, hips=(0.0, 0.05, 0.0),
          body=dict(Hips=(-2, 14, -1), Spine=(-6, 10, -2), Chest=(-8, 10, -2), UpperChest=(-6, 4, 0)),
          L=Foot(0.15, 1.14, yaw=360, pitch=44, lift=0.1, planted=False),
          R=Foot(-0.12, -0.2, yaw=-10, pitch=46, lift=0.14, model=True),
          look=(0, 18),
          rh=(26, 72, 0.43), elbow_r=(-0.7, 0.2, -0.6), blade=(40, 76),
          lh=(70, -44, 0.38), elbow_l=(0.8, -0.3, -0.4),
          shoulder_r=(0, 0, -10))
    m.key(31, "Coming down: the left foot lands, knees taking it, the blade still high.",
          ease='In', root=(0, 0, 1.08), root_turn=360, hips=(0.0, -0.2, 0.0),
          L=Foot(0.17, 1.25, yaw=360),
          R=Foot(-0.09, 0.9, yaw=350, pitch=14, planted=False),
          body=dict(Hips=(10, 6, -1), Spine=(6, 4, -1), Chest=(4, 4, -1), UpperChest=(2, 2, 0)),
          look=(0, 0),
          rh=(14, 56, 0.40), elbow_r=(-0.7, -0.1, -0.6), blade=(34, 70),
          lh=(60, -40, 0.34), elbow_l=(0.8, -0.4, -0.4),
          shoulder_r=(0, 0, 0))
    m.key(37, "The flick: the blade swung down and out to the right, a clean stop.",
          ease=('Overshoot', 0.06), hips=(0.01, -0.17, 0.0),
          R=Foot(-0.08, 0.97, yaw=351),
          body=dict(Hips=(8, -14, 1), Spine=(5, -6, 1), Chest=(4, -6, 1), UpperChest=(2, -2, 0)),
          rh=(-66, -28, 0.43), elbow_r=(-0.8, -0.5, -0.4), blade=(-82, -34),
          lh=(44, -40, 0.34), elbow_l=(0.8, -0.5, -0.3))
    m.key(44, "Held a beat.", ease='Linear', hips=(0.01, -0.16, 0.0))
    finish(m, 52, "Ready again.", 1.08)
    return m


MOVES = [light_1, light_2, light_3, dash_attack, dodge, skill]

SIDECAR = '''(
    type: "sapphire::assets::AssetMeta",
    version: 1,
    data: (
        id: "%s",
        importer: KeyPose((
            rig: "6f8a81b6-29c4-485f-b161-847b001d561f",
        )),
    ),
)
'''


def main(args=None, folder=FOLDER, stamp=STAMP):
    args = sys.argv[1:] if args is None else args
    writing = '--write' in args
    numbers = '--write-numbers' in args
    force = '--force' in args
    if force and not numbers:
        sys.exit('--force goes with --write-numbers, the only write that ever refuses')
    verbose = '-v' in args
    only = [a for a in args if not a.startswith('-')]
    built = {}
    for make in MOVES:
        move = make()
        built[move.name] = move
        if not only or move.name in only:
            move.check(verbose=verbose)
    if writing:
        for name, move in built.items():
            with open(os.path.join(folder, name + '.keypose.ron'), 'w') as f:
                f.write(move.ron())
            with open(os.path.join(folder, name + '.keypose.ron.meta'), 'w') as f:
                f.write(SIDECAR % frames.IDS[name])
        print(f'wrote the key-pose documents in {folder}')
        note = frames.numbers_note(folder, built)
        if note:
            print(note)
    if numbers:
        if frames.write_numbers(folder, built, stamp, force):
            print(f'wrote {frames.NUMBERS}. It owes the simulation its cooked copy: SAPPHIRE_BAKE_MOVES=1 cargo test '
                  '-p sapphire_pipeline --test player_moves the_simulations_copy, then cargo xtask replay --update')
        else:
            print(f'{frames.NUMBERS} is already what frames.py writes')


if __name__ == '__main__':
    main()
