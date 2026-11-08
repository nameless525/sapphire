"""Pose by targets, interpolate by joints, and write a move as a `.keypose.ron` document.

Model space: +X the character's left, +Y up, +Z forward. A turn is a rotation in rest space about the
bone's joint, X then Y then Z, composed down the chain, exactly as `sapphire_pipeline::keypose`.
"""
import json
import math

import rig

RIG = rig.load()


# ---------------------------------------------------------------- vectors and quaternions
def add(a, b): return (a[0] + b[0], a[1] + b[1], a[2] + b[2])
def sub(a, b): return (a[0] - b[0], a[1] - b[1], a[2] - b[2])
def mul(a, s): return (a[0] * s, a[1] * s, a[2] * s)
def dot(a, b): return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
def cross(a, b): return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])
def length(a): return math.sqrt(dot(a, a))
def norm(a):
    l = length(a)
    return (a[0] / l, a[1] / l, a[2] / l) if l > 1e-9 else (0.0, 0.0, 0.0)
def lerp(a, b, w): return tuple(x + (y - x) * w for x, y in zip(a, b))


QI = (0.0, 0.0, 0.0, 1.0)
def qaxis(axis, deg):
    h = math.radians(deg) * 0.5
    s = math.sin(h)
    return (axis[0] * s, axis[1] * s, axis[2] * s, math.cos(h))
def qmul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return (aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz)
def qinv(q): return (-q[0], -q[1], -q[2], q[3])
def qrot(q, v):
    x, y, z, w = q
    t = (2 * (y * v[2] - z * v[1]), 2 * (z * v[0] - x * v[2]), 2 * (x * v[1] - y * v[0]))
    return (v[0] + w * t[0] + y * t[2] - z * t[1], v[1] + w * t[1] + z * t[0] - x * t[2],
            v[2] + w * t[2] + x * t[1] - y * t[0])
def qnorm(q):
    l = math.sqrt(sum(c * c for c in q))
    return tuple(c / l for c in q)
def slerp(a, b, w):
    d = sum(x * y for x, y in zip(a, b))
    if d < 0:
        b, d = tuple(-c for c in b), -d
    if d > 0.9995:
        return qnorm(tuple(x + (y - x) * w for x, y in zip(a, b)))
    ang = math.acos(min(d, 1.0))
    s = math.sin(ang)
    f, g = math.sin((1 - w) * ang) / s, math.sin(w * ang) / s
    return qnorm(tuple(x * f + y * g for x, y in zip(a, b)))
def turn(x=0.0, y=0.0, z=0.0):
    return qnorm(qmul(qaxis((0, 0, 1), z), qmul(qaxis((0, 1, 0), y), qaxis((1, 0, 0), x))))
def to_euler(q):
    """(x, y, z) degrees with R = Rz(z) Ry(y) Rx(x)."""
    x, y, z, w = q
    r00 = 1 - 2 * (y * y + z * z)
    r10 = 2 * (x * y + w * z)
    r20 = 2 * (x * z - w * y)
    r21 = 2 * (y * z + w * x)
    r22 = 1 - 2 * (x * x + y * y)
    if abs(r20) >= 1.0 - 1e-12:
        # At a pitch of ±90°, X and Z describe the same axis. Pick X = 0 and recover Z
        # from the second matrix column, which still has a reliable direction.
        r01 = 2 * (x * y - w * z)
        r11 = 1 - 2 * (x * x + z * z)
        return 0.0, math.copysign(90.0, -r20), math.degrees(math.atan2(-r01, r11))
    b = math.asin(max(-1.0, min(1.0, -r20)))
    a = math.atan2(r21, r22)
    c = math.atan2(r10, r00)
    return (math.degrees(a), math.degrees(b), math.degrees(c))
