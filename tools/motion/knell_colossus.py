"""Pose the boss's clips (M3 C2). Run with --write to update its key-pose documents.

First drafts nobody has judged. Its advance is KayKit's walk, retargeted by the pack's sidecar;
these are the rest: waking, its guard, its turns, its attacks, its phase change and its falls. Each
attack's wind-up, active and recovery are the move's frame data to come (part 2's moves document),
so a clip's length is the move's: slam 44 + 6 + 50, sweep 36 + 10 + 40, stomp 30 + 4 + 36,
charge 28 + 18 + 40, leap 40 + 6 + 60 ticks.

Its staff is skinned to `handslot.r`, the bell at the far end, about 1.8 m out: `grip` puts the slot,
`aim` points the staff from it toward the bell, and `hold` puts the left hand that far up the shaft.
"""
import sys
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

from figure_pose import Clip, main, yaw  # noqa: E402
from pose import norm  # noqa: E402

RIG_ID = '49e92e56-e2b1-4cb0-93b9-6c7aef439350'
FIGURE = 'knell_colossus'

LEFT, RIGHT = (0.42, 0.14), (-0.42, -0.22)

# Its guard: feet wide, a little sunk, the staff upright in the right hand with the bell above its
# head, the left hand open at its side.
GUARD = dict(
    hips=(0.0, -0.1, 0.0), pelvis=(0.0, -8.0, 0.0), spine=(3.0, 0.0, 0.0), chest=(4.0, 4.0, 0.0),
    head=(-4.0, 4.0, 0.0),
    left_foot=(LEFT[0], LEFT[1], 0.0, 8.0, 0.0), right_foot=(RIGHT[0], RIGHT[1], 0.0, -14.0, 0.0),
    knees=(0.15, 0.15),
    grip=(-0.62, 1.62, 0.5), aim=norm((-0.06, 1.0, 0.16)), roll=(0.0, 0.0, -1.0), elbow=(-0.6, -0.8, -0.2),
    hold=None, reach=(0.88, 1.62, 0.3), elbow_l=(0.6, -0.8, -0.3),
)

# Both hands on the staff, raised over the right shoulder, the bell behind and above: every
# overhead blow's wind-up.
RAISED = dict(
    hips=(0.0, -0.16, -0.06), pelvis=(-4.0, -24.0, 0.0), spine=(-8.0, -6.0, 0.0), chest=(-10.0, -8.0, 0.0),
    head=(-8.0, 28.0, 0.0),
    grip=(-0.3, 2.62, 0.2), aim=norm((-0.15, 0.55, -0.82)), roll=(0.0, 0.8, 0.6), elbow=(-0.8, 0.2, -0.5),
    hold=0.3, reach=None, elbow_l=(0.8, 0.2, -0.4),
)

# The blow landed: bent over, the bell on the ground in front of it.
LANDED = dict(
    hips=(0.0, -0.42, 0.18), pelvis=(16.0, -6.0, 0.0), spine=(14.0, 0.0, 0.0), chest=(12.0, 0.0, 0.0),
    head=(-22.0, 6.0, 0.0),
    grip=(-0.08, 1.5, 0.78), aim=norm((0.0, -0.65, 0.76)), roll=(0.0, 0.76, 0.65), elbow=(-0.8, -0.6, 0.0),
    hold=0.18, elbow_l=(0.8, -0.6, 0.0),
)

# Kneeling on its right knee, the staff planted beside it and the head bowed: how it waits, and how
# it falls.
KNEELING = dict(
    hips=(0.0, -0.66, -0.2), pelvis=(8.0, -4.0, 0.0), spine=(10.0, 0.0, 0.0), chest=(10.0, 0.0, 0.0),
    head=(30.0, 0.0, 0.0),
    left_foot=(0.38, 0.5, 0.0, 6.0, 0.0), right_foot=(-0.36, -0.9, 0.28, -6.0, 60.0), knees=(0.1, 0.0),
    grip=(-0.7, 1.1, 0.5), aim=norm((0.0, 1.0, 0.05)), roll=(0.0, 0.0, -1.0), elbow=(-0.8, -0.6, 0.0),
    hold=None, reach=(0.5, 1.05, 0.55), elbow_l=(0.6, -0.4, 0.4),
)


