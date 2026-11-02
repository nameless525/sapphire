# The M2 exit run

M2 ends when its four exit tests are answered ([m2-plan.md](m2-plan.md) §1): one character against one enemy feels right on a gamepad and on touch, a saved move plays on the next attempt with no restart, replays are deterministic, and Mobile-Low holds 30 fps thermally throttled. This file is the procedure for all four, for someone coming back to this cold. Agents have run everything that needs no window and no phone, and "Rehearsal" at the end says what ran. **Nobody has played this fight.**

| Exit test | Who answers it | Needs | Takes | Where it stands |
|---|---|---|---|---|
| 1. Feel, gamepad | The maintainer | A gamepad | 30 minutes for review 3, 90 for all three | Not judged. Its window has never opened on a desktop |
| 1. Feel, touch | The maintainer | A phone | 20 minutes | Not judged. Played by synthetic touches on an emulator |
| 2. Iteration | Anyone | A phone for its second half | 10 minutes | Holds on the desktop, an emulator and a simulator. A phone is owed |
| 3. Determinism | CI | Nothing | 1 minute by hand | Holds on every pull request |
| 4. Budget | Anyone | A phone | 25 minutes, unattended | Mac numbers only ([engine-spec.md](engine-spec.md) §3.5) |

**The shortest path:** connect a gamepad, run review 3 below (30 minutes), and read [m2-go-no-go.md](m2-go-no-go.md). Review 3 is the plan's real go/no-go check, and everything else can wait for it.

## Exit test 1: a fight on a gamepad

```bash
mkdir -p target
cargo xtask run --platform desktop --incremental --features dev -- --arena --fight 2>&1 | tee target/review.log
```

That cooks, builds, opens a window and starts a fight. In a window `--fight` turns on the cues, the live HUD, the sword and combat's bindings. It starts over 4 s after someone falls. On launch the pointer is captured, the cursor hides and the mouse turns the camera: "The pointer and focus" below says how to get it back. A first build takes about 4 minutes. `dev` is what brings hot reload, live link and the overlay, and what an edit below needs. `--incremental` is an optimized build with no LTO, so an edit to code rebuilds in seconds, not minutes, for about 0.3 ms more CPU in a frame on the same fight ("What changes what"). `--release` is what the numbers elsewhere were taken on, and plays the same fight to the same checksum.

If the pad isn't seen (`cargo xtask run` starts a bare binary), try `--platform macos`, which starts an app bundle and points it at the same cook. **Not run:** an agent may not open a window, so neither command has run as written, and the desktop window's loop has never run at all. I3 ran the same loop on an emulator and a simulator, and found a panic there that a debug window would have hit, fixed since. If it misbehaves, that's the first finding: keep the log.

### Controls

From [combat_bindings.ron](../crates/sapphire_controls/combat_bindings.ron), by a button's position. The layout is Zenless Zone Zero's, written from memory, so say if it's wrong.

| Verb | What it does here | Pad | Keyboard and mouse |
|---|---|---|---|
| Move | 2 m/s at half tilt, 6 m/s from 0.9 of a tilt | Left stick | W A S D |
| Look | Turn the view. It drifts back behind you | Right stick | Mouse, arrow keys |
| Zoom | The camera's distance | None | Wheel |
| Light | The three-hit string, and the dash attack out of a dodge | West (X, Square) | J, left button |
| Dodge | A leap back. The first 100 ms are the perfect dodge | South (A, Cross) | Left Shift |
| Skill | Two hits, safe to start | North (Y, Triangle) | E |
| Lock-on | Hold the line to the enemy | Right stick press | L, middle button |
| Jump | The controller's jump | Left shoulder | Space |
| Swap | With `--party` only. The HUD's health trail starts over for whoever comes in | Right shoulder | Tab |
| Heavy | Bound, with no move on it yet | East (B, Circle) | K, right button |
| Ultimate | Bound, with no move on it yet | Right trigger | Q |
| Start over | A new fight now | D-pad down | R |
| Pause | Stops the steps, shows a veil and a two-bar mark, and frees the cursor | Start | Esc, P |
| Resume | From the pause. It takes the pointer again | Start, East | Esc, P |
| Leave | Opens the pause, and a second press in the pause leaves | Back | Backspace |

