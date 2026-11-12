# M5 handoff notes

What M5 inherits from M4, for whoever starts it. Like [m4-handoff.md](m4-handoff.md), it's a map. Each item gets a line or two and a pointer to where the detail lives: the spec ([engine-spec.md](engine-spec.md)) by section, the plan ([m4-plan.md](m4-plan.md)) by package ID, the runbook ([m4-exit-run.md](m4-exit-run.md)), the store checklists ([docs/store/](store/)), or the code.

**Written on 2026-11-11** (M4 Z4, part 1) on the landing line 2d18c9b8: main at d8b621cc with X3, R1, T4, Z3a and Z3b on it. Z3c's pushed branch was read too, and its fixes count as landing with the line. H4, Z1, Z3d, Z3e and Z2's final exit run were still to come, so this part leaves a marked slot for each.

TODO(Z4 part 2): the head the final exit run took, its date, and that every package above has landed on `main`; then drop "part 1" and the line above it.

**Spec §10 names no milestone after M4.** What M5 is, a second slice, a content push, or the off-ramp, is the maintainer's to choose. This file is what any of them starts from, and its "Before a first upload" is what the maintainer acts on first if M5 is a release.

**Nobody has played anything.** As of 2026-11-11 the maintainer hasn't played M2's fight, M3's slice or M4's packaged game, so review 3 from [m2-exit-run.md](m2-exit-run.md) has never run. M4 made a playtest cheap (F1, F2) and packaged the game for three stores, and whether any of it is fun is still unknown. It is still the largest risk in the project.

## Where M4 stands

M4 took M3's game, the Lantern Ward from the title to the Knell Colossus, and made it something that could leave this machine. It added no content and tuned no fight. The game now boots behind a screen that draws while its pipelines warm, patches itself from a signed release, steps down a thermal ladder, keeps its symbols, has the accessibility settings action games are asked for, and builds a store package for iOS, Android, Windows, Linux and macOS. A playtest build keeps every run as a replay.

| Exit test ([m4-plan.md](m4-plan.md) §1) | Where it stands |
|---|---|
| 1. No shader hitches | Every pipeline builds on a warm-up pool no frame waits on (H1). The game boots behind a screen of one pipeline while the rest warm (H2). `declares_every_setting` walks every setting at every in-place rung of every tier with no frame-thread or first-use build (H3). Before H4 the boot's longest frame was the fight's stand, 124 ms on cold Metal against the 33 ms allowed. TODO(Z4 part 2): H4's result: whether `zero_hitches` passes in CI on lavapipe and on cold Metal here, the worst load-screen frame against twice the tier's interval, and what became of the stand frame and of P4's 100–250 ms remake under the boot screen. |
| 2. Patches download only changed chunks | Proved on the desktop, the Simulator and the emulator against a server on this Mac (P4). A screen's edit moved 4,384 bytes: the signed manifest, one pack's header and table, and one range of blocks. A killed download resumed, and a release from an untrusted key was refused. `cargo xtask patch rehearse` is the lane. Owed: a CDN, the maintainer's key, TLS against a real certificate, and a phone. |
| 3. Builds pass the stores | Every platform's package builds and passes every validator that runs without an account (S1–S4, C1). The nightly `packages.yml` ran all five jobs green for the first time on 2026-11-11 (run 43002018202). The checklists count Apple 22 of 32 rules met, Play 20 of 29 and Steam 21 of 30 (Z3b). The rest is the maintainer's accounts, identity, art and the stores' reviews ("Before a first upload"). |
| 4. The look at every tier, scale and rung | 12 `tier-*` and 16 `tier-*-rung-*` goldens, and the GPU test `look_tiers` holding lines and the two-tone edge to each floor (T0, T2). The ladder walks down and back on modelled traces and under the emulator's thermal override, and the game follows it, changing tier only at a pause or a wipe (T3). Known desktop GPUs are capped by a table (T4). TODO(Z4 part 2): Z3e's review of warm-up and tiers. Owed: a phone, throttled, after 15 minutes. |
| 5. Every crash is readable | A forced crash in a shipping build names its function, file and line on macOS, the Simulator and the emulator, from symbols kept from the same link, and every shipped binary is stripped with its ID checked against them (C1, C2). Windows and Linux are proved by a crash test on each CI desktop, not by their packages. TODO(Z4 part 2): Z3d's final fixes, and the emulator run for the Android fault address it fixed. |
| 6. Combat more people can play | Every item of §3's list is a setting, saved and migrated (`PlayerSettings` version 4) and reached by pad, keyboard and touch (A1–A6). With flashes reduced and by default, the scripted fight passes WCAG 2.3.1 (A3, held by a test since Z3c). The smallest text is 14.0 px at 1280×800 by default, in both languages. Warnings are marks of their own shape, held apart under three dichromacies (A5). At the largest size on a phone, every screen fits (Z3c). Owed: players who need these settings, and a reader of Japanese. |
| 7. Still authorable | The golden, scripted-input, shelved-zone and placeholder-sound steps are documents now (R1–R4). Two Rust steps remain: a screen showing a value no model holds, which is code by design, and the glyph a new remappable binding shows (A4), which the plan's "but one" missed ("Rust steps left"). |
| 8. Ready to play | `cargo xtask playtest` installs a playtest build, and `playtest pull` brings sessions back and replays each run to the device's checksums: on the desktop, a private Simulator and the emulator (F1). `playtest report` reads a session out per encounter (F2), against the review sheet in [playtest.md](playtest.md). Owed: someone playing, and `--platform ios` on a phone. |

TODO(Z4 part 2): Z1's shipping benches, from spec §3's "Measured as shipped", which Z1 writes: binary sizes, cold and warm starts to the title on Metal, the Simulator and the emulator, warm-up per tier, memory, patch sizes for P0's edits and the ladder's trace under the override. And Z2's figures for each exit test from [m4-exit-run.md](m4-exit-run.md)'s final run.

**The reviews (Z3a–Z3e)** read five areas after they landed, as M3's did:

- **The patcher, as an attacker would (Z3a):** two fixes. A host that dribbled a byte inside every read timeout held the boot open as long as it liked, and `ureq` followed an `https://` host's redirect to plain `http://`. The signature, rollback and every cap held up. Five small gaps stay (see "Patches").
- **Packaging, against the checklists (Z3b):** the settings got a privacy policy page, Valve's pads their own glyphs, and Android's `INTERNET` permission its reason. Every met rule's test was found and read against the code.
- **Accessibility, against exit test 6 (Z3c):** nine fixes and holds. A settings file could remap the pause, the way out or a stick. The large interface size ran out of height on a phone for the pause, the character screen and the dialogue. A toggle didn't let go across a pause as a hold does. The flash test of the scripted fight is now a test, though no CI lane runs it yet.
- **Crash capture (Z3d):** TODO(Z4 part 2): its final commits. Its pushed branch (6197f5a7) fixes seven things: ELF leaves with no line, a minidump frame's inlined calls listed outermost first, Linux and Android fault addresses read as 0, a signal record's address read in the wrong file, a report naming another build of a shipped file without saying so, panic reports naming modules by full path, and the player's environment in Linux minidumps.
- **Warm-up and tiers (Z3e):** TODO(Z4 part 2): what it fixed and what it left, after H4 and T4.

What each left is under "Everything deferred", and what binds now is under "Rules the reviews made".

## The stores' next deadlines

Each was read on 2026-11-11 at the source named, unless it says otherwise. "Expected" is our inference from a store's yearly pattern, not an announcement.

| Store | Rule | Date | Source | Where the game stands |
|---|---|---|---|---|
| Google Play | New apps and updates target API 36 | Since 2026-08-31; extensions ran to 2026-11-01, now past | developer.android.com/google/play/requirements/target-sdk; Play Console Help answer/11926878 | Met: `targetSdk = 36`, read back by `package`'s badging check (S3) |
| Google Play | API 37 | Not announced on either page above. Expected about 2027-08-31, a level each August (36 in 2026; 35 in 2025 and 34 in 2024 from memory, not re-read). Third-party summaries say Google's Android 17 beta post (2026-02) named August 2027; that post, read here, doesn't (unverified) | as above; android-developers.googleblog.com, "The first beta of Android 17" | Owed by mid-2027: raise `targetSdk` in `platform/android/app/build.gradle.kts` and run S3's emulator checks again (Back, edge to edge). Android 17 drops the opt-out from large screens' orientation and resizing rules at target 37, and exempts games, which `android:appCategory="game"` declares (the same post) |
| Google Play | Updates without 16 KB page support can't be released | 2027-02-01 | developer.android.com/guide/practices/page-sizes | Met: `package` checks every `LOAD` segment and `zipalign -c -P 16` (S3) |
| Google Play | A new personal account runs a closed test of 12 testers for 14 days before Production | Lead time, not a date | answer/14151465, read 2026-11-10 (S5) | Owed: the maintainer's testers. Start it at least 14 days before a release |
| App Store | Uploads built with Xcode 26 and an iOS 26 SDK | Since 2026-04-28 | developer.apple.com/news/upcoming-requirements | Met with Xcode 26.2: `package` writes the `DT*` keys and refuses another SDK (S2) |
| App Store | Xcode 27 and an iOS 27 SDK | Not announced. Expected about April 2027, as Xcode 26's came on 2026-04-28 and, from memory and not re-read, Xcode 16's on 2025-04-24 and Xcode 15's on 2024-04-29 | as above | Owed by then: build on the new Xcode and check S2's archive keys again |
| App Store | iOS and iPadOS apps target iOS 13 or later | Since 2026-09-09 | as above | Met: `MinimumOSVersion` 17.0 (S1) |
| App Store | Answers to the new age rating questions | Since 2026-01-31 | as above | Drafted in [apple.md](store/apple.md#age-rating); the maintainer enters them |
| App Store | Trader status for the EU (Digital Services Act): required to submit since 2024-10-16, and apps without it removed from the EU since 2025-02-17 | In force | as above | Owed: the maintainer declares it in App Store Connect, or leaves the EU's storefronts out. Not in [apple.md](store/apple.md)'s rules, which predate this read |
| App Store | Accessibility Nutrition Labels: voluntary, required "over time" | No date | apple.md rule 22, read 2026-11-10 | Drafted; three features claimed, each held by a test |
| Steam | Store page review 3–5 business days; submit at least 7 business days before it goes live. Build review 3–5 business days; plan for at least 7 | Standing | partner.steamgames.com/doc/store/review_process | Owed: the maintainer's submissions |
| Steam | Pre-release build reviews take up to 12 business days, and a new pre-release checklist: at least five minutes played through Steam's Play button, the redistributables Steam installs named (or none), achievements and Steam Cloud complete if listed, and five screenshots tagged as gameplay | Announced by Valve on 2026-11-03 | Inven Global's report of Valve's Steamworks Development post (Valve's post wasn't read; its review page above doesn't mention the change yet) | Plan for 12 business days. The five minutes need the maintainer's account. The redistributables need checking: see "Windows' C runtime" under "Packages and stores" |
| Steam | Steam Linux Runtime 3.0 (sniper) for the native Linux build | No deadline; a 4.0 exists | [steam.md](store/steam.md) rule 15 (S4, 2026-11-10); 4.0 from S4's hand-back | Built and checked against 3.0 in CI; choosing the runtime in Steamworks is the maintainer's |

