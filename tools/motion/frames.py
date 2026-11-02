"""`player.moves.ron`: the moves' frame data, windows and combat numbers, written out beside their clips."""
import os

from pose import *

SIDECAR_ID = '6fc61cbc-8189-4b6d-a4a0-b0e09475c6ce'
IDS = {
    'light_1': '6d321a52-1aed-4279-bc32-a177b8fd81d4',
    'light_2': '0c4be2d4-4149-4852-80c5-109683aad3e1',
    'light_3': '5fe300c9-a4a3-4339-a556-7bc112f2bb59',
    'dash_attack': '997ddfa0-e2bf-4887-94dd-e8f51cb6e958',
    'dodge': '9b5f353a-2c97-45c7-a70f-bcab216de811',
    'skill': '9491f5d1-0c11-45c9-8139-8f54918b25f6',
}


def blade_socket():
    rest = tuple(RIG['rightHand']['rot'])
    middle = add(GRIP_R, mul(BLADE_R, BLADE_LENGTH * 0.5))
    offset = qrot(qinv(rest), middle)
    q0 = from_basis(cross(BLADE_R, EDGE_R), BLADE_R, EDGE_R)
    rotation = qnorm(qmul(qinv(rest), q0))
    if rotation[3] < 0:
        rotation = tuple(-c for c in rotation)
    return offset, rotation


_offset, _rotation = blade_socket()

HEADER = """// The player's moves (M2 O3): frame data in 60 Hz ticks, for a character with a blade in its right
// hand. Zenless Zone Zero's rhythm is the target: a press moves the character within three ticks, a cut
// lands inside an eighth of a second, a string flows as fast as the player presses, and a dodge gets out
// of nearly anything. These are tuning values, made to be changed by the feel reviews.
//
// Each move plays the clip of its name beside this file (`light_1.keypose.ron`), and a cook bakes its
// root motion and its blade from that clip. One tick of a move is one tick of its clip, so a change to a
// move's length is a change to its clip's too. Windows move freely inside it.
//
// `blade` is a socket on the right hand: the middle of a 0.9 m blade leaving the fist on the thumb side,
// 22 degrees toward the fingers, its +Y running grip to tip. A hit is a capsule along it, and the sandbox
// draws `sword.glb` there. Written by tools/motion/moves.py, so edit it there.
//
// E3 hangs effects on the cues: `swing` is the blade starting to cut, and `step`, `skid` and
// `finisher_land` a foot taking the weight. The combat numbers are first guesses for Feel review 2, on
// the scale `warrior.moves.ron` was tuned to: the three-hit string does 28 damage and 36 poise.
//
// The five moves after those are what a hit forces (M2 G6). They have no clip of their own: the
// character's graph draws them from its reactions and deaths, and they stand still while it does.
//
// The last is a party's swap-in assist (M2 G7), on the dash attack's clip, and `swap` is the press that
// brings it. Only a party reads either: with no party, the button does nothing.
(
    type: "sapphire::moves::MoveSetDocument",
    version: 1,
    data: (
        intents: [
            // A dodge beats a skill beats an attack when two could fire at one instant. A press of
            // `light` lasts long enough to carry from anywhere in a cut into the next.
            (name: "light", button: Light, buffer: 14, priority: 1),
            (name: "skill", button: Skill, buffer: 10, priority: 2),
            (name: "dodge", button: Dodge, buffer: 8, priority: 3),
            (name: "swap", button: Swap, buffer: 12, priority: 4),
        ],
        neutral: [
            (intent: "light", move: "light_1"),
            (intent: "skill", move: "skill"),
            (intent: "dodge", move: "dodge"),
        ],
        fighter: (
            health: 300,
            poise: 60,
            // Four seconds without a hit that takes poise, and the meter is full again.
            poise_refill: 240,
            hurtboxes: [
                // From the ground to the crown of the head at 1.62 m. A dodge turns it off.
                (name: "body", offset: (0.0, 0.81, 0.0), shape: Capsule(radius: 0.3, length: 1.02)),
            ],
            reactions: (
                light: "flinch",
                medium: "stagger",
                heavy: "knockdown",
                poise_break: "dazed",
                defeat: "defeat",
            ),
        ),
        moves: [
"""

