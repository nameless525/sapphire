# Playtesting the slice

This is the procedure for watching someone play the Lantern Ward, M3's slice, on a playtest build, and for turning what they did into answers that point at a file. It carries [m2-exit-run.md](m2-exit-run.md)'s three feel reviews from the arena's one fight to the ward's rooms and boss, and adds M4's settings. Its sheet is for the maintainer to fill in beside the player. The session the build keeps answers the counting questions, so the person watching can ask about feel.

| Step | Takes | Needs |
|---|---|---|
| Build and install | 4 minutes the first time | The device: a Mac, an iPhone (signing, see below), an Android phone, or the Simulator or emulator for a dry run |
| Play | 30 minutes | Someone who hasn't played it, and a pad or a phone |
| Bring it back and read it | 2 minutes | Nothing |
| Talk it through, with the sheet | 20 minutes | The report, and the person |

**Nobody has run this with a person.** Every command below has run on this Mac, the iOS Simulator and the Android emulator with scripted input ([m4-plan.md](m4-plan.md) F1 and F2). A phone's own install is owed: `--platform ios` has never reached a device.

## Before they play

```bash
cargo xtask playtest --platform desktop          # this Mac, in a window: a pad works here
cargo xtask playtest --platform android          # the connected phone, or the emulator
cargo xtask playtest --platform ios --identity <name> --profile <file> --device <name>   # an iPhone
```

That's the game built for release with its `playtest` feature, installed and started as `cargo xtask run` would. A store build is built without the feature, so it keeps nothing and its pause has no mark. Each time the app starts it begins a session: every run's recording, a row of frame times and heat each second, the settings as they change, the build and the device, and trips to the background. It writes them at least every 10 s, so a crash or a kill loses at most that.

Tell the player three things, and nothing about how to fight:

- Start a new game and play the ward until the boss on the terrace falls, or for 30 minutes.
- **Mark a moment** from the pause (Start, Esc or P, then "Mark this moment") whenever something feels wrong, unfair, slow, confusing or great. It costs them nothing, and you'll get its exact tick back.
- Say out loud what they're thinking.

Write down what they say beside the time on the clock. Don't help unless they're stuck for a minute, and write down where.

## After they play

```bash
cargo xtask playtest pull --platform desktop     # or android, ios-sim, ios --device <id>
cargo xtask playtest report target/playtest/desktop/sessions/<session>
```

`pull` copies every session on the device to `target/playtest/<platform>/sessions/`, writes each run's file and checksums from the journal the device kept it in (as of the session's last keep: a session killed mid-keep reads as of the keep before), and replays each run here to the checksums the device reached. A difference is a determinism bug, the most important thing a session can find: keep the session and say so. `report` replays the session's runs again and reads them out. It prints the report and writes `report.ron` beside the session. `sapphire_playtest`'s `report` module says how each number is counted.

The report gives, for each run, split by the ward's encounters (the market, the garden, the court and the terrace), and the time outside them:

| What it says | What it's counted from |
|---|---|
| The part's ticks and seconds, and how the encounter ended: cleared, started over (a return to a checkpoint, or the zone left) or going as the run ended | The simulation's encounter phases after each step |
| Damage taken, in how many hits, and falls | The hits' cues on the player's side, and its defeats |
| Damage dealt, in how many hits landed | The hits' cues the player's side's attacks made on anyone else, each with its attack's damage (M4 F3) |
| Dodges, perfect dodges and counters | Starts of moves with a perfect-dodge window, `perfect_dodge` cues, and moves a counter window started |
| Presses a second, and idle seconds | The run file's own input: buttons going down, and stretches of a second or more with nothing held and the stick at rest |
| Frame times and heat | `frames.csv`'s rows in the part: the mean and worst frame, frames late past one and a half intervals, the OS's hottest thermal state, the least headroom, and Low Power Mode |
| Each mark: its run, tick and the encounter it fell in | `session.ron`, with the command that draws that tick |

A run on one machine that doesn't reach the device's checksum says so on its line: its counts are then this machine's run, not the player's. A session from a device of another architecture (an x86-64 emulator, read here on arm64) isn't held to its checksums, since those are that architecture's.

**A mark's moment.** Each mark comes with a command like this one:

```bash
cargo xtask playtest --platform desktop -- --replay <session>/runs/2/ward-run-2.sreplay --at-tick 4182 --capture mark-1.png
```

It plays the run's file through the game, with no window, to that tick, and writes the frame. Without `--at-tick` and `--capture`, `--replay` plays the run in a window from its start, past the title.

## The sheet

