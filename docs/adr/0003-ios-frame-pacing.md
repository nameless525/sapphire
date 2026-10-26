# ADR 0003: iOS frame pacing (M1 F7)

- **Status:** accepted (M1). The device check in spec §6.2 is still to run.
- **Date:** 2026-10-26

## Context

Spec §6.2 asks iOS for 60 fps on Mobile-High and 30 on Mobile-Low (the tier's `frame_rate` in `tiers.ron`), with the display's preferred rate set so a ProMotion panel doesn't run at 120 Hz for a 60 fps game, and with even frame intervals. Open question Q13 recorded that it couldn't be done on SDL's main callbacks as M0 left them (ADR 0002).

What SDL 3.4.18, the version `Cargo.lock` builds, does on iOS:

- **SDL's iOS entry owns the frame loop.** `SDL_EnterAppMainCallbacks` (`src/main/ios/SDL_sysmain_callbacks.m`) initializes the app, then creates a `CADisplayLink` held in a file-static object and returns into UIKit. Every tick calls `SDL_IterateMainCallbacks(true)`: pump events, dispatch them, run `SDL_AppIterate`. On a display faster than 60 Hz it asks for `CAFrameRateRange(80, 120, 120)`; otherwise it keeps the default range, the display's own rate. `SDL_HINT_MAIN_CALLBACK_RATE` is read only by the generic loop (desktop, Android) and Emscripten's. Nothing in SDL's API reaches that link.
- **Each window has a second link.** `SDL_uikitviewcontroller` creates one on displays faster than 60 Hz, also asking for 80–120 Hz, and does nothing on its ticks. It exists to keep the panel fast. The only public call that reaches it is `SDL_SetiOSAnimationCallback`, which stops it before starting an animation callback, and starts none if the callback is null.
- On a ProMotion iPad (and an iPhone whose Info.plist sets `CADisableMinimumFrameDurationOnPhone`), either link holds the panel at 120 Hz and runs the main thread at 120 Hz whatever the game's rate. On every iPhone it runs iteration at 60 Hz even for a 30 fps tier: the iOS 26.3 simulator ran the Mobile-Low sandbox at 60.0 fps at M1's base (a560a84).

What wgpu 30 can do: its Metal surface presents with `[MTLCommandBuffer presentDrawable:]` (`wgpu-hal` `metal::Queue::present`). It has no path to `presentDrawable:afterMinimumDuration:` or `presentDrawable:atTime:`, and no way to present drawables from a `CAMetalDisplayLink`, which acquires them itself. So the engine decides when a frame starts, and FIFO decides when it is shown: the first vsync after the GPU finishes.

The candidates Q13 named, and others found on the way:

1. **Present-interval pacing on SDL's link.** Let SDL's link run and skip iterations, or set a minimum present duration. The minimum duration is out of reach through wgpu 30. Skipping iterations leaves the panel at 120 Hz and the main thread waking at 120 Hz on ProMotion. SDL's link exposes no vsync timestamps either, only the time `SDL_AppIterate` happens to run. With the panel at twice the frame rate and no timed present, a frame shows one or two vsyncs after it starts depending on whether its GPU work ends before or after the 8.3 ms mark: alternating 8.3 / 25 ms intervals, the 120/60 judder.
2. **An engine link beside SDL's.** SDL's link keeps running, iterating, and asking for 120 Hz, so the panel stays fast. Two links would drive one loop.
3. **An engine link in place of SDL's, on SDL's own main-callback functions.** Do what `SDL_sysmain_callbacks.m` does, with the engine's link.
4. **Re-implementing the main callbacks with public API** (`SDL_AddEventWatch`, `SDL_PeepEvents`). This duplicates SDL's lifecycle dispatch, the event watch that delivers backgrounding and termination the moment they happen. It also makes `SDL_HasMainCallbacks()` false, which changes when SDL's UIKit backend observes lifecycle notifications.
5. **`SDL_SetiOSAnimationCallback` as the driver.** It runs at an integer divisor of `maximumFramesPerSecond` through the deprecated `preferredFramesPerSecond`, gives no timestamps and is tied to a window. SDL's main-callback link would still run.
6. **`CAMetalDisplayLink` (iOS 17+).** It hands out drawables with a target presentation time, so it paces presentation itself. This is the long-term answer, but wgpu has to present drawables it didn't acquire. That is later work.

## Decision

1. **The engine owns iOS's frame clock (option 3).** On iOS, `sapphire_platform` enters SDL's main callbacks through `ios_loop.rs` instead of `SDL_EnterAppMainCallbacks`:
   - `SDL_InitMainCallbacks` initializes the app and installs SDL's lifecycle event watch.
   - The engine's `CADisplayLink` (objc2 bindings, already in the tree through wgpu's Metal backend) calls `SDL_IterateMainCallbacks(true)` on every tick, on the main run loop in the default mode, as SDL's link did.
   - `SDL_QuitMainCallbacks` and `exit` end the process, as SDL's iOS entry does.
   - Event dispatch, lifecycle events and termination stay SDL's code.
   - These three functions are SDL internals (`src/main/SDL_main_callbacks.h`), reachable because SDL is linked statically. Patch releases can change them and SDL's iOS entry too, so the workspace pins `sdl3-sys` exactly (`=0.7.2`, which pins SDL 3.4.18's sources). A compile-time assertion that SDL is exactly 3.4.18 stops the build on any upgrade, patch releases included, until the signatures and the entry are checked again.
