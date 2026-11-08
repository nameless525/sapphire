"""Author the warrior's contact poses. Run with --write to update the enemy's key-pose assets."""

import argparse
import json
import math
from pathlib import Path

from guard_clearance import penalty as guard_clearance_penalty, floor_penalty, defeat_approach, swipe_approach
import re
import struct
import uuid

from pose import (
    QI, add, cross, dot, forearm_twist, length, lerp, limb_vectors, mul, norm, qaxis,
    qinv, qmul, qnorm, qrot, reach_chain, reject, slerp, sub, to_euler, turn,
)


FOLDER = Path(__file__).resolve().parents[2] / 'content/combat/enemy'
RIG_ID = '322aa57e-957d-45f1-8d51-0c1d80f5d1d6'
SCALE = 0.8
X, Y, Z = (1, 0, 0), (0, 1, 0), (0, 0, 1)

# The rig's model-space chain. Unanimated helper joints are folded into the offsets.
BONES = {
    'Hips': ('hips', None),
    'Spine': ('spine', 'Hips'),
    'Chest': ('chest', 'Spine'),
    'Head': ('head', 'Chest'),
}
for side, suffix in (('Left', '.l'), ('Right', '.r')):
    for stem, name, parent in (
        ('UpperLeg', 'upperleg', 'Hips'), ('LowerLeg', 'lowerleg', side + 'UpperLeg'),
        ('Foot', 'foot', side + 'LowerLeg'), ('Toes', 'toes', side + 'Foot'),
        ('UpperArm', 'upperarm', 'Chest'), ('LowerArm', 'lowerarm', side + 'UpperArm'),
        ('Hand', 'wrist', side + 'LowerArm'),
    ):
        BONES[side + stem] = (name + suffix, parent)


def read_rig():
    data = (FOLDER / 'warrior.glb').read_bytes()
    json_size = struct.unpack_from('<I', data, 12)[0]
    nodes = json.loads(data[20:20 + json_size])['nodes']
    parents = {child: i for i, node in enumerate(nodes) for child in node.get('children', [])}

    def place(i):
        node = nodes[i]
        pos, rot = tuple(node.get('translation', (0, 0, 0))), tuple(node.get('rotation', QI))
        if i not in parents:
            return pos, rot
        parent, parent_rot = place(parents[i])
        return add(parent, qrot(parent_rot, pos)), qnorm(qmul(parent_rot, rot))

    rest = {node['name']: place(i) for i, node in enumerate(nodes)}
    joints = {bone: rest[name][0] for bone, (name, _) in BONES.items()}
    slots = {side: rest['handslot.' + side[0].lower()] for side in ('Left', 'Right')}
    return joints, slots


REST, SLOTS = read_rig()
SLOT_OFFSET = {side: sub(pos, REST[side + 'Hand']) for side, (pos, _) in SLOTS.items()}
# Bottom profiles measured on the foot and toe pieces in the bind mesh.
HEEL = (0, 0, -.144)
BALL = (0, .022, .151)
FOOT_SOLE = ((0, .001, -.144), (0, .01, -.05), (0, .019, .017), BALL)
TOE_SOLE = ((0, .022, .116), (0, .022, .24), (0, .052, .298))


def angle_about(axis, source, target):
    return math.degrees(math.atan2(dot(axis, cross(source, target)), dot(source, target)))


def aim_rotation(source, target):
    cosine = dot(source, target)
    if cosine < -.9999:
        fallback = Y if abs(source[1]) < .9 else X
        return qaxis(norm(cross(source, fallback)), 180)
    return qnorm((*cross(source, target), 1 + cosine))


def foot_contact(side, target, root):
    x, z, lift, yaw, pitch = target
    ankle, toe = REST[side + 'Foot'], REST[side + 'Toes']
    ankle_to_toe = sub(toe, ankle)
    center = (ankle[0], 0, 0)
    rotation = turn(pitch, yaw)
    toe_pitch = pitch if pitch <= 0 else pitch * (.12 + .52 * min(lift / .15, 1))
    toe_rotation = turn(toe_pitch, yaw)
    pivot = BALL if pitch > 0 else HEEL
    ankle_to_pivot = sub(ankle, add(center, pivot))
    target_ankle = add(add((x, 0, z - root), qrot(turn(0, yaw), pivot)), qrot(rotation, ankle_to_pivot))
    toe_offset = qrot(rotation, ankle_to_toe)
    clearance = min(
        min(qrot(rotation, sub(add(center, point), ankle))[1] for point in FOOT_SOLE),
        min(add(toe_offset, qrot(toe_rotation, sub(add(center, point), toe)))[1] for point in TOE_SOLE),
    )
    return (target_ankle[0], lift + .004 - clearance, target_ankle[2]), rotation, toe_rotation


class Pose:
    def __init__(self, hips, pelvis, spine, chest, head):
        self.hips = hips
        self.turns = {bone: QI for bone in BONES}
        for bone, angles in zip(('Hips', 'Spine', 'Chest', 'Head'), (pelvis, spine, chest, head)):
            self.turns[bone] = turn(*angles)
        self.pos, self.rot = {}, {}
        self.sword_target = None
        self.offhand = None
        self.release_pose = None
        self.free_target = None
        self.free_orientation = None
        self.roll_preference = .08
        self.cuff_clearance = 0
        self.cuff_approach = None
        self.fk()

    def fk(self):
        for bone, (_, parent) in BONES.items():
            if parent is None:
                self.pos[bone] = add(REST[bone], self.hips)
                self.rot[bone] = self.turns[bone]
            else:
                self.pos[bone] = add(self.pos[parent], qrot(self.rot[parent], sub(REST[bone], REST[parent])))
                self.rot[bone] = qmul(self.rot[parent], self.turns[bone])

    def set_world(self, bone, rotation):
        self.turns[bone] = qmul(qinv(self.rot[BONES[bone][1]]), rotation)
        self.fk()

    def reach(self, upper, lower, end, target, pole, fallback, reach=.981):
        start = self.pos[upper]
        rest_upper, rest_lower = sub(REST[lower], REST[upper]), sub(REST[end], REST[lower])
        # Bind-pose bend normals retain the front of each bone and mirror the arms.
        hinge = cross(rest_upper, rest_lower)
        upper_turn, lower_turn, _ = reach_chain(start, rest_upper, rest_lower, target, pole,
                                                hinge, fallback, reach)
        self.set_world(upper, upper_turn)
        self.set_world(lower, lower_turn)

    def boots(self, targets, root, knees_up=0):
        self.boot_targets = targets, root
        contacts = {side: foot_contact(side, target, root)
                    for side, target in zip(('Left', 'Right'), targets)}
        # Keep support within a comfortably flexed knee's reach. Solve pelvis height
        # before either ankle so stance contacts do not independently stretch or skate.
        drop = 0
        for side, target in zip(('Left', 'Right'), targets):
            if target[2] >= .085:
                continue
            upper, lower, foot = (side + part for part in ('UpperLeg', 'LowerLeg', 'Foot'))
            reach = .965 * (length(sub(REST[lower], REST[upper])) + length(sub(REST[foot], REST[lower])))
            from_ankle = sub(self.pos[upper], contacts[side][0])
            horizontal = math.hypot(from_ankle[0], from_ankle[2])
            max_height = math.sqrt(max(.26**2, reach**2 - horizontal**2))
            weight = 1 - min(1, target[2] / .085)
            drop = max(drop, (from_ankle[1] - max_height) * weight)
        self.hips = sub(self.hips, (0, min(.17, drop), 0))
        self.fk()

        for side, target in zip(('Left', 'Right'), targets):
            ankle, boot, toe = contacts[side]
            sign = 1 if side == 'Left' else -1
            toes_forward = qrot(turn(0, target[3]), Z)
            pelvis_forward = qrot(self.rot['Hips'], Z)
            pole = lerp(add(lerp(toes_forward, pelvis_forward, .34), (sign*.12, 0, 0)),
                        (sign*.26, 1, .39), knees_up)
            fallback = lerp(toes_forward, Y, knees_up)
            self.reach(side + 'UpperLeg', side + 'LowerLeg', side + 'Foot', ankle, pole, fallback)
            self.set_world(side + 'Foot', boot)
            self.set_world(side + 'Toes', toe)

    def arm(self, side, wrist, pole, rotation=None, reach=.981):
        sign = 1 if side == 'Left' else -1
        upper, lower, hand = (side + part for part in ('UpperArm', 'LowerArm', 'Hand'))
        fallback = qrot(self.rot['Chest'], (sign*.5, -.8, -.43))
        self.reach(upper, lower, hand, wrist, pole, fallback, reach)
        if rotation is None:
            self.set_world(hand, qmul(self.rot[lower], turn(0, sign*6, -sign*11)))
            return
        axis = norm(sub(self.pos[hand], self.pos[lower]))
        self.set_world(lower, forearm_twist(self.rot[lower], rotation, axis, share=.72))
        self.set_world(hand, rotation)

    def sword(self, grip, rotation, pole):
        self.sword_target = grip, rotation, pole

    def free_hand(self, wrist, pole):
        self.free_target = wrist, pole

    def elbow_pole(self, side, wrist, pole, previous, step_limit):
        shoulder, elbow, hand = (side + part for part in ('UpperArm', 'LowerArm', 'Hand'))
        axis = norm(sub(wrist, self.pos[shoulder]))
        preferred = reject(pole, axis)
        if previous is None:
            return preferred
        last_axis = norm(sub(previous.pos[hand], previous.pos[shoulder]))
        last_bend = reject(sub(previous.pos[elbow], previous.pos[shoulder]), last_axis)
        transported = qrot(aim_rotation(last_axis, axis), norm(last_bend))
        correction = angle_about(axis, transported, preferred) * min(1, length(preferred) * 2.5)
        return qrot(qaxis(axis, max(-step_limit, min(step_limit, correction))), transported)

    def solve_sword(self, previous):
        grip, authored_rotation, pole = self.sword_target
        slot_rotation = SLOTS['Right'][1]
        axis = qrot(authored_rotation, Y)
        shoulder = self.pos['RightUpperArm']
        upper = sub(REST['RightLowerArm'], REST['RightUpperArm'])
        lower = sub(REST['RightHand'], REST['RightLowerArm'])
        fallback = qrot(self.rot['Chest'], (-.65, -.75, -.48))
        palm_axis = norm(SLOT_OFFSET['Right'])
        clearance = self.cuff_clearance
        pole_step = 8 if clearance else 17
        elbow_steps = (-4, 0, 4) if clearance else (-16, -8, 0, 8, 16)
        if previous:
            _, last_blade = previous.blade()
            last_axis = qrot(last_blade, Y)
            swing = aim_rotation(last_axis, axis)
            blade = qmul(swing, last_blade)
            rolls = (-12, -8, -4, 0, 4, 8, 12) if clearance else (-28, -17, -8, -3, 0, 3, 8, 17, 28)
        else:
            blade, rolls = authored_rotation, range(-180, 180, 10)

        def candidate(roll, elbow_turn):
            weapon = qmul(qaxis(axis, roll), blade)
            hand = qmul(weapon, qinv(slot_rotation))
            wrist = sub(grip, qrot(hand, SLOT_OFFSET['Right']))
            reach_axis = norm(sub(wrist, shoulder))
            preferred_bend = reject(pole, reach_axis)
            bend = self.elbow_pole('Right', wrist, pole, previous, pole_step)
            bend = qrot(qaxis(reach_axis, elbow_turn), bend)
            first, second, miss = limb_vectors(shoulder, upper, lower, wrist, bend, fallback, .992)
            forearm = norm(second)
            elbow = add(shoulder, first)
            flex = 1 - dot(qrot(hand, palm_axis), forearm)
            roll_preference = 1 - dot(qrot(weapon, X), qrot(authored_rotation, X))
            fold = max(0, .31 - length(sub(wrist, shoulder)))
            score = 250 * miss * miss + 38 * fold * fold + 2.1 * flex
            score += self.roll_preference * roll_preference + (elbow_turn / 45)**2 * .25
            score += .72 * (1 - dot(norm(bend), norm(preferred_bend))) * min(1, length(preferred_bend))
            elbow_rise = max(0, elbow[1] - shoulder[1] - max(0, wrist[1] - shoulder[1]) * .65)
            score += 12 * elbow_rise * elbow_rise
            if previous:
                # Maintain one elbow line as the wrist crosses the chest or the arm
                # approaches full extension. The gameplay path fixes the blade axis.
                score += .12 * (roll / 28)**2
                score += .92 * length(sub(elbow, previous.pos['RightLowerArm']))
                last_forearm = norm(sub(previous.pos['RightHand'], previous.pos['RightLowerArm']))
                score += .72 * (1 - dot(forearm, last_forearm))
            if clearance:
                # Keep the guard fixed to the closed hand. Score the actual
                # forearm surface in that guard frame before choosing the pose.
                exact_fallback = qrot(self.rot['Chest'], (-.5, -.8, -.43))
                first, second, _ = limb_vectors(shoulder, upper, lower, wrist, bend, exact_fallback, .992)
                upper_turn, lower_turn, _ = reach_chain(shoulder, upper, lower, wrist, bend,
                                                        cross(upper, lower), exact_fallback, .992)
                lower_turn = forearm_twist(lower_turn, hand, norm(second), share=.72)
                elbow = add(shoulder, first)
                actual_wrist = add(elbow, second)
                actual_grip = add(actual_wrist, qrot(hand, SLOT_OFFSET['Right']))
                score += clearance * 95000 * guard_clearance_penalty(
                    shoulder, upper_turn, elbow, lower_turn, actual_grip, weapon)
                score += 320000 * floor_penalty(actual_grip, weapon, actual_wrist, hand)
                if self.cuff_approach:
                    blade_side, forearm_origin = self.cuff_approach
                    score += 8000 * (1 - dot(qrot(weapon, X), blade_side))
                    score += 16000 * length(sub(first, forearm_origin))**2
            return score, wrist, bend, hand

        _, wrist, bend, hand = min(
            (candidate(roll, elbow) for roll in rolls for elbow in elbow_steps),
            key=lambda choice: choice[0],
        )
        self.arm('Right', wrist, bend, hand, reach=.992)
        self.solve_free_hand(previous)

    def solve_free_hand(self, previous):
        target, pole = self.free_target
        weight = 0
        if self.offhand:
            weight = self.offhand
            grip, blade = self.blade()
            contact = add(grip, qrot(blade, (.01, -.22, 0)))
            wrist_turn = qmul(qmul(blade, qaxis(Y, 180)), qinv(SLOTS['Left'][1]))
            wrist = sub(contact, qrot(wrist_turn, SLOT_OFFSET['Left']))
            target = lerp(target, wrist, weight)
        bend = self.elbow_pole('Left', target, pole, previous, 13)
        self.arm('Left', target, bend)
        if weight == 0:
            if self.free_orientation is not None:
                contact_turn, contact_weight = self.free_orientation
                rotation = slerp(self.rot['LeftHand'], contact_turn, contact_weight)
                self.arm('Left', target, bend, rotation)
            return
        forearm = norm(sub(self.pos['LeftHand'], self.pos['LeftLowerArm']))
        release = previous is not None and weight < (previous.offhand or 0)
        if release:
            # Keep the twist and wrist bend from the last held frame as the hand
            # peels away. Re-aiming at the rolling blade can flip the wrist branch.
            self.release_pose = previous.release_pose
            if self.release_pose is None:
                upper = sub(previous.pos['LeftLowerArm'], previous.pos['LeftUpperArm'])
                lower = sub(previous.pos['LeftHand'], previous.pos['LeftLowerArm'])
                bind_hinge = norm(cross(sub(REST['LeftLowerArm'], REST['LeftUpperArm']),
                                        sub(REST['LeftHand'], REST['LeftLowerArm'])))
                hinge = qrot(previous.rot['LeftLowerArm'], bind_hinge)
                twist = angle_about(norm(lower), norm(cross(upper, lower)), hinge)
                self.release_pose = twist, previous.turns['LeftHand']
            twist, relative = self.release_pose
            lower_turn = qmul(qaxis(forearm, twist * weight), self.rot['LeftLowerArm'])
            self.set_world('LeftLowerArm', lower_turn)
            self.set_world('LeftHand', qmul(lower_turn, slerp(turn(0, 6, -11), relative, weight)))
            return

        free_turn = self.rot['LeftHand']
        hand_axis = qrot(wrist_turn, norm(SLOT_OFFSET['Left']))
        eased_axis = norm(lerp(forearm, hand_axis, weight))
        wrist_turn = qmul(aim_rotation(hand_axis, eased_axis), wrist_turn)
        self.arm('Left', target, bend, slerp(free_turn, wrist_turn, weight))

    def blade(self):
        rotation = self.rot['RightHand']
        return add(self.pos['RightHand'], qrot(rotation, SLOT_OFFSET['Right'])), qmul(rotation, SLOTS['Right'][1])