Each question says what in the report helps answer it, and where the change lives. A field is a document's path and the field in it. "What changes what" in [m2-exit-run.md](m2-exit-run.md) says how each kind of file reaches a running fight (a save, a cook or a rebuild), and its tables name more fields than these. A change to a fight's moves or brains moves the replays' checksums: `cargo xtask replay --update`, as there.

### Moving and the camera (m2's review 1, in the ward)

Watch the Gate Square, the lanes between rooms, and the rooms' edges.

| Question | The report | If it's wrong |
|---|---|---|
| Does the character go where the thumb says, at once? Does stopping feel planted? | Idle seconds outside encounters, where someone stood still to look around | [player.moves.ron](../content/combat/player/player.moves.ron) `fighter.movement`: `run_speed`, `walk_speed`, `acceleration`, `deceleration`, `turn_rate_degrees` |
| Is the stick too touchy, or too dead, near its middle? | | [combat_bindings.ron](../crates/sapphire_controls/combat_bindings.ron): `DeadZone`, `Scale` |
| Does the camera keep both fighters in frame in the lanes and against the rooms' walls, without jerking or clipping? Does it turn by itself too much? | A mark's frame, from its command | [default_camera.ron](../crates/sapphire_camera/default_camera.ron): `combat.framing`, `collision`, `clearing` (`tolerance`, `better_by`, `settle`, `after_look`; `reach: 0` turns it off) |
| Do they get lost? Do they find each room's gate, and come back to the right place after a fall? | The time outside encounters, each part's `started over` | [lantern_ward.zone.ron](../content/zones/lantern_ward/lantern_ward.zone.ron): `checkpoints` (place, yaw, volume) and the encounters' trigger volumes |
| Does a jump go where they meant? | | `fighter.movement`: `jump_speed`, `gravity`, `air_acceleration`, `coyote_ticks` |

### Attacks (m2's review 2)

| Question | The report | If it's wrong |
|---|---|---|
| Does a press swing now? Does the string flow as fast as they press, and not faster? | Presses a second, against hits landed: many presses and few hits, the presses go nowhere | [player.moves.ron](../content/combat/player/player.moves.ron): `intents`' `buffer` and `priority`; each move's `cancels` (`from`, `to`, `allow`) |
| Can they read wind-up, strike and recovery on each move? | | Each move's `startup`, `active`, `recovery`, which add up to its clip; the pose is [tools/motion](../tools/motion/README.md)'s |
| Does a hit land everywhere at once: freeze, spark, shake, sound, rumble? | | The move's `attack`: `hitstop`, `knockback`, `cue`; [default_cues.ron](../crates/sapphire_cues/default_cues.ron)'s row for that cue and weight; [default_screen_effects.ron](../crates/sapphire_world/default_screen_effects.ron) |
| Is the motion good enough to judge the fight by? | | Not tuning: m2-plan §6's answer is an animator or a bought pack |

### The fights (m2's review 3, carried to the ward's four encounters)

The market and the garden are warriors and Lancers in two waves each, the court three warriors then both Lancers after its gate's cutscene, and the terrace the Knell Colossus.

