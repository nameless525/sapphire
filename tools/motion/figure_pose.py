"""Pose a figure's clips by target, and write them as `.keypose.ron` documents (M3 C2).

A figure is a model `sapphire_pipeline gen-figures` writes from a spec in `tools/figures/`, with
KayKit's joint names and its weapon skinned to `handslot.r`. Its clips are first drafts, posed here
the way `warrior.py` poses the warrior's: a key says where the feet stand, where the weapon is and
which way it points, where the other hand goes and how the body turns, and every tick between keys
is solved by inverse kinematics from targets blended between them, so an arc stays an arc.

A figure's module (`lancer.py`, `knell_colossus.py`) lists its clips as keys and calls `main`.
It writes nothing unless told to with `--write`, and fails when a written file differs from what it
would write, so a clean checkout is what it makes.

Spaces, as `pose.py`'s: +X is the figure's left, +Y up, +Z forward. Feet are in the clip's world
(where the figure stood at tick 0, facing +Z), so a planted foot stays put while the root travels;
the weapon and hands are in the model's space, which travels and turns with the root.
"""
import argparse
import json
import math
import struct
import uuid
from pathlib import Path

from pose import (
    QI, add, cross, dot, forearm_twist, from_basis, length, lerp, mul, norm, qinv, qmul, qnorm, qrot,
    reach_chain, reject, slerp, sub, to_euler, turn, weight,
)

REPO = Path(__file__).resolve().parents[2]
X, Y, Z = (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0)

# Humanoid bones by the rig's names, and their parents in the chain a turn composes down.
BONES = {
    'Hips': ('hips', None),
    'Spine': ('spine', 'Hips'),
    'Chest': ('chest', 'Spine'),
    'Head': ('head', 'Chest'),
}
for _side, _suffix in (('Left', '.l'), ('Right', '.r')):
    for _stem, _name, _parent in (
        ('UpperLeg', 'upperleg', 'Hips'), ('LowerLeg', 'lowerleg', _side + 'UpperLeg'),
        ('Foot', 'foot', _side + 'LowerLeg'), ('Toes', 'toes', _side + 'Foot'),
        ('UpperArm', 'upperarm', 'Chest'), ('LowerArm', 'lowerarm', _side + 'UpperArm'),
        ('Hand', 'wrist', _side + 'LowerArm'),
    ):
        BONES[_side + _stem] = (_name + _suffix, _parent)

# Each limb's flexion axis at rest, in the model's space: knees bend back about +X, elbows bring the
# forearm forward, about -Y on the left and +Y on the right. The T-pose's limbs are straight, so the
# rest can't say.
HINGES = {'Leg': {'Left': X, 'Right': X}, 'Arm': {'Left': (0.0, -1.0, 0.0), 'Right': Y}}


def read_glb(path):
    data = Path(path).read_bytes()
    size = struct.unpack_from('<I', data, 12)[0]
    return json.loads(data[20:20 + size])


class Rig:
    """A figure's rest skeleton, read from its model: each joint's place and rotation in model space."""

    def __init__(self, model):
        document = read_glb(model)
        nodes = document['nodes']
        parents = {child: i for i, node in enumerate(nodes) for child in node.get('children', [])}

        def place(i):
            node = nodes[i]
            pos, rot = tuple(node.get('translation', (0, 0, 0))), tuple(node.get('rotation', QI))
            if i not in parents:
                return pos, rot
            parent, parent_rot = place(parents[i])
            return add(parent, qrot(parent_rot, pos)), qnorm(qmul(parent_rot, rot))

        self.joints = {node['name']: place(i) for i, node in enumerate(nodes)}
        self.rest = {bone: self.joints[name][0] for bone, (name, _) in BONES.items()}
        # Where each hand's slot is from its wrist, at rest.
        self.slot = {side: sub(self.joints['handslot.' + side[0].lower()][0], self.rest[side + 'Hand'])
                     for side in ('Left', 'Right')}
        # How far the model's lowest point, its soles, is from the lower of a foot's ankle and toes.
        bottom = min(document['accessors'][primitive['attributes']['POSITION']]['min'][1]
                     for mesh in document['meshes'] for primitive in mesh['primitives'])
        self.sole = bottom - min(self.rest['LeftToes'][1], self.rest['LeftFoot'][1])

    def length(self, upper, lower):
        return length(sub(self.rest[lower], self.rest[upper]))


