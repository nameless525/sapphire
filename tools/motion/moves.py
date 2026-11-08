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
from light3_sleeve import add_sleeve_keys
from pose import *
from rig import REPO

FOLDER = os.path.join(REPO, 'content/combat/player')
# The digest of what `--write-numbers` last wrote, kept out of `content/`, which is cooked.
STAMP = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'player.moves.written')

# Common sword guard for idle, combat starts and recoveries. The blade points up and
# out from a low hilt with bent elbows, leaving the face and centerline open.
IDLE_HIPS = (0.015, -0.041, -0.036)
IDLE_BODY = dict(Hips=(3, -15, 0), Spine=(2, 5, 0), Chest=(0, 5, -1), UpperChest=(-1, 3, 0))
IDLE_ARMS = dict(
    rh=(-13, -50, 0.34), elbow_r=(-0.62, -0.75, -0.25), blade=(-21, 35),
    lh=(28, -53, 0.32), elbow_l=(0.68, -0.68, -0.18),
    shoulder_r=(0, 0, 0), shoulder_l=(0, 0, 0),
)
IDLE_L = (0.17, 0.17, 6)
IDLE_R = (-0.08, -0.11, -13)

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


def sword(azimuth, elevation, roll):
    """Aim the blade and set its edge roll from the horizontal sweep direction."""
    direction = polar(azimuth, elevation)
    edge = qrot(qaxis(direction, roll), leading_edge(azimuth, 0))
    return dict(blade=(azimuth, elevation), edge=edge)


def start(move, say):
    return finish(move, 0, say, 0.0, fingers=HANDS)


def finish(move, tick, say, root_z, root_x=0.0, ease='Smooth', fingers=None):
    feet = idle_feet(root_z, root_x)
    for foot in feet.values():
        foot.yaw += move.keys[-1]['root_turn'] if move.keys else 0.0
    return move.key(tick, say, ease=ease, root=(root_x, 0, root_z), hips=IDLE_HIPS,
                    body=IDLE_BODY, look=(0, 1), fingers=fingers, **feet, **IDLE_ARMS)

# What the end of one hit leaves the body doing: the next hit starts from it.
AFTER_1 = dict(
    hips=(-0.035, -0.143, -0.018),
    body=dict(Hips=(6, 23, -3), Spine=(5, 8, -3), Chest=(8, 9, -2), UpperChest=(5, 4, 0)),
    look=(6, -3), shoulder_r=(0, 4, 0), shoulder_l=(0, -2, 0),
    rh=(49, -31, 0.371), elbow_r=(-0.32, -0.76, -0.47), blade=(94, -14),
    lh=(109, -46, 0.344), elbow_l=(0.64, -0.53, -0.43),
)
AFTER_1_FEET = dict(L=(0.17, -0.40, 12, 22), R=(-0.104, 0.08, -13, 0))

AFTER_2 = dict(
    hips=(0.049, -0.134, -0.039),
    body=dict(Hips=(6, -23, 3), Spine=(4, -9, 2), Chest=(3, -12, 2), UpperChest=(1, -6, 0)),
    look=(-4, 2), shoulder_r=(0, -1, -5), shoulder_l=(0, -4, 1),
    rh=(-63, 30, 0.35), elbow_r=(-0.70, -0.55, -0.41), **sword(-128, 60, -1),
    lh=(42, -34, 0.325), elbow_l=(0.73, -0.57, -0.24),
)
AFTER_2_FEET = dict(L=(0.19, 0.02, 14, 0), R=(-0.104, -0.42, -13, 24))


def heel_contact(foot, pitch):
    heel = qrot(qaxis((0, 1, 0), foot.yaw), HEEL_FROM_TOE)
    return Foot(foot.x + heel[0], foot.z + heel[2], yaw=foot.yaw, pitch=pitch, pivot='heel')


def light_1():
    m = Move('light_1', 34, """The first cut of the string (M2 O3): a forehand slash from high on the right down across to the
low left, on a step of the right foot. Hit on ticks 7 to 10. It leaves the blade low on the left and
the right foot forward, where `light_2` starts; left alone, the back foot steps through to ready.

""" + HEADER)
    start(m, "Ready, as the combat idle holds the character.")
    landing = Foot(-0.104, 0.78, yaw=-13)
    m.key(2, "The pelvis drops toward the left shoe. The right elbow has only begun to draw back.",
          ease='Linear', root=(0, 0, 0.008), hips=(0.047, -0.111, -0.029),
          body=dict(Hips=(5, -23, 4), Spine=(4, 1, 0), Chest=(2, 2, -1), UpperChest=(0, 2, 0)),
          look=(0, 1), shoulder_r=(0, -3, 0),
          rh=(-33, -35, 0.336), **sword(-47, 34, -37), lh=(27, -47, 0.321),
          elbow_l=(0.69, -0.66, -0.2))
    m.key(4, "The free shoulder comes forward as the sword-side ribs draw back.",
          ease='Linear', root=(0, 0, 0.023), hips=(0.083, -0.152, -0.011),
          body=dict(Hips=(6, -29, 5), Spine=(3, -3, -1), Chest=(0, -5, -3), UpperChest=(-2, -3, -2)),
          shoulder_r=(0, -8, -3), shoulder_l=(0, -7, 2),
          R=Foot(-0.088, 0.021, yaw=-13, pitch=30, lift=0.057),
          look=(-5, 1, 2),
          rh=(-62, 7, 0.335), elbow_r=(-0.67, -0.65, -0.34), **sword(-91, 44, -20),
          lh=(32, -36, 0.322), elbow_l=(0.72, -0.61, -0.24))
    m.key(5, "The right knee passes the support foot while the shoulders continue to load.",
          ease='Linear', root=(0, 0, 0.035), hips=(0.087, -0.158, 0.038),
          R=Foot(-0.098, 0.302, yaw=-14, pitch=5, lift=0.091),
          body=dict(Hips=(5, -30, 5), Spine=(2, -7, 0), Chest=(-2, -10, -4), UpperChest=(-3, -5, -3)),
          shoulder_r=(0, -10, -4), shoulder_l=(0, -9, 2), look=(-8, 2, 2),
          rh=(-77, 20, 0.343), elbow_r=(-0.65, -0.62, -0.4), **sword(-117, 49, -7),
          lh=(25, -29, 0.341), elbow_l=(0.72, -0.6, -0.25))
    m.key(6, "The right foot swings ahead as the loaded left ankle drives the pelvis forward.",
          ease='Linear', root=(0, 0, 0.049), hips=(0.082, -0.155, 0.075),
          R=Foot(-0.103, 0.571, yaw=-14, pitch=-13, lift=0.065),
          body=dict(Hips=(5, -29, 5), Spine=(1, -11, 2), Chest=(-4, -15, -5), UpperChest=(-4, -7, -3)),
          shoulder_r=(0, -11, -5), shoulder_l=(0, -10, 2), look=(-10, 3, 2),
          rh=(-91, 28, 0.352), elbow_r=(-0.64, -0.59, -0.46), **sword(-137, 53, 3),
          lh=(23, -24, 0.353), elbow_l=(0.72, -0.59, -0.25))
    m.key(7, "The right heel catches while the ribs and sword finish drawing back.",
          ease='Linear', root=(0, 0, 0.06), hips=(0.061, -0.191, 0.118),
          body=dict(Hips=(5, -19, 5), Spine=(0, -15, 2), Chest=(-5, -19, -5), UpperChest=(-4, -9, -3)),
          shoulder_r=(0, -12, -5), shoulder_l=(0, -10, 2),
          R=heel_contact(landing, -15),
          look=(-14, 3, 2),
          rh=(-106, 34, 0.357), elbow_r=(-0.64, -0.58, -0.5), **sword(-152, 56, 10),
          lh=(23, -19, 0.365), elbow_l=(0.72, -0.59, -0.25))
    m.key(8, "The cut starts in the hips: they turn and drive forward while the arm stays back.",
          ease='In', root=(0, 0, 0.14), hips=(0.035, -0.166, 0.068),
          body=dict(Hips=(7, -10, 2), Spine=(3, -12, 2), Chest=(0, -20, -2), UpperChest=(-1, -8, 0)),
          shoulder_r=(0, -10, -4), shoulder_l=(0, -7, 1),
          L=Foot(0.17, 0.17, yaw=6, pitch=16, lift=0.025),
          R=heel_contact(landing, -8),
          look=(-16, -2),
          rh=(-88, 33, 0.37), elbow_r=(-0.62, -0.6, -0.48), **sword(-142, 57, 0),
          lh=(37, -23, 0.362), elbow_l=(0.72, -0.58, -0.26))
    m.key(9, "Then the chest, and the arm comes after it, the wrist still behind.",
          ease='Linear', root=(0, 0, 0.3), hips=(0.019, -0.105, 0.021),
          body=dict(Hips=(7, 1, -1), Spine=(5, -2, -1), Chest=(7, -7, -3), UpperChest=(4, -4, 0)),
          shoulder_r=(0, -3, -2), shoulder_l=(0, -3, 0),
          L=Foot(0.17, 0.17, yaw=6, pitch=26, lift=0.043), R=landing,
          look=(-8, -4),
          rh=(-56, 14, 0.388), elbow_r=(-0.65, -0.59, -0.47), **sword(-81, 42, -60),
          lh=(62, -30, 0.353), elbow_l=(0.71, -0.55, -0.33))
    m.key(10, "The right hip drives over the planted shoe while the ribs are still behind it.",
          ease='Linear', root=(0, 0, 0.44), hips=(-0.030, -0.120, 0.053),
          body=dict(Hips=(7, 22, 1), Spine=(6, 1, -3), Chest=(10, -2, -4), UpperChest=(5, 0, -1)),
          shoulder_r=(0, 7, -2), shoulder_l=(0, 4, -1),
          L=Foot(0.17, 0.17, yaw=8, pitch=33, lift=0.059),
          look=(-2, -5),
          rh=(-6.07, -23.52, 0.3538), elbow_r=(-0.54, -0.56, -0.62), **sword(-16, 15, -90),
          lh=(87, -35, 0.35), elbow_l=(0.69, -0.53, -0.39))
    m.key(11, "The right knee catches the strike. The ribs are still turning after the pelvis brakes.",
          ease='Linear', root=(0, 0, 0.56), hips=(-0.092, -0.171, 0.077),
          body=dict(Hips=(8, 30, 2), Spine=(7, 7, -5), Chest=(13, 6, -6), UpperChest=(8, 4, -2)),
          shoulder_r=(0, 13, 0), shoulder_l=(0, 7, -2),
          L=Foot(0.17, 0.17, yaw=10, pitch=37, lift=0.061),
          look=(4, -6),
          rh=(45.61, -31.99, 0.3963), elbow_r=(-0.44, -0.64, -0.43), **sword(60, -12, -90),
          lh=(107, -40, 0.35), elbow_l=(0.66, -0.52, -0.42))
    m.key(14, "The sternum and sword shoulder finish across the planted knee; the point follows low.",
          ease='Out', root=(0, 0, 0.65), hips=(-0.104, -0.203, 0.088),
          body=dict(Hips=(7, 24, 2), Spine=(7, 13, -7), Chest=(18, 17, -8), UpperChest=(10, 9, -5)),
          shoulder_r=(0, 13, 4), shoulder_l=(0, 6, -2),
          L=Foot(0.17, 0.265, yaw=12, pitch=34, lift=0.043),
          look=(3, -4),
          rh=(58, -42, 0.390), elbow_r=(-0.27, -0.73, -0.55), **sword(124, -27, -138),
          lh=(119, -46, 0.342), elbow_l=(0.64, -0.53, -0.43))
    m.key(18, "The eyes return first. The ribs unwind while the tucked hilt and low point lag.",
          ease='Linear', root=(0, 0, 0.686), hips=(-0.075, -0.176, 0.033),
          body=dict(Hips=(6, 20, -2), Spine=(6, 9, -5), Chest=(11, 13, -4), UpperChest=(7, 7, -1)),
          L=Foot(0.17, 0.298, yaw=12, pitch=30), shoulder_r=(0, 7, 1), shoulder_l=(0, 2, -1),
          look=(3, -1), rh=(51, -36, 0.370), elbow_r=(-0.30, -0.75, -0.48), **sword(108, -20, -130),
          lh=(117, -46, 0.346), elbow_l=(0.64, -0.53, -0.43))
    m.key(22, "Settled and open: blade low on the left, right foot forward, ready to be cancelled.",
          ease='Out', root=(0, 0, 0.7),
          L=Foot(0.17, 0.30, yaw=12, pitch=22), **AFTER_1)
    m.key(26, "The free shoulder starts back and the weight gathers over the right foot.", ease='Linear',
          hips=(-0.071, -0.122, 0.028), shoulder_r=(0, 2, 0), shoulder_l=(0, -3, 0),
          L=Foot(0.173, 0.371, yaw=10, pitch=31, lift=0.046),
          body=dict(Hips=(6, 20, -2), Spine=(3, 10, -3), Chest=(8, 8, -2), UpperChest=(4, 4, 0)),
          look=(4, -1), rh=(36, -29, 0.39), blade=(68, 3), lh=(88, -41, 0.37))
    m.key(30, "Left alone, the back foot comes through, the blade coming back to the middle.",
          root=(0, 0, 0.78), hips=(-0.052, -0.092, 0.018), shoulder_r=(0, 0, 0), shoulder_l=(0, -4, 0),
          body=dict(Hips=(6, 0, -1), Spine=(4, 6, -1), Chest=(4, 6, -1), UpperChest=(2, 2, 0)),
          L=Foot(0.171, 0.81, yaw=6, pitch=14, lift=0.094),
          look=(4, -2),
          rh=(-20, -36, 0.36), elbow_r=(-0.5, -0.8, -0.4), **sword(10, 30, -120),
          lh=(40, -40, 0.32), elbow_l=(0.7, -0.6, -0.3))
    finish(m, 34, "Ready again.", 0.89, ease='Out')
    return m


