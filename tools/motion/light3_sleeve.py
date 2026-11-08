"""The three sleeve corrective curves written with the authored Light3 pose."""
import re

from pose import BONE_ORDER, slerp, to_euler, turn

TRACKS = (
    ("__sleeve_p8_left", 13, 16, 16, 19),
    ("__sleeve_p8_right_onset", 11, 12, 12, 13),
    ("__sleeve_p8_right_crest", 12, 13, 14, 16),
)
TRACK_KEYS = {11, 13, 16, 17, 19}
SAMPLED_KEYS = {12, 14, 15, 18}
KEY = re.compile(r"(?m)^ {12}\(tick: (\d+),.*?^ {12}\}\),", re.DOTALL)


def ramp(tick, start, end):
    value = max(0.0, min(1.0, (tick - start) / (end - start)))
    return value * value * (3.0 - 2.0 * value)


def weights(tick):
    fields = []
    for name, start, peak, release, end in TRACKS:
        value = ramp(tick, start, peak) * (1.0 - ramp(tick, release, end))
        fields.append(f'"{name}": {value:.9f}')
    return ", ".join(fields)


def numbers(values):
    return ", ".join(f"{value:.9f}" for value in values)


def sampled_key(move, tick):
    pose, root, root_turn = move.sample(tick)
    rows = [
        f"            (tick: {tick}, ease: Linear, root: ({numbers(root)}), "
        f"root_turn: {root_turn:.9f}, hips: ({numbers(pose.hips)}), pose: {{",
    ]
    for name in BONE_ORDER:
        x, y, z = to_euler(pose.turns[name])
        rows.append(f"                {name}: (x: {x:.9f}, y: {y:.9f}, z: {z:.9f}),")
    before = [key for key in move.keys if key["tick"] < tick]
    following = next(key for key in move.keys if key["tick"] > tick)
    fraction = (tick - before[-1]["tick"]) / (following["tick"] - before[-1]["tick"])
    # An inserted pose key also needs any finger turns changing across this linear segment.
    for name, end in following["fingers"].items():
        start = next((key["fingers"][name] for key in reversed(before) if name in key["fingers"]), (0, 0, 0))
        if start != end:
            x, y, z = to_euler(slerp(turn(*start), turn(*end), fraction))
            rows.append(f"                {name}: (x: {x:.9f}, y: {y:.9f}, z: {z:.9f}),")
    rows.append(f"            }}, morphs: {{{weights(tick)}}}),")
    return "\n".join(rows) + "\n\n"


def add_sleeve_keys(move, document):
    """Add sleeve weights and sample the intervening linear pose keys."""
    ticks = {key["tick"] for key in move.keys}
    if not TRACK_KEYS <= ticks or SAMPLED_KEYS & ticks:
        raise ValueError("Light3's sleeve key times must be reviewed with the new authored pose")
    for tick in SAMPLED_KEYS:
        following = next(key for key in move.keys if key["tick"] > tick)
        if following["ease"] != "Linear":
            raise ValueError("Light3's added sleeve keys require an existing linear pose segment")
    previous = 0

    def amend(match):
        nonlocal previous
        tick = int(match[1])
        added = "".join(sampled_key(move, at) for at in sorted(SAMPLED_KEYS) if previous < at < tick)
        previous = tick
        key = match[0]
        if tick in TRACK_KEYS:
            key = key[:-2] + f", morphs: {{{weights(tick)}}}),"
        return added + key

    return KEY.sub(amend, document)