def curve(tick, keys):
    """A shape-preserving Hermite track. Contacts with equal neighbors stay pinned."""
    for i, ((first, a), (last, b)) in enumerate(zip(keys, keys[1:])):
        if tick > last:
            continue
        if tick <= first:
            return a
        duration = last - first
        u = (tick - first) / duration
        before, after = keys[max(0, i - 1)], keys[min(len(keys) - 1, i + 2)]

        def tangent(v0, v1, d0, d1):
            if d0 == 0 or d1 == 0 or v0 * v1 <= 0:
                return 0
            return (d0 + d1) / (d0 / v0 + d1 / v1)

        result = []
        for x, y, prior, following in zip(a, b, before[1], after[1]):
            d0, d1 = first - before[0], after[0] - last
            slope = (y - x) / duration
            left = tangent((x - prior) / (d0 or 1), slope, d0, duration)
            right = tangent(slope, (following - y) / (d1 or 1), duration, d1)
            result.append(x + (y - x) * (3 - 2 * u) * u * u
                          + duration * u * (1 - u) * ((1 - u) * left - u * right))
        return tuple(result)
    return keys[-1][1]


def linear(tick, keys, interpolate=lerp):
    for (first, a), (last, b) in zip(keys, keys[1:]):
        if tick <= last:
            return interpolate(a, b, max(0, tick - first) / (last - first))
    return keys[-1][1]


def nlerp(a, b, weight):
    if sum(x * y for x, y in zip(a, b)) < 0:
        b = tuple(-x for x in b)
    return qnorm(lerp(a, b, weight))


def move_data(name):
    """Read the authored timing, root travel and blade capsules without writing the move set."""
    source = (FOLDER / 'warrior.moves.ron').read_text()
    body = re.search(r'name: "' + name + r'",\s+startup: (\d+),\s+active: (\d+),\s+recovery: (\d+),(.*?)(?=\n            \),)', source, re.S)
    startup, active, recovery = (int(body[i]) for i in (1, 2, 3))
    offsets = r'\(at: (\d+), offset: \(([^)]+)\)(?:, rotation: \(([^)]+)\))?\)'
    root = [(0, (0, 0, 0))]
    blade_pos, blade_rot = [], []
    keys = re.search(r'root: \[(.*?)\]', body[4], re.S)
    for tick, offset, _ in re.findall(offsets, keys[1]):
        root.append((int(tick), tuple(float(v) / SCALE for v in offset.split(','))))
    keys = re.search(r'name: "blade",\s+keys: \[(.*?)\]', body[4], re.S)
    if keys:
        for tick, offset, rotation in re.findall(offsets, keys[1]):
            blade_pos.append((int(tick), tuple(float(v) for v in offset.split(','))))
            blade_rot.append((int(tick), qnorm(tuple(float(v) for v in rotation.split(','))) if rotation else QI))
    return startup, active, startup + active + recovery, root, blade_pos, blade_rot


# hip translation | pelvis | spine | chest | head. Each successive link can lag the one before it.
GUARD = (0.015, -.10, -.035, 6, -12, -3, 12, -8, 3, 8, -9, 2, -13, 26, -3)
GUARD_LEFT = (.37, .21, 0, 12, 0)
GUARD_RIGHT = (-.34, -.22, 0, -18, 0)
GUARD_GRIP = (-.56, 1.15, .32)
GUARD_SWORD = (31, -24, 24)
GUARD_FREE = (.53, 1.03, .36)
RIGHT_ELBOW = (-.30, -.75, -.72)
LEFT_ELBOW = (.35, -.75, -.68)


def rest_boot(boot, distance):
    return (boot[0], boot[1] + distance, *boot[2:])


def upright(body, boots, root, grip, sword, free, right_elbow=RIGHT_ELBOW, left_elbow=LEFT_ELBOW):
    pose = Pose(body[:3], body[3:6], body[6:9], body[9:12], body[12:15])
    pose.boots(boots, root)
    pose.sword(grip, sword, right_elbow)
    pose.free_hand(free, left_elbow)
    return pose


def authored(tick, body, left, right, grip, blade, free, root=0, right_elbow=None, left_elbow=None):
    return upright(curve(tick, body), (curve(tick, left), curve(tick, right)), root, grip, blade, free,
                   RIGHT_ELBOW if right_elbow is None else right_elbow,
                   LEFT_ELBOW if left_elbow is None else left_elbow)


def attacking(name, body, left, right, sword, free, right_elbow=None, left_elbow=None,
              two_hand=None, blade_roll=None, startup_blend=4, release_blend=5):
    startup, active, ticks, root, path, rotation = move_data(name)
    assert sword[0] == (0, GUARD_GRIP + GUARD_SWORD)
    assert sword[-1][1] == GUARD_GRIP + GUARD_SWORD

    def target(tick):
        center = linear(tick, path)
        blade = linear(tick, rotation, nlerp)
        if blade_roll:
            blade = qmul(blade, qaxis(Y, curve(tick, blade_roll)[0]))
        # The hit capsule spans the whole edge, starting about a hand above the crossguard.
        grip = mul(sub(center, qrot(blade, (0, .49, 0))), 1 / SCALE)
        return grip, blade

    def build(tick):
        grip_pose = curve(tick, sword)
        grip, blade = grip_pose[:3], turn(*grip_pose[3:])
        begin, end = startup - startup_blend, startup + active + release_blend
        if begin < tick < end:
            weight = min((tick - begin) / startup_blend, (end - tick) / release_blend, 1)
            weight = weight * weight * (3 - 2 * weight)
            hit_grip, hit_blade = target(tick)
            grip, blade = lerp(grip, hit_grip, weight), slerp(blade, hit_blade, weight)
        left_grip = curve(tick, free)
        offhand_pole = curve(tick, left_elbow) if left_elbow else LEFT_ELBOW
        pose = authored(tick, body, left, right, grip, blade, left_grip, linear(tick, root)[2],
                        right_elbow=curve(tick, right_elbow) if right_elbow else None,
                        left_elbow=offhand_pole)
        if name == 'lunge':
            pose.roll_preference = .9
        if two_hand:
            weight = curve(tick, two_hand)[0]
            if weight > 0:
                pose.offhand = weight
        return pose

    return ticks, build


def swipe():
    # Sit over the rear boot and coil across the ribs. The left foot catches the cut while
    # the left elbow trails behind, then the rear foot comes under the hips.
    # Return the hilt in front of the waist while the free hand clears the hip.
    total, build = attacking('swipe', [
        (0, GUARD), (8, (-.04,-.14,-.08, 7,-21,-5, 10,-12,2, 6,-13,4, -13,41,-2)),
        (15, (-.138,-.182,-.143, 6,-33,-9, 8,-14,2, 7,-18,4, -9,52,-3)),
        (18, (-.177,-.204,-.165, 4,-37,-10, 8,-17,2, 7,-23,4, -8,64,-4)),
        (23, (-.08,-.13,-.06, 8,-22,-4, 8,-16,0, 7,-19,5, -10,48,-5)),
        (26, (.025,-.18,-.13, 5,2,3, 6,-9,4, 8,-12,7, -9,18,-8)),
        (29, (.11,-.23,.025, 8,23,8, 9,16,3, 12,16,6, -19,-41,-8)),
        (34, (.17,-.25,.13, 9,40,8, 11,22,5, 13,14,4, -22,-51,-9)),
        (43, (.07,-.16,.09, 6,17,4, 13,8,1, 8,3,1, -13,-20,-5)),
        (56, GUARD), (60, GUARD),
    ], [
        (0, GUARD_LEFT), (16, GUARD_LEFT), (21, (.40,.37,.08,15,-14)),
        (25, (.43,.68,.089,19,-9)), (26, (.438,.792,.058,18,-7)),
        (27, (.44,.90,0,18,-5)), (29, (.44,.90,0,18,0)),
        (44, (.44,.90,0,18,0)), (49, (.40,.95,.07,17,8)), (56, rest_boot(GUARD_LEFT,.75)),
    ], [
        (0, GUARD_RIGHT), (20, GUARD_RIGHT), (25, (-.34,-.22,0,-18,24)),
        (27, (-.34,-.22,0,-18,30)),
        (29, (-.34,-.19,.035,-19,32)), (35, (-.33,.14,.16,-20,25)),
        (43, rest_boot(GUARD_RIGHT,.75)),
    ], [
        (0, GUARD_GRIP + GUARD_SWORD), (8, (-.62,1.17,.25,51,-43,-12)),
        (18, (-.81,1.20,-.24,84,-113,-11)), (22, (-.73,1.25,.15,90,-73,-4)),
        (30, (.65,1.04,.35,101,75,7)), (33, (.78,.876,.458,112,85,10)),
        (37, (.72,.84,.534,111,71,10.0000)),
        (42, (.429,.84,.73,107,49,10.0000)),
        (44, (.175,.863,.741,90.000,38,3)),
        (46, (-.10,.904,.704,86,28,-2)),
        (49, (-.35,.97,.49,76,13,-11)),
        (57, GUARD_GRIP + GUARD_SWORD),
    ], [
        (0, GUARD_FREE), (14, (.47,1.25,.58)), (22, (.41,1.34,.68)),
        (28, (.52,1.31,-.07)), (32, (.54,1.01,-.33)),
        (38, (.58,.96,-.22)),
        (44, (.62,.94,.02)),
        (50, (.56,.98,.24)), (57, GUARD_FREE),
    ], right_elbow=[(0, RIGHT_ELBOW), (24, (-.85,-.72,-.47)), (27, (-.45,-.72,-.75)),
                    (31, (.07,-.68,-.82)), (42, RIGHT_ELBOW)],
        left_elbow=[(0, LEFT_ELBOW), (26, (.36,-.86,-.44)), (33, (.77,-.80,-.10)),
                    (44, (.50,-.87,-.38)), (52, LEFT_ELBOW)], release_blend=3)

    def support_return(tick):
        pose = build(tick)
        lead, lower, lift = curve(tick, [
            (38, (0,0,0)), (41, (1,0,0)), (44, (1,0,0)),
            (46, (.45,.65,.30)), (49, (0,1,1)),
            (52, (0,.50,.65)), (56, (0,0,0)),
        ])
        if lead or lower or lift:
            head = pose.rot['Head']
            pose.set_world('Chest', qmul(turn(-4.0 * lead, 0, 0), pose.rot['Chest']))
            pose.set_world('Head', head)
            grip, blade, pole = pose.sword_target
            grip = add(grip, (-.035 * lift, -.015 * lower, .035 * lead + .06 * lift))
            pole = add(pole, (0, -.16 * lead - .4 * lower, .7 * lead - .32 * lower))
            pose.sword(grip, blade, pole)
        return pose

    return total, support_return


