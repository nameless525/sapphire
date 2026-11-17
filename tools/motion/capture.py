"""A synthetic motion capture: two strides of a walk, written as BVH for the pipeline's BVH importer (M6 A3).

Agents download no capture, so this stands in for one, with what a capture brings that a hand-keyed clip
doesn't: a hierarchy resting in an A-pose (each arm hangs 45 degrees below the horizontal with every channel
at zero), 30 frames a second, centimeters, positions in the capture volume, noise on every channel, and a
foot that slides. The left foot's second stance slides `SLIDE` meters forward over frames `INJECTED`, and no
planted foot moves anywhere else beyond the noise: the pipeline's clip report must find it there and only
there.

The legs are solved by inverse kinematics from where each ankle should be, so a planted foot stays planted
to the rounding of four decimals. Hips, legs and feet turn about X alone (with the noise), the knee forward.

It writes the same bytes on every machine: only IEEE arithmetic, `math.sqrt` (correctly rounded everywhere)
and `math.floor`, with its own sine and arctangent, never the platform's. `test_capture.py` holds the
committed file to what this writes.

    python3 tools/motion/capture.py          # checks the walk, writes nothing
    python3 tools/motion/capture.py --write  # writes tools/sapphire_pipeline/fixtures/bvh/synthetic_walk.bvh
"""
import math
import os
import sys

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUTPUT = os.path.join(REPO, 'tools/sapphire_pipeline/fixtures/bvh/synthetic_walk.bvh')

FPS = 30
FRAMES = 61  # two seconds, both ends
CYCLE = 1.0  # seconds a stride, two steps
SPEED = 0.9  # meters a second, along +Z
STANCE = 0.6  # of a stride, each foot
STRIKE = SPEED * CYCLE * STANCE / 2  # meters ahead of the hips where a foot lands
LIFT = 0.08  # meters a swinging ankle rises
ANKLE = 0.08  # meters above the floor a planted ankle stands

# The injected slide: the left foot's second stance (frames 30 to 47) drifts forward over these frames, at
# most 0.23 m/s, slow enough that the pipeline's contacts keep the foot planted through it.
INJECTED = (34, 44)
SLIDE = 0.05

HIP_WIDTH = 0.09
SOCKET_DROP = 0.06
THIGH = 0.42
SHIN = 0.42

PI = 3.141592653589793


def _sin(x):
    """Sine by its series after reducing to [-pi/2, pi/2]: arithmetic alone, so every machine agrees."""
    x = x - 2.0 * PI * math.floor(x / (2.0 * PI) + 0.5)
    if x > PI / 2:
        x = PI - x
    elif x < -PI / 2:
        x = -PI - x
    x2 = x * x
    term, total = x, x
    for n in range(1, 12):
        term = -term * x2 / ((2 * n) * (2 * n + 1))
        total += term
    return total


def _cos(x):
    return _sin(x + PI / 2)


def _atan(x):
    """Arctangent: past 1 by its reflection, then halved twice and summed as a series."""
    if x > 1.0:
        return PI / 2 - _atan(1.0 / x)
    if x < -1.0:
        return -PI / 2 - _atan(1.0 / x)
    for _ in range(2):
        x = x / (1.0 + math.sqrt(1.0 + x * x))
    x2 = x * x
    term, total = x, x
    for n in range(1, 14):
        term = -term * x2
        total += term / (2 * n + 1)
    return 4.0 * total


def _atan2(y, x):
    if x > 0.0:
        return _atan(y / x)
    if x < 0.0:
        return _atan(y / x) + (PI if y >= 0.0 else -PI)
    return PI / 2 if y > 0.0 else (-PI / 2 if y < 0.0 else 0.0)


def _acos(x):
    return _atan2(math.sqrt(max(0.0, 1.0 - x * x)), x)


def _degrees(radians):
    return radians * (180.0 / PI)


def _smooth(s):
    s = min(1.0, max(0.0, s))
    return s * s * (3.0 - 2.0 * s)


