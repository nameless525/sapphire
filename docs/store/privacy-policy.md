# The game's privacy policy (draft)

- **Status:** a draft for the maintainer (M4 S5, 2026-11-10). Nothing here is published. Every store asks for a policy at a public URL, even for a game that collects nothing: Apple ("App information", read 2026-11-10), Google Play (User Data policy, read 2026-11-10) and Steam's store page all link one. Apple and Play also want it reachable inside the game (App Review Guideline 5.1.1(i); Play's User Data policy), which the game doesn't do yet: see [apple.md](apple.md#owed-to-the-maintainer).
- **What the maintainer fills in:** the bracketed names. The game's name is S1's placeholder until the maintainer chooses one (m4-plan §7, "The identity").
- **The Japanese** is a draft for a native reader's review (m4-plan §7, "People").

## Why "collects nothing" holds

Each claim below was checked against the tree at S5's base (m4-stack-s4, 2026-11-10). If any of these change, so must the policy, the App Store's privacy answers ([apple.md](apple.md#app-privacy)) and Play's data safety form ([google-play.md](google-play.md#data-safety)).

| Claim | Checked in |
|---|---|
| The shipped game makes no network connection. | `cargo tree -p game_app -e normal -i ureq` finds no HTTP client in the game's link; no crate in `game/` or the runtime's `crates/` that the game links opens a socket. The live link (`sapphire_livelink`) is behind the `dev` feature, which no store package builds. |
| Crash reports stay on the device. | `sapphire_crash`'s crate docs: reports are written to the player's directory and pruned to the newest 8 runs; uploading was removed in 2026-10 and nothing sends them. |
| Saves and settings stay on the device. | `sapphire_save` and `game_ui::menus::settings` write under the platform's user directory. Android's manifest turns cloud backup off (`allowBackup="false"`), and its transfer rules move only the user directory. |
| No tracking, no advertising, no analytics, no third-party SDK that collects. | The privacy manifest (`platform/ios/PrivacyInfo.xcprivacy`): `NSPrivacyTracking` false, no collected data types, no tracking domains. No SDK of that kind is a dependency (`Cargo.lock`, `platform/android/app/build.gradle.kts`). |
| Playtest sessions are only in playtest builds. | M4 F1, on its own branch at S5's base: sessions are kept by `sapphire_playtest`, a `dev_only` crate that only `game_app --features playtest` links, so a store build has none. They're local files, brought back over a cable by `xtask playtest pull`. Recheck this row when F1 lands. |
| A patch download, when a later version turns it on, sends no identifier. | `sapphire_patch::http`: one HTTP GET per file of the release, with a `Range` header for the bytes it lacks. ureq 3.5.1 adds `Host`, `User-Agent: ureq/3.5.1` and `Accept: */*`, and no cookie (read from its source, 2026-11-10). No account, device ID or setting is sent. M4 P4 links it into the game; until then the game has no network code at all. |

**The one condition:** the patch host, like any web server, sees the device's IP address with each request. Apple doesn't count an IP address that's used to answer the request and not kept ("App privacy details on the App Store", read 2026-11-10), but it does count one that's logged and stored. Play counts approximate location inferred from an IP address (Play's data safety help, read 2026-11-10). So "collects nothing" stays true only if the maintainer's host keeps no access logs, or keeps none that identify a device after the request is answered. That is the maintainer's choice of host (m4-plan §7, item 4), and the policy below states it as a condition to confirm.

## English

> **Privacy policy for [Game name]**
>
> Last updated: [date]
>
> [Game name] is made by [developer name]. This policy says what the game does with information about you. In short: it collects nothing.
>
> **What we collect.** Nothing. The game has no account, no sign-in, no advertising, no analytics and no tracking. It doesn't send us, or anyone else, any information about you or your device.
>
> **What stays on your device.** The game keeps your save, your settings and, if the game ever crashes, a crash report, in its own folder on your device. Crash reports are never sent anywhere; the game keeps the latest few and deletes older ones. Uninstalling the game deletes all of this.
>
> **Updates.** [Leave out this paragraph until the game downloads updates.] The game may download content updates from [update host]. To do this, it asks for the update's files, naming only which parts of them it needs. It sends no identifier, account or information about you. Like any website, the server that answers sees your device's internet address; [developer name] keeps no record of it after the request is answered.
>
> **The store you got the game from.** Apple, Google or Valve may collect information under their own privacy policies, for example when you buy or download the game, or if you choose to share crash reports with developers in your device's settings. Their policies apply to that.
>
> **Children.** The game isn't directed at children under 13, and it collects nothing from anyone of any age.
>
> **Changes.** If this policy changes, the new version will be posted here with a new date.
>
> **Contact.** [contact address]

## 日本語（草稿、ネイティブの確認待ち）

> **[ゲーム名] プライバシーポリシー**
>
> 最終更新日：[日付]
>
> [ゲーム名] は [開発者名] が制作しています。このポリシーでは、ゲームがお客様の情報をどう扱うかを説明します。要点：ゲームは情報を一切収集しません。
>
> **収集する情報** ありません。アカウント、ログイン、広告、分析、トラッキングはありません。お客様やお使いの端末に関する情報を、当方にも第三者にも送信しません。
>
> **端末に保存されるもの** セーブデータ、設定、そしてゲームが異常終了した場合のクラッシュレポートを、端末内のゲーム専用フォルダに保存します。クラッシュレポートはどこにも送信されません。新しいものを数件だけ残し、古いものは削除します。ゲームをアンインストールすると、これらはすべて削除されます。
>
> **アップデート** [ゲームがアップデートをダウンロードするようになるまで、この段落は載せません。] ゲームは [アップデートの配信元] からコンテンツのアップデートをダウンロードすることがあります。その際は、必要なファイルとその範囲だけを指定して取得します。識別子、アカウント、お客様に関する情報は送信しません。一般的なウェブサイトと同じく、応答するサーバーには端末のインターネットアドレスが届きますが、[開発者名] は応答後にその記録を残しません。
>
> **ゲームを入手したストア** Apple、Google、Valve は、ゲームの購入やダウンロードの際、または端末の設定で開発者へのクラッシュレポート共有を選んだ場合などに、それぞれのプライバシーポリシーに基づいて情報を収集することがあります。その扱いには各社のポリシーが適用されます。
>
> **お子様について** 本ゲームは13歳未満のお子様を対象としていません。また、年齢にかかわらず、どなたからも情報を収集しません。
>
> **変更について** このポリシーを変更する場合は、新しい日付とともにこのページに掲載します。
>
> **お問い合わせ** [連絡先]