def overhead():
    # Draw outside the weapon shoulder and load the rear heel. The belt starts
    # the transfer to the lead boot before the chest follows through the chop.
    # The free hand reaches around the face, then the elbow opens in front of
    # the helmet while both hands guide the hilt into the raised chamber.
    return attacking('overhead', [
        (0, GUARD), (9, (-.09,-.17,-.10, 4,-23,-8, 12,-8,1, 6,-7,1, -12,28,3)),
        (16, (-.14,-.225,-.15, 7,-26,-9, 12,-9,3, 5,-5,4, -12,35,-4)),
        (25, (-.125,-.125,-.145, -3,-20,-8, -7,-4,2, -12,0,3, 12,22,-2)),
        (31, (-.16,-.145,-.155, -5,-23,-11, -8,-5,3, -10,2,4, 13,28,-2)),
        (37, (-.145,-.13,-.13, -6,-21,-10, -7,-7,3, -12,-2,4, 16,28,-2)),
        (40, (-.0466,-.083,-.105, -5,-13,-7, -6,-8,-2.5632, -11,-5,.2032, 12.08,25,-2)),
        (42, (.053,-.026,-.065, -4,-7,-3, -6,-6,-8.982, -9,-3,-4.118, 6.8,15,-2)),
        (45, (.145,.055,.03, -4,-3,2, -7,-4,-11.26, -6,-2,-5.74, -1,8,-2)),
        (47, (.175,.039,.064, -1,0,5, -4,-2,-11.26, -1,1,-5.74, -2,-2,-2)),
        (49, (.2084,-.12,.085, 9,3,8, 3,2,-10.1192, 5,3,-5.0408, -13.52,-6,-4)),
        (52, (.2122,-.27,.135, 14,5,10, 9,4,-3.9856, 12,5,.1056, -19.36,-9,-5)),
        (56, (.191,-.31,.172, 13,6,9, 17,7,1.148, 22,7,4.252, -24.2,-13,-5)),
        (61, (.147,-.28,.162, 11,4,7, 19,6,4, 24,7,5, -19,-9,-4)),
        (68, (.128,-.262,.131, 10,0,6, 15,4,3, 13,3,4, -10,0,-4)),
        (74, (.094,-.224,.094, 9,-2,5, 10,1,2, 8,0,3, -10,5,-3)),
        (81, (.060,-.159,.051, 7,-5,4, 10,-2,2, 7,-2,2, -11,11,-3)),
        (94, GUARD), (100, GUARD),
    ], [
        (0, GUARD_LEFT), (38, GUARD_LEFT), (42, (.38,.36,.12,10,8)),
        (46, (.40,.71,.18,11,-12)), (49, (.43,1.15,.044,13,-10)), (50, (.43,1.24,0,13,0)), (74, (.43,1.24,0,13,0)),
        (80, (.40,1.21,.08,12,11)), (91, rest_boot(GUARD_LEFT,1)),
    ], [
        (0, GUARD_RIGHT), (37, GUARD_RIGHT), (41, (-.337,-.211,0,-18,12)),
        (45, (-.32,-.17,0,-18,29)), (48, (-.32,-.16,.065,-18,38)),
        (53, (-.34,.04,.08,-20,38)), (62, (-.32,.40,.13,-19,19)), (74, rest_boot(GUARD_RIGHT,1)),
    ], [
        (0, GUARD_GRIP + GUARD_SWORD), (10, (-.67,1.39,.22,19,-41,21)),
        (19, (-.64,1.72,.12,-5,-32,21)), (26, (-.40,1.99,-.11,-21,-13,15)),
        (33, (-.34,2.1200,-.04,-28,-8,11)),
        (38, (-.32,2.1250,-.09,-33,-8,13)), (42, (-.24,2.22,-.26,-26,-4,12)), (45, (0,2.30,0,5,0,0)),
        (51, (.04,.94,.60,119,5,0)), (54, (.09,.74,.62,124,8,-4)),
        # Arrest the hilt close to the first catch. The edge finishes its
        # outward turn after the ribs, then the rear replant frees the lift.
        (56, (.127,.768,.598,120.83,11.78,-5.46)),
        (59, (.158,.776,.562,121,17,-7)), (63, (.188,.782,.534,122,24,-8)),
        (70, (.171,.800,.500,116,27,-7)), (74, (.112,.837,.482,106,23,-5)),
        (82, (-.10,1.04,.44,70,8,2)),
        (93, GUARD_GRIP + GUARD_SWORD),
    ], [
        (0, GUARD_FREE), (12, (.43,1.37,.40)), (18, (.47,1.49,.61)),
        (21, (.37,1.61,.57)), (25, (.24,1.74,.26)),
        (34, (.60,1.78,.294)), (40, (.66,1.68,.57)), (44, (.65,1.52,.482)), (49, (.67,1.36,.38)),
        (56, (.67,1.13,.48)), (63, (.63,1.00,.43)), (74, (.51,.97,.34)),
        (83, (.47,.99,.31)), (94, GUARD_FREE),
    ], right_elbow=[(0, RIGHT_ELBOW), (20, (-.85,-.65,-.54)), (42, (-1,.09,-.52)),
                    (51, (-.52,-.72,-.77)), (64, RIGHT_ELBOW)],
        left_elbow=[(0, LEFT_ELBOW), (19, (.75,-.8,.266)),
                    (33, (.75,-.8,.38)), (43, (.75,-.9,.04)), (53, (.78,-.83,.09)), (74, LEFT_ELBOW)],
        two_hand=[(0, (0,)), (13, (0,)), (27, (1,)), (33, (1,)), (46, (0,)), (100, (0,))])


def lunge():
    # The rear foot pushes. The forward knee travels first through the airborne
    # part of the dash and catches its mass on the landing cue.
    # Keep the weapon shoulder back through toe-off so the elbow can open beneath
    # the hilt and the blade has room to pass outside the skull.
    startup, active, ticks, root, _, _ = move_data('lunge')
    travel = root[-1][1][2]

    def moving(at, x, z, lift, yaw, pitch):
        return at, (x, z + linear(at, root)[2], lift, yaw, pitch)

    lead_catch = moving(38, .40, .50, 0, 13, 0)[1]
    rear_catch = moving(43, -.30, -.27, 0, -20, 20)[1]

    assert (startup, active, ticks) == (30, 8, 72)
    return attacking('lunge', [
        (0, GUARD), (11, (-.06,-.16,-.12, 7,-21,-6, 11,-9,2, 9,-10,3, -14,33,-3)),
        (22, (.034,-.27,-.294, 12,-27,-8, 16,-11,1, 10,-6,3, -19,33,-4)),
        (27, (.127,-.25,-.376, 10,-24,-5, 11,-9,2, 7,-6,3, -14,35,-4)),
        (30, (.175,-.354,-.292, 9,-19,-3, 10,-8,3, 6,-4,2, -13,28,-3)),
        (32, (.1898,-.36,-.2144, 7,-16,0, 9,-7,2, 7,-4,3, -13,20,-3)),
        (35, (.1332,-.24,-.1196, 9,-13,2, 8,-5,3, 7,0,3, -17,14,-4)),
        (38, (.08,-.20,.025, 8,-7,6, 9,-3,3, 9,3,3, -14,2,-6)),
        (41, (.11,-.28,.085, 10,-3,7, 11,0,3, 10,3,3, -17,-4,-6)),
        (46, (.13,-.27,.085, 10,0,5, 14,4,3, 11,6,3, -21,-7,-6)),
        (51, (.095,-.22,.07, 10,-3,3, 13,0,2, 9,3,2, -16,-2,-5)),
        (66, GUARD), (72, GUARD),
    ], [
        (0, GUARD_LEFT), (15, GUARD_LEFT), (24, (.37,.23,.07,11,8)),
        (30, (.34,.16,.22,8,-8)), moving(32,.35,.47,.26,9,-20),
        moving(35,.38,.55,.20,12,-19), moving(36,.39,.51,.15,12,-14),
        moving(37,.40,.48,.064,13,-7), (38, lead_catch), (49, lead_catch),
        (56, (.39,travel+.19,.08,13,7)),
        (66, rest_boot(GUARD_LEFT,travel)),
    ], [
        (0, GUARD_RIGHT), (27, GUARD_RIGHT), (30, (-.35,-.23,0,-19,23)),
        moving(32,-.36,-.67,.11,-23,47), moving(35,-.36,-.55,.23,-24,44),
        moving(38,-.35,-.51,.17,-23,31), (43, rear_catch),
        (48, (*rear_catch[:4], 0)), (51, (*rear_catch[:4], 0)),
        (57, (-.34,travel-.27,.10,-18,8)),
        (66, rest_boot(GUARD_RIGHT,travel)),
    ], [
        (0, GUARD_GRIP + GUARD_SWORD), (14, (-.69,1.06,-.18,79,-31,-4)),
        (21, (-.63,1.09,-.47,85,-18,-5)), (25, (-.64,1.10,-.73,88,-5,-5)),
        # Reverse the draw before launch, then meet the first capsule key in stride.
        (27, (-.63,1.14,-.896,90,-2,-6)), (28, (-.61,1.19,-.88,90,0,-3)),
        (29, (-.58,1.25,-.845,90,0,0)), (30, (-.55,1.2875,-.675,90,0,0)),
        (39, (-.18,1.24,.63,91,0,0)), (42, (-.38,1.13,.53,93,-5,-4)),
        (47, (-.56,1.02,.43,96,-14,-6)), (54, (-.61,1.05,.37,72,-23,-9)), (66, GUARD_GRIP + GUARD_SWORD),
    ], [
        (0, GUARD_FREE), (13, (.45,1.03,.44)), (26, (.34,1.08,.54)),
        (33, (.45,1.17,.32)), (38, (.53,1.01,-.18)), (43, (.67,.90,-.13)), (49, (.68,.84,.14)), (56, (.61,.92,.23)), (65, GUARD_FREE),
    ], right_elbow=[(0, RIGHT_ELBOW), (18, (-.75,-.87,-.30)), (25, (-.56,-1,-.20)),
                    (30, (-.60,-1,.04)), (36, (-.95,-.57,-.15)), (42, (-.65,-.65,-.75)),
                    (50, RIGHT_ELBOW)], blade_roll=[(24,(0,)), (30,(-16,)), (36,(-8,)), (42,(0,))], startup_blend=1)


def reaction(name, body, left, right, sword, free, carry=None):
    _, _, ticks, root, _, _ = move_data(name)

    def build(tick):
        grip = curve(tick, sword)
        if carry is None:
            return authored(tick, body, left, right, grip[:3], turn(*grip[3:]), curve(tick, free),
                            linear(tick, root)[2])
        blade = turn(*grip[3:])
        offset, lift, roll, envelope = carry
        weight, = curve(tick, envelope)
        axis = qrot(blade, Y)
        direction = norm(add(axis, (0,lift*weight,0)))
        blade = qmul(qmul(aim_rotation(axis, direction), blade), qaxis(Y, roll*weight))
        pose = authored(tick, body, left, right, add(grip[:3], mul(offset,weight)),
                        blade, curve(tick, free), linear(tick, root)[2])
        pose.roll_preference = .08 + 1.62*weight
        if name == 'stagger_light':
            pose.cuff_clearance, = curve(tick, [(0,(0,)), (2,(.65,)), (5,(1,)), (ticks,(1,))])
        return pose

    return ticks, build


def stagger_light():
    # One shoulder gets punched back. The rear foot gives ground before the guard returns.
    return reaction('stagger_light', [
        (0, GUARD),
        (2, (-.027,-.105,-.074, 1,-10,-4, -2,-4,-5, -5,1,-5, 3,14,9)),
        (5, (-.075,-.153,-.143, -3,-2,-7, -7,5,-7, -8,9,-5, 16,-6,8)),
        (8, (-.095,-.189,-.099, 2,1,-7, 7,5,-1, 5,6,1, -4,-7,6)),
        (11, (-.06,-.167,-.042, 8,0,-4, 13,3,4, 11,5,2, -13,-8,2)),
        (15, (-.022,-.124,-.026, 8,-6,-3, 12,-4,3, 9,-5,2, -13,10,-1)),
        (19, GUARD),
    ], [
        (0, GUARD_LEFT), (8, GUARD_LEFT), (11, (.39,.12,.074,16,6)),
        (17, rest_boot(GUARD_LEFT,-.375)),
    ], [
        (0, GUARD_RIGHT), (2, GUARD_RIGHT), (5, (-.35,-.44,.074,-19,11)),
        (8, rest_boot(GUARD_RIGHT,-.375)),
    ], [
        (0, GUARD_GRIP + GUARD_SWORD), (4, (-.64,1.16,.04,46,-54,-9)),
        (7, (-.71,1.09,-.19,60,-75,-19)), (12, (-.56,1.00,.23,71,-37,-10)),
        (19, GUARD_GRIP + GUARD_SWORD),
    ], [(0, GUARD_FREE), (3, (.55,1.10,.20)), (7, (.66,1.21,.13)), (11, (.60,1.06,.19)), (18, GUARD_FREE)],
        ((-.105,0,.015), 0, 70, [(0,(0,)), (3,(.75,)), (6,(1,)), (12,(1,)), (18,(0,))]))


