"""Author the hero's guard and ground strides on the same rig as the sword moves.

Run with --write to regenerate the hero's locomotion key-pose documents in content/lookdev/character/.
"""
import argparse
from math import atan2, cos, degrees, hypot, pi, radians, sin, sqrt
from pathlib import Path

from frames import write_changed
from hem_fold import apply_ground_hem_folds
from moves import HANDS, IDLE_ARMS, IDLE_BODY, IDLE_HIPS, LEFT_FIST, LEFT_OPEN, SIDECAR, finish, idle_feet, start
from pose import HEEL_FROM_TOE, REST, TOE_HEIGHT, Foot, Move, add, length, lerp, polar, qaxis, qinv, qmul, qrot, sub, to_euler, turn
from rig import REPO


HEADER = """Turns are degrees in the hero's rest space, about each joint, X then Y then Z.
The hero faces +Z with its left arm along +X at rest. Ticks are 60 a second.
Written by tools/motion/locomotion.py."""


def guard():
    m = Move('idle_combat', 120, """The sword guard. The weight shifts over the front foot on an inhale; the eye line and sword
point settle after the chest. Both shoes stay where the stance put them.

""" + HEADER)
    start(m, "The stance the attacks start and finish in.")
    for t, x, y, z, turn_y, pitch, hilt, blade, elbow in (
        (29, 0.009, -0.006, 0.009, -1.0, 0.6, 0.9, -1.0, -1.2),
        (55, 0.005, 0.006, 0.004, 1.1, -0.9, 1.8, 2.8, 1.5),
        (86, -0.008, -0.006, -0.009, -0.6, 0.8, -0.5, -1.5, -0.6),
    ):
        body = dict(IDLE_BODY)
        ph, yh, rh = body['Hips']
        body['Hips'] = (ph + 0.4 * pitch, yh + turn_y, rh - 0.35 * turn_y)
        pc, yc, rc = body['Chest']
        body['Chest'] = (pc + pitch, yc - 0.5 * turn_y, rc + 0.4 * turn_y)
        arms = dict(IDLE_ARMS)
        az, el, reach = arms['rh']
        arms['rh'] = (az + hilt, el - 0.4 * hilt, reach + 0.002 * hilt)
        az, el = arms['blade']
        arms['blade'] = (az - 0.6 * hilt, el + blade)
        az, el, reach = arms['lh']
        arms['lh'] = (az - 0.7 * elbow, el + elbow, reach + 0.002 * elbow)
        m.key(t, "", hips=add(IDLE_HIPS, (x, y, z)), body=body, look=(0.4 * turn_y, 1.0),
              **idle_feet(), **arms)
    finish(m, 120, "Back in guard.", 0.0)
    return m


# Keep these speeds in step with character.animgraph.ron.
WALK_SPEED = 2.0
JOG_SPEED = 3.8
RUN_SPEED = 6.0
JOG_TICKS = 34
RUN_TICKS = 26
RUN_LANDING_TICK = 6
STRAFE_SPEED = 5.52
BACK_SPEED = 0.95

# Keep a loose fist in the free hand as running cadence increases. The sword grip is unchanged.
RUN_HANDS = {**HANDS, **{name: lerp(values, LEFT_FIST[name], 0.50) for name, values in LEFT_OPEN.items()},
             'LeftThumbProximal': (0, -23, -12), 'LeftThumbDistal': (0, -26, 0)}

# Each contact trace advances opposite the controller by exactly its distance traveled.
# Swing heights are clearance at the ball, and pitches are positive with the heel up.
WALK_SWING = [
    # tick, position of ball, clearance, foot pitch
    (24, -0.382, 0.000, 57),
    (26, -0.401, 0.051, 50),
    (29, -0.344, 0.075, 34),
    (32, -0.174, 0.091, 22),
    (35,  0.116, 0.086, 8),
    (38,  0.353, 0.081, -9),
    (40,  0.438, 0.062, -16),
    (42,  0.426, 0.039, -14),
]
RUN_SWING = [
    (8, -0.340, 0.000, 63),
    (10, -0.432, 0.109, 68),
    (13, -0.359, 0.196, 58),
    (16, -0.170, 0.215, 43),
    (19,  0.078, 0.219, 23),
    (22,  0.306, 0.185, 10),
    (25,  0.463, 0.135, -3),
    (27,  0.543, 0.100, -8),
    (30,  0.452, 0.014, -5),
]


def curve(keys, tick, first_velocity=None, last_velocity=None):
    """Cubic interpolation with slope limits, so clearance cannot overshoot the contact plane."""
    if tick <= keys[0][0]:
        return keys[0][1:]
    for i in range(len(keys) - 1):
        a, b = keys[i], keys[i + 1]
        if tick > b[0]:
            continue
        dt = b[0] - a[0]
        u = (tick - a[0]) / dt
        u2, u3 = u * u, u * u * u

        def slope(j, k):
            if j == 0:
                if k == 1 and first_velocity is not None:
                    return first_velocity
                p, q = keys[:2]
            elif j == len(keys) - 1:
                if k == 1 and last_velocity is not None:
                    return last_velocity
                p, q = keys[-2:]
            else:
                p, mid, q = keys[j - 1:j + 2]
                lo = (mid[k] - p[k]) / (mid[0] - p[0])
                hi = (q[k] - mid[k]) / (q[0] - mid[0])
                return 2.0 * lo * hi / (lo + hi) if lo * hi > 0.0 else 0.0
            return (q[k] - p[k]) / (q[0] - p[0])
        return tuple((2 * u3 - 3 * u2 + 1) * a[k] + (u3 - 2 * u2 + u) * dt * slope(i, k)
                     + (-2 * u3 + 3 * u2) * b[k] + (u3 - u2) * dt * slope(i + 1, k)
                     for k in range(1, len(a)))
    return keys[-1][1:]


def sole_offset(pitch):
    return qrot(qaxis((1, 0, 0), pitch), HEEL_FROM_TOE)


