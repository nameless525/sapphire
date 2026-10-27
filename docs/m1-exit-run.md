# The M1 exit run (X2)

M1 ends when its three exit tests pass ([engine-spec.md](engine-spec.md) §10):

1. Editing a ramp, a material parameter or a time-of-day key shows up on the desktop *and* a phone without a restart.
2. The look-dev scene meets the tier budgets on all five platforms.
3. Golden images pass on three backends.

This is the procedure for the first, and for the phone measurements the second needs. Everything except the phones was rehearsed on 2026-10-26 (the desktop, the iOS Simulator and the Android emulator; results at the end), so a phone run repeats a known procedure. The steps that only a device can prove are listed as such.

## What changes

The scene is X1's look-dev town from its hero shot at 10:00: `--look-dev hero --time-of-day 10`. Six edits, each made, pushed, checked, then undone, pushed and checked again:

| Kind | File | Edit | How it reaches the app | What to look for |
|---|---|---|---|---|
| Ramp | `content/samples/swatch.ramp.ron` | The `cloth` material's three `day` stops, blue to red (`#272a52` → `#5a1620`, `#4f5aa0` → `#b03040`, `#9aa8e0` → `#f0a0a8`) | cook, push the pack | The shirt's shadow side turns from blue to red |
| Character material | `content/lookdev/character/materials/Tops_01_CLOTH.material.ron` | `base_color: (1.0, 1.0, 1.0, 1.0)` → `(1.0, 0.55, 0.62, 1.0)` | cook, push the pack | The shirt turns rose |
| Environment material | `content/lookdev/town/materials/cobbles.material.ron` | `base_color: (1.0, 1.0, 1.0, 1.0)` → `(0.55, 0.75, 1.0, 1.0)` | cook, push the pack | The paving turns blue |
| Time-of-day key | `crates/sapphire_world/default_day.ron` | The 10:00 key's `exposure: 0.0` → `0.8` | push the document | The whole frame brightens |
| Cvar | `post.filter_strength` | `1` → `0` | set it | The grade's color filter goes |
| WGSL | `crates/sapphire_render/shaders/post.wgsl` | In `fs_composite`, `srgb_to_linear(display)` → `srgb_to_linear(display * vec3<f32>(1.0, 0.8, 0.8))` | push the source | The whole frame tints red |

**The ramp needs a setup edit.** The look-dev character has no ramps yet (X1 part 2 owes them), so the run first points the shirt at the sample ramp: in `Tops_01_CLOTH.material.ron`, `diffuse: Ramp((map: "1ace5ab9-aceb-42fa-bbcc-953141739d56", mix: 1.0))`, the region's `ramp_row: Some(1)` (the ramp's `cloth` row) and `light_split`'s `threshold: 0.7`, so the shadow side, where a ramp shows, covers most of the shirt. Push that, then edit the ramp. Once the character has ramps of her own, edit those instead.

**After X1 part 2a** the look-dev stage runs on its own day, `content/lookdev/lookdev_day.ron`: edit a key of that file (the one at or nearest the shot's hour) instead of the default day's, and give it to `watch` by name (`cargo xtask link watch content/lookdev/lookdev_day.ron`).

**Cooking after editing a compiled-in source is slow once.** `sapphire_pipeline` links `sapphire_render` and `sapphire_world`, which compile the WGSL sources and the default day in, so the first `cargo xtask content` after editing either rebuilds the pipeline (26 to 63 s in the rehearsal, on a loaded machine). Make the content edits first, or expect one slow cook. Every other cook took 0.5 to 0.8 s.

The rehearsal also edited the character's own shader, `anime_character.wgsl`, tinting the opaque pass's color before it is fogged (`fogged(shaded.color * vec3<f32>(1.0, 0.55, 0.55), …)`): the character alone turns red.

**After C7 lands** (Material 8, the new layers), re-run the steps it touches before counting them: both material edits and the ramp (the documents' fields and the ramp's binding may move, and the cooked materials change version, so every material recooks once), and the character shader edit, whose line C7's fragment stage restructures; pick the equivalent line in the new code. Build the app and the tool from the same commit, as always: an app and a tool of different protocol versions refuse each other (captures came with version 2).

A script made the edits, pushed, captured and checked each, and always undid them, for the rehearsal; the procedure below is the same by hand.

## Before any device

```bash
cargo xtask content --cook mobile    # also `cargo xtask content` (desktop) for the desktop leg
cargo xtask link help                # builds the tool
```

Push only what the device's cook holds: a phone reads `target/content/mobile/shared.spack`, the desktop `target/content/desktop/shared.spack`. `cargo xtask link status` names the device's cook, and `cargo xtask link push` with no files pushes every pack of that cook.

## Desktop, headless

The windowed sandbox takes focus, so the desktop leg runs offscreen: `--offscreen` (`dev` builds) draws the scene at 60 frames a second without a window until it is stopped (Ctrl-C or `kill`), with hot reload and live link on.

```bash
cargo xtask run --platform desktop --release --features dev -- --look-dev hero --time-of-day 10 --offscreen --size 1280x720
```

In another terminal:

```bash
cargo xtask link capture before.png
# make an edit; for content, then:
cargo xtask content
cargo xtask link capture after.png
```

Hot reload (A8) finds every edit itself: documents and WGSL sources when they are saved, content when `cargo xtask content` rewrites the pack. To exercise the phones' path on the desktop instead, add `--no-hot-reload` and push each edit (`cargo xtask link push …`, `cargo xtask link cvar …`) as below.

## iOS Simulator

```bash
cargo xtask run --platform ios-sim --release --features dev -- --look-dev hero --time-of-day 10
```

The simulator shares the Mac's network, so the tool reaches the app at 127.0.0.1:7811 as on the desktop. Screenshots: `xcrun simctl io booted screenshot <png>` (portrait: it is the device's framebuffer) or `cargo xtask link capture <png>` (the frame as drawn). When done: `xcrun simctl terminate booted dev.sapphire.sandbox` and `xcrun simctl shutdown booted`.