def stagger_heavy():
    # Bigger recoil, a backward catch step, then a visible buckle and a low exposed hold.
    return reaction('stagger_heavy', [
        (0, GUARD), (4, (-.06,-.13,-.17, -2,-7,-5, -3,0,-4, -7,4,-5, 9,7,9)),
        (9, (-.11,-.13,-.19, -9,5,-9, -10,5,-7, -13,12,-9, 19,-17,10)),
        (16, (-.075,-.24,-.12, 11,13,-2, 19,6,4, 23,1,8, -14,-19,-3)),
        (22, (-.045,-.30,-.10, 14,9,-1, 21,4,2, 24,-3,6, -13,-12,-5)),
        (27, (.012,-.309,-.072, 14,5,3, 22,-2,1, 22,-4,5, -16,-4,-2)),
        (31, (.072,-.288,-.048, 15,3,4, 22,-4,2, 17,-5,4, -16,1,-4)),
        (35, (.034,-.205,-.020, 12,-1,-1, 17,-5,2, 11,-6,2, -10,11,-3)),
        (41, GUARD),
    ], [
        (0, GUARD_LEFT), (4, GUARD_LEFT), (9, (.43,.10,.14,16,26)),
        (15, (.45,-.37,.17,17,19)), (20, (.46,-.61,0,17,0)),
        (34, (.46,-.61,0,17,0)), (37, (.40,-.68,.066,14,4)),
        (41, rest_boot(GUARD_LEFT,-.9375)),
    ], [
        (0, GUARD_RIGHT), (1, GUARD_RIGHT), (5, (-.38,-.57,.095,-20,-6)),
        (8, (-.41,-.98,.055,-23,-11)), (11, (-.43,-1.21,0,-23,-8)), (26, (-.43,-1.21,0,-23,0)),
        (31, (-.41,-1.25,.073,-20,7)), (35, rest_boot(GUARD_RIGHT,-.9375)),
    ], [
        (0, GUARD_GRIP + GUARD_SWORD), (5, (-.77,1.29,.14,40,-76,-10)),
        (10, (-.88,1.23,-.14,58,-112,-18)), (17, (-.63,.68,.06,112,-84,-3)),
        (23, (-.58,.58,.12,121,-54,0)), (30, (-.58,.60,.14,115,-48,-2)),
        (41, GUARD_GRIP + GUARD_SWORD),
    ], [(0, GUARD_FREE), (5, (.66,1.18,-.05)), (8, (.75,1.29,-.19)), (15, (.67,.87,.23)), (22, (.55,.64,.28)),
        (29, (.58,.66,.26)), (41, GUARD_FREE)],
        ((-.14,.025,-.06), .195, 105, [(0,(0,)), (8,(0,)), (16,(1,)), (30,(1,)), (38,(0,))]))


DOWN = (-.075,-.53,-.14, -81,-9,-8, -5,-5,-2, -2,1,-1, 20,19,2)
DOWN_LEFT = (.41,.73,0,19,-9)
DOWN_RIGHT = (-.43,.68,0,-23,-13)
DOWN_GRIP = (-.77,.285,-.415)
DOWN_SWORD = (88,-97,3)
DOWN_FREE = (.625,.209,-.48)
DOWN_CUFF_ELBOW = (-.31846385,-.04910841,.13854782)


def settle_down_carry(pose, weight, held):
    grip, blade, pole = pose.sword_target
    blade = qmul(blade, qaxis(Y, -55*weight))
    pose.sword(grip, blade, pole)
    pose.roll_preference += 8*weight
    if held:
        pose.cuff_approach = qrot(blade, X), DOWN_CUFF_ELBOW


def low_pose(tick, body, left, right, sword, free, root=0, floor_hand=0, carry=0):
    grip = curve(tick, sword)
    hips, *upper = split_body(curve(tick, body))
    pose = Pose(add(hips, (0, 0, carry)), *upper)
    pose.boots((curve(tick, left), curve(tick, right)), root, knees_up=floor_hand)
    pose.sword(grip[:3], turn(*grip[3:]), RIGHT_ELBOW)
    pose.free_hand(curve(tick, free), lerp(LEFT_ELBOW, (1, -.15, -.28), floor_hand))
    return pose


def split_body(body):
    return body[:3], body[3:6], body[6:9], body[9:12], body[12:15]


def knockdown():
    _, _, ticks, root, _, _ = move_data('knockdown')
    travel = root[-1][1][2]

    def moving(at, x, z, lift, yaw, pitch):
        return at, (x, z + linear(at, root)[2], lift, yaw, pitch)

    body = [
        # Take the impact over the still-loaded toe. The belt keeps some
        # forward carry while the upper back starts to give.
        (0, (-.035,-.170,.045, 5,-10,-5, 5,-6,-3, 6,-3,0, -2,13,3)),
        (2, (-.043,-.184,.078, 0,-9,-6, 8,-4,-3, 8,0,-1, -2,13,3)),
        (4, (-.060,-.153,.048, -6,-8,-7, 7,-3,-2, 7,1,-1, 0,12,3)),
        (6, (-.081,-.070,-.054, -9,-8,-9, 4,0,0, 4,2,-1, 4,9,3)),
        (8, (-.10,.006,-.19, -14,-8,-10, 2,2,2, 2,3,-1, 8,6,3)),
        (12, (-.13,.035,-.26, -26,-8,-13, -6,10,4, -10,1,2, 18,10,1)),
        # The belt tips first. The ribs trail on the way down, then finish
        # their fall across the smaller pelvis rebound.
        (19, (-.13,-.12,-.24, -54,-11,-12, 0,4,3, -4,-2,-1, 23,15,3)),
        (24, (-.096,-.31,-.189, -72,-14,-12, 2,-3,-1, -4,-2,-1, 20,16,3)),
        (27, (-.081,-.432,-.151, -79,-13,-10, 1,-4,-2, -3,0,-2, 22,16,4)),
        (29, (-.075,-.484,-.14, -84,-11,-9, -3,-5,-2, -4,1,-1, 22,16,3)),
        (33, (-.08,-.454,-.14, -81,-10,-9, -5,-5,-2, -4,1,-1, 24,18,2)),
        (38, (-.078,-.491,-.14, -80,-9,-8, -6,-5,-2, -3,1,-1, 21,18,2)),
        (44, DOWN),
    ]
    # The free boot is carried into flight as the right toe finishes its
    # push. Aim both tracks in travel space before the launch.
    left = [(0, (.37,-.105,.235,13,8)), (2, (.38,-.028,.196,14,6)),
            (4, (.39,.025,.205,14,8)), moving(9,.41,.47,.35,14,14),
            moving(17,.47,.64,.51,17,22), moving(26,.43,.65,.31,20,3),
            moving(31,.41,.73,.14,19,-9), (40, rest_boot(DOWN_LEFT,travel))]
    right = [(0, (-.275,.166,0,-17,10)), (3, (-.275,.166,0,-18,22)),
             (4, (-.286,.126,.050,-19,23)), (5, (-.317,.060,.083,-20,23)),
             (6, (-.35,.00,.106,-20,22)), moving(10,-.38,.06,.30,-23,17),
             moving(18,-.44,.35,.56,-23,28), moving(27,-.42,.60,.20,-24,2),
             (39, rest_boot(DOWN_RIGHT,travel))]
    sword = [(0, (-.755,1.040,.347,47,-23,-26)),
             (2, (-.803,1.100,.393,49,-29,-27)),
             (5, (-.817,1.260,.265,43,-47,-16)),
             (8, (-.749,1.385,.085,43,-57,-10)),
             (12, (-.799,1.267,-.055,65,-76,4)),
             (16, (-.83,1.02,-.25,79,-86,12)), (24, (-.79,.56,-.42,77,-67,15)),
             (29, (-.76,.336,-.365,88,-85,4)), (34, (-.765,.329,-.40,88,-92,3)),
             (40, (-.77,.294,-.414,88,-97,3)), (45, DOWN_GRIP + DOWN_SWORD)]
    free = [(0, (.60,1.03,.38)), (3, (.63,1.18,.30)), (5, (.66,1.31,.20)),
            (10, (.79,1.40,.10)), (18, (.86,.97,-.16)),
            (28, (.70,.222,-.55)), (34, (.66,.242,-.48)), (44, DOWN_FREE)]
    def build(tick):
        # Let the pelvis catch up to the knockback through the launch instead
        # of giving back the full forward carry in a single airborne step.
        carry, = curve(tick, [(0,(0,)), (4,(0,)), (5,(.009,)), (6,(.025,)),
                              (7,(.053,)), (8,(.062,)), (9,(.035,)), (10,(.010,)), (11,(0,))])
        pose = low_pose(tick, body, left, right, sword, free, linear(tick, root)[2],
                        floor_hand=curve(tick, [(0,(0,)), (18,(.25,)), (34,(1,))])[0],
                        carry=carry)
        # Lift the wrist through the first rebound, where the drawn blade
        # otherwise falls below the floor before the settled carry.
        lift, = curve(tick, [(23,(0,)), (26,(.025,)), (28,(.065,)),
                             (31,(.040,)), (34,(.031,)), (36,(.024,)), (39,(0,))])
        grip, blade, pole = pose.sword_target
        pose.sword(add(grip, (0,lift,0)), blade, pole)
        hold, = curve(tick, [(0,(0,)), (23,(0,)), (32,(.7,)), (40,(1,)), (ticks,(1,))])
        settle_down_carry(pose, hold, tick >= 42)
        return pose

    return ticks, build


def recover():
    # Roll onto the glove and tuck the lead boot. Move the belt over the
    # planted heel while low, then lift the torso as the glove leaves the floor.
    _, _, ticks, root, _, _ = move_data('recover')
    travel = root[-1][1][2]
    body = [
        (0, DOWN), (5, (.014,-.521,-.132, -70,-11,-21, 17,-7,-5, 13,0,4, 19,13,-7)),
        (9, (.132,-.482,-.102, -61,-13,-26, 17,-8,-5, 15,2,3, 15,13,-8)),
        (12, (.214,-.465,-.035, -55,-15,-26, 19,-7,-4, 15,3,3, 17,15,-7)),
        (16, (.272,-.461,.090, -42,-14,-22, 22,-8,-4, 13,2,4, 13,19,-6)),
        (19, (.270,-.447,.148, -27,-13,-17, 22,-8,-3, 15,1,2, 2,19,-3)),
        (22, (.251,-.392,.153, -4,-12,-13, 22,-7,-3, 17,-1,2, -12,20,-2)),
        (26, (.198,-.299,.123, 11,-12,-10, 20,-7,-2, 14,-4,3, -19,22,1)),
        (31, (.104,-.191,.070, 12,-13,-6, 15,-8,0, 9,-7,4, -17,24,0)),
        (36, (.036,-.115,-.009, 8,-12,-4, 12,-8,2, 7,-9,3, -12,26,-2)),
        (40, GUARD),
    ]
    left = [(0, DOWN_LEFT), (3, DOWN_LEFT), (7, (.43,.50,.083,21,-15)),
            (11, (.386,.219,0,15,-6)), (14, (.386,.219,0,15,0)),
            (31, (.386,.219,0,15,0)), (35, (.374,.28,.042,13,6)),
            (40, rest_boot(GUARD_LEFT,travel))]
    right = [(0, DOWN_RIGHT), (4, DOWN_RIGHT), (9, (-.40,.40,.132,-18,11)),
             (14, (-.42,.008,.084,-20,27)), (18, (-.41,-.18,0,-20,29)),
             (25, (-.41,-.18,0,-20,13)), (29, (-.375,-.15,.047,-18,6)),
             (33, rest_boot(GUARD_RIGHT,travel))]
    sword = [
        (0, DOWN_GRIP + DOWN_SWORD),
        (4, (-.61,.32,-.397,88,-87,4)),
        (8, (-.452,.427,-.422,84,-76,5)),
        (12, (-.361,.557,-.388,79,-72,-3)),
        (16, (-.412,.652,-.245,75,-67,-9)),
        (21, (-.557,.756,.009,70,-63,-12)),
        (27, (-.730,.827,.153,58,-48,-9)),
        (34, (-.626,1.038,.291,47,-33,-5)),
        (40, GUARD_GRIP + GUARD_SWORD),
    ]
    # The braced glove's lower skin is .247 model units below this wrist turn.
    support = (.795, .253, -.345)
    brace = turn(82.5, -10.6, -123.3)
    free = [(0, DOWN_FREE), (2, (.673,.251,-.431)), (5, support), (15, support), (19, (.845,.439,-.165)),
            (22, (.856,.599,.114)), (27, (.752,.754,.216)), (33, (.615,.895,.281)), (40, GUARD_FREE)]

    def build(tick):
        travel_at = linear(tick, root)[2]
        folded = curve(tick, [(0,(1,)), (14,(.87,)), (22,(.37,)), (29,(0,))])[0]
        pose = low_pose(tick, body, left, right, sword, free, travel_at, floor_hand=folded)
        wrist, pole = pose.free_target
        plant = curve(tick, [(0,(1,)), (15,(1,)), (23,(0,))])[0]
        pose.free_hand(sub(wrist, (0, 0, travel_at * plant)), pole)
        brace_weight, = curve(tick, [(0,(0,)), (5,(1,)), (15,(1,)), (23,(0,))])
        if brace_weight > 0:
            pose.free_orientation = brace, brace_weight
        # Roll the guard out of the folded knee's path before the weapon rises.
        clearance, = curve(tick, [(0,(0,)), (4,(.18,)), (10,(.8,)), (16,(1,)),
                                  (24,(1,)), (33,(.52,)), (40,(0,))])
        grip, blade, pole = pose.sword_target
        pose.sword(grip, qmul(blade, turn(0, -5*clearance, -30*clearance)), pole)
        # While the palm bears weight, carry the hilt outside the folded knee
        # and turn its long guard toward the clear side of the arm.
        route, = curve(tick, [(0,(0,)), (2,(0,)), (4,(.65,)), (8,(1,)),
                              (15,(1,)), (19,(.55,)), (24,(0,))])
        if route:
            grip, blade, pole = pose.sword_target
            pose.sword(add(grip, mul((-.10,.012,-.045), route)),
                       qmul(blade, qaxis(Y, 60*route)), pole)
        pose.cuff_clearance, = curve(tick, [(0,(1,)), (17,(1,)), (25,(0,))])
        hold, = curve(tick, [(0,(1,)), (3,(.88,)), (8,(.13,)), (12,(0,)), (40,(0,))])
        settle_down_carry(pose, hold, tick <= 1)
        return pose

    return ticks, build