def light_2():
    m = Move('light_2', 36, """The second cut (M2 O3): a backhand rising from low on the left to high on the right, on a step of
the left foot. Hit on ticks 7 to 10. It starts where `light_1` settles and leaves the blade high on
the right, with the left foot forward, where `light_3` starts.

""" + HEADER)
    lf, rf = AFTER_1_FEET['L'], AFTER_1_FEET['R']
    landing = Foot(0.19, 0.62, yaw=14)
    m.key(0, "Where the first cut settles.", fingers=HANDS,
          L=Foot(lf[0], lf[1], yaw=lf[2], pitch=lf[3]), R=Foot(rf[0], rf[1], yaw=rf[2], pitch=rf[3]), **AFTER_1)
    m.key(3, "At once: the weight drops onto the right foot, the blade pulling further back and low.",
          ease='Linear', root=(0, 0, 0.018), hips=(-0.083, -0.183, 0.004),
          body=dict(Hips=(8, 32, -4), Spine=(7, 9, -4), Chest=(9, 10, -3), UpperChest=(5, 5, 0)),
          shoulder_r=(0, 8, 1), shoulder_l=(0, 5, -2),
          L=Foot(lf[0], lf[1], yaw=lf[2], pitch=29, pivot='ball'),
          look=(10, -3),
          rh=(64, -43, 0.377), elbow_r=(-0.34, -0.75, -0.5), **sword(125, -28, -150),
          lh=(106, -39, 0.346), elbow_l=(0.66, -0.52, -0.43))
    m.key(5, "Swing the left knee through from behind the loaded right ankle.",
          ease='Linear', root=(0, 0, 0.038), hips=(-0.095, -0.198, 0.043),
          L=Foot(0.183, 0.027, yaw=9, pitch=23, lift=0.114),
          body=dict(Hips=(8, 34, -6), Spine=(8, 12, -4), Chest=(13, 14, -4), UpperChest=(7, 7, -2)),
          shoulder_r=(0, 9, 2), shoulder_l=(0, 7, -2), look=(13, -2),
          rh=(69, -45, 0.39), elbow_r=(-0.31, -0.75, -0.50), **sword(137, -29, -117),
          lh=(111, -36, 0.351), elbow_l=(0.65, -0.51, -0.44))
    m.key(6, "The left leg extends through the swing with the heel leading.",
          ease='Linear', root=(0, 0, 0.049), hips=(-0.092, -0.196, 0.056),
          L=Foot(0.188, 0.344, yaw=12, pitch=-11, lift=0.08),
          body=dict(Hips=(8, 33, -6), Spine=(8, 13, -4), Chest=(14, 15, -4), UpperChest=(7, 8, -2)),
          shoulder_r=(0, 10, 2), shoulder_l=(0, 8, -2), look=(14, -2),
          rh=(71, -45, 0.394), elbow_r=(-0.3, -0.75, -0.5), **sword(141, -28, -108),
          lh=(112, -35, 0.352), elbow_l=(0.65, -0.5, -0.44))
    m.key(7, "The left heel catches while the right hip is still coiled for the upswing.",
          ease='Linear', root=(0, 0, 0.06), hips=(-0.089, -0.192, 0.075),
          body=dict(Hips=(8, 26, -6), Spine=(8, 17, -4), Chest=(16, 19, -4), UpperChest=(7, 8, -2)),
          shoulder_r=(0, 10, 2), shoulder_l=(0, 8, -2),
          L=heel_contact(landing, -15),
          look=(14, -2),
          rh=(73, -45, 0.396), elbow_r=(-0.29, -0.75, -0.51), **sword(143, -28, -100),
          lh=(113, -34, 0.352), elbow_l=(0.65, -0.5, -0.44))
    m.key(8, "The hips swing back through first, the arm still low on the left.",
          ease='In', root=(0, 0, 0.13), hips=(-0.065, -0.176, 0.048),
          body=dict(Hips=(7, 12, -3), Spine=(6, 14, -3), Chest=(11, 15, -2), UpperChest=(5, 7, 0)),
          shoulder_r=(0, 7, 1), shoulder_l=(0, 4, -1),
          L=heel_contact(landing, -8),
          R=Foot(rf[0], rf[1], yaw=rf[2], pitch=13, lift=0.02),
          look=(8, -1),
          rh=(55, -44, 0.397), elbow_r=(-0.38, -0.72, -0.5), **sword(114, -31, -100),
          lh=(107, -39, 0.352), elbow_l=(0.66, -0.52, -0.41))
    m.key(9, "The cut, rising: the blade comes under and up across the front.",
          ease='Linear', root=(0, 0, 0.26), hips=(-0.024, -0.133, 0.035),
          body=dict(Hips=(6, -2, -1), Spine=(4, 0, 0), Chest=(6, 5, 0), UpperChest=(3, 4, 0)),
          shoulder_r=(0, 5, -1), shoulder_l=(0, -4, 1),
          L=landing, R=Foot(rf[0], rf[1], yaw=rf[2], pitch=23, lift=0.037),
          look=(4, 0),
          rh=(9, -36, 0.38), elbow_r=(-0.39, -0.55, -0.59), **sword(32, -10, -120),
          lh=(94, -43, 0.347), elbow_l=(0.67, -0.53, -0.37))
    m.key(10, "Through the front, the blade at its fastest.",
          ease='Linear', root=(0, 0, 0.38), hips=(0.017, -0.111, 0.016),
          body=dict(Hips=(5, -14, 1), Spine=(2, -5, 2), Chest=(3, -5, 2), UpperChest=(1, -3, 1)),
          shoulder_r=(0, 6, -5), shoulder_l=(0, -8, 2),
          R=Foot(rf[0], rf[1], yaw=rf[2], pitch=32, lift=0.052),
          look=(-1, 1),
          rh=(-29, -4, 0.396), elbow_r=(-0.55, -0.56, -0.57), **sword(-34, 28, -110),
          lh=(80, -44, 0.344), elbow_l=(0.7, -0.55, -0.32))
    m.key(11, "The left foot supports the rising chest as the blade climbs past the right shoulder.",
          ease='Linear', root=(0, 0, 0.5), hips=(0.050, -0.113, 0.016),
          body=dict(Hips=(5, -24, 3), Spine=(1, -10, 3), Chest=(0, -11, 4), UpperChest=(-2, -5, 2)),
          shoulder_r=(0, 2, -9), shoulder_l=(0, -9, 3),
          R=Foot(rf[0], rf[1], yaw=rf[2], pitch=34, lift=0.053),
          look=(-6, 2),
          rh=(-61, 29, 0.385), elbow_r=(-0.66, -0.56, -0.45), **sword(-82, 63, -70),
          lh=(64, -43, 0.344), elbow_l=(0.71, -0.56, -0.29))
    m.key(14, "The left hip receives the weight. The ribs and blade continue past it toward the rear shoulder.",
          ease='Out', root=(0, 0, 0.571), hips=(0.098, -0.179, 0.056),
          body=dict(Hips=(6, -22, -2), Spine=(0, -16, 4), Chest=(-5, -19, 7), UpperChest=(-5, -8, 3)),
          shoulder_r=(0, -7, -10), shoulder_l=(0, -7, 3),
          R=Foot(rf[0], 0.146, yaw=rf[2], pitch=33, lift=0.033),
          look=(-5, 3),
          rh=(-78, 43, 0.361), elbow_r=(-0.76, -0.48, -0.38), **sword(-144, 59, 4),
          lh=(49, -34, 0.332), elbow_l=(0.73, -0.57, -0.26))
    m.key(18, "The rear ball returns to the floor as the elbow folds and the sword settles into the finisher’s pickup.",
          ease='Linear', root=(0, 0, 0.59), hips=(0.066, -0.155, 0.009),
          R=Foot(rf[0], 0.18, yaw=rf[2], pitch=28), shoulder_r=(0, -1, -6), shoulder_l=(0, -4, 1),
          body=dict(Hips=(6, -23, 4), Spine=(3, -12, 3), Chest=(2, -14, 4), UpperChest=(1, -6, 1)),
          look=(-4, 2), rh=(-65, 31, 0.350), elbow_r=(-0.70, -0.55, -0.42), **sword(-132, 57, -2),
          lh=(42, -34, 0.325), elbow_l=(0.73, -0.57, -0.24))
    m.key(22, "Settled and open: blade high on the right, left foot forward.",
          ease='Out', root=(0, 0, 0.6), R=Foot(rf[0], 0.18, yaw=rf[2], pitch=24), **AFTER_2)
    m.key(27, "The blade starts down as the free shoulder rolls forward.", ease='Linear',
          hips=(0.071, -0.103, -0.008), shoulder_r=(0, 3, -3), shoulder_l=(0, -5, 1),
          R=Foot(-0.11, 0.215, yaw=-13, pitch=29, lift=0.05),
          body=dict(Hips=(6, -24, 2), Spine=(4, -8, 2), Chest=(2, -8, 2), UpperChest=(0, -4, 0)),
          look=(-3, 2), rh=(-53, 11, 0.345), elbow_r=(-0.67, -0.57, -0.40), **sword(-90, 49, -34),
          lh=(40, -40, 0.336), elbow_l=(0.72, -0.60, -0.24))
    m.key(32, "Left alone, the back foot comes up, the body draws back over it and the blade comes down.",
          root=(0, 0, 0.5), hips=(0.039, -0.073, -0.014),
          R=Foot(-0.086, 0.284, yaw=-12, pitch=17, lift=0.07),
          body=dict(Hips=(6, -22, 1), Spine=(3, 0, 1), Chest=(3, 2, 1), UpperChest=(1, 2, 0)),
          look=(2, 0),
          rh=(-30, -30, 0.36), elbow_r=(-0.5, -0.8, -0.4), **sword(5, 40, -90),
          lh=(30, -42, 0.32), elbow_l=(0.7, -0.6, -0.3))
    finish(m, 36, "Ready again.", 0.45, ease='Out')
    return m