class Pose:
    """Turns for every humanoid bone, and where forward kinematics puts each."""

    def __init__(self, rig):
        self.rig = rig
        self.hips = (0.0, 0.0, 0.0)
        self.turns = {bone: QI for bone in BONES}
        self.pos, self.rot = {}, {}
        self.fk()

    def fk(self):
        rest = self.rig.rest
        for bone, (_, parent) in BONES.items():
            if parent is None:
                self.pos[bone] = add(rest[bone], self.hips)
                self.rot[bone] = self.turns[bone]
            else:
                self.pos[bone] = add(self.pos[parent], qrot(self.rot[parent], sub(rest[bone], rest[parent])))
                self.rot[bone] = qnorm(qmul(self.rot[parent], self.turns[bone]))

    def set_world(self, bone, rotation):
        parent = BONES[bone][1]
        parent_rot = self.rot[parent] if parent else QI
        self.turns[bone] = qnorm(qmul(qinv(parent_rot), rotation))
        self.fk()

    def chain(self, limb, side, target, pole, fallback):
        """Solves a leg or an arm to put its end at `target`; returns how far it fell short."""
        upper, lower, end = ((side + 'UpperLeg', side + 'LowerLeg', side + 'Foot') if limb == 'Leg'
                             else (side + 'UpperArm', side + 'LowerArm', side + 'Hand'))
        rest = self.rig.rest
        r1, r2 = sub(rest[lower], rest[upper]), sub(rest[end], rest[lower])
        upper_rot, lower_rot, miss = reach_chain(self.pos[upper], r1, r2, target, pole,
                                                 HINGES[limb][side], fallback, reach=.985)
        self.set_world(upper, upper_rot)
        self.set_world(lower, lower_rot)
        return miss

    def slot(self, side):
        """Where a hand's slot is now, and the rotation its weapon has from rest."""
        hand = side + 'Hand'
        return add(self.pos[hand], qrot(self.rot[hand], self.rig.slot[side])), self.rot[hand]


def grip_rotation(direction, roll):
    """The rotation taking a slot's rest axes (its weapon along +Z, its top along +Y) to a weapon
    pointing along `direction` with its top toward `roll`."""
    forward = norm(direction)
    up = norm(reject(roll, forward))
    if length(up) < 1e-6:
        up = norm(reject(X if abs(forward[0]) < .9 else Y, forward))
    return from_basis(cross(up, forward), up, forward)


def yaw(vector, degrees):
    return qrot(turn(0, degrees, 0), vector)


# ---------------------------------------------------------------- keys and their blend

# A key's fields, and what a first key holds of those it doesn't name.
DEFAULTS = {
    'ease': 'Smooth',
    'root': (0.0, 0.0),         # travel since tick 0, in the clip's world: (x, z)
    'turn': 0.0,                # root turn since tick 0, degrees about +Y
    'hips': (0.0, 0.0, 0.0),    # the hips from rest, in model space
    'pelvis': (0.0, 0.0, 0.0),  # turns, x then y then z, degrees
    'spine': (0.0, 0.0, 0.0),
    'chest': (0.0, 0.0, 0.0),
    'head': (0.0, 0.0, 0.0),
    # Each foot's ankle in the clip's world: (x, z, lift, yaw, pitch).
    'left_foot': None,
    'right_foot': None,
    'knees': (0.0, 0.0),        # how far each knee's pole leans out, left and right
    # The right hand's slot: where, in model space, and which way the weapon points and its top faces.
    'grip': None,
    'aim': Z,
    'roll': Y,
    'elbow': (-0.4, -1.0, -0.4),
    # The left hand: on the weapon this far from the right's slot, or free at `reach` in model space.
    'hold': None,
    'reach': None,
    'elbow_l': (0.4, -1.0, -0.4),
}
VECTORS = {'aim', 'roll', 'elbow', 'elbow_l'}