def death():
    # The pelvis hits on the body-land cue, followed by the ribs and the head.
    # The free palm catches the roll while the blade settles outboard of the body.
    _, _, ticks, root, _, _ = move_data('death')
    body = [
        (0, GUARD), (5, (-.12,-.06,-.12, -8,-5,-10, -10,6,-8, -12,13,-7, 16,-20,7)),
        (13, (-.15,-.19,-.20, 3,-16,-15, 19,0,-4, 16,14,-8, -12,-3,7)),
        (23, (-.17,-.36,-.20, -8,-27,-18, 8,-7,-4, 13,5,-10, 11,9,-2)),
        (32, (-.22,-.46,-.24, -28,-34,-33, -5,5,-9, -2,-4,-12, 16,14,5)),
        (40, (-.30,-.510,-.27, -19,-35,-63, -2,3,2, -2,-5,5, 10,14,18)),
        (44, (-.31,-.519,-.28, -19,-35,-61, -4,2,-4, -3,-6,-4, 14,13,22)),
        (49, (-.31,-.524,-.28, -19,-35,-62, -3,2,-3, -2,-6,-4, 5,15,13)),
        (60, (-.31,-.528,-.28, -19,-35,-62, -3,2,-3, -2,-6,-4, 8,16,16)),
    ]
    left = [(0, GUARD_LEFT), (7, GUARD_LEFT), (13, (.47,.09,.12,19,7)),
            (20, (.53,-.11,0,21,-3)), (26, (.56,-.13,0,19,8)),
            (37, (.43,-.31,.12,11,31)), (49, (.32,-.48,.035,20,5)), (55, (.31,-.48,0,21,-8))]
    right = [(0, GUARD_RIGHT), (4, GUARD_RIGHT), (11, (-.40,-.65,.07,-22,17)),
             (17, (-.46,-.78,0,-23,14)), (29, (-.45,-.83,.04,-26,30)),
             (39, (-.46,-.66,.10,-28,25)), (51, (-.46,-.52,0,-26,-3)), (57, (-.46,-.47,0,-26,-8))]
    sword = [(0, GUARD_GRIP + GUARD_SWORD), (8, (-.83,1.24,.02,65,-88,-12)),
             (19, (-.73,.73,.16,97,-72,-7)), (25, (-.48,.64,-.34,96,-76,-7)), (28, (-.34,.58,-.53,96,-78,-5)),
             (30, (-.25,.55,-.63,95,-78,-4)),
             (35, (-.028,.40,-.925,92,-82,-2)),
             (40, (.04,.245,-.975,91,-82,0)),
             (45, (.1,.17,-.975,90,-81,0)),
             (52, (.09,.142,-.945,90,-80,0)),
             (59, (.06,.138,-.955,91,-80,0))]
    free = [(0, GUARD_FREE), (8, (.76,1.13,.12)), (18, (.55,.75,.28)),
            (29, (.5425,.46,.073)),
            (36, (.645,.305,-.085)),
            (40, (.655,.245,-.12)),
            (44, (.645,.215,-.145)),
            (49, (.635,.218,-.16)),
            (57, (.625,.215,-.19))]
    def build(tick):
        pose = low_pose(tick, body, left, right, sword, free, linear(tick, root)[2],
                        floor_hand=curve(tick, [(0,(0,)), (25,(.35,)), (45,(1,))])[0])
        catch, = curve(tick, [(0,(0,)), (31,(0,)), (44,(1,))])
        hand, elbow = pose.free_target
        pose.free_hand(add(hand, mul((.25,-.015,.2),catch)),
                  add(elbow,mul((.4,0,.4),catch)))
        if 23 < tick < 39:
            # The belt and boots start the collapse. Keep the ribs open for a
            # beat and let the head lose its line after the shoulder falls.
            spine_pitch, spine_roll, rib_pitch, rib_roll, head_pitch, head_roll = curve(tick, [
                (23, (0,0,0,0,0,0)), (27, (5,5,6,4,12,12)),
                (31, (2,2,5,5,13,15)), (35, (0,0,1,1,7,9)),
                (39, (0,0,0,0,0,0)),
            ])
            head = qmul(turn(head_pitch, 0, head_roll), pose.rot['Head'])
            pose.turns['Spine'] = qmul(turn(spine_pitch, 0, spine_roll), pose.turns['Spine'])
            pose.turns['Chest'] = qmul(turn(rib_pitch, 0, rib_roll), pose.turns['Chest'])
            pose.fk()
            pose.set_world('Head', head)
        pose.roll_preference, = curve(tick, [(0, (.08,)), (21, (.08,)), (31, (1.4,)), (42, (3,)), (50, (8,))])
        # Retain the continuous arm branch selected through the floor catch.
        outward, = curve(tick, [(15, (0,)), (20, (1,)), (24, (1,)), (27, (0,))])
        grip, blade, pole = pose.sword_target
        blade = qmul(qaxis(X, 5 * outward), blade)
        support, = curve(tick, [(17, (0,)), (20, (1,)), (22, (1,)), (25, (0,))])
        pose.sword(add(grip, (-.024 * outward, .012 * outward + .014 * support, 0)),
                   blade, add(pole, (0, .13 * support, 0)))
        pose.cuff_approach = defeat_approach(tick)
        pose.cuff_clearance, = curve(tick, [
            (0, (0,)), (5, (1,)), (57, (1,)), (61, (.25,)),
        ])
        elbow_weight, = curve(tick, [(18, (0,)), (29, (1,)), (42, (1,)), (55, (.6,))])
        roll_weight, = curve(tick, [(18, (0,)), (31, (1,)), (39, (1,)), (49, (0,))])
        grip, blade, elbow = pose.sword_target
        early, = curve(tick, [(0,(0,)), (13,(0,)), (23,(1,)), (28,(1,)), (37,(0,))])
        grip = add(grip, mul((.0,.14,-.6),early))
        blade = qmul(blade, qaxis(Y, 18 * roll_weight))
        pose.sword(grip, blade, lerp(elbow, (0, -.85, -.85), elbow_weight))
        return pose

    return ticks, build


def guard(tick):
    # Small asymmetric shifts, with both boots pinned throughout the loop.
    phase = math.tau * tick / 120
    weight = math.sin(phase)
    breath = (1 - math.cos(phase)) * .5
    offsets = (.018*weight, .018*breath, -.016*weight, 0, 2*weight, 2*weight,
               -3*breath, 2*weight, 0, -2*breath, 0, weight, 3*breath, -3*weight, -weight)
    # Open the hardware away from the cuff and scarf before entering a step.
    # The breathing pose, boot positions and grip attachment stay the same.
    blade = qmul(turn(*add(GUARD_SWORD, (-3*breath, -4*weight, 3*weight))), turn(0,85,-6))
    pose = upright(tuple(a + b for a, b in zip(GUARD, offsets)), (GUARD_LEFT, GUARD_RIGHT),
                   0, add(GUARD_GRIP, (-.055 - .012*weight, .008 + .025*breath, .013*weight)),
                   blade, add(GUARD_FREE, (.018*weight, .02*breath, .015*weight)))
    pose.roll_preference = 3
    return pose


def settle_left_circle(pose, tick, load, catch):
    # Keep the wide catch moving into support, then brake it across the stance.
    # The narrow gather passes under the guard sooner. The belt and boots keep
    # their own cadence, while the chest roll keeps the established head line.
    chest_x, = curve(tick, [
        (0, (.055,)), (3, (.092,)), (6, (.142,)), (9, (.167,)),
        (13, (.172,)), (15, (.164,)), (17, (.151,)), (19, (.121,)),
        (21, (.071,)), (24, (.026,)), (30, (.018,)), (32, (.026,)), (34, (.055,)),
    ])
    chest_roll, = curve(tick, [
        (0, (2,)), (2, (4,)), (5, (-3,)), (8, (-11,)),
        (13, (-12,)), (16, (-13,)), (18, (-15,)), (20, (-18,)),
        (22, (-12,)), (25, (-5,)), (30, (-2,)), (32, (-1,)), (34, (2,)),
    ])
    pivot = pose.pos['Spine']
    x, y, _ = sub(pose.pos['Chest'], pivot)
    original = math.atan2(x, y)
    desired = math.asin((chest_x - pivot[0]) / math.hypot(x, y))
    pitch, yaw, _ = to_euler(pose.rot['Chest'])
    roll = qaxis(Z, math.degrees(original - desired))
    pose.set_world('Spine', qmul(roll, pose.rot['Spine']))
    pose.set_world('Chest', turn(pitch, yaw, chest_roll))
    pose.set_world('Head', turn(10 + 2*catch, 2 + 1.5*load, -2.5))


