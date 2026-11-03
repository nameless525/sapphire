# M2 go/no-go

**For the maintainer: read this page first.** The decision is yours, and it's the one spec §10 set for M2's end: carry on in Rust into M3, or take the off-ramp, moving gameplay and tools to Godot with gdext and keeping the Rust systems.

**Summary.** The slice is built: one character against one enemy, with real moves, a real brain and a live HUD, in the look-dev town's square, tunable while it runs. Agents built it, and **nobody has played it**, so what the gate asks, whether it feels right and tunes quickly enough, has no answer yet. What can be measured without a person holds, most of it by a wide margin: a saved change to a move's timing plays on the next attempt in about 0.5 s and at most 1.2 s against 2 s, replays agree across three OSes with no allocation in a step, and a whole fight's frame leaves room on an Apple M5, though no phone has run any of it. Five agents who hadn't built it then reviewed the code and found 31 bugs, now fixed, three of which would have cost you a save. The recommendation below is to carry on in Rust, and **it rests on evidence short of the feel test**: nothing measured points at the off-ramp, and the largest risks, whether the motion is good enough and whether it's fun, wouldn't be fixed by it. Thirty minutes with a gamepad can overturn that.

## What you need to do to decide

1. **Thirty minutes, with a gamepad on this Mac:** review 3 in [m2-exit-run.md](m2-exit-run.md). `cargo xtask run --platform desktop --incremental --features dev -- --arena --fight`. The window takes the pointer on launch, and Esc or P gives it back. Fight for ten minutes, change a move's timing and a camera number, and play the next attempt.
2. **Answer two questions in your own words.** Is this worth tuning further? When you tuned it, did it feel slow?
3. **Read your answers against "What would change the answer", below.**

All three reviews take 90 minutes in one sitting, and a phone takes about 45 minutes more when one exists. **A phone doesn't decide this today.** It decides the gate's mobile half, which spec §10 also asks for, and a mobile-first game can't ship on the desktop's answer. Deciding without one settles the question of tuning and leaves that one open.

**The plan's order wasn't kept.** [m2-plan.md](m2-plan.md) §5 says that if review 3 isn't working, "stop before spending anything on phase 5". You told the run to keep going without asking, so phase 5 (I2, I3, G7, Z1, Z3) was built with no review behind it. Its cost is spent, and the question stands as it did.

## What's proven, and by what

The exit tests' numbers were taken on 410cf24b, before the review's fixes, and not taken again. A spot check on 7a28c828, in [m2-exit-run.md](m2-exit-run.md), found the routes behaving and a script's fight on the same checksum.

| Claim | Evidence | Conditions |
|---|---|---|
| A saved move plays on the next attempt with no restart (exit test 2, desktop) | Real saves of `player.moves.ron` against a running fight: **median 487 ms, p90 765, most 1,183** over 25 rounds; a fresh process's first edit 556, most 585, over 4. Against 2 s | Release `dev`, Apple M5, offscreen at 1280×720, load average 11–17. Up to 500 ms of each is the script's wait for its next press, and each is to the first move on the new data, the edited one in 86 of 124 rounds |
| The same over live link | **Median 436 ms, p90 887, most 1,338;** first edit 223, most 508. The device replies in 6–26 ms | The same, load 15–26 |
| The same on an emulator and a simulator, over live link | Emulator: median 259 ms, p90 704, most 906 over 20 rounds. Simulator: median 357 ms, p90 657, most 843 over 16 rounds. Against 5 s | They draw on the Mac's GPU. **Neither is a phone** |
| Replays are deterministic (exit test 3) | 11 replays, 19,318 ticks, reach their checksums on Linux and Windows (one file between them) and macOS, on every pull request. Sessions played by touch on an emulator, and by a script on a simulator, replay on the Mac to the checksum the device logged | CI. macOS by a local stand-in since the outage (below) |
| No heap allocation in a fixed step (R6, gate 4) | Enforced on every step of every replay, naming the tick. Mean step 2.3–4.6 µs and longest 255 µs over two runs, against 1,000 µs | Apple M5, `cargo xtask replay`'s build: `dev`, optimized, not `--release` |
| A fight's frame fits | CPU p50 0.24–0.36 ms; GPU p50 1.35 ms at Mobile-Low's settings, 6.9 ms at Desktop-High's. One row is repeatably over: Desktop-Low's post with a fused screen effect up, 0.78 ms against 0.7 (spec §3.5) | **Apple M5.** Not a phone, not throttled |
| Nothing grows | 17.8 minutes at a true 30 fps: footprint +0.05 MB a minute, 43 fights ending on one checksum | Apple M5, Mobile-Low's size and cook. A load this light says nothing about heat |
| Touch reaches the fight | Won and lost by real multi-touch events on an emulator (I3), and fought again by them on 410cf24b | An emulator's touches are exact, and its timing means nothing |
| The simulation holds under fuzz | 51,678 swaps, 542 of them forced, 156 parties down, 6,609 perfect dodges, 6,035 counters and 1,007 defeats, across 200 seeds × 3 swap rates at 4,000 ticks, each run twice: the same results, no allocation. 400 restarts of 5 scenarios and 2 hours of ticks in 4: live bytes flat | A reviewer's temporary harness, deleted after the run, so nothing in the repository repeats it. arm64 only, so no proof across OSes. CI's replays are that |
| Regressions get caught | 142 golden scenes on three backends and about 3,750 `#[test]`s, required on every pull request | Software rasterizers on Linux and Windows |