def shoe(tick, side, running):
    swing = RUN_SWING if running else WALK_SWING
    speed = (RUN_SPEED if running else WALK_SPEED) / 60.0
    first_pitch = swing[-1][3]
    heel_offset = sole_offset(first_pitch)
    toe_z = swing[-1][1] + heel_offset[2] - HEEL_FROM_TOE[2]
    # These pitch curves separate heel rocker, midstance, and push-off.
    pitch_keys = ([(0, first_pitch), (2, 0), (4, 9), (6, 35), (8, 63)] if running else
                  [(0, first_pitch), (5, 0), (11, 0), (16, 12), (21, 38), (24, 57)])
    swing = [(swing[0][0], toe_z - speed * swing[0][0], 0.0, swing[0][3]), *swing[1:-1],
             (swing[-1][0], swing[-1][1], -heel_offset[1] - TOE_HEIGHT, first_pitch)]
    if tick <= swing[0][0]:
        pitch, = curve(pitch_keys, tick)
        z = toe_z - speed * tick
        lift = 0.0
        if pitch < 0:
            # Before the ball touches, keep the heel fixed against the moving floor.
            offset = sole_offset(pitch)
            z += HEEL_FROM_TOE[2] - offset[2]
            lift = -offset[1] - TOE_HEIGHT
    else:
        z, lift, pitch = curve(swing, tick, first_velocity=-speed, last_velocity=-speed)
    # Stay on separate tracks; the lifted foot moves out slightly before stepping through.
    p = (tick - swing[0][0]) / (swing[-1][0] - swing[0][0])
    arc = sin(pi * max(0.0, p)) ** 2
    x = side * ((0.090 if running else 0.096) + 0.023 * arc)
    yaw = side * (3.5 + 2.0 * arc)
    return Foot(x, z, yaw=yaw, pitch=pitch, lift=lift, toe=-max(0, pitch) * (1 - min(0.65, arc)), model=True)


def arm_aim(x, y, z):
    return degrees(atan2(x, z)), degrees(atan2(y, hypot(x, z))), sqrt(x*x + y*y + z*z)


def leg_ceiling(side, foot, hips_x, hips_z, pelvis, knee):
    """Highest pelvis that can reach this shoe with at least `knee` degrees of flex."""
    upper, lower, end = (REST[side + b] for b in ('UpperLeg', 'LowerLeg', 'Foot'))
    a, b = length(sub(lower, upper)), length(sub(end, lower))
    reach = sqrt(a*a + b*b + 2*a*b*cos(radians(knee)))
    ankle = foot.ankle(side)
    socket = add(REST['Hips'], qrot(pelvis, sub(upper, REST['Hips'])))
    dx, dz = socket[0] + hips_x - ankle[0], socket[2] + hips_z - ankle[2]
    return ankle[1] + sqrt(max(0.0, reach*reach - dx*dx - dz*dz)) - socket[1]



def soften_limit(value, limit, width=0.018):
    """Blend into a reach bound without a sharp change of pelvis velocity."""
    below = max(0.0, width - abs(value - limit)) / width
    return min(value, limit) - width * 0.25 * below * below


def arriving_flex(tick, running):
    if running:
        value, = curve([(0, 24.0), (4, 10.0), (24, 10.0), (30, 24.0)], tick)
    else:
        value, = curve([(0, 13.0), (3, 10.0), (34, 10.0), (42, 13.0)], tick)
    return value


def trunk_in_model_space(hips, spine, chest, upper_chest):
    """Give each visible torso segment its own orientation through contact and release."""
    result = {'Hips': hips}
    parent = turn(*hips)
    for name, angles in (('Spine', spine), ('Chest', chest), ('UpperChest', upper_chest)):
        model = turn(*angles)
        result[name] = to_euler(qmul(qinv(parent), model))
        parent = model
    return result


def weight_cycle(tick, running):
    """One support side receives the hips, then gives them back for the opposite catch."""
    if running:
        lateral = [(0, 0.008, 0.8), (2, 0.019, 0.3), (5, 0.032, 2.9),
                   (8, 0.019, 2.0), (11, 0.003, 0.7), (15, -0.008, -0.8)]
        fore_aft = [(0, 0.028), (3, 0.014), (6, 0.025), (9, 0.046), (12, 0.039), (15, 0.028)]
        load = [(0, 0.16), (2, 0.74), (3, 1.0), (6, 0.31), (9, -0.64), (12, -0.38), (15, 0.16)]
    else:
        lateral = [(0, 0.008, 0.5), (4, 0.024, 1.3), (9, 0.039, 3.3),
                   (13, 0.034, 2.7), (17, 0.013, 0.9), (21, -0.008, -0.5)]
        fore_aft = [(0, -0.005), (4, -0.013), (10, 0.004), (14, 0.010), (21, -0.005)]
        load = [(0, 0.14), (4, 1.0), (7, 0.69), (11, -0.34), (16, -0.56), (21, 0.14)]
    half = lateral[-1][0]
    side = 1 if tick % (2 * half) < half else -1
    x, roll = curve(lateral, tick % half)
    z, = curve(fore_aft, tick % half)
    return side * x, z, side * roll, load


def walk_support(tick, keys):
    """Mirror the load and release on the opposite support half."""
    side = 1 if tick % 42 < 21 else -1
    return side * curve(keys, tick % 21)[0]


# The ribs receive the load as the pelvis comes over the shoe, then release
# before the next heel strike. The hilt stays on its previous course through
# that catch and eases across the body once the elbow takes the load.
WALK_RIB_ROLL = [(0, 0.12), (2, -0.05), (4, -0.40), (6, -0.88), (8, -1.0),
                 (10, -0.98), (12, -0.75), (15, -0.49), (19, -0.29), (21, -0.12)]
WALK_HILT_CARRY = [(0, 0.91), (2, 1.0), (6, 1.0), (8, 0.94), (11, 0.44),
                   (14, -0.32), (17, -0.70), (19, -0.82), (21, -0.91)]


def carried_sword(wrist, blade, wrist_frame, tip_frame):
    """Start the hilt in the chest's carry frame and let the tip follow its turn later."""
    return dict(rh=arm_aim(*qrot(wrist_frame, wrist)),
                blade=arm_aim(*qrot(tip_frame, polar(*blade)))[:2])