The checklists ([apple.md](store/apple.md), [google-play.md](store/google-play.md), [steam.md](store/steam.md)) keep each rule with its source. Read this table again before each upload: the stores move every year.

## Before a first upload

Everything here needs the maintainer's accounts, keys or judgement, which no agent may use (m4-plan §3, "Accounts and publishing"). The steps are in order. Package on `main`, in a full clone with no changes: off `main`, `package` stops unless given `--dev`, and a dev build's number is marked as one.

### For every store

1. **Choose the identity.** In `platform/identity.toml`, set `game_app`'s `bundle-id` (permanent once a store has seen it), `bundle-name` and display names in both languages, and delete `placeholder = true`. Replace `platform/icon/icon.png`, the master every store's icon is made from by `package`, if the generated one won't do (S1). Put the game's name into the store drafts in [docs/store/](store/).
2. **Host the privacy policy.** Fill in [privacy-policy.md](store/privacy-policy.md)'s bracketed names and host it at a public URL. Set that URL as `privacy-url` in `content/loc/en/game_ui.ftl` and `content/loc/ja/game_ui.ftl`; once the identity is chosen, `package` refuses the placeholder address. "Collects nothing" holds only if the patch host keeps no access logs that identify a device: choose the host with that in mind.
3. **Make a release key and choose a patch host,** or ship a build that can never be patched: a build trusts only the keys and host compiled into it.
   - The key is a 32-byte ed25519 seed. Keep it offline, never in the repository or an environment file:
     ```bash
     openssl rand -hex 32 > release-key.hex
     printf '302e020100300506032b657004220420%s' "$(cat release-key.hex)" | xxd -r -p \
       | openssl pkey -inform DER -pubout -outform DER | tail -c 32 | xxd -p -c 32
     ```
     The second command prints its public half. It needs OpenSSL 3 or later: macOS's own `/usr/bin/openssl` (LibreSSL 3.3.6) can't read the key. Checked here with OpenSSL 4.0.3 on the public test seed, where it printed `sapphire_patch::TEST_KEY`'s public half.
   - Put the public half in `platform/patch.toml`'s `release-keys`, and the host's `https://` URL, ending in `/`, in its `host`.
   - Sign the first release with the package: `SAPPHIRE_RELEASE_KEY="$(cat release-key.hex)" cargo xtask package --platform <platform> --release-version 1 --expires <YYYY-MM-DD> --min-build <the build number>`. Copy the package's `release/` to the host, `release.smanifest` last. A patch later is the "A patch" recipe below.
   - Mind each release's expiry: a game takes a release only before it expires, so one left on the host past its date stops reaching anyone who hasn't taken it yet. Publish the next release before then.
4. **Read [m4-plan.md](m4-plan.md) §3's decisions** that most need the maintainer's eye (§7, item 7): patches carry presentation only, no Xcode project, native Linux on the Deck, dynamic resolution on by default, the default flashes as they are with the reduced setting offered, and no CDN chosen.
5. **Confirm the VRM character's licence allows a commercial release** (S5's note). The other third-party content's provenance is in `content/third_party/README.md`.

### The App Store