def light_3():
    m = Move('light_3', 52, """The finisher (M2 O3): a two-handed chop on a long bound of the left foot, the heavy end of the
string. Hit on ticks 16 to 19. It starts where `light_2` settles, catches the rising sword, winds it over the right
shoulder and drives it diagonally into a low, left-foot landing.

""" + HEADER)
    lf, rf = AFTER_2_FEET['L'], AFTER_2_FEET['R']
    m.key(0, "Where the second cut settles.", fingers=HANDS,
          L=Foot(lf[0], lf[1], yaw=lf[2], pitch=lf[3]), R=Foot(rf[0], rf[1], yaw=rf[2], pitch=rf[3]), **AFTER_2)
    rear_load = Foot(rf[0], rf[1], yaw=rf[2], pitch=24, pivot='ball')
    front_plant = Foot(0.17, 1.57, yaw=6)
    front_heel = heel_contact(front_plant, -9)
    rear_plant = Foot(-0.145, 1.01, yaw=-23, pivot='ball')
    rear_reset = idle_feet(1.4)['R']
    m.key(2, "The pelvis draws diagonally toward the rear shoe while the sword shoulder stays in place.",
          ease='Linear', root=(0, 0, -0.013), hips=(-0.025, -0.171, -0.108), R=rear_load,
          body=dict(Hips=(6, -27, 7), Spine=(5, -6, 0), Chest=(5, -8, 0), UpperChest=(3, -5, 0)),
          look=(-5, 1, 2), shoulder_r=(0, -1, -3), shoulder_l=(0, -5, 2),
          rh=(-60, 27, 0.351), elbow_r=(-0.73, -0.57, -0.37), **sword(-133, 55, 18),
          lh=(26, -25, 0.324), elbow_l=(0.72, -0.57, -0.24))
    m.key(4, "The rear leg receives the load. The ribs and hilt remain forward of the turning hip.",
          ease='Out', root=(0, 0, -0.028), hips=(-0.096, -0.207, -0.181), R=rear_load,
          body=dict(Hips=(5, -34, 9), Spine=(6, -1, -3), Chest=(5, -6, -2), UpperChest=(4, -4, 0)),
          look=(-3, 1, 2), shoulder_r=(0, -3, -1), shoulder_l=(0, -6, 2),
          rh=(-48, 21, 0.331), elbow_r=(-0.76, -0.62, -0.29), **sword(-135, 44, 36),
          lh=(9, -23, 0.315), elbow_l=(0.72, -0.58, -0.24), lhand=turn(-17, 3, 1))
    m.key(6, "The lead heel peels and the rear knee lengthens. The sternum rises under the waiting hilt.",
          ease='Linear', root=(0, 0, -0.03), hips=(-0.103, -0.182, -0.174),
          L=Foot(lf[0], lf[1], yaw=lf[2], pitch=12),
          body=dict(Hips=(4, -35, 8), Spine=(1, -4, -5), Chest=(-3, -7, -5), UpperChest=(-4, -5, -2)),
          look=(-3, 2, 1), shoulder_r=(0, -3, -2), shoulder_l=(0, -6, 2),
          rh=(-48.05, 15.09, 0.3805), elbow_r=(-0.78, -0.52, -0.29), **sword(-137, 45, 44),
          lh=(-23, 6, 0.283), elbow_l=(0.75, -0.45, -0.23), lhand=turn(-37, 5, 2))
    m.key(8, "The left hand closes behind the right and the elbows follow the lifted ribs.",
          root=(0, 0, -0.03), hips=(-0.092, -0.157, -0.141),
          L=Foot(lf[0], lf[1], yaw=lf[2], pitch=18), R=rear_load,
          body=dict(Hips=(2, -33, 5), Spine=(-2, -7, -2), Chest=(-4, -9, -1), UpperChest=(-4, -3, 0)),
          look=(-3, 3, -1), fingers=LEFT_FIST,
          rh=(-30.05, 38.25, 0.3595), elbow_r=(-0.86, -0.13, -0.24), **sword(-145, 46, 56), two_hands=True,
          elbow_l=(0.85, -0.04, -0.23), shoulder_r=(0, -2, -8), shoulder_l=(0, -7, 8))
    m.key(11, "The hilt rises in front of the temple as the lead knee folds into the bound.",
          ease='Linear', root=(0, 0, 0.015), hips=(-0.064, -0.10, -0.092),
          L=Foot(0.2, 0.25, yaw=11, pitch=26, lift=0.125), R=Foot(rf[0], rf[1], yaw=-19, pitch=36),
          body=dict(Hips=(0, -30, 2), Spine=(-5, -6, 4), Chest=(-8, -6, 5), UpperChest=(-7, -2, 3)),
          look=(-3, 7, 8), neck_share=0.63,
          rh=(-18.70, 61.83, 0.3996), elbow_r=(-0.83, 0.23, -0.45), **sword(-157, 30, 70), two_hands=True,
          elbow_l=(0.90, 0.12, 0.10), shoulder_r=(0, 0, -12), shoulder_l=(0, -11, 9))
    # Roll the support thumb along its own fingers as the hands draw close at the crest.
    # The palm and the other four fingers hold their original wrap.
    chamber_thumb = dict(LeftThumbProximal=(5, -61, 6), LeftThumbDistal=(0, -30, 0))
    closed_thumb = dict(LeftThumbProximal=(0, -26, -14), LeftThumbDistal=(0, -30, 0))
    m.key(13, "The rear ball pushes off. The sternum starts forward while the blade trails behind the raised hands.",
          ease='Linear', root=(0, 0, 0.13), hips=(-0.041, -0.068, -0.068),
          L=Foot(0.22, 0.54, yaw=10, pitch=-3, lift=0.23), R=Foot(rf[0], rf[1] + 0.08, yaw=-21, pitch=51),
          body=dict(Hips=(-3, -27, 1), Spine=(-8, -5, 4), Chest=(-11, -5, 5), UpperChest=(-6, -2, 3)),
          look=(-4, 11, 7), neck_share=0.64,
          fingers=chamber_thumb,
          rh=(-3.59, 66.43, 0.3931), elbow_r=(-0.84, 0.19, -0.44), **sword(-161, 18, 80), two_hands=True,
          elbow_l=(0.90, 0.12, 0.10), shoulder_l=(0, -10, 10))
    m.key(16, "The pelvis and lead knee drive forward. The hilt accelerates before the blade falls.",
          ease='Linear', root=(0, 0, 0.62), hips=(0.008, 0.012, 0.0),
          body=dict(Hips=(6, -5, -2), Spine=(4, 5, -3), Chest=(10, 4, -4), UpperChest=(7, 3, -3)),
          L=Foot(0.21, 1.11, yaw=8, pitch=-12, lift=0.245),
          R=Foot(-0.14, 0.29, yaw=-18, pitch=49, lift=0.19),
          look=(-1, 18, 2), neck_share=0.68,
          fingers=chamber_thumb,
          rh=(0.47, 47.30, 0.4152), elbow_r=(-0.82, -0.34, -0.25), **sword(-21, 77, -70), two_hands=True,
          elbow_l=(0.88, -0.35, 0.10), shoulder_r=(0, 0, -7), shoulder_l=(0, -17, 9))
    m.key(17, "The hilt leads the tip through the center while the lead heel reaches for the ground.",
          ease='Linear', root=(0, 0, 0.85), hips=(0.034, -0.055, 0.026),
          body=dict(Hips=(7, 0, -2), Spine=(6, 6, -3), Chest=(13, 7, -5), UpperChest=(9, 4, -3)),
          L=Foot(0.195, 1.35, yaw=7, pitch=-10, lift=0.15),
          R=Foot(-0.145, 0.58, yaw=-20, pitch=47, lift=0.13),
          look=(0, 9, 2), neck_share=0.56,
          fingers=dict(LeftThumbProximal=(3.33, -49.33, -0.67), LeftThumbDistal=(0, -30, 0)),
          rh=(9.13, 22.32, 0.4069), elbow_r=(-0.8, -0.4, -0.4), **sword(2, 54, -60), two_hands=True,
          elbow_l=(0.74, -0.35, -0.44), shoulder_r=(0, 0, -3), shoulder_l=(0, -7, 8))
    m.key(19, "The front shoe takes the chop. The rear ball finds the floor before the heel settles.",
          ease='Linear', root=(0, 0, 1.3), hips=(0.086, -0.221, 0.068),
          body=dict(Hips=(7, 9, -4), Spine=(8, 4, -5), Chest=(18, 6, -6), UpperChest=(10, 3, -3)),
          L=front_heel, R=Foot(rear_plant.x, rear_plant.z, yaw=rear_plant.yaw, pitch=26, pivot='ball'),
          look=(2, -5),
          fingers=closed_thumb,
          rh=(16, -36, 0.421), elbow_r=(-0.72, -0.57, -0.3), **sword(20, -16, -50), two_hands=True,
          elbow_l=(0.74, -0.55, -0.29), shoulder_r=(0, 0, 0), shoulder_l=(0, 0, 0))
    m.key(21, "The front sole rolls flat and the knee yields. The rib cage keeps traveling after the hips brake.",
          ease='Out', root=(0, 0, 1.352), hips=(0.113, -0.278, 0.088), L=front_plant,
          R=Foot(rear_plant.x, rear_plant.z, yaw=rear_plant.yaw, pitch=8, pivot='ball'),
          body=dict(Hips=(8, 11, -6), Spine=(10, 6, -5), Chest=(22, 7, -6), UpperChest=(11, 4, -3)),
          look=(2, -6), shoulder_r=(0, 5, 2), shoulder_l=(0, -5, -2),
          rh=(23, -43, 0.413), elbow_r=(-0.68, -0.6, -0.34), **sword(26, -23, -43), two_hands=True,
          elbow_l=(0.73, -0.55, -0.3))
    m.key(23, "The back heel touches. The blade finishes down and to the outside as the eyes find the target.",
          ease='Linear', root=(0, 0, 1.36), hips=(0.110, -0.299, 0.081), R=rear_plant,
          body=dict(Hips=(8, 11, -7), Spine=(11, 7, -6), Chest=(23, 11, -7), UpperChest=(11, 6, -4)),
          look=(3, -4), shoulder_r=(0, 5, 3), shoulder_l=(0, -5, -3),
          rh=(32, -45, 0.405), elbow_r=(-0.65, -0.62, -0.36), **sword(42, -26, -38), two_hands=True,
          elbow_l=(0.72, -0.56, -0.33))
    m.key(26, "The pelvis rises on the lead leg while the ribs are still over the low sword.",
          ease='Linear', hips=(0.125, -0.255, 0.080), R=Foot(rear_plant.x, rear_plant.z, yaw=rear_plant.yaw, pitch=22, pivot='ball'),
          body=dict(Hips=(6, 8, -5), Spine=(8, 5, -4), Chest=(15, 5, -5), UpperChest=(8, 2, -1)),
          look=(2, -1), shoulder_r=(0, 6, 8), shoulder_l=(0, -5, -3),
          fingers=dict(LeftThumbProximal=(12, -26, -26), LeftThumbDistal=(0, -30, 0)),
          rh=(29.44, -61.25, 0.3826), elbow_r=(-0.67, -0.64, -0.24), **sword(41, -24, -40), two_hands=True,
          elbow_l=(0.73, -0.55, -0.31))
    m.key(30, "The sternum comes up as the rear foot passes. The free palm peels up as the sword stays low.",
          ease='Linear', root=(0, 0, 1.387), hips=(0.124, -0.207, 0.063),
          R=Foot(-0.12, 1.17, yaw=-17, pitch=28, lift=0.096),
          body=dict(Hips=(6, 0, -5), Spine=(4, 1, -1), Chest=(6, 3, 0), UpperChest=(3, 1, 1)),
          shoulder_r=(0, 7, 6), shoulder_l=(0, -4, 1),
          look=(1, 2), fingers=LEFT_OPEN,
          rh=(22.89, -50.25, 0.3804), elbow_r=(-0.66, -0.67, -0.21), **sword(35, -14, -44),
          lh=(-17, -43, 0.23), elbow_l=(0.72, -0.56, -0.26), lhand=turn(-15, 0, 2))
    m.key(35, "The right sole catches. The sternum comes upright while the point still trails the rising hilt.",
          ease='Out', root=(0, 0, 1.4), hips=(0.063, -0.149, 0.021), R=rear_reset, shoulder_r=(0, 1, 0), shoulder_l=(0, 0, 0),
          body=dict(Hips=(5, -13, -1), Spine=(4, 3, 0), Chest=(5, 5, -1), UpperChest=(2, 3, 0)),
          look=(0, 2),
          rh=(0, -52, 0.338), elbow_r=(-0.62, -0.73, -0.26), **sword(10, 4, -52),
          lh=(22, -48, 0.33), elbow_l=(0.68, -0.68, -0.18), lhand=QI)
    m.key(43, "The sword point settles after the knees and sternum come back over the two feet.",
          ease='Out', hips=(0.01, -0.053, -0.026),
          body=dict(Hips=(3, -16, 0), Spine=(3, 4, 0), Chest=(1, 5, -1), UpperChest=(0, 3, 0)),
          look=(0, 1),
          rh=(-16, -50, 0.34), elbow_r=(-0.62, -0.75, -0.25), **sword(-27, 41, -57),
          lh=(31, -52, 0.322), elbow_l=(0.68, -0.68, -0.18))
    finish(m, 52, "Ready again.", 1.4)
    return m