In `dev` builds the `` ` `` key toggles the overlay, whose combat panel has the intent buffer, the move's windows and a timeline. `--debug-overlay=combat` starts with it on.

### The pointer and focus

A fight someone's playing in a desktop's window holds the pointer, so the mouse goes on turning the camera past the screen's edges and a click can't land outside.

- **Getting the pointer back:** Esc or P pauses, shows a veil and a two-bar mark, and frees the cursor. Esc or P resumes and captures it again. Backspace opens the pause too, and Backspace in the pause leaves. Cmd-Q quits at any time, and the window's close button works once paused.
- **Going elsewhere:** Cmd-Tab or clicking away pauses a played fight. Coming back with a click does nothing: no swing, no resume, and the cursor stays free until Esc or P. A button held through Esc doesn't swing as the fight resumes, and the next press does.
- **`--script`** neither captures the pointer nor pauses on focus loss, so a scripted fight goes on behind other windows and its checksum holds. The same goes for a replay and a recording a script drives.
- **The overlay:** the pointer is free while it's up.
- **Launched without focus:** a fight runs with no capture and no pause until it's focused.

**Check these by hand, because only a real window can show them.** Tests hold what decides all of this with no window, and what SDL 3.4.18 does with it comes from reading its source:

- How the mouse feels at the look scale of −0.008 radians a point ([combat_bindings.ron](../crates/sapphire_controls/combat_bindings.ron)).
- No jump on the first motion after capture or resume.
- The cursor's warp to the centre, and its return on release.
- `FocusLost` and `FocusGained` around Cmd-Tab, minimize and clicking away. The log's `pause paused=true` says a pause began.
- A MacBook trackpad's fingers staying out of the HUD's zones. A desktop shows the on-screen stick and buttons only once a finger has landed, so if they appear when a finger rests on the pad, desktops need to ignore touches.
- How the veil looks. `--hud live --hud-paused` with `--capture` draws it with no window.
- When those machines exist: Windows raw input, and X11 and Wayland relative mode.

### What to write down

- The pad's model, and the `quality tier` line from the log.
- For each review below, each question's answer in your own words. "The hit feels late" is enough. Numbers aren't needed: the table under "What changes what" turns words into fields.
- Anything you wanted to change and couldn't, and how long each change you did make took. That's what the off-ramp asks about.
- `target/review.log`. Each hit, each fight's result and any error is in it, and `frame stats` every 2 s.
- Anything wrong on the list under "The pointer and focus".

## The feel reviews (Z2)

M2's phases are all built, so **the three reviews can run in one sitting**, in this order, in about 90 minutes. The plan spaced them out so a bad answer could stop the work behind it. That's gone, because the work is done, so each still comes before the next only because its answer changes how to read the next. If there's time for only one, run review 3.

Each uses the command above. Save edits as you go: `git diff` is the session's record, and `git checkout <file>` undoes one. A saved document is cooked by the next `cargo xtask run`, so it carries over a relaunch.

To fight something that hardly fights back, for reviews 1 and 2: in [warrior.moves.ron](../content/combat/enemy/warrior.moves.ron), set `patience: (min: 3600, max: 3600)` and `health: 5000`, and save. It stands off, takes hits and swings about once a minute (3,600 ticks is the most a document allows). Undo it with `git checkout` before review 3.

### Review 1: moving and the camera (30 minutes)

The plan's question: **run around the square on a gamepad. Do moving and the camera feel good?**

Try: run the square's whole floor, stop dead, reverse, run into the pavilion's posts and along a wall, walk by half tilt, jump. Circle the enemy with your thumb off the right stick. Lock on and circle again. Run behind the tree and the stalls so the view has to find you.

Pillars: 1 (responsive), 5 (camera), 6 (alive: hair and the look in motion).

- Does the character go where the thumb says, at once? Does stopping feel planted?
- Do the walk and the run look like someone walking and running? (O2 calls the walk plain and the run over-split in the legs.)
- Does the camera keep both fighters in frame without jerking or clipping? (Nobody has judged: the player behind a tree after a 3 s push, the enemy filling the frame beside a stall, the enemy between camera and player, a view through the pavilion.)
- Does the hair follow? Does it hold its shape on the run? (`--springs off` shows the difference.)

### Review 2: attacks against a dummy (30 minutes)

The plan's questions: **do timing and impact feel good? Is motion quality good enough to judge by?**

Try: light, light, light at your own pace and as fast as you can. Cancel each cut with a dodge. Dodge, then light for the dash attack. Skill from a stand, and out of the string's second hit. Land hits from each side. `R` starts over. `--debug-draw combat` on the command line draws the hit volumes against the pose.

Pillars: 2 (readable), 3 (impact), 4 (cancels and dodges), 6.

- Does a press swing now? Does the string flow as fast as you press, and not faster?
- Can you read wind-up, strike and recovery on each move?
- Does a hit land everywhere at once: the freeze, the spark, the shake, the sound and the rumble?
- **Is the motion good enough to judge the fight by?** The player's moves are poses solved from targets, with no animator. O3's own read: the finisher and the skill's coil are strongest, the dodge is weakest, `light_2`'s anticipation is weak and the dash attack's hit wants a trail. If this fails, say so, because the plan's answer is an animator or a bought pack, not tuning (§6, §7).
- Do the sound and rumble suit each hit? Nobody has heard or felt them, and every number is a guess.

### Review 3: the fight, and tuning it (30 minutes): the real go/no-go check

The plan's question: **fight the enemy, and try tuning a move yourself.** `git checkout` the dummy first.

Fight for ten minutes. Win and lose. Dodge its overhead on purpose, which is the telegraphed one: a perfect dodge slows the world and a press answers with the dash attack. Then change something you didn't like, by the routes below, and play the next attempt. Going to your editor pauses the fight, and Esc or P ends the pause. Do at least one change by hand in a text editor and one in the timeline editor.

Pillars: all of 1 to 6.

- Is it fun? Would you keep playing? What's missing first?
- Does the dodge-and-counter rhythm land the way the reference does? ([m2-plan.md](m2-plan.md) §2 names Zenless Zone Zero.)
- Is the enemy fair and readable? Does it read as a threat? (`warrior.moves.ron`'s `brain:` is a first guess. Mashing light still wins in 13–15 s with 220–280 of 300 health left, on a script. Armour on the swipe is the next lever.)
- **Did tuning feel slow?** This is the question the off-ramp hangs on. Say which change took longest, and whether it was the engine or the finding out what to change. The spec's words: "if tuning still feels slow".

Then go on to [m2-go-no-go.md](m2-go-no-go.md).

## What changes what

Every row below ran against a running offscreen fight on 2026-11-02, except where it says it can't. Times are from the rehearsal on 410cf24b unless a row says otherwise (release `dev`, Apple M5, load average 11–30).

**How each route reaches a running fight:**

| Route | What you do | Time to see it | What says it worked |
|---|---|---|---|
| **Save** | Save the file | About 0.13 s, plus waiting for the next attempt | The log's `… reloaded`, then `the next attempt plays the new moves` for a move |
| **Push** (a phone) | `cargo xtask link push <file>` | The tool's `(in N ms)`, 6–25 ms here, plus the next attempt | The tool's reply |
| **Cook** | Save, then `cargo xtask content` | 0.8–1.5 s for the cook, then about 0.15 s | The log's `asset swapped in` |
| **Restart** | Stop, and run the command again, which cooks | 1.3–2.4 s to a fight with no window, plus a cook's 1–2 s if anything changed | A new fight |
| **Rebuild** | Stop, and run the command again, which builds | 3.6–4.2 s for a one-line edit under `--incremental`, of which 1.4–5.5 s is the build. Under `--release` the build alone is 76–152 s | A new fight |

A move in progress finishes on what it started with. A bad document changes nothing, and the log, or `link push`'s non-zero exit, names the move and the track. That includes a misspelt field (`cancles:` is refused by name, and doesn't compile as a move with no such track), a root key or knockback past 1,000 m, and a shape under 1 mm. A cook that started before your last save doesn't undo it: where the app has the file, what lands is the file over the cook's tables. `cargo xtask link status` lists each document's edits and what each fighter is playing, with its revision and windows.

### Moves

[player.moves.ron](../content/combat/player/player.moves.ron), and [warrior.moves.ron](../content/combat/enemy/warrior.moves.ron) for the enemy. The file's comments and the timeline editor name every field. All **Save**.

| If this is wrong | Change |
|---|---|
| A hit lands too early or late | The hitbox's `from` and `to`. **This moves the hit, not the blade:** a move plays its clip tick for tick, so to move what's seen, re-pose it (below) |
| The phases read wrong | `startup`, `active`, `recovery`, which must keep adding up to the clip's length. A length is refused by name, because only a cook can change it |
| A hit feels weak or strong | `attack`'s `hitstop`, `knockback`, `finish_freeze`, `cue` |
| The next cut won't come when pressed, or comes too soon | `cancels`' `from`, `to` and `allow` |
| A press gets lost, or fires late | `intents`' `buffer` and `priority` |
| It homes in too much or too little | `warps`' `max_distance`, `max_turn`, `keep` |
| The dodge saves too much or too little | `invulnerable`, `perfect_dodge`'s `from` and `to`, its `volume`'s size |
| The perfect dodge's slowdown | `perfect_dodge`'s `slow_motion` (`per_mille` must divide 1,000,000) and `counter`'s `ticks` and `allow` |
| Damage and how long a fight lasts | `attack`'s `damage` and `poise`, `fighter`'s `health`, `poise`, `poise_refill` |
| What a hit does to someone | `fighter`'s `reactions`, and those moves' own numbers (`flinch`, `stagger`, `knockdown`, `dazed`; for the warrior `stagger_light`, `stagger_heavy`, `knockdown`) |
| The enemy is too quick, or too slow to come | `brain`'s `patience`, `rest`, `recover`, `spacing`, `pace` |
| It picks the wrong attack | `brain`'s `attacks`: `weight`, `reach`, `against`, `recent` |
| It's easy to interrupt | `armour: (from, to, threshold)` on the move. The swipe has none, and that's why mashing wins |

### Everything else

| If this is wrong | Change | Route |
|---|---|---|
| Camera distance, height, how much it leads | [default_camera.ron](../crates/sapphire_camera/default_camera.ron): `combat.framing`'s `distance`, `pivot_bias`, `max_lead`, `safe_frame`, `fov_y`. `lock_on` the same | Save |
| The camera lags or snaps | The same file's `*_half_life`s, and `look`'s `max_turn_rate`, `idle_delay` | Save |
| The camera at walls | The same file's `collision` | Save |
| A shake | The same file's `shakes.presets`: `duration`, `frequency`, `rotation`, `translation`. Held to 0.05 rad and 0.05 m in sum | Save |
| What a hit fires | [default_cues.ron](../crates/sapphire_cues/default_cues.ron), a row by cue and weight: `effects`, `sounds`, `shake`, `screen` | Save |
| Rumble | The same rows' `rumble: (low, high, seconds)` | Save. Only a pad can judge |
| A screen effect's look | [default_screen_effects.ron](../crates/sapphire_world/default_screen_effects.ron): `light_hit`, `medium_hit`, `heavy_hit`, `poise_break`, `finish`, `perfect_dodge`, `skill_flash` | Save |
| The light and hour | [lookdev_day.ron](../content/lookdev/lookdev_day.ron), and `--time-of-day <hours>` on the command | Save |
| A cut's blend, a reaction's blend, walk and run blending | [character.animgraph.ron](../content/lookdev/character/character.animgraph.ron): `combat`'s `decay`, `exit`, each move's `decay`, `locomotion`'s `samples`, `smoothing`, `rate`, `transitions`. It takes effect at the next state's start | Save or push (134 ms and 7 ms, on f8fa8f3f) |
| The same on the enemy | [warrior.animgraph.ron](../content/combat/enemy/warrior.animgraph.ron), the same fields | Cook. A save isn't watched, and a push is refused by name |
| A move's pose, and what's seen when | [moves.py](../tools/motion/moves.py), by [its README](../tools/motion/README.md): `--write`, read a sheet with `sheet.py`, then cook. A cook rebakes the move's tables | Cook |
| Walk, run, strafe or a reaction's clip | The pack's sidecars under [kaykit_character_animations](../content/third_party/kaykit_character_animations/): a `retarget:` entry's `adjust`, `gain`, `follow` | Cook |
| A material | `content/combat/enemy/`, `content/lookdev/character/materials/` | Cook |
| How a sound plays | [content/audio/](../content/audio/)`<name>.cue.ron`: `volume_db`, `pitch`, `attenuation`, `sounds` | Restart: the mixer holds what it decoded at launch |
| Which sound a hit plays | `default_cues.ron`'s `sounds`, by a cue's ID. A cue already loaded is a Save. One that wasn't loaded at launch is refused live, keeping what fires, because only the table's cues load then, so it's a Restart | Save or Restart |
| Sparks and trails | [default_effects.ron](../crates/sapphire_vfx/default_effects.ron): compiled in, with no reload | Rebuild |
| Run speed, acceleration, turning, jump | [profile.rs](../crates/sapphire_gameplay/src/controller/profile.rs), `ControllerProfile::default()`: `run_speed`, `walk_speed`, `acceleration`, `deceleration`, `turn_rate_degrees`, `jump_speed`, `gravity`. **Code, not data,** and the likeliest complaint of review 1. Moves every replay's checksum, so finish with `cargo xtask replay --update` | Rebuild |
| Stick dead zones, look speed, bindings | [combat_bindings.ron](../crates/sapphire_controls/combat_bindings.ron): `DeadZone`, `Scale`. Compiled in | Rebuild |
| Hair's swing and limits | `SpringTuning::ACTION` in [springs.rs](../crates/sapphire_anim/src/springs.rs): `root_inertia`, `anchor_inertia`, `swing_limit`, `bend_limit` | Rebuild |
| HUD layout, colours, damage numbers | Constants in [hud.rs](../crates/sapphire_ui/src/hud.rs). `--hud-scale <n>` and `--hud-insets l,t,r,b` on the command | Rebuild |
| Touch layout, stick radius | [touch.rs](../crates/sapphire_ui/src/touch.rs), [hud.rs](../crates/sapphire_ui/src/hud.rs) | Rebuild |

### Keeping what you tuned

- **Save edits are in the working tree.** Commit them, or `git checkout` them.
- **After tuning the player's moves,** rewrite the simulation's cooked copy and the replays: `SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy`, then `cargo xtask replay --update`. A test fails saying so if it's forgotten. A `dev` build's fight already starts from a newer cook, so nothing looks wrong until then.
- **Any tuning of a fight's moves or the warrior moves replays' checksums:** `cargo xtask replay --update`, read what moved, and commit. x86-64's come from CI's artifact ([development-iteration.md](development-iteration.md), "Filling in x86-64's checksums"), so it costs two CI rounds.

### The timeline editor

With a fight running in another terminal, on the default port:

```bash
cargo xtask editor content/combat/player/player.moves.ron
```

It opens a window, and **nobody has opened it**. Drag a window's ends or body, or a cue, tick by tick. Space plays from the frame you're on, with the real simulation and a dummy. Cmd+S saves and pushes, and the side panel shows what it replaced or what the device refused. [development-iteration.md](development-iteration.md), "The timeline editor", has every key and the list of what no test can show. With no file named, or one that won't open, its list holds the repository's documents, and picking from that list with edits open asks first. If it won't open, the text file and a save do the same job. With no window: `cargo run -p sapphire_editor -- <file> --capture <png> --move light_1 --tick 8 --content target/content/desktop`.

### A recorded session joins the replay suite

Each review should end with its session kept: Z2 asks for them, because a session someone played is a regression test no script writes.

1. **Record it on committed moves,** in the same way the fight runs, with a file name of your own:

   ```bash
   cargo xtask run --platform desktop --incremental --features dev -- --arena --record target/review-3.sreplay --scenario arena-fight
   ```

   One fight, and it doesn't start over; quitting writes it. A name already there is refused. **A recording ends its file at the first live edit** and the fight goes on, because a replay can only answer for what it recorded. So tune first, commit, then record. A session recorded with edits you haven't rewritten into the simulation's copy won't replay (`MovesChanged`).
2. **Check it:** the log's `recorded a replay … checksum=<x>` must equal what a headless run reaches:

   ```bash
   mkdir -p /tmp/session && cp target/review-3.sreplay /tmp/session/
   cargo build --release -p sapphire_replay
   target/release/sapphire_replay check --update --dir /tmp/session --out /tmp/session/out
   ```

   It prints `ends at <x>`. A difference is a determinism bug and the most important thing the session can find, so keep the file and the log.
3. **Add it:** copy it to `tests/replays/<name>.sreplay`, run `cargo xtask replay --update`, and commit both. CI then fails on Linux and Windows until x86-64's checksums are filled in (above).

## A touch fight on a phone

What follows was rehearsed on a headless Android emulator and an iOS simulator (I3, [m2-plan.md](m2-plan.md)). No phone has run it. Everything that needed a person's hands was played by real multi-touch events on the emulator, and the list at the end is what only a phone can show.

### Android

With the phone in developer mode, plugged in, and `adb devices` listing it:

```bash
JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home \
  cargo xtask run --platform android --release --features dev -- --arena --fight