def from_basis(u, v, w):
    """The rotation taking the unit axes X, Y, Z to u, v, w."""
    m00, m10, m20 = u
    m01, m11, m21 = v
    m02, m12, m22 = w
    tr = m00 + m11 + m22
    if tr > 0:
        s = math.sqrt(tr + 1.0) * 2
        q = ((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
    elif m00 > m11 and m00 > m22:
        s = math.sqrt(1.0 + m00 - m11 - m22) * 2
        q = (0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
    elif m11 > m22:
        s = math.sqrt(1.0 + m11 - m00 - m22) * 2
        q = ((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
    else:
        s = math.sqrt(1.0 + m22 - m00 - m11) * 2
        q = ((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
    return qnorm(q)
def frame(u, hint):
    """A right-handed frame (u, v, w) whose w is `hint` made perpendicular to u."""
    u = norm(u)
    w = norm(sub(hint, mul(u, dot(hint, u))))
    v = cross(w, u)
    return u, v, w
def reject(v, axis):
    """Remove the component along a unit axis."""
    return sub(v, mul(axis, dot(v, axis)))
def perpendicular(axis):
    cardinal = min(((1, 0, 0), (0, 1, 0), (0, 0, 1)), key=lambda v: abs(dot(v, axis)))
    return norm(reject(cardinal, axis))
def plane_direction(hint, axis, fallback, threshold):
    projected = reject(norm(hint), axis)
    strength = length(projected)
    if strength >= threshold:
        return mul(projected, 1.0 / strength)
    direction = norm(reject(fallback, axis))
    if length(direction) == 0.0:
        direction = perpendicular(axis)
    if strength > 1e-9:
        angle = math.degrees(math.atan2(dot(axis, cross(direction, projected)), dot(direction, projected)))
        blend = strength / threshold
        direction = qrot(qaxis(axis, angle * blend * blend * (3.0 - 2.0 * blend)), direction)
    return direction
def angle_between(a, b):
    return math.degrees(math.atan2(length(cross(a, b)), dot(a, b)))
def between_frames(a, b):
    """The rotation taking frame a to frame b."""
    return qmul(from_basis(*b), qinv(from_basis(*a)))
def polar(az, el, r=1.0):
    """Azimuth about +Y from +Z toward +X, elevation up: degrees."""
    a, e = math.radians(az), math.radians(el)
    return (math.sin(a) * math.cos(e) * r, math.sin(e) * r, math.cos(a) * math.cos(e) * r)


# ---------------------------------------------------------------- the skeleton
PARENT = {
    'Hips': None, 'Spine': 'Hips', 'Chest': 'Spine', 'UpperChest': 'Chest', 'Neck': 'UpperChest', 'Head': 'Neck',
}
for side in ('Left', 'Right'):
    PARENT[side + 'Shoulder'] = 'UpperChest'
    PARENT[side + 'UpperArm'] = side + 'Shoulder'
    PARENT[side + 'LowerArm'] = side + 'UpperArm'
    PARENT[side + 'Hand'] = side + 'LowerArm'
    PARENT[side + 'UpperLeg'] = 'Hips'
    PARENT[side + 'LowerLeg'] = side + 'UpperLeg'
    PARENT[side + 'Foot'] = side + 'LowerLeg'
    PARENT[side + 'Toes'] = side + 'Foot'
ORDER = list(PARENT.keys())
REST = {b: tuple(RIG[b[0].lower() + b[1:]]['pos']) for b in ORDER}

SOLE = 0.0
TOE_HEIGHT = REST['LeftToes'][1]
ANKLE_FROM_TOE = sub(REST['LeftFoot'], REST['LeftToes'])
# Contact landmarks measured on the flat sole of the hero's bind mesh.
HEEL_FROM_TOE = (0.0, -TOE_HEIGHT, -0.159)
TIP_FROM_TOE = (0.0, -TOE_HEIGHT, 0.056)

# The blade, in the hand's rest frame: a grip across the fist, leaving on the thumb side and leaning 22
# degrees toward the fingers, as a closed fist holds it. The edge continues the knuckles.
GRIP_R = (-0.065, -0.012, 0.0)
BLADE_R = norm((-0.375, 0.0, 0.927))
EDGE_R = norm((-0.927, 0.0, -0.375))
GRIP_L = (0.065, -0.012, 0.0)
BLADE_L = norm((0.375, 0.0, 0.927))
EDGE_L = norm((0.927, 0.0, -0.375))
BLADE_LENGTH = 0.9


class Pose:
    def __init__(self):
        self.hips = (0.0, 0.0, 0.0)
        self.turns = {b: QI for b in ORDER}
        self.pos = {}
        self.rot = {}

    def fk(self):
        pos, rot = {}, {}
        for b in ORDER:
            p = PARENT[b]
            if p is None:
                rot[b] = self.turns[b]
                pos[b] = add(REST[b], self.hips)
            else:
                rot[b] = qnorm(qmul(rot[p], self.turns[b]))
                pos[b] = add(pos[p], qrot(rot[p], sub(REST[b], REST[p])))
        self.pos, self.rot = pos, rot
        return self

    def grip(self):
        return add(self.pos['RightHand'], qrot(self.rot['RightHand'], GRIP_R))

    def blade_dir(self):
        return qrot(self.rot['RightHand'], BLADE_R)

    def tip(self):
        return add(self.grip(), mul(self.blade_dir(), BLADE_LENGTH))

    def sole(self, side):
        """Ball, heel and tip of the shoe, in model space."""
        toes = self.pos[side + 'Toes']
        foot_rot, toe_rot = self.rot[side + 'Foot'], self.rot[side + 'Toes']
        return (add(toes, qrot(toe_rot, (0.0, -TOE_HEIGHT, 0.0))),
                add(toes, qrot(foot_rot, HEEL_FROM_TOE)),
                add(toes, qrot(toe_rot, TIP_FROM_TOE)))

def limb_vectors(start, rest_upper, rest_lower, target, pole, fallback, reach=0.998):
    """Return the solved segment displacements and any distance missed at the target."""
    upper_length, lower_length = length(rest_upper), length(rest_lower)
    total = upper_length + lower_length
    line = sub(target, start)
    distance = length(line)
    direction = norm(line) if distance > 1e-9 else norm(add(rest_upper, rest_lower))
    folded = abs(upper_length - lower_length) + 0.012 * min(upper_length, lower_length)
    extension = max(folded, total * min(reach, 0.999999))
    end = min(max(distance, folded), extension)
    # As the requested pole goes through the reach axis it has no usable direction.
    # Ease toward the anatomical fallback across that region to avoid a hinge flip.
    bend = plane_direction(pole, direction, fallback, 0.24)

    along = (upper_length**2 + end**2 - lower_length**2) / (2.0 * end)
    middle = add(mul(direction, along), mul(bend, math.sqrt(max(0.0, upper_length**2 - along**2))))
    return middle, sub(mul(direction, end), middle), abs(distance - end)


def reach_chain(start, rest_upper, rest_lower, target, pole, hinge, fallback, reach=0.998):
    """Solve two rigid segments in one plane. `hinge` is the signed rest flexion axis.

    The pole and its fallback are model-space directions from the shoulder-to-wrist or
    hip-to-ankle line. The result is two model-space rotations from bind and the distance
    from the requested target that the chain could not reach.
    """
    upper, lower, miss = limb_vectors(start, rest_upper, rest_lower, target, pole, fallback, reach)
    axis = cross(upper, lower)
    r_upper = between_frames(frame(rest_upper, hinge), frame(upper, axis))
    r_lower = between_frames(frame(rest_lower, hinge), frame(lower, axis))
    return r_upper, r_lower, miss


def solve_chain(pose, upper, lower, end, target, pole, bend_away):
    """Solve a hero limb with a signed anatomical flexion axis from the bind rig."""
    pose.fk()
    r1 = sub(REST[lower], REST[upper])
    r2 = sub(REST[end], REST[lower])
    if bend_away:
        # The elbow is almost straight in bind. The thumb side of the hand gives the
        # flexion plane independently of the small, asymmetric bind offsets.
        side = 'left' if upper.startswith('Left') else 'right'
        radial = sub(RIG[side + 'IndexProximal']['pos'], RIG[side + 'LittleProximal']['pos'])
        hinge = cross(r1, radial)
        fallback = qrot(pose.rot['UpperChest'], (0.0, -0.65, -1.0))
    else:
        hinge = cross(r1, r2)
        fallback = qrot(pose.rot['Hips'], (0.0, 0.0, 1.0))
    return reach_chain(pose.pos[upper], r1, r2, target, pole, hinge, fallback)


def set_rot(pose, bone, world_rot):
    """Sets `bone`'s turn so its rotation is `world_rot` (its parent's already placed)."""
    parent = PARENT[bone]
    parent_rot = pose.rot[parent] if parent else QI
    pose.turns[bone] = qnorm(qmul(qinv(parent_rot), world_rot))
    pose.fk()


class Foot:
    """Where a foot is: its ball's ground position, its yaw, how far its heel is up (degrees, about the
    ball), how far it's off the ground, and how far it rolls."""

    def __init__(self, x, z, yaw=0.0, pitch=0.0, lift=0.0, roll=0.0, toe=None, planted=None,
                 model=False, pivot=None):
        self.x, self.z, self.yaw, self.pitch, self.lift, self.roll = x, z, yaw, pitch, lift, roll
        self.toe = toe  # the toes' own bend from the foot, degrees of x; None keeps them flat on the ground
        self.planted = (lift == 0.0 and not model) if planted is None else planted
        self.model = model  # x and z and yaw are in model space, whatever the root does
        if pivot not in (None, 'heel', 'ball'):
            raise ValueError(f"Unknown foot pivot {pivot!r}")
        self.pivot = pivot

    def rotation(self, root_turn=0.0):
        turned = 0.0 if self.model else root_turn
        return qnorm(qmul(qaxis((0, 1, 0), self.yaw - turned),
                          qmul(qaxis((0, 0, 1), self.roll), qaxis((1, 0, 0), self.pitch))))

    def toes_rotation(self, root_turn=0.0):
        turned = 0.0 if self.model else root_turn
        if self.toe is not None:
            return qnorm(qmul(self.rotation(root_turn), qaxis((1, 0, 0), self.toe)))
        if self.pivot == 'heel':
            return self.rotation(root_turn)
        if self.lift > 0.0:
            return qnorm(qmul(self.rotation(root_turn), qaxis((1, 0, 0), -min(self.pitch, 20.0) * 0.5)))
        return qaxis((0, 1, 0), self.yaw - turned)

    def ankle(self, side, root=(0.0, 0.0, 0.0), root_turn=0.0):
        """Ankle in model space, with the chosen heel or ball on the authored sole point."""
        ankle_from_toe = sub(REST[side + 'Foot'], REST[side + 'Toes'])
        rotation = self.rotation(root_turn)
        height = REST[side + 'Toes'][1]
        if self.pivot == 'heel':
            at = (self.x, self.lift, self.z)
            offset = qrot(rotation, sub(ankle_from_toe, HEEL_FROM_TOE))
        elif self.pivot == 'ball':
            at = (self.x, self.lift, self.z)
            offset = add(qrot(rotation, ankle_from_toe),
                         qrot(self.toes_rotation(root_turn), (0.0, height, 0.0)))
        else:
            # Existing clips author the toe joint, with its bind height above the floor.
            at = (self.x, height + self.lift, self.z)
            offset = qrot(rotation, ankle_from_toe)
        return add(at if self.model else to_model(at, root, root_turn), offset)


def leading_edge(az, lean):
    """An edge hint for a blade sweeping round toward higher azimuth: leading along the sweep, and `lean`
    degrees down from it, which shows a horizontal cut's flat to a camera at chest height."""
    a, l = math.radians(az), math.radians(lean)
    return (math.cos(a) * math.cos(l), -math.sin(l), -math.sin(a) * math.cos(l))


def to_model(world, root, root_turn):
    """A point in the move's start space into model space at this root."""
    return qrot(qaxis((0, 1, 0), -root_turn), sub(world, root))


def solve_leg(pose, side, foot, root, root_turn, knee=None):
    q = foot.rotation(root_turn)
    ankle = foot.ankle(side, root, root_turn)
    turned = 0.0 if foot.model else root_turn
    pole = knee if knee is not None else qrot(qaxis((0, 1, 0), foot.yaw - turned), (0.0, 0.0, 1.0))
    r_up, r_low, short = solve_chain(pose, side + 'UpperLeg', side + 'LowerLeg', side + 'Foot', ankle, pole, False)
    set_rot(pose, side + 'UpperLeg', r_up)
    set_rot(pose, side + 'LowerLeg', r_low)
    set_rot(pose, side + 'Foot', q)
    set_rot(pose, side + 'Toes', foot.toes_rotation(root_turn))
    return short


def hand_frame(blade, edge_hint, blade_rest, edge_rest, fallback=None):
    b = norm(blade)
    e = plane_direction(edge_hint, b, fallback if fallback is not None else perpendicular(b), 0.24)
    return between_frames((blade_rest, edge_rest, cross(blade_rest, edge_rest)), (b, e, cross(b, e)))


def forearm_twist(lower_rot, hand_rot, axis, share=0.5):
    """Move grip roll into the forearm without changing branches at an opposed palm."""
    axis = norm(axis)
    relative = qmul(hand_rot, qinv(lower_rot))
    along, scalar = dot(relative[:3], axis), relative[3]
    # Sine of the axial angle. The response is proportional near neutral and tapers off
    # when the target asks for a palm reversal, where the shortest signed angle jumps.
    magnitude = scalar * scalar + along * along
    sine = 2.0 * along * scalar / magnitude if magnitude > 1e-9 else 0.0
    twist = min(75.0, max(-75.0, math.degrees(sine) * share))
    return qnorm(qmul(qaxis(axis, twist), lower_rot))


def set_grip(pose, side, blade, edge, blade_rest, edge_rest):
    hand, lower = side + 'Hand', side + 'LowerArm'
    forearm = norm(sub(pose.pos[hand], pose.pos[lower]))
    lower_rot = pose.rot[lower]
    rotation = hand_frame(blade, edge if edge is not None else forearm, blade_rest, edge_rest,
                          qrot(lower_rot, edge_rest))
    set_rot(pose, lower, forearm_twist(lower_rot, rotation, forearm))
    set_rot(pose, hand, rotation)


def solve_right_arm(pose, wrist, elbow, blade=None, edge=None, hand=None):
    r_up, r_low, short = solve_chain(pose, 'RightUpperArm', 'RightLowerArm', 'RightHand', wrist, elbow, True)
    set_rot(pose, 'RightUpperArm', r_up)
    set_rot(pose, 'RightLowerArm', r_low)
    if blade is not None:
        set_grip(pose, 'Right', blade, edge, BLADE_R, EDGE_R)
    elif hand is not None:
        pose.turns['RightHand'] = hand
        pose.fk()
    return short


def solve_left_arm(pose, wrist, elbow, hand=None, grip=False):
    r_up, r_low, short = solve_chain(pose, 'LeftUpperArm', 'LeftLowerArm', 'LeftHand', wrist, elbow, True)
    set_rot(pose, 'LeftUpperArm', r_up)
    set_rot(pose, 'LeftLowerArm', r_low)
    if grip:
        set_grip(pose, 'Left', pose.blade_dir(), None, BLADE_L, EDGE_L)
    elif hand is not None:
        pose.turns['LeftHand'] = hand
        pose.fk()
    return short


# ---------------------------------------------------------------- eases, as KeyEase::weight
def back(u, s): return (s + 1.0) * u * u * u - s * u * u
def back_strength(share):
    dip = lambda s: 4.0 * s * s * s / (27.0 * (s + 1.0) * (s + 1.0))
    low, high = 0.0, 32.0
    for _ in range(32):
        m = (low + high) * 0.5
        if dip(m) < share:
            low = m
        else:
            high = m
    return (low + high) * 0.5
def weight(ease, u, strength=None):
    u = max(0.0, min(1.0, u))
    if u <= 0.0:
        return 0.0
    if u >= 1.0:
        return 1.0
    kind = ease[0] if isinstance(ease, tuple) else ease
    if kind == 'Smooth':
        return u * u * (3 - 2 * u)
    if kind == 'Linear':
        return u
    if kind == 'In':
        return u * u
    if kind == 'Out':
        return u * (2 - u)
    if kind == 'Hold':
        return 0.0
    if kind == 'Anticipate':
        return back(u, back_strength(ease[1]) if strength is None else strength)
    if kind == 'Overshoot':
        return 1.0 - back(1.0 - u, back_strength(ease[1]) if strength is None else strength)
    if kind == 'Curve':
        _, x1, y1, x2, y2 = ease
        bez = lambda t, a, b: 3 * (1 - t) * (1 - t) * t * a + 3 * (1 - t) * t * t * b + t * t * t
        low, high = 0.0, 1.0
        for _ in range(24):
            m = (low + high) * 0.5
            if bez(m, x1, x2) < u:
                low = m
            else:
                high = m
        return bez((low + high) * 0.5, y1, y2)
    raise ValueError(ease)
def ease_text(ease):
    if isinstance(ease, tuple):
        return f"{ease[0]}({', '.join(fmt(v, 3) for v in ease[1:])})"
    return ease


def fmt(v, places=1):
    s = f"{v:.{places}f}"
    if '.' in s:
        s = s.rstrip('0').rstrip('.')
    if s in ('-0', ''):
        s = '0'
    return s


# ---------------------------------------------------------------- a move, as a list of solved keys
class Move:
    def __init__(self, name, ticks, comment):
        self.name, self.ticks, self.comment = name, ticks, comment
        self.keys = []
        self.feet = {}

    def key(self, tick, say, ease='Smooth', root=None, root_turn=None, hips=None, body=None,
            L=None, R=None, knee_L=None, knee_R=None, look=None, neck_share=0.42, fingers=None,
            rh=None, elbow_r=None, blade=None, edge=None, lh=None, elbow_l=None, lhand=None, two_hands=False,
            shoulder_r=None, shoulder_l=None, raw=None, arms_in_chest=False):
        previous = self.keys[-1] if self.keys else None
        pose = Pose()
        if previous:
            pose.hips = previous['pose'].hips
            pose.turns = dict(previous['pose'].turns)
        root = root if root is not None else (previous['root'] if previous else (0.0, 0.0, 0.0))
        root_turn = root_turn if root_turn is not None else (previous['root_turn'] if previous else 0.0)
        if hips is not None:
            pose.hips = hips
        for bone, values in (body or {}).items():
            pose.turns[bone] = turn(*values)
        if shoulder_r is not None:
            pose.turns['RightShoulder'] = turn(*shoulder_r)
        if shoulder_l is not None:
            pose.turns['LeftShoulder'] = turn(*shoulder_l)
        pose.fk()
        L = L if L is not None else (previous['L'] if previous else None)
        R = R if R is not None else (previous['R'] if previous else None)
        knee_L = knee_L if knee_L is not None else (previous['knee_L'] if previous else None)
        knee_R = knee_R if knee_R is not None else (previous['knee_R'] if previous else None)
        problems = []
        for side, foot, knee in (('Left', L, knee_L), ('Right', R, knee_R)):
            if foot is None:
                continue
            short = solve_leg(pose, side, foot, root, root_turn, knee)
            if short > 0.001:
                problems.append(f"{side} leg short by {short*100:.1f} cm")
        # Head: looks at (az, el) in model space, whatever the chest does.
        if look is not None:
            want = qmul(qaxis((0, 1, 0), look[0]), qaxis((1, 0, 0), -look[1]))
            if len(look) > 2:
                want = qmul(want, qaxis((0, 0, 1), look[2]))
            pose.fk()
            base = pose.rot['UpperChest']
            total = qmul(qinv(base), want)
            pose.turns['Neck'] = slerp(QI, total, neck_share)
            pose.fk()
            pose.turns['Head'] = qnorm(qmul(qinv(pose.rot['Neck']), want))
        pose.fk()
        # Most moves aim in model space. A chest-relative carry lets a moving ribcage
        # lead the upper arm while the hilt and blade have independently authored lag.
        arm_frame = pose.rot['UpperChest'] if arms_in_chest else QI
        shoulder = pose.pos['RightUpperArm']
        if rh is not None:
            wrist = add(shoulder, qrot(arm_frame, polar(*rh)))
            pole = qrot(arm_frame, elbow_r if elbow_r is not None else (-0.5, -0.8, -0.3))
            direction = qrot(arm_frame, polar(*blade)) if blade is not None else None
            short = solve_right_arm(pose, wrist, pole, direction,
                                    qrot(arm_frame, edge) if edge is not None else None)
            if short > 0.001:
                problems.append(f"right arm short by {short*100:.1f} cm")
        pose.fk()
        shoulder = pose.pos['LeftUpperArm']
        if two_hands:
            # Behind the right fist, along the grip.
            at = add(pose.grip(), mul(pose.blade_dir(), -0.09))
            wrist = at
            pole = qrot(arm_frame, elbow_l if elbow_l is not None else (0.6, -0.8, -0.2))
            short = 0.0
            for _ in range(3):
                short = solve_left_arm(pose, wrist, pole, grip=True)
                wrist = sub(at, qrot(pose.rot['LeftHand'], GRIP_L))
            if short > 0.001:
                problems.append(f"left arm short by {short*100:.1f} cm")
        elif lh is not None:
            wrist = add(shoulder, qrot(arm_frame, polar(*lh)))
            pole = qrot(arm_frame, elbow_l if elbow_l is not None else (0.5, -0.8, -0.3))
            short = solve_left_arm(pose, wrist, pole, hand=lhand)
            if short > 0.001:
                problems.append(f"left arm short by {short*100:.1f} cm")
        for bone, values in (raw or {}).items():
            pose.turns[bone] = turn(*values)
        pose.fk()
        euler = {b: to_euler(pose.turns[b]) for b in ORDER}
        rounded = {bone: turn(*(round(v, 1) for v in euler[bone])) for bone in ORDER}
        strength = (back_strength(ease[1])
                    if isinstance(ease, tuple) and ease[0] in ('Anticipate', 'Overshoot') else None)
        self.keys.append(dict(tick=tick, say=say, ease=ease, root=root, root_turn=root_turn, pose=pose, L=L, R=R,
                              knee_L=knee_L, knee_R=knee_R, euler=euler, fingers=fingers or {},
                              rounded=rounded, strength=strength))
        for p in problems:
            print(f"  ! {self.name} tick {tick}: {p}")
        return pose

    # ---- sampling, as keypose::sample does
    def sample(self, t):
        keys = self.keys
        i = 0
        while i + 1 < len(keys) and keys[i + 1]['tick'] <= t:
            i += 1
        if i + 1 == len(keys):
            a = keys[i]
            pose = Pose()
            pose.hips = a['pose'].hips
            pose.turns = dict(a['rounded'])
            return pose.fk(), a['root'], a['root_turn']
        a, b = keys[i], keys[i + 1]
        u = (t - a['tick']) / (b['tick'] - a['tick'])
        w = weight(b['ease'], u, b['strength'])
        pose = Pose()
        pose.hips = lerp(a['pose'].hips, b['pose'].hips, w)
        for bone in ORDER:
            # What the document holds: rounded Euler turns.
            qa = a['rounded'][bone]
            qb = b['rounded'][bone]
            pose.turns[bone] = slerp(qa, qb, w)
        return pose.fk(), lerp(a['root'], b['root'], w), a['root_turn'] + (b['root_turn'] - a['root_turn']) * w

    def world(self, p, root, root_turn):
        return add(root, qrot(qaxis((0, 1, 0), root_turn), p))

    # ---- what a look at the numbers can catch
    def check(self, verbose=False, slip_limit=0.03):
        print(f"--- {self.name}: {self.ticks} ticks")
        rows = []
        for t in range(self.ticks + 1):
            pose, root, rt = self.sample(t)
            row = dict(t=t, pose=pose, root=root, rt=rt)
            row['tip'] = self.world(pose.tip(), root, rt)
            row['grip'] = self.world(pose.grip(), root, rt)
            for side in ('Left', 'Right'):
                row[side] = tuple(self.world(p, root, rt) for p in pose.sole(side))
                row[side + 'Toes'] = self.world(pose.pos[side + 'Toes'], root, rt)
            rows.append(row)
        # Below the ground, and where the feet are.
        low = {}
        for row in rows:
            for side in ('Left', 'Right'):
                sole = min(point[1] for point in row[side])
                if sole < -0.01:
                    low.setdefault(side, []).append((row['t'], round(sole, 3)))
        for side, items in low.items():
            print(f"  below ground {side}: {items[:8]}")
        # Slip while planted.
        for side, key_name in (('Left', 'L'), ('Right', 'R')):
            for a, b in zip(self.keys, self.keys[1:]):
                fa, fb = a[key_name], b[key_name]
                if fa is None or fb is None or not (fa.planted and fb.planted):
                    continue
                if (fa.x, fa.z, fa.pivot) != (fb.x, fb.z, fb.pivot):
                    continue
                worst = 0.0
                for t in range(a['tick'], b['tick'] + 1):
                    row = rows[t]
                    contact = row[side][int(fa.pivot == 'heel')] if fa.pivot else row[side + 'Toes']
                    worst = max(worst, math.hypot(contact[0] - fa.x, contact[2] - fa.z))
                if worst > slip_limit:
                    print(f"  slip {side} {a['tick']}..{b['tick']}: {worst*100:.1f} cm")
        # Joint angles against human limits.
        worst = {}
        for row in rows:
            pose = row['pose']
            for side in ('Left', 'Right'):
                for joint, upper, lower, end in (('elbow', 'UpperArm', 'LowerArm', 'Hand'),
                                                 ('knee', 'UpperLeg', 'LowerLeg', 'Foot')):
                    a, b, c = (pose.pos[side + bone] for bone in (upper, lower, end))
                    name = side + ' ' + joint
                    worst[name] = max(worst.get(name, 0), angle_between(sub(b, a), sub(c, b)))
                forearm = sub(REST[side + 'Hand'], REST[side + 'LowerArm'])
                flex = angle_between(forearm, qrot(pose.turns[side + 'Hand'], forearm))
                worst[side + ' wrist'] = max(worst.get(side + ' wrist', 0), flex)
        print('  max ' + ', '.join(f"{k} {v:.0f}" for k, v in sorted(worst.items())))
        most = {}
        for key in self.keys:
            for bone in ORDER:
                w = max(-1.0, min(1.0, key['pose'].turns[bone][3]))
                angle = 2 * math.degrees(math.acos(abs(w)))
                most[bone] = max(most.get(bone, 0.0), angle)
        print('  turns ' + ', '.join(f"{b} {v:.0f}" for b, v in most.items() if v > 0))
        hits = []
        lowest = (9.0, 0)
        for row in rows:
            pose = row['pose']
            grip, tip = pose.grip(), pose.tip()
            b = norm(sub(tip, grip))
            a0 = add(grip, mul(b, 0.14))
            if tip[1] < lowest[0]:
                lowest = (tip[1], row['t'])
            for name, p0, p1, radius in body_capsules(pose):
                d = segment_distance(a0, tip, p0, p1)
                if d < radius:
                    hits.append((row['t'], name, round(d - radius, 3)))
        if hits:
            summary = {}
            for t, name, depth in hits:
                summary.setdefault(name, []).append(t)
            print('  blade through ' + ', '.join(f"{n}@{v[0]}..{v[-1]}" for n, v in summary.items()))
        print(f"  lowest tip {lowest[0]:.2f} at {lowest[1]}")
        if verbose:
            for row in rows:
                tip, grip = row['tip'], row['grip']
                speed = 0.0
                if row['t'] > 0:
                    speed = length(sub(tip, rows[row['t'] - 1]['tip']))
                print(f"  {row['t']:3d} root {row['root'][2]:+.2f} hipy {row['pose'].pos['Hips'][1]:.2f} "
                      f"grip {grip[0]:+.2f} {grip[1]:+.2f} {grip[2]:+.2f}  tip {tip[0]:+.2f} {tip[1]:+.2f} {tip[2]:+.2f} "
                      f"v {speed:.2f}  L {row['Left'][0][2]:+.2f} R {row['Right'][0][2]:+.2f}")
        return rows

    # ---- the document
    def ron(self):
        out = [line.rstrip() for line in self.comment.strip('\n').split('\n')]
        out = ['// ' + line if line else '//' for line in out]
        out += ['(', '    type: "sapphire::pipeline::KeyPoseClip",', '    version: 1,', '    data: (',
                f'        ticks: {self.ticks},', '        keys: [']
        held = {}
        held_hips, held_root, held_turn = None, None, None
        for n, key in enumerate(self.keys):
            if n:
                out.append('')
            for line in key['say'].strip().splitlines():
                out.append('            // ' + line.strip())
            head = [f"tick: {key['tick']}"]
            if n and key['ease'] != 'Smooth':
                head.append('ease: ' + ease_text(key['ease']))
            root3 = tuple(round(v, 3) for v in key['root'])
            if root3 != (held_root or (0.0, 0.0, 0.0)):
                head.append(f"root: ({', '.join(fmt(v, 3) for v in root3)})")
                held_root = root3
            rt = round(key['root_turn'], 1)
            if rt != (held_turn or 0.0):
                head.append(f"root_turn: {fmt(rt, 1)}")
                held_turn = rt
            hips3 = tuple(round(v, 3) for v in key['pose'].hips)
            if hips3 != (held_hips or (0.0, 0.0, 0.0)):
                head.append(f"hips: ({', '.join(fmt(v, 3) for v in hips3)})")
                held_hips = hips3
            bones = []
            for bone in BONE_ORDER:
                e = tuple(round(v, 1) for v in key['euler'][bone])
                if e != held.get(bone, (0.0, 0.0, 0.0)):
                    held[bone] = e
                    parts = [f"{axis}: {fmt(v)}" for axis, v in zip('xyz', e) if v != 0.0]
                    bones.append((bone, f"{bone}: ({', '.join(parts)}),"))
            fingers = key['fingers']
            lines = [f"            ({', '.join(head)}, pose: {{"]
            group_last = None
            for bone, text in bones:
                group = GROUP[bone]
                if group_last is not None and group != group_last and n == 0:
                    lines.append('')
                group_last = group
                lines.append('                ' + text)
            if fingers:
                lines.append('')
                for bone, values in fingers.items():
                    parts = [f"{axis}: {fmt(v)}" for axis, v in zip('xyz', values) if v != 0.0]
                    lines.append(f"                {bone}: ({', '.join(parts)}),")
            morphs = key.get('morphs', {})
            if morphs:
                weights = ', '.join(f"{json.dumps(name)}: {fmt(value, 6)}" for name, value in morphs.items())
                lines.append(f"            }}, morphs: {{{weights}}}),")
            else:
                lines.append('            }),')
            out += lines
        out += ['        ],', '    ),', ')', '']
        return '\n'.join(out)


def segment_distance(a0, a1, b0, b1):
    u, v, w = sub(a1, a0), sub(b1, b0), sub(a0, b0)
    a, b, c, d, e = dot(u, u), dot(u, v), dot(v, v), dot(u, w), dot(v, w)
    denom = a * c - b * b
    s = 0.0 if denom < 1e-9 else max(0.0, min(1.0, (b * e - c * d) / denom))
    t = 0.0 if c < 1e-9 else max(0.0, min(1.0, (b * s + e) / c))
    s = 0.0 if a < 1e-9 else max(0.0, min(1.0, (b * t - d) / a))
    return length(sub(add(a0, mul(u, s)), add(b0, mul(v, t))))


def body_capsules(pose):
    p = pose.pos
    head = add(p['Head'], qrot(pose.rot['Head'], (0.0, 0.09, 0.01)))
    return [
        ('torso', p['Hips'], p['Spine'], 0.13),
        ('torso', p['Spine'], p['UpperChest'], 0.13),
        ('torso', p['UpperChest'], p['Neck'], 0.12),
        ('head', head, head, 0.12),
        ('left thigh', p['LeftUpperLeg'], p['LeftLowerLeg'], 0.075),
        ('right thigh', p['RightUpperLeg'], p['RightLowerLeg'], 0.075),
        ('left shin', p['LeftLowerLeg'], p['LeftFoot'], 0.05),
        ('right shin', p['RightLowerLeg'], p['RightFoot'], 0.05),
        ('left arm', p['LeftUpperArm'], p['LeftLowerArm'], 0.04),
        ('left forearm', p['LeftLowerArm'], p['LeftHand'], 0.035),
    ]


GROUP = {}
BONE_ORDER = []
for group, bones in [
    ('body', ['Hips', 'Spine', 'Chest', 'UpperChest', 'Neck', 'Head']),
    ('legs', ['LeftUpperLeg', 'LeftLowerLeg', 'LeftFoot', 'LeftToes', 'RightUpperLeg', 'RightLowerLeg', 'RightFoot', 'RightToes']),
    ('arms', ['LeftShoulder', 'LeftUpperArm', 'LeftLowerArm', 'LeftHand', 'RightShoulder', 'RightUpperArm', 'RightLowerArm', 'RightHand']),
]:
    for bone in bones:
        GROUP[bone] = group
        BONE_ORDER.append(bone)
