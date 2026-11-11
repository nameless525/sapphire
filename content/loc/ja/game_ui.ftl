### ゲームが表示するすべての文字列 (M3 U7、C3 で集約)。
###
### Machine-quality Japanese: written by an agent and not yet read by a Japanese reader, who owes every
### line here a pass. Take this mark off only once they have. `en/game_ui.ftl` says how a string is added.

window-title = サファイア

title-name = サファイア
title-tagline = はじめての戦い
title-continue = つづきから
title-new = はじめから
title-settings = 設定
title-leave = 終了

pause-title = 一時停止
pause-resume = 再開
pause-character = キャラクター
pause-settings = 設定
pause-to-title = タイトルへ戻る
pause-leave = 終了
# Only a playtest build's pause shows it (M4 F1).
pause-mark = この場面に印をつける

settings-title = 設定
settings-language = 言語
settings-language-en = English
settings-language-ja = 日本語
settings-display = 画面
settings-graphics = グラフィック
settings-sound = サウンド
settings-subtitles = 字幕
settings-controls = 操作
settings-more = ›
settings-back = 戻る
settings-switch-on = オン
settings-switch-off = オフ
settings-percent = { $percent }%

display-title = 画面
display-size = 表示サイズ
display-size-small = 小
display-size-normal = 標準
display-size-large = 大
display-text = 文字サイズ
display-text-normal = 標準
display-text-large = 大
display-text-larger = 特大
display-shake = 画面の揺れ
display-flashes = フラッシュ
display-flashes-normal = 標準
display-flashes-reduced = 軽減
display-camera = カメラの動き
display-timing = タイミングアシスト
display-timing-off = オフ
display-timing-wide = 広め
display-timing-wider = さらに広め
display-back = 戻る

graphics-title = グラフィック
graphics-quality = 画質
graphics-quality-automatic = 自動
graphics-quality-high = 高
graphics-quality-low = 低
graphics-cap = フレームレート上限
graphics-cap-30 = 30 fps
graphics-cap-60 = 60 fps
graphics-cap-120 = 120 fps
graphics-cap-unlimited = 無制限
graphics-dynamic = 解像度の自動調整
graphics-saver = バッテリー節約
graphics-back = 戻る

notice-title = 光過敏性発作について
notice-body = ごく一部の方が、強い光の点滅や模様を見て発作を起こすことがあります。このゲームの戦闘には光の点滅があります。ご本人やご家族に発作やてんかんの経験がある方は、遊ぶ前に医師に相談してください。めまい、見え方の変化、目や筋肉のけいれんを感じたら、すぐに中止してください。
notice-reduced = 戦闘で画面全体が点滅する回数は抑えてあります。フラッシュを「軽減」にすると、その点滅も暗く抑えます。あとで「設定」の「画面」から変えられます。
notice-flashes = フラッシュ
notice-continue = 続ける

sound-title = サウンド
sound-volume = 全体の音量
sound-music = 音楽
sound-effects = 効果音
sound-voice = ボイス
sound-mono = モノラル音声
sound-rumble = 振動
sound-back = 戻る

subtitles-title = 字幕
subtitles-shown = 字幕表示
subtitles-size = 文字サイズ
subtitles-size-small = 小
subtitles-size-normal = 標準
subtitles-size-large = 大
subtitles-size-larger = 特大
subtitles-background = 下地
subtitles-background-off = なし
subtitles-background-light = 弱め
subtitles-background-normal = 標準
subtitles-background-solid = 強め
subtitles-speakers = 話者名
subtitles-back = 戻る

controls-title = 操作
controls-auto-target = タッチのオートターゲット
controls-auto-target-weak = 弱
controls-auto-target-normal = 標準
controls-auto-target-strong = 強
controls-held = 長押しかトグルか
controls-hold-light = 攻撃
controls-hold-heavy = 強攻撃
controls-hold-dodge = 回避
controls-hold-skill = スキル
controls-hold-lock-on = ロックオン
controls-held-hold = 長押し
controls-held-toggle = トグル
controls-back = 戻る
controls-remap = ボタンとキー

remap-title = ボタンとキー
remap-tab-gamepad = コントローラー
remap-tab-keyboard = キーボード
remap-action-move-forward = 前へ移動
remap-action-move-back = 後ろへ移動
remap-action-move-left = 左へ移動
remap-action-move-right = 右へ移動
remap-action-light = 攻撃
remap-action-heavy = 強攻撃
remap-action-dodge = 回避
remap-action-skill = スキル
remap-action-ultimate = 必殺技
remap-action-swap = キャラクターチェンジ
remap-action-lock-on = ロックオン
remap-action-jump = ジャンプ
remap-action-pause = 一時停止
remap-action-look = カメラ
remap-glyph = { $glyph }
remap-reset = 初期設定に戻す
remap-back = 戻る
remap-press-gamepad = ボタンを押してください:
remap-press-keyboard = キーを押してください:
remap-cancel = { $glyph } でキャンセル
remap-cancel-row = キャンセル
remap-kept = { $glyph } は次の操作に使われています:
remap-taken-title = すでに使われています
remap-taken = { $glyph } はすでに次の操作に使われています:
remap-swap = 入れ換える
remap-keep = 元のままにする

character-title = キャラクター
character-name-wanderer = 旅人
character-level = Lv. { $level }
character-hp = HP
character-hp-value = { NUMBER($hp) } / { NUMBER($max) }
character-attack = 攻撃力
character-defense = 防御力
character-crit = 会心率
character-number = { NUMBER($value) }
character-percent = { $value }%
character-skill-attack = 連撃
character-skill-dodge = 回避
character-skill-art = スキル
character-detail-attack = 押すと斬りつけ、続けて押すと連撃になる。長押しで力をため、ガードを崩す重い一撃を放つ。
character-detail-dodge = 攻撃をかわす。当たる瞬間に合わせると、一瞬だけ世界が遅くなる。
character-detail-art = 強力な一撃。使うと再使用まで待ち時間があり、残り秒数は HUD に表示される。
character-back = 戻る

dialogue-next = 次へ
dialogue-sample-keeper = 港の番人
dialogue-sample-line = 嵐は夜のうちに過ぎ、最初の舟はもう灯台の先へ出ている。鐘が鳴るまで、一緒に見張りをしてくれないか。
dialogue-sample-stay = 鐘が鳴るまでここにいよう。
dialogue-sample-ask = 何を見張っているんだ？
dialogue-sample-go = まだ終わらせる戦いがある。
dialogue-sample-later = また明日にしてくれ。

hud-level = Lv. { $level }
hud-objective = 目標
hud-objective-arena = 敵を倒せ
hud-pause = 一時停止
hud-skip = スキップ
hud-banner-checkpoint = チェックポイント到達
hud-banner-victory = 勝利
hud-banner-defeat = 敗北

enemy-warrior = 骸骨の戦士
enemy-lancer = 槍使い

boss-captain = 骸骨の隊長
boss-knell-colossus = 弔鐘の巨像

boot-step-shaders = シェーダーを準備中
boot-step-loading = 読み込み中
boot-step-update-check = アップデートを確認中
boot-step-update = アップデートをダウンロード中
boot-step-update-install = アップデートをインストール中
boot-percent = { $percent }%

wipe-loading = 読み込み中