```

That cooks, builds, installs and launches it, and streams the app's log until it ends. The fight starts over 4 s after someone falls. For a session that's kept and can be checked afterwards, end the command with `-- --arena --record fight.sreplay --scenario arena-fight` instead: a recording is a single fight, so it doesn't start over, and quitting ends it.

Two traps. A recording's name is refused if it's already there: the app opens, logs `it exists; delete it first to replace it` and closes, looking like a crash. So name each session (`fight-2.sreplay`), or `adb shell run-as dev.sapphire.sandbox rm cache/fight.sreplay`. And the launcher's icon starts the app with no arguments, so that's the sandbox's own scene and not a fight: rerun the command, or start it again with `adb shell am start` and its `sapphire.args` as [m1-exit-run.md](m1-exit-run.md) does.

Back opens the pause: a tap ends it, and Back again leaves. A pause shows a veil and a two-bar mark here as on a desktop.

### iPhone

```bash
cargo xtask run --platform ios --release --features dev --identity <identity> --profile <mobileprovision> -- --arena --fight
```

The same `--arena --record fight.sreplay --scenario arena-fight` records here.

[m1-exit-run.md](m1-exit-run.md)'s "iPhone" section has the signing. An iPhone has no Back, so there's no pause by touch and no way out but the system's: go home and swipe the app away. A recording is kept each time the app goes to the background, because iOS never tells an app it was killed there.

### Fetching and checking a recording

```bash
mkdir -p /tmp/fight
adb exec-out run-as dev.sapphire.sandbox cat cache/fight.sreplay > /tmp/fight/fight.sreplay
cargo build --release -p sapphire_replay
target/release/sapphire_replay check --update --dir /tmp/fight --out /tmp/fight/out
```

On an iPhone, take it from the app's container with `xcrun devicectl device copy from`, under `Library/Caches/Sapphire/Sapphire Sandbox/` (not tried: there's been no iPhone to try it on). The app logs `recorded a replay … checksum=<x>` at a clean quit and `kept the recording so far … checksum=<x>` in the background, and the checker prints `ends at <x>`: they must be the same. A difference is a determinism bug, and the most important thing the session can find, so keep the file and the log.

### What to look for

The log says what the fight did, so a session can be read back after it's played:

| Line | What it says |
|---|---|
| `hit landed tick=… attacker=… target_actor=… damage=… outcome=…` | Each hit, once. Actor 1 is the player |
| `a fight ended fight=… ticks=… fell_at=… player_health=… enemy_health=… checksum=… allocations=0` | A fight's result. `allocations` must stay 0 on iOS. Android's counts are per process, so other threads show there: the headless replay's count is the one that holds |
| `hud button …`, `hud touch …`, `tick actions …` (`dev` builds) | What a button did, where the stick sat, what a step read |
| `frame stats … pacing=… thermal=…` | Every 2 s. Mobile-Low's target is 30 fps, with 0 missed in a quiet stretch |
| `audio stats … xruns=…` | `played` rises with the fight, and `xruns` should stay 0 |

Play these, in this order, in one session:

1. Walk with the stick: the character goes where the thumb points, as the view sees it, and keeps doing so after dragging the view around.
2. Hold the stick and tap attack, then dodge, then skill: each tap is one move. Holding a button is one press, not a string.
3. Drag anywhere that isn't a control: the view turns. Keep the stick and attack going through it, with three fingers on the glass.
4. Slide a thumb off attack and back: it keeps its button until it lifts.
5. Win a fight and lose one. With a thumb still on the stick when the next starts, the character walks on and a held attack doesn't swing until it's pressed again.
6. Go home mid-fight and come back: it picks up where it stopped. So does pulling down the notification shade or taking a call, with both thumbs down as it happens, and no input is stuck afterwards.
7. Turn the phone to the other landscape. On Android, Back, Back: the log ends in `quit presented=…` and `main returned status=0`.
8. If there's a Bluetooth pad: its first press takes the on-screen controls away, a finger brings them back, and only the pad that's in use rumbles on a hit.

On a `dev` build, after `adb forward tcp:7811 tcp:7811` (`iproxy 7811:7811` for an iPhone), `cargo xtask link capture <png>` takes a frame of the running app, and `cargo xtask link cvar debug.lose_device true` loses the GPU device to see the fight rebuild and carry on. Two fingers held still for a second in the open toggle the overlay (never two thumbs on the controls).

### What only a phone can show

An emulator's touches arrive as exact coordinates in perfect time, so none of these has been judged:

- **The layout and its sizes.** Whether the stick and buttons fall under real thumbs on a real phone, whether attack can be mashed without missing, whether the buttons are too close to dodge without looking, and what they look like at arm's length. They're I2's numbers, unchanged here.
- **The stick's feel:** its 60-point radius, a thumb's resting drift and how far a thumb actually travels, and the 2 m/s walk against the 6 m/s run a full push gives.
- **Latency to a thumb:** a press to a swing on the screen, in a 30 fps fight. The emulator's frame timing says nothing about a phone.
- **The view's drag rate:** 0.006 radians a point, I2's number. A drag up looks up while a pad's stick up looks down.
- **The pause's tap on a dense screen:** `tap_slop` is 10 window points, and Android's are pixels, about 3.3 dp on a 3× phone, so a tap may read as a drag, leaving Back as the only way out of a pause. The review read this and couldn't make it fail with no phone.
- **Haptics and sound:** there are no phone haptics, by choice. Whether a fight reads without them, how cues sound through a phone's speaker, and iOS's Ambient session with the player's music on and the silent switch.
- **Real interruptions:** a real call, an alarm, the iOS control center and the Android gesture navigation's edge swipes, whose touches the OS takes back in its own timing. The emulator proved `CANCEL` and the shade, and the simulator can't inject touches at all.
- **Heat and memory:** a fight throttled after 15 minutes against Mobile-Low's budgets (below), and whether a phone with little memory raises warnings mid-fight.
- **A phone's GPU drivers:** every run so far drew through Metal or MoltenVK on an M5, so an Adreno, Mali or A-series compile or a real device loss hasn't been seen. `debug.lose_device` is the same path, by a different cause.

## Exit test 2: a saved change on the next attempt

The test: a saved change to a move's timing plays on the next attempt with no restart, in under 2 s from save on the desktop and under 5 s on a phone over live link.

**What's being timed:** from the save to the first frame of a following attempt on the new data. A move in progress finishes first, so part of any number is waiting for the next press. The script below presses every half second, so up to half a second of each number is that wait. A person pressing when ready waits for nothing but themselves. The log's line names whichever of the fighter's moves starts first on the new data: in the rehearsal's 124 rounds `light_1` 86 times, and `light_2`, `light_3` or a knockdown otherwise. From that line on every start reads the edit, so a script may need a press or two more to reach the move you edited.

**The edit:** `light_1` in `content/combat/player/player.moves.ron`, moved 2 ticks later: `startup: 7` to `9`, `recovery: 23` to `21`, and `light_1_hit`'s `from: 7, to: 11` to `from: 9, to: 13`. Undo it to make the next round's edit. Any change is one round; the same bytes are no edit.

### On a desktop

With no window, which is how it's been measured, and which nothing about a window changes:

```bash
cargo xtask run --platform desktop --release --features dev -- \
  --offscreen --size 1280x720 --arena --fight --script attempts