def keys_of(rig, keys):
    """Fills every key's fields from the key before it, and the first's from rest."""
    filled, before = [], dict(DEFAULTS)
    rest = rig.rest
    before['left_foot'] = (rest['LeftFoot'][0], rest['LeftFoot'][2], 0.0, 0.0, 0.0)
    before['right_foot'] = (rest['RightFoot'][0], rest['RightFoot'][2], 0.0, 0.0, 0.0)
    for index, key in enumerate(keys):
        unknown = set(key) - set(DEFAULTS) - {'tick', 'dense'}
        if unknown:
            raise ValueError(f'key {index}: {sorted(unknown)}')
        now = dict(before)
        now.update(key)
        now['ease'] = key.get('ease', 'Smooth')
        filled.append(now)
        before = now
    if filled[0]['tick'] != 0:
        raise ValueError('the first key is at tick 0')
    if any(b['tick'] <= a['tick'] for a, b in zip(filled, filled[1:])):
        raise ValueError('keys go by increasing tick')
    if filled[0]['root'] != (0.0, 0.0) or filled[0]['turn'] != 0.0:
        raise ValueError("the first key's root is where the clip starts")
    return filled


def blend(a, b, share):
    """`a` toward `b`: vectors by direction, everything else straight."""
    out = {}
    for field, value in a.items():
        other = b[field]
        if field in ('ease', 'tick', 'dense'):
            out[field] = value
        elif value is None or other is None:
            if field == 'hold':
                # A hand lets go of the weapon as a blend starts and takes it again as it ends.
                out[field] = value if share <= 0 else other if share >= 1 else None
            elif field == 'reach' and value is None and share > 0:
                out[field] = other
            else:
                out[field] = value if share < 1 else other
        elif field == 'hold':
            out[field] = value + (other - value) * share
        elif field in VECTORS:
            out[field] = norm(lerp(value, other, share))
        elif isinstance(value, tuple):
            out[field] = lerp(value, other, share)
        else:
            out[field] = value + (other - value) * share
    return out


def at_tick(keys, tick):
    for a, b in zip(keys, keys[1:]):
        if a['tick'] <= tick <= b['tick']:
            u = (tick - a['tick']) / (b['tick'] - a['tick'])
            return blend(a, b, weight(b['ease'], u))
    return dict(keys[-1])


# ---------------------------------------------------------------- solving a tick

def solve(rig, key):
    """A pose from a blended key, and the furthest any limb fell short of its target."""
    pose = Pose(rig)
    pose.hips = key['hips']
    for bone, field in (('Hips', 'pelvis'), ('Spine', 'spine'), ('Chest', 'chest'), ('Head', 'head')):
        pose.turns[bone] = turn(*key[field])
    pose.fk()
    miss = 0.0
    root, root_turn = key['root'], key['turn']
    for side, field, lean in (('Left', 'left_foot', key['knees'][0]), ('Right', 'right_foot', key['knees'][1])):
        x, z, lift, foot_yaw, pitch = key[field]
        place = yaw((x - root[0], 0.0, z - root[1]), -root_turn)
        target = (place[0], rig.rest[side + 'Foot'][1] + lift, place[2])
        facing = foot_yaw - root_turn
        sign = 1 if side == 'Left' else -1
        pole = add(yaw(Z, facing), (sign * lean, 0.0, 0.0))
        miss = max(miss, pose.chain('Leg', side, target, pole, qrot(pose.rot['Hips'], Z)))
        pose.set_world(side + 'Foot', turn(pitch, facing, 0))
        pose.set_world(side + 'Toes', turn(0, facing, 0))
    if key['grip'] is not None:
        miss = max(miss, hold(pose, 'Right', key['grip'], key['aim'], key['roll'], key['elbow']))
        if key['hold'] is not None:
            slot, _ = pose.slot('Right')
            point = add(slot, mul(norm(key['aim']), key['hold']))
            miss = max(miss, hold(pose, 'Left', point, key['aim'], key['roll'], key['elbow_l']))
    if key['reach'] is not None and key['hold'] is None:
        miss = max(miss, pose.chain('Arm', 'Left', key['reach'], key['elbow_l'], qrot(pose.rot['Chest'], (0, -.8, -.4))))
        pose.set_world('LeftHand', pose.rot['LeftLowerArm'])
    return pose, miss


