"""Key poses for the Lantern Ward's cutscenes (M3 Q3), posed by target as the player's moves are. See the README.

usage: cutscene.py [--write] [-v]

With no flag it writes nothing, and prints what `Move.check` finds. `--write` writes the key-pose documents
beside the timeline that plays them, in `content/zones/lantern_ward/`. These aren't moves: no frame data names
them, and a timeline's character track plays them through the graph's combat slot.
"""
import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import frames
from moves import HANDS, IDLE_ARMS, IDLE_BODY, IDLE_HIPS, SIDECAR, idle_feet, sword
from pose import *
from rig import REPO

FOLDER = os.path.join(REPO, 'content/zones/lantern_ward')

# Each document's sidecar ID, kept here so a write never makes a new one.
IDS = {
    'court_guard': '08570a0b-1c6b-4fb6-873b-bff74e6d2264',
}

HEADER = """Turns are degrees in the character's rest space, about each bone's own joint, X then Y then Z.
The character faces +Z, with its left arm along +X at rest. Ticks are 60 a second. Written by
tools/motion/cutscene.py, so edit it there."""


def court_guard():
    m = Move('court_guard', 204, """The court's gate (M3 Q3): the character answers the warden by drawing the blade down and back,
then bringing it up into a high guard aimed up the court, and holding it there as the shot widens. The
feet stay where they stand: the cutscene holds the fight, and nothing in it moves a character.

""" + HEADER)
    feet = idle_feet()
    m.key(0, "Ready, as the fight left the character.", root=(0, 0, 0), hips=IDLE_HIPS, body=IDLE_BODY,
          look=(0, 1), fingers=HANDS, **feet, **IDLE_ARMS)
    m.key(30, "The eyes come up to the warden first. The shoulders follow a little.",
          hips=IDLE_HIPS, body=dict(Hips=(3, -12, 0), Spine=(2, 4, 0), Chest=(0, 4, -1), UpperChest=(-2, 2, 0)),
          look=(0, 6), **IDLE_ARMS)
    m.key(66, "The hips turn into the right side and the blade goes down and back past the hip, the free hand open.",
          ease='Smooth', hips=(0.0, -0.06, -0.04),
          body=dict(Hips=(5, -24, 2), Spine=(5, -6, 1), Chest=(3, -6, 1), UpperChest=(2, -2, 0)),
          look=(-3, 4), shoulder_r=(0, -4, 1), shoulder_l=(0, -5, 1),
          rh=(-61, -38, 0.36), elbow_r=(-0.55, -0.75, 0.25), **sword(-100, -20, -30),
          lh=(30, -40, 0.33), elbow_l=(0.70, -0.64, -0.20))
    m.key(110, "The body unwinds and the blade rises in front, both hands meeting on the grip.",
          ease='Smooth', hips=(0.01, -0.05, -0.03),
          body=dict(Hips=(4, -8, 0), Spine=(3, 2, 0), Chest=(1, 3, 0), UpperChest=(-2, 2, 0)),
          look=(0, 5), shoulder_r=(0, 2, 0), shoulder_l=(0, -2, 0),
          rh=(-6, -18, 0.36), elbow_r=(-0.60, -0.72, -0.30), **sword(-4, 48, -10),
          lh=(8, -22, 0.34), elbow_l=(0.62, -0.70, -0.30))
    m.key(150, "The high guard: hilt at the chin, point up the court, weight settled low.",
          ease='Out', hips=(0.01, -0.07, -0.03),
          body=dict(Hips=(5, -10, 0), Spine=(3, 3, 0), Chest=(1, 4, 0), UpperChest=(-2, 2, 0)),
          look=(0, 4), shoulder_r=(0, 3, 0), shoulder_l=(0, -3, 0),
          rh=(-8, -10, 0.34), elbow_r=(-0.62, -0.70, -0.32), **sword(-6, 58, -10),
          lh=(10, -16, 0.33), elbow_l=(0.64, -0.68, -0.32))
    m.key(204, "Held, breathing, as the view goes back behind the character.",
          ease='Smooth', hips=(0.01, -0.068, -0.03),
          body=dict(Hips=(5, -10, 0), Spine=(3, 3, 0), Chest=(1, 4, 0), UpperChest=(-2, 2, 0)),
          look=(0, 4), shoulder_r=(0, 3, 0), shoulder_l=(0, -3, 0),
          rh=(-8, -11, 0.34), elbow_r=(-0.62, -0.70, -0.32), **sword(-6, 57, -10),
          lh=(10, -17, 0.33), elbow_l=(0.64, -0.68, -0.32))
    return m


POSES = [court_guard]


def main(args=None, folder=FOLDER):
    args = sys.argv[1:] if args is None else args
    unknown = [arg for arg in args if arg not in {'--write', '-v'}]
    if unknown:
        sys.exit(f'unknown arguments: {", ".join(unknown)}\n{__doc__}')
    for make in POSES:
        move = make()
        move.check(verbose='-v' in args)
        if '--write' in args:
            frames.write_changed(os.path.join(folder, move.name + '.keypose.ron'),
                                 move.ron().replace('\n', os.linesep))
            frames.write_changed(os.path.join(folder, move.name + '.keypose.ron.meta'),
                                 (SIDECAR % IDS[move.name]).replace('\n', os.linesep))
    if '--write' in args:
        print(f'wrote the key-pose documents in {folder}')


if __name__ == '__main__':
    main()