## What isn't proven

- **Whether it's fun, or feels right.** Nobody has played it. Pillars 1 to 7 have stand-in numbers (a press moves the character by tick 1), and the plan itself says the reviews overrule them.
- **Whether the motion is good enough to judge the fight by.** There's no animator. The player's moves are poses solved from targets, and locomotion and reactions are chibi clips retargeted onto the character. O2 and O3 both call their results first drafts.
- **That the window works on a desktop by hand.** An outside reviewer ran a scripted fight in a native Mac window on 7ee1583f, near 60 fps once settled, and a resize held. Nobody has played in one: no pad, mouse look, capture or focus pause has run. Agents here may not open a window. I3 ran the same loop on an emulator and a simulator. The review added capturing the pointer, pausing on lost focus and a veil, with tests of what decides them and nothing that shows what SDL does.
- **The timeline editor by hand.** Tests and captures run it, and its window has never opened. Two of its fixes were made by reading alone.
- **Sound and rumble.** Nobody has heard or felt either.
- **Anything on a phone.** After three milestones there's no phone number in this project: not a frame, not a shader compile, not a thermal state. So exit test 2's phone half, exit test 4 and touch by thumb are all open.
- **End-to-end latency,** press to glass, which spec §4.2 asked of M2.
- **Windows and Linux on real GPUs,** owed since M0.
- **Godot.** There's no prototype and no measurement of the off-ramp, so everything here about it is reasoning. "Months, not years" is the research report's estimate, not a result.

## What the review found

After the packages were done, five agents who hadn't built the code reviewed it: the simulation, animation, input and audio, the fight in the app, and tuning and tools. They found **31 bugs**, 5, 3, 6, 4 and 13 of them, in code that CI passed, and all 31 are fixed. 29 came with a test that their reports say failed first, not re-run against the old code for this page. The other two were made by reading. About 20 smaller items, mostly plausible and not made to fail, stay open in [m3-handoff.md](m3-handoff.md). The same pass added three things a desktop's window lacked: capturing the pointer, pausing on lost focus and a pause you can see. A sixth agent read the camera's framing pass, the last change to land, and found six more, all fixed, each with a test its report says failed first, in code whose tests and bounds had passed.

