# Google Play: checklist and store text

- **Status:** M4 S5's checklist, 2026-11-10, for the AAB S3 builds (`cargo xtask package --platform android`). It serves exit test 3 ([m4-plan.md §1](../m4-plan.md#exit-tests)): every check that runs without an account runs in `package`, and the rest is owed to the maintainer (§7).
- **How to read it:** each rule names its source and the date it was read, then what meets it, or "owed". Play's help pages loaded as text, except where a row says "summary".
- **The app ID** `com.example.sapphire.game` and the name are S1's placeholders until the maintainer chooses them.
- **Related:** [the privacy policy](privacy-policy.md), [the App Store](apple.md), [Steam](steam.md).

**Count:** 29 rules: 20 met here (five of them as drafts the maintainer enters), 9 owed: 8 to the maintainer's account or Play's own checks (5, 6, 18, 23–27) and 1 to a person (16).

**Reviewed (M4 Z3b, 2026-11-11):** every met rule's test or command was found and its claim read against the code at d8b621cc. Rule 8 is met by the settings' privacy page, and the data safety answer is rechecked against P4's patcher.

## Rules

| # | Rule | Source, read | Met by | Status |
|---|---|---|---|---|
| 1 | New apps ship as an Android App Bundle. | `google-play.md` research note and S3, 2026-11-10 | `package --platform android` builds the AAB with `bundleRelease --offline` and runs `bundletool validate` (S3, `xtask/src/platforms/aab.rs`) | Met |
| 2 | New apps and updates target API 36 or higher, since 2026-08-31. | "Target API level requirements", developer.android.com/google/play/requirements/target-sdk, 2026-11-10 | `targetSdk = 36` (`platform/android/app/build.gradle.kts`); the badging check in `package` reads it back | Met; the next level (37) isn't announced on that page |
| 3 | Native code supports 16 KB pages for targets 35 and up; updates that don't are blocked from 2027-02-01. | "Support 16 KB page sizes", developer.android.com/guide/practices/page-sizes, 2026-11-10 | `package` checks every `LOAD` segment's 16 KB alignment and `zipalign -c -P 16` on each APK (S3) | Met |
| 4 | The base module is at most 500 MB compressed. | "Maximum size limits", Play Console Help, 2026-11-10 (S3) | `bundletool get-size total` in `package`: about 35 MB a device | Met |
| 5 | Uploads are signed with an upload key; Play App Signing holds the app's key. | S3, 2026-11-10 | `SAPPHIRE_UPLOAD_KEYSTORE` and its passwords sign the release (S3); without them it's the debug key, which `package` warns Play refuses | Owed: the maintainer's key |
| 6 | Every app posts a privacy policy URL in Play Console, even one that accesses no personal data. | "User Data" policy, support.google.com/googleplay/android-developer/answer/10144311, 2026-11-10 | [privacy-policy.md](privacy-policy.md), a draft naming the developer, a contact, what's collected (nothing), and retention and deletion | Owed: the maintainer hosts it at a public, non-PDF URL |
| 7 | The policy names the developer or the app as the listing does. | same | The draft's `[developer name]` and `[Game name]` | Met (draft) |
| 8 | A privacy policy link or text inside the app too. | same | The settings' Privacy policy page shows what the game collects and the policy's address (M4 Z3b), as [Apple's rule 7](apple.md#rules) | Met; the address is the maintainer's (rule 6) |
| 9 | The data safety form, completed even by an app that collects nothing, with the policy's link. | "Provide information for Google Play's Data safety section", answer/10787469, 2026-11-10 (the first half verbatim, the rest a summary) | [Data safety](#data-safety) below | Met (draft); the maintainer enters it |
| 10 | A content rating from IARC's questionnaire; an app without one is removed. | "Content ratings", answer/9898843, and "Complete the content rating questionnaire", answer/9859655, 2026-11-10 | [Content rating](#content-rating) below | Met (draft); the maintainer enters it |
| 11 | Target audience and content: the age groups the app is for. | "Target audience and content", answer/9867159, 2026-11-10 | [Target audience](#target-audience-and-content) below: 13 and over, not designed for children | Met (draft) |
| 12 | The ads declaration and app access instructions. | "Prepare your app for review", answer/9859455, 2026-11-10 | No ads; nothing is behind a sign-in | Met (draft) |
| 13 | A permissions declaration for high-risk or sensitive permissions. | same | The badging lists `VIBRATE` and `INTERNET` only, both normal permissions (S3's `checks/badging.txt`); the manifest says why it asks for `INTERNET`: the patcher | Met: none to declare |
| 14 | Store listing: name 30 characters, short description 80, full description 4000, full- and half-width characters counting alike. | "Add preview assets to showcase your app" and "Create and set up your app", answer/9859152, 2026-11-10 | [Store text](#store-text), held to the limits by `store_shots::tests::each_stores_text_fits_its_limits` | Met |
| 15 | Metadata: no ranking or award claims, prices or promotions, no unattributed testimonials, no emoji, no calls to action or words like "best", "#1", "top" or "new" in the short description. | "Metadata" policy, answer/9898842, 2026-11-10 | The drafts say what's built and claim nothing else | Met |
| 16 | The drafts read well to a Japanese player. | m4-plan §7 | | Owed: a native reader |
| 17 | Screenshots: 2 to 8 per device type, JPEG or 24-bit PNG with no alpha, 320 to 3840 px a side, the long side at most twice the short. Large screens: at least 4, 1080 to 7680 px, 16:9. A game's recommendations want at least 3 landscape at 1920×1080 or more. | "Add preview assets", answer/9866151, 2026-11-10 | `cargo xtask store-shots`: six RGB PNGs per size in [Screenshots](#screenshots); `store_shots::tests` holds the sizes to this doc and Play's bounds | Met |
| 18 | A feature graphic, 1024×500, to publish. | same | Not made: it's key art | Owed: the maintainer's art |
| 19 | A 512×512 icon, 32-bit PNG. | same | S1's `icons` makes Play's 512 px icon from `platform/icon/icon.png` | Met: the icon is the maintainer's own (2026-11-13) |
| 20 | Games declare their category, which also keeps Android 16's large-screen orientation rules off them. | "Android 16 behaviour changes" and the adaptive-apps guide, as the scouts' note gives them (2026-11-10) | `android:appCategory="game"` in the manifest | Met |
| 21 | Edge to edge and Back at target API 36. | "Behavior changes: apps targeting Android 16", 2026-11-10 (S3) | S3 on the emulator: drawn edge to edge with a cutout emulated; `KEYCODE_BACK` reaches the game | Met on the emulator; a phone's back gesture owed (S3) |
| 22 | Release lint passes; the bundle and APKs are valid and aligned. | S0, S3 | Lint fails the package on error; `bundletool validate`, `build-apks --local-testing`, `zipalign`, `aapt2 dump badging` (S3) | Met |
| 23 | A Play Console account and the app record. | m4-plan §7 | | Owed: the maintainer's account |
| 24 | A new personal account runs a closed test with 12 testers opted in for 14 days in a row before Production. | "App testing requirements for new personal developer accounts", answer/14151465, 2026-11-10 | | Owed: the maintainer's testers |
| 25 | The internal testing track. | `google-play.md` research note (unverified) | `checks/local-testing.apks` installs on the emulator (S3) as the nearest stand-in | Owed: the maintainer's upload |
| 26 | Play's pre-launch report. | `google-play.md` research note (unverified) | Only Google's device lab runs it | Owed: the upload |
| 27 | Review. | | | Owed: Play's review |
| 28 | Native symbols for Play's crash reports. | developer.android.com/build/include-native-symbols, 2026-11-10 (S3) | `debugSymbolLevel = "FULL"`: the AAB carries `libgame_app.so.dbg` (S3) | Met |
| 29 | Families policy, for apps aimed at children. | "Target audience and content", 2026-11-10 | Not designed for children (rule 11) | Met: not needed |

## Owed to the maintainer

1. **The identity:** the app ID (permanent once uploaded) and the name. The icon is the maintainer's own since 2026-11-13.
2. **The account:** Play Console, the app record, Play App Signing and the upload key, which enters `package` through `SAPPHIRE_UPLOAD_KEYSTORE` (rules 5, 23).
3. **The privacy policy's URL** (rule 6), from [privacy-policy.md](privacy-policy.md), once the patch host's logs are settled, and the same address as `privacy-url` in the game's strings (rule 8, [apple.md](apple.md#owed-to-the-maintainer) item 4).
4. **Play Console's App content page:** the data safety form, the content rating questionnaire, target audience, ads and app access, from the drafts below (rules 9–12).
5. **The listing:** the drafts below, the screenshots, and a feature graphic (rule 18).
6. **Testing:** the internal track, then a closed test of 12 testers for 14 days on a new personal account, then Production (rules 24, 25).

## Screenshots

`cargo xtask store-shots` draws each shot at each size, in English and Japanese, into `target/store-shots/google-play/<size>/<language>/`, never committed. Play's page gives no separate sizes for 7" and 10" tablets, only its large-screen bounds, so one tablet set serves both. A shot stands for the device's screen: its pixels per point and its touch controls.

| Size | Pixels | Points | Tier | Controls | Source |
|---|---|---|---|---|---|
| `phone` | 1920×1080 | 3 | mobile-high | touch | answer/9866151: a game's landscape screenshots at 1920×1080 or more; a phone at xxhdpi |
| `tablet` | 2560×1440 | 2 | mobile-high | touch | same: large screens, 16:9, 1080 to 7680 px; a tablet at xhdpi |

## Store text

Drafts. Each field's limit is in its heading; a test holds every draft to it. The Japanese is for a native reader's review.

#### English: name (30 characters)

```text
Sapphire
```

#### English: short description (80 characters)

```text
3D sword action in an anime art style: dodge, counter, and face the ward's boss.
```

#### English: full description (4000 characters)

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

#### Japanese: short description (80 characters)

```text
アニメ調の3D剣戟アクション。攻撃をかわして反撃し、灯籠区のボスに挑もう。
```

#### Japanese: full description (4000 characters)

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

## Data safety

Play Console's data safety form, from [the privacy policy's audit](privacy-policy.md#why-collects-nothing-holds). Play's help (answer/10787469, 2026-11-10) says "collect" means sending data off the device, and that an app collecting nothing still completes the form and links its policy.

| Question | Answer | Why |
|---|---|---|
| Does your app collect or share any of the required user data types? | No | The shipped game sends nothing off the device. Crash reports, saves and settings stay on it. |
| Is all of the user data collected by your app encrypted in transit? | Not asked once the answer above is No | |
| Do you provide a way for users to request that their data is deleted? | Not asked once the answer above is No | Uninstalling deletes everything the game keeps |
| Privacy policy | [privacy-policy.md](privacy-policy.md), at the maintainer's URL | |

**Rechecked after P4 (M4 Z3b, 2026-11-11).** A build checks for a patch only once `platform/patch.toml` names a host, and today none is named, so the answer is "No". With a host, the download sends requests with no identifier, but Play's help says approximate location inferred from an IP address must be declared, and that data processed only to answer a request in real time is still entered in the form though not shown on the listing (the help's verbatim half; its summarized half reads otherwise, so read the live page then). If the patch host keeps no logs, the answer stays "No"; if it keeps them, "Approximate location" and "Device or other IDs" need weighing.

## Content rating

IARC's questionnaire in Play Console. Play's pages (answer/9898843 and 9859655, 2026-11-10) give its steps (an email for IARC, a category, the questions) but not the questions themselves, so these answers are by topic, from what the game does; the maintainer maps them onto the questions as Console asks them.

| Topic | Answer |
|---|---|
| Category | Game |
| Violence | Yes: frequent fantasy combat. The player's character fights skeleton warriors, a lancer, a captain and an armoured colossus with a sword, in an anime art style. No blood, no gore, no dismemberment; a defeated enemy falls. No violence against realistic humans or animals. |
| Fear | Mild: skeleton enemies. No horror themes. |
| Sexuality and nudity | None |
| Language | None |
| Controlled substances | None |
| Gambling and simulated gambling | None |
| Crude humour | None |
| User interaction: chat, user-generated content, sharing location, interacting with others | None: single player, offline |
| Digital purchases | None |
| Ads | None |

**Expected:** PEGI 7 or 12, ESRB Everyone 10+ or Teen, for fantasy violence; IARC decides.

## Target audience and content

Play's age groups (answer/9867159, 2026-11-10): **13–15, 16–17 and 18 and over.** "Not designed for children": no ads, no data collected, but sword combat throughout. Play warns that youthful-looking characters can make an app read as aimed at children: the store text and shots show combat throughout, and how the character reads is for a person to judge (rule 16's reader, or the maintainer). **Ads:** none. **App access:** all of the game is available with no sign-in.