2. **The link asks for exactly the target rate.** It uses `preferredFrameRateRange = (r, r, r)`, or the default range without a target, so a ProMotion panel can run at 60 or 30 Hz. SDL's per-window link is stopped at window creation through `SDL_SetiOSAnimationCallback(window, 1, NULL, NULL)`.
3. **A pure pacer gates and measures** (`sapphire_platform::pacing::FramePacer`). It is fed each tick's vsync time and the display's refresh period.
   - **Frames are due on whole refreshes.** Each frame owes the target interval in refreshes and is due on the first refresh that pays it. What a frame overpays carries to the next, so frames keep the target rate on any panel and never run faster: 60 fps takes 2 refreshes at 120 Hz, 3, 2, 3, 2, 2 at 144 Hz and 2, 1, 2, 1 at 90 Hz.
   - A tick before its frame is due is held back, so a link the system runs faster than asked never doubles frames.
   - A frame late for its refresh starts the count afresh rather than take the time back with a short interval.
   - Counting is in refreshes (each interval rounded to whole refreshes), not a running deadline in nanoseconds. That keeps decisions exact under timestamp noise, and the cadence can't drift when the reported refresh is a little off the panel's (60 against 59.94 Hz).
   - It counts frames on time, target intervals missed, frames doubled and ticks held back.
   - A trip to the background and a change of target aren't measured. Frames in the background get at most one target interval, so a tick that comes back before the return is reported simulates none of the time away.
   - It is deterministic and unit-tested with injected 60, 90, 120 and 144 Hz ticks, noisy, late and irregular ones.
4. **The frame's time is the time between the frames' vsyncs.** The pacer is fed `CADisplayLink.targetTimestamp`, the vsync the frame will be shown at, not `timestamp`, the one just shown. `Platform::frame_delta` is then the time each frame will be on screen, even for the first frame after a change of rate, as Apple advises for timing content. The app feeds it to the fixed step and to presentation. Steady frames then run exactly two fixed steps at 30 fps, whatever the main thread's scheduling jitter.
5. **The target comes from the quality tier.** The app hands `Quality::frame_rate` (`tiers.ron`'s `frame_rate`) to `Platform::set_frame_rate` at startup and on every tier change. The link follows from its next tick.
6. **Desktop and Android are unchanged.** SDL's loop runs frames back to back and FIFO waits for vsync. The pacer only measures there, against the same target, from each frame's start time.
7. **Stats (rule R9):** `Platform::frame_pacing` gives the source, target, display refresh, this frame's interval and the counters. The sandbox's periodic log line and the overlay's stats panel show them.
8. **No `CADisableMinimumFrameDurationOnPhone`.** No tier runs above 60 fps. Adding the key, and a tier or setting at 120, is how a 120 fps option would come.

## Consequences

- **Thermal and power:** on ProMotion the panel can drop to the game's rate, and the main thread wakes once per frame rather than per refresh. §6.3's "offer a 30 fps lock" is a `set_frame_rate` call away.
- **Even starts, FIFO's showing.** Frames start on vsyncs at the target cadence. On the simulator (60 Hz display) the sandbox held 30.0 and 60.0 fps, with every interval in steady windows exactly 33.33 or 16.67 ms. That covered 600–1,500-frame runs, a trip to the background and a live tier change from 30 to 60. Startup and the tier change's renderer rebuild each cost one long frame. When each frame is shown is still FIFO's: on a panel running faster than the target, a frame whose GPU work ends before the next refresh shows early. The panel can now run at the target on ProMotion. A fixed 60 Hz iPhone at 30 fps (the Mobile-Low reference class) can't, and there a frame that finishes in under 16.7 ms shows a vsync early unless every frame stays on the same side of that mark. The device check in spec §6.2 measures this. If it shows, the fix is timed presentation: wgpu presenting at a time (or `CAMetalDisplayLink`), or a present gate that holds `present()` until one refresh before the frame's vsync.
- **Input latency is as before.** Events are pumped and dispatched on the same tick, just before the frame. A 30 fps tier now samples input every 33 ms, where SDL's link ran it at 60 (and missed the target).
- **SDL coupling:** three internal functions, the exact pin and the version assertion. Any SDL upgrade, patch releases included, is a deliberate step: it re-reads `SDL_main_callbacks.h`, `SDL_main_callbacks.c` and `SDL_sysmain_callbacks.m`, then moves the pin and the assertion together. If SDL ever exposes its iOS link's rate, this can go back to `SDL_EnterAppMainCallbacks`.
- **Android:** nothing changes now. Its pacing (spec §6.2: a limiter that divides the refresh evenly, `ANativeWindow_setFrameRate`) reuses the pacer with a vsync source of its own (Choreographer), or present timing once wgpu exposes it. The pacer already holds the target on the 90, 120 and 144 Hz panels common there.
- ADR 0002's consequence "iOS frame pacing is SDL's" is replaced by this record, and Q13 is closed in spec §11.
