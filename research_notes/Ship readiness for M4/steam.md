# Steam: what shipping on it asks of Sapphire

A scout's read on 2026-11-10, web only, nothing built or run. "ours:" marks inference; everything else is a source's claim with its link. Where a fact came from memory of Valve's docs rather than a page read today, it says "may be stale".

## 1. Steamworks review and what only an account holder can do

- Valve: build review before release "typically takes 3-5 business days", plan for at least 7; the store page must be submitted for review first, at least 7 business days before it should go live. https://partner.steamgames.com/doc/store/review_process
- News (Nov 2026): Valve announced on 3 Nov 2026 that pre-release reviews take up to 12 business days, mostly because of repeat reviews of broken builds, and tightened its pre-release checklist. https://godisageek.com/2026/11/steam-game-review-12-business-days/ , https://www.invenglobal.com/articles/28152/valve-says-steam-pre-release-reviews-now-take-up-to-12-business-days
- Onboarding page gives the first review as 2-7 business days. https://partner.steamgames.com/doc/gettingstarted/onboarding
- What review checks (Valve's review page, from memory of it, may be stale): the build launches and is playable from Steam, matches the store page (screenshots, description, languages, controller claims), no broken/placeholder content, store page has required assets and accurate system requirements. https://partner.steamgames.com/doc/store/review_process
- Account-holder-only steps (Steam Direct, may be stale): Steamworks partner account, US$100 app credit fee per app (recoupable after US$1,000 gross), tax interview (W-8/W-9), bank details, identity verification, and a waiting period between paying and release. https://partner.steamgames.com/doc/gettingstarted/onboarding , https://partner.steamgames.com/doc/store/releasing
- ours: the engine cannot do any of these; M4 can only make the build reviewable (launches cold, no crash on first run, supported languages and controller support truthfully declared).

## 2. Steam Deck compatibility

- Valve's criteria (Verified / Playable / Unsupported) cover input, display/legibility, seamlessness and system support. https://partner.steamgames.com/doc/steamdeck/compat
- Input: full controller support with a default controller config; glyphs must match Deck controls (Xbox-style is accepted as Playable, not Verified, may be stale); no mouse/keyboard needed. Same page.
- Text entry: a game that needs text must open the on-screen keyboard through a Steamworks API, or provide its own controller-usable entry. Same page.
- Display: supports 1280x800 (16:10) or 1280x720 with letterboxing; default settings must run at playable framerates. Same page.
- Legibility: the smallest on-screen font must not fall below 9 px tall at 1280x800, 12 px recommended, readable at 30 cm; user-configurable text size recommended. Same page. Checked against the page on 2026-11-10 (M4 A2): as it says, though it never says which height of a character it means, so A2 measures the em.
- Seamlessness: no launcher, or a launcher fully navigable by controller; no anti-cheat incompatibility. Same page; sleep/resume expectation is from memory, may be stale.
- 2026: the same programme now spans "SteamOS Verified" for Steam Machine and Steam Frame; display tests are not run for Steam Machine. https://partner.steamgames.com/doc/steamhardware/compat , https://www.gamingonlinux.com/guides/view/what-steam-deck-steam-machine-steam-frame-and-steamos-verified-ratings-mean/
- ours: our text-size setting scales UI points; we need a check that the smallest default font is >= 12 px at 1280x800, and the settings screen needs a resolution/quality preset that defaults sanely there.

## 3. Proton vs native Linux, and the runtime

- Steam Linux Runtime 3.0 ("sniper") is the current runtime after 1.0 "scout" and 2.0 "soldier"; community reports say it is the default container for native games on desktop Linux. https://news.lavx.hu/article/lutris-0-5-21-adds-support-for-running-games-inside-valve-s-latest-steam-runtime
- Reddit (Oct 2024, unverified against Valve): Steamworks gained a per-app "Linux Runtime" selector under Installation. https://redlib.hbubli.cc/r/linux_gaming/comments/1g6ds3c/valve_makes_a_big_improvement_for_native_linux
- Valve's runtime docs (from memory, may be stale): new native games should target sniper; scout is legacy. https://gitlab.steamrt.steamos.cloud/steamrt/steam-runtime-tools/-/blob/main/docs/slr-for-game-developers.md
- Proton being default does not override native Linux builds (GamingOnLinux, Jun 2025). https://www.gamingonlinux.com/2025/06/steam-beta-finally-enables-proton-on-linux-fully-making-linux-gaming-simpler/
- ours: Valve tests Deck compatibility on whatever build Deck runs; shipping native Linux means the Deck runs that build, so its bugs count. Shipping only Windows and letting Proton run it is the common indie choice. Couldn't find a fresh Valve statement preferring either.
- ours: a native build should be linked against sniper's glibc/library set (build in its SDK container), with SDL3 static as today.

## 4. SteamPipe

- Depots hold files per OS/language; builds are uploaded with steamcmd/ContentBuilder using app and depot build scripts, logged into a Steamworks account. https://partner.steamgames.com/doc/sdk/uploading
- Diffing: SteamPipe splits each file into roughly 1 MB chunks and reuses matching chunks from the previous build. Same page.
- A changed offset in a pack's table of contents (4-8 bytes) still makes a new 1 MB chunk; inserting or resizing an asset shifts everything after it. Same page.
- Valve's advice: keep changes localized, don't reorder assets, limit pack file size, align (Unreal: 1 MB padding alignment so shifts are multiples of the chunk size); an alternate build algorithm exists on request. Same page.
- Forum, not official: compressing whole packages defeats delta patching. https://forums.unrealengine.com/t/steam-update-size/480513
- ours: our pack format should place assets at 1 MB-aligned offsets in a stable order (by asset id, not cook order), compress per asset, keep the TOC in its own small file or at the end, and split into several packs (by stage/language) so a patch touches few.

## 5. macOS and Windows signing

- macOS (Valve docs, may be stale): Steam requires Mac builds to be notarized by Apple since macOS 10.15, and Apple Silicon is supported natively; needs an Apple Developer account. https://partner.steamgames.com/doc/store/application/platforms
- Windows: no Steam requirement to Authenticode-sign found; unsigned games launched by Steam don't trigger SmartScreen the way a downloaded installer does (ours, from general knowledge; not verified today).

## 6. Steam Input with SDL3

- SDL (both 2 and 3) detects Steam Input's virtual controller and works with it; Steam Input can also remap to a pad layout the game sees as XInput/SDL. https://partner.steamgames.com/doc/features/steam_controller (may be stale)
- Valve requires Deck-correct glyphs for Verified, not Steam's own glyph art; ISteamInput can return glyphs if the game uses the Steam Input API. https://partner.steamgames.com/doc/steamdeck/compat
- ours: our SDL3 path plus a Deck glyph set (SDL reports the Steam Deck as its own controller type) should satisfy Verified without ISteamInput.