REACTIONS = """            (
                // 300 ms: a light hit's. A dodge gets out of its end.
                name: "flinch",
                startup: 0,
                active: 6,
                recovery: 12,
                cancels: [(from: 10, to: 18, allow: [(intent: "dodge", move: "dodge")])],
                cues: [(name: "reaction", at: 0)],
            ),
            (
                // 500 ms: a medium hit's.
                name: "stagger",
                startup: 0,
                active: 10,
                recovery: 20,
                cancels: [(from: 22, to: 30, allow: [(intent: "dodge", move: "dodge")])],
                cues: [(name: "reaction", at: 0)],
            ),
            (
                // 1,167 ms: a heavy hit's, down and up again, untouchable throughout.
                name: "knockdown",
                startup: 0,
                active: 30,
                recovery: 40,
                invulnerable: [(from: 0, to: 70)],
                cancels: [(from: 60, to: 70, allow: [(intent: "dodge", move: "dodge")])],
                cues: [(name: "knocked_down", at: 0)],
            ),
            (
                // 1,500 ms: a broken poise's, open to punishment: hits land and it stays dazed.
                name: "dazed",
                startup: 0,
                active: 60,
                recovery: 30,
                armour: [(from: 0, to: 90, threshold: 1000)],
                cues: [(name: "reaction", at: 0)],
            ),
            (
                // 2 s, with no way out: the fight's over for this one.
                name: "defeat",
                startup: 0,
                active: 30,
                recovery: 90,
                cues: [(name: "falls", at: 0)],
            ),
"""

FOOTER = """        ],
    ),
)
"""

MOVES = [
    dict(
        name='light_1',
        say=["567 ms. The first cut: 117 ms of wind-up, then a slash from high on the right to low",
             "on the left on a step of the right foot, carrying about 0.7 m. The next press takes over",
             "from tick 14, the blade just past its mark; left alone, the back foot steps through."],
        frames=(7, 4, 23),
        hitboxes=[('light_1_hit', 7, 11, 'Capsule(radius: 0.22, length: 0.9)',
                   'damage: 6, poise: 8, weight: Light, hitstop: 3, knockback: (distance: 0.25, ticks: 8)')],
        cancels=[(2, 34, [('dodge', 'dodge', None)]),
                 (14, 28, [('light', 'light_2', None), ('skill', 'skill', None)])],
        warps=[(0, 7, 0.6, 0.8, 1.0)],
        cues=[('swing_windup', 0), ('swing', 7), ('step', 11)],
        active=(7, 11),
    ),
    dict(
        name='light_2',
        say=["600 ms. The second cut, a backhand rising from low on the left to high on the right on",
             "a step of the left foot, carrying about 0.6 m. It starts where the first settles, and",
             "ends where the finisher starts."],
        frames=(7, 4, 25),
        hitboxes=[('light_2_hit', 7, 11, 'Capsule(radius: 0.22, length: 0.9)',
                   'damage: 8, poise: 10, weight: Light, hitstop: 4, knockback: (distance: 0.3, ticks: 8)')],
        cancels=[(2, 36, [('dodge', 'dodge', None)]),
                 (14, 30, [('light', 'light_3', None), ('skill', 'skill', None)])],
        warps=[(0, 7, 0.6, 0.8, 1.0)],
        cues=[('swing', 7), ('step', 11)],
        active=(7, 11),
    ),
    dict(
        name='light_3',
        say=["867 ms. The finisher commits: 267 ms gathering low and rising with the blade overhead,",
             "then a two-handed chop on a long bound of 1.4 m. It holds the landing, with only a dodge",
             "to get out early."],
        frames=(16, 4, 32),
        hitboxes=[('light_3_hit', 16, 20, 'Capsule(radius: 0.28, length: 0.9)',
                   'damage: 14, poise: 18, weight: Medium, hitstop: 7, finish_freeze: 16, '
                   'knockback: (distance: 0.7, ticks: 12), cue: "hit_heavy"')],
        cancels=[(4, 52, [('dodge', 'dodge', None)]),
                 (44, 52, [('light', 'light_1', None)])],
        warps=[(0, 16, 1.0, 1.0, 1.1)],
        cues=[('swing_windup', 0), ('swing', 16), ('finisher_land', 19)],
        active=(16, 20),
    ),
    dict(
        name='dodge',
        say=["400 ms: a low leap straight back, 2.4 m in 14 ticks. Invulnerable for 217 ms, the first",
             "100 ms of them perfect: a swing reaching a body-sized volume then slows the world, and a",
             "press answers with the dash attack. An attack out of its end is the dash attack too."],
        frames=(1, 13, 10),
        hurtboxes=[('body', 1, 14, 'Off')],
        invulnerable=[(1, 14)],
        perfect_dodge=[(1, 7)],
        cancels=[(9, 24, [('light', 'dash_attack', None), ('skill', 'skill', None)]),
                 (16, 24, [('dodge', 'dodge', 'Instant')])],
        cues=[('dodge_start', 0), ('skid', 10)],
        active=None,
    ),
    dict(
        name='dash_attack',
        say=["567 ms: out of a dodge's end, thrown forward low with the blade trailing, cutting across",
             "on the way past. It covers about 2.6 m and homes in on what it's aimed at."],
        frames=(5, 5, 24),
        hitboxes=[('dash_hit', 5, 10, 'Capsule(radius: 0.26, length: 0.9)',
                   'damage: 10, poise: 12, weight: Medium, hitstop: 5, knockback: (distance: 0.5, ticks: 10)')],
        cancels=[(2, 34, [('dodge', 'dodge', None)]),
                 (18, 34, [('light', 'light_1', None)])],
        warps=[(0, 10, 2.0, 1.2, 1.1)],
        cues=[('swing', 5), ('skid', 10)],
        active=(5, 10),
    ),
    dict(
        name='skill',
        say=["867 ms: a burst with two hits, safe to start and open to punishment after. A coiled",
             "drop that takes a hit for free, a whole turn with the arm out, and a rising cut coming",
             "out of the turn, ended by a flick of the blade."],
        frames=(12, 14, 26),
        hitboxes=[('skill_hit_1', 12, 16, 'Capsule(radius: 0.24, length: 0.9)',
                   'damage: 8, poise: 10, weight: Light, hitstop: 4, knockback: (distance: 0.2, ticks: 6)'),
                  ('skill_hit_2', 21, 26, 'Capsule(radius: 0.28, length: 0.9)',
                   'damage: 16, poise: 24, weight: Medium, hitstop: 8, knockback: (distance: 0.6, ticks: 10)')],
        invulnerable=[(0, 12)],
        parry=[(2, 6)],
        cancels=[(38, 52, [('dodge', 'dodge', None), ('light', 'light_1', None)])],
        warps=[(0, 12, 1.0, 1.2, 1.0)],
        cues=[('skill_flash', 0), ('skill_hit', 12), ('skill_hit', 21), ('step', 31)],
        active=(12, 26),
    ),
]

