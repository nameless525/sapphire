"""Lower tee folds for ground travel, jumping, and the deep hit recovery."""
from math import atan2, degrees

from pose import qinv, qrot, sub

LEFT = '__hem_fold_left'
RIGHT = '__hem_fold_right'
DEEP_LEFT = '__hem_fold_deep_left'
DEEP_RIGHT = '__hem_fold_deep_right'
RECOVERY_REAR = '__hem_fold_recovery_rear'

# Follow the thigh that carries the load. The moving launch starts with room
# for either gait phase, then the lead thigh gathers under the same panel.
WEIGHTS = {
    'jump_prepare': {
        0: (0, 0), 1: (.15, 0), 2: (.30, 0), 3: (.40, 0), 4: (.22, 0), 5: (.10, 0),
    },
    'air': {
        0: (.10, 0), 3: (.20, 0), 7: (.65, .15), 12: (.85, .60), 17: (1, .65),
        21: (.90, .55), 27: (.60, .20), 33: (.30, .05), 37: (.20, 0), 40: (.20, 0),
    },
    'air_run': {
        0: (.28, .34), 4: (.55, .16), 8: (.85, .28), 13: (.88, .46), 18: (.85, .48),
        22: (.85, .36), 28: (.64, .18), 33: (.32, .05), 37: (.20, 0), 40: (.20, 0),
    },
    'land': {
        0: (.20, 0), 1: (.275, 0), 2: (.35, 0), 3: (.45, 0), 4: (.55, 0), 6: (.60, 0),
        10: (.36, 0), 15: (.15, 0), 20: (0, 0),
    },
    'land_move': {
        0: (.20, 0), 1: (.34, 0), 3: (.44, 0), 5: (.28, 0),
        7: (.19, 0), 10: (.20, 0), 14: (.10, 0), 18: (0, 0),
    },
    # On the running catches, the load moves from the arriving left ball to
    # the advancing right thigh. Carry the fold through the graph crossfade.
    'land_run': {
        0: (.20, 0), 1: (.24, .12), 2: (.21, .41), 3: (.15, .73),
        4: (.06, .94), 5: (0, 1), 6: (0, 1),
    },
    'land_accel': {
        0: (.20, 0), 1: (.18, .22), 2: (.11, .51), 3: (.04, .78), 4: (0, .90),
        5: (0, .70), 6: (.05, .40), 7: (.12, .21), 8: (.23, .11), 9: (.38, .04),
    },
}


# Flex of the thigh in the pelvis frame, measured over a complete run stride.
# Leave the straight leg free and let the folded panel move with the rising knee.
THIGH_FLEX = ((31, 0), (34, .06), (39, .16), (46, .28), (53, .41),
              (61, .54), (69, .70), (76, .85), (81, .92))

BRAKE = {
    'left': {0: .35, 2: .42, 5: .40, 8: .37, 11: .32, 15: .12, 20: 0},
    'right': {0: .65, 2: .58, 5: .43, 8: .52, 11: .38, 15: .27, 20: 0},
}
LAUNCH_RIGHT = {
    0: (0, 0), 2: (.28, .28), 4: (.64, .40), 6: (.63, .28),
    8: (.36, .09), 10: (.19, .04), 13: (.04, .10), 17: (0, 0),
}
# A short rise over the receiving thigh during push-off and the first catch.
LAUNCH_DEEP = {0: 0, 2: .04, 4: .30, 6: .30, 8: .28, 10: .09, 13: 0, 17: 0}

# Turn the receiving front edge before the knee reaches its highest point.
JOG_DEEP = {1: .02, 2: .05, 3: .09, 4: .135, 5: .135, 6: .095, 7: .07, 8: .045}

# The rear panel folds only after the hips clear the floor.
RECOVERY_REAR_WEIGHTS = {34: .58, 36: .82, 38: 1, 40: 1, 44: 1, 48: .58}


def run_folds(pose):
    from_model = qinv(pose.rot['Hips'])

    def follow_thigh(side):
        _, y, z = qrot(from_model, sub(pose.pos[side + 'LowerLeg'], pose.pos[side + 'UpperLeg']))
        angle = degrees(atan2(z, -y))
        for (lo, low_weight), (hi, high_weight) in zip(THIGH_FLEX, THIGH_FLEX[1:]):
            if angle <= hi:
                u = max(0.0, (angle - lo) / (hi - lo))
                return low_weight + u * (high_weight - low_weight)
        return THIGH_FLEX[-1][1]

    return follow_thigh('Left'), follow_thigh('Right')


def set_folds(key, left, right):
    key.setdefault('morphs', {}).update({LEFT: left, RIGHT: right})


def set_deep_folds(key, left, right):
    key.setdefault('morphs', {}).update({DEEP_LEFT: left, DEEP_RIGHT: right})


def run_deep_fold(gain):
    # The front edge turns a little further only at the top of the stride.
    rise = min(1., max(0., (gain - .52) / .40))
    return .19 * rise * rise * (3 - 2 * rise)


def apply_ground_hem_folds(move):
    if move.name == 'jog':
        half_stride = move.ticks // 2
        for key in move.keys:
            right_phase = key['tick'] % move.ticks
            left_phase = (right_phase + half_stride) % move.ticks
            set_deep_folds(key, JOG_DEEP.get(left_phase, 0), JOG_DEEP.get(right_phase, 0))
    elif move.name == 'run':
        for key in move.keys:
            left, right = run_folds(key['pose'])
            set_folds(key, left, right)
            set_deep_folds(key, run_deep_fold(left), run_deep_fold(right))
    elif move.name.startswith('brake_'):
        support = move.name.removeprefix('brake_')
        for key in move.keys:
            gain = BRAKE[support][key['tick']]
            set_folds(key, *((0, gain) if support == 'left' else (gain, 0)))
    elif move.name.startswith('launch_'):
        right_support = move.name == 'launch_right'
        for key in move.keys:
            weights = LAUNCH_RIGHT[key['tick']]
            set_folds(key, *(weights if right_support else weights[::-1]))
            deep_fold = DEEP_LEFT if right_support else DEEP_RIGHT
            key['morphs'][deep_fold] = LAUNCH_DEEP[key['tick']]


def apply_hem_folds(move):
    if move.name == 'knockdown':
        assert RECOVERY_REAR_WEIGHTS.keys() <= {key['tick'] for key in move.keys}, 'Refit the recovery hem weights'
        for key in move.keys:
            key.setdefault('morphs', {})[RECOVERY_REAR] = RECOVERY_REAR_WEIGHTS.get(key['tick'], 0)
        return
    weights = WEIGHTS.get(move.name)
    if weights is None:
        return
    if move.name in ('land_run', 'land_accel'):
        # These endpoint poses are copied from the run keys they rejoin.
        resume = move.keys[-1]
        left, right = run_folds(resume['pose'])
        weights = {**weights, resume['tick']: (left, right)}
        set_deep_folds(resume, run_deep_fold(left), run_deep_fold(right))
    assert set(weights) == {key['tick'] for key in move.keys}, f'Refit the hem weights for {move.name}'
    for key in move.keys:
        set_folds(key, *weights[key['tick']])