- **For the simulation:** its promises held. The fuzz run above found no divergence and no allocation. Its five bugs were at the edges of what a document or a fallen character can do: a defeated character still walked on the stick, a swap press outlived its buffer by a tick, a new fight's setup read the old document, a root key of 1e30 m panicked a debug build's step, and a shape under 1 mm made a fighter nobody could hit.
- **Against "built and green":** 31 bugs got past every gate, and nobody can say how many are left. Many are what a person would have met in the first minutes: a fight starting over left sparks, shakes and rumble running, a paused view went on moving, and a held button swung in the next fight.
- **The two windows are the weakest evidence.** The editor took 9 of tuning's 13 fixes, and two of those were made by reading alone. The desktop window's capture and focus pause rest on tests of the decisions and on reading SDL's source. Review 3 touches both first, so that's where to expect the next bug. A bug there is a finding about that window, and says little about Rust.
- **On "the tuning loop is fast":** the timing stood, but the loop could lose your work. Three fixes would each have cost you a save. A misspelt field (`cancles:`) compiled as a move with no such track, so saving deleted the track with only `as revision 2` to say so. A cook that started before a save replaced it in the running fight, while the file said otherwise and nothing would bring it back. Any pick in the editor's file list read the file over your edits and their undo. All three are fixed. So are a save of the character's graph doing nothing in a fight, though the docs said it would, and `recovery: 400000000` hanging the editor on the file that needed fixing.

## The evidence for carrying on in Rust

**The tuning loop is already fast for what's data.** The research report's bar was "tuning hitstop, cancel windows and camera through data hot reload". All three are a save away, with no cook and no restart. A hit's `hitstop` and a move's `cancels` in `player.moves.ron` are the measured 0.5 s, including the wait for a press, `default_camera.ron` applied 121 ms after a save, and the character's graph 134 ms, with the enemy's on the same route. Poses, materials and clips take a cook of 0.75–1.5 s and swap in about 0.15 s later. A bad document keeps what's playing and names the move and track (Apple M5, load 5–30). The review closed three ways this loop could lose work (above).

**The simulation is cheap, headless and checksummed,** which is what a combat game most needs and what's hardest to add later. A duel's step takes about 3 µs against 1,000 µs. All 11 replays run with no GPU in the tree in about 0.12 s, a test can play a whole fight, CI proves three OSes agree, and a session played on a device can be checked on a Mac. If the off-ramp kept these systems behind gdext they'd survive, so this is evidence for Rust systems under either answer.

**The budgets leave room, on a Mac.** At Mobile-Low's settings the M5's 99th percentiles leave ×79 on the CPU and ×11 on the GPU (spec §3.5). An A14 isn't an M5, so this says only that nothing is obviously out of reach.

**The look is already here, and it's in Rust:** ramps, SDF faces, outlines and the post stack, held by goldens. It was M1's whole work. Keeping it on the off-ramp means either rebuilding it in Godot or running a wgpu renderer beside Godot, and nobody has designed that.

**The largest open risks don't move with the engine.** Motion quality, an enemy that reads, and one person authoring anime-grade content are content problems. Godot's animation tools would help a person work on them, but the clips still have to come from somewhere.

## The evidence against

**Some tuning is code, and code means a restart.** The report's bar also named `subsecond` hot-patching, and spec §7.2 planned it with a `sapphire_dylib`. **Neither was built.** So what isn't data needs a rebuild and a new fight. The build itself turned out to be the smaller part, once the rehearsal measured it: a one-line edit takes 76–152 s under `--release`, which has thin LTO and no incremental compilation, and 1.4–5.5 s under the new `--incremental`, which has neither limit, costs about 0.3 ms more CPU on a 16.7 ms frame and plays to the same checksums (Apple M5, load 7–33). From the command to a fight taking input, with no window, is 3.6–4.2 s after the edit against 1.3–2.4 s with none. A script language's reload might keep the fight going, though nobody has measured one here. These are code today: hair's swing limits, target choice, HUD and touch layout, bindings and sparks.

How the character moves was the first of that list and the likeliest thing review 1 wants to change, so it's no longer on it: run speed, acceleration, turn rate, jump and the rest are each fighter's `movement`, in its document, and a save reaches the next step (below).

If you find yourself in those files and the restart is what hurts, that's a real strike, and it's fixable in Rust before it decides anything.

**The tools are thin.** T3's editor is a narrow timeline, and it's never been opened. M3 asks for a document-model editor with undo, play-in-editor, a sequencer, UI with text and layout, localization and saves, and its exit test is "a boss authored with no Rust changes", which means Luau bindings. Godot brings an editor, animation tooling and UI. That's the strongest reason to take the off-ramp, and it doesn't depend on this slice: M2 was built to answer tuning, and it can't answer this.

**Authoring is text and Python.** A move is a RON document and a posing script. That suits an agent. Whether it suits you is part of review 3.