# What comes after the reactions, so that adding one moves no other move's place in the set.
ASSISTS = [
    dict(
        name='swap_in',
        clip='dash_attack',
        say=["567 ms: a party's next member arriving, on the dash attack's clip: a throw forward low that",
             "cuts across on the way past, untouchable until it's about to land. Its aim is whoever left's."],
        frames=(5, 5, 24),
        hitboxes=[('swap_hit', 5, 10, 'Capsule(radius: 0.26, length: 0.9)',
                   'damage: 12, poise: 14, weight: Medium, hitstop: 5, knockback: (distance: 0.5, ticks: 10)')],
        invulnerable=[(0, 5)],
        cancels=[(18, 34, [('dodge', 'dodge', None), ('light', 'light_1', None)])],
        warps=[(0, 10, 2.0, 1.2, 1.1)],
        cues=[('swap_in', 0), ('swing', 5), ('skid', 10)],
        active=(5, 10),
    ),
]


def fnum(v, places=3):
    s = f"{v:.{places}f}"
    s = s.rstrip('0')
    if s.endswith('.'):
        s += '0'
    if s == '-0.0':
        s = '0.0'
    return s


def vec(values, places=3):
    return '(' + ', '.join(fnum(v, places) for v in values) + ')'


def spans(out, name, items, indent):
    if not items:
        return
    out.append(f"{indent}{name}: [" + ', '.join(f"(from: {a}, to: {b})" for a, b in items) + "],")


def allow_text(allow):
    intent, target, when = allow
    extra = f', when: {when}' if when else ''
    return f'(intent: "{intent}", move: "{target}"{extra})'


