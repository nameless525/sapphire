# The M2 exit run

M2 ends when one character against one enemy feels right with a gamepad and with touch, replays are deterministic in CI, and Mobile-Low holds 30 fps while fighting ([engine-spec.md](engine-spec.md) §10). This file starts with what's needed for touch; Z3 adds the rest.

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

Back opens the pause: a tap ends it, and Back again leaves.

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
| `frame stats … pacing=…` | Every 2 s. Mobile-Low's target is 30 fps, with 0 missed in a quiet stretch |
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
- **Haptics and sound:** there are no phone haptics, by choice. Whether a fight reads without them, how cues sound through a phone's speaker, and iOS's Ambient session with the player's music on and the silent switch.
- **Real interruptions:** a real call, an alarm, the iOS control center and the Android gesture navigation's edge swipes, whose touches the OS takes back in its own timing. The emulator proved `CANCEL` and the shade, and the simulator can't inject touches at all.
- **Heat and memory:** a fight throttled after 15 minutes against Mobile-Low's budgets (Z1), and whether a phone with little memory raises warnings mid-fight.
- **A phone's GPU drivers:** every run so far drew through Metal or MoltenVK on an M5, so an Adreno, Mali or A-series compile or a real device loss hasn't been seen. `debug.lose_device` is the same path, by a different cause.