```

Save the file. The log says:

```
moves of player.moves.ron, as revision 1; its clips' motion stays as last cooked, which only a cook changes reloaded
the next attempt plays the new moves document="player.moves.ron" mv="light_1" revision=1 actor=1 tick=151 since_ms=164.5
```

Save to attempt is about 0.13 s (the watcher's debounce and the frame it lands on) plus `since_ms`. `cargo xtask link status` shows the fighter's revision and windows.

Over live link, the phone's path on a desktop:

```bash
mkdir -p /tmp/tune && cp content/combat/player/player.moves.ron /tmp/tune/
# edit /tmp/tune/player.moves.ron, then:
cargo xtask link push /tmp/tune/player.moves.ron
```

The tool prints `(in N ms)`. N plus the log's `since_ms` is the number.

### On a phone

Start a fight that presses by itself, or play one yourself, on a `dev` build:

```bash
JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home \
  cargo xtask run --platform android --release --features dev -- --arena --fight --script attempts --tier mobile-low
```

```bash
cargo xtask run --platform ios --release --features dev --identity <identity> --profile <mobileprovision> -- \
  --arena --fight --script attempts --tier mobile-low
```

Then, in another terminal, after `adb forward tcp:7811 tcp:7811` or `iproxy 7811:7811`:

```bash
cargo xtask link status
cargo xtask link push content/combat/player/player.moves.ron
```

`cargo xtask link watch content/combat/player/player.moves.ron` pushes each save, which is the real loop. A document needs no cook, so a phone's cooked pack stays as it is, and a phone keeps its edits only until it restarts: a relaunch through `cargo xtask run` cooks the saved file in.

A change that needs a cook (the enemy's graph, a pose): `cargo xtask content --cook mobile`, then `cargo xtask link push`, which sends only what changed. Add both to the number.

### What to record

Device, OS, tier and the build's commit. For 20 rounds on a fight that's been edited at least once (warm), and 4 rounds each on a fresh launch's first edit (cold):

- the tool's `(in N ms)` and the log's `since_ms`, and their sum;
- their median, 90th percentile and most;
- `frame stats`' `worst_ms` through the rounds, because an edit shouldn't stall a frame;
- anything refused.

**Pass:** the most of the warm rounds and of the cold rounds is under 5 s on a phone, and 2 s on a desktop.

## Exit test 3: replays

The test: every combat replay reaches its recorded checksum for the runner's architecture on all three desktop OSes, with no heap allocation inside the fixed step (spec §9, gates 2 and 4).

**CI enforces it.** The `Replays` job in [ci.yml](../.github/workflows/ci.yml) runs `cargo --locked xtask --locked replay` on `ubuntu-24.04`, `windows-2025` and `macos-26` on every pull request, and it's required. Linux and Windows compare against one x86-64 file, which is what proves they agree, and macOS against the arm64 file. `Lint` holds the simulation's tree free of SDL, wgpu and the banned math.

By hand, in about a minute from cold and 20 ms once built:

```bash
cargo xtask replay
```

It ends `11 replays reach their checksums on aarch64`, each line with its ticks, final checksum, allocations in the step and its mean and longest step. A failure names the first tick that differs. A step that allocated fails too.

- `cargo xtask replay --list` names what's there.
- `cargo xtask replay --replay arena-fight-won` runs one.
- `cargo xtask content`, then `cargo test -p sapphire_sandbox --test replays -- --ignored --test-threads=1` runs them beside the renderer.
- To check a recording from anywhere, see "A recorded session joins the replay suite" and "Fetching and checking a recording".

**Moved on purpose:** `cargo xtask replay --update`, read what moved, commit, and fill in x86-64's from CI ([development-iteration.md](development-iteration.md)). **Moved on Windows alone:** a determinism bug, never a value to update.

## Exit test 4: Mobile-Low, throttled

The test: Mobile-Low holds 30 fps while thermally throttled, after 15 minutes of combat (spec §3). Nothing short of a phone answers it. Spec §3.5 has the same fight on an Apple M5, which says the work is all there and nothing grows.

**What thermal state means.** A phone can't hold its peak clocks. As it heats, the OS lowers CPU and GPU clocks to protect the battery and the hand holding it, so a frame that fits when cold can miss when hot. §3's budgets are for the hot phone. The OS says how far along it is. On iOS, `thermalState`: `nominal`, `fair`, `serious` (the OS is reducing performance) and `critical`. On Android, thermal status from `NONE` through `LIGHT`, `MODERATE`, `SEVERE` and `CRITICAL`, to `EMERGENCY` and `SHUTDOWN`. The app reads it every 2 s and logs one scale in `frame stats`: `thermal=Nominal`, `Fair` (Android's `LIGHT` and `MODERATE`), `Serious` (`SEVERE`), `Critical` (and past) or `Unknown`.

### Running it

Battery charged, screen brightness at maximum, plugged in over USB, which is what the log needs and which adds heat, on a table with no case. Note the room's temperature.

```bash
mkdir -p target
JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home \
  cargo xtask run --platform android --release -- --arena --fight --script fight-back --tier mobile-low 2>&1 | tee target/soak.log