## Android emulator and phones

```bash
cargo xtask run --platform android --release --features dev -- --look-dev hero --time-of-day 10 --live-link connect=127.0.0.1:7811
adb reverse tcp:7811 tcp:7811
cargo xtask link --listen 127.0.0.1:7811 status
```

The app dials the desktop through `adb reverse`, once a second until a tool listens, so each `--listen` command first waits up to a second for the dial. For many commands in a row, either keep one tool running (`cargo xtask link --listen 127.0.0.1:7811 watch`) or let the app listen and forward to it: drop `--live-link` (the app listens on its loopback), `adb forward tcp:7811 tcp:7811`, and use the tool without `--listen`. Screenshots: `adb exec-out screencap -p > <png>` or `cargo xtask link capture <png>`. When done: `adb emu kill` (the emulator), or `adb shell am force-stop dev.sapphire.sandbox` (a phone).

`cargo xtask run --platform android` uses the connected device, so a phone needs only USB debugging on (Developer options) and the computer accepted on it; `adb devices` lists it. With an emulator running too, set `ANDROID_SERIAL` to the phone's serial.

## iPhone

What the user provides, once:

1. **An Apple ID signed in to Xcode** (Settings > Accounts). A free personal team works; its profiles last 7 days.
2. **The iPhone paired and in Developer Mode:** connect it, accept the trust prompt, then Settings > Privacy & Security > Developer Mode, and restart it.
3. **A development provisioning profile** for the bundle identifier `dev.sapphire.sandbox` that lists the phone. Without an Xcode project, the shortest way is a throwaway one: in Xcode, File > New > Project > iOS App, bundle identifier `dev.sapphire.sandbox`, the personal team, the phone as the run destination, then Product > Build. Xcode registers the phone and writes the profile to `~/Library/Developer/Xcode/UserData/Provisioning Profiles/` (older Xcodes: `~/Library/MobileDevice/Provisioning Profiles/`). If Xcode says the identifier is taken, use one of your own (`dev.<you>.sapphire`) and set `bundle-id` to it in `apps/sapphire_sandbox/Cargo.toml` for the run, without committing it.
4. **The signing identity's name:** `security find-identity -v -p codesigning` lists it, as `Apple Development: <name> (<id>)`.
5. **`iproxy`** for USB forwarding: `brew install libimobiledevice`.

Then:

```bash
xcrun devicectl list devices                     # the phone's name or identifier
cargo xtask run --platform ios --release --features dev \
    --identity "Apple Development: <name> (<id>)" \
    --profile ~/Library/Developer/Xcode/UserData/Provisioning\ Profiles/<uuid>.mobileprovision \
    --device "<phone>" -- --look-dev hero --time-of-day 10
```

The first launch fails until the developer is trusted on the phone: Settings > General > VPN & Device Management. In another terminal:

```bash
iproxy 7811:7811      # older libimobiledevice: iproxy 7811 7811
cargo xtask link status
```

`iproxy` forwards the Mac's port 7811 to the phone's loopback, where the app listens. From here the steps are the desktop's: cook `mobile`, push, `cargo xtask link capture <png>`. There is no command-line screenshot for an iOS 17 or later phone in Xcode's tools, so captures come over live link. `cargo xtask bundle --platform ios` without `--identity` and `--profile` builds and signs ad hoc, which checks everything but the signing.

Over Wi-Fi instead: `--live-link 0.0.0.0:7811` on the app, `--addr <phone's address>:7811` on the tool. That opens the app to the network (logged as a warning); use it on trusted networks only.