def sword_support(tick, running):
    """Let the sword arm finish reaching, then fold as weight arrives on its side."""
    if running:
        half = 15
        reach = [(0, -0.55), (2, -0.25), (4, 0.54), (6, 1.0), (8, 0.38),
                 (12, 0.0), (26, 0.0), (28, -0.29), (30, -0.55)]
    else:
        half = 21
        reach = [(0, -0.58), (2, -0.43), (4, 0.28), (7, 1.0), (10, 0.66),
                 (14, 0.11), (17, 0.0), (34, 0.0), (39, -0.31), (42, -0.58)]
    return curve(reach, (tick - half) % (2 * half))[0]


def gait(name, running, bearing=0.0):
    if bearing:
        return walking_or_diagonal_gait(name, running, bearing)
    return forward_gait(name)


def walking_or_diagonal_gait(name, running, bearing):
    ticks = (RUN_SWING if running else WALK_SWING)[-1][0]
    half = ticks // 2
    m = Move(name, ticks, """Left heel contact defines phase zero in each forward or diagonal stride.
The ball follows a separate swing path; the loaded foot stays on the floor as the hips pass over it.
The weapon hand rides beside the body, and the free arm counters the stepping leg.

""" + HEADER)
    for tick in range(ticks + 1):
        lt, rt = tick % ticks, (tick + half) % ticks
        phase = 2 * pi * lt / ticks
        reach, lateral = cos(phase), sin(phase)
        L, R = shoe(lt, 1, running), shoe(rt, -1, running)
        hx, hz, pelvis_roll, support_load = weight_cycle(lt, running)
        # The clavicle gives with the weight before the elbow folds underneath it.
        arm_load = 0.0 if bearing else sword_support(lt, running)
        clavicle_load = 0.0 if bearing else sword_support(lt + 1.4, running)
        elbow_load = max(0.0, arm_load)
        # Keep the free hand alongside the hip through the swing, then let it come back toward
        # the center as it reaches ahead of the ribs.
        lead = -cos(phase - (0.35 if running else 0.24))
        free_pass = 1.0 - lead * lead
        rear_pass = max(0.0, -lateral) * free_pass
        front_pass = max(0.0, lateral) * free_pass
        hem_pass = max(0.0, cos(phase - (5.65 if running else 5.46))) ** 4

        def loading(delay):
            return curve(support_load, (lt - delay) % half)[0]

        if running:
            # The stance knee takes the first drop; shoulders and hilt are still in their prior arc.
            y, = curve([(0, -0.067), (3, -0.082), (6, -0.060), (9, -0.039),
                        (10, -0.036), (12, -0.043), (15, -0.067)], tick % half)
            hip = (0, -8.5 * reach, pelvis_roll)
            spine = (12 + 1.6 * loading(0.5), -1.3 * cos(phase - 0.11), -1.1 * sin(phase + 0.29))
            chest = (19 + 2.2 * loading(1.4), 9.4 * cos(phase - 0.30), -2.6 * sin(phase + 0.11))
            upper = (22 + 2.6 * loading(2.2), 10.7 * cos(phase - 0.38), -3.2 * sin(phase + 0.03))
            carry = cos(phase - 0.49)
            wrist_frame = turn(1.3 * loading(2.0), 8.4 * cos(phase - 0.43), -2.4 * sin(phase + 0.04))
            tip_frame = turn(0.0, 5.2 * cos(phase - 0.77), -1.8 * sin(phase - 0.16))
            wrist = (-0.100 - 0.009 * lateral,
                     -0.329 - 0.014 * carry - 0.017 * loading(3.1) + 0.014 * arm_load,
                     0.105 + 0.048 * carry)
            blade = (-146 + 5.5 * cos(phase - 0.72),
                     -25 + 5.2 * sin(phase - 0.53) + 3.6 * (loading(2.3) - loading(3.7)))
            front_catch = max(0.0, cos(phase - 1.03)) ** 4
            arms = dict(lh=arm_aim(0.064 + 0.024 * free_pass + 0.009 * lateral + 0.058 * rear_pass
                                   + 0.025 * front_pass + 0.029 * hem_pass + 0.020 * front_catch,
                                   -0.306 + 0.066 * lead + 0.015 * rear_pass + 0.008 * front_pass
                                   + 0.011 * hem_pass + 0.007 * front_catch,
                                   -0.020 + 0.177 * lead),
                        shoulder_r=(0, 1.3 * carry, -1.4 + 2.1 * loading(2.5) + 1.2 * clavicle_load),
                        shoulder_l=(0, 2.6 * lead, 2.3 * lead))
            elbow_r = qrot(wrist_frame, (-0.22 + 0.075 * elbow_load, -0.60 - 0.085 * elbow_load, -0.77))
            elbow_l = (0.31 - 0.055 * elbow_load, -0.27, -0.92)
        else:
            y, = curve([(0, -0.052), (4, -0.059), (8, -0.031), (12, -0.014),
                        (15, -0.021), (19, -0.047), (21, -0.052)], tick % half)
            hip = (2 + 0.7 * loading(0.0), -7 * reach, pelvis_roll)
            spine = (3 + 1.0 * loading(0.7), -0.8 * cos(phase - 0.10), -0.9 * sin(phase - 0.03))
            chest = (4 + 1.7 * loading(1.8), 5.8 * cos(phase - 0.24), 2.0 * walk_support(lt, WALK_RIB_ROLL))
            upper = (4 + 2.1 * loading(2.7), 6.9 * cos(phase - 0.31), 2.4 * walk_support(lt - 0.8, WALK_RIB_ROLL))
            carry = walk_support(lt, WALK_HILT_CARRY)
            wrist_frame = turn(0.8 * loading(2.8), 6.0 * cos(phase - 0.36), -1.7 * sin(phase - 0.17))
            tip_frame = turn(0.0, 3.4 * cos(phase - 0.63), -1.1 * sin(phase - 0.34))
            thigh_pass = max(0.0, -cos(phase - 0.65)) ** 2
            wrist = (-0.105 - 0.016 * (0.5 - 0.5 * reach) - 0.029 * thigh_pass,
                     -0.375 - 0.009 * carry - 0.010 * loading(3.7) + 0.018 * arm_load + 0.012 * thigh_pass,
                     0.074 + 0.041 * carry + 0.014 * thigh_pass)
            blade = (-39 + 3.1 * cos(phase - 0.52),
                     -37 + 3.2 * sin(phase - 0.26) + 2.5 * (loading(4.2) - loading(5.9)))
            # At the lead heel catch the free hand is still passing the shorts as a stop
            # gathers the legs. Leave a little room for the hand to settle into guard.
            gather_pass = max(0.0, cos(phase - 0.20)) ** 12
            arms = dict(lh=arm_aim(0.071 + 0.023 * free_pass + 0.006 * lateral + 0.035 * rear_pass
                                   + 0.022 * front_pass + 0.017 * hem_pass + 0.014 * gather_pass,
                                   -0.385 + 0.019 * lead + 0.012 * rear_pass + 0.007 * (front_pass + hem_pass)
                                   + 0.006 * gather_pass,
                                   -0.006 + 0.129 * lead),
                        shoulder_r=(0, 1.1 * carry, -1 + 1.2 * loading(3.0) + 1.4 * clavicle_load),
                        shoulder_l=(0, 2.4 * lead, 2.0 * lead))
            elbow_r = (-0.48 + 0.13 * elbow_load, -0.42 - 0.08 * elbow_load, -0.76)
            elbow_l = (0.33, -0.27, -0.92)
        body = trunk_in_model_space(hip, spine, chest, upper)
        arms.update(carried_sword(wrist, blade, wrist_frame, tip_frame))
        # Preserve a little flexion through the contact and flight extremes.
        if bearing:
            # Move along the diagonal while retaining a view of the target. The hips and feet
            # turn into the step; the upper body and hilt stay available in front.
            heading = qaxis((0, 1, 0), bearing)
            scale = STRAFE_SPEED / RUN_SPEED
            for foot in (L, R):
                foot.x, _, foot.z = qrot(heading, (foot.x, 0.0, scale * foot.z))
                foot.yaw += bearing * 0.78
            hx, _, hz = qrot(heading, (hx, 0.0, hz))
            hips_pitch, hips_yaw, hips_roll = body['Hips']
            body['Hips'] = (hips_pitch, hips_yaw + 0.55 * bearing, hips_roll - 0.046 * bearing)
            chest_pitch, chest_yaw, chest_roll = body['Chest']
            body['Chest'] = (chest_pitch - 2, chest_yaw - 0.19 * bearing, chest_roll - 0.025 * bearing)
            # In the rightward diagonal the ribs roll in toward the free arm. Follow that hip
            # farther out on the return, then make room for the sleeve as the elbow comes through.
            inside_step = max(0.0, (cos(phase - 0.43) + 0.4) / 1.4) if bearing < 0.0 else 0.0
            arms.update(rh=arm_aim(-0.11, -0.279, 0.145 + 0.028 * reach),
                        blade=(-42 + 3.5 * reach, 17 + 4 * lateral),
                        lh=arm_aim(0.111 + 0.016 * free_pass + 0.022 * rear_pass + 0.055 * inside_step,
                                   -0.30 + 0.03 * lead + 0.012 * (rear_pass + inside_step), 0.12 * lead))
            elbow_r = (-0.48, -0.42, -0.76)
            if bearing < 0.0:
                elbow_l = (0.31 + 0.24 * max(0.0, lead) ** 2, -0.27, -0.92)
        pelvis = turn(*body['Hips'])
        y = soften_limit(y, leg_ceiling('Left', L, hx, hz, pelvis, arriving_flex(lt, running)))
        y = soften_limit(y, leg_ceiling('Right', R, hx, hz, pelvis, arriving_flex(rt, running)))
        note = {0: "Left heel contact.", half: "Right heel contact.", ticks: "Left heel contact, loop."}.get(tick, "")
        knees = qaxis((0, 1, 0), 0.81 * bearing)
        eye_level = 2.8 if running else 1.5
        m.key(tick, note, ease='Linear', hips=(hx, y, hz), body=body,
              look=(0.07 * bearing + sin(phase - 0.55), eye_level + 0.65 * loading(4.2), 0.65 * lateral),
              fingers=HANDS if tick == 0 else None, L=L, R=R,
              knee_L=qrot(knees, (0.092 + 0.026 * lateral, 0.0, 1.0)),
              knee_R=qrot(knees, (-0.092 + 0.026 * lateral, 0.0, 1.0)),
              elbow_r=elbow_r, elbow_l=elbow_l,
              lhand=turn(7.0 * free_pass, -2.5 - 3.5 * lead, -4.0 - 4.0 * lead), **arms)
    return m