def hold(pose, side, point, aim, roll, elbow):
    """Puts a hand's slot at `point`, its weapon along `aim`."""
    rotation = grip_rotation(aim, roll)
    wrist = sub(point, qrot(rotation, pose.rig.slot[side]))
    sign = 1 if side == 'Left' else -1
    miss = pose.chain('Arm', side, wrist, elbow, qrot(pose.rot['Chest'], (sign * .5, -.8, -.43)))
    lower, hand = side + 'LowerArm', side + 'Hand'
    axis = norm(sub(pose.pos[hand], pose.pos[lower]))
    pose.set_world(lower, forearm_twist(pose.rot[lower], rotation, axis, share=.6))
    pose.set_world(hand, rotation)
    return miss


# ---------------------------------------------------------------- a clip

class Clip:
    """A clip by its keys: `ticks` long, holding its last key, a key every `step` ticks between those
    authored and every tick through a key's `dense` span."""

    def __init__(self, name, description, ticks, keys, step=2):
        self.name, self.description, self.ticks, self.keys, self.step = name, description, ticks, keys, step


def sample(rig, clip):
    keys = keys_of(rig, clip.keys)
    if keys[-1]['tick'] > clip.ticks:
        raise ValueError(f'{clip.name}: a key past its {clip.ticks} ticks')
    dense = set()
    for key in clip.keys:
        if 'dense' in key:
            dense.update(range(key['tick'], key['tick'] + key['dense'] + 1))
    ticks = sorted({key['tick'] for key in keys} | dense | set(range(0, keys[-1]['tick'] + 1, clip.step)))
    samples = []
    for tick in ticks:
        key = at_tick(keys, tick)
        pose, miss = solve(rig, key)
        samples.append((tick, key, pose, miss))
    return samples


def soles(rig, pose):
    """The lowest point under each foot, from the ankle and the toes."""
    lows = []
    for side in ('Left', 'Right'):
        lows.append(min(pose.pos[side + 'Foot'][1], pose.pos[side + 'Toes'][1]) + rig.sole)
    return lows


def check(rig, clip, samples, verbose=False):
    """Warnings: a limb short of its target by over 2 cm, a sole under the ground."""
    problems = []
    for tick, key, pose, miss in samples:
        if miss > .02:
            problems.append(f'{clip.name} tick {tick}: a limb is {miss * 100:.1f} cm short')
        low = min(soles(rig, pose))
        if low < -.015:
            problems.append(f'{clip.name} tick {tick}: a sole is {-low * 100:.1f} cm under the ground')
        if verbose:
            slot, _ = pose.slot('Right')
            print(f'{clip.name} {tick:4}: hips {pose.pos["Hips"][1]:.3f} slot ({slot[0]:.2f}, {slot[1]:.2f}, {slot[2]:.2f})')
    return problems


def number(value):
    return f'{value:.4f}'.rstrip('0').rstrip('.') if abs(value) >= .00005 else '0'


def ron(tool, clip, samples):
    travels = any(key['root'] != (0.0, 0.0) or key['turn'] != 0.0 for _, key, _, _ in samples)
    lines = [
        f'// Generated by tools/motion/{tool} --write.',
        '// ' + clip.description,
        '(',
        '    type: "sapphire::pipeline::KeyPoseClip",',
        '    version: 1,',
        '    data: (',
        f'        ticks: {clip.ticks},',
        '        keys: [',
    ]
    for tick, key, pose, _ in samples:
        head = f'            (tick: {tick}, ease: Linear, hips: ({", ".join(number(v) for v in pose.hips)})'
        if travels:
            head += f', root: ({number(key["root"][0])}, 0, {number(key["root"][1])}), root_turn: {number(key["turn"])}'
        lines.append(head + ', pose: {')
        values = []
        for bone, rotation in pose.turns.items():
            angles = ', '.join(f'{axis}: {number(v)}' for axis, v in zip('xyz', to_euler(rotation)))
            values.append(f'{bone}: ({angles}),')
        for i in range(0, len(values), 2):
            lines.append('                ' + ' '.join(values[i:i + 2]))
        lines.append('            }),')
    lines.extend(['        ],', '    ),', ')', ''])
    return '\n'.join(lines)


