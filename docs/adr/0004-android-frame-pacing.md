# ADR 0004: Android frame pacing (M1 F8)

- **Status:** accepted (M1). The phone check in spec §6.2 is still to run.
- **Date:** 2026-10-27

## Context

Spec §6.2 plans Android's pacing as FIFO plus a frame limiter that divides the refresh rate evenly (30 fps on 120 Hz presents every fourth vsync), a refresh-rate hint through `ANativeWindow_setFrameRate` (API 30+), and no Swappy. F7 (ADR 0003) built iOS's display-link pacing on a pure pacer and left Android measuring only. Mobile-Low's budget (spec §3) assumes 30 fps, and M1's exit test measures it on phones.

What SDL 3.4.18, the version `Cargo.lock` builds, does on Android:

- **SDL's loop runs frames back to back.** SDLActivity's SDL thread calls `SDL_main`, which enters `SDL_EnterAppMainCallbacks`: on Android that is SDL's generic loop (`src/main/generic/SDL_sysmain_callbacks.c`), which calls `SDL_IterateMainCallbacks(true)` (pump events, dispatch them, run `SDL_AppIterate`) again as soon as it returns. FIFO presentation, waiting for a free swapchain image, is the only brake, so frames run at the panel's rate whatever the tier wants. On the emulator's 60 Hz display the Mobile-Low sandbox ran at 49 to 60 fps at the base (91c6cb6), about 100 frames per 2-second window coming before they were due. A 120 Hz phone would run it at up to 120.
- **`SDL_HINT_MAIN_CALLBACK_RATE`** makes that loop sleep to a deadline on the clock (`SDL_DelayPrecise`). The deadline has no relation to vsync, so frames beat against the panel, and nothing asks the panel to slow down.
- **A paused app blocks inside SDL's event pump.** `Android_PumpEvents` waits on the lifecycle queue until the app resumes (`SDL_HINT_ANDROID_BLOCK_ON_PAUSE`, on by default), inside whichever iteration's pump saw the pause.
- **SDL's display mode** carries `Display.getRefreshRate()` as it was when the surface last changed size, and nothing updates it when the rate changes later.
- **SDL's thread has no looper.** It is a plain Java thread, and SDL's own loopers (its sensor thread's) live on threads of their own.

What the NDK offers at the app's minimum, API 29: `AChoreographer_postFrameCallback64` reports a vsync's time (on `CLOCK_MONOTONIC`) to a callback on the calling thread's looper. From API 30 there are `ANativeWindow_setFrameRate` and `AChoreographer_registerRefreshRateCallback`, which reports the vsync period when it changes (and, on the emulator's Android 16, once on registration).

The candidates:

1. **SDL's callback-rate hint.** It sleeps on the clock rather than to a vsync, so frames drift against the panel, and a 120 Hz panel stays at 120 Hz.
2. **The choreographer on a thread of its own, handing vsyncs to the SDL thread** (Swappy's design). That thread wakes at the display's rate, or needs start and stop logic, and every frame crosses threads.
3. **The choreographer on the SDL thread's own looper.** The SDL thread posts one frame callback and sleeps in its looper until the callback reports the vsync.
4. **Waiting for the vsync inside `App::iterate`,** under SDL's own loop. Events would then be pumped before the wait, up to a frame interval before the frame that handles them.
5. **Swappy.** It wraps the swapchain's present (and `VK_GOOGLE_display_timing`) on a swapchain it can see. wgpu owns the swapchain, so the spec already ruled it out.
6. **Present timing** (`VK_GOOGLE_display_timing`). wgpu 30 doesn't expose it.

## Decision

1. **The engine owns Android's loop (option 3),** as it owns iOS's frame clock (ADR 0003). `android_loop.rs` enters SDL's main callbacks through `SDL_InitMainCallbacks`, then loops: wait for a vsync, then `SDL_IterateMainCallbacks(true)`. It ends with `SDL_QuitMainCallbacks`. That is SDL's generic entry with a vsync wait in place of the rate hint's sleep.
   - Event dispatch, lifecycle events and quitting stay SDL's code.
   - These are the SDL internals ADR 0003 already relies on, behind the same exact `sdl3-sys` pin. A second compile-time assertion that SDL is exactly 3.4.18 names the files an upgrade must re-read for Android: the generic entry and the pause handling in `SDL_androidevents.c`.
2. **The vsync source is the choreographer on the SDL thread's own looper.** The loop prepares a looper on SDLActivity's SDL thread and takes the thread's `AChoreographer`.
   - Each wait posts one frame callback (unless one is already posted) and polls the looper until it reports the vsync, so the thread sleeps between vsyncs and never spins. Nothing crosses threads, and no thread wakes at the display's rate while a frame's work runs.
   - After 100 ms without a vsync the loop iterates on the clock (`CLOCK_MONOTONIC`, the choreographer's), so SDL's events, and the app, keep going should vsyncs stop. Ticks only ever move forward: a vsync no later than the last tick is dropped.
3. **The same pacer gates the vsyncs** (`PacingSource::Choreographer`). Every vsync is a tick: SDL pumps and dispatches events, and the frame clock asks the pacer whether the vsync starts a frame. A vsync before the frame is due is held back. Frames are due on whole refreshes, as on iOS, so the target holds and is never exceeded on 60, 90, 120 and 144 Hz panels. Events are pumped on the vsync that starts the frame, just before it.
4. **The refresh comes from the choreographer** where it reports one (API 30+), ahead of SDL's display mode, which stays the fallback (API 29, or before the first report).
   - The reports lag the vsyncs. In particular a lifted frame-rate override (the app's vsyncs going from 30 back to 60 Hz) is reported after the vsyncs change, and a vsync taken for two refreshes when it is one would start a frame early.
   - So under choreographer pacing, a tick less than three quarters of the reported refresh after the tick before divides that refresh by the whole number nearest the ratio (2 to 8) until the next report. A refresh shorter than the panel's is safe, whole vsyncs being whole multiples of it. A tick more than eight times sooner can only be one off the vsyncs (the loop's clock tick after a timeout) and changes nothing, so the refresh never falls below an eighth of the reported one.
5. **The refresh-rate hint.** Every native window a `SurfaceHandle` holds is asked for the target rate with `ANativeWindow_setFrameRate`, resolved at run time, with the default compatibility (games and UI) and so its default strategy (switch only if seamless).
   - It applies when the target changes (at startup and on every tier change, through `Platform::set_frame_rate`) and to each new native window: Android hands the app a new one after every trip to the background. A `None` target asks for the system's choice again (0 fps).
   - On a panel that can't switch, Android 12 and later may throttle the app's vsyncs to the hinted rate instead, a frame-rate override. The emulator's 60 Hz display did exactly that for a 30 fps tier: the app's vsyncs came at 30 Hz, so the limiter held nothing back.
6. **Coming back from the background.** SDL blocks in its event pump while the app is paused, inside an iteration whose vsync the loop has already taken. When the app is back (`DidEnterForeground`), the frame clock drops that vsync, and the next vsync starts the first frame back, with one target interval.
7. **The frame's time is the time between the frames' vsyncs:** `Platform::frame_delta` on Android, as on iOS, for the fixed step and animation. On Android these are the vsyncs frames start on rather than the ones they are shown at (`AChoreographer_postVsyncCallback`'s expected presentation times need API 33).
8. **Desktop is unchanged.** SDL's loop runs frames back to back, FIFO paces them, and the pacer only measures.
9. **Stats (rule R9):** `Platform::frame_pacing` reports the choreographer as the source, the reported refresh, and the counters (on time, missed, doubled, held back). The sandbox's `frame stats` line and the overlay's Frame pacing rows show them.

## Consequences

- **On the emulator** (`sapphire_api36`, Android 16, 60 Hz display, `-gpu host`), the Mobile-Low sandbox holds 30.0 fps with every interval exactly 33.33 ms in steady windows, against 49 to 60 fps at the base. Mobile-High holds 60.0 fps at 16.67 ms in the windows the emulator doesn't stall. Spec §6.2 has the log lines.
- **No running ahead.** With the target at the panel's rate (60 fps at 60 Hz), frames start on vsyncs instead of queueing up to the swapchain's depth ahead of the display, so input reaches the screen sooner, but a frame that overruns its refresh repeats one where the queue could have absorbed it. On the emulator, whose stalls run to 40 to 115 ms, the base reached 60.0 fps in 8 of 14 windows at Mobile-High and F8 in 5; a phone check decides whether a target at the panel's rate should let FIFO run ahead instead.
- **Power:** a 30 fps tier draws 30 frames a second, and the SDL thread sleeps until each vsync. A held vsync costs one wake and one event pump. With the hint honoured (a panel that drops to the rate, or a frame-rate override), none are held.
- **Input latency:** events are pumped on the vsync that starts the frame, as on iOS. A 30 fps tier samples input every 33 ms instead of every presented frame at the panel's rate.
- **Even starts, FIFO's showing.** Frames start on vsyncs at the target cadence; each is shown at the first vsync after its GPU work. On a panel running faster than the target, a frame that finishes early shows early. That is ADR 0003's consequence too. The fix is timed presentation (`VK_GOOGLE_display_timing`) once wgpu exposes it.
- **Android 10 (API 29)** gets the limiter, but neither the hint nor the refresh reports. The refresh is SDL's (the rate when the surface last changed size), with the faster-vsync guard above.
- **SDL coupling:** the internals are ADR 0003's, and an SDL upgrade now re-reads two entries (iOS's and the generic one) and Android's pause handling.
- **What only a phone can prove** (spec §6.2): a panel faster than 60 Hz dropping to the hinted rate, its refresh reports, 90 and 120 Hz cadences on real vsyncs (the unit tests cover them), and real GPU frame times against the budgets.
- ADR 0003's consequence "Android: nothing changes now" is replaced by this record.