def gait(ticks, speed, direction, duty, height, lean, swing):
    """Two steps at nominal graph speed, with contact targets in travel space."""
    stride = speed * ticks / (60 * SCALE)
    side, _, forward = direction
    running = duty < .5
    width = .345 if side else .235 if running else .255
    stagger = .12 if side or forward < 0 else .022
    depth = -.215 if running else -.23 if side else -.155
    reach = stride * duty * .5
    stance_pitch = [(0, (-7,)), (.20, (0,)), (.63, (0,)), (1, (24,))]
    swing_pitch = [(0, (24,)), (.28, (38,)), (.63, (7,)), (.85, (-14,)), (1, (-7,))]
    swing_height = [(0, (0,)), (.25, (.73,)), (.52, (1,)), (.81, (.50,)), (1, (0,))]

    def step(phase, sign):
        if phase < duty:
            amount, lift = reach - stride * phase, 0
            pitch = curve(phase / duty, stance_pitch)[0]
        else:
            u = (phase - duty) / (1 - duty)
            # The swing first releases in the direction of the support foot, then
            # recovers past the hip. It arrives already matching contact velocity.
            amount = (-reach + 2 * reach * (3 - 2*u) * u*u
                      - stride * (1 - duty) * u * (1 - u) * (1 - 2*u))
            lift = height * curve(u, swing_height)[0]
            pitch = curve(u, swing_pitch)[0]
        if forward < 0:
            pitch = -.70 * pitch
        if side:
            pitch *= .56
        yaw = sign * (13 if side else 9) + side * 7
        return (sign * width + side * amount, sign * stagger + forward * amount, lift, yaw, pitch)

    # Accept the mass over the front half of each stance, then unload before toe-off.
    # A delayed torso transfer keeps the upper body moving when the hips change sides.
    load_curve = [(0, (.10,)), (.22, (.93,)), (.46, (1,)), (.77, (.69,)), (1, (0,))]
    compression = [(0, (.018,)), (.24, (-.046,)), (.43, (-.037,)),
                   (.78, (.030,)), (1, (.018,))]
    if running:
        compression = [(0, (.032,)), (.27, (-.055,)), (.48, (-.032,)),
                       (.84, (.044,)), (1, (.032,))]

    def load(phase):
        phase %= 1
        return curve(phase / duty, load_curve)[0] if phase < duty else 0

    def transfer(phase):
        left = load(phase)
        right = load(phase + .5)
        return (left - right) / (.35 + left + right)

    weight_track = [(0, (0,)), (.20, (1,)), (.39, (.76,)), (.65, (.21,)), (.90, (0,)), (1, (0,))]

    def build(tick):
        phase = (tick % ticks) / ticks
        left, right = step(phase, 1), step((phase + .5) % 1, -1)
        rhythm = math.tau * phase
        lateral = transfer(phase)
        follow = transfer(phase - 3.25 / ticks)
        direction = forward if forward else .65
        lead = direction * math.cos(rhythm - .29)
        shoulder = direction * math.cos(rhythm - .83)
        swing_phase = math.sin(rhythm - 1.05) * (-1 if forward < 0 else 1)
        load_beat = (2 * phase) % 1
        sink = curve(load_beat, compression)[0]
        catch = curve(load_beat, weight_track)[0]
        chest_catch = curve((load_beat - 5 / ticks) % 1, weight_track)[0]
        pitch_load = math.sin(math.tau * load_beat - .85)

        hip_x = (.17 if side else .155) * lateral + side * .04
        hip_yaw = -2 - 9 * lead + 8 * side
        hip_roll = -2 + 7 * lateral - 3 * side
        spine_yaw = -1 + 11 * shoulder - 3 * side
        chest_yaw = 2 + 12 * shoulder - 5 * side
        if running:
            # Keep the forward drive and let the high free elbow lead out of compression.
            hip_z = .06 + .036 * pitch_load
            hip_pitch = 4 + lean * .33 + 4 * pitch_load
            spine_pitch = 9 + lean * .36 - 2.5 * pitch_load
            chest_pitch = 5 + lean * .34 + 2.5 * pitch_load
            spine_roll, chest_roll = 2 - 6 * follow, 1 - 5 * follow
            carry = (-.46 - .025 * follow, .056 - .045 * chest_catch, .32 - .077 * swing_phase)
            blade = turn(3 + 7 * chest_catch, -5 - 6 * follow, 23 + 6 * follow)
            free = (.53 + .035 * follow, -.235 + .17 * max(0, swing_phase), .01 + swing * swing_phase)
        elif side:
            # The belt transfers into the new boot. The spine follows, then the
            # ribs and free elbow catch, with the blade retaining the old line briefly.
            sternum = transfer(phase - (4.7 if side > 0 else 4.25) / ticks)
            weapon = transfer(phase - (6.5 if side > 0 else 5.5) / ticks)
            elbow = side * transfer(phase - (5.25 if side > 0 else 4.4) / ticks)
            bias = side * weapon
            wrist_catch = curve((load_beat - (9 if side > 0 else 7.5) / ticks) % 1, weight_track)[0]
            hip_z = .025 + .023 * pitch_load
            hip_pitch, spine_pitch, chest_pitch = 5 + 3 * catch, 7 - 2 * catch, 4 + 4 * chest_catch
            spine_roll = 1 - 10 * follow - 3 * side
            chest_roll = 1 - 8 * sternum - 3 * side
            counter_roll = (10 if side > 0 else 6) * (sternum - weapon)
            if side > 0:
                carry = (-.51 - .025 * bias, -.092 - .05 * wrist_catch, .44 - .049 * bias)
                blade = turn(18 + 7 * wrist_catch, -3 + 4 * bias, 22 - 8 * bias + counter_roll)
                free = (.61 + .08 * elbow, -.19 - .042 * chest_catch, .19 + swing * swing_phase)
            else:
                carry = (-.51 - .074 * bias, -.10 - .034 * wrist_catch, .40 + .080 * bias)
                blade = turn(21 + 9 * wrist_catch, -15 - 7 * bias, 28 + 6 * bias + counter_roll)
                free = (.57 - .055 * elbow, -.30 - .042 * chest_catch, .04 - swing * swing_phase)
        elif forward < 0:
            # The hips make space behind the guard. The sternum and off fist stay
            # in front of the pelvis as the rearward step absorbs weight.
            hip_z = -.070 - .043 * catch + .032 * pitch_load
            hip_pitch = -3 - 5 * catch
            spine_pitch = 10 + 5 * catch
            chest_pitch = 6 + 6 * chest_catch
            spine_roll, chest_roll = 2 - 9 * follow, 1 - 7 * follow
            carry = (-.425 - .033 * follow, -.025 + .048 * chest_catch, .48 - .043 * swing_phase)
            blade = turn(6 - 8 * chest_catch, -4 - 6 * follow, 19 + 5 * follow)
            free = (.46 + .045 * follow, -.19 - .047 * chest_catch, .23 + swing * swing_phase)
        else:
            # The march settles forward into each heel, then the rib cage comes
            # across the support. The sword drops a beat behind the shoulder.
            hip_z = .06 + .044 * catch + .024 * pitch_load
            hip_pitch = 4 + 5 * catch
            spine_pitch = 8 + 2 * catch
            chest_pitch = 5 + 7 * chest_catch
            spine_roll, chest_roll = 2 - 11 * follow, 1 - 8 * follow
            carry = (-.47 - .034 * follow, -.11 - .055 * chest_catch, .40 - .094 * swing_phase)
            blade = turn(17 + 9 * chest_catch, -8 - 6 * follow, 24 + 6 * follow)
            free = (.54 + .045 * follow, -.25 - .048 * chest_catch, .07 + swing * swing_phase)

        body = (
            hip_x, depth + sink, hip_z,
            hip_pitch, hip_yaw, hip_roll,
            spine_pitch, spine_yaw, spine_roll,
            chest_pitch, chest_yaw, chest_roll,
            10 - .87 * (hip_pitch + spine_pitch + chest_pitch),
            2 - .86 * (hip_yaw + spine_yaw + chest_yaw),
            -2 - (.82 if side else .70) * (hip_roll + spine_roll + chest_roll),
        )
        pose = Pose(*split_body(body))
        pose.boots((left, right), 0)
        chest, chest_turn = pose.pos['Chest'], pose.rot['Chest']
        pose.sword(add(chest, qrot(chest_turn, carry)), qmul(chest_turn, blade),
                   qrot(chest_turn, RIGHT_ELBOW))
        pose.free_hand(add(chest, qrot(chest_turn, free)), qrot(chest_turn, LEFT_ELBOW))
        if side > 0:
            # Keep the established guard path as the body shifts beneath it.
            settle_left_circle(pose, tick % ticks, lateral, catch)
        return pose

    return ticks, build


def charge():
    """Two driving steps at the charge sample's 3.96 m/s."""
    ticks, duty = 34, .40
    stride = 3.96 * ticks / (60 * SCALE)
    front = .505
    back = front - stride * duty
    swing_travel = stride * (1 - duty)
    swing_lift = [(0, (0,)), (.075, (.119,)), (.21, (.244,)),
                  (.37, (.308,)), (.53, (.292,)), (.70, (.211,)), (.86, (.090,)), (1, (0,))]
    swing_pitch = [(0, (36,)), (.16, (48,)), (.36, (42,)),
                   (.66, (7,)), (.85, (-13,)), (1, (-9,))]
    stance_pitch = [(0, (-9,)), (.17, (0,)), (.56, (0,)),
                    (.81, (21,)), (1, (36,))]

    def step(phase, sign):
        if phase < duty:
            # Nominal graph travel cancels this displacement; keep a fixed heel,
            # then a fixed ball as the ankle rises into push-off.
            z = front - stride * phase
            lift = 0
            pitch = curve(phase / duty, stance_pitch)[0]
        else:
            u = (phase - duty) / (1 - duty)
            # The swing leaves and arrives at stance speed. The extra early
            # lag folds the trailing heel before the shin reaches forward.
            z = back + (front - back) * u*u*(3 - 2*u)
            z -= swing_travel * u*(1-u)*(1-2*u) + 1.9 * u*u*(1-u)**3
            lift = curve(u, swing_lift)[0]
            pitch = curve(u, swing_pitch)[0]
        return (sign * .257, z + sign * .012, lift, sign * 7, pitch)

    # A heel catch compresses briefly. The belt rises during late support,
    # then crests before the next heel meets the ground.
    load = [(0, (-.169, .012, 0)), (.23, (-.212, .035, 1)),
            (.45, (-.154, .017, .47)), (.62, (-.102, -.008, 0)),
            (.79, (-.133, -.027, 0)), (1, (-.169, .012, 0))]

    def build(tick):
        phase = (tick % ticks) / ticks
        rhythm = math.tau * phase
        step_phase = (2 * phase) % 1
        hip_y, hip_z, absorb = curve(step_phase, load)
        stance = math.sin(rhythm + .30)
        pelvis_lead = math.cos(rhythm + .12)
        ribs_lead = math.cos(rhythm - .49)
        free_drive = -math.cos(rhythm + .14)
        body = (
            .051 * stance, hip_y, hip_z,
            5 + 4*absorb, -3 - 11*pelvis_lead, -1 + 4*stance,
            12 + 3*absorb, 8*ribs_lead, 1 - 4*math.sin(rhythm - .29),
            9 + 2*absorb, -3 + 8*ribs_lead, 1 - 2*math.sin(rhythm - .67),
            -17 - 5*absorb, 6 - 5*ribs_lead, -2,
        )
        # Keep the sword beside the ribs while the free elbow drives opposite
        # its lead knee. The fist stays below the eye line as it returns.
        carry = (-.68, -.018 - .031*absorb, .34 + .042*free_drive)
        blade = qmul(turn(3 + 5*absorb, -8 - 4*ribs_lead, 26 + 3*stance), turn(0, -3, -31))
        free = (.555 + .027*stance + .16*max(0, -free_drive),
                -.29 + .117*free_drive, .13 + .195*free_drive)
        pose = Pose(*split_body(body))
        pose.roll_preference = 6
        pose.boots((step(phase, 1), step((phase + .5) % 1, -1)), 0)
        chest, chest_turn = pose.pos['Chest'], pose.rot['Chest']
        pose.sword(add(chest, qrot(chest_turn, carry)), qmul(chest_turn, blade),
                   qrot(chest_turn, RIGHT_ELBOW))
        pose.free_hand(add(chest, qrot(chest_turn, free)), qrot(chest_turn, LEFT_ELBOW))
        return pose

    return ticks, build


def guarded_steps(ticks, speed, heading):
    """Guarded forward and backward steps with a planted double-support transfer."""
    stride = speed * ticks / (60*SCALE)
    retreating = heading < 0
    duty = .635 if retreating else .65
    front = .356 if retreating else .397
    back = front - stride*duty
    stance = [(0, (-8,)), (.21, (0,)), (.66, (0,)), (1, (22,))]
    swing = [(0, (22,)), (.22, (31,)), (.55, (12,)), (.84, (-12,)), (1, (-8,))]
    lift_track = [(0,(0,)), (.21,(.57,)), (.49,(1,)), (.78,(.57,)), (1,(0,))]
    compression = [
        (0, (-.124, .15)), (.16, (-.163, .79)), (.27, (-.185, 1)),
        (.43, (-.162, .67)), (.70, (-.108, .12)), (.86, (-.101, 0)),
        (1, (-.124, .15)),
    ]

    def boot(p, sign):
        if p < duty:
            amount, lift = front-stride*p, 0
            pitch, = curve(p/duty, stance)
        else:
            u = (p-duty)/(1-duty)
            amount = back+(front-back)*u*u*(3-2*u)
            amount -= stride*(1-duty)*u*(1-u)*(1-2*u)
            lift = (.116 if retreating else .153)*curve(u, lift_track)[0]
            pitch, = curve(u, swing)
        return (sign*(.302 if retreating else .283),
                heading*amount+sign*(.094 if retreating else .024), lift,
                sign*(14 if retreating else 11), -.73*pitch if retreating else pitch)

    def build(tick):
        p = (tick % ticks) / ticks
        rhythm = math.tau*p
        weight = math.sin(rhythm+.39)
        lead = math.cos(rhythm-.31)*heading
        rib_lag = math.sin(rhythm-.32)
        y, load = curve((2*p)%1, compression)
        if retreating:
            hip_z = -.016 - .050*load
            hip_pitch, spine_pitch, chest_pitch = -5-3*load, 10+4*load, 10+2*load
        else:
            hip_z = .032 + .047*load
            hip_pitch, spine_pitch, chest_pitch = 4+3*load, 8+2*load, 6+2*load
        hip_roll = -1 + 5*weight
        rib_roll = -3*rib_lag
        body = (
            .073*weight, y, hip_z,
            hip_pitch, -3-7*lead, hip_roll,
            spine_pitch, -2+4*math.cos(rhythm-.58)*heading, rib_roll,
            chest_pitch, 6*math.cos(rhythm-.76)*heading, 1-2*rib_lag,
            7 - .83*(hip_pitch+spine_pitch+chest_pitch), 3-4*lead, -2-.76*(hip_roll+rib_roll),
        )
        pose = Pose(*split_body(body))
        pose.boots((boot(p,1),boot((p+.5)%1,-1)),0)
        chest, chest_turn = pose.pos['Chest'], pose.rot['Chest']
        drive = -math.cos(rhythm+.17)
        f, g = math.cos(rhythm-.23), math.sin(rhythm-.40)
        grip = (-.692+.014*f, -.041-.025*g, .31+.032*f)
        blade = turn(15+3*g, -10+4*f, -7+3*f)
        free = (.567+.04*max(0,-drive), -.255-.041*load, .18+.125*drive)
        pose.sword(add(chest,qrot(chest_turn,grip)),qmul(chest_turn,blade),qrot(chest_turn,RIGHT_ELBOW))
        pose.free_hand(add(chest,qrot(chest_turn,free)),qrot(chest_turn,LEFT_ELBOW))
        pose.roll_preference = 6
        return pose

    return ticks, build