# The backstep can branch while the rear shoe is loaded. The dash starts on that shoe
# so its acceleration continues through the branch without a trip through idle guard.
DODGE_BRAKE_ROOT = -2.257
DODGE_BRAKE_POSE = dict(
    hips=(-0.094, -0.241, -0.054),
    body=dict(Hips=(8, -18, 4), Spine=(9, 7, -1), Chest=(17, 7, -2), UpperChest=(8, 4, -1)),
    look=(0, 6), rh=(-54, -56, 0.362), elbow_r=(-0.71, -0.61, -0.24), **sword(-76, -15, -66),
    lh=(28, -30, 0.355), elbow_l=(0.72, -0.63, -0.19),
)


def dodge_brake_feet(root_z):
    return dict(L=Foot(0.176, root_z + 0.192, yaw=6, pitch=-8, lift=0.055),
                R=Foot(-0.091, root_z - 0.239, yaw=-15, pivot='ball', planted=False))


def dash_attack():
    m = Move('dash_attack', 34, """The dash attack (M2 O3): out of a dodge's end or from ready, thrown forward low with the blade
trailing, and cut across on the way past. Hit on ticks 5 to 9. Warp bends it toward a target.

""" + HEADER)
    handoff = dodge_brake_feet(0.0)
    m.key(0, "Weight on the rear shoe at the end of the backstep.", fingers=HANDS, **handoff, **DODGE_BRAKE_POSE)
    front, rear = idle_feet(2.64).values()
    brake = handoff['R']
    m.key(1, "Load the rear shoe into the reversal as the blade starts to trail.",
          ease='Linear', root=(0, 0, 0.09), hips=(-0.077, -0.217, -0.001),
          L=Foot(0.173, 0.269, yaw=6, pitch=7, lift=0.066),
          R=Foot(brake.x, brake.z, yaw=brake.yaw, pitch=17, pivot='ball'),
          body=dict(Hips=(8, -20, 3), Spine=(10, 3, -1), Chest=(18, 1, -2), UpperChest=(8, 0, -1)),
          look=(0, 5), shoulder_r=(0, -5, 2), rh=(-61, -55, 0.358), **sword(-87, -5, -59),
          lh=(23, -34, 0.349), elbow_l=(0.72, -0.63, -0.19))
    m.key(2, "The right heel peels from the floor as the left knee rises.",
          ease='Linear', root=(0, 0, 0.235), hips=(-0.047, -0.172, 0.018),
          L=Foot(0.175, 0.52, yaw=7, pitch=28, lift=0.105),
          R=Foot(brake.x, brake.z, yaw=brake.yaw, pitch=47, pivot='ball'),
          body=dict(Hips=(7, -22, 2), Spine=(8, -5, -1), Chest=(14, -7, -3), UpperChest=(7, -4, -2)),
          look=(0, 4), shoulder_r=(0, -9, 2), rh=(-73, -45, 0.367), **sword(-94, 14, -50), lh=(20, -24, 0.348))
    m.key(3, "Push off the rear ball and drive the left knee forward; the sword trails outside the hip.",
          ease='Linear', root=(0, 0, 0.42), hips=(-0.022, -0.137, 0.061),
          L=Foot(0.177, 0.82, yaw=7, pitch=21, lift=0.163),
          R=Foot(-0.086, 0.012, yaw=-15, pitch=43, lift=0.035),
          body=dict(Hips=(8, -25, 3), Spine=(12, -5, -2), Chest=(19, -7, -5), UpperChest=(9, -4, -3)),
          shoulder_r=(0, -11, 2), shoulder_l=(0, -7, 0),
          look=(-1, 5),
          rh=(-97, -30, 0.376), elbow_r=(-0.76, -0.56, -0.3), **sword(-126, 5, -60),
          lh=(23, -20, 0.358), elbow_l=(0.72, -0.59, -0.22))
    m.key(5, "The front knee leads the flight. Keep the right elbow soft and the tip trailing.",
          ease='Linear', root=(0, 0, 1.0), hips=(0.006, -0.076, 0.064),
          L=Foot(0.181, 1.42, yaw=7, pitch=-1, lift=0.22),
          R=Foot(-0.097, 0.48, yaw=-14, pitch=51, lift=0.13),
          body=dict(Hips=(7, -25, 4), Spine=(11, -7, -4), Chest=(17, -8, -5), UpperChest=(8, -4, -4)),
          look=(-3, 6), shoulder_r=(0, -8, 2), shoulder_l=(0, -8, 0),
          rh=(-119, -19, 0.393), elbow_r=(-0.84, -0.37, -0.42), **sword(-160, 7, -80),
          lh=(34, -17, 0.375), elbow_l=(0.73, -0.52, -0.28))
    m.key(6, "The hips open before the sword arm. The left elbow folds out of the blade's path.",
          ease='Linear', root=(0, 0, 1.34), hips=(0.019, -0.092, 0.055),
          L=Foot(0.184, 1.77, yaw=8, pitch=-10, lift=0.172),
          R=Foot(-0.11, 0.83, yaw=-14, pitch=51, lift=0.145),
          body=dict(Hips=(8, -16, 2), Spine=(10, -1, -3), Chest=(16, -4, -4), UpperChest=(8, -2, -2)),
          look=(-2, 5), shoulder_r=(0, -6, 0), shoulder_l=(0, -4, 0),
          rh=(-99, -12, 0.417), elbow_r=(-0.8, -0.46, -0.41), **sword(-116, 7, -70),
          lh=(44, -20, 0.371), elbow_l=(0.75, -0.50, -0.29))
    m.key(7, "The chest brings the blade around as the left heel reaches.",
          ease='Linear', root=(0, 0, 1.72), hips=(0.023, -0.134, 0.056),
          L=Foot(0.185, 2.14, yaw=8, pitch=-15, lift=0.118),
          R=Foot(-0.11, 1.23, yaw=-14, pitch=53, lift=0.13),
          body=dict(Hips=(8, -6, 0), Spine=(10, 4, -2), Chest=(17, 4, -3), UpperChest=(8, 3, -1)),
          look=(-2, 4), shoulder_r=(0, 0, -2), shoulder_l=(0, 1, -1),
          rh=(-64, -6, 0.423), elbow_r=(-0.8, -0.47, -0.4), **sword(-62, 4, -100),
          lh=(65, -27, 0.359), elbow_l=(0.7, -0.48, -0.4))
    m.key(8, "The blade crosses the center as the lead leg starts to take the ground.",
          ease='Linear', root=(0, 0, 1.96), hips=(0.029, -0.171, 0.053),
          L=Foot(0.18, 2.39, yaw=8, pitch=-12, lift=0.083),
          R=Foot(-0.11, 1.51, yaw=-14, pitch=51, lift=0.102),
          body=dict(Hips=(8, 7, -1), Spine=(10, 8, -3), Chest=(17, 8, -3), UpperChest=(8, 4, -1)),
          look=(0, 2), shoulder_r=(0, 7, -3), shoulder_l=(0, 5, -2),
          rh=(-18, -10, 0.425), elbow_r=(-0.72, -0.58, -0.4), **sword(-14, 0, -120),
          lh=(92, -32, 0.365), elbow_l=(0.7, -0.42, -0.5))
    m.key(9, "Past the center. The lead heel contacts first with the rear leg coming under the hips.",
          ease='Linear', root=(0, 0, 2.17), hips=(0.047, -0.202, 0.045),
          L=Foot(0.154, 2.44, yaw=9, pitch=-12, pivot='heel', planted=False),
          R=Foot(-0.112, 1.77, yaw=-15, pitch=48, lift=0.079),
          body=dict(Hips=(8, 18, -2), Spine=(10, 11, -3), Chest=(16, 11, -3), UpperChest=(8, 5, -1)),
          look=(3, 1), shoulder_r=(0, 11, 0), shoulder_l=(0, 7, -2),
          rh=(22, -14, 0.423), elbow_r=(-0.46, -0.68, -0.47), **sword(40, -4, -120),
          lh=(111, -36, 0.359), elbow_l=(0.7, -0.4, -0.51))
    m.key(11, "The lead shoe rolls down through a short skid. The trailing ball catches the floor.",
          ease='Linear', root=(0, 0, 2.45), hips=(0.064, -0.241, 0.065),
          L=Foot(0.17, 2.755, yaw=8, pivot='ball', planted=False),
          R=Foot(-0.12, 2.10, yaw=-17, pitch=37, pivot='ball'),
          body=dict(Hips=(8, 25, -4), Spine=(10, 13, -4), Chest=(15, 14, -5), UpperChest=(8, 6, -3)),
          look=(7, -2), shoulder_r=(0, 13, 2), shoulder_l=(0, 7, -1),
          rh=(62, -20, 0.421), elbow_r=(-0.22, -0.78, -0.54), **sword(108, -9, -120),
          lh=(120, -38, 0.354), elbow_l=(0.71, -0.36, -0.5))
    rear_brace = Foot(-0.12, 2.25, yaw=-17, pivot='ball')
    m.key(14, "The rear heel touches. The lead knee compresses before the blade finishes its low sweep.",
          ease='Out', root=(0, 0, 2.6), hips=(0.078, -0.28, 0.071), L=front, R=rear_brace,
          body=dict(Hips=(8, 26, -6), Spine=(10, 13, -5), Chest=(19, 15, -6), UpperChest=(9, 7, -3)),
          look=(7, -3), shoulder_r=(0, 9, 3), shoulder_l=(0, 7, -2),
          rh=(73, -29, 0.397), elbow_r=(-0.24, -0.77, -0.55), **sword(131, -23, -133),
          lh=(117, -45, 0.346), elbow_l=(0.71, -0.37, -0.47))
    m.key(18, "The lead hip moves over the shoe. The ribs unwind before the sword shoulder releases.",
          ease='Linear', root=(0, 0, 2.635), hips=(0.120, -0.228, 0.058),
          R=Foot(rear_brace.x, rear_brace.z, yaw=rear_brace.yaw, pitch=37, pivot='ball'),
          body=dict(Hips=(7, 15, -6), Spine=(7, 10, -4), Chest=(10, 12, -4), UpperChest=(5, 6, -2)),
          look=(2, 1), shoulder_r=(0, 7, 3), shoulder_l=(0, 3, -1),
          rh=(59, -37, 0.366), elbow_r=(-0.44, -0.75, -0.37), **sword(111, -18, -139),
          lh=(93, -45, 0.346), elbow_l=(0.71, -0.44, -0.4))
    m.key(22, "The trailing foot passes under the pelvis. The elbow folds but the blade stays outside the thighs.",
          ease='Linear', root=(0, 0, 2.64), hips=(0.108, -0.179, 0.042),
          R=Foot(-0.109, 2.42, yaw=-15, pitch=24, lift=0.067),
          body=dict(Hips=(6, 9, -4), Spine=(6, 7, -2), Chest=(8, 8, -2), UpperChest=(4, 4, -2)),
          look=(2, 2), shoulder_r=(0, 2, 0), shoulder_l=(0, 0, 0),
          rh=(33, -46, 0.344), elbow_r=(-0.66, -0.71, 0.02), **sword(71, -4, -131),
          lh=(55, -45, 0.33), elbow_l=(0.7, -0.55, -0.32))
    m.key(27, "The rear step catches and the pelvis settles toward center before the blade point returns.",
          ease='Out', hips=(0.042, -0.079, -0.008), R=rear,
          body=dict(Hips=(4, -12, -2), Spine=(4, 5, -1), Chest=(4, 6, -1), UpperChest=(2, 3, 0)),
          look=(1, 2),
          rh=(-11, -52, 0.342), elbow_r=(-0.61, -0.75, -0.26), **sword(-3, 27, -83),
          lh=(32, -51, 0.327), elbow_l=(0.69, -0.64, -0.19))
    finish(m, 34, "Ready again.", 2.64)
    return m