| Question | The report | If it's wrong |
|---|---|---|
| Is it fun? Would they keep playing? What's missing first? | Every encounter's time and falls; where the marks are | Write it down in their words: it's the go/no-go |
| Does the dodge-and-counter rhythm land the way Zenless Zone Zero's does? | Dodges, perfect dodges and counters in each encounter. Many dodges and few perfect ones: the window, or the telegraph. Perfect dodges with no counter: they didn't know to press, or the window's too short | The dodge in [player.moves.ron](../content/combat/player/player.moves.ron): `invulnerable`, `perfect_dodge`'s `from`, `to` and `volume`, its `slow_motion` (`per_mille`, `steps`) and `counter` (`ticks`, `allow`) |
| Is each kind of enemy fair and readable? Does it read as a threat? | Damage taken and hits in each encounter, by who's in it; hits landed against it, for one they couldn't touch | Its brain in its `.enemy.ron`: [warrior.enemy.ron](../content/combat/enemy/warrior.enemy.ron), [lancer.enemy.ron](../content/combat/lancer/lancer.enemy.ron), [knell_colossus.enemy.ron](../content/combat/knell_colossus/knell_colossus.enemy.ron): `patience`, `rest`, `recover`, `spacing`, `pace`, and each attack's `weight`, `reach`, `against`, `recent`. What a swing gives away is its move's `startup` in its `.moves.ron`, and `armour` there says what it shrugs off |
| Are there too many at once, or too long a wait between waves? | Each encounter's seconds; idle time inside one | The room's `.encounter.ron` ([market](../content/zones/lantern_ward/market.encounter.ron), [garden](../content/zones/lantern_ward/garden.encounter.ron), [court](../content/zones/lantern_ward/court.encounter.ron), [terrace](../content/zones/lantern_ward/terrace.encounter.ron)): `tokens` and `token_rest` (how many may attack at once, and how soon again), each wave's `when` and `gap` |
| Does a fight take too long, or end too soon? Does a hit hurt as much as it looks? | Seconds, damage taken and dealt, falls. Damage dealt in an encounter against its enemies' health says how many of their health bars a fight took; hits landed over its seconds, how often they connected | Each attack's `damage` and `poise` in its `.moves.ron`; `fighter`'s `health`, `poise`, `poise_refill` |
| Does the boss read as a boss? Does its second wind feel earned? | The terrace's time, damage and falls | [knell_colossus.enemy.ron](../content/combat/knell_colossus/knell_colossus.enemy.ron): `phases` (`at: 500` is its second wind), the `tolling` brain, and its script, [knell_colossus.luau](../content/combat/knell_colossus/knell_colossus.luau) |
| On touch: do the cuts find the enemy they meant? | Hits landed against presses, by encounter, and the marks. It counts a hit on anyone, not the one they meant: ask | `PlayerSettings.auto_target` (the settings' row), and its weights in [default_targeting.ron](../crates/sapphire_gameplay/default_targeting.ron). The one setting a run reads, so the run records it |
| Did a change feel slow to make? (Only if you tune with them there) | | m2-exit-run's "What changes what": say which change took longest, and whether it was the engine or finding what to change |

### The story between fights

| Question | The report | If it's wrong |
|---|---|---|
| Does the court's gate cutscene blend in and out of play? Do they want to skip it? | A mark during it | [court_gate.timeline.ron](../content/zones/lantern_ward/court_gate.timeline.ron) |
| Do they read the guide's conversation, and understand the choice? | | [ward_guide.dialogue.ron](../content/dialogue/ward_guide.dialogue.ron), and its strings in `content/loc/<locale>/dialogue.ftl` |
| Do they know what to do next, and when they've won? | The time outside encounters | [game.flow.ron](../content/game.flow.ron): `beats` and `holds` |

### Settings (M4)

The session keeps the settings at the start and after each change, as `settings-<n>.ron` beside `session.ron`, whose `settings` list says when and at which tick each was saved. Ask what they changed and why, and read the files for what they chose. Each field is `PlayerSettings`'s ([settings.rs](../game/game_ui/src/menus/settings.rs)), its rows in [menus.ron](../content/ui/menus.ron) and [settings.ui.ron](../content/ui/settings.ui.ron).

| Question | The report | If it's wrong |
|---|---|---|
| Can they read every screen, and the HUD in a fight, at arm's length or on the phone? | Settings files: `size`, `text_size` | The defaults in `PlayerSettings::default`; the layout in [default_hud.ron](../crates/sapphire_ui/default_hud.ron) |
| Are the subtitles readable, and on when they'd want them? | `subtitles` (`shown`, `size`, `background`, `speakers`) | The same fields' defaults |
| Was the shake, a flash or the camera's punch too much? | `shake`, `flashes`, `camera_motion` | The defaults; a cue's `shake` in [default_cues.ron](../crates/sapphire_cues/default_cues.ron); a screen effect in [default_screen_effects.ron](../crates/sapphire_world/default_screen_effects.ron) |
| Was the music, the effects or a voice too loud? | `sound`, `music`, `effects`, `voice`, `mono` | The defaults, and a sound's `volume_db` in `content/audio/<name>.cue.ron` |
| Did holding a button tire them? | `holds` (hold or toggle per action), `rumble` | The defaults |
| Did they change the language, and did anything read wrong? | `language` | `content/loc/<locale>/` |
| Did it look rough, run hot or drain the battery, and did they change the graphics page for it? | `quality` (automatic or a held tier), `frame_cap`, `dynamic_resolution`, `battery_saver`; with Frames and heat below | The tiers' numbers and ladder in [tiers.ron](../crates/sapphire_render/tiers.ron); the defaults |

### Frames and heat

| Question | The report | If it's wrong |
|---|---|---|
| Did it stutter where they fought? Did the phone get hot? | Each encounter's worst frame, frames late, the hottest thermal state and the least headroom; Low Power Mode | The tier's numbers in [tiers.ron](../crates/sapphire_render/tiers.ron); a late frame inside an encounter is a fight's cost, outside one the ward's streaming. Mark the moment and capture it to see what was on screen |

## What to keep

- The session directory, with its `report.ron`, and your notes beside it, by time.
- A run worth keeping as a regression test is a `.sreplay` already: copy it into `tests/replays/` and run `cargo xtask replay --update`, as m2-exit-run's "A recorded session joins the replay suite" says.
- Every determinism failure, with the session.
