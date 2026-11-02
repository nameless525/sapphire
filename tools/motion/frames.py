"""`player.moves.ron`'s numbers as this tool first wrote them: frame data, windows and combat numbers.

The file is theirs to change from there, in the timeline editor or by hand, so it's written only when asked,
and an edited file is replaced only when forced. See the README.

Its fighter's `movement` section isn't this tool's at all: it writes today's numbers into a file that has
none, and otherwise carries over what's there, word for word, forced or not. Tuning how a character moves
is no edit of what this owns.
"""
import difflib
import hashlib
import os
import sys

from pose import *

NUMBERS = 'player.moves.ron'
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
// draws `sword.glb` there.
//
// Written by tools/motion/moves.py --write-numbers, which won't replace edits made here unless forced.
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
//
// `movement` is how the character moves. That tool never replaces it: it's tuned here, forced or not.
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
@MOVEMENT@        ),
        moves: [
"""

MOVEMENT = """            // How this character moves. Save, and the next step moves by these, with no restart and no
            // cook: a character mid-jump finishes that jump's arc under the new gravity, and its new jump
            // speed waits for its next jump. Metres, seconds and degrees. A number left out is the
            // controller's default, and sapphire_moves' `MOVEMENT_LIMITS` says what each may be.
            movement: (
                // Full speed from `run_stick` of the stick's tilt on, and `walk_speed` at `walk_stick`.
                run_speed: 6.0,
                walk_speed: 2.0,
                run_stick: 0.9,
                walk_stick: 0.5,
                // m/s². At these a press is at full speed from tick 4, and a stop takes 6 ticks.
                acceleration: 90.0,
                deceleration: 60.0,
                // How fast facing follows the stick. Motion doesn't wait for it.
                turn_rate_degrees: 1080.0,
                // A jump reaches jump_speed² / (2 × gravity), here 1.11 m, and may reach 2.5 m at most.
                jump_speed: 7.0,
                gravity: 22.0,
                air_acceleration: 18.0,
                max_fall_speed: 30.0,
                // Ticks after walking off a ledge in which a jump still works.
                coyote_ticks: 5,
                // What the feet take: slopes, steps, and how far they follow ground that drops away.
                slope_limit_degrees: 50.0,
                step_height: 0.3,
                ground_snap: 0.3,
                stance_radius: 0.15,
                max_slide_speed: 8.0,
                // The capsule, around feet that stay where they stand when these change.
                height: 1.6,
                radius: 0.3,
                skin: 0.01,
            ),
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


def numbers_text(clips, movement=MOVEMENT):
    text = [HEADER.replace('@MOVEMENT@', movement).rstrip('\n')]
    for d in MOVES:
        text.append(move_text(d, clips))
    text.append(REACTIONS.rstrip('\n'))
    for d in ASSISTS:
        text.append(move_text(d, clips))
    text.append(FOOTER)
    return '\n'.join(text)


def _read(path):
    try:
        with open(path, newline='') as f:
            return f.read()
    except FileNotFoundError:
        return None


def _code(text):
    """Each character of `text` outside its comments and strings, with where it is."""
    at = 0
    while at < len(text):
        if text.startswith('//', at):
            at = text.find('\n', at)
            if at < 0:
                return
        elif text[at] == '"':
            at += 1
            while at < len(text) and text[at] != '"':
                at += 2 if text[at] == '\\' else 1
            at += 1
        else:
            yield at, text[at]
            at += 1


def _movement_span(text):
    """Where the fighter's `movement` section sits, from the comments right above it through the end of
    its line, whatever its layout: `(start, end)` by character, or None for a file with no such section.
    It's the only field of that name three brackets in: the file's, `data`'s and `fighter`'s."""
    depth = 0
    opened = None
    closed = None
    for at, c in _code(text):
        if c in '([{':
            depth += 1
        elif c in ')]}':
            depth -= 1
            if opened is not None and depth == 3:
                closed = at + 1
                break
        elif opened is None and depth == 3 and text.startswith('movement', at) and not text[at - 1].isalnum() \
                and text[at + len('movement'):].lstrip().startswith(':'):
            opened = at
    if closed is None:
        return None
    rest = len(text) - len(text[closed:].lstrip(' \t'))
    if text[rest:rest + 1] == ',':
        closed = rest + 1
    line_end = text.find('\n', closed)
    end = len(text) if line_end < 0 else line_end + 1
    start = text.rfind('\n', 0, opened) + 1
    if text[start:opened].strip():
        return opened, closed
    while start > 0:
        above = text.rfind('\n', 0, start - 1) + 1
        if not text[above:start].lstrip().startswith('//'):
            break
        start = above
    return start, end


def _without_movement(text):
    span = _movement_span(text)
    return text if span is None else text[:span[0]] + text[span[1]:]


def _movement_of(held):
    """What a file holds as its `movement` section, ready to write back; today's numbers if it has none."""
    span = None if held is None else _movement_span(held)
    if span is None:
        return MOVEMENT
    section = held[span[0]:span[1]]
    return section if section.endswith('\n') else section + '\n'


def _digest(text):
    return hashlib.sha256(_without_movement(text).encode()).hexdigest()


def _last_written(stamp):
    held = _read(stamp)
    return held.split()[-1] if held else None


def _changes(held, text):
    lines = difflib.unified_diff(held.splitlines(), text.splitlines(), 'the file', 'this tool', lineterm='', n=0)
    shown = list(lines)
    return '\n'.join(shown[:40] + ['...'] if len(shown) > 40 else shown)


def numbers_note(folder, clips):
    """What `--write` says of the file it leaves alone, once that isn't what `--write-numbers` would write."""
    held = _read(os.path.join(folder, NUMBERS))
    try:
        text = numbers_text(clips, _movement_of(held))
    except AssertionError as error:
        name, numbers, ticks = error.args[0]
        return (f'frames.py has {name} at {numbers} ticks and its clip at {ticks}. If {NUMBERS} does too, the cook '
                'will refuse it: a move plays its clip tick for tick.')
    if held == text:
        return None
    return f'{NUMBERS} is not what frames.py would write. --write left it alone; --write-numbers writes it.'


def write_numbers(folder, clips, stamp, force=False):
    """Writes `player.moves.ron`, unless something else has changed it since this last did: `stamp` holds
    the digest of what this last wrote, so tuning can't be thrown away by accident. Returns whether it
    changed the file. The digest leaves out the `movement` section, which stays whatever this does."""
    path = os.path.join(folder, NUMBERS)
    held = _read(path)
    text = numbers_text(clips, _movement_of(held))
    edited = held is not None and held != text and _digest(held) != _last_written(stamp)
    if edited and not force:
        sys.exit(f'{path} has changed since this tool last wrote it, so nothing was written.\n'
                 f'Copy what you want to keep into frames.py, or pass --force to replace the file. Its differences:\n'
                 f'{_changes(held, text)}')
    if held != text:
        with open(path, 'w', newline='') as f:
            f.write(text)
    with open(stamp, 'w', newline='') as f:
        f.write(f'# What tools/motion/moves.py --write-numbers last wrote to {NUMBERS}. A file that no longer '
                f'matches has been edited since, and it\'s not replaced unless forced.\n{_digest(text)}\n')
    with open(os.path.join(folder, NUMBERS + '.meta'), 'w', newline='') as f:
        f.write(f'''(
    type: "sapphire::assets::AssetMeta",
    version: 1,
    data: (
        id: "{SIDECAR_ID}",
        importer: MoveSet(()),
    ),
)
''')
    return held != text


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