def dodge():
    m = Move('dodge', 24, """The dodge (M2 O3): a low leap straight back, keeping its eyes on the enemy, 2.4 m in 14 ticks. The
front leg folds out of the way while the back foot reaches to brake. The left hand stays on
the target as the right arm pulls the blade out to the side, then both return to guard.

""" + HEADER)
    start(m, "Ready.")
    front, rear = idle_feet(-2.4).values()
    rear_contact = Foot(rear.x - 0.055, rear.z + 0.065, yaw=-21, pitch=22, pivot='ball', planted=False)
    m.key(1, "A quick dip loads the left shoe. The hilt folds in toward the right hip.",
          ease='Linear', root=(0, 0, -0.105), hips=(0.018, -0.098, 0.002),
          L=Foot(IDLE_L[0], IDLE_L[1], yaw=IDLE_L[2], pitch=9, pivot='ball'),
          R=Foot(-0.09, -0.28, yaw=-15, pitch=12, lift=0.035),
          body=dict(Hips=(6, -17, 2), Spine=(5, 4, -1), Chest=(8, 8, -2), UpperChest=(4, 4, -1)),
          look=(0, 2), rh=(-37, -48, 0.334), **sword(-47, 22, -60), lh=(25, -48, 0.326))
    m.key(2, "Drive off the left ball, sending the back leg toward the new support.",
          ease='Linear', root=(0, 0, -0.26), hips=(0.012, -0.118, 0.046),
          L=Foot(IDLE_L[0], IDLE_L[1], yaw=IDLE_L[2], pitch=29, pivot='ball'),
          R=Foot(-0.115, -0.55, yaw=-17, pitch=12, lift=0.107),
          body=dict(Hips=(7, -20, 4), Spine=(6, 5, -2), Chest=(10, 9, -3), UpperChest=(6, 4, -2)),
          look=(0, 3),
          rh=(-61, -48, 0.352), elbow_r=(-0.83, -0.46, -0.28), **sword(-70, 11, -65),
          lh=(16, -24, 0.348), elbow_l=(0.72, -0.65, -0.18))
    m.key(4, "The left foot releases. The knees split, and the blade stays close to the outside hip.",
          ease='Linear', root=(0, 0, -0.7), hips=(-0.005, -0.058, 0.058),
          L=Foot(0.185, -0.315, yaw=8, pitch=45, lift=0.093),
          R=Foot(-0.16, -1.12, yaw=-19, pitch=27, lift=0.18),
          body=dict(Hips=(2, -24, 6), Spine=(7, 7, -2), Chest=(12, 10, -4), UpperChest=(6, 5, -2)),
          look=(0, 5),
          rh=(-73, -50, 0.35), elbow_r=(-0.85, -0.40, -0.2), **sword(-94, -4, -75),
          lh=(9, -17, 0.34), elbow_l=(0.72, -0.66, -0.18))
    m.key(6, "Apex: the front knee collects as the back leg reaches and the torso stays over the line of travel.",
          ease='Linear', root=(0, 0, -1.2), hips=(-0.042, -0.054, 0.026),
          L=Foot(0.196, -0.97, yaw=8, pitch=29, lift=0.205),
          R=Foot(-0.19, -1.65, yaw=-22, pitch=19, lift=0.171),
          body=dict(Hips=(0, -24, 7), Spine=(7, 8, -2), Chest=(12, 9, -4), UpperChest=(6, 5, -2)),
          look=(0, 5),
          rh=(-73, -57, 0.37), elbow_r=(-0.81, -0.5, -0.22), **sword(-103, -15, -80),
          lh=(13, -16, 0.354), elbow_l=(0.73, -0.63, -0.19))
    m.key(8, "Spot the landing. The rear leg extends and the front knee starts to open.",
          ease='Linear', root=(0, 0, -1.63), hips=(-0.061, -0.091, -0.006),
          L=Foot(0.19, -1.34, yaw=7, pitch=13, lift=0.162),
          R=Foot(-0.178, -2.05, yaw=-22, pitch=19, lift=0.089),
          body=dict(Hips=(4, -22, 5), Spine=(7, 8, -1), Chest=(11, 9, -3), UpperChest=(5, 4, -2)),
          look=(1, 5),
          rh=(-66, -57, 0.365), elbow_r=(-0.77, -0.54, -0.22), **sword(-98, -22, -80),
          lh=(17, -20, 0.359), elbow_l=(0.73, -0.62, -0.19))
    m.key(9, "The right ball touches just before the brake. The chest remains ahead of the retreating hips.",
          ease='Linear', root=(0, 0, -1.825), hips=(-0.078, -0.142, -0.046),
          L=Foot(0.187, -1.54, yaw=7, pitch=10, lift=0.121),
          R=Foot(-0.152, -2.29, yaw=-21, pitch=29, pivot='ball', planted=False),
          body=dict(Hips=(5, -21, 5), Spine=(9, 8, -2), Chest=(14, 8, -4), UpperChest=(7, 4, -3)),
          look=(0, 6),
          rh=(-62, -58, 0.364), elbow_r=(-0.74, -0.56, -0.23), **sword(-94, -22, -77),
          lh=(21, -23, 0.362), elbow_l=(0.73, -0.62, -0.19))
    m.key(10, "The hips keep moving into a short skid as the heel drops.",
          ease='Linear', root=(0, 0, -2.0), hips=(-0.091, -0.192, -0.079),
          L=Foot(0.185, -1.73, yaw=7, pitch=8, lift=0.087), R=rear_contact,
          body=dict(Hips=(7, -20, 4), Spine=(10, 8, -2), Chest=(16, 8, -3), UpperChest=(8, 4, -2)),
          look=(0, 6),
          rh=(-59, -58, 0.364), elbow_r=(-0.72, -0.59, -0.24), **sword(-89, -22, -75),
          lh=(23, -25, 0.365), elbow_l=(0.73, -0.62, -0.19))
    m.key(11, "The rear heel settles under the hips and the knee takes the brake.",
          ease='Linear', root=(0, 0, -2.137), hips=(-0.098, -0.225, -0.069),
          L=Foot(0.18, -1.905, yaw=6, pitch=-1, lift=0.078),
          R=Foot(-0.104, -2.481, yaw=-17, pivot='ball', planted=False),
          body=dict(Hips=(8, -19, 5), Spine=(10, 8, -1), Chest=(18, 8, -3), UpperChest=(9, 4, -2)),
          look=(0, 6),
          rh=(-56, -56, 0.362), elbow_r=(-0.71, -0.61, -0.24), **sword(-83, -18, -70),
          lh=(26, -29, 0.359), elbow_l=(0.72, -0.63, -0.19))
    m.key(12, "The rear knee stays loaded while the front shoe reaches forward.",
          ease='Linear', root=(0, 0, DODGE_BRAKE_ROOT),
          **dodge_brake_feet(DODGE_BRAKE_ROOT), **DODGE_BRAKE_POSE)
    m.key(13, "The left heel searches for its mark while the right knee returns from its compression.",
          ease='Linear', root=(0, 0, -2.335), hips=(-0.078, -0.223, -0.021), R=rear,
          L=Foot(front.x, front.z, yaw=front.yaw, pitch=-13, lift=0.043),
          body=dict(Hips=(8, -18, 3), Spine=(8, 7, -1), Chest=(14, 7, -2), UpperChest=(7, 3, -1)),
          look=(0, 7),
          rh=(-49, -55, 0.359), elbow_r=(-0.69, -0.63, -0.24), **sword(-68, -10, -60),
          lh=(31, -34, 0.351), elbow_l=(0.72, -0.64, -0.19))
    m.key(16, "Both feet take the rebound. The lead hand drops as the sword returns to the center.",
          ease='Out', root=(0, 0, -2.4), hips=(-0.041, -0.149, -0.022), L=front,
          body=dict(Hips=(6, -16, 2), Spine=(6, 6, -1), Chest=(9, 7, -1), UpperChest=(4, 3, -1)),
          look=(0, 3),
          rh=(-30, -53, 0.351), elbow_r=(-0.67, -0.67, -0.24), **sword(-43, 13, -50),
          lh=(35, -44, 0.335), elbow_l=(0.70, -0.66, -0.18))
    m.key(20, "The head and chest settle before the blade point.",
          ease='Out', hips=IDLE_HIPS, body=IDLE_BODY, look=(0, 1),
          rh=(-18, -50, 0.34), elbow_r=(-0.64, -0.73, -0.25), blade=(-27, 38),
          lh=(30, -51, 0.324), elbow_l=(0.68, -0.68, -0.18))
    finish(m, 24, "Ready again.", -2.4)
    return m