def _noise(frame, channel):
    """A number in [-1, 1) from the frame and channel alone: splitmix64."""
    z = (frame * 0x9E3779B97F4A7C15 + channel * 0xBF58476D1CE4E5B9 + 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
    z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
    z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
    z ^= z >> 31
    return (z >> 11) / float(1 << 52) - 1.0


# The hierarchy: name, parent, offset in meters, whether it has channels. An end site is a name of None.
_ARM = 0.27 / math.sqrt(2.0)
_FOREARM = 0.25 / math.sqrt(2.0)
_HAND = 0.08 / math.sqrt(2.0)
JOINTS = [
    ('Hips', None, (0.0, 0.0, 0.0)),
    ('Spine', 'Hips', (0.0, 0.10, 0.0)),
    ('Spine1', 'Spine', (0.0, 0.15, 0.0)),
    ('Neck', 'Spine1', (0.0, 0.20, 0.0)),
    ('Head', 'Neck', (0.0, 0.10, 0.0)),
    ('LeftShoulder', 'Spine1', (0.04, 0.17, 0.0)),
    ('LeftArm', 'LeftShoulder', (0.13, 0.0, 0.0)),
    ('LeftForeArm', 'LeftArm', (_ARM, -_ARM, 0.0)),
    ('LeftHand', 'LeftForeArm', (_FOREARM, -_FOREARM, 0.0)),
    ('RightShoulder', 'Spine1', (-0.04, 0.17, 0.0)),
    ('RightArm', 'RightShoulder', (-0.13, 0.0, 0.0)),
    ('RightForeArm', 'RightArm', (-_ARM, -_ARM, 0.0)),
    ('RightHand', 'RightForeArm', (-_FOREARM, -_FOREARM, 0.0)),
    ('LeftUpLeg', 'Hips', (HIP_WIDTH, -SOCKET_DROP, 0.0)),
    ('LeftLeg', 'LeftUpLeg', (0.0, -THIGH, 0.0)),
    ('LeftFoot', 'LeftLeg', (0.0, -SHIN, 0.0)),
    ('LeftToeBase', 'LeftFoot', (0.0, -0.06, 0.12)),
    ('RightUpLeg', 'Hips', (-HIP_WIDTH, -SOCKET_DROP, 0.0)),
    ('RightLeg', 'RightUpLeg', (0.0, -THIGH, 0.0)),
    ('RightFoot', 'RightLeg', (0.0, -SHIN, 0.0)),
    ('RightToeBase', 'RightFoot', (0.0, -0.06, 0.12)),
]
END_SITES = {
    'Head': (0.0, 0.18, 0.0),
    'LeftHand': (_HAND, -_HAND, 0.0),
    'RightHand': (-_HAND, -_HAND, 0.0),
    'LeftToeBase': (0.0, 0.0, 0.05),
    'RightToeBase': (0.0, 0.0, 0.05),
}
ROOT_CHANNELS = 'Xposition Yposition Zposition Zrotation Xrotation Yrotation'
JOINT_CHANNELS = 'Zrotation Xrotation Yrotation'
LOWER_BODY = {'Hips', 'LeftUpLeg', 'LeftLeg', 'LeftFoot', 'LeftToeBase',
              'RightUpLeg', 'RightLeg', 'RightFoot', 'RightToeBase'}


def hips_at(t):
    """The hips' place, meters: travelling at `SPEED`, lowest as both feet are down."""
    phase = t / CYCLE - math.floor(t / CYCLE)
    return (0.0, 0.94 - 0.0275 * _cos(4.0 * PI * (phase - 0.05)), SPEED * t)


def ankle_at(frame, side):
    """Where a foot's ankle should be on `frame`, meters, and whether it's planted. The right foot runs half
    a stride behind the left."""
    t = frame / FPS
    shift = 0.0 if side == 'Left' else 0.5
    x = HIP_WIDTH if side == 'Left' else -HIP_WIDTH
    local = t / CYCLE - shift
    stride = math.floor(local)
    phase = local - stride

    def plant(k):
        """Where stance `k` stands, with the injected slide's offset by the end of it."""
        z = SPEED * CYCLE * (k + shift) + STRIKE
        return z, (SLIDE if side == 'Left' and k == 1 else 0.0)

    if phase < STANCE:
        z, _ = plant(stride)
        if side == 'Left' and stride == 1:
            z += SLIDE * _smooth((frame - INJECTED[0]) / (INJECTED[1] - INJECTED[0]))
        return (x, ANKLE, z), True
    s = (phase - STANCE) / (1.0 - STANCE)
    start, slid = plant(stride)
    end, _ = plant(stride + 1)
    start += slid
    rise = 16.0 * s * s * (1.0 - s) * (1.0 - s)
    return (x, ANKLE + LIFT * rise, start + (end - start) * _smooth(s)), False


def solve_leg(hips, ankle, side):
    """The thigh's, knee's and foot's turns about X, radians, that put the ankle at `ankle` with the knee
    forward and the foot flat."""
    socket_y = hips[1] - SOCKET_DROP
    dy, dz = ankle[1] - socket_y, ankle[2] - hips[2]
    reach = math.sqrt(dy * dy + dz * dz)
    if reach > THIGH + SHIN - 1e-6:
        raise ValueError(f'the {side} leg can\'t reach: {reach:.4f} m')
    # A turn of `a` about X carries a bone hanging down along -Y to (y, z) = (-cos a, -sin a).
    toward = _atan2(-dz, -dy)
    bend = _acos((THIGH * THIGH + reach * reach - SHIN * SHIN) / (2.0 * THIGH * reach))
    thigh = toward - bend
    knee = 2.0 * bend
    return thigh, knee, -(thigh + knee)


def frame_values(frame):
    """Every channel's value on `frame`, in the hierarchy's order: centimeters and degrees."""
    t = frame / FPS
    phase = t / CYCLE - math.floor(t / CYCLE)
    hips = hips_at(t)
    turns = {name: [0.0, 0.0, 0.0] for name, _, _ in JOINTS}  # Z, X, Y in degrees
    for side in ('Left', 'Right'):
        thigh, knee, foot = solve_leg(hips, ankle_at(frame, side)[0], side)
        turns[side + 'UpLeg'][1] = _degrees(thigh)
        turns[side + 'Leg'][1] = _degrees(knee)
        turns[side + 'Foot'][1] = _degrees(foot)
    swing = _cos(2.0 * PI * phase)
    turns['Spine'][1] = 3.0 + 1.5 * _sin(4.0 * PI * phase)
    turns['Spine1'][2] = 4.0 * swing
    turns['Neck'][2] = -2.0 * swing
    turns['Head'][1] = -3.0
    turns['LeftArm'] = [-25.0, 14.0 * swing, 0.0]
    turns['RightArm'] = [25.0, -14.0 * swing, 0.0]
    turns['LeftForeArm'][1] = -15.0 - 5.0 * swing
    turns['RightForeArm'][1] = -15.0 + 5.0 * swing

    values = []
    channel = 0

    def add(value, amplitude):
        nonlocal channel
        values.append(value + amplitude * _noise(frame, channel))
        channel += 1

    for position in hips:
        add(100.0 * position, 0.02)
    for name, _, _ in JOINTS:
        # Capture noise: under a tenth of a millimeter at a foot from the legs, a few tenths of a degree above.
        amplitude = 0.02 if name in LOWER_BODY else 0.3
        for value in turns[name]:
            add(value, amplitude)
    return values


def _number(value):
    text = f'{value:.4f}'
    return '0.0000' if text == '-0.0000' else text


def bvh():
    """The file, as text."""
    lines = ['HIERARCHY']
    children = {}
    for name, parent, _ in JOINTS:
        children.setdefault(parent, []).append(name)
    offsets = {name: offset for name, _, offset in JOINTS}

    def write(name, depth):
        pad = '\t' * depth
        lines.append(f'{pad}{"ROOT" if depth == 0 else "JOINT"} {name}')
        lines.append(f'{pad}{{')
        lines.append(f'{pad}\tOFFSET ' + ' '.join(_number(100.0 * v) for v in offsets[name]))
        channels = ROOT_CHANNELS if depth == 0 else JOINT_CHANNELS
        lines.append(f'{pad}\tCHANNELS {len(channels.split())} {channels}')
        for child in children.get(name, []):
            write(child, depth + 1)
        if name in END_SITES:
            lines.append(f'{pad}\tEnd Site')
            lines.append(f'{pad}\t{{')
            lines.append(f'{pad}\t\tOFFSET ' + ' '.join(_number(100.0 * v) for v in END_SITES[name]))
            lines.append(f'{pad}\t}}')
        lines.append(f'{pad}}}')

    write('Hips', 0)
    lines.append('MOTION')
    lines.append(f'Frames: {FRAMES}')
    lines.append(f'Frame Time: {1.0 / FPS:.7f}')
    for frame in range(FRAMES):
        lines.append(' '.join(_number(value) for value in frame_values(frame)))
    return '\n'.join(lines) + '\n'


def check():
    """Whether every leg reaches and the planted feet hold still but for the slide: a list of problems."""
    problems = []
    for side in ('Left', 'Right'):
        planted_at = None
        for frame in range(FRAMES):
            ankle, planted = ankle_at(frame, side)
            try:
                solve_leg(hips_at(frame / FPS), ankle, side)
            except ValueError as error:
                problems.append(f'frame {frame}: {error}')
            if not planted:
                planted_at = None
                continue
            planted_at = planted_at or ankle
            moved = abs(ankle[2] - planted_at[2])
            injected = side == 'Left' and INJECTED[0] < frame <= 47
            if moved > 1e-9 and not injected:
                problems.append(f'frame {frame}: the {side.lower()} foot slides {moved:.4f} m off the injection')
    return problems


def main(argv):
    problems = check()
    for problem in problems:
        print(problem)
    if problems:
        return 1
    text = bvh()
    if '--write' in argv:
        os.makedirs(os.path.dirname(OUTPUT), exist_ok=True)
        with open(OUTPUT, 'w', encoding='utf-8', newline='\n') as f:
            f.write(text)
        print(f'wrote {os.path.relpath(OUTPUT, REPO)}: {FRAMES} frames, {len(text)} bytes')
    else:
        print(f'{FRAMES} frames at {FPS} a second; the left foot slides {SLIDE} m over frames {INJECTED}')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