def at(tick, *bases, **fields):
    key = {}
    for base in bases:
        key.update(base)
    key.update(fields)
    key['tick'] = tick
    return key


def feet(turn, forward=0.0):
    """Both guard feet turned with the root and moved along with it."""
    out = {}
    for field, (x, z), foot_yaw in (('left_foot', LEFT, 8.0), ('right_foot', RIGHT, -14.0)):
        place = yaw((x, 0.0, z + forward), turn)
        out[field] = (place[0], place[2], 0.0, foot_yaw + turn, 0.0)
    return out


def foot(field, turn, forward=0.0, lift=0.0, pitch=0.0):
    (x, z), foot_yaw = (LEFT, 8.0) if field == 'left_foot' else (RIGHT, -14.0)
    place = yaw((x, 0.0, z + forward), turn)
    return {field: (place[0], place[2], lift, foot_yaw + turn, pitch)}


def awaken():
    return Clip('awaken', 'Kneeling still as a statue, it lifts its head, stands, rings the bell overhead and takes its guard.', 150, [
        at(0, KNEELING),
        at(24, KNEELING, ease='Smooth', head=(4.0, 0.0, 0.0)),
        at(60, GUARD, ease='Smooth', hips=(0.0, -0.3, -0.04), pelvis=(4.0, -6.0, 0.0), right_foot=(-0.42, -0.4, 0.0, -14.0, 0.0),
           left_foot=(LEFT[0], 0.3, 0.0, 8.0, 0.0), grip=(-0.66, 1.42, 0.5)),
        at(84, GUARD, left_foot=(LEFT[0], 0.3, 0.0, 8.0, 0.0), right_foot=(-0.42, -0.4, 0.0, -14.0, 0.0)),
        at(100, GUARD, ease='Out', left_foot=(LEFT[0], 0.3, 0.0, 8.0, 0.0), right_foot=(-0.42, -0.4, 0.0, -14.0, 0.0),
           hips=(0.0, -0.07, -0.04), spine=(-6.0, 0.0, 0.0), chest=(-8.0, 0.0, 0.0), head=(-18.0, 0.0, 0.0),
           grip=(-0.52, 2.5, 0.3), aim=norm((-0.05, 1.0, 0.0)), reach=(1.2, 2.2, 0.3), elbow_l=(0.6, -0.4, -0.6)),
        at(108, ease='Out', dense=4, grip=(-0.5, 2.56, 0.34), aim=norm((0.15, 1.0, 0.1))),
        at(116, aim=norm((-0.2, 1.0, 0.0))),
        at(150, GUARD, ease='Smooth'),
    ])


def guard():
    """It loops: the first key is the last."""
    return Clip('guard', 'Its guard, breathing: the chest rises and the bell sways a little.', 120, [
        at(0, GUARD),
        at(60, GUARD, hips=(0.0, -0.08, 0.0), chest=(1.0, 5.0, 0.0), head=(-6.0, 6.0, 0.0),
           grip=(-0.62, 1.66, 0.48), aim=norm((-0.1, 1.0, 0.13))),
        at(120, GUARD),
    ])


def turning(name, sign):
    side = 'left' if sign > 0 else 'right'
    lead, trail = ('left_foot', 'right_foot') if sign > 0 else ('right_foot', 'left_foot')
    return Clip(name, f'A quarter turn to its {side} in two heavy steps, the staff kept upright.', 60, [
        at(0, GUARD),
        at(10, GUARD, ease='Out', hips=(0.0, -0.14, 0.0), turn=sign * 10.0),
        at(16, GUARD, turn=sign * 30.0, **foot(lead, sign * 30.0, lift=0.22), **foot(trail, 0.0)),
        at(22, GUARD, turn=sign * 50.0, **foot(lead, sign * 50.0), **foot(trail, 0.0)),
        at(38, GUARD, turn=sign * 80.0, **foot(lead, sign * 50.0), **foot(trail, sign * 80.0, lift=0.22)),
        at(46, GUARD, ease='In', turn=sign * 90.0, **feet(sign * 90.0)),
        at(60, GUARD, turn=sign * 90.0, **feet(sign * 90.0)),
    ])