def running_shoe(tick, side, fast):
    """Heel-to-ball rocker and a swing led by the knee, then caught by the retracting foot."""
    speed = (RUN_SPEED if fast else JOG_SPEED) / 60.0
    ticks = RUN_TICKS if fast else JOG_TICKS
    release = 7 if fast else 12
    # Ball at flat-foot load; the heel, about 16 cm behind, takes the initial contact.
    ahead = 0.365 if fast else 0.354
    catch = -8.0 if fast else -12.0
    push = 61.0 if fast else 56.0
    width = 0.087 if fast else 0.091
    yaw = 4.0 * side
    flat_heel = qrot(qaxis((0, 1, 0), yaw), (0.0, 0.0, HEEL_FROM_TOE[2]))
    roller = ([(0, catch), (1.7, 0), (3, 3), (4.7, 18), (6, 42), (release, push)] if fast else
              [(0, catch), (3, 0), (6, 0), (8, 5), (9.5, 18), (11, 38), (release, push)])
    if tick <= release:
        pitch, = curve(roller, tick)
        if pitch < 0.0:
            return Foot(side * width + flat_heel[0], ahead + flat_heel[2] - speed * tick,
                        yaw=yaw, pitch=pitch, pivot='heel', model=True)
        return Foot(side * width, ahead - speed * tick, yaw=yaw, pitch=pitch,
                    toe=-pitch, pivot='ball', model=True)

    # First curl the heel under the seat; then drive the knee and keep the shin folded. The shoe
    # overtakes the knee only near the catch, when it is already coming backward in world space.
    # z/lift are ball coordinates, pitch is the shoe, and toe is the angle relative to the shoe.
    airborne = ([(8, -.402, .051, 67, -54), (10, -.435, .164, 74, -33),
                 (12, -.354, .280, 73, -16), (14, -.217, .309, 63, -5),
                 (16, -.020, .290, 48, -1), (18, .177, .224, 29, 0),
                 (20, .312, .143, 11, 0), (22, .407, .087, -4, 0),
                 (24, .442, .079, -8, 0), (25, .438, .055, -8, 0)] if fast else
                [(13, -.450, .046, 58, -49), (15, -.467, .132, 64, -32),
                 (17, -.416, .208, 60, -15), (19, -.310, .244, 53, -5),
                 (22, -.101, .237, 38, 0), (25, .139, .179, 22, 0),
                 (28, .314, .116, 5, 0), (30, .409, .105, -5, 0),
                 (32, .434, .077, -10, 0), (33, .414, .056, -12, 0)])
    # Use the same anatomical heel at the end of swing and the start of stance. The height of
    # this ball above the floor is the rotated sole length, not a separate clearance guess.
    catch_heel = qrot(qmul(qaxis((0, 1, 0), yaw), qaxis((1, 0, 0), catch)),
                      (0.0, 0.0, HEEL_FROM_TOE[2]))
    x, lift_offset, z_offset = sub(flat_heel, catch_heel)
    keys = [(release, ahead - speed * release, 0.0, push, -push),
            *airborne, (ticks, ahead + z_offset, lift_offset, catch, 0.0)]
    z, lift, pitch, toe = curve(keys, tick, first_velocity=-speed, last_velocity=-speed)
    swing = (tick - release) / (ticks - release)
    outward = (0.017 if fast else 0.021) * sin(pi * swing) ** 2
    x = side * (width + outward) + x * swing ** 3
    return Foot(x, z, yaw=yaw, pitch=pitch, lift=lift, toe=toe, pivot='ball', model=True)


