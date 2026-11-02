# The player's moves, posed by target

`content/combat/player/`'s key-pose documents and `player.moves.ron` are this tool's output. To re-pose a move, edit it here and write again. A hand edit to those files lasts only until the next `--write`.

It's plain Python 3 with no dependencies, and it writes nothing unless told to.

```
python3 tools/motion/moves.py                  # checks every move, writes nothing
python3 tools/motion/moves.py light_3 -v       # one move's checks, tick by tick
python3 tools/motion/moves.py --write          # writes the documents and player.moves.ron
python3 tools/motion/sheet.py light_1 skill:0,4,8,12,16  # cooks, then draws contact sheets
```

## What's here

| File | What it does |
|---|---|
| `moves.py` | The six moves, one function each: a key names where the feet stand, where the hand and blade go, how the body turns and where the head looks. |
| `pose.py` | Solves a key's legs and arms by inverse kinematics, samples between keys as `sapphire_pipeline::keypose` does, checks a move (feet under the ground, slipping feet, the blade through the body, joint turns) and writes the `.keypose.ron`. |
| `frames.py` | `player.moves.ron`: frame data, cancel windows, hits, combat numbers and cues. `swap_in`, a party's assist, is frames on the dash attack's clip, with no key-pose document of its own. |
| `rig.py` | Reads the character's rest skeleton from its VRM, so nothing derived from it is committed. |
| `sheet.py` | Writes the documents, cooks them, and runs the sandbox's `--contact-sheet` (`--blade on`, springs off) at chosen ticks and views. |

## Working on a move

1. Edit its function in `moves.py`, or its numbers in `frames.py`. A move's length must equal its clip's: `frames.py` refuses otherwise.
2. Read a sheet: `python3 tools/motion/sheet.py light_1:0,3,7,8,9,10,11,15,22 --views 205,90`. The default views are from behind and from the side, which is what a fight's camera sees and what shows a silhouette best. Sheets go to `target/motion/`.
3. Run `cargo test -p sapphire_pipeline --test player_moves`, which holds each move to its shape. It says nothing about whether the move looks good.
4. Commit `moves.py` or `frames.py` with what they wrote. A clean checkout's `moves.py --write` changes nothing.
5. The simulation keeps a cooked copy of the set for the arena's fight: `SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy`, then `cargo xtask replay --update`.

The timeline editor (`cargo xtask editor`) opens `player.moves.ron` as it does any moves file, and says this tool wrote it: what it changes there lasts until the next `--write`, so copy numbers worth keeping back into `frames.py`.

## Posing rules learned the hard way

- **Lean from `Spine`, `Chest` and `UpperChest`.** Keep `Hips` pitch to about 8°. Past about 12°, the shorts' seat pushes through the shirt's back as a black patch.
- **Key every tick through a strike.** Bones blend the short way, so a sweep over a few keys sags and sends the tip through the body. Those keys also let the hips lead, then the chest, then the arm.
- **Root turn:** a key names little more than a quarter turn, for the same reason.
- **A foot bearing weight stays where it is.** Let the body draw back over it, and let the other foot step.
- **`root` keeps y at 0.** A leap's height is the hips' and feet's in the clip, so every move stays a `strike`.

Directions: the character faces +Z and +X is its left. In `rh=(az, el, r)` and `blade=(az, el)`, azimuth runs from +Z toward +X and elevation up, in degrees.