def slam():
    return Clip('slam', 'The bell raised over its shoulder with both hands and brought down on the ground before it.', 100, [
        at(0, GUARD),
        at(36, RAISED, ease='Smooth', **feet(0.0)),
        at(44, RAISED, ease='Out', hips=(0.0, -0.1, -0.1), grip=(-0.4, 2.8, 0.1), aim=norm((-0.12, 0.45, -0.88))),
        at(46, ease='In', dense=4, hips=(0.0, -0.18, 0.0), pelvis=(0.0, -12.0, 0.0), spine=(0.0, 0.0, 0.0),
           chest=(0.0, -2.0, 0.0), head=(-12.0, 14.0, 0.0), grip=(-0.3, 2.85, 0.5), aim=norm((-0.06, 1.0, 0.1)),
           **foot('left_foot', 0.0, forward=0.25)),
        at(50, LANDED, ease='In', **foot('left_foot', 0.0, forward=0.25)),
        at(64, LANDED, hips=(0.0, -0.44, 0.18), head=(-26.0, 4.0, 0.0)),
        at(100, GUARD, ease='Smooth', **foot('left_foot', 0.0, forward=0.25)),
    ])


def sweep():
    return Clip('sweep', 'Wound far to its right, the staff swung flat round its front at a fighter\'s waist, bell first.', 86, [
        at(0, GUARD),
        at(30, ease='Smooth', hips=(0.0, -0.24, -0.04), pelvis=(0.0, -40.0, 0.0), spine=(4.0, -10.0, 0.0),
           chest=(4.0, -22.0, 0.0), head=(-8.0, 54.0, 0.0), **foot('left_foot', 0.0, forward=0.1),
           grip=(-0.72, 1.78, -0.22), aim=norm((-0.75, 0.2, -0.62)), roll=(0.0, 1.0, 0.0), elbow=(-0.6, -0.8, -0.4),
           hold=-0.35, reach=None, elbow_l=(0.4, -1.0, 0.4)),
        at(36, grip=(-0.74, 1.77, -0.26), aim=norm((-0.72, 0.18, -0.68))),
        at(38, ease='In', dense=10, pelvis=(0.0, -24.0, 0.0), chest=(4.0, -12.0, 0.0), head=(-8.0, 34.0, 0.0),
           grip=(-0.5, 1.78, 0.3), aim=norm((-0.85, -0.12, 0.5)), hold=None, reach=(0.9, 1.8, 0.0)),
        at(41, pelvis=(0.0, -4.0, 0.0), chest=(4.0, 2.0, 0.0), head=(-8.0, 6.0, 0.0),
           grip=(-0.3, 1.7, 0.56), aim=norm((-0.15, -0.18, 1.0))),
        at(44, pelvis=(0.0, 16.0, 0.0), chest=(4.0, 16.0, 0.0), head=(-8.0, -20.0, 0.0),
           grip=(0.06, 1.68, 0.56), aim=norm((0.7, -0.18, 0.7)), reach=(0.85, 1.85, -0.3)),
        at(46, ease='Out', pelvis=(0.0, 30.0, 0.0), chest=(4.0, 26.0, 0.0), head=(-8.0, -36.0, 0.0),
           grip=(0.3, 1.64, 0.46), aim=norm((1.0, -0.14, 0.1))),
        at(60, pelvis=(0.0, 18.0, 0.0), chest=(4.0, 14.0, 0.0), head=(-8.0, -18.0, 0.0),
           grip=(0.12, 1.7, 0.5), aim=norm((0.8, 0.1, 0.5)), hold=None, reach=(0.8, 1.7, 0.4)),
        at(86, GUARD, ease='Smooth', **foot('left_foot', 0.0, forward=0.1)),
    ])