def stride_curve(keys, tick, period, mirror=None):
    """Interpolate through the loop seam, optionally mirroring a single support half."""
    if mirror is not None:
        keys = [*keys, *((t + period / 2, *(value * sign for value, sign in zip(values, mirror)))
                         for t, *values in keys)]
    before = (keys[-1][0] - period, *keys[-1][1:])
    after = (period, *keys[0][1:])
    after_next = (keys[1][0] + period, *keys[1][1:])
    return curve([before, *keys, after, after_next], tick % period)


# Each row begins a named support beat, in ticks from the receiving heel.
# Pelvis rows contain x, y, z and pitch, yaw, roll. The hips turn onto the planted leg
# while the ribs (spine pitch, chest pitch, upper pitch, upper yaw/roll) take the load later.
WALK_PELVIS = [
    (0, -.014, -.053, -.007,  1, -9, -1.0),
    (4,  .015, -.068, -.017,  4, -8,  1.8),  # Accept weight in double support.
    (9,  .041, -.037, -.002,  3, -3,  3.9),
    (13, .041, -.016,  .012,  0,  3,  3.2),  # Rise over the flat foot.
    (17, .021, -.030,  .013, -1,  8,  1.2),
    (19, .005, -.044,  .004,  0,  9,  0.0),  # Heel rises; the next shoe reaches.
]
JOG_PELVIS = [
    (0, -.010, -.042,  .010,  3, -8.0, -0.8),
    (4,  .022, -.078,  .012,  6, -7.0,  3.2),  # Soft catch and clear load.
    (8,  .035, -.059, -.005,  5, -1.0,  3.3),
    (11, .028, -.049, -.013,  4,  5.0,  2.1),  # Recover height into the ball.
    (13, .016, -.036, -.009,  2,  7.5,  1.1),
    (15, .002, -.023,  .002,  2,  8.5,  0.1),  # Brief travel before the next catch.
]
RUN_PELVIS = [
    (0, -.010, -.055, -.002,  5,  -9, -0.7),
    (2,  .013, -.081, -.018,  8,  -8,  2.6),  # Drop into the receiving knee.
    (4,  .028, -.074,  .004,  8,  -3,  3.5),
    (6,  .034, -.047,  .016,  6,   4,  2.7),  # Hip drives over the forefoot.
    (8,  .024, -.028,  .019,  3,   8,  1.7),  # Release into flight.
    (10, .010, -.022,  .023,  2,  10,  0.6),
    (12,-.005, -.038,  .010,  4,  10,  0.0),
]

WALK_RIBS = [
    (0, 4,  5,  6,  8,  1.7),
    (4, 6,  7,  8, 10, -0.4),
    (7, 5,  8,  9,  9, -3.5),
    (11,3,  4,  5,  4, -4.0),
    (15,2,  2,  3, -3, -2.7),
    (19,3,  4,  5, -7, -1.9),
]
JOG_RIBS = [
    (0, 8,  11, 13,  9,  1.5),
    (3, 11, 14, 16, 12, -0.5),
    (6, 12, 17, 19, 10, -3.9),
    (9, 10, 15, 17,  4, -4.4),
    (12,7,  11, 13, -2, -2.9),
    (15,6,   9, 11, -8, -1.8),
]
RUN_RIBS = [
    (0, 13, 19, 22,   9,  2.2),
    (2, 17, 24, 27,  14,  0.2),
    (4, 19, 26, 29,  12, -4.1),
    (6, 17, 24, 26,   5, -5.2),
    (8, 13, 20, 23,  -3, -4.1),
    (10,11, 17, 20,  -9, -2.8),
    (12,12, 18, 21, -11, -2.2),
]

