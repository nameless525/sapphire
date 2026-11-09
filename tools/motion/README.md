# The player's moves, posed by target

`content/combat/player/`'s key-pose documents are this tool's output. To re-pose a move, edit it here and write again: a hand edit to one lasts only until the next `--write`.

`player.moves.ron` holds the moves' numbers, and **they're tuned in the file**, in the timeline editor (`cargo xtask editor`) or by hand. This tool wrote its first draft and rewrites it only when asked with `--write-numbers`, which refuses to replace a file changed since it last wrote it. Nothing here can throw tuning away without `--force`.

An incomplete comment, literal or delimiter structure refuses a numbers write, including a forced one, because its movement cannot be safely recovered. Unchanged key-pose documents, metadata and the ownership stamp keep their modification times.

Its fighter's `movement` section, how the character moves, was never this tool's. `--write-numbers` carries over what the file holds, word for word, forced or not, writes today's numbers only into a file with none, and leaves the section out of the digest, so tuning it is no edit to refuse.

It's plain Python 3 with no dependencies, and it writes nothing unless told to.

```
python3 tools/motion/moves.py                  # checks every move, writes nothing
python3 tools/motion/moves.py light_3 -v       # one move's checks, tick by tick
python3 tools/motion/moves.py --write          # writes the key-pose documents, and only those
python3 tools/motion/moves.py --write-numbers  # writes player.moves.ron from frames.py, if nothing else has changed it
python3 tools/motion/moves.py --write-numbers --force  # writes it whatever's there
python3 tools/motion/sheet.py light_1 skill:0,4,8,12,16  # cooks, then draws contact sheets
```

## What's here

| File | What it does |
|---|---|
| `moves.py` | The seven moves, one function each: a key names where the feet stand, where the hand and blade go, how the body turns and where the head looks. `ultimate` (M3 Q3b) takes `light_3`'s keys for its bound, chop and recovery at later ticks, so a re-pose of the finisher reaches it. |
| `cutscene.py` | Key poses a timeline's character track plays, not a move (M3 Q3): `court_guard`, for the Lantern Ward's gate. `--write` writes them beside the timeline in `content/zones/lantern_ward/`; each needs a row in the character's graph. |
| `pose.py` | Solves a key's legs and arms by inverse kinematics, samples between keys as `sapphire_pipeline::keypose` does, checks a move (feet under the ground, slipping feet, the blade through the body, joint turns) and writes the `.keypose.ron`. |
| `frames.py` | `player.moves.ron` as `--write-numbers` writes it: frame data, cancel windows, hits, combat numbers and cues. `swap_in`, a party's assist, is frames on the dash attack's clip, with no key-pose document of its own. |
| `player.moves.written` | The digest of what `--write-numbers` last wrote, which tells it whether anything has changed the file since. |
| `test_writing.py` | Holds who writes what, `movement` included: `python3 -B -m unittest discover -s tools/motion -p 'test_*.py'`. |
| `rig.py` | Reads the character's rest skeleton from its VRM, so nothing derived from it is committed. |
| `sheet.py` | Writes the key-pose documents, cooks them, and runs the sandbox's `--contact-sheet` (`--blade on`, springs off) at chosen ticks and views. |
| `figure_pose.py` | The same posing for a generated figure (M3 C2, `tools/figures/`): its rest from its `.glb`, a key's feet in the clip's world and its weapon's slot, aim and roll in the model's, a free or holding left hand, and `--write`. A hand lets go of the weapon as a blend starts and takes it again as one ends. |
| `lancer.py`, `knell_colossus.py` | Each figure's clips as keys, named as its spec: `python3 tools/motion/lancer.py` checks, `--write` writes `content/combat/lancer/`'s documents. `test_figure_pose.py` holds what's committed to them. |

## Working on a move

1. Edit its function in `moves.py`. A move's length must equal its clip's, or the cook refuses it, so a clip that changes length takes the same change in `player.moves.ron`; `--write` says when `frames.py` no longer fits.
2. Read a sheet: `python3 tools/motion/sheet.py light_1:0,3,7,8,9,10,11,15,22 --views 205,90`. The default views are from behind and from the side, which is what a fight's camera sees and what shows a silhouette best. Sheets go to `target/motion/`. Generator warnings remain visible when the sheet tool runs. Use `--travel` to inspect world-space motion; default in-place sheets alone do not establish foot locking or centimeter-scale ground/blade clearance.
3. Run `cargo test -p sapphire_pipeline --test player_moves`, which holds each move to its shape. It says nothing about whether the move looks good.
4. Commit `moves.py` with what it wrote. On a clean checkout, `--write` and `--write-numbers` change nothing.

## Tuning a move's numbers

1. Change them in `player.moves.ron`, in the timeline editor or by hand. The editor's banner quotes what the file says about this tool.
2. The simulation keeps a cooked copy of the set for the arena's fight, so any change that reaches a cook owes it a rewrite: `SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy`, then `cargo xtask replay --update`. Forget, and that test fails saying exactly this.
3. Leave `frames.py` behind, or copy the numbers worth keeping into it. It's only needed to add a move or change the file's layout: edit it, and `--write-numbers` then shows what it would lose from the file. When the file is already what `frames.py` writes, it writes nothing and takes the file as its own again.

## Posing rules learned the hard way

- **Lean from `Spine`, `Chest` and `UpperChest`.** Keep `Hips` pitch to about 8°. Past about 12°, the shorts' seat pushes through the shirt's back as a black patch.
- **Key every tick through a strike.** Bones blend the short way, so a sweep over a few keys sags and sends the tip through the body. Those keys also let the hips lead, then the chest, then the arm.
- **Root turn:** a key names little more than a quarter turn, for the same reason.
- **A foot bearing weight stays where it is.** Let the body draw back over it, and let the other foot step.
- **`root` keeps y at 0.** A leap's height is the hips' and feet's in the clip, so every move stays a `strike`.

Directions: the character faces +Z and +X is its left. In `rh=(az, el, r)` and `blade=(az, el)`, azimuth runs from +Z toward +X and elevation up, in degrees.
