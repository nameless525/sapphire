# The App Store: checklist and store text

- **Status:** M4 S5's checklist, 2026-11-10, for the iOS archive S2 builds (`cargo xtask package --platform ios`). It serves exit test 3 ([m4-plan.md §1](../m4-plan.md#exit-tests)): what can be proved without an account is proved here, and the rest is owed to the maintainer, who holds the account (§7).
- **How to read it:** each rule names its source and the date it was read, then what meets it: a test, a command, or "owed". Apple's help pages render in the browser, so most were read through a summary of the page rather than its text: those say "summary" and their exact wording is unverified. A rule from an earlier package's reading keeps that package's date.
- **The game's name** is S1's placeholder (`platform/identity.toml`), as is the bundle ID `com.example.sapphire.game`, which Apple won't take. The maintainer chooses both first.
- **Related:** [the privacy policy](privacy-policy.md), [Google Play](google-play.md), [Steam](steam.md).

**Count:** 32 rules: 22 met here (three of them as drafts the maintainer enters), 10 owed: 7 to the maintainer's identity, account or Apple's review (6, 18, 27–30, 32), 2 to a person's judgement (16, 20) and 1 to a later package (7).

## Rules

| # | Rule | Source, read | Met by | Status |
|---|---|---|---|---|
| 1 | Uploads are built with Xcode 26 or later and an iOS 26 SDK, since 2026-04-28. | "Upcoming requirements", developer.apple.com/news/upcoming-requirements, 2026-11-10 | `xtask package --platform ios` writes the `DT*` keys from the installed Xcode and SDK and refuses an executable whose `LC_BUILD_VERSION` names another SDK (S2, `xtask/src/platforms/ios_store.rs`) | Met |
| 2 | Apps target iOS 13 or later, since 2026-09-09. | same, 2026-11-10 | `MinimumOSVersion` 17.0 in `platform/ios/Info.plist`, checked against the executable (S1) | Met |
| 3 | A required-reason API the binary uses is declared in `PrivacyInfo.xcprivacy`, or the upload is refused (ITMS-91053). | "Describing use of required reason API", 2026-11-10 (S0, S2) | `package` scans the shipping executable's imports and selectors and fails on an undeclared category (`xtask/src/platforms/privacy_manifest.rs`, its unit tests) | Met |
| 4 | The privacy manifest says whether the app tracks and what it collects. | same, 2026-11-10 (S2) | `NSPrivacyTracking` false, no collected types or tracking domains; `plutil -lint` in `package`'s checks | Met |
| 5 | The App Store's privacy answers describe what the app collects: data sent off the device and kept longer than the request needs. | "App privacy details on the App Store", developer.apple.com/app-store/app-privacy-details, 2026-11-10 | [App privacy](#app-privacy) below: "Data Not Collected", from [the privacy policy's audit](privacy-policy.md#why-collects-nothing-holds) | Met (draft); the maintainer enters it |
| 6 | A privacy policy URL, for every iOS and macOS app. | "App information", developer.apple.com/help/app-store-connect/reference/app-information/app-information, 2026-11-10 (summary) | [privacy-policy.md](privacy-policy.md), a draft | Owed: the maintainer hosts it |
| 7 | The privacy policy is also reachable inside the app. | App Review Guidelines 5.1.1(i), developer.apple.com/app-store/review/guidelines, 2026-11-10 (summary; the page says updated 2026-10-16) | Nothing yet: no screen opens or shows it | Owed: a settings row or a title link, a later package |
| 8 | `ITSAppUsesNonExemptEncryption` says whether the app uses encryption beyond the exempt kinds. | "Complying with encryption export regulations": not readable 2026-11-10 (renders client-side); "Overview of export compliance", 2026-11-10 (summary) | `false` in `platform/ios/Info.plist`: the game links no encryption (`cargo tree -p game_app -i ureq` finds none) | Met while true; owed once P4 links the patcher's TLS (below) |
| 9 | An app icon at 1024 px, compiled into `Assets.car`, with the Info.plist's icon keys. | S0 and S1's reading of actool's output, 2026-11-10 | `actool` in `package` (S1); `CFBundleIcons` and `CFBundleIcons~ipad` | Met |
| 10 | A launch screen. | Apple's Info.plist reference, as S1 read it | `UILaunchScreen` (an empty dict) in the Info.plist | Met |
| 11 | `CFBundleVersion` grows with each upload. | App Store Connect's upload rules, as S1 read them | The build number is the count of commits on `main` (S1, `xtask/src/version.rs`), which only grows | Met |
| 12 | The games category, which also lets iOS start Game Mode. | `apple.md` research note, 2026-11-10 | `LSApplicationCategoryType` `public.app-category.games` (S2) | Met |
| 13 | The app's capabilities match the devices it runs on. | Info.plist reference, as S1 read it | `UIRequiredDeviceCapabilities` arm64 and metal | Met |
| 14 | Screenshots: 1 to 10 per display size, JPEG or PNG with no alpha. A 6.9" or 6.3" iPhone set, and a 13" iPad set if the app runs on iPad. | "Screenshot specifications", developer.apple.com/help/app-store-connect/reference/screenshot-specifications, 2026-11-10 (summary) | `cargo xtask store-shots`: six shots at each size in [Screenshots](#screenshots), RGB PNGs; `store_shots::tests` holds the sizes to this doc | Met |
| 15 | Screenshots show the app in use, not only its title art or splash screen. | Guideline 2.3.3, 2026-11-10 (verbatim) | Every shot is a golden scene of play: fights, a cutscene, the court, the hub | Met |
| 16 | Screenshots and previews suit a 4+ rating even if the app is rated higher. | Guideline 2.3.8, 2026-11-10 (verbatim) | The shots show stylized sword fights against skeletons and armour, with no blood | Owed: a person judges the shots |
| 17 | The name has 2 to 30 characters and the subtitle at most 30; promotional text 170, description 4000, keywords 100 bytes. | "App information" and "Platform version information", 2026-11-10 (summary); Guideline 2.3.7 (verbatim) | [Store text](#store-text), held to those limits by `store_shots::tests::each_stores_text_fits_its_limits` | Met |
| 18 | No placeholder content or metadata in what's submitted. | Guideline 2.1(a), 2026-11-10 (verbatim) | The game's name, bundle ID and icon are placeholders (`placeholder = true`), which `xtask` refuses to pass off as chosen | Owed: the maintainer's identity |
| 19 | Apps don't download code that adds or changes features. | Guideline 2.5.2, 2026-11-10 (verbatim) | Patches carry presentation only (m4-plan §3): `sapphire_patch`'s `a_release_that_changes_what_the_simulation_reads_is_refused`, and `xtask package --patch-from` refuses one | Met |
| 20 | No realistic portrayals of people being killed or maimed. | Guideline 1.1.2, 2026-11-10 (verbatim) | The enemies are skeletons and armoured figures in an anime art style; a defeat is a fall, with no blood | Owed: a person judges |
| 21 | The age rating questionnaire is answered (its new form, since 2026-01-31). | "Age ratings values and definitions", 2026-11-10 (summary); "Upcoming requirements", 2026-11-10 | [Age rating](#age-rating) below | Met (draft); the maintainer enters it |
| 22 | Accessibility Nutrition Labels: voluntary now, required "over time"; a feature is claimed only if every common task works with it. | "Overview of Accessibility Nutrition Labels", 2026-11-10 (summary) | [Accessibility labels](#accessibility-nutrition-labels) below, each tied to its test | Met (draft) |
| 23 | Tracking needs App Tracking Transparency. | Guideline 5.1.2, 2026-11-10 (summary) | The game tracks nobody (rule 4) | Met: not needed |
| 24 | An app with sign-in offers Sign in with Apple and account deletion. | Guideline 4.8 and 5.1.1(v), as the scouts' note gives them | The game has no account | Met: not needed |
| 25 | The archive is what Xcode's Organizer reads, with its dSYM. | S0's spike and S2, 2026-11-10 | `package` lays out `SapphireGame.xcarchive` and checks it offline (`checks/`: plists, `codesign --verify --strict --deep`, UUIDs, `atos` lines) | Met |
| 26 | The app has no entitlements it doesn't use. | S2's reading, 2026-11-10 | `platform/ios/package.entitlements` grants nothing; read back from the signature | Met |
| 27 | Distribution signing: an Apple Distribution certificate and a provisioning profile. | S0, S2 | `xcodebuild -exportArchive` stops at signing without them | Owed: the maintainer's account |
| 28 | The app record, agreements, tax and banking in App Store Connect. | "TestFlight overview", as the scouts' note gives it | | Owed: the maintainer's account |
| 29 | TestFlight internal testing: up to 100 team members, no Beta App Review. | developer.apple.com/testflight, as the scouts' note gives it (unverified) | | Owed: the maintainer's account |
| 30 | App Store Connect's own validation of the upload. | S2 | Only Apple's servers run it | Owed: the maintainer's upload |
| 31 | Symbols for crash reports. | S2 | The dSYM is in the archive's `dSYMs/` | Met; uploading it is owed with the archive |
| 32 | Review itself. | Guidelines, 2026-11-10 | | Owed: Apple's review |

## Owed to the maintainer

1. **The identity:** the game's name, its bundle ID (permanent once uploaded) and its icon (rule 18). Replace `platform/identity.toml`'s placeholders.
2. **The account:** the Apple Developer Program, an Apple Distribution certificate and a profile for the bundle ID, the App Store Connect record, agreements, tax and banking (rules 27, 28).
3. **Export compliance (rule 8).** Today the game links no encryption, so `ITSAppUsesNonExemptEncryption` is false and App Store Connect asks nothing. Once M4 P4 links the patcher, the game carries `rustls` with `ring` for HTTPS to the patch host: standard TLS, but the game's own library, not iOS's. Apple's encryption page couldn't be read here (it renders client-side), so whether that stays exempt is the maintainer's to confirm on that page and in App Store Connect's export compliance questions, and S2's Info.plist comment says the same. Apple's overview says the developer carries the liability for claiming an exemption.
4. **The privacy policy's URL** (rule 6), from [privacy-policy.md](privacy-policy.md), after confirming the patch host keeps no access logs that identify a device. A link to it inside the game (rule 7) needs a later package.
5. **App Store Connect's forms:** the privacy answers, the age rating and the accessibility labels below; the store text and screenshots.
6. **The upload:** `xcodebuild -exportArchive -archivePath target/package/ios/SapphireGame.xcarchive -exportOptionsPlist platform/ios/ExportOptions.plist -exportPath <dir>` with the team set, then the upload from Organizer or Transporter, then TestFlight's internal testers (rules 29, 30).
7. **A person's look** at the screenshots for 4+ (rule 16) and at the fight for Guideline 1.1 (rule 20), and a native reader's at the Japanese.

## Screenshots

`cargo xtask store-shots` draws each shot at each size, in English and Japanese, into `target/store-shots/apple/<size>/<language>/`, never committed. The game is landscape only, so every size is landscape. Apple's page (summary) lists the 6.9" sizes as accepted and the 6.3" ones as the required medium display, scaled from the 6.9" set if missing: both are drawn, so either reading is covered. A shot stands for the device's screen: its pixels per point, and the touch controls a phone shows.

| Size | Pixels | Points | Tier | Controls | Source |
|---|---|---|---|---|---|
| `iphone-6.9` | 2868×1320 | 3 | mobile-high | touch | "Screenshot specifications", 6.9" (iPhone 17 Pro Max and others), landscape |
| `iphone-6.3` | 2622×1206 | 3 | mobile-high | touch | same, 6.3" (medium display), landscape |
| `ipad-13` | 2752×2064 | 2 | mobile-high | touch | same, 13" iPad, landscape; required because `UIDeviceFamily` has 2 |

The tier is the one a recent phone picks (spec §3). The shots, in order: the Market Court's fight, a boss's phase mark, the ward's boss on its terrace, the court gate's cutscene with its subtitles, the court as a continue stands it, and the ward's hub (`sapphire_testkit::store_shots::SHOTS`). The cutscene and the court are drawn in each language; the others have no words and are drawn once.

## Store text

Drafts. Each field's limit is in its heading; a test holds every draft to it. The Japanese is for a native reader's review.

#### English: name (30 characters)

```text
Sapphire
```

#### English: subtitle (30 characters)

```text
Sword action in an anime world
```

#### English: promotional text (170 characters)

```text
Fight through the Lantern Ward: dodge at the last moment, counter, and face the Knell Colossus. In English and Japanese, with no purchases and no ads.
```

#### English: keywords (100 bytes)

```text
action,anime,sword,fantasy,combat,boss,dodge,counter,offline,controller
```

#### English: description (4000 characters)

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
- Touch controls, or a game controller.
- Settings for text and interface size; subtitles, their size, background and speaker names; camera shake, screen flashes and camera motion; separate music, effects and voice volumes; mono sound; auto-target strength; and hold or toggle for each action you can hold.
- English and Japanese.

No purchases, no ads and no account. Plays offline.
```

#### Japanese: name (30 characters)

```text
サファイア
```

#### Japanese: subtitle (30 characters)

```text
アニメ調の剣戟アクション
```

#### Japanese: promotional text (170 characters)

```text
灯籠区を駆け抜け、ぎりぎりで攻撃をかわして反撃し、弔鐘の巨像に挑もう。英語と日本語に対応。課金も広告もありません。
```

#### Japanese: keywords (100 bytes)

```text
アクション,アニメ,剣,ファンタジー,ボス,回避,オフライン
```

#### Japanese: description (4000 characters)

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
・タッチ操作とゲームコントローラーに対応。
・文字とインターフェースの大きさ、字幕（大きさ、背景、話し手の名前）、画面の揺れ・フラッシュ・カメラの動き、音楽・効果音・音声それぞれの音量、モノラル音声、オートターゲットの強さ、長押しできる操作ごとの「長押し／切り替え」を設定できます。
・英語と日本語に対応。

課金、広告、アカウント登録はありません。オフラインで遊べます。
```

## App privacy

App Store Connect's "App Privacy" answers, from [the privacy policy's audit](privacy-policy.md#why-collects-nothing-holds):

- **Do you or your third-party partners collect data from this app?** No. The label reads "Data Not Collected".
- **Tracking:** none, so no App Tracking Transparency prompt.
- **Why:** nothing leaves the device but, once P4 lands, a patch download's requests, which carry no identifier. Apple doesn't count an IP address used to answer a request and not kept ("App privacy details", 2026-11-10). If the patch host keeps access logs, this answer changes to declare them.

## Age rating

App Store Connect's age rating questionnaire, as "Age ratings values and definitions" groups it (2026-11-10, summary: the questionnaire's own answer options weren't on the page, so the levels here are its "infrequent" and "frequent" or none). The maintainer enters it and reads the rating it gives.

| Group | Item | Answer | Why |
|---|---|---|---|
| In-app controls | Parental controls; age assurance | None | The game has neither |
| Capabilities | Unrestricted web access; user-generated content; social media; messaging and chat; advertising | No to each | No browser, no sharing, no chat, no ads |
| Violence | Cartoon or fantasy violence | Frequent | Combat is the game: sword fights against skeletons and armoured fantasy figures, in an anime art style |
| Violence | Realistic violence; prolonged graphic or sadistic violence; guns or other weapons | None; none; the sword is a fantasy weapon | No blood or gore; a defeated enemy falls |
| Mature themes | Profanity or crude humour; horror or fear themes; mature or suggestive themes; alcohol, tobacco or drug use | None; infrequent or mild (skeleton enemies); none; none | |
| Sexuality or nudity | All items | None | |
| Medical or wellness | All items | None | |
| Chance-based activities | Gambling; simulated gambling; contests; loot boxes | None | No purchases, no random rewards |

The page (summary) maps frequent cartoon or fantasy violence to 13+ in the new ratings, and infrequent to 9+. **Expected:** 13+, which a person confirms in App Store Connect.

## Accessibility Nutrition Labels

Apple asks, per device, whether every common task can be done with each feature ("Overview of Accessibility Nutrition Labels" and each feature's evaluation criteria, 2026-11-10, summary). The common tasks here: start or continue from the title, walk the ward, fight, watch a cutscene, talk to the guide, change settings. A feature is claimed only where a test proves it. The answers are the same on iPhone and iPad.

| Feature | Claim | Proved by, or why not |
|---|---|---|
| VoiceOver | No | No screen reader for menus (m4-plan §3, "Accessibility's scope") |
| Voice Control | No | Not built |
| Larger Text | No | Apple asks for text to grow to at least 200%. The text size reaches 125% on a phone's frame (`game_ui::screen::PHONE_TEXT_SCALE`): `the_text_size_reaches_every_screen_and_the_subtitles_theirs` and `every_screen_fits_at_the_largest_sizes_at_every_aspect` (`game_ui/tests/text_size.rs`) |
| Dark Interface | No | The ward is drawn in daylight; no dark mode |
| Differentiate Without Color Alone | **Yes** | Every warning a fight gives has its own shape (M4 A5): `sapphire_cues/tests/marks.rs`'s `every_pair_of_marks_stays_apart_under_each_dichromacy`, `a_named_phase_is_marked_on_the_enemy_that_crossed_it` and `every_perfect_dodge_had_its_window_marked_first`; the health bars' colour pair gets an inked edge (the `hud-*` goldens); the colour-vision view is held to the simulation by `the_color_vision_view_shows_each_pixel_as_the_simulation_does` (`sapphire_render`'s GPU tests) |
| Sufficient Contrast | No | No test measures the menus' contrast against Apple's 4.5:1 |
| Reduced Motion | **Yes** | Camera shake and camera motion are settings (M4 A1, A2): `the_players_effect_scale_reaches_the_screen_effects` (`game_combat`'s scene tests), `each_share_scales_its_own_effects` (`sapphire_look::post`), `the_defaults_change_nothing_and_each_setting_scales_what_it_names` (`game_ui::menus::settings`). Apple accepts an in-app setting in place of the system's. Reduced flashes cap at a quarter today; A3's flash limiter (owed: A3) makes them meet WCAG 2.3.1. The game doesn't yet read iOS's own Reduce Motion |
| Captions | **Yes** | Subtitles are on by default (`Subtitles::default`, read by the `game-ward-gate-*` goldens, which draw the game's own defaults); cutscenes: `a_cutscenes_subtitles_show_in_every_language_at_every_aspect` (`game_ui/tests/menus.rs`); fight voice lines: `every_voice_line_in_a_fight_has_a_caption` (`game_ui/tests/strings.rs`) and `a_said_line_is_captioned_while_its_time_lasts` (`game_combat::cues`). Sounds that warn of an attack are captioned by their shape on screen (A5), not by text |
| Audio Descriptions | No | Not in M4 |

Owed: players who use these features, judging whether the claims hold for them (m4-plan §7, "People").