# The free elbow holds the previous swing through impact and takes over before the push.
# Different half-stride timing keeps the walking reach, jogging float and sprint drive distinct.
WALK_ARM = [(0, -.91), (3, -1), (7, -.84), (12, -.19), (16, .57), (19, .83)]
JOG_ARM = [(0, -.92), (2, -1), (5, -.79), (9, .06), (12, .76), (15, .92)]
RUN_ARM = [(0, -.88), (1, -1), (3, -.84), (6, .07), (9, .79), (11, .94)]


def forward_gait(name):
    walking = name == 'walk'
    fast = name == 'run'
    ticks = WALK_SWING[-1][0] if walking else RUN_TICKS if fast else JOG_TICKS
    half = ticks // 2
    pelvis_keys, rib_keys, arm_keys = {
        'walk': (WALK_PELVIS, WALK_RIBS, WALK_ARM),
        'jog': (JOG_PELVIS, JOG_RIBS, JOG_ARM),
        'run': (RUN_PELVIS, RUN_RIBS, RUN_ARM),
    }[name]
    flex = ([(0, 24), (3, 25), (6, 16), (9, 12), (21, 12), (26, 24)] if fast else
            [(0, 19), (4, 20), (10, 13), (15, 10), (27, 10), (34, 19)])
    m = Move(name, ticks, """Left heel contact defines phase zero. The hips accept the catch, then advance
over the shoe while the ribs finish the preceding turn. The free elbow starts the next
drive. The hilt follows the upper chest; the point finishes the previous arc before coming around.

""" + HEADER)

    def arm_swing(tick):
        return stride_curve(arm_keys, tick, ticks, (-1,))[0]

    for tick in range(ticks + 1):
        lt, rt = tick % ticks, (tick + half) % ticks
        phase = 2 * pi * lt / ticks
        x, y, z, pitch, yaw, roll = stride_curve(pelvis_keys, lt, ticks, (-1, 1, 1, 1, -1, -1))
        h = (pitch, yaw, roll)
        sp, cp, up, counter, rib_roll = stride_curve(rib_keys, lt, ticks, (1, 1, 1, -1, -1))
        body = trunk_in_model_space(h, (sp, .21 * yaw, -.23 * roll),
                                    (cp, .81 * counter, .76 * rib_roll),
                                    (up, counter, rib_roll))
        if walking:
            left, right = shoe(lt, 1, False), shoe(rt, -1, False)
            knee_l, knee_r = arriving_flex(lt, False), arriving_flex(rt, False)
        else:
            left, right = running_shoe(lt, 1, fast), running_shoe(rt, -1, fast)
            knee_l, knee_r = curve(flex, lt)[0], curve(flex, rt)[0]
        pelvis = turn(*h)
        ceiling = min(leg_ceiling('Left', left, x, z, pelvis, knee_l),
                      leg_ceiling('Right', right, x, z, pelvis, knee_r))
        y = soften_limit(y, ceiling)

        # Free hand lifts as it comes through. Keep the elbow outside the shirt and take
        # the shoulder with it before the wrist overtakes. The backstroke lengthens under gravity.
        lead = arm_swing(lt)
        shoulder_lead = arm_swing(lt + .85)
        rear = max(0, -lead)
        reach = max(0, lead)
        pass_hip = 1 - lead * lead
        l_x, l_y, l_z, travel = ((.101, -.354, .022, .145) if walking else
                                 (.106, -.263, .074, .146) if not fast else
                                 (.108, -.222, .113, .163))
        free = (l_x + .034 * rear + .047 * pass_hip + .015 * reach,
                l_y + (.029 if walking else .068) * lead + .011 * pass_hip,
                l_z + travel * lead)

        # All targets below are in the upper chest frame. Countersteer the point with
        # the *change* in the rib angle, so the wrist first follows the chest and the
        # blade takes longer to finish the arc without snapping on the shoulder reversal.
        carry = -arm_swing(lt - 1.1)
        carry_lag = -arm_swing(lt - (3.4 if walking else 2.4))
        _, _, old_pitch, old_yaw, old_roll = stride_curve(
            rib_keys, lt - (3.2 if walking else 2.1), ticks, (1, 1, 1, -1, -1))
        delayed_tip = qmul(qinv(turn(up, counter, rib_roll)), turn(old_pitch, old_yaw, old_roll))
        r_x, r_y, r_z, blade_az, blade_el = ((-.145, -.350, .094, -43, -35) if walking else
                                           (-.149, -.255, .167, -105, -38) if not fast else
                                           (-.154, -.198, .199, -139, -43))
        catch = max(0.0, carry_lag)
        wrist = (r_x - .012 * pass_hip - .015 * max(0, lead),
                 r_y + .023 * carry + .017 * catch,
                 r_z + (.023 if walking else .034) * carry)
        aim = arm_aim(*qrot(delayed_tip, polar(blade_az + 4.0 * carry_lag,
                                               blade_el + 2.7 * carry_lag)))[:2]
        edge = (.85, -.40, .32) if walking else (-.41, -.39, .84) if fast else (-.11, -.17, .98)
        note = {0: "Left heel receives the stride.", half: "Right heel receives the stride.",
                ticks: "Left heel receives the next loop."}.get(tick, "")
        m.key(tick, note, ease='Linear', hips=(x, y, z), body=body,
              look=(.14 * old_yaw, (1.2 if walking else 2.6) + .13 * (sp - cp), .2 * rib_roll),
              fingers=(HANDS if walking else RUN_HANDS) if tick == 0 else None,
              L=left, R=right, knee_L=(.090 + .018 * sin(phase), 0, 1),
              knee_R=(-.090 + .018 * sin(phase), 0, 1),
              arms_in_chest=True, lh=arm_aim(*free), rh=arm_aim(*wrist), blade=aim, edge=qrot(delayed_tip, edge),
              elbow_l=(.44 + .045 * rear, -.42 - .09 * reach, -.86),
              elbow_r=(-.43 + .11 * catch, -.65 - .10 * catch, -.67),
              lhand=turn(3.0 + 6.0 * pass_hip + 9.0 * reach * reach, -3.0 - 4.5 * lead, -4 - 4 * lead),
              shoulder_l=(0, (3.7 if walking else 5.2) * shoulder_lead, 2.9 * shoulder_lead),
              shoulder_r=(.8 * carry_lag, 2.0 * carry, -1.0 - .62 * rib_roll + 1.3 * catch))
    return m