```

```bash
mkdir -p target
cargo xtask run --platform ios --release --identity <identity> --profile <mobileprovision> -- \
  --arena --fight --script fight-back --tier mobile-low 2>&1 | tee target/soak.log
```

A script fights over and over, unattended, with everything on: real moves on both fighters, IK, springs, the sword, cues through the real mixer, the HUD. Leave it for 25 minutes, then stop it (Android: `adb shell am force-stop dev.sapphire.sandbox`, and the tool then exits 1, as it does for any killed process; iPhone: swipe it away). No `dev` feature, so it's what a player would run. This command hasn't run on a phone. The Android one ran on an emulator for 8.5 minutes ("Rehearsal"), and no iPhone has run the iOS one.

Don't touch it: a fight on a script is a slightly harder load than a person's, since it never stops fighting.

### What to read

From `target/soak.log`:

- **`frame stats`, every 2 s:** `fps`, `worst_ms`, `pacing=… N on time, N missed, N doubled`, `thermal`, `live_bytes`, and the stages `sim_ms`, `extract_ms`, `render_ms`.
- **`a fight ended …`:** `checksum` is the same every fight, and the same as a Mac's: `--offscreen --tier mobile-low --arena --fight --script fight-back` logs the line too.
- **`quality tier`:** `mobile-low`.
- **Memory:** Android, `adb shell dumpsys meminfo dev.sapphire.sandbox | grep -E 'TOTAL|Graphics'` at 5 and at 25 minutes. iPhone: Xcode's memory gauge, if it's attached (not tried). Budget 1.6 GB (spec §3.1). On the Mac it's 350 MB.
- **A low-memory warning, an audio `xruns` count above 0, anything at `WARN`:** record them.
- **GPU passes, once, not during the soak:** `--features dev --debug-overlay=stats` and `cargo xtask link capture <png>` shows §3.3's rows, against a GPU frame of 26 ms.

### Pass and fail

These lines are this file's reading of "holds", which the plan doesn't define. Over the **last 10 minutes** of the 25, with the phone at `thermal=Serious` or hotter:

- **Pass:** no 2 s window under 28 fps, at most 1% of frames missed (`missed` over `on time`), `live_bytes` and the meminfo total flat, and no fight's checksum differs.
- **Fail:** 30 fps isn't held by those lines. The answer is then in which stage grew: `render_ms` or a GPU frame near 33 ms, against spec §3.5's table, and it decides the ladder in spec §6.3 (dynamic resolution, optional passes).
- **Not throttled:** if `thermal` never reaches `Serious` in 25 minutes, that's a result, and the phone passes only as far as it went: record the highest state, say so, and run again warmer (charging from a wall socket, in the sun, under a cover, or after 15 minutes of `--tier mobile-high`). At a cold state the test hasn't asked its question. Mobile-Low at 30 fps may simply not heat some phones, which would itself be good news, and a reason to rerun at Mobile-High.

A crash ends the run, and the log's last lines and the OS's crash report are the finding.

### What to record

Phone model, OS, build commit, room temperature, on charge or not. A line every 5 minutes: fps range, worst `worst_ms`, missed, thermal state, `live_bytes`, meminfo. The minute `thermal` first reached each level, the fights played and the checksum. On both reference phones, per spec §3.5: an iPhone 12 and a Snapdragon 865-class phone.

## Rehearsal

Run on 2026-11-02 on an Apple M5 (24 GB, macOS 27.0.1, AC power), with other agents building beside it, on 410cf24b, which is before WA-3's clocks and the review's fixes. Those numbers weren't taken again. The second table says what ran on f8fa8f3f, the review's top. "Load" is the 1-minute load average. Builds are release `dev` unless a row says otherwise. The emulator is a private headless `sapphire_api36` and the simulator a private iPhone one, deleted afterwards. **Both draw on this Mac's GPU, and neither says what a phone does.** Nobody played anything: every press came from a script or a synthetic touch.

### What ran

| Exit test | What | Saw |
|---|---|---|
| 3 | `cargo xtask replay`, with `--list` and `--replay arena-fight-won` | 11 replays, 19,318 ticks, every checksum, no allocation. Mean step 2.3–4.6 µs and longest 69–255 µs over two runs |
| 3 | `cargo --locked xtask --locked replay` and `cargo test -p sapphire_sandbox --test replays -- --ignored --test-threads=1` | Both pass |
| 3 | An offscreen recording checked by hand (`--offscreen --arena --record … --scenario arena-fight --script …`, then `sapphire_replay check`) | 1,189 ticks. Log and checker agree on `2ab1ca4951fd2875` |
| 1, 3 | A fight by real multi-touch on the emulator, recorded (`--arena --record z3-touch.sreplay --scenario arena-fight --tier mobile-low`), fetched with `run-as` and checked on the Mac | 2,621 ticks, 156 button events, 31 hits. Back, Back left it cleanly (`main returned status=0`). Device and Mac both `458467478718ac11`, no allocation. Copied into `tests/replays/`, `cargo xtask replay --update` took it and all 12 passed. Taken out again, so it isn't in the suite |
| 2 | A real save of `player.moves.ron` on the Mac, Desktop-Low at 1280×720, `--offscreen --arena --fight --script attempts` | Warm over 25 rounds: median 487 ms, p90 765, most 1,183. Fresh process's first edit, over 4: median 556, most 585. `since_ms` median 359. Load 11–17, longest frame 39 ms |
| 2 | `link push` to the same | Warm: median 436 ms, p90 887, most 1,338. First edit: median 223, most 508. Tool 6–26 ms. Load 15–26, longest frame 49 ms |
| 2 | T2's path: a real save of `slice.moves.ron` under `--offscreen --record … --scenario duel --script attempts` | Warm over 20 rounds: median 378 ms, p90 461, most 585. Fresh process's first edit, over 3: median 351, most 398. Load 4–8, longest frame 12 ms |
| 2 | `link push` to the emulator, Mobile-Low, `adb forward` to a private port | Over 20 rounds: median 259 ms, p90 704, most 906 (the first edit after launch). Tool median 28 ms, most 51. 30 fps |
| 2 | `cargo xtask link watch content/combat/player/player.moves.ron` on the emulator | Save to attempt 517, 270, 489 and 504 ms |
| 2 | `link push` to the simulator, Mobile-Low, on a private port | Over 16 rounds: median 357 ms, p90 657, most 843. Tool median 21 ms, most 41. Load 110–190. `link capture` of 2622×1206 took 361 ms |
| 2 | A cook, then a push, on the emulator (`cargo xtask content --cook mobile`, `link push`, the enemy's graph) | 1.35 s and 0.13 s, 1.0 KB sent |
| 1, 2 | Every Save and Cook row of "What changes what", on a running fight at 640×360 | The times in those tables. Load 20–30 |
| 1 | `cargo xtask bundle --platform macos --incremental --features dev` | Builds and bundles. Not launched, since that opens a window |
| 2 | The desktop's commands as written, with a private address: `cargo xtask run … --offscreen --size 1280x720 --arena --fight --script attempts`, then `cargo xtask link status`, `push /tmp/tune/player.moves.ron` and `capture` | Quality tier Desktop-High. Push 16 ms, `since_ms` 415. Capture 145 ms |
| 2 | A restart: `cargo xtask run --platform desktop --incremental --features dev -- --offscreen --size 640x360 --tier desktop-low --arena --fight --script fight-back`, from the command to a `status` reply, nothing changed | 1.3–1.8 s, and 2.0–2.4 s under `--release`. A window adds whatever it adds. The fight ended on the same checksum as `--release`'s, `0dfb07dd0da45a9c`. Load about 8 |
| 2 | A rebuild, the same command after `run_speed: 6.0` became `6.5` in `profile.rs` | 3.6–4.2 s from the command to a `status` reply. The fight's checksum moved to `65ed657c12a51555`, and went back when the edit was undone. Load about 7 |
| 2 | One-line edits to what the sheet calls Rebuild, the build alone | `--incremental`: 1.4 s (the HUD's margin), 1.7 s (a spring's limit), 2.1 s (run speed), 2.8 s (a function in the renderer), 4.1 and 5.5 s (a function added to `sapphire_core` and to `sapphire_math`, 19 and 22 crates). Load about 10. A plain debug build: 1.3–2.1 s on doc comments and small edits. `--release`: 76–152 s with 4 build jobs, load 17–33. First builds after a cook: 130 s, 94 s and 147 s with 4 jobs |
| 2 | One whole fight on those three builds (`--bench 1 --tier desktop-low --size 1920x1080 --arena --replay tests/replays/arena-fight-won.sreplay --cues --hud live --blade on --time-of-day 10`), alternating, least of 3 rounds after a discarded one | CPU frame p50 0.54 ms `--release`, 0.82 ms `--incremental`, 1.44 ms debug. GPU frame p50 5.14, 4.90 and 8.30 ms, p99 7.94, 7.10 and 14.99 ms. Load 7–13 |
| 4 | Its Android command as written, no `dev`, on the emulator | 8.5 minutes, 19 fights, one checksum (`0dfb07dd0da45a9c`), no allocation in a step. `live_bytes` 21.35–21.43 MB, PSS 168.7 MB, `thermal=Nominal` throughout. Load reached 328 and fps fell as low as 4.4 for it, then came back to 30, so it times nothing. The tool exits 1 at `force-stop`, as it should for a killed process |
| 4 | Its script on the simulator (a `dev` bundle from `cargo xtask bundle --platform ios-sim --release --features dev`) | 3 fights, the same checksum as the emulator and the Mac. 30 fps on the display link, `live_bytes` 23.0 MB, `thermal=Nominal` |
| 4 | `cargo xtask bundle --platform ios --release`, signed ad hoc, which checks everything but the signing | Builds and bundles. No device installs it |
| 4 | The thermal reading, by `adb shell cmd thermalservice override-status 3`, then `reset` | `thermal=Serious`, then back. The simulator and an unforced emulator read `Nominal`, and an iPhone's real states are unproven |
| 1 | `--debug-draw combat --cues --hud live`, `--springs off`, `--debug-overlay=combat`, `--party --script party-swap`, `--time-of-day 19.75`, each offscreen with a capture | All ran, and the pictures show what they should |
| 1 | The dummy recipe under `--script fight-back` | After 75 s the player has 240 of 300 and the warrior 3,886 of 5,000 |
| 1 | `sapphire_editor content/combat/player/player.moves.ron --capture <png>` | The timeline and the clay figure draw on the real moves. On the CPU, with no window |

### What ran on the review's tree (f8fa8f3f)

Offscreen at 640×360, `--incremental`, Desktop-Low, load about 5.

| What | Saw |
|---|---|
| A save of `character.animgraph.ron` in `--arena --fight --script attempts` | Reloaded in 134 ms: `character's animation graph, from the next state's start` |
| A push of the same | Exit 0 in 7 ms, with the same reply |
| A push of `warrior.animgraph.ron` | Exit 1: `only character.animgraph.ron takes edits, and a cook brings any other graph` |
| A save of `warrior.animgraph.ron` | Nothing, in 4 s |
| A save of `player.moves.ron` with `cancles:` | Refused in 123 ms, naming the line, the field and `MoveDocument` |
| `--capture … --arena --hud live --hud-paused --capture-ticks 30` | The veil and the two-bar mark in the frame's middle, read from the picture |
| `--script fight-back`, one fight | The same checksum as on 410cf24b, `0dfb07dd0da45a9c` |
| `cargo xtask replay` | 11 replays pass, on the `fight` and `party-swap` checksums the review moved |

### What didn't run, and why

- **Any desktop window,** because agents may not open one. So `cargo xtask run --platform desktop` and `--platform macos` without `--offscreen`, `--arena --record` in a window, the pointer's capture, the pause on losing focus, the veil on a real window, the overlay's keys and everything in the three reviews.
- **A gamepad, rumble, sound.** No pad is attached, and nobody can listen.
- **`cargo xtask editor`,** whose window nobody has opened.
- **A phone,** which doesn't exist here. So `cargo xtask run --platform ios`, `iproxy`, `devicectl`, Xcode's gauges, and any thumb on the touch controls.
- **A 25-minute soak.** The emulator's 8.5 minutes show the route works, and an emulator doesn't heat.
- **A touch on iOS.** `simctl` can't inject one, so the simulator's script pressed for it.

### What it found

- **Nothing read the OS's thermal state,** so exit test 4 had nothing to read on an iPhone. `Platform::thermal_state` is new, and `frame stats` now carries `thermal` and `live_bytes`.
- **A fight's graphs had no document route.** On 410cf24b the docs and the tool's usage said a saved graph reached a running fight, and it reached only `--graph`'s show. The tuning review found the same thing and gave the character's graph a route. `warrior.animgraph.ron` stays cook-only.
- **`patience` stops at 3,600 ticks,** so the dummy swings about once a minute, and doesn't stand idle for good as a first draft of this file said.
- **A timing script locks onto `attempts`' beat.** The first harness pushed at a fixed spacing and read 195 ms every round. Rounds here are spaced at random, so they sample the wait.
- **`link watch` pushes a named document twice** when the device lists it too. The second is "the same moves", and harmless.
- **The attempt line doesn't always name the edited move.** It names the first move to start on the new data, 86 times in 124 rounds the edited one. The numbers here are to that line.
- **Fights after an edit end on another checksum,** because they start from the edit, and a relaunch goes back to the cook. Compare checksums only across fights with no edit.
- **A one-line edit took 76–152 s under `--release`,** with its thin LTO and no incremental compilation. `cargo xtask run --incremental` is new, and the sheet's commands use it.