## What to measure and record

For each target and each of the six kinds, edit and undo:

- **Push → reply:** the tool prints `(in N ms)`, from the request to the device's answer. A pack is answered once its changed assets are swapped in; a document, cvar or shader once applied. The next frame shows it.
- **Edit → visible:** for content, the cook's time plus the push's. On the desktop with hot reload, the app logs `pack changed`, `asset swapped in`, `shaders reloaded` and `default day reloaded` with timestamps.
- **That it shows, and goes:** a capture before, after the edit and after the undo. The undo's capture must match the first (the rehearsal compared 640-pixel-wide reductions: the share of pixels more than 6 code values apart, against a frame-to-frame noise of 0.01% to 1.4%).
- **The cost of a shader push:** the app logs `live link rebuilt shaders … elapsed=`. Pipelines are rebuilt on the frame's thread, so a push holds the frame that long: 2.1 to 2.6 s for a cold compile on the simulator and 2.6 to 3.5 s on the emulator, which the frame stats show as one long frame. Record it on the phones; it is the one push that hitches.
- **Frame pacing and budgets** (spec §3): the tier the device gets (the `quality tier` log line); the `frame stats` log line every two seconds (fps, best and worst frame, on-time and missed display-link ticks on iOS); and the overlay's stats panel for CPU stages and GPU passes against §3.2 and §3.3 (`--debug-overlay=stats`, then `cargo xtask link capture`). Mobile-Low holds 30 fps with a GPU frame of at most 26 ms; Mobile-High 60 fps and 13 ms. Measure again after 15 minutes, throttled (spec §3).
- **Pushed bytes:** `sent N of M` per pack; a material edit sends a few hundred bytes, a ramp edit about 3.4 KB.

Record per device: model, OS, the tier, the build's commit, and for each kind the push → reply time (edit and undo), the cook time, the before / edit / undo captures and anything refused.

## Only a device can prove

- **iPhone:** signing with a real identity and profile; `devicectl` installing and launching, the app receiving its arguments after `--` and its console reaching the terminal; `iproxy` reaching the app's loopback listener; a capture from an A-series GPU's surface (BGRA, as on the simulator); Wi-Fi's local network prompt, if any.
- **Android phone:** a real Vulkan driver's (Adreno, Mali) pipeline compile times for a pushed shader; the surface's format, which captures convert from; `adb reverse` on a USB-connected phone rather than the emulator.
- **Both:** real frame times and GPU pass timings against the Mobile tiers' budgets, the tier a real device's memory selects, and how long a pushed shader holds a frame.

## Rehearsal (2026-10-26, main at dd118d1 with the rehearsal's changes)

All six kinds showed and went away again on every target, with the captures and the device screenshots agreeing on the iOS Simulator and the Android emulator. Push → reply in ms, for the edit and its undo (a cold shader compile, then a warm one):

| Kind | Desktop, live link | iOS Simulator | Android emulator |
|---|---|---|---|
| Cvar | 15 / 9 | 13 / 8 | 17 / 18 |
| Time-of-day key | 26 / 34 | 19 / 50 | 31 / 19 |
| WGSL (`post.wgsl`) | 2,436 / 44 | 2,605 / 32 | 3,554 / 102 |
| WGSL (`anime_character.wgsl`) | 1,666 / 85 | 2,072 / 74 | 2,572 / 138 |
| Character material | 51 / 35 | 44 / 46 | 28 / 48 |
| Environment material | 28 / 28 | 42 / 68 | 38 / 55 |
| Ramp | 44 / 45 | 38 / 39 | 49 / 41 |

Content took a cook of 0.5 to 0.8 s first (after the one slow cook described above). On the desktop with hot reload instead of live link, from saving the file to the change applied, by the app's log: the time-of-day key 0.12 to 0.30 s, WGSL 0.13 to 0.14 s (Metal's shader cache warm from the live link run), and content 0.66 to 0.79 s including the cook, the swap coming 0.12 to 0.14 s after the pack was written (the watcher's 100 ms debounce, the remount and the upload). The desktop ran the release `dev` build offscreen at 1280×720 on Desktop-High; the simulator (iPhone 17 Pro, iOS 26.3) on Mobile-Low at 2622×1206, holding 30 fps but for the shader pushes; the emulator (`sapphire_api36`, Android 16, `-gpu host`: the Mac's GPU through MoltenVK, so its compile times say nothing about a phone's driver) on Mobile-Low at 2400×1080. On the emulator, where the tool listens and the app dials, each command also waited up to a second for the dial (wall times of 0.1 to 1 s around the replies above). A pushed pack was swapped in about 30 ms after it arrived on every target, and a capture's PNG (2 MB or so) took 0.4 to 1.2 s to come back.