BACK_SWING = [
    (31, 0.298, 0.040, -15),
    (34, 0.286, 0.067, -15),
    (38, 0.178, 0.088, -8),
    (42, 0.001, 0.087, 4),
    (46, -0.160, 0.061, 13),
    (50, -0.227, 0.029, 22),
    (54, -0.206, 0.000, 23),
]


def back_shoe(tick, side):
    z0 = BACK_SWING[-1][1]
    pitch_keys = [(0, 23), (4, 12), (8, 0), (20, 0), (25, -3), (31, -15)]
    if tick <= BACK_SWING[0][0]:
        pitch, = curve(pitch_keys, tick)
        z = z0 + tick * (BACK_SPEED / 60.0)
        lift = 0.0
        if pitch < 0:
            offset = sole_offset(pitch)
            z += HEEL_FROM_TOE[2] - offset[2]
            lift = -offset[1] - TOE_HEIGHT
    else:
        end = BACK_SWING[0][0]
        release = back_shoe(end, side)
        velocity = BACK_SPEED / 60.0
        z, lift, pitch = curve([(end, release.z, release.lift, release.pitch), *BACK_SWING[1:]],
                               tick, first_velocity=velocity, last_velocity=velocity)
    swing = max(0.0, (tick - BACK_SWING[0][0]) / (BACK_SWING[-1][0] - BACK_SWING[0][0]))
    width = 0.116 + 0.015 * sin(pi * swing) ** 2
    return Foot(side * width, z, yaw=side * 9, pitch=pitch, lift=lift, model=True,
                toe=-max(0.0, pitch) * (1 - sin(pi * swing) ** 2))


def back_gait():
    ticks = BACK_SWING[-1][0]
    half = ticks // 2
    m = Move('walk_back', ticks, """Steps backward without turning the back to the target. The ball of
one foot finds the floor behind the hips before the other foot unloads. The sword stays in guard.

""" + HEADER)
    for tick in range(ticks + 1):
        lt, rt = tick % ticks, (tick + half) % ticks
        phase = 2 * pi * lt / ticks
        load, reach = sin(phase), cos(phase)
        left, right = back_shoe(lt, 1), back_shoe(rt, -1)
        body = dict(Hips=(-2, -5 * reach, -3 * load), Spine=(2, 2 * reach, 1.8 * load),
                    Chest=(1, 4 * reach, 1.5 * load), UpperChest=(0, 1.5 * reach, 0.3 * load))
        hx, hz = 0.024 * load, -0.018 + 0.012 * reach
        y = -0.044 - 0.012 * cos(phase * 2 - 0.35)
        pelvis = turn(*body['Hips'])
        y = min(y, leg_ceiling('Left', left, hx, hz, pelvis, 12.0),
                leg_ceiling('Right', right, hx, hz, pelvis, 12.0))
        m.key(tick, "", ease='Linear', hips=(hx, y, hz), body=body,
              look=(0.5 * load, 1.0, 0.75 * load), L=left, R=right,
              fingers=HANDS if tick == 0 else None,
              rh=arm_aim(-0.082, -0.310, 0.163 + 0.035 * reach), blade=(-28 + 3 * reach, 27 + 4 * load),
              elbow_r=(-0.66, -0.65, -0.32), lh=arm_aim(0.122 + 0.012 * reach, -0.329, 0.075 - 0.066 * reach),
              elbow_l=(0.44, -0.48, -0.72), lhand=turn(0.0, -3.0, -3.0 - 3.0 * load))
    return m

def brake_pose(support):
    """The old support checks the ribs. The receiving heel catches before the back step closes."""
    side = 1 if support == 'left' else -1
    m = Move(f'brake_{support}', 20, """Body keys for the grounded braking phrase.
The initial support checks momentum; a receiving heel then takes the load before the back
shoe closes. The delayed hilt and blade finish the running swing while the hips settle.
The runtime places the shoes from the previous solved heel and ball, on the same owner clock.

""" + HEADER)
    # Tick, hip x/y/z, pitches from pelvis to upper chest, twist, roll,
    # shoulder-to-wrist positions and sword aim.
    keys = [
        (0,  (.025, -.068,  .018), ( 5, 13, 18, 20),   5,  0, (.146, -.270, .112), (-.145, -.286, .106), (-138, -19)),
        (2,  (.042, -.092, -.027), ( 2,  9, 16, 18),   8, -3, (.172, -.231, .166), (-.160, -.299, .091), (-140, -17)),
        (5,  (.036, -.118, -.093), (-5, -2, -5, -8),  11, -4, (.212, -.193, .160), (-.177, -.282, .085), (-132, -10)),
        (8,  (-.022,-.156, -.112), (-9, -3,  3,  6),  -7,  5, (.198, -.175, .176), (-.158, -.261, .106), (-113,   0)),
        (11, (-.056,-.136, -.082), (-5,  1,  5,  7), -10,  6, (.171, -.205, .152), (-.133, -.245, .153), ( -89,  12)),
        (15, (-.032,-.076, -.052), ( 2,  3,  0, -1),  -5,  3, (.146, -.260, .150), (-.102, -.243, .189), ( -48,  27)),
    ]
    notes = {0: 'Carry the outgoing stride.', 2: 'The hips begin to check while the shoulders are still moving.',
             5: 'Sit behind the catch and open the free elbow for balance.',
             8: 'The receiving heel takes weight and the ribs give over it.',
             11: 'The old support can leave. The hilt finally comes forward.',
             15: 'Stand up through the closing step while the tip catches the hand.'}
    for t, (x, y, z), (hip, spine, chest, upper), yaw, roll, left, right, blade in keys:
        body = trunk_in_model_space((hip, -9 + side * yaw, .55 * side * roll),
                                    (spine, -.15 * side * yaw, .68 * side * roll),
                                    (chest, -.77 * side * yaw, side * roll),
                                    (upper, -.96 * side * yaw, 1.2 * side * roll))
        m.key(t, notes[t], hips=(side * x, y, z), body=body, **idle_feet(),
              look=(0.3 * side * yaw, 1.8 if t < 11 else 1.0, -.25 * side * roll),
              fingers=HANDS if t == 0 else None,
              lh=arm_aim(*left), elbow_l=(.71, -.44, -.51), lhand=turn(6, -5, -8),
              shoulder_l=(0, 0.29 * side * yaw, -2.0), shoulder_r=(0, -0.16 * side * yaw, 1.0),
              rh=arm_aim(*right), elbow_r=(-.50, -.65, -.57), blade=blade)
    finish(m, 20, 'The weight and sword settle into guard.', 0.0)
    return m