def skill():
    m = Move('skill', 52, """The skill (M2 O3): a coiled drop with the blade held low behind, then a whole turn with the arm out,
which cuts on ticks 12 to 15, and a rising cut coming out of the turn on ticks 21 to 25, off the
ground at its top. The first cut stays low; the second stretches upward over the left shoulder,
then the left leg takes the landing and the sword rolls back into guard.

""" + HEADER)
    start(m, "Ready.")
    pivot_x, pivot_z = 0.21, 0.56
    front, rear = idle_feet(1.08).values()
    slash_elbow = (-0.55, -0.75, 0.25)

    def spin_foot(yaw, pitch):
        return Foot(pivot_x, pivot_z, yaw=yaw, pitch=pitch, pivot='ball')

    m.key(2, "The hips turn into the loading shoe. The sword clears the right knee on its way back.",
          ease='Linear', root=(0, 0, 0.008), hips=(-0.03, -0.096, -0.039),
          body=dict(Hips=(5, -24, 2), Spine=(5, -6, 1), Chest=(3, -6, 1), UpperChest=(2, -2, 0)),
          L=Foot(0.18, 0.26, yaw=6, pitch=13, lift=0.056),
          look=(-3, 1), shoulder_r=(0, -4, 1), shoulder_l=(0, -5, 1),
          rh=(-61, -35, 0.369), elbow_r=slash_elbow, **sword(-82, 9, -30),
          lh=(17, -32, 0.339), elbow_l=(0.72, -0.62, -0.22))
    m.key(5, "Step out of the rear leg into the turn. The free elbow stays bent as the blade trails low.",
          ease='Linear', root=(0, 0, 0.036), hips=(-0.055, -0.201, -0.022),
          body=dict(Hips=(8, -38, 3), Spine=(7, -10, 2), Chest=(8, -12, 3), UpperChest=(5, -6, 2)),
          L=Foot(0.198, 0.431, yaw=8, pitch=-8, lift=0.104),
          R=Foot(IDLE_R[0], IDLE_R[1], yaw=-19, pitch=29, pivot='ball'),
          look=(-12, 0), shoulder_r=(0, -10, 3), shoulder_l=(0, -8, 2),
          rh=(-105, -39, 0.376), elbow_r=slash_elbow, **sword(-145, -10, 0),
          lh=(12, -19, 0.377), elbow_l=(0.72, -0.53, -0.28))
    m.key(6, "The back ball pushes and releases. The front heel reaches while the sword-side ribs lag.",
          ease='Linear', root=(0.003, 0, 0.064), hips=(-0.023, -0.182, 0.001),
          body=dict(Hips=(7, -39, 3), Spine=(7, -11, 2), Chest=(8, -13, 3), UpperChest=(5, -7, 2)),
          L=Foot(0.208, 0.482, yaw=8, pitch=-14, lift=0.086),
          R=Foot(-0.088, -0.104, yaw=-20, pitch=42, lift=0.047),
          look=(-14, 0), shoulder_r=(0, -10, 3), shoulder_l=(0, -8, 2),
          rh=(-117, -35, 0.381), elbow_r=slash_elbow, **sword(-157, -12, 4),
          lh=(10, -16, 0.384), elbow_l=(0.73, -0.53, -0.28))
    m.key(7, "A brief flight brings the hips onto the lead leg's landing line.",
          ease='Linear', root=(0.008, 0, 0.096), hips=(0.019, -0.179, 0.031),
          body=dict(Hips=(7, -41, 4), Spine=(8, -11, 2), Chest=(8, -13, 3), UpperChest=(5, -7, 2)),
          L=Foot(0.21, 0.529, yaw=8, pitch=-12, lift=0.059),
          R=Foot(-0.102, -0.075, yaw=-22, pitch=37, lift=0.083),
          look=(-14, 0), shoulder_r=(0, -10, 3), shoulder_l=(0, -8, 2),
          rh=(-126, -32, 0.385), elbow_r=slash_elbow, **sword(-162, -13, 7),
          lh=(9, -13, 0.391), elbow_l=(0.73, -0.53, -0.28))
    m.key(8, "The left ball is down. Weight shifts into the turn while the shoulders and sword lag.",
          ease='Linear', root=(0.014, 0, 0.125), hips=(0.064, -0.228, 0.035),
          body=dict(Hips=(8, -42, 3), Spine=(9, -11, 2), Chest=(9, -12, 2), UpperChest=(5, -7, 0)),
          L=spin_foot(8, 0), R=Foot(-0.119, -0.036, yaw=-24, pitch=34, lift=0.103),
          look=(-14, 0), shoulder_r=(0, -9, 3), shoulder_l=(0, -7, 2),
          rh=(-134, -31, 0.388), elbow_r=slash_elbow, blade=(-166, -13),
          lh=(9, -11, 0.395), elbow_l=(0.73, -0.52, -0.28))
    m.key(12, "The loaded left knee gathers under the pivot. The free hand and right knee draw in.",
          ease='Out', root=(0.031, 0, 0.15), hips=(0.091, -0.256, 0.054),
          body=dict(Hips=(8, -38, 2), Spine=(10, -11, 2), Chest=(10, -12, 2), UpperChest=(5, -6, 0)),
          L=spin_foot(12, 8), R=Foot(-0.161, -0.02, yaw=-25, pitch=33, lift=0.105),
          look=(-6, 0, 2), shoulder_r=(0, -10, 3), shoulder_l=(0, -5, 2),
          rh=(-129, -25, 0.391), elbow_r=slash_elbow, blade=(-169, -11),
          lh=(15, -19, 0.373), elbow_l=(0.75, -0.50, -0.29))
    m.key(13, "Lead with the pelvis. The right knee starts in before the arm opens into the slash.",
          ease='Linear', root=(0.06, 0, 0.25), root_turn=31, hips=(0.061, -0.235, 0.038),
          body=dict(Hips=(7, -26, 2), Spine=(9, -12, 2), Chest=(9, -13, 2), UpperChest=(4, -6, 0)),
          L=spin_foot(38, 12), R=Foot(-0.192, -0.23, yaw=-31, pitch=31, lift=0.047, model=True),
          look=(-7, 0), shoulder_r=(0, -7, 2), shoulder_l=(0, -2, 1),
          rh=(-97, -21, 0.375), elbow_r=slash_elbow, **sword(-144, -8, 10),
          lh=(29, -25, 0.365), elbow_l=(0.68, -0.54, -0.28))
    m.key(14, "The planted ball turns the belt. The collar banks toward the sword side as the elbow opens.",
          ease='Linear', root=(0.11, 0, 0.367), root_turn=66, hips=(0.033, -0.203, 0.039),
          body=dict(Hips=(6, -4, -4), Spine=(6.4, -6.4, 6.6), Chest=(3.1, -7.7, 7.5), UpperChest=(3.7, -4.8, 5.9)),
          L=spin_foot(72, 15), R=Foot(-0.246, -0.24, yaw=-23, pitch=34, lift=0.11, model=True),
          look=(-12, 0), shoulder_r=(0, 0, -2), shoulder_l=(0, 3, 0),
          rh=(-77.77, -8.97, 0.3849), elbow_r=slash_elbow, blade=(-91, 1), edge=(-0.004, 0.564, 0.826),
          lh=(62, -32, 0.351), elbow_l=(0.62, -0.55, -0.27))
    m.key(15, "The sword crosses the front while the collar is still banked behind the belt.",
          ease='Linear', root=(0.136, 0, 0.477), root_turn=106, hips=(0.006, -0.174, 0.038),
          body=dict(Hips=(5, 5, -5), Spine=(4.5, -4.4, 6.4), Chest=(4, -6.5, 9.3), UpperChest=(1.9, -4.5, 7.1)),
          L=spin_foot(112, 18), R=Foot(-0.265, -0.247, yaw=-20, pitch=31, lift=0.133, model=True),
          look=(-14, 0), shoulder_r=(0, 8, -2), shoulder_l=(0, 6, 1),
          rh=(-63.69, -3.78, 0.4067), elbow_r=slash_elbow, **sword(-75, 4, -20),
          lh=(75, -36, 0.342), elbow_l=(0.64, -0.55, -0.27))
    m.key(16, "The hip checks its rise over the planted ball. The free elbow stays folded.",
          ease='Linear', root=(0.148, 0, 0.524), root_turn=142, hips=(-0.01, -0.157, 0.039),
          body=dict(Hips=(4, 12, -6), Spine=(2.4, -4.3, 8.3), Chest=(3, -6.7, 9.1), UpperChest=(1.1, -3.7, 6.9)),
          L=spin_foot(148, 17), R=Foot(-0.257, -0.264, yaw=-19, pitch=30, lift=0.14, model=True),
          look=(-13, 1), shoulder_r=(0, 10, -1), shoulder_l=(0, 6, 1),
          rh=(-61.96, -4.28, 0.4000), elbow_r=slash_elbow, **sword(-71, 3, -20),
          lh=(82, -39, 0.343), elbow_l=(0.64, -0.55, -0.25))
    # Between the two hits, carry the ribs and sword behind the turning belt.
    # Let them catch up before the next active blade pose.
    m.key(17, "The supporting knee folds and turns the belt before the ribs and sword come around.",
          ease='Linear', root=(0.16, 0, 0.57), root_turn=178, hips=(-0.03, -0.166, 0.034),
          body=dict(Hips=(4, 10, -7), Spine=(1.2, -4.8, 10.2), Chest=(1.8, -4.3, 7), UpperChest=(-0.8, -3.6, 6)),
          L=spin_foot(182, 16), R=Foot(-0.247, -0.28, yaw=-18, pitch=28, lift=0.142, model=True),
          look=(-12, 1), shoulder_r=(0, 10, 0), shoulder_l=(0, 5, 1),
          rh=(-74.76, -4.81, 0.3943), elbow_r=slash_elbow, blade=(-82.02, 2.34), edge=(0.11673, -0.34008, 0.93312),
          lh=(73.64, -38.16, 0.3550), elbow_l=(0.65, -0.55, -0.23))
    m.key(18, "The hip gathers back over the ball as the trailing knee draws in and the blade coasts.",
          ease='Linear', root=(0.156, 0, 0.597), root_turn=209, hips=(-0.052, -0.190, 0.028),
          body=dict(Hips=(6, 0, -8), Spine=(1.7, 2.6, 8.4), Chest=(0.6, 0.5, 5.8), UpperChest=(-0.9, -0.5, 4.3)),
          L=spin_foot(214, 15), R=Foot(-0.23, -0.282, yaw=-17, pitch=27, lift=0.138, model=True),
          look=(-10, 1), shoulder_r=(0, 8, 0), shoulder_l=(0, 4, 0),
          rh=(-83.01, -7.32, 0.3869), elbow_r=slash_elbow, blade=(-87.02, -1.24), edge=(0.05633, -0.34614, 0.93649),
          lh=(72.26, -35.41, 0.3572), elbow_l=(0.65, -0.55, -0.23))
    m.key(19, "The hip reaches its low point over the pivot while the ribs carry the first sweep through.",
          ease='Linear', root=(0.148, 0, 0.62), root_turn=240, hips=(-0.073, -0.213, 0.006),
          body=dict(Hips=(8, -8, -9), Spine=(3.7, 5.4, 7.5), Chest=(3.4, 9.3, 1.1), UpperChest=(0.8, 6.7, -0.7)),
          L=spin_foot(246, 13), R=Foot(-0.216, -0.276, yaw=-16, pitch=26, lift=0.127, model=True),
          look=(-7, 0), shoulder_r=(0, 5, 1), shoulder_l=(0, 3, 0),
          rh=(-83.07, -14.78, 0.3693), elbow_r=slash_elbow, blade=(-81.97, -10.96), edge=(0.19683, -0.34388, 0.91815),
          lh=(78.41, -34.21, 0.3461), elbow_l=(0.65, -0.54, -0.23))
    m.key(20, "The hip starts away from the pivot as the ribs and free shoulder finish the sweep.",
          ease='Linear', root=(0.131, 0, 0.65), root_turn=270, hips=(-0.056, -0.209, -0.025),
          body=dict(Hips=(10, -11, -6), Spine=(4.8, 6.3, 3.9), Chest=(4.6, 9.3, -4.9), UpperChest=(3.1, 5.7, -4.6)),
          L=spin_foot(276, 15), R=Foot(-0.208, -0.28, yaw=-17, pitch=22, lift=0.095, model=True),
          look=(-5, 0), shoulder_r=(0, 2, 2), shoulder_l=(0, 2, 0),
          rh=(-68.23, -30.71, 0.3982), elbow_r=(-0.61, -0.89, 0.12), blade=(-62.92, -21.70), edge=(0.56177, -0.55926, 0.60963),
          lh=(74.88, -36.03, 0.3201), elbow_l=(0.65, -0.54, -0.22))
    m.key(21, "The planted leg drives the belt up while the ribs stay turned and the blade stays low.",
          ease='Linear', root=(0.112, 0, 0.682), root_turn=300, hips=(-0.017, -0.188, -0.036),
          body=dict(Hips=(9, -9, 1), Spine=(5.7, 3.6, -2.6), Chest=(5.6, 5.7, 0.3), UpperChest=(4.8, 5.7, 1)),
          L=spin_foot(307, 17), R=Foot(-0.206, -0.29, yaw=-18, pitch=18, lift=0.07, model=True),
          look=(-4, 0), shoulder_r=(0, -2, 3), shoulder_l=(0, -2, 0),
          rh=(-50.76, -47.42, 0.4322), elbow_r=(-0.46, -0.84, 0.43), **sword(-42, -32, -60),
          lh=(67, -37, 0.327), elbow_l=(0.66, -0.53, -0.2))
    m.key(22, "The pivot releases. The ribs and sword follow the rising hip.",
          ease='Linear', root=(0.073, 0, 0.748), root_turn=322, hips=(0.005, -0.150, -0.011),
          body=dict(Hips=(4, 3, 1), Spine=(3.1, 1.9, -1.1), Chest=(3.4, 1.5, 0.8), UpperChest=(3.5, 3.7, 3.4)),
          L=Foot(0.185, 0.72, yaw=328, pitch=32, lift=0.009),
          R=Foot(-0.195, -0.273, yaw=-15, pitch=28, lift=0.106, model=True),
          look=(-3, 2), shoulder_r=(0, -1, 0), shoulder_l=(0, -3, 0),
          rh=(-27.68, -24.15, 0.4311), elbow_r=slash_elbow, **sword(-27, 0, -50),
          lh=(69, -34, 0.349), elbow_l=(0.66, -0.53, -0.2))
    m.key(23, "The belt slows as the lifted ribs and sword finish turning into the jump.",
          ease='Linear', root=(0.04, 0, 0.82), root_turn=340, hips=(0.018, -0.1, -0.002),
          body=dict(Hips=(2, 8, 0), Spine=(-2, 6, 0), Chest=(-4, 6, 0), UpperChest=(-4, 2, 0)),
          L=Foot(0.17, 0.95, yaw=347, pitch=21, lift=0.115),
          R=Foot(-0.192, -0.28, yaw=-13, pitch=34, lift=0.128, model=True),
          look=(-1, 6),
          rh=(-12, 13, 0.423), elbow_r=slash_elbow, **sword(-8, 41, -50),
          lh=(78, -36, 0.349), elbow_l=(0.67, -0.54, -0.22), shoulder_r=(0, 0, -3))
    m.key(25, "The pelvis begins to brake while the ribs and high hand finish the cut.",
          ease='Linear', root=(0.012, 0, 0.984), root_turn=356, hips=(0.032, 0.033, 0.011),
          body=dict(Hips=(-2, 7, -2), Spine=(-6, 12, -2), Chest=(-7, 13, -3), UpperChest=(-5, 5, -1)),
          L=Foot(0.157, 1.182, yaw=358, pitch=4, lift=0.241),
          R=Foot(-0.178, -0.32, yaw=-10, pitch=49, lift=0.214, model=True),
          look=(0, 10),
          rh=(22.68, 69.66, 0.3939), elbow_r=(-0.8, -0.3, 0.23), **sword(31, 73, -160),
          lh=(92, -41, 0.34), elbow_l=(0.69, -0.52, -0.23), shoulder_r=(0, 0, -11))
    m.key(28, "The hips line up the heel while the ribs and sword shoulder carry the turn a little farther.",
          ease='Linear', root=(0, 0, 1.056), root_turn=360, hips=(0.039, -0.028, 0.028),
          body=dict(Hips=(6, -2, -4), Spine=(-4, 12, -1), Chest=(-2, 16, -1), UpperChest=(-2, 6, 0)),
          L=Foot(0.174, 1.237, yaw=366, pitch=-9, lift=0.138),
          R=Foot(-0.16, 0.845, yaw=346, pitch=41, lift=0.183),
          look=(1, 6),
          rh=(30.53, 56.05, 0.3774), elbow_r=(-0.75, -0.48, -0.2), **sword(41, 74, -160),
          lh=(87, -41, 0.337), elbow_l=(0.69, -0.52, -0.23), shoulder_r=(0, 0, -8))
    m.key(31, "The left heel and pelvis take the drop. The sternum and hilt hang above the catch for a beat.",
          ease='In', root=(0, 0, 1.08), hips=(0.091, -0.181, 0.06), L=heel_contact(front, -9),
          R=Foot(-0.11, 0.928, yaw=347, pitch=33, lift=0.075),
          body=dict(Hips=(8, 0, -6), Spine=(4, 6, -2), Chest=(5, 9, -3), UpperChest=(3, 5, -2)),
          look=(2, 1),
          rh=(7, 34, 0.355), elbow_r=(-0.69, -0.24, -0.49), **sword(83, 52, -136),
          lh=(70, -37, 0.335), elbow_l=(0.70, -0.55, -0.22), shoulder_r=(0, 5, -5))
    m.key(33, "The sole settles and the ribs pass over it. The sword shoulder is last to come down.",
          ease='Linear', hips=(0.109, -0.223, 0.074), L=front,
          R=Foot(-0.095, 0.952, yaw=348, pitch=22, lift=0.058),
          body=dict(Hips=(8, -3, -6), Spine=(9, 3, -4), Chest=(14, 5, -4), UpperChest=(7, 3, -2)),
          look=(1, -1), shoulder_r=(0, 5, -3), shoulder_l=(0, -4, -1),
          rh=(3, 17, 0.346), elbow_r=(-0.69, -0.34, -0.45), **sword(62, 36, -114),
          lh=(53, -40, 0.328), elbow_l=(0.69, -0.57, -0.22))
    m.key(36, "The rear sole catches. The hilt lowers before the point curls back to guard.",
          ease='Linear', hips=(0.072, -0.168, 0.034), R=rear,
          body=dict(Hips=(7, -7, -2), Spine=(6, 3, -1), Chest=(6, 2, -1), UpperChest=(3, 1, 0)),
          look=(1, 1),
          rh=(-6, -28, 0.333), elbow_r=(-0.65, -0.59, -0.34), **sword(23, 16, -77),
          lh=(49, -46, 0.329), elbow_l=(0.68, -0.61, -0.21), shoulder_r=(0, 0, 0))
    m.key(43, "The chest rises over both feet while the point settles back toward the opponent.",
          ease='Out', hips=(0.016, -0.064, -0.027),
          body=dict(Hips=(4, -16, 0), Spine=(4, 4, 0), Chest=(3, 5, -1), UpperChest=(1, 3, 0)),
          look=(0, 1),
          rh=(-15, -47, 0.344), elbow_r=(-0.63, -0.73, -0.26), **sword(-27, 31, -50),
          lh=(29, -51, 0.323), elbow_l=(0.68, -0.66, -0.18))
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
    allowed = {'--write', '--write-numbers', '--force', '-v'}
    names = {make.__name__ for make in MOVES}
    unknown = [arg for arg in args if arg not in allowed and arg not in names]
    if unknown:
        sys.exit(f'unknown arguments: {", ".join(unknown)}\n{__doc__}')
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
            # Keep the text-mode line endings these source files used before no-op writes.
            document = move.ron()
            if name == 'light_3':
                document = add_sleeve_keys(move, document)
            frames.write_changed(os.path.join(folder, name + '.keypose.ron'), document.replace('\n', os.linesep))
            frames.write_changed(os.path.join(folder, name + '.keypose.ron.meta'),
                                 (SIDECAR % frames.IDS[name]).replace('\n', os.linesep))
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