1. **The account:** the Apple Developer Program, an Apple Distribution certificate, an App Store profile for the bundle ID, and the App Store Connect record, agreements, tax and banking. Trader status for the EU, or no EU storefronts.
2. **Export compliance.** The game links `rustls` on `ring` for HTTPS to the patch host (P4), and `ITSAppUsesNonExemptEncryption` stays `false` in `platform/ios/Info.plist`, which claims the exemption; that file's comment still says the game links no encryption, which was true before P4. Confirm it on Apple's encryption page and in App Store Connect's questions, or change the key. The liability for the claim is the developer's ([apple.md](store/apple.md#owed-to-the-maintainer), item 3).
3. **Package:** `cargo xtask package --platform ios` on a Mac with Xcode 26. It writes `target/package/ios/<bundle-name>.xcarchive` and its checks.
4. **Sign and upload:** add `<key>teamID</key><string>YOUR TEAM ID</string>` to `platform/ios/ExportOptions.plist`, then
   ```bash
   xcodebuild -exportArchive -archivePath target/package/ios/<bundle-name>.xcarchive \
     -exportOptionsPlist platform/ios/ExportOptions.plist -exportPath target/package/ios/export \
     -allowProvisioningUpdates
   ```
   and upload the `.ipa` with Transporter, or distribute the archive from Xcode's Organizer. S2 saw Xcode run eight of its distribution steps and stop in the ninth, signing, so what Xcode checks after signing is unseen.
5. **App Store Connect's forms,** from [apple.md](store/apple.md)'s drafts: "Data Not Collected", the age rating (13+ expected), the accessibility labels, the store text, the privacy URL, and screenshots from `cargo xtask store-shots` (`target/store-shots/apple/`), picked by eye. A person checks the shots suit a 4+ rating (Guideline 2.3.8).
6. **TestFlight:** internal testers need no Beta App Review.

### Google Play

1. **The account:** Play Console, the app record, and Play App Signing.
2. **An upload key,** kept offline: `keytool -genkeypair -v -keystore upload.jks -alias upload -keyalg RSA -keysize 4096 -validity 10000`.
3. **Package** with JDK 17 as `JAVA_HOME`:
   ```bash
   SAPPHIRE_UPLOAD_KEYSTORE=<path>/upload.jks SAPPHIRE_UPLOAD_KEYSTORE_PASSWORD=<…> \
   SAPPHIRE_UPLOAD_KEY_ALIAS=upload SAPPHIRE_UPLOAD_KEY_PASSWORD=<…> \
     cargo xtask package --platform android
   ```
   Without the keystore it signs with the debug key and warns that Play refuses it. `package.json`'s certificate SHA-256 must equal the one Play Console's upload key page shows.
4. **Play Console's App content,** from [google-play.md](store/google-play.md)'s drafts: data safety, content rating, target audience (13 and over), ads (none) and app access, and the privacy URL. The listing needs a 1024×500 feature graphic, which is art nobody has made, and screenshots from `store-shots`.
5. **Testing:** upload the AAB to internal testing. A new personal account then runs a closed test of 12 testers for 14 days in a row before Production.

### Steam

1. **The account:** Steamworks, the app credit fee, tax and bank details. Put the real app and depot IDs in `platform/steam.toml` and delete `placeholder = true`.
2. **The packages, from one commit:** `cargo xtask package --platform macos` on a Mac. Windows and Linux need their hosts: take `package-windows` and `package-linux` from the nightly `packages.yml` run on that commit (kept 14 days), or dispatch it, and put them beside the Mac's under `target/package/`.
3. **macOS:** sign the package's app with a Developer ID, under the hardened runtime and the package's own entitlements, then notarize and staple (S0's commands, `research_notes/Ship readiness for M4/s0-spike.md`):
   ```bash
   codesign --force --sign "Developer ID Application: <name> (<TEAM>)" --options runtime --timestamp \
     --entitlements platform/macos/package.entitlements <bundle-name>.app
   ditto -c -k --keepParent <bundle-name>.app <bundle-name>.zip
   xcrun notarytool submit <bundle-name>.zip --keychain-profile <profile> --wait
   xcrun stapler staple <bundle-name>.app
   ```
4. **Upload:** `cargo xtask steampipe` writes the scripts into `target/package/steam/`; then `steamcmd +login <account> +run_app_build target/package/steam/app_build_<app>.vdf +quit`. In Steamworks: the client icon (the Windows package's `icons/app.ico`), each depot's launch option from `depots.txt`, Steam Linux Runtime 3.0 for Linux, and the redistributables Windows needs, if any.
5. **The store page,** from [steam.md](store/steam.md)'s drafts: the text, the languages, the content survey with its generative AI answer (the maintainer's), five or more gameplay screenshots, and the capsules, which are art nobody has made. Submit it at least 7 business days before release.
6. **Before the build's review:** play at least five minutes through Steam's Play button on each OS the page lists, which also checks the macOS overlay under the hardened runtime (steam.md rule 14). Then plan for 12 business days of review.
7. **A Deck** for its self-check (steam.md rules 25–27).

## Device checks still owed

Nothing here has been run, for want of a device or a person. [m4-exit-run.md](m4-exit-run.md) has M4's procedures, and the older runbooks theirs.

**M4's:**

- **A phone of each OS:** a phone driver's cold compile of the warm-up, and how long boot's screen holds it; a store install's first launch; the ladder under real heat, a real headroom forecast, what ADPF does to the clocks, and iOS's real thermal states and Low Power Mode (T1, T2); a patch over `https://` (P4); a crash, and the store's own crash report (C2); a real GPU loss and the OS killing the app in the background (S6); a finger on a real iPhone's HUD; Android's back gesture and SDL's safe area on a real cutout (S3); `xtask playtest --platform ios` and its `devicectl` pull, written and never run (F1).
- **A desktop GPU** other than this Mac's: a driver's cold cache (exit test 1), and any GPU in T4's table, none of which has run the game.
- **A Steam Deck:** SDL's pad type under Steam Input, the frame rate at 800p by default, and Valve's self-check (S4, T3).
- **A real display** for the desktop frame cap (T3).
- **Ears and hands:** the volumes, mono and scaled rumble, captions at a fight's pace (A2), and the timing assist's numbers in a fight (A6).
- **People:** players who use the accessibility settings, a reader of Japanese for the new strings, the notice and the store text, and someone to pick the store shots by eye (one shows a hit's heavy chromatic aberration, S5).

**Still agents' to run:** the emulator's cold boot under the boot screen (H1, H2: counts only, since the emulator slows any drawing frame during a build) and the emulator run of Z3d's Android fault address. TODO(Z4 part 2): drop either that H4, Z1 or Z2 ran.

**M3's, M2's and M1's, carried:** [m4-handoff.md](m4-handoff.md), "Device checks still owed", has each with its pointer. M4 settled only Windows' save retry, whose test CI's Windows job runs. The rest stand: review 3 and the slice played; motion judged and sound heard; a window on any desktop, and the editor's; a desktop's pointer, focus and gamepad; a phone for everything M1 to M3 built, and latency on glass; iOS touch; x86-64 Android's Luau; the emulator crash nobody cleared; and the open questions Q3, Q4, Q5's device half, Q9 and Q14. M4's handoff's two items "owed to the maintainer", export compliance and a patch on a phone, are in "Before a first upload" and "M4's" above.

## What M4 settled from its handoff

These items in [m4-handoff.md](m4-handoff.md) are closed and don't carry:

- **Rust steps in the recipes:** a golden is a row (R1), a scripted input a document (R2), a shelved zone found by the build (R3), and a placeholder sound a spec (R4).
- **CI near its limits** (I1): only `main` writes caches, App tests run in four lanes, and lavapipe's goldens in three shards.
- **The inventory's blocking warm-up** (H1, H2): builds run on a warm-up pool behind screens that draw. **Full PSO warm-up** with them.
- **The phone's missing Back, nothing pausing on return, and a lost device ending the run** (S6).
- **Windows' save retry,** a device check: its test (`a_rename_waits_out_a_file_held_for_a_moment`) runs in CI's Windows job on every pull request.
- **Store and signing** (track S), up to the accounts; **patching** (track P), from M3's "Not in M3".
- **Secondary motion's worst frame,** in part (H5): p99 fell to 0.72 ms on Desktop-Low, still over its 0.5 (see "Warm-up, tiers and frames").

## What M5 builds on

M3's systems are as [m4-handoff.md](m4-handoff.md)'s "What M4 builds on" has them: the combat stack, the game's flow, the data path, world and streaming, and the tools. M4 added the following. Each row of [m4-plan.md](m4-plan.md) §4 has the detail, and its spec section the design.

### Crates and layering

48 crates in `crates/`, four in `game/`, the sandbox and the tools, each placed in [xtask/layers.toml](../xtask/layers.toml). M4's new ones:

- **`sapphire_patch`** (L4): the download (P2) and the install (P3).
- **`sapphire_playtest`** (`dev_only`): a playtest's session on disk (F1). Only `game_app`'s `playtest` feature links it, so a store build can't reach it.
- **`tools/sapphire_symbols`:** checks stripped binaries against their symbols (C1), writes Breakpad `.sym` files, and reads crashes (C2).

**Kinds and versions.** No asset kind was added: `VoiceLines` 25 is still the last. `PlayerSettings` is version 4, `EnemyDocument` 2 and `TargetingTuning` 2. The replay format is still 4, with commands 5 (auto-target strength, A1) and 6 (timing assist, A6). `CHECKSUM_LAYOUT` is still 9 and `PACK_VERSION` 4. New documents: the release manifest (format 1), `GoldenScenes` 1, the GPU table 1, `SessionDocument` 1 and the playtest `Report` 1. The cooker's version is 3 (C3). There are 39 replays and 342 golden scenes.

### Boot, warm-up and hitches

- **The warm-up pool** (`sapphire_jobs::warm_up_pool`, H0, H1): `Renderer::warm` returns a `WarmUpRequest` and `prepare` takes finished work in. `PipelineStats` counts `frame_thread_builds`, which fail a bench, and `frame_waits`.
- **The boot** (`game_flow::boot`, H2, P4): `Step::Update`, then the renderer, the packs' and first zone's pipelines, the stage, the stand, the screens and the declared pipelines, under `BootScreen`, drawn by `InterfaceScreen`, the interface's one pipeline. Hitches count from its first frame.
- **Everything reachable is declared** (H3): `game_app`'s `declares_every_setting` reads tiers, rungs and settings from their documents. A pipeline a new setting switches fails it until declared.
- TODO(Z4 part 2): H4's `zero_hitches` lane: what it plays, where CI runs it, and what it changed in the stand.

### Releases and patches

- **The release manifest** (`sapphire_assets::manifest`, P1): ed25519-signed, with a version that only grows, an expiry, the minimum app build, each pack's header hash and each file outside the packs. A shipping build mounts only packs its manifest names.
- **`cargo xtask release`** lays a release out for a static host; `package` signs one with the build's options (P3). `Manifest::unpatchable_change` is the one rule for what a patch may not carry.
- **`Installation`** (P3) assembles a new release beside the current one and switches by one rename. **`game_flow::update`** (P4) checks, downloads and installs in the boot, and never blocks play. The keys and host are `platform/patch.toml`'s, both empty.

### Tiers, heat and the ladder

- **Heat** (T1): `Platform::thermal()` gives one `ThermalReading` a frame (Android's state, headroom and hint session; iOS's state and Low Power Mode). Cvars `thermal.*` force it.
- **The ladder** (T2): `quality::ladder::Ladder` over each tier's `ladder` in `tiers.ron`, with holds on the way back. Every rung sheds in place.
- **In the game** (T3): `game_app::tiers::TierFollower` takes heat each frame and changes tier in place, only while the show holds still. The player has quality, a frame cap, dynamic resolution and a battery saver. A Deck starts on Desktop-Low.
- **Known GPUs** (T4): `crates/sapphire_render/gpus.ron`, a ceiling per family that only lowers what `tiers.ron`'s rules pick.

### Settings and accessibility

- **`PlayerSettings` version 4** (`game/game_ui/src/menus/settings.rs`), on the settings screen and its display, graphics, sound, subtitles, controls and privacy pages, each a document and a row in `content/ui/menus.ron` (A1–A4, A6, T3, Z3b).
- **What they change** (A2, A3): `EffectScale` on post's screen effects, each bus's volume, mono, text and subtitle size. Reduced flashes holds a swing of 0.08. `sapphire_testkit flashes` is a WCAG 2.3.1 analyser.
- **Marks by shape** (A5): `EffectPlace::Crown` marks; `r.color_vision` shows the frame under a dichromacy.
- **Remapping and toggles** (A4, Z3c): `sapphire_controls::remap` and `remappable`; `Trigger::Toggle`.
- **The timing assist** (A6): `default_timing_assist.ron`, compiled in, in the run's tables only while it's on.

### Packages, symbols and crashes

- **`cargo xtask package --platform ios|ios-sim|android|macos|windows|linux`** (S1–S4, C1, C2): a shipping build with its cook, icon, symbols, `checks/` and `package.json`. The build number is the count of commits on `main`.
- **`cargo xtask steampipe`**, **`store-shots`**, **`symbols`** and **`symbolicate <report>…`**. A forced crash is `--crash fault|panic`, or on Android `adb shell setprop debug.sapphire.crash fault`.
- **`.github/workflows/packages.yml`** (I2) builds all five nightly at 11:43 UTC, on dispatch, and on pull requests touching what packaging reads.

### Playtests

`cargo xtask playtest --platform <p>`, `playtest pull` and `playtest report <session>` (F1, F2). [playtest.md](playtest.md) is the review sheet, each question pointing at the report's line and the document to change. `game_app --replay <file> --capture <png> --at-tick <n>` draws a mark's moment.

### CI

Pull requests run Lint, Test on three OSes, Replays, four App tests lanes (`walk-desktop-low`, `walk-mobile-low`, `wave-bench` and `the-rest`), the goldens on Metal, lavapipe and WARP, and the mobile builds (I1). Only `main` writes caches, and CI also runs nightly on `main`. H3's settings walk has 40 minutes a tier on lavapipe.

## How to add

M3's first pillar is that a new enemy, encounter, stage, boss, cutscene or screen is documents ([m3-plan.md](m3-plan.md) §2). The recipes below say where that holds. **A step that still edits Rust is marked "Rust step", and each is listed under "Everything deferred" as a bug against the pillar.**

Every recipe ends the same way: `cargo xtask content` to cook, `cargo xtask replay` for the replays, the sandbox's replay lane for the app, and a commit. Then come x86-64's checksums from CI's artifact for any replay that's new or moved ([development-iteration.md](development-iteration.md), "Filling in x86-64's checksums"), which costs two CI rounds.

**What's left of Rust:** two steps. A screen showing a value no model holds is code by design, and a new remappable binding's glyph is a field of `game_ui::menus::BindingGlyphs` (M4 A4; "A game screen", step 3). A golden is a row in `tests/golden/scenes.ron` and its PNG since M4 R1 ("A golden"). A zone a scenario shelves needs no row since M4 R3: the build finds it. A replay's scripted input is a document since M4 R2 (`content/inputs/`), and a placeholder sound is a spec since M4 R4 (`tools/sounds/`). No test pins the shipped Lantern Ward's content any more (M3 Z5e): the tests that held its counts and lists read them from its documents, so a checkpoint, an encounter's fighter, a cutscene or a conversation added to the ward or its run passes them with only `content/` changed. Such a change still moves the ward's replays, and a fight that plays differently needs `ward-whole-walk` recorded again by its recorder ([m4-handoff.md](m4-handoff.md), "Tools"), which is data.

**Walked for real.** The enemy-kind and boss recipes were followed end to end on a scratch branch on 2026-11-09, with no Rust edit:

- a new kind on the Lancer's figure with moves and two brains of its own, in the ward's hub;
- a new boss on the Knell Colossus's figure at four fifths, with three phases and a script of its own, in the proving ground.

Both cooked and recorded replays. Both played beside the renderer to their checksums with no first-use build, and the captures showed each kind drawn and fighting. A one-off test read the replays: the kind changed brain at its phase, the boss's script changed its brain and forced its move on the crossing step, and the boss's document changed its brain at its next phase. `bosses_are_content.rs` held the new boss to exit test 2. Then the branch was deleted. What the walk found is in the recipes, or fixed ([m4-handoff.md](m4-handoff.md), "Fixed in M3").

### A move

For the player, on a clip of its own ([m3-handoff.md](m3-handoff.md) has M2's version):

1. **Pose it.** Add a function in [tools/motion/moves.py](../tools/motion/moves.py) to its `MOVES`, and give it an ID in `frames.py`'s `IDS`. `--write` writes `<name>.keypose.ron` and its sidecar. `--write-numbers` puts `frames.py`'s draft frame numbers into the moves document. Read a sheet with `sheet.py`.
2. **Frame it.** Add a row to [player.moves.ron](../content/combat/player/player.moves.ron), last in `moves`, with `name`; `startup`, `active` and `recovery`, adding up to the clip's length; `motion`; `hitboxes` on path `blade`; `cancels`, `warps` and `cues`; and optionally `invulnerable`.

   A row has no intent. A press reaches a move through the top-level `intents` and `neutral`, or through another move's `cancels`. M3 added two optional fields: `cost`, against the fighter's `energy` (Q3c), and `cinematic`, naming a timeline (Q3b): `<name>.timeline.ron` beside the moves document, on the `Steps` clock. Every fight on the set loads it by that name (Z5a), and a name with no timeline there fails the scenario, naming the move.
3. **Its tests find it** (Z5e): [player_moves.rs](../tools/sapphire_pipeline/tests/player_moves.rs) reads the moves with a clip of their own from `player.moves.ron` (a `clip`, and `<name>.keypose.ron` beside the document) and the attacks from those with `hitboxes`. `each_move_plays_the_clip_of_its_name` holds every row: a move plays the key-pose document of its name, an assist the one its `source` names, and only a reaction has no clip. The clip's checks (feet, limits, root motion, the blade fastest on the hit) then hold the new move with no edit.
4. **Give it a row in the graph:** [character.animgraph.ron](../content/lookdev/character/character.animgraph.ron)'s `clips` and `combat.moves`. Without it, `every_move_a_fighter_has_is_in_its_graph` fails.
5. **Give any new cue a row** in [default_cues.ron](../crates/sapphire_cues/default_cues.ron). Without it, `every_cue_the_recordings_publish_has_a_row` fails. A warning the player reads in a fight is a mark of its own shape over the fighter (`at: Crown`), never a colour alone (M4 A5): `tests/marks.rs` holds each pair of marks apart under the three dichromacies.
6. **Cook and rewrite the simulation's copy,** `crates/sapphire_gameplay/baked/player.smoves`:
   ```bash
   cargo xtask content
   SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy
   cargo xtask replay --update
   ```

### A kind of enemy, with its brains

Walked. The worked examples are [content/combat/lancer/](../content/combat/lancer/) and [warrior.enemy.ron](../content/combat/enemy/warrior.enemy.ron).

1. **A figure, if it's a new body.** Write `tools/figures/<name>.figure.ron`: KayKit's joint names at the figure's own places, up to six painted regions, and the shapes along the bones, with the weapon weighted to `handslot.r`. Then:
   ```bash
   cargo run -p sapphire_pipeline --profile tools -- gen-figures --specs tools/figures --out content
   ```
   This writes `content/combat/<name>/`'s model and maps. The material, ramp, ILM and sidecars are hand-written after the Lancer's. Stand it with `--look-dev enemy --model <mesh>`. The walk reused the Lancer's figure and clips, so it started at step 3.
2. **Clips.** Retargeted: add the mesh's ID as a `retarget:` target in the KayKit packs' sidecars (`content/third_party/kaykit_character_animations/**/*.glb.meta`); the `.glb` files themselves stay byte-identical. Key-posed: a module per figure in `tools/motion/` (as `lancer.py` does through `figure_pose.py`) writes `.keypose.ron` documents with `--write`.
3. **Moves:** `<name>.moves.ron` with a `MoveSet` sidecar, as [lancer.moves.ron](../content/combat/lancer/lancer.moves.ron) is. It holds `intents` with no buttons, `fighter` (health, poise, capsule), `moves`, and the five reactions under the warrior's names, which a hit forces. Two kinds may share a moves document, and each plays a copy. A heavy attack's wind-up says `telegraph` at tick 0, as the warrior's `overhead` does: the fight shows it as a chevron over the enemy (M4 A5). The perfect-dodge window's glint needs nothing: it's read from the attack's hitboxes.
4. **Graph:** `<name>.animgraph.ron`, with a row for every move and reaction. Kinds that name the same mesh and graph share them.
5. **The kind:** `<name>.enemy.ron` with an `Enemy` sidecar ([lancer.enemy.ron](../content/combat/lancer/lancer.enemy.ron)). It names itself, `name: "enemy-<name>"`, the key the HUD calls its fighters by (M4 X2): add that key to `content/loc/{en,ja}/game_ui.ftl` and any new Han to `content/fonts/ui.charset.ron`. A kind with no `name` fails its cook, and `cargo test -p game_ui --test strings` fails one whose name either language lacks or that isn't under `enemy-`. It names `moves`, by ID; `brains`, by name, each with spacing, pace, patience, rest, `attacks` pressing intents by name, and `rise`; the starting `brain`; `phases`, in thousandths of health, where a phase may name a `brain` to change to; and `look`: `mesh`, `graph`, `held` (`None` when the weapon is skinned in), `scale`, `height`, `eyes_below_crown` and `framed_radius`. The walk's kind had a second brain at its 500 phase, and changed to it on the hit that crossed it.
6. **Into a zone:** see "An encounter in a zone". A spawner's `spawns` names the kind by its file's stem.

### A boss, with phases and a script

Walked. The worked example is [content/combat/knell_colossus/](../content/combat/knell_colossus/), and B3's test boss is [captain.enemy.ron](../content/combat/enemy/captain.enemy.ron).

1. **A kind of enemy, as above,** with three more things in its `.enemy.ron`:
   - `boss: true`, and its `name` is `boss-<name>` rather than `enemy-<name>`: the key the HUD draws on its bar (Z3e, M4 X2). The strings test holds it under `boss-`.
   - `phases` with names, `(at: 500, name: Some("tolling"))`. The hit that crosses a phase says its name as the boss's cue and changes the brain the phase names. The HUD's boss bar marks each phase. From there the music plays the score's phase of that name, if it has one (see "Music by encounter phase"). The fight marks the crossing over the boss's head by itself, the cue table's `named_phase` row (M4 A5), so a phase needs no row of its own; give its name one only for something more, and keep the name apart from any move's cue.
   - `script: "<id>"`, naming a `.luau` by its sidecar's ID. The sidecar's importer is `Script(())`.
2. **The script.** It returns `names` and its hooks: `on_phase(actor, phase, health)`, `on_move`, `on_hit`, `on_defeat` and `on_beat` are in [sapphire.d.luau](../crates/sapphire_script/sapphire.d.luau). **Phases count from 1, and so do places in `names`;** the walk's first try checked `phase == 0` and never fired. Commands such as `sapphire.set_brain(actor, place)` and `sapphire.force_move(actor, place)` take a place in `names`. A script is pure decisions, with no strings at run time (plan §3, "Luau's job"), and the scan refuses string concatenation and a bare `for` over a table.
3. **Into a zone, content only.** Add a spawner whose `spawns` is the kind, an encounter that calls it, and the scenario updates the next recipe names. The commit that does this must touch only `content/`, `tests/replays/` and `tests/golden/`, or `bosses_are_content.rs` fails. The boss's name must be in no `.rs` file, test messages included, and its script must be reached from the zone.
4. **Kinds with scripts can share a zone.** Each kind's script hears only its own fighters: their phases, moves and defeats, the hits they land or take, and every beat. A fight's scripts share one VM, with its memory limit and each step's budget. They also share one board of 64 numbers, as they share flags. So a script that keeps numbers on the board should use slots that no other script in its zones uses.
5. **Hot reload:** in a `dev` build, `link push <file>.luau` reloads the script into the running fight between steps, and the board is kept.

### An encounter in a zone

Walked, as part of both recipes above.

1. **The encounter:** `<name>.encounter.ron` beside the zone, with an `Encounter` sidecar ([market.encounter.ron](../content/zones/lantern_ward/market.encounter.ron)). It holds `starts_on`, a trigger's event; `faction`; `tokens`, how many may swing at once, and `token_rest`; and `waves`, each with `when` (`Start`, `Remaining(n)` or `After(ticks)`), a `gap` between spawns, and `spawners` by name.
2. **In the zone:** spawners for its fighters (`spawns` is the kind's stem); a trigger (`Box` or `Sphere`) whose `event` starts it; and its ID in `encounters`. A ground-level box must reach about 0.25 m under the paving, or feet never enter it. The cook refuses a spawner off the navmesh, and an encounter naming a spawner or trigger the zone lacks.
3. **Every scenario on that zone** (`content/scenarios/*.scenario.ron` whose `zone` is it) must list the new encounter in `encounters`, and in `cleared` if it shouldn't run there. A scenario that doesn't fails to set up.

   Its kinds needn't be listed anywhere else. The zone's spawners say what it spawns, so a scenario adds each kind that its encounters' spawners name after the kinds it lists, with that kind's moves and script. Cleared encounters count too. The kind is the `<spawns>.enemy.ron` under `content/combat/`, which the build compiles in. A kind's name is its file's, so two `.enemy.ron` files with one name fail the build. A name with no kind fails the scenario's setup, naming the spawner.

   A scenario's `max_actors` must hold its cast and every fighter its encounters bring, cleared ones aside, since a fallen fighter keeps its place: a game's flow refuses a scenario that's too small (`Game::new`, G1c), where the last arrivals would come to nobody and their encounter clear with no fight. `room_for_everyone_counts_the_cast_and_every_fighter_to_come` holds that count to what the zone plans for every scenario, and `ward_flow` holds the flow's run to its pool, so a fighter more in the ward means raising `ward-run`'s `max_actors`, a document's number.
4. **A scenario of its own:** a `cast` member with `moves: "player"` standing at a player spawner inside the trigger, as [ward-terrace.scenario.ron](../content/scenarios/ward-terrace.scenario.ron) does. Then record a replay:
   ```bash
   cargo xtask replay --record <name> --scenario <scenario> --script fight-back --ticks <n>
   ```
   The script is any document in `content/inputs/`, whose `fight-back` serves any encounter. An input none of them gives is a new `<name>.inputs.ron` there, which takes no Rust (M4 R2).
5. **Every replay on the zone moves,** because the zone's digest is in `tables_hash`. Re-stamp them with `cargo xtask replay --update`. The walk moved six. x86-64's lines are owed from CI. On the Lantern Ward, a fight that plays differently also needs `ward-whole-walk` recorded again by its recorder ([m4-handoff.md](m4-handoff.md), "Tools"), which is data. No test counts the ward's kinds or fighters (Z5e): `a_zones_encounters_bring_the_kinds_they_spawn` reads them from every zone's spawners.
6. **A golden** of the fight in its zone, if you want one: a row in `tests/golden/scenes.ron` with the replay's `--arena --replay` capture and a tick, and its PNG ("A golden"). Exit test 2 allows both in the boss's commit, since `tests/golden/` is one of the three directories it may touch.

### A zone, with checkpoints and exits

The worked examples are [content/zones/lantern_ward/](../content/zones/lantern_ward/), [proving_hub/](../content/zones/proving_hub/) and [proving_ground/](../content/zones/proving_ground/). Spec §5.21 is the reference.

1. **The zone:** `content/zones/<name>/<name>.zone.ron` with a `Zone` sidecar. The cook finds it by its suffix and packs `zone_<name>.spack`. The `ZoneDocument` denies unknown fields. It has `grid: (cell_size)`; `placements` of prefabs by GUID (`.assembly.ron`) and `instances` of meshes; `lights`; `probes` (a `.probes.ron` by ID); `day` and `rigs`, as relative paths; `colliders` (by ID); `navigation: Some((min, max, seeds))`, where `seeds` is optional; and `spawners`, `triggers`, `checkpoints`, `encounters`, `splines` and `exits`.
2. **Colliders:** `<name>.colliders.ron` with a `Colliders` sidecar. This is what bodies walk on and what the navmesh bakes from, since placements bring no colliders. A blocker needs its `groups` (`CharacterBlocker`, `CameraBlocker`); a `block_` or `camera_` prefix is only a convention, which a test holds for the arena alone.
3. **Checkpoints:** `(name, position, yaw_degrees, volume)`, reached outside a fight. A save names the latest. The cook refuses one off the navmesh.
4. **Exits:** `(name, to: "<zone id>", arrive: "<spawner there>", prefetch)`, with a trigger whose `event` is the exit's name. The cook reads the other zone to check `arrive`. A hub's `prefetch` brings the next zone in ahead of time.
5. **Look at it:** `--zone <name>[:<spawner>]` on the sandbox, with `--capture`. In the editor, `cargo xtask content --cook editor` first, then the zone pane moves placements.
6. **A scenario and a walk:** a scenario document on the zone, as above, with the zones a trip reaches in its `shelf`. The gameplay build finds every zone a scenario shelves, and the encounters beside it (M4 R3). A replay of a trip runs on the simulation's copy of each one's cook, since its navmesh takes a bake: take a new zone's copies once, into `crates/sapphire_gameplay/baked/zones/`, and again whenever the zone moves:
   ```bash
   cargo xtask content
   SAPPHIRE_BAKE_ZONES=1 cargo test -p sapphire_pipeline --test zone_baked
   ```
   Until then the build warns, and a scenario that shelves the zone fails its setup, saying how. `zone_baked` holds every copy to its documents. A trip in the game itself is data. A new zone is otherwise documents, held by the cook's checks. On the Lantern Ward, `zone_ward.rs` reads its checkpoints from the document (Z5e): each is a walk from the start, and no two share a name.

### A dialogue, with story flags

The worked example is [ward_guide.dialogue.ron](../content/dialogue/ward_guide.dialogue.ron). Spec §5.23 is the reference.

1. **Flags:** add the names to [content/flags.ron](../content/flags.ron), at most 256. A flag is an `i32` that reads 0 until set. The cook refuses a flag that isn't listed.
2. **The dialogue:** `content/dialogue/<name>.dialogue.ron` with a `Dialogue` sidecar. It holds `start`, optionally `opens_on` (an event name), and `nodes`. Each node has `say: (speaker, line, vo)`, `set` flag writes, `next`, up to 4 `choices` (each with `line`, `to`, `when`, `set`) and `branch` arms with `otherwise`. A condition is a small language over flags: `flag.x >= 1 and not (flag.y == 2)`.
3. **Strings:** every key starts `dialogue-`. Add them to `content/loc/en/dialogue.ftl` and `content/loc/ja/dialogue.ftl`, and new Han to `content/fonts/ui.charset.ron`.
4. **Opening it:** the run's scenario lists it in `dialogues`, with `flags` naming the registry, as `ward-run` does. Then either a beat in [game.flow.ron](../content/game.flow.ron) opens it (`then: Dialogue("<id>")`, as the guide opens after the gate), or a zone trigger whose `event` is its `opens_on` does. The fight holds while it's open. `Game::new` refuses a flow whose beats name a dialogue its scenario doesn't load. In the shipped run, `ward_flow`'s `what_the_run_names_resolves_and_plays` cooks each dialogue `ward-run` names and walks it to its end, `plays_the_ward` counts what the scenario names, and the whole walk's recorder answers whatever the run opens (Z5e).
5. **Look at it:** `--ui-game dialogue --ui-dialogue <name>:<n>` previews the panel on `<name>.dialogue.ron`, `<n>` lines in; with no name it's the ward's guide, as the `game-guide-*` goldens show it. The sandbox's build finds each dialogue in `content/dialogue/` by its sidecar (Z5e). In the game: `cargo test -p game_app --features dev --test opens_the_guide -- --ignored`, and `plays_the_ward` for the flow's.

### A cutscene

The worked example is [court_gate.timeline.ron](../content/zones/lantern_ward/court_gate.timeline.ron). Spec §5.23 is the reference.

1. **The timeline:** `<name>.timeline.ron` with a `Timeline` sidecar. It holds `length`; `clock`, which is `Real` for a cutscene (it holds the simulation at a step's edge) or `Steps` for a cinematic in a fight; `camera`, with blends in and out and keys; `characters`, by role, playing rows of their graph and placed by keys; `cues`; `post` offsets; `subtitles` by key; `time_scale` (not on a `Real` clock); and gameplay `events`. Every cutscene can be skipped. There's no field for it, since only the time scale and events reach the simulation.
2. **A new pose:** a `.keypose.ron` and its sidecar, named as a clip and a move in [character.animgraph.ron](../content/lookdev/character/character.animgraph.ron).
3. **Subtitles:** keys and speakers in `content/loc/{en,ja}/cutscenes.ftl`, a line under `subtitle-` and a speaker under `speaker-`: the HUD shows only keys under those prefixes. `tools/sapphire_pipeline/tests/cutscene_strings.rs` and `game_ui --test strings` hold them.
4. **Look at it:** `--arena --timeline <id>` plays one on the sandbox, and the editor's sequencer pane scrubs it.
5. **Starting it:** the run's scenario lists it in `timelines`, and a beat in [game.flow.ron](../content/game.flow.ron) plays it (`when: Started("<encounter>")`, `then: Timeline("<id>")`, with an optional `align` checkpoint the player is brought back to first), as the gate's does. `ward_flow` holds the shipped flow to what its scenario loads, and each timeline it names to its sidecar's ID and its place; the whole walk counts at least one cutscene (Z5e). A beat's timeline is recorded into the run's file, so the whole walk's replay holds it with no scripted input. A replay that plays it on a scenario with no flow is a scripted input document with a `PlayTimeline` at a tick (`content/inputs/`, M4 R2), which takes no Rust. Its goldens, if you want them, are rows in `tests/golden/scenes.ron` ("A golden").

### A game screen

The worked examples are the six documents in [content/ui/](../content/ui/). Spec §5.17 is the reference.

1. **The screen:** `content/ui/<name>.ui.ron` with its sidecar, naming the model it reads, a list of `(path, kind)` that a cook holds every binding to: `menu.model.ron` (a way out, a run to continue, the player's settings), or `character`, `dialogue` or `hud.model.ron` for a screen showing those live values. Its nodes are laid out by `taffy`, with `key_from` for a key chosen at run time under a prefix, and it has `timelines`, `states` and `transitions`.
2. **Strings:** keys in `content/loc/{en,ja}/game_ui.ftl`, the Japanese under the file's machine-quality mark, and new Han in `content/fonts/ui.charset.ron`. Then run `cargo test -p game_ui --test strings --test menus`.
3. **Its row** in `content/ui/menus.ron` (M3 U9): its `name`, its `document` by the sidecar's ID, its `model` (`Menu` unless it reads another), `modal: false` only for words under the fight, what `back` does beyond its machine, and a row for each focusable node: `Command(..)` for the flow, `Open("<screen>")`, `Switch("<screen>")` for a tab that takes the screen's place, `Step(..)` for a setting, `Remap("<binding>")` to remap a binding (M4 A4), or `Nothing`. **Rust step:** a binding nobody could remap before needs a field of its name in `game_ui::menus::BindingGlyphs` for its row to show a glyph. A row on another screen that opens it is how it's reached; that screen's document needs the node. `game_ui`'s menus and strings tests read the list, so they hold its nodes and keys with no change to them, and `a_new_screen_is_documents` is the worked example.

   Changing an existing screen's look, layout, words or motion is documents alone: the editor's screen pane shows an edit on the next frame, and live link reaches a running game. A `.model.ron` is in field order by path, so on a merge, rebuild it from its model struct.
4. **Look at it:** run
   ```bash
   cargo xtask run --platform desktop -- --capture <png> --ui-plain --ui-game <name> --ui-locale ja
   ```
   with `--ui-pseudo` for pseudo-localization. Goldens at 4:3, 16:9, 21:9 and the Deck's 16:10 are rows in `tests/golden/scenes.ron` on the `game-screen` base, each with its `--ui-game` screen and `--ui-locale` ("A golden").

### Music by encounter phase, and voice lines

The worked examples are `content/audio/generated/music/lantern.score.ron` and `content/audio/generated/voice/{en,ja}.voice.ron`. Spec §5.15 is the reference.

1. **A score:** a `Score` document with `tempo_bpm`, `beats_per_bar` and `bars`; `stems`, each with its `.ogg` by ID; `phases`, each naming the stems it plays; and `fade_beats`, `volume_db` and `duck_db`. Its sidecar's importer is `Score(())`. The phases a fight chooses are `waiting`, `running` and `cleared`, and the flow's `music` names those for the title, a defeat and a win. A boss's named phase plays in place of `running` once its health is past it, if the score has a phase of that name (Z5a): the Knell Colossus's `tolling` is the worked example. A phase change fades only the stems it changes, from the next bar line.
2. **Which score:** `music:` in [default_cues.ron](../crates/sapphire_cues/default_cues.ron), by ID. That's a data edit, compiled in.
3. **Voice lines:** each line is a `Cue` document on the `Voice` bus with its `.ogg`. Each language's `VoiceLines` document lists `(key, cue)`, and `voices:` in `default_cues.ron` names each language's document. A cue-table row speaks a line with `line: Some("<key>")` when the simulation publishes that row's cue. The voice language follows the player's language setting.
4. **A placeholder sound,** until a recording replaces it, is a spec, `tools/sounds/<name>.sound.ron`, which takes no Rust: a stem of the score's tempo, beats and bars playing one of the generator's instruments (a pad's chords, a pulse's notes, a drum pattern, a bell tune or a toll), or a line's syllables at a speaker's pitch (`tools/sapphire_soundgen`'s docs). `cargo run -p sapphire_soundgen` from the workspace root writes each missing `.ogg` to the `path` its spec names; add its row to the `SOURCE.md` beside it, which the generator's tests hold, and name it from the score or the line's cue. A recorded `.ogg` needs no spec.
5. **Rust steps:** a line spoken on anything but a published cue needs code, and a conversation's lines aren't voiced (see "Everything deferred").

### A golden

A golden is a row and its PNG (M4 R1). The scenes are `tests/golden/scenes.ron`, whose header says what each field means and why each family of scenes pins what it pins.

1. **The row:** a block at the end of `scenes`, after a comment saying what it shows: its `name` (the PNG's stem, lowercase letters and hyphens), `ticks`, its `size` if not 640×360, a `base` if it shares one, and `args`, the rest of the sandbox's `--capture` command line. Pin off what it isn't about, as its neighbours do, so a later feature doesn't move it; a `--game` scene pins `--tier`. Leave out `gates` unless a review chose its own, and `ci_seconds` until CI has timed it: it counts as the costliest scene meanwhile.
2. **Its Metal golden:** `cargo xtask golden --update --scene <name>`, which writes `tests/golden/<name>.metal.png`. Look at it. Its log in `target/golden/<name>.log` must have `first_use=0` on its "captured frame" line.
3. **The other backends,** from the PR's first CI run, which fails on their missing goldens and uploads the captures: each golden job's artifact holds `<name>.<family>.actual.png`, which becomes `tests/golden/<name>.<family>.png` for `vulkan-lavapipe` and `dx12-warp` once compared against the Metal golden by eye, and the scene's `ci_seconds` come from the same artifacts' `timings.json`, one per family. The testkit's tests then hold all three PNGs to the row.

### A patch

A patch is a release of the content that changes only what the simulation doesn't read: screens, strings, textures, meshes, sounds, effects, the HUD and the bindings (m4-plan §3, "What a patch may carry"). The flow, the flags, the targeting and body tuning and `game.documents.ron` ship only with an app build. M4 P4 is the reference; `cargo xtask patch rehearse` walks the whole road on this machine.

1. **The keys and the host,** once (the maintainer's, "Before a first upload", which has the commands): the public half of each signing seed goes in `platform/patch.toml`'s `release-keys`, and the URL the release is served from in its `host`, `https://` and ending in `/`. Every build after that trusts those keys and checks that host. A build made before has neither, and starts on what it shipped. `cargo xtask package` refuses a binary or a manifest holding the test key.
2. **The first release:** the build that ships carries a signed manifest, `cargo xtask package --platform <platform> --release-version 1 --expires <YYYY-MM-DD> --min-build <n>`, with the seeds in `SAPPHIRE_RELEASE_KEY`. Copy the package's `release/` to the host, `release.smanifest` last.
3. **The patch:** edit `content/`, then `cargo xtask release --cook <desktop|mobile> --out <host directory> --release-version <n+1> --expires <YYYY-MM-DD> --min-build <n> --patch-from <host directory>/manifests/<n>.smanifest`, again with the seeds in `SAPPHIRE_RELEASE_KEY`. It refuses, naming the file, a change the simulation reads, and a release the installed one wouldn't take. One release a cook: desktop and mobile each have their own host directory.
4. **Try it before it ships:** serve the directory with `cargo xtask patch serve <dir> --port <port>`, and start a `dev` build installed with the release before, with `--update-host http://127.0.0.1:<port>/` (the emulator after `adb reverse tcp:<port> tcp:<port>`). Its boot screen fetches, installs and stands on the patch, and the server prints the bytes each request moved. A release signed by a seed of the maintainer's needs a build that trusts it, so try a dev build signed with the test seed first (`patch rehearse` does).
5. **Ship it:** copy `packs/`, `files/` and `manifests/` to the host first, and `release.smanifest` last. A game that starts before the last copy finds the release before, and the next launch finds this one.

### A GPU

Since M4 T4. The table is [gpus.ron](../crates/sapphire_render/gpus.ron), whose header says how each family was placed.

1. **A row:** a `family` name, its `vendor`, the `names` that recognise it and a `ceiling`, the highest desktop tier it may take. A row only lowers a tier: the rules in `tiers.ron` still decide under it.
2. **Names match as runs of tokens,** case, `(R)`, `(TM)` and punctuation aside, and the longest match wins, so `GeForce RTX 2060 SUPER` can override `GeForce RTX 2060`. A table where two names could match one adapter without one holding the other is refused on load.
3. **Hold it:** `cargo test -p sapphire_render quality`. `golden_adapters_select_as_before_the_gpu_table` keeps lavapipe, WARP, the paravirtual Metal device and the Apple M5 matching no row, so no golden moves.

### A rung of the ladder, or a tier

Since M4 T2 and T3. The tiers are [tiers.ron](../crates/sapphire_render/tiers.ron), compiled in.

1. **A rung** is a row in a tier's `ladder`: `Scale` (a render scale), `Shed` (optional passes, each an `OptionalPass`), `FrameRate` (a cap) or `Tier` (a lower tier). The renderer refuses a ladder on load (`quality::ladder`'s `validate`) with more than 8 rungs, scales that don't fall within 0.25 to 1, a pass shed twice or one only a new renderer can shed (the cirrus or a wind level), a second cap or one not under the tier's rate, and a tier rung that isn't lower, of the same class and last. So every rung sheds in place.
2. **Goldens:** a `tier-*-rung-*` row in `tests/golden/scenes.ron` with `--rung <depth>` ("A golden"). Scene names take letters and hyphens only, so a depth is spelled out (`tier-face-desktop-high-rung-two`).
3. **Its walk is data:** `declares_every_setting` stands each shipped tier and holds each in-place rung, so a new rung or tier joins it with no edit. `look_tiers` holds the lines and the two-tone edge at each tier's floor.

### A setting

A setting is code by design, not a pillar recipe: it's a field of `PlayerSettings`.

1. **The field:** a new `VERSION` of `PlayerSettings` with an upgrade from the last, the old version frozen beside it, and its bytes held in `game/game_ui/tests/settings_formats.rs`.
2. **The row:** a `Step(..)` in `content/ui/menus.ron` on its page's document, and its strings in both languages. The menus walk and `declares_every_setting` find it by its row.
3. **What it changes:** presentation reaches the show through `GameShow::follow_presentation`. A setting that changes what a step reads enters the run's tables and is recorded as a replay command, as A1's auto-target strength and A6's timing assist are, and its default must equal what the committed replays ran on. `game_flow`'s `only_the_auto_target_strength_and_the_timing_assist_reach_a_run` holds every other setting to a fight's checksums and tables, so such a setting joins that test's two.

## Rules the reviews made

**M2's, still binding** ([m3-handoff.md](m3-handoff.md), "Rules the reviews made", has each with its test): every struct in a move document denies unknown fields, and so does every M3 document; `MAX_MOVE_REACH` and `MIN_SHAPE_EXTENT`; a defeat takes the stick; `LiveMoves::start` sets up on the edit; a frame's clocks go `alpha` of the way along their last step; a fight starting over ends what the last one set going; a live cue table can't name a sound nobody loaded; `PointerState` decides who has a desktop's pointer.

**M3's packages' own,** each held by a test or a scan:

- **No allocation in a step.** M2's gate 4 holds through every new system. Around a script hook, the VM's allocations count apart (`script_allocations`), but a Rust allocation in a binding still fails. Warming, streaming and the collector run between steps.
- **A script is pure decisions.** Bindings take numbers only, and are generated from `crates/sapphire_script/api.toml` by `cargo xtask script-api`. A bad place, slot or actor is a fault that takes back the hook's commands. Shipping stays `panic = "abort"`, so a binding never returns a Rust `Err`.
- **Nothing reads the OS's math.** The simulation, Luau (libm builtins off, `-ffp-contract=off`) and anything a cook bakes for them use pinned functions only.
- **A cook is the same bytes everywhere** for what the simulation reads: navmesh bakes, zones and scripts. CI's three OSes hold each pin.
- **Kinds are contiguous** in the order they land. **A golden or replay that moves** moves in the package that moves it, with the other backends' and architectures' values from CI.

**M3's reviews',** by area:

- **Renderer and UI (Z3a):** anything whose plan is made in `prepare` settles before a lost frame's `recorded` check, as shadows and insets do. An inset's picture snaps to its plate's whole pixels. A bound key's problem clears once it reads under its prefix. The shape cache compares floats by the bits it hashes.
- **Editor and live link (Z3b):** every document kind in the content opens for editing, so a new kind needs an arm in `tests/fuzz.rs`. The screen pane follows the model file its document names. A drag held through an undo or a save stays one step. Live link counts the app's work (16 requests) apart from each connection's unwritten replies (16).
- **Simulation, scripts and saves (Z3c):** the game's save types (`ZoneStart`, `FlagValues`) keep their version 1 bytes in `crates/sapphire_gameplay/tests/save_formats.rs`, so a field change needs a new `VERSION` with an upgrade that reads them. `ScriptVm::ask` reads another VM's condition as false, as `fire` refuses its hooks. A failed directory flush leaves the new save in its slot, so the game writes again rather than calling it lost.
- **Content and streaming (Z3d):** a pack encoding is keyed by its kind's number, and a zone's kept parse is checked again on every cook. Files under `content/third_party/` stay as downloaded: a corrected copy lives beside its users with the original's sidecar ID, and the original gets a new ID and `cook: false`, as the look-dev character's model does.
- **Platform, input, audio and strings (Z3e):** a boss's name is `boss-<kind>` in both languages, held by `game_ui`'s strings test, and never in `.rs` text. The music bus carries the player's volume. The app thread's own events are never dropped (`Inbox::hold_own`). A captured input stays captured until it's at rest, and a finger slot past `MAX_FINGERS` is dropped. Windows' save rename waits out a held file.
- **The flow and the game (Z3f):** a pause holds a fall or a win too, a quit is heard anywhere, and a conversation that ends under a pause leaves the pause holding play. A won save continues without winning again, and a refused timeline ends at once. The flow's marks are taken only in the flow's zone, and a beat's step allocates nothing. A title choice made under its wipe is kept, and a restart waits for a trip to end. The game's captures refuse the flags they can't honour.

**M4's packages' own,** each held by a test or a scan:

- **No pipeline builds on the frame's thread** after the boot screen's first frame (H1, H3). A pipeline a setting or a rung switches is declared, or `declares_every_setting` fails. An owner a tier shapes joins `Renderer::swap_owners`, and a pipeline family `Held`, or a tier change builds it on the frame (T3).
- **Every rung sheds in place** (T2), so no rung builds a pipeline.
- **A patch carries presentation only.** `Manifest::unpatchable_change` decides, from whether a replay reads the file (`NamedDocument::simulation`). The flow, the flags, the targeting and body tuning and `game.documents.ron` ship only with an app build (P1, P3).
- **No key file is committed.** Tests derive keys from fixed seeds, and `package` refuses a binary or a manifest holding the test key (P4). Integrity rests on the hashes and the signature, never on TLS.
- **A setting that changes what a step reads** is in the run's tables and its recording, and nothing else reaches a checksum (A1, A6). `PlayerSettings` keeps every version's bytes frozen.
- **A cook is the same bytes on every machine of one architecture** (C3). The cooker's crates ban `std`'s transcendentals (`clippy.toml`, which check-layers requires of `[cooker]` crates) and use `sapphire_math::det`, and a stage whose output can change bumps its version.
- **A warning the player reads in a fight is a mark of its own shape** (A5), held apart under the three dichromacies.
- **Text:** the smallest is at least 12 px at 1280×800 by default, and every screen fits at the largest size at every aspect (A2, Z3c).
- **A shipping package holds none of the build machine's paths,** found by a byte scan (C2), and every shipped binary is stripped with its ID matched to its symbols (C1).
- **No placeholder passes for a chosen one** (S1, S4, Z3b): the identity and Steam's IDs say `placeholder = true` while they are placeholders, and a placeholder bundle ID or privacy address lies under a domain RFC 2606 reserves, so `package` can tell. It refuses a placeholder privacy address once the identity is chosen.
- **A file packaging reads needs a filter** in `packages.yml`, held by xtask's `packages_workflow` test (I2). Packaging jobs read caches and never save them.
- **A golden is a row:** `scenes.ron` refuses the runner's own arguments, and every file in `tests/golden/` is a scene's (R1).
- **A desktop GPU's row only lowers a tier,** and the golden adapters match none (T4).
- **A store build has no playtest code:** `sapphire_playtest` is `dev_only` (F1).

**M4's reviews':**

- **The patcher (Z3a):** a body keeps 4 KiB a second after the read timeout's grace (`Timeouts::min_rate`). An `https://` host's redirects stay on `https://`, and plain `http://` reaches only this machine and follows none.
- **Packaging (Z3b):** the privacy page summarizes [privacy-policy.md](store/privacy-policy.md), so a change to one changes the other.
- **Accessibility (Z3c):** a saved remap passes `sapphire_controls::remappable`: the combat context, a device's suffix, and a press of that device other than its cancel. No settings file can take the pause, the resume, the way out or a stick.
- **Crash capture (Z3d):** a Linux or Android fault's address is read from `si_addr` through `sapphire_crash`'s `siginfo` module, never `crash-handler`'s `ssi_addr`. TODO(Z4 part 2): the rest of Z3d's rules as it lands.
- **Warm-up and tiers (Z3e):** TODO(Z4 part 2).

## Everything deferred

### Rust steps left in the "How to add" recipes

M3's first pillar counts each of these as a bug.

- **A new screen** is documents (M3 U9). Only one that shows a value no model holds, or has a row do something no `RowAction` does, is Rust, which is code by design.
- **A remappable binding's glyph** (A4): the binding, its `Remap` row and its strings are documents, but the glyph its row shows is a field of `game_ui::menus::BindingGlyphs`, named by the action. The plan's exit test 7 counted only the screen. Deriving the fields from the remap screens' rows would close it.

### By area

Each item names the package and its hand-back or row, and why it waits.

**Patches** (P1–P4, Z3a):

- **Five gaps Z3a reported, each needing the maintainer's signature or the player's own disk to matter:** a host serving the current release forever reads as up to date, its expiry unread; the launch doesn't hold a patch to its `min_app_build`; links planted in `patch/` are followed; the expiry trusts the device's clock; `release` checks a published pack's header and size only. Windows names ending in `.` or naming a device pass, in a signed manifest only.
- **No downloads in the background or while playing,** no parallel requests and no bandwidth cap (P2, P4). Not needed for a 4 KB patch; a larger one would want them.
- **A power cut** during an install is untested (P3): fault injection covers every step, but not the disk's own behaviour.
- **`--patch-from` doesn't compare packed assets of the simulation's kinds** (P3). The game runs on its compiled-in copies, so a pack's copy can't move a checksum.
- **Cooks differ across architectures** (C3, Z3a): every texture and 78 meshes differ between x86-64 and AArch64, and one x86-64 machine takes another path through the encoders, untraced. A patch from another architecture's cook would carry most of the 37.8 MB cook. Cook releases on one fixed runner image; `release --patch-from` printing the new blocks' bytes would catch a mistake.
- **`fdsm`'s font fields use `std`'s `f64` maths** (C3): third-party, outside the cooker's ban.
- **No command makes a release key or prints its public half** (found writing "Before a first upload"): the maintainer uses OpenSSL's recipe above.
- **Android's TLS roots are `webpki-roots`** (decided at P0): no user-installed CAs, and roots age until an app update. The platform verifier's Kotlin half needs a download.

**Warm-up, tiers and frames** (H1–H5, T0–T4):

- TODO(Z4 part 2): what H4 and Z3e leave here.
- **`AssetServer::finish` uploads every pending asset,** not only the one asked for (H2), so the uploader that panics in `find_zone` is a hazard whenever unrelated uploads are queued. Read on the CPU with `finish_cpu`, as screens and zone inventories are.
- **The first frame after boot loads the fight's sounds** (H2): long, but not a pipeline.
- **H3's walk takes the keyboard's path only,** and holds the cue table's effects all at once rather than as a fight times them.
- **The heat status is asked on the frame thread,** 0.1–0.4 ms once a second on the emulator (T1). A status listener would take it off the frame if a phone hitches.
- **ADPF's frame report includes the swapchain's wait** (T1), so a GPU-bound frame reads long.
- **The emulator's constant headroom of 0.90 reads `Light`** (T2), so the ladder sits a rung down there, and a phone between 0.85 and 0.95 will too.
- **Whether the frame graph culls the scene prepass on Mobile-High when GTAO is shed** wasn't checked (T2).
- **Secondary motion on Desktop-Low:** p99 0.72 ms against its 0.5 (H5). Only a cheaper solve, which moves poses and goldens, gets under it.
- **T4's table is unmeasured:** names a driver reports otherwise (old RADV, NVK) fall to the rules, and Apple's M3 and later and integrated PC GPUs have no rows.

**Packages and stores** (S0–S5, I2, Z3b):

- **Windows' C runtime** (found by Z4, unverified): `.cargo/config.toml` doesn't link the CRT statically, so the Windows game likely imports `VCRUNTIME140.dll`, which a clean Windows lacks, and `package`'s PE check doesn't read imports. CI's runners have the runtime installed, so they wouldn't show it. Either declare Microsoft's Visual C++ redistributable in Steamworks, or link the CRT statically with a package change. Check with `dumpbin /dependents` on the packaged executable.
- **Larger Text, VoiceOver and Sufficient Contrast aren't claimed** (S5): text reaches 125% on a phone where Apple asks 200%, there's no screen reader, and nothing measures the menus' contrast. The game doesn't read iOS's own Reduce Motion.
- **`UIRequiresFullScreen` may be deprecated on iPadOS 26** (Z3b, unverified).
- **`steampipe` isn't run in CI** (I2): no job holds all three desktop packages.
- **Linux's packaged game hasn't drawn:** CI's Linux job has no GPU (steam.md rule 16).
- **macOS is arm64 only** (S4); a universal binary if Intel Macs are wanted.
- **The `macos-26` runner has 7 GB of RAM** for a fat-LTO link (I2).

**Crashes and symbols** (C1–C3, Z3d):

- **No `STACK CFI`** (C2): the walk past the crashing frame uses frame pointers, then scanning. **No PDB inlines.**
- **The macOS monitor names the executable `<Unknown>`** in a minidump; the reader finds it by ID instead (C2).
- **Z3d's noticed, not fixed:** the Linux minidumper's `request_dump` has no timeout, so a hung monitor hangs the game; the handler can deadlock re-entered on a different signal; `dwarf.rs` can overflow `sh_offset + sh_size` on a hostile file (a tool); panic messages and minidumps' module lists and memory may hold paths; `forced.rs`'s `c as u8` is an unnecessary cast to clippy on Android. TODO(Z4 part 2): drop what Z3d's final commits closed.
- **Crash upload** stays out (m4-plan §3, "Not in M4"): there's no account to send to.

**Settings and accessibility** (A1–A6, Z3c):

- **`flashes.rs` is in no CI lane** (Z3c): it needs a GPU and a cook, 137 s here. A lane or the nightly run should take it.
- **The stress fight passes 2.3.1 by default with no margin** (A3): four of its six transitions in its worst second are the camera cutting past a pillar, which is `sapphire_camera::collision`'s, not an effect's.
- **The HUD's health trail tells ember from rose only by an inked edge** (A5, Z3c): 0.096 apart in Oklab.
- **Nothing orders a screen's rows reachable** when scrolling is added elsewhere (Z3c): the walk catches it per screen.
- **Remapping:** mouse bindings and sticks aren't remappable, glyphs are text in US key names, and the listen screen's cancel glyph doesn't follow a pad changed mid-wait (A4).
- **Captions:** 2.5 s is a guess, and lines said in one step show only the last (A2).
- **A cue on the Music bus would take the whole volume twice** (A2's noticed; not checked since).
- **The timing assist has no `GameDocuments` row,** so a game can't swap its numbers in (A6).
- **The sandbox's party is told apart by tint alone** (A5): the sandbox's, not the game's.
- **No screen reader or audio description** (m4-plan §3).

**Playtests** (F1, F2):

- **A label typed by a player:** the format takes one, the pause's row gives none (F1).
- **Each keep rewrites a run's checksums whole,** about 3.8 MB every 10 s near the 4-hour cap, and a kill between the replay and its checksums could leave them mismatched (F1).
- **The report can't see** a move cut off within one step, damage beyond the attack's number, or hits the player landed (F2).
- **A capture's session clock runs fast,** so its seconds played are short (F2).

**The game and content** (X1, X2, S6):

- **No objectives in the flow** (X1): beats that re-fire on a continue and strings in both languages are a package of their own.
- **The collider check can't see** a placement scaled past its colliders, or a collider nothing names (X1).
- **The cooked kind carries neither its name nor its script** (X2, M3), so a game reading kinds from packs needs a format change.
- **`game_app`'s `KeptSettings` duplicates `game_flow`'s kept settings file** (S6's noticed).

**CI and tools** (I1, R1, R4):

- **I1 left:** the Test jobs' second build for the audio device test, Lint's clippy as parallel jobs, `cargo-hakari`, and the Windows golden build's second compile.
- **Caches are about 9 to 10 GB of the repository's 10 GB** (I1). A workflow that saves more evicts them.
- **121 Metal captures differ from their goldens on this Mac,** within the gates (R1). Drift between machines, not a regression.
- **Placeholder sounds compare by bytes only on macOS arm64,** and within 5% RMS elsewhere (R4).

### Waiting since M3

[m4-handoff.md](m4-handoff.md)'s "Known to the run" and "From the spec's 'Can't do' lines" have each; M4 took none of them (m4-plan §3, "Waiting"): voice in conversations; Progress v1's one zone; `tables_hash` at a zone entry; the script VM's heap in replays; one board for every script; no production path loading a `.sscript`; the editor's scenario choice and live tuning of kinds; post's gradient packing; the "Can't do" lines for zones, navigation, kinds, encounters, timelines, dialogue, music and scripts; memory-mapped packs and mip streaming; live link's pairing token; nightly device runs and gate 5. The renderer's "warm-up blocks a load" is closed (H1, H2).

### Not in M4

From m4-plan §3: crash upload and telemetry; accounts and online services; iOS background downloads; Play Asset Delivery and Apple-hosted packs; the Steamworks SDK; upscalers and HDR; a screen reader; patching the simulation's data; Windows code signing; the Mac App Store; consoles; XPBD cloth, ragdolls, dynamic props, decals, GPU particles, lip-sync and a real second character. [m4-handoff.md](m4-handoff.md)'s "Carried from M3's handoff, unchanged" has the older list: hot-patched Rust, sound reload, D3, D4 and N3–N5's limits, and the renderer's missing features.

## Known limits and risks

- **Feel is unknown, and M4 packaged a game on it.** Nobody has played review 3, M2's fight, M3's slice or M4's build. A playtest is now one command, and its sessions come back as replays and a report, so the answer is cheap to act on. If it's the off-ramp, warm-up, patching, tiers, symbols and packaging serve any game, and what goes is `game/` and the editor.
- **No phone has run anything,** after five milestones. Every mobile number is this Mac's (an Apple M5) at the phone tiers' settings, the iOS Simulator's or the Android emulator's. The ladder, ADPF, a phone driver's cold compile and a patch over the network land on phones untested.
- **No account:** every store's last steps are the maintainer's, and Apple's server-side validation, Play's pre-launch report and Steam's review see what no local check can. Budget a round of fixes after the first upload to each.
- **The stores move every year:** Play's API 37 by about August 2027, Apple's next Xcode by about April 2027, and Steam's reviews up to 12 business days ("The stores' next deadlines").
- **"Collects nothing" rests on the patch host** keeping no access logs that identify a device.
- **Export compliance is a legal call** the run can't make: the game links its own TLS.
- **More bugs are likely.** M4's reviews found and fixed TODO(Z4 part 2): the count of Z3a–Z3e's fixes and holds once Z3d and Z3e land. Z4 found two more gaps while writing this file (Windows' C runtime, and no command for a release key).
- **Cooks differ across architectures,** so where a release is cooked decides what a patch moves.
- **A moved checksum costs two CI rounds,** because no machine here runs x86-64.
- **Motion without an animator, machine translation, our own navigation runtime, a VM in the step, software rasterizers in place of GPUs, and scenarios compiled in** all carry from [m4-handoff.md](m4-handoff.md)'s risks unchanged.
- **GitHub's macOS runners were out** for long stretches in M3 and M4, when landings were checked by a local stand-in.