def launch_pose(support):
    """Bend over the old support, drive away from its ball, and catch the new stride."""
    side = 1 if support == 'left' else -1
    m = Move(f'launch_{support}', 17, """Body keys for the grounded start and restart phrase.
The pelvis moves into the loaded shoe before pushing away. The free elbow counters the
receiving knee while the sword-side shoulder comes through with the hilt still behind it.
The runtime solves the starting rocker and heel-first receiving step on the same owner clock.

""" + HEADER)
    start(m, 'The previously displayed guard or recovering posture carries in.')
    # Tick, hip x/y/z, pitches from pelvis to upper chest, twist, roll,
    # free-hand counter swing, sword wrist position and sword aim.
    keys = [
        (2,  (.049, -.107, -.024), ( 3, 10, 13, 15),  7, -4, .014, (-.127, -.270, .105), (-55,  21)),
        (4,  (.066, -.142,  .038), ( 9, 19, 25, 29), 13, -6, .112, (-.160, -.292, .025), (-87,   5)),
        (6,  (.032, -.096,  .091), (12, 23, 30, 33), 16, -3, .181, (-.155, -.278, .014), (-113,-11)),
        (8,  (-.020,-.116,  .068), ( 9, 19, 25, 27), 11,  3, .176, (-.144, -.255, .054), (-131,-18)),
        (10, (-.035,-.107,  .039), ( 7, 16, 21, 23),  5,  4, .126, (-.131, -.259, .095), (-139,-17)),
        (13, (-.016,-.068,  .022), ( 6, 14, 19, 21), -2,  2, .071, (-.132, -.276, .124), (-137,-18)),
        (17, (.014, -.058,  .012), ( 5, 13, 19, 21), -7, -1, .026, (-.137, -.276, .128), (-136,-18)),
    ]
    notes = {2: 'Weight drops over the first shoe before the hip drive.',
             4: 'The hips and shoulders break forward while the hilt lags.',
             6: 'The old ball gives the last push. The opposite elbow drives.',
             8: 'The new heel checks the pelvis. The sword is still settling behind the body.',
             10: 'Accept the first stride through the knee and ribs.',
             13: 'The trailing arm recovers into the next step.',
             17: 'Carry through into the synced gait.'}
    for t, (x, y, z), (hip, spine, chest, upper), yaw, roll, counter, right, blade in keys:
        body = trunk_in_model_space((hip, side * yaw, .5 * side * roll),
                                    (spine, -.22 * side * yaw, .68 * side * roll),
                                    (chest, -.78 * side * yaw, side * roll),
                                    (upper, -.96 * side * yaw, 1.2 * side * roll))
        wrist_l = (.125 + .055 * counter, -.265 + .25 * counter * side, .023 + side * counter)
        wrist_r = (right[0], right[1], right[2] + (1 - side) * counter * .25)
        m.key(t, notes[t], hips=(side * x, y, z), body=body, **idle_feet(),
              look=(.19 * side * yaw, 2.5, -.26 * side * roll),
              lh=arm_aim(*wrist_l), elbow_l=(.55, -.39, -.74), lhand=turn(8 + 44 * counter, -6, -8),
              shoulder_l=(0, -15 * side * counter, 2 * side * counter),
              shoulder_r=(0, .15 * side * yaw, 1),
              rh=arm_aim(*wrist_r), elbow_r=(-.39, -.66, -.64), blade=blade)
    return m


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--write', action='store_true')
    parser.add_argument('clips', nargs='*',
                        help='Limit validation and writing to the named clips (default: all).')
    args = parser.parse_args()
    folder = Path(REPO) / 'content/lookdev/character'
    clips = [
        (guard(), None),
        (gait('walk', False), 'fbec3961-0e19-4289-9f89-6bba045a6f33'),
        (gait('jog', True), '83268f6b-36f8-5637-aec5-a326de518b6e'),
        (gait('run', True), '4315c96c-311a-4762-90f7-90bef1b89ec4'),
        (back_gait(), '8de71a71-5b97-536a-9aae-3d5a6fcc4450'),
        (gait('strafe_left', True, 60), '2690ea79-80aa-5daf-9ac5-c04f22cbe329'),
        (gait('strafe_right', True, -60), '7b639f71-2c36-51ae-9b4d-5bfd4a34f2ea'),
    ]
    clips += [
        (brake_pose('left'), '4696608f-e046-4b30-989d-5c3b4a03d45c'),
        (brake_pose('right'), '3c2c3d8c-dfe4-49f1-b6eb-2b6b341f7c16'),
        (launch_pose('left'), '4f0ea3c9-3da7-4c56-953c-676724a9a13e'),
        (launch_pose('right'), 'da0c530e-a6c3-4895-9bc4-d6a5507bd3b9'),
    ]
    unknown = set(args.clips) - {m.name for m, _ in clips}
    if unknown:
        parser.error(f"unknown locomotion clips: {', '.join(sorted(unknown))}")
    for m, new_id in clips:
        if args.clips and m.name not in args.clips:
            continue
        apply_ground_hem_folds(m)
        m.check()
        if args.write:
            path = folder / f'{m.name}.keypose.ron'
            write_changed(path, m.ron())
            if new_id is not None:
                write_changed(path.with_suffix('.ron.meta'), SIDECAR % new_id)


if __name__ == '__main__':
    main()