def stomp():
    return Clip('stomp', 'Its right foot raised high and stamped down, the staff planted for balance.', 70, [
        at(0, GUARD),
        at(26, GUARD, ease='Smooth', hips=(0.08, -0.04, -0.02), pelvis=(-6.0, -4.0, 6.0), spine=(-4.0, 0.0, -4.0),
           chest=(-4.0, 0.0, -2.0), head=(-14.0, 0.0, 0.0), right_foot=(-0.42, 0.2, 0.75, -14.0, -10.0),
           knees=(0.15, 0.3), grip=(-0.52, 1.8, 0.45), aim=norm((-0.08, 1.0, 0.04))),
        at(30, ease='Out', right_foot=(-0.42, 0.24, 0.82, -14.0, -12.0), hips=(0.08, -0.02, -0.02)),
        at(33, ease='In', dense=4, hips=(0.0, -0.3, 0.06), pelvis=(10.0, -8.0, 0.0), spine=(6.0, 0.0, 0.0),
           chest=(6.0, 2.0, 0.0), head=(-14.0, 4.0, 0.0), right_foot=(-0.42, 0.3, 0.0, -14.0, 0.0),
           knees=(0.15, 0.15), grip=(-0.62, 1.5, 0.5)),
        at(44, hips=(0.0, -0.32, 0.06)),
        at(70, GUARD, ease='Smooth', right_foot=(-0.42, 0.3, 0.0, -14.0, 0.0)),
    ])


def charge():
    end = 7.0
    run = dict(
        hips=(0.0, -0.2, 0.2), pelvis=(18.0, -4.0, 0.0), spine=(8.0, 0.0, 0.0), chest=(6.0, 0.0, 0.0),
        head=(-24.0, 4.0, 0.0),
        grip=(-0.15, 1.75, 0.95), aim=norm((0.05, 0.12, 1.0)), roll=(0.0, 1.0, 0.0), elbow=(-0.8, -0.6, -0.2),
        hold=-0.05, reach=None, elbow_l=(0.8, -0.6, 0.0),
    )
    keys = [
        at(0, GUARD),
        at(22, run, ease='Smooth', hips=(0.0, -0.32, 0.1), **feet(0.0)),
        at(28, run, ease='Out', hips=(0.0, -0.34, 0.06), **foot('left_foot', 0.0, forward=0.3)),
    ]
    # Four long strides through the active ticks, a foot planted and the other in the air by turns.
    for i, tick in enumerate((31, 35, 39, 43)):
        root = (i + 0.6) * end / 4.0
        lead, trail = ('right_foot', 'left_foot') if i % 2 == 0 else ('left_foot', 'right_foot')
        keys.append(at(tick, run, ease='Linear', root=(0.0, root), hips=(0.0, -0.14 if i % 2 == 0 else -0.18, 0.22),
                       **foot(lead, 0.0, forward=root + 0.25), **foot(trail, 0.0, forward=root - 0.2, lift=0.35, pitch=-20.0)))
    keys += [
        at(46, run, ease='Out', root=(0.0, end), hips=(0.0, -0.36, 0.1), pelvis=(10.0, -4.0, 0.0),
           **foot('left_foot', 0.0, forward=end + 0.1), **foot('right_foot', 0.0, forward=end - 0.4)),
        at(86, GUARD, ease='Smooth', root=(0.0, end), **feet(0.0, forward=end)),
    ]
    return Clip('charge', 'Head down and the staff levelled like a ram, it charges seven metres and stops hard.', 86, keys)


def slam_double():
    second = dict(LANDED, grip=(0.12, 1.5, 0.76), aim=norm((0.25, -0.65, 0.72)), pelvis=(16.0, 10.0, 0.0))
    return Clip('slam_double', 'The bell brought down twice: once before it, then raised again and down a step to its left.', 150, [
        at(0, GUARD),
        at(36, RAISED, ease='Smooth', **feet(0.0)),
        at(44, RAISED, ease='Out', grip=(-0.4, 2.8, 0.1), aim=norm((-0.12, 0.45, -0.88))),
        at(46, ease='In', dense=4, hips=(0.0, -0.18, 0.0), pelvis=(0.0, -12.0, 0.0), spine=(0.0, 0.0, 0.0),
           chest=(0.0, -2.0, 0.0), head=(-12.0, 14.0, 0.0), grip=(-0.3, 2.85, 0.5), aim=norm((-0.06, 1.0, 0.1))),
        at(50, LANDED, ease='In'),
        at(62, LANDED, hips=(0.0, -0.44, 0.18)),
        at(88, RAISED, ease='Smooth', pelvis=(-4.0, -10.0, 0.0), **foot('left_foot', 0.0, forward=0.3)),
        at(94, RAISED, ease='Out', pelvis=(-4.0, -6.0, 0.0), grip=(-0.36, 2.8, 0.1), aim=norm((-0.05, 0.45, -0.88)),
           **foot('left_foot', 0.0, forward=0.3)),
        at(96, ease='In', dense=4, hips=(0.04, -0.18, 0.0), pelvis=(0.0, 4.0, 0.0), spine=(0.0, 0.0, 0.0),
           chest=(0.0, 4.0, 0.0), head=(-12.0, 0.0, 0.0), grip=(-0.2, 2.85, 0.5), aim=norm((0.1, 1.0, 0.1)), hold=0.18),
        at(100, second, ease='In', **foot('left_foot', 0.0, forward=0.3)),
        at(114, second, hips=(0.0, -0.44, 0.18), **foot('left_foot', 0.0, forward=0.3)),
        at(150, GUARD, ease='Smooth', **foot('left_foot', 0.0, forward=0.3)),
    ])


