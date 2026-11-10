"""The character's humanoid rest skeleton, read from its VRM: each bone's model-space place and rotation."""
import json
import os
import struct

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
VRM = os.path.join(REPO, 'content/lookdev/character/character.vrm')


def _qmul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return (aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz)


def _qrot(q, v):
    x, y, z, w = q
    t = (2 * (y * v[2] - z * v[1]), 2 * (z * v[0] - x * v[2]), 2 * (x * v[1] - y * v[0]))
    return (v[0] + w * t[0] + y * t[2] - z * t[1], v[1] + w * t[1] + z * t[0] - x * t[2],
            v[2] + w * t[2] + x * t[1] - y * t[0])


def load(path=VRM):
    """`{bone: {'pos': (x, y, z), 'rot': (x, y, z, w)}}`, by VRM's bone names (`rightHand`)."""
    with open(path, 'rb') as f:
        data = f.read()
    json_length = struct.unpack('<I', data[12:16])[0]
    doc = json.loads(data[20:20 + json_length])
    nodes = doc['nodes']
    parents = {child: i for i, node in enumerate(nodes) for child in node.get('children', [])}

    def place(i):
        node = nodes[i]
        translation = node.get('translation', [0, 0, 0])
        rotation = node.get('rotation', [0, 0, 0, 1])
        if i not in parents:
            return tuple(translation), tuple(rotation)
        at, turned = place(parents[i])
        return tuple(a + b for a, b in zip(at, _qrot(turned, translation))), _qmul(turned, rotation)

    bones = doc['extensions']['VRMC_vrm']['humanoid']['humanBones']
    return {name: dict(zip(('pos', 'rot'), place(bone['node']))) for name, bone in bones.items()}