def circle_step(direction):
    """A lead catch followed by a shorter gather; the two contacts share graph phase."""
    ticks = 40
    stride = 1.40 * ticks / (60 * SCALE)
    side = direction[0]
    _, carry_from_r8 = gait(34, 1.40, direction, .53, .12, 1, .12)
    stance_pitch = [(0, (-6,)), (.19, (0,)), (.63, (0,)), (1, (13,))]
    swing_pitch = [(0, (13,)), (.25, (23,)), (.57, (9,)), (.86, (-11,)), (1, (-6,))]
    lead_lift = [(0, (0,)), (.23, (.68,)), (.49, (1,)), (.78, (.58,)), (1, (0,))]
    gather_lift = [(0, (0,)), (.12, (.62,)), (.32, (1,)), (.68, (.66,)), (1, (0,))]
    hip_track = [
        (0, (.027, -.281, .018, 4)),
        (.12, (.093, -.259, .03, 8)),
        (.28, (.134, -.237, .043, 4)),
        (.45, (.077, -.208, .027, 2)),
        (.56, (.012, -.219, .016, 1)),
        (.66, (-.045, -.238, .008, 5)),
        (.80, (-.068, -.221, .005, 2)),
        (.90, (-.025, -.217, .011, 1)),
        (.96, (.008, -.252, .015, 2)),
        (1, (.027, -.281, .018, 4)),
    ]

    def boot(phase, sign):
        lead = sign == side
        p = (phase - (0 if lead else .54)) % 1
        duty = .60 if lead else .47
        front = .315 if lead else .282
        back = front - stride * duty
        if p < duty:
            amount, lift = front - stride*p, 0
            pitch, = curve(p / duty, stance_pitch)
        else:
            u = (p - duty) / (1 - duty)
            # Keep the new contact in travel space. The gather clears the
            # lead boot without taking the same arc as the next wide catch.
            amount = back + (front - back)*u*u*(3 - 2*u)
            amount -= stride*(1 - duty)*u*(1-u)*(1-2*u)
            lift = (.118 if lead else .12) * curve(u, lead_lift if lead else gather_lift)[0]
            pitch, = curve(u, swing_pitch)
        # On the left gather, keep the following right boot a small fixed
        # distance out and back; both boot soles stay in travel space.
        gather_gap = .02 if side > 0 and not lead else 0
        return (sign * ((.391 if lead else .405) + gather_gap) + side*amount,
                (.185 + .60*gather_gap)*sign, lift, 13*sign + side*6, pitch)

    def build(tick):
        synced = (tick % ticks) / ticks
        phase = (synced + (.54 if side < 0 else 0)) % 1
        lead_turn = math.cos(math.tau * (phase - .055))
        rib_lag = math.cos(math.tau * (phase - .15))
        x, y, z, compress = curve(phase, hip_track)
        hip_roll = -side * (4 + 4*lead_turn)
        rib_roll = side * (3 + 3*rib_lag)
        body = (
            side*x, y, z,
            5 + .30*compress, -2 + side*(6 - 6*lead_turn), hip_roll,
            7 - .20*compress, side*(5*rib_lag - 2), rib_roll,
            5 + .44*compress, side*(5*rib_lag - 3), -side*(3 + 3*rib_lag),
            -5 - .24*compress, -side*(3*rib_lag + 1), -side*(hip_roll*side*.71),
        )
        pose = Pose(*split_body(body))
        pose.boots((boot(phase, 1), boot(phase, -1)), 0)
        carry = carry_from_r8(synced * 34)
        if side > 0:
            # Keep the broad leading-side guard, turning the guard hardware
            # around the cuff instead of crossing the belt on the gather.
            grip, blade, pole = carry.sword_target
            pose.sword(add(grip, (-.045,.018,.012)), qmul(blade, turn(0,15,-36)), pole)
        else:
            # The weapon-side step carries the hilt beside the ribs; its
            # blade tips forward as the trailing foot comes through.
            f = math.cos(math.tau*synced - .23)
            g = math.sin(math.tau*synced - .40)
            chest, chest_turn = pose.pos['Chest'], pose.rot['Chest']
            grip = (-.68 + .014*f, -.041 - .025*g, .31 + .032*f)
            blade = turn(15 + 3*g, -10 + 4*f, -7 + 3*f)
            pose.sword(add(chest, qrot(chest_turn, grip)), qmul(chest_turn, blade),
                       qrot(chest_turn, RIGHT_ELBOW))
        pose.roll_preference = 6
        pose.free_hand(*carry.free_target)
        return pose

    return ticks, build


CLIPS = {
    'guard': (lambda: (120, guard), 'Asymmetric guard with a slow chest lift and planted boots.'),
    'advance': (lambda: guarded_steps(60, .93, 1), 'Deliberate heel catch into a guarded forward march.'),
    'charge': (charge, 'Longer charge steps with a loaded heel catch and a brief flight phase.'),
    'retreat': (lambda: guarded_steps(42, 1.20, -1), 'Guarded backstep with a toe catch and heel brake.'),
    'circle_left': (lambda: circle_step(X), 'Lead catch and gather toward the free hand.'),
    'circle_right': (lambda: circle_step((-1, 0, 0)), 'Lead catch and gather toward the weapon.'),
    'swipe': (swipe, 'Rear-foot coil, crossing cut and a forward catch step.'),
    'overhead': (overhead, 'Rear-foot load, two-hand chop and low return outside the armor.'),
    'lunge': (lunge, 'Rear-foot coil, airborne thrust and a low two-step brake.'),
    'stagger_light': (stagger_light, 'Shoulder recoil into one short catch step.'),
    'stagger_heavy': (stagger_heavy, 'Backstep and a full-torso buckle with an exposed hold.'),
    'knockdown': (knockdown, 'Backward fall, floor rebound and a still landing pose.'),
    'recover': (recover, 'Roll onto the free hand, fold a boot beneath the hips and push up.'),
    'death': (death, 'Twisting side collapse with a separate floor settle.'),
}


def open_low_carry(pose, distance, roll):
    # Follow the original elbow plane as the grip skirts the waist armor.
    # Turning around the existing blade axis opens the pommel without lifting the blade.
    grip, blade = pose.blade()
    shoulder = pose.pos['RightUpperArm']
    wrist = pose.pos['RightHand']
    before = norm(sub(wrist, shoulder))
    hand = qmul(qaxis(qrot(blade, Y), roll), pose.rot['RightHand'])
    grip = add(grip, (-distance, 0, 0))
    wrist_target = sub(grip, qrot(hand, SLOT_OFFSET['Right']))
    after = norm(sub(wrist_target, shoulder))
    bend = reject(sub(pose.pos['RightLowerArm'], shoulder), before)
    bend = qrot(aim_rotation(before, after), bend)
    pose.arm('Right', wrist_target, bend, hand, reach=.992)


def open_defeat_elbow(pose, tick):
    # Keep the closed grip on the chosen blade path and guide the elbow through
    # the early detour with enough margin for the rigid cuff to pass the guard.
    angle, = curve(tick, [(17, (0,)), (20, (-8,)), (22, (-8,)), (25, (0,))])
    shoulder = pose.pos['RightUpperArm']
    wrist = pose.pos['RightHand']
    axis = norm(sub(wrist, shoulder))
    pole = reject(sub(pose.pos['RightLowerArm'], shoulder), axis)
    bend = qrot(qaxis(axis, angle), pole)
    pose.arm('Right', wrist, bend, pose.rot['RightHand'], reach=.992)


def finish_swipe_recovery(pose, tick):
    # Ease the hilt around the scarf and turn the elbow under the guard.
    # Keep the swing axis and carry the elbow bend into the nearby wrist target.
    x, y, z, roll, elbow_turn = curve(tick, [
        (29, (0, 0, 0, 0, 0)), (30, (0, 0, 0, 0, 0)),
        (31, (.035, -.04, .05, -8, 0)), (32, (-.035, .03, 0, -8, -8)),
        (33, (-.035, .03, 0, -8, -32)), (35, (-.035, .035, .01, -11, -32)),
        (37, (-.045, .05, .03, -21, -30)), (39, (-.020, .02, .02, -18, -36)), (40, (-.025, .01, .028, -17, -34)),
        (42, (-.015, -.005, .022, -6, -12)), (44, (0, 0, 0, 0, 0)),
    ])
    grip, blade = pose.blade()
    grip = add(grip, (x, y, z))
    hand = qmul(qaxis(qrot(blade, Y), roll), pose.rot['RightHand'])
    wrist = sub(grip, qrot(hand, SLOT_OFFSET['Right']))
    shoulder = pose.pos['RightUpperArm']
    before = norm(sub(pose.pos['RightHand'], shoulder))
    after = norm(sub(wrist, shoulder))
    elbow = reject(sub(pose.pos['RightLowerArm'], shoulder), before)
    elbow = qrot(qaxis(after, elbow_turn), qrot(aim_rotation(before, after), elbow))
    pose.arm('Right', wrist, elbow, hand, reach=.992)


def open_swipe_elbow(pose, tick):
    # The hilt has crossed the body. Keep the grip closed and ease the elbow
    # under the guard before the return to the original carry.
    angle, = curve(tick, [(40, (0,)), (43, (-10,)), (50, (-10,)), (55, (0,))])
    shoulder = pose.pos['RightUpperArm']
    wrist = pose.pos['RightHand']
    axis = norm(sub(wrist, shoulder))
    pole = reject(sub(pose.pos['RightLowerArm'], shoulder), axis)
    bend = qrot(qaxis(axis, angle), pole)
    pose.arm('Right', wrist, bend, pose.rot['RightHand'], reach=.992)


def solved_guard():
    pose = guard(0)
    pose.solve_sword(None)
    open_low_carry(pose, .0275, 0)
    return pose


def carry_heavy_stagger(poses):
    # Carry the closed grip back with the struck shoulder. Let the elbow open
    # as the weight drops, then raise the grip outside the belt into the guard.
    settled = solved_guard()
    guard_grip, _ = settled.blade()
    guide = [
        (0, guard_grip), (3, (-.67, 1.29, .26)), (8, (-.79, 1.40, .06)),
        (11, (-.82, 1.30, -.02)), (16, (-.78, .89, .03)), (21, (-.73, .63, .12)),
        (27, (-.73, .60, .18)), (32, (-.70, .71, .22)), (36, (-.68, .94, .30)),
        (41, guard_grip),
    ]
    arm = ('RightUpperArm', 'RightLowerArm', 'RightHand')
    hinge = norm(cross(sub(REST[arm[1]], REST[arm[0]]), sub(REST[arm[2]], REST[arm[1]])))
    accents = [
        (0, (0, 0)), (8, (-18, -20)), (15, (-20.8, 5.2)),
        (23, (-22, 16)), (30, (-22, 16)), (41, (0, 0)),
    ]
    for tick, pose in enumerate(poses):
        for bone in arm:
            pose.turns[bone] = settled.turns[bone]
        flex, elbow_plane = curve(tick, accents)
        pose.turns[arm[1]] = qmul(pose.turns[arm[1]], qaxis(hinge, flex))
        pose.fk()
        shoulder = pose.pos[arm[0]]
        grip, _ = pose.blade()
        carry = norm(sub(grip, shoulder))
        target = norm(sub(curve(tick, guide), shoulder))
        aim = aim_rotation(carry, target)
        transport = qmul(qaxis(target, elbow_plane), aim)
        pose.set_world(arm[0], qmul(transport, pose.rot[arm[0]]))


def plant_attack(pose, body):
    """Change the load through the belt and ribs, then replant the boots."""
    pose.hips = add(pose.hips, body[:3])
    for i, bone in enumerate(('Hips', 'Spine', 'Chest', 'Head')):
        pose.turns[bone] = qmul(pose.turns[bone], turn(*body[3 + i*3:6 + i*3]))
    pose.fk()
    pose.boots(*pose.boot_targets)


def stage_overhead(poses):
    # Settle into the rear boot before lifting the hilt. Return the chest
    # toward the target as the heel reaches into the existing live strike.
    windup = [
        (5, (0,)*15),
        (13, (-.09,-.064,-.072, 4,-3,7, 0,0,0, 0,0,0, -3,5,-5)),
        (17, (-.105,-.078,-.068, 4,-4,8, 0,0,0, 0,0,0, -4,7,-6)),
        (23, (-.08,-.046,-.048, 0,-3,6, 0,0,0, 0,0,0, 2,5,-4)),
        (31, (-.035,-.038,-.022, 1,-1,4, 0,0,0, 0,0,0, 0,2,-3)),
        (37, (0,)*15),
    ]
    for tick in range(6, 37):
        plant_attack(poses[tick], curve(tick, windup))

    # The stock steel query ends at tick 56. Keep that endpoint and let
    # the elbow carry the low hilt out past the armor afterward. The ribs
    # straighten after the rear boot arrives, then the grip comes up to guard.
    body = [
        (56, (0,)*15),
        (60, (.050,-.037,-.059, 3,0,4, -9,-3,-2, -5,-4,1, 2,0,-2)),
        (65, (.041,-.028,-.035, 2,1,3, -7,-2,-2, -4,-3,1, 2,0,-1)),
        (71, (-.019,.015,.010, 0,0,0, -2,1,0, -1,-1,0, 1,0,0)),
        (78, (-.035,.025,.007, -1,0,0, 2,0,-1, 1,-2,1, -1,1,0)),
        (90, (0,)*15),
    ]
    settled = solved_guard()
    guard_grip, guard_blade = settled.blade()
    start_grip, start_blade = poses[56].blade()
    release = [
        (56, start_grip), (57, (.124,.760,.643)), (58, (.089,.765,.727)),
        (60, (-.083,.796,.895)), (63, (-.255,.823,.930)),
        (67, (-.374,.818,.878)), (74, (-.486,.906,.75)), (80, (-.608,1.01,.592)),
        (87, (-.652,1.094,.409)), (95, guard_grip),
    ]
    blade_path = [
        (56, start_blade), (59, turn(114,-9,16)), (63, turn(106,-22,23)),
        (70, turn(100,-20,25)),
        (81, turn(61,-31,21)), (95, guard_blade),
    ]
    pole = [
        (56, (-.52,-.70,-.75)), (60, (-.44,-.58,-.9)),
        (69, (-.56,-.62,-.85)), (78, (-.46,-.75,-.72)), (91, RIGHT_ELBOW),
    ]
    for tick in range(57, 96):
        pose = poses[tick]
        plant_attack(pose, curve(tick, body))
        pose.sword(curve(tick, release), linear(tick, blade_path, slerp), curve(tick, pole))
        pose.cuff_clearance, = curve(tick, [(57,(.025,)), (58,(.18,)), (62,(1,))])
        pose.roll_preference, = curve(tick, [(57,(.6,)), (66,(1.5,)), (77,(4,)), (90,(9,))])
        pose.solve_sword(poses[tick - 1])
    for tick in range(77, len(poses)):
        pose = poses[tick]
        weight, = curve(tick, [(77,(0,)), (84,(.24,)), (92,(.85,)), (96,(1,))])
        for bone in ('Spine', 'Chest', 'Head', 'RightUpperArm', 'RightLowerArm', 'RightHand',
                     'LeftUpperArm', 'LeftLowerArm', 'LeftHand'):
            pose.turns[bone] = slerp(pose.turns[bone], settled.turns[bone], weight)
        pose.fk()


