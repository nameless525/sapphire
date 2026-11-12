# Steam: checklist, store text and the Deck self-check

- **Status:** M4 S5's checklist, 2026-11-10, for the desktop packages S4 builds (`cargo xtask package --platform macos|windows|linux`, then `cargo xtask steampipe`). It serves exit test 3 ([m4-plan.md §1](../m4-plan.md#exit-tests)): what runs without an account is checked in `package`, and the rest is owed to the maintainer (§7). It starts from S4's table of Steam rules and checks each against the tree.
- **How to read it:** each rule names its source and the date it was read, then what meets it, or "owed". Steamworks' pages loaded as text; where only a summary of a page's second half came back, the row says "summary".
- **IDs:** `platform/steam.toml`'s app and depot IDs are Valve's examples, marked placeholders.
- **Related:** [the privacy policy](privacy-policy.md), [the App Store](apple.md), [Google Play](google-play.md).

**Count:** 30 rules: 21 met here (two of them as drafts the maintainer enters), 9 owed: 6 to the maintainer's account, art or Valve's review (2, 11, 13, 26, 28, 29) and 3 to a run under Steam or a Deck (15, 16, 25). Rule 10 is met for English; its Japanese waits for a reader.

**Reviewed (M4 Z3b, 2026-11-11):** every met rule's test or command was found and its claim read against the code at d8b621cc. The packages workflow's first full run (#291, run 43002018202, all five jobs passed) meets rule 30 and moves rules 15 and 16 to what only Steam or a Deck shows; rule 19 is met now that Valve's pads have glyphs of their own.

## Rules

| # | Rule | Source, read | Met by | Status |
|---|---|---|---|---|
| 1 | Screenshots: at least 5, 1920×1080 or larger at 16:9, showing only gameplay: no concept art, cinematic stills, awards or marketing copy. | "Graphical assets", partner.steamgames.com/doc/store/assets, and its "standard assets" page (the count from its summary), 2026-11-10; "Review process", 2026-11-10 | `cargo xtask store-shots`: six shots at 1920×1080 in [Screenshots](#screenshots), all of play; `store_shots::tests` holds the sizes to this doc | Met |
| 2 | Capsules: header 920×430, small 462×174, main 1232×706 and vertical 748×896 (required); library capsule 600×900, hero 3840×1240, logo; only the game's name as text. | "Graphical assets" and "Store graphical asset rules", 2026-11-10 | Not made: they're key art | Owed: the maintainer's art |
| 3 | The short description is plain text, "a few hundred characters", with no time-based words and no links. | "Store page description", partner.steamgames.com/doc/store/page/description, 2026-11-10 | [Store text](#store-text): held under 300 characters by `store_shots::tests::each_stores_text_fits_its_limits` | Met |
| 4 | The description links nowhere and advertises no other game. | same | The drafts | Met |
| 5 | Languages are declared as the game's settings offer them: interface, full audio, subtitles. | "Localization and languages", partner.steamgames.com/doc/store/localization, 2026-11-10 | [Languages](#languages): interface and subtitles in English and Japanese, from `the_settings_show_what_they_are_and_the_language_moves_every_screen` and `a_cutscenes_subtitles_show_in_every_language_at_every_aspect` (`game_ui/tests/menus.rs`) | Met (draft) |
| 6 | Every feature the store page lists is in the build. | "Review process", 2026-11-10 | The drafts name only what's built | Met |
| 7 | In-game purchases go through the Steam Wallet. | same | The game sells nothing | Met: not needed |
| 8 | The mature content survey: general content, mature content, and generative AI. | "Content survey", partner.steamgames.com/doc/gettingstarted/contentsurvey, 2026-11-10 | [Content survey](#content-survey) below | Met (draft) |
| 9 | Single-player content plays with no internet connection. | "Getting your game ready for Steam Deck and Steam Machine", 2026-11-10 | The game needs no connection: it checks for a patch only once `platform/patch.toml` names a host, and a check that can't connect starts the game on what it has (M4 P4; [the privacy policy's audit](privacy-policy.md#why-collects-nothing-holds)) | Met |
| 10 | The store text reads well in each language it's offered in. | m4-plan §7 | English drafted here | Met for English; owed: a Japanese reader |
| 11 | Generative AI: content made with AI tools is described in the survey. | "Content survey", 2026-11-10 | The code was written with an AI assistant; the art and sounds are third-party or procedural (`content/third_party/README.md`, `sapphire_soundgen`) | Owed: the maintainer answers, against the survey's own wording |
| 12 | macOS builds are 64-bit and use the hardened runtime, with no app-sandbox entitlement, which Steam doesn't support. | "Platforms", partner.steamgames.com/doc/store/application/platforms, 2026-11-10 (the page dates from 2019); Apple's notarization page (S4) | `package --platform macos` signs with the hardened runtime and `platform/macos/package.entitlements`, which grants nothing, and reads both back (S4) | Met |
| 13 | macOS builds are notarized, with a Developer ID. | same | | Owed: the maintainer's Apple account |
| 14 | The Steam overlay may need `disable-library-validation` on macOS. | same (it names that entitlement and `allow-dyld-environment-variables`) | Not granted (rule 12) | Met for a build with no overlay; S4 owes the check with Steam running |
| 15 | The Linux build runs in the Steam Linux Runtime the app chooses: 3.0, sniper. | S4, from the runtime's package manifest, 2026-11-10; "Linux", the platforms page's own, 2026-11-10 (summary: a runtime is chosen per branch) | `package --platform linux` refuses a library or symbol version newer than sniper's (S4, `xtask/src/platforms/elf.rs`); CI builds it inside sniper's SDK, and the check passed there (`packages.yml`, run 43002018202, 2026-11-11) | Owed: a run under the runtime, from Steam (a Deck or a Linux machine); choosing the runtime is the maintainer's |
| 16 | The build launches from Steam on every OS the store page lists. | "Review process", 2026-11-10 | macOS: the packaged game drew its title and a new game at 1280×800, headless (S4); Windows: CI draws the title from the package on WARP (`packages.yml`, run 43002018202) | Owed: Linux's packaged game drawing (CI's Linux job has no GPU), a launch from Steam itself, and Valve's review |
| 17 | App and depot build scripts. | "Uploading to Steam", partner.steamgames.com/doc/sdk/uploading (S4) | `cargo xtask steampipe` writes them, one depot per OS (S4) | Met |
| 18 | Windows builds need no Authenticode signature. | None found (S4) | | Met: not needed |
| 19 | Deck: on-screen glyphs match the controller in use. | "Steam Deck compatibility review", partner.steamgames.com/doc/steamdeck/compat, 2026-11-10 (summary) | `GamepadFamily::Steam` tells Valve's controllers apart (S4); the remap screens draw the pad in hand's glyphs (A4), a Deck's as it's printed: L1, R2, L3, the back grips L4 to R5 (`sapphire_controls::remap`'s `a_glyph_follows_the_pads_labels`, M4 Z3b) | Met here; that SDL names a Deck's pad as Steam's under Steam Input is owed to a Deck (S4) |
| 20 | Deck: every screen works with the controller alone, under its default configuration. | same | The pad walk at 1280×800: `every_menu_walks_fits_and_comes_from_the_tables_by_every_device_at_every_aspect` (`game_ui/tests/menus.rs`); a conversation by pad: `a_pad_finds_the_hidden_answer_and_the_branch_back` (`game_ui/tests/dialogue.rs`) | Met |
| 21 | Deck: text entry by Steam's keyboard, or the game's own controller entry. | same | No screen takes text (S4) | Met |
| 22 | Deck: runs at 1280×800 (preferred) or 1280×720. | same | Every screen's golden at 1280×800 in both languages, `game-*-deck-*` (22 scenes, S4), and the [Deck's screenshots](#screenshots) | Met |
| 23 | Deck: the smallest text is at least 9 px at 1280×800; 12 recommended. | same, read word for word by A2 on 2026-11-10 | `every_screens_smallest_text_is_legible_on_a_deck_and_everywhere_else` (`game_ui/tests/text_size.rs`): 14.0 px by default in both languages, 10.9 at the smallest | Met |
| 24 | Deck: no launcher, or one a controller can use. | same | The game opens on its title (S4) | Met |
| 25 | Deck: a playable frame rate by default, 30 fps at 800p. | same (summary) | | Owed: a Deck (T3's tiers, m4-plan §7) |
| 26 | Deck: Valve runs the compatibility review; results come a week before they're published. | same (summary) | | Owed: Valve's review |
| 27 | No device warnings on the Deck. | same (summary) | | Met as far as known: the game shows none of its own; a Deck confirms it with rule 26 |
| 28 | The Steamworks account, the app credit fee, tax and bank details, and the store page's review, at least 7 business days before release. | "Review process", 2026-11-10; "Onboarding", as the scouts' note gives it | | Owed: the maintainer's account |
| 29 | The upload: `steamcmd +login <account> +run_app_build <app build> +quit`, then the client icon and launch options in Steamworks. | S4, 2026-11-10 | `steampipe` writes `depots.txt` with each depot's files and launch option | Owed: the maintainer's account |
| 30 | The Windows package: per-monitor DPI, VERSIONINFO and icon read back from the executable. | S4 (Steam asks for none of it; Windows' own conventions) | `package --platform windows` reads them back (S4, `pe.rs`), and CI reads the version as Windows does (`packages.yml`'s Windows job, run 43002018202, 2026-11-11) | Met |
| 31 | A runtime the game needs and a player's machine may lack, the Visual C++ redistributable for one, is either installed by Steam, ticked under Installation, Redistributables in Steamworks ("installed only if necessary"), or not needed. | "Common Redistributables", partner.steamgames.com/doc/features/common_redist, 2026-11-11; "Install Scripts", partner.steamgames.com/doc/sdk/installscripts, 2026-11-11 | Not needed: a shipping Windows build links the C runtime statically (`+crt-static`, and `platform/windows/static_crt.cmake` for SDL's CMake build; M4 X4), and `package --platform windows` fails if the executable imports a C runtime DLL (`VCRUNTIME*`, `MSVCP*`, `ucrtbase`, `api-ms-win-crt-*`). Leave the box unticked | Owed: `packages.yml`'s Windows job on the change, and a launch on a clean Windows install |

## Owed to the maintainer

1. **The account:** Steamworks, the app credit fee, tax and bank details, and the real app and depot IDs in `platform/steam.toml` (rules 28, 29).
2. **Art:** the capsules and the library assets (rule 2).
3. **The forms:** the store text, the languages and the content survey below; the generative AI answer is theirs to give (rule 11).
4. **macOS:** a Developer ID certificate, notarization and stapling (rule 13), and whether the overlay needs an entitlement (rule 14).
5. **The upload and review:** `steamcmd` (rule 29), the store page's review and the build's (rules 16, 28), and the Deck's (rule 26).
6. **A Deck,** for its frame rate at 800p and the self-check below (rules 25–27).

## Screenshots

`cargo xtask store-shots` draws each shot at each size, in English and Japanese, into `target/store-shots/steam/<size>/<language>/`, never committed. Steam takes 1920×1080 and up; the Deck's own 1280×800 isn't a store slot, but it's the Deck self-check's evidence, drawn by the same shots. A desktop's screen shows no touch controls, and a Deck's none either, since a pad's in its hands.

| Size | Pixels | Points | Tier | Controls | Source |
|---|---|---|---|---|---|
| `desktop` | 1920×1080 | 1 | desktop-high | none | "Graphical assets": screenshots at 1920×1080 or more, 16:9 |
| `deck` | 1280×800 | 1 | desktop-low | none | "Steam Deck compatibility review": 1280×800 preferred |

The Deck's tier is a guess, Desktop-Low: which tier a Deck picks is T3's, and owed to a Deck.

## Store text

Drafts. Steam gives no exact limit for the short description, only "a few hundred characters", so these keep under 300; the description has none given, and these keep under 4000. A test holds every draft to its heading's limit. The Japanese is for a native reader's review.

#### English: short description (300 characters)

```text
A 3D action game in an anime art style. Fight through the Lantern Ward with light and heavy attacks, a skill and an ultimate; dodge just as a blow lands, counter, and face the Knell Colossus. In English and Japanese, with a controller or keyboard and mouse.
```

#### English: about this game (4000 characters)

```text
Sapphire is a 3D action game in an anime art style. Walk the Lantern Ward, a walled town of courts and markets, and fight to light its lantern again.

The fight
- Light and heavy attacks, a skill and an ultimate.
- Dodge just as a blow lands and the world slows for a moment: then counter.
- Skeleton warriors, lancers and a captain, and the Knell Colossus, a boss that changes as it weakens.
- Every enemy warning has its own shape as well as its colour.

The ward
- Its courts, its market and a terrace above them.
- A cutscene at the court's gate and a conversation with the ward's guide, with subtitles.

Play your way
- A game controller, or keyboard and mouse. Every screen works with a controller alone, and the game runs at the Steam Deck's 1280×800.
- Settings for text and interface size; subtitles, their size, background and speaker names; camera shake, screen flashes and camera motion; separate music, effects and voice volumes; mono sound; auto-target strength; and hold or toggle for each action you can hold.
- English and Japanese.

No purchases and no account. Plays offline.
```

#### Japanese: short description (300 characters)

```text
アニメ調のグラフィックで描く3Dアクションゲーム。弱攻撃と強攻撃、スキル、必殺技を駆使して灯籠区を進み、攻撃が当たる瞬間に回避して反撃し、弔鐘の巨像に挑もう。英語と日本語に対応。コントローラー、またはキーボードとマウスで遊べます。
```

#### Japanese: about this game (4000 characters)

```text
サファイアは、アニメ調のグラフィックで描く3Dアクションゲームです。中庭と市場が連なる城壁の町「灯籠区」を進み、消えた灯籠に再び火をともしましょう。

戦い
・弱攻撃と強攻撃、スキル、必殺技。
・攻撃が当たる瞬間に回避すると、一瞬だけ時間がゆっくりに。そこから反撃。
・骸骨の戦士、槍使い、骸骨の隊長、そして弱るほどに戦い方を変えるボス「弔鐘の巨像」。
・敵の攻撃の予兆は、色だけでなく形でも見分けられます。

灯籠区
・中庭、市場、そしてそれを見下ろす高台。
・中庭の門でのカットシーンと、案内人との会話。字幕つきです。

自分に合った遊び方
・ゲームコントローラー、またはキーボードとマウスで操作できます。すべての画面をコントローラーだけで操作でき、Steam Deck の 1280×800 にも対応しています。
・文字とインターフェースの大きさ、字幕（大きさ、背景、話し手の名前）、画面の揺れ・フラッシュ・カメラの動き、音楽・効果音・音声それぞれの音量、モノラル音声、オートターゲットの強さ、長押しできる操作ごとの「長押し／切り替え」を設定できます。
・英語と日本語に対応。

課金、アカウント登録はありません。オフラインで遊べます。
```

## Languages

Steamworks' language table, as the game's settings offer each (the "Localization and languages" page, 2026-11-10):

| Language | Interface | Full audio | Subtitles |
|---|---|---|---|
| English | Yes | No | Yes |
| Japanese | Yes | No | Yes |

Full audio isn't claimed: the voices are placeholder sounds, the same in both languages (M4 R4).

## Content survey

Steam's content survey (its page, 2026-11-10, lists its three sections but not their questions; the maintainer maps these onto the form as it asks them):

- **General content** (which sets regional ratings): frequent fantasy violence, sword fights against skeletons and armoured figures in an anime art style; no blood or gore; a defeated enemy falls. No sexual content, nudity, strong language, drugs, alcohol or gambling. No purchases. Single player, no chat, no user-generated content.
- **Mature content:** none of its categories.
- **Generative AI:** owed (rule 11). No content the player sees or hears is generated while playing.

## Deck self-check

Valve runs the compatibility review itself and offers no tool for developers ("Steam Deck compatibility review", 2026-11-10, summary). This is the run's own check against Valve's criteria, so the maintainer knows what to expect before asking for one.

| Valve's criterion | Evidence here | Status |
|---|---|---|
| **Input:** the default controller configuration reaches every part of the game. | SDL reads Steam Input's gamepad; every menu is walked by pad at 1280×800 (`every_menu_walks_fits_and_comes_from_the_tables_by_every_device_at_every_aspect`), and a conversation (`a_pad_finds_the_hidden_answer_and_the_branch_back`) | Met here; SDL's controller type under Steam Input is owed to a Deck (S4) |
| **Input:** glyphs match the Deck's controls. | `GamepadFamily::Steam` (S4), drawn as the Deck's buttons are printed on the remap screens (A4, Z3b) | Met here; SDL's type under Steam Input owed to a Deck |
| **Input:** text entry. | No screen takes text | Met |
| **Display:** 1280×800. | The `game-*-deck-*` goldens: every screen, both languages (S4); `store-shots`' `deck` size draws the game in play there | Met |
| **Display:** the smallest text at least 9 px at 800p, 12 recommended. | `every_screens_smallest_text_is_legible_on_a_deck_and_everywhere_else`: 14.0 px, 10.9 at the smallest setting | Met |
| **Performance:** a playable frame rate by default, 30 fps at 800p. | None: no Deck | Owed |
| **Seamlessness:** no launcher; no warnings. | The game opens on its title | Met as far as known |
| **System:** runs as a native Linux build. | Steam Linux Runtime 3.0's bounds, checked by `package --platform linux` (S4) and passed in CI's sniper SDK (2026-11-11) | Owed: a Deck |