def leap():
    end = 4.5
    return Clip('leap', 'A crouch, a leap of four and a half metres with the bell raised, and a landing that brings it down.', 106, [
        at(0, GUARD),
        at(30, RAISED, ease='Smooth', hips=(0.0, -0.46, -0.04), pelvis=(10.0, -18.0, 0.0), spine=(6.0, -6.0, 0.0),
           chest=(4.0, -8.0, 0.0), head=(-20.0, 24.0, 0.0), knees=(0.25, 0.25), **feet(0.0)),
        at(34, ease='Out', root=(0.0, 0.4), hips=(0.0, 0.0, 0.0), pelvis=(-4.0, -14.0, 0.0),
           **foot('left_foot', 0.0, forward=0.1, lift=0.12, pitch=30.0), **foot('right_foot', 0.0, lift=0.16, pitch=40.0)),
        at(38, root=(0.0, 1.9), hips=(0.0, 0.7, 0.0), **foot('left_foot', 0.0, forward=2.2, lift=0.95),
           **foot('right_foot', 0.0, forward=1.7, lift=0.85, pitch=10.0), grip=(-0.36, 2.9, 0.1)),
        at(40, ease='In', root=(0.0, 3.0), hips=(0.0, 0.55, 0.0), **foot('left_foot', 0.0, forward=3.3, lift=0.75),
           **foot('right_foot', 0.0, forward=2.8, lift=0.7), grip=(-0.3, 2.95, 0.4), aim=norm((-0.06, 1.0, 0.1)),
           pelvis=(0.0, -12.0, 0.0), spine=(0.0, 0.0, 0.0), chest=(0.0, -2.0, 0.0), head=(-12.0, 14.0, 0.0)),
        at(43, LANDED, ease='In', dense=4, root=(0.0, end), hips=(0.0, -0.5, 0.2), knees=(0.25, 0.25),
           **feet(0.0, forward=end)),
        at(60, LANDED, root=(0.0, end), hips=(0.0, -0.48, 0.18), **feet(0.0, forward=end)),
        at(106, GUARD, ease='Smooth', root=(0.0, end), **feet(0.0, forward=end)),
    ])


def phase_shift():
    return Clip('phase_shift', 'Its second wind: it straightens to its full height, lifts the bell high in both hands, and drives the staff down into the ground.', 150, [
        at(0, GUARD),
        at(20, GUARD, ease='Out', hips=(0.0, -0.3, -0.02), spine=(10.0, 0.0, 0.0), chest=(10.0, 0.0, 0.0), head=(24.0, 0.0, 0.0)),
        at(66, ease='Smooth', hips=(0.0, -0.06, -0.06), pelvis=(-4.0, 0.0, 0.0), spine=(-6.0, 0.0, 0.0),
           chest=(-8.0, 0.0, 0.0), head=(-26.0, 0.0, 0.0), grip=(-0.1, 2.72, -0.05), aim=norm((0.05, 1.0, 0.06)),
           roll=(0.0, 0.0, -1.0), elbow=(-0.8, 0.2, -0.4), hold=0.14, reach=None, elbow_l=(0.8, 0.2, -0.4)),
        at(90, hips=(0.0, -0.07, -0.06), head=(-30.0, 0.0, 0.0), grip=(-0.1, 2.76, -0.05)),
        at(98, ease='In', dense=6, hips=(0.0, -0.4, 0.12), pelvis=(14.0, 0.0, 0.0), spine=(10.0, 0.0, 0.0),
           chest=(8.0, 0.0, 0.0), head=(-18.0, 0.0, 0.0), grip=(-0.15, 1.55, 0.7), aim=norm((0.0, 1.0, 0.1))),
        at(116, hips=(0.0, -0.42, 0.12)),
        at(150, GUARD, ease='Smooth'),
    ])