def stage_lunge(poses):
    # Let the belt collect over the push foot while the weapon shoulder is
    # still drawing back. Keep the held blade rigid in the active interval.
    windup = [
        (6, (0,)*15),
        (14, (-.085,-.047,-.025, 3,-3,6, 2,-1,-1, 1,0,-2, -3,4,-3)),
        (20, (-.143,-.054,-.046, 4,-2,8, 3,-2,-2, 1,1,-2, -5,4,-3)),
        (24, (-.115,-.041,-.023, 2,0,6, 2,-1,-1, 1,0,-2, -3,2,-2)),
        (27, (-.051,-.016,.005, 1,0,3, 1,0,-1, 0,0,-1, -1,0,-1)),
        (30, (0,)*15),
    ]
    for tick in range(7, 30):
        plant_attack(poses[tick], curve(tick, windup))

    # Keep the brake low through the second catch. Pull the grip to the
    # outside before the rear elbow folds and the front boot lifts.
    body = [
        (38, (0,)*15),
        (42, (.041,-.036,.031, 3,0,4, 3,-2,-2, -4,-2,-1, -5,2,-2)),
        (46, (.048,-.061,.053, 4,1,4, 4,-1,-3, -3,-2,-1, -4,1,-1)),
        (50, (.024,-.036,.033, 2,2,2, 2,0,-2, -1,-1,0, 0,0,0)),
        (57, (0,.008,-.018, 0,0,0, -2,0,0, 0,0,0, 1,0,0)),
        (66, (0,)*15),
    ]
    clearance = [
        (38, (0,0,0)), (40, (-.048,.003,.092)),
        (42, (-.098,.030,.142)), (45, (-.074,.023,.086)), (48, (0,0,0)),
    ]
    for tick in range(39, 66):
        pose = poses[tick]
        shoulder, elbow, wrist = (pose.pos['Right' + part] for part in ('UpperArm', 'LowerArm', 'Hand'))
        before = norm(sub(wrist, shoulder))
        pole = reject(sub(elbow, shoulder), before)
        hand = pose.rot['RightHand']
        plant_attack(pose, curve(tick, body))
        wrist = add(wrist, curve(tick, clearance))
        axis = norm(sub(wrist, pose.pos['RightUpperArm']))
        pose.arm('Right', wrist, qrot(aim_rotation(before, axis), pole), hand, reach=.992)


def meet_attack_guard(poses, first, finish, handoff):
    # Let the upper arm and elbow bring the closed grip back to carry as
    # the pelvis and boots settle, then carry it into the solved guard.
    guard_pose = solved_guard()
    right = ('RightUpperArm', 'RightLowerArm', 'RightHand')
    upper = ('Spine', 'Chest', 'Head', 'LeftUpperArm', 'LeftLowerArm', 'LeftHand')
    for tick in range(first + 1, len(poses)):
        pose = poses[tick]
        phase = min(1, (tick - first) / (finish - first))
        body, = curve(phase, [(0, (0,)), (1, (1,))])
        for bone in upper:
            pose.turns[bone] = slerp(pose.turns[bone], guard_pose.turns[bone], body)
        for bone, mid in zip(right, handoff):
            weight, = curve(phase, [(0, (0,)), (mid, (.5,)), (1, (1,))])
            pose.turns[bone] = slerp(pose.turns[bone], guard_pose.turns[bone], weight)
        pose.fk()


def samples(name, make, shared_carry=True):
    ticks, build = make()
    poses = []
    right_arm = ('RightUpperArm', 'RightLowerArm', 'RightHand')
    movement_arm = None
    if shared_carry and name in ('advance', 'charge', 'retreat', 'circle_left', 'circle_right'):
        carry = solved_guard()
        movement_arm = tuple(carry.turns[bone] for bone in right_arm)
    fall_arm = None
    if name == 'knockdown':
        # Hold the incoming carry across the toe catch, open through the
        # launch and lower the weapon to the shared down-to-rise pose.
        fall_arm = json.loads(Path(__file__).with_name('fall-carry-arm.json').read_text())
        assert len(fall_arm) == ticks + 1
    for tick in range(ticks + 1):
        pose = build(tick)
        previous = poses[-1] if poses else None
        arm = movement_arm if fall_arm is None else fall_arm[tick]
        if arm is None:
            pose.solve_sword(previous)
        else:
            for bone, rotation in zip(right_arm, arm):
                pose.turns[bone] = tuple(rotation)
            pose.fk()
            pose.solve_free_hand(previous)
        poses.append(pose)
    if name == 'recover':
        guard_return = solved_guard()
        for tick, pose in enumerate(poses):
            gain, = curve(tick, [(4,(0,)), (7,(1,)), (14,(1,)), (16,(0,))])
            if gain:
                shoulder = pose.pos['RightUpperArm']
                hand = pose.pos['RightHand']
                axis = norm(sub(hand, shoulder))
                pole = reject(sub(pose.pos['RightLowerArm'], shoulder), axis)
                pose.arm('Right', hand, qrot(qaxis(axis, -12*gain), pole),
                         pose.rot['RightHand'], reach=.992)
            # Retain the same hilt path while the upper guard clears the cuff.
            roll, = curve(tick, [(2,(0,)), (4,(-4.5,)), (8,(-4.5,)), (10,(-1,)), (12,(0,))])
            if roll:
                open_low_carry(pose, 0, roll)
            # After both feet have taken the load, return to the actual
            # guard's arm pocket while the belt finishes centering.
            guard_weight, = curve(tick, [(31,(0,)), (34,(.22,)), (37,(.78,)), (40,(1,))])
            if guard_weight:
                for bone in ('RightUpperArm', 'RightLowerArm', 'RightHand',
                             'LeftUpperArm', 'LeftLowerArm', 'LeftHand'):
                    pose.turns[bone] = slerp(pose.turns[bone], guard_return.turns[bone], guard_weight)
                pose.fk()
    if name == 'death':
        for tick in range(18, 25):
            open_defeat_elbow(poses[tick], tick)
    if name == 'swipe':
        for tick in range(31, 40):
            finish_swipe_recovery(poses[tick], tick)
        for tick in range(40, ticks + 1):
            pose = build(tick)
            strength, grip, blade, direction, elbow = swipe_approach(tick)
            pose.sword(grip, blade, pose.sword_target[2])
            pose.cuff_clearance = strength
            pose.cuff_approach = direction, elbow
            pose.solve_sword(poses[tick - 1])
            poses[tick] = pose
        for tick in range(41, 55):
            open_swipe_elbow(poses[tick], tick)
        # Keep the released arm on the verified return after the legs have
        # settled into the same pose. Ease out of the earlier load first.
        arms = ('RightUpperArm', 'RightLowerArm', 'RightHand')
        release = json.loads(Path(__file__).with_name('swipe-return-arm.json').read_text())
        for frame, turns in release.items():
            tick = int(frame)
            weight, = curve(tick, [(37,(0,)), (42,(1,))])
            for bone, rotation in zip(arms, turns):
                poses[tick].turns[bone] = slerp(poses[tick].turns[bone], rotation, weight)
            poses[tick].fk()
    if name == 'stagger_light':
        # Carry the arm back into the same measured guard pocket instead of
        # asking the solver to find a separate wrist branch on the last frame.
        settled = guard(0)
        settled.solve_sword(None)
        open_low_carry(settled, .0275, 0)
        for tick in range(15, ticks+1):
            weight, = curve(tick, [(15,(0,)), (19,(1,))])
            for bone in ('RightUpperArm', 'RightLowerArm', 'RightHand'):
                poses[tick].turns[bone] = slerp(poses[tick].turns[bone], settled.turns[bone], weight)
            poses[tick].fk()
    # The locomotion seam closes on the same grip and elbow.
    if name in ('guard', 'advance', 'charge', 'retreat', 'circle_left', 'circle_right'):
        poses[-1] = poses[0]
    if movement_arm is not None:
        # Preserve the support sampling used by the existing gait. A quieter
        # sword arm alone must not reduce the authored body keys.
        _, _, support_keys = samples(name, make, shared_carry=False)
        return ticks, poses, support_keys
    if name == 'overhead':
        stage_overhead(poses)
    elif name == 'lunge':
        stage_lunge(poses)
    # Keep the simulation's active frames verbatim and preserve solved support between keys.
    pinned = {0, ticks}
    if name == 'knockdown':
        pinned.update(range(1, 47))
    if name == 'recover':
        pinned.update(range(2, 25))
        pinned.update(range(32, 40))
    if name in ('swipe', 'overhead', 'lunge'):
        startup, active, _, _, _, _ = move_data(name)
        pinned.update(range(startup, startup + active + 1))
        for tick in range(startup, startup + active + 1):
            pose = poses[tick]
            grip, blade = pose.sword_target[:2]
            actual_grip, actual_blade = pose.blade()
            error = length(sub(actual_grip, grip)) * SCALE
            assert error < .045, f'{name} tick {tick}: blade center misses capsule by {error:.3f} m'
            assert dot(qrot(actual_blade, Y), qrot(blade, Y)) > .999

    def keys(start, end):
        a, b = poses[start], poses[end]
        worst = (0, start)
        for tick in range(start + 1, end):
            mid = poses[tick]
            u = (tick - start) / (end - start)
            worst_bone = max(1 - abs(dot4(mid.turns[bone], slerp(a.turns[bone], b.turns[bone], u)))
                             for bone in BONES)
            worst_hips = length(sub(mid.hips, lerp(a.hips, b.hips, u)))
            worst = max(worst, (max(worst_bone / .000035, worst_hips / .003), tick))
        if worst[0] > 1:
            pivot = worst[1]
            yield from keys(start, pivot)
            yield pivot
            yield from keys(pivot, end)

    result = sorted(pinned)
    selected = {tick for first, last in zip(result, result[1:]) for tick in keys(first, last)}
    # Keep the existing contact and body keys. This pass only redirects the sword arm.
    if name == 'guard':
        for pose in poses[:-1]:
            open_low_carry(pose, .0275, 0)
    elif name == 'circle_left':
        clearance = [
            (0, (.020, 1)), (4, (.017, 0)), (8, (0, 0)), (18, (0, 0)),
            (22, (.017, 3)), (25, (.0375, 8)), (29, (.0375, 8)),
            (32, (.026, 5)), (34, (.020, 1)),
        ]
        for tick, pose in enumerate(poses[:-1]):
            distance, roll = curve(tick * 34 / ticks, clearance)
            if distance > 0:
                open_low_carry(pose, distance, roll)
    if name == 'stagger_heavy':
        carry_heavy_stagger(poses)
    if name == 'lunge':
        first = 47
        assert first in pinned | selected
        meet_attack_guard(poses, first, 66, (.46, .35, .40))
        selected.update(keys(first, ticks))
    return ticks, poses, sorted(pinned | selected)


def dot4(a, b):
    return sum(x*y for x, y in zip(a, b))


def number(value):
    return f'{value:.4f}'.rstrip('0').rstrip('.') if abs(value) >= .00005 else '0'


def ron(name, make, description):
    ticks, poses, selected = samples(name, make)
    lines = [
        '// Generated by tools/motion/warrior.py --write.',
        '// ' + description,
        '(',
        '    type: "sapphire::pipeline::KeyPoseClip",',
        '    version: 1,',
        '    data: (',
        f'        ticks: {ticks},',
        '        keys: [',
    ]
    for tick in selected:
        pose = poses[tick]
        hips = ', '.join(number(v) for v in pose.hips)
        lines.append(f'            (tick: {tick}, ease: Linear, hips: ({hips}), pose: {{')
        values = []
        for bone, rotation in pose.turns.items():
            angles = ', '.join(f'{axis}: {number(v)}' for axis, v in zip('xyz', to_euler(rotation)))
            values.append(f'{bone}: ({angles}),')
        for i in range(0, len(values), 2):
            lines.append('                ' + ' '.join(values[i:i + 2]))
        lines.append('            }),')
    lines.extend(['        ],', '    ),', ')', ''])
    return '\n'.join(lines)


def asset_id(name):
    return str(uuid.uuid5(uuid.UUID(RIG_ID), f'warrior/{name}'))


def meta(name):
    return f'''(
    type: "sapphire::assets::AssetMeta",
    version: 1,
    data: (
        id: "{asset_id(name)}",
        importer: KeyPose((
            rig: "{RIG_ID}",
        )),
    ),
)
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--write', action='store_true', help='write clips and their sidecars')
    args = parser.parse_args()
    for name, (make, description) in CLIPS.items():
        path = FOLDER / f'{name}.keypose.ron'
        files = ((path, ron(name, make, description)), (path.with_suffix('.ron.meta'), meta(name)))
        for file, contents in files:
            if file.exists() and file.read_text() == contents:
                continue
            if not args.write:
                parser.exit(1, f'{file.name} differs; run warrior.py --write\n')
            file.write_text(contents)


if __name__ == '__main__':
    main()