**Everything has one consumer.** One character, one enemy, one stage, one moveset. What's data today hasn't been shown to generalize, and adding an enemy still needs Rust in three places ([m3-handoff.md](m3-handoff.md)).

**What took how long says little.** In the plan's own terms M2 was 4 L, 23 M and 5 S packages plus three reviews: at a week or two for an L, under a week for an M and a day or two for an S, about 20 to 35 person-weeks. It was built in about 12 hours of calendar time, by commit times from 2026-11-01 23:20, by up to nine agents in parallel: 200 commits and about 129,000 lines of Rust, and a review pass after. **That isn't evidence about Rust and a person.** Agents don't feel a slow compile as you would, and don't need an editor. It does mean sunk cost is a weak argument: redoing the gameplay layer costs less than the report's "months" assumed, if agents do it.

## What would change the answer

| If you find | Then |
|---|---|
| It's fun or close, and tuning felt quick | **Carry on.** Run the phone's tests when one exists, and start M3 |
| It isn't fun, and tuning data was quick | **Don't take the off-ramp yet.** Tune for a session, then look at what's wrong. Motion quality means an animator or a bought pack (plan §6, §7), and would be as wrong in Godot |
| Tuning data felt slow | Say what was slow. Finding the right field is the editor and the sheet, and fixable. If it's the loop, it contradicts the numbers above, and that's worth seeing |
| What you want to tune is code (hair, targeting) | Move it into documents and try again before deciding. It's small work, a row at a time, as movement's was |
| It's fine, but you want an editor for M3 | That's the off-ramp's real case, and it doesn't rest on this slice. It deserves its own spike before M3 starts: gdext driving the headless `Simulation`, with a wgpu viewport inside Godot |
| The window doesn't open, or the fight misbehaves on a desktop | A bug, and it says nothing yet. Keep the log, and try again after the fix |
| A phone fails exit test 4 | Decide by how much. Read which row grew against spec §3.5, and walk spec §6.3's ladder first. Failing outright at Mobile-Low threatens the mobile-first plan under either engine |

## Recommendation

**Carry on in Rust, once you've played it and it isn't wrong for reasons the engine owns.** This is a recommendation, not a finding, and it comes from the agents who built the thing: the test the gate exists for hasn't run, and an agent can't run it. What rests on evidence is narrower. Nothing measured points at the off-ramp, the data loop beats its bar by a wide margin, and the simulation has properties worth keeping under either answer. The review doesn't move it: nothing it found is a reason to leave Rust, and all of it was fixed in Rust within the pass. It does lower how much "built and green" should reassure you.

**The one change this recommended ahead of review 1 is done, rather than waiting for review 1 to find the movement wrong.** A fighter's controller numbers live in its move-set document, `player.moves.ron` and `warrior.moves.ron` hold today's values to edit, and saving `run_speed` moves the next step of a running fight. A real file write reaches the step after it in a median of 136 ms (p90 144, most 149, of 25 saves, and 135–139 ms as a fresh process's first edit), most of that hot reload's 100 ms debounce, on an offscreen fight at Desktop-Low, 1280×720, on this M5 at load 6–7, and the speed it logs there is one tick's change from the old number. So what you're likeliest to want to change first doesn't start the fight over. With today's numbers no fight changed: every replay's per-tick checksums stayed the same.

## Notes on the evidence

- **CI during the macOS outage.** GitHub's macOS runners were out from about 12:30 to about 23:20 UTC on 2026-11-02. Landings in that window were verified on Metal, tests, replays and iOS builds by a local stand-in running CI's four macOS jobs on this Mac, not by CI's own. The docs check, the leftovers, movement as data and the camera's framing came after, on CI's own macOS jobs. Linux, Windows, Android and lint jobs ran in CI as usual.
- **Which tree.** The exit tests' numbers come from 410cf24b. WA-3's clocks and the review's fixes came after. A row saying 7a28c828 was taken there.
- **Every number here** comes from one Apple M5 laptop on AC power, with other agents building beside it, which is why ranges and worst cases are quoted. It isn't one of the spec's reference devices. [m2-exit-run.md](m2-exit-run.md), "Rehearsal", has the exit tests' conditions, and spec §3.5 has the budgets'.