def write(folder, clips):
    text = [HEADER.rstrip('\n')]
    for d in MOVES:
        text.append(move_text(d, clips))
    text.append(REACTIONS.rstrip('\n'))
    for d in ASSISTS:
        text.append(move_text(d, clips))
    text.append(FOOTER)
    with open(os.path.join(folder, 'player.moves.ron'), 'w') as f:
        f.write('\n'.join(text))
    with open(os.path.join(folder, 'player.moves.ron.meta'), 'w') as f:
        f.write(f'''(
    type: "sapphire::assets::AssetMeta",
    version: 1,
    data: (
        id: "{SIDECAR_ID}",
        importer: MoveSet(()),
    ),
)
''')


def move_text(d, clips):
    played = d.get('clip', d['name'])
    clip = clips[played]
    s, a, r = d['frames']
    assert s + a + r == clip.ticks, (d['name'], s + a + r, clip.ticks)
    i = ' ' * 16
    lines = ['            (']
    for line in d['say']:
        lines.append(f"{i}// {line}")
    lines += [f'{i}name: "{d["name"]}",', f"{i}startup: {s},", f"{i}active: {a},", f"{i}recovery: {r},"]
    lines.append(f"{i}motion: (")
    lines.append(f"{i}    clip: (")
    lines.append(f'{i}        source: "{IDS[played]}",')
    if d['active']:
        lines.append(f"{i}        paths: [(")
        lines.append(f'{i}            name: "blade",')
        lines.append(f'{i}            follows: Bone("RightHand"),')
        lines.append(f"{i}            offset: {vec(_offset)},")
        lines.append(f"{i}            rotation: {vec(_rotation, 4)},")
        lines.append(f"{i}        )],")
    lines.append(f"{i}    ),")
    lines.append(f"{i}),")
    if d.get('hitboxes'):
        lines.append(f"{i}hitboxes: [")
        for name, a0, b0, shape, attack in d['hitboxes']:
            lines.append(f'{i}    (')
            lines.append(f'{i}        name: "{name}",')
            lines.append(f'{i}        from: {a0},')
            lines.append(f'{i}        to: {b0},')
            lines.append(f'{i}        path: "blade",')
            lines.append(f'{i}        shape: {shape},')
            lines.append(f'{i}        attack: ({attack}),')
            lines.append(f'{i}    ),')
        lines.append(f"{i}],")
    if d.get('hurtboxes'):
        lines.append(f"{i}hurtboxes: [")
        for name, a0, b0, state in d['hurtboxes']:
            lines.append(f'{i}    (name: "{name}", from: {a0}, to: {b0}, state: {state}),')
        lines.append(f"{i}],")
    spans(lines, 'invulnerable', d.get('invulnerable'), i)
    for a0, b0 in d.get('perfect_dodge', ()):
        lines.append(f"{i}perfect_dodge: [(")
        lines.append(f"{i}    from: {a0},")
        lines.append(f"{i}    to: {b0},")
        lines.append(f"{i}    volume: (offset: (0.0, 0.9, 0.0), shape: Capsule(radius: 0.55, length: 1.0)),")
        lines.append(f"{i}    slow_motion: (per_mille: 200, steps: 30),")
        lines.append(f'{i}    counter: (ticks: 45, allow: [(intent: "light", move: "dash_attack")]),')
        lines.append(f"{i})],")
    spans(lines, 'parry', d.get('parry'), i)
    lines.append(f"{i}cancels: [")
    for a0, b0, allows in d['cancels']:
        if len(allows) == 1:
            lines.append(f"{i}    (from: {a0}, to: {b0}, allow: [{allow_text(allows[0])}]),")
        else:
            lines.append(f"{i}    (from: {a0}, to: {b0}, allow: [")
            for allow in allows:
                lines.append(f"{i}        {allow_text(allow)},")
            lines.append(f"{i}    ]),")
    lines.append(f"{i}],")
    if d.get('warps'):
        lines.append(f"{i}warps: [")
        for a0, b0, dist, turn_, keep in d['warps']:
            lines.append(f"{i}    (from: {a0}, to: {b0}, max_distance: {dist}, max_turn: {turn_}, keep: {keep}),")
        lines.append(f"{i}],")
    lines.append(f"{i}cues: [")
    for name, at in d['cues']:
        lines.append(f'{i}    (name: "{name}", at: {at}),')
    lines.append(f"{i}],")
    lines.append('            ),')
    return '\n'.join(lines)