def meta(rig_id, figure, name):
    clip_id = uuid.uuid5(uuid.UUID(rig_id), f'{figure}/{name}')
    return f'''(
    type: "sapphire::assets::AssetMeta",
    version: 1,
    data: (
        id: "{clip_id}",
        importer: KeyPose((
            rig: "{rig_id}",
        )),
    ),
)
'''


def motion(clip, samples, weapon):
    """A move's `motion` for a clip, in `.moves.ron`'s words: the root where a key moves it, and the
    weapon's hit path at each authored key and every tick of a dense span. `weapon` is the path's name
    and how far along the weapon from the slot its middle is; the path's +Y runs along the weapon."""
    name, along = weapon
    ticks = {key['tick'] for key in clip.keys}
    for key in clip.keys:
        if 'dense' in key:
            ticks.update(range(key['tick'], key['tick'] + key['dense'] + 1))
    ticks.add(clip.ticks if clip.ticks in {tick for tick, *_ in samples} else max(tick for tick, *_ in samples))
    root, path, last = [], [], None
    for tick, key, pose, _ in samples:
        if tick not in ticks:
            continue
        place = (key['root'][0], key['root'][1], math.radians(key['turn']))
        if place != last and tick > 0:
            yaw_text = f', yaw: {number(place[2])}' if place[2] else ''
            root.append(f'(at: {tick}, offset: ({number(place[0])}, 0.0, {number(place[1])}){yaw_text}),')
        last = place
        slot, rotation = pose.slot('Right')
        point = add(slot, mul(qrot(rotation, Z), along))
        rotation = qnorm(qmul(rotation, (math.sin(math.pi / 4), 0.0, 0.0, math.cos(math.pi / 4))))
        offset = ', '.join(number(v) for v in point)
        turn_text = ', '.join(number(v) for v in rotation)
        path.append(f'(at: {tick}, offset: ({offset}), rotation: ({turn_text})),')
    lines = [f'// {clip.name}: {clip.ticks} ticks', 'root: [', *('    ' + line for line in root), '],']
    lines += ['paths: [', f'    (name: "{name}", keys: [', *('        ' + line for line in path), '    ]),', '],']
    return '\n'.join(lines)


def main(tool, figure, rig_id, clips, description, weapon=None):
    """Checks every clip, and writes each with `--write`: `figure` is the folder under
    `content/combat/` and its model's stem. With `--motion`, prints each clip's root and its weapon's
    hit path (`weapon`, as [`motion`] takes it) for its move in `<figure>.moves.ron`."""
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument('clips', nargs='*', help='only these clips')
    parser.add_argument('--write', action='store_true', help='write the key-pose documents and their sidecars')
    parser.add_argument('--motion', action='store_true', help="print each clip's move motion: root and hit path")
    parser.add_argument('-v', '--verbose', action='store_true', help='print each sample')
    args = parser.parse_args()
    folder = REPO / 'content/combat' / figure
    rig = Rig(folder / f'{figure}.glb')
    problems = []
    for clip in clips:
        if args.clips and clip.name not in args.clips:
            continue
        samples = sample(rig, clip)
        if args.motion and weapon is not None:
            print(motion(clip, samples, weapon))
        problems += check(rig, clip, samples, args.verbose)
        path = folder / f'{clip.name}.keypose.ron'
        for file, contents in ((path, ron(tool, clip, samples)), (path.with_suffix('.ron.meta'), meta(rig_id, figure, clip.name))):
            if file.exists() and file.read_text() == contents:
                continue
            if not args.write:
                parser.exit(1, f'{file.name} differs; run {tool} --write\n')
            file.write_text(contents)
    for problem in problems:
        print('warning:', problem)