def stagger():
    return Clip('stagger', 'Rocked back a step, the bell swinging wide, then its guard again.', 60, [
        at(0, GUARD),
        at(8, GUARD, ease='Out', dense=4, root=(0.0, -0.2), hips=(0.0, -0.06, -0.14), pelvis=(-8.0, -12.0, 0.0),
           spine=(-10.0, 0.0, 4.0), chest=(-14.0, 8.0, 6.0), head=(-20.0, 16.0, 0.0),
           grip=(-0.74, 1.8, 0.2), aim=norm((-0.5, 0.9, -0.3)), reach=(0.95, 1.85, 0.0)),
        at(22, GUARD, root=(0.0, -0.4), hips=(0.0, -0.2, -0.04), **foot('right_foot', 0.0, forward=-0.5),
           **foot('left_foot', 0.0, forward=-0.3)),
        at(60, GUARD, ease='Smooth', root=(0.0, -0.4), **foot('right_foot', 0.0, forward=-0.5),
           **foot('left_foot', 0.0, forward=-0.3)),
    ])


def stagger_break():
    return Clip('break', 'Its guard broken: it drops to a knee, bowed over the planted staff, and gets up again.', 180, [
        at(0, GUARD),
        at(10, GUARD, ease='Out', hips=(0.0, -0.1, -0.12), spine=(-8.0, 0.0, 0.0), chest=(-12.0, 0.0, 0.0),
           head=(-18.0, 0.0, 0.0), grip=(-0.76, 1.8, 0.3), aim=norm((-0.3, 1.0, -0.1))),
        at(30, KNEELING, ease='In', head=(36.0, 0.0, 0.0)),
        at(40, KNEELING, ease='Out', hips=(0.0, -0.7, -0.18), head=(38.0, 0.0, 0.0)),
        at(130, KNEELING, ease='Smooth', head=(30.0, 6.0, 0.0)),
        at(160, GUARD, ease='Smooth', hips=(0.0, -0.3, -0.04), right_foot=(-0.42, -0.4, 0.0, -14.0, 0.0),
           left_foot=(LEFT[0], 0.3, 0.0, 8.0, 0.0)),
        at(180, GUARD, left_foot=(LEFT[0], 0.3, 0.0, 8.0, 0.0), right_foot=(-0.42, -0.4, 0.0, -14.0, 0.0)),
    ])


def death():
    return Clip('death', 'Its last: it sinks to a knee, the bell lowered, and bows over the staff to go still as stone.', 180, [
        at(0, GUARD),
        at(16, GUARD, ease='Out', hips=(0.0, -0.08, -0.12), spine=(-10.0, 0.0, 0.0), chest=(-14.0, 6.0, 0.0),
           head=(-26.0, 0.0, 0.0), grip=(-0.72, 1.8, 0.4), aim=norm((-0.2, 1.0, 0.0)), reach=(1.1, 1.9, 0.2)),
        at(60, KNEELING, ease='Smooth'),
        at(110, KNEELING, ease='Smooth', hips=(0.0, -0.76, -0.12), pelvis=(18.0, -4.0, 0.0), spine=(16.0, 0.0, 0.0),
           chest=(14.0, 0.0, 0.0), head=(44.0, 0.0, 0.0), grip=(-0.6, 1.0, 0.62), aim=norm((0.08, 1.0, 0.22)),
           reach=(0.0, 1.1, 0.95), elbow_l=(0.6, -0.6, 0.2)),
        at(180, ease='Out', head=(46.0, 0.0, 0.0)),
    ])


CLIPS = [
    awaken(), guard(), turning('turn_left', 1.0), turning('turn_right', -1.0), slam(), sweep(), stomp(),
    charge(), slam_double(), leap(), phase_shift(), stagger(), stagger_break(), death(),
]

if __name__ == '__main__':
    main('knell_colossus.py', FIGURE, RIG_ID, CLIPS, __doc__.splitlines()[0])
