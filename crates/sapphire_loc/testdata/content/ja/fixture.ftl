# The same strings in Japanese. Japanese has one plural category, so `other` is all a count can choose.
# `only-english`, `only-english-count` and `only-english-brand` are missing on purpose: the cook's report
# lists them.

-brand = サファイア
-realm = { $case ->
    [inner] 内なる領域
   *[other] 領域
}
-sword = 剣
    .gender = neuter

title = メインメニュー
save = セーブ
    .tooltip = 進行状況をセーブします
    .shortcut = Ctrl+S
only-attributes =
    .label = 属性だけ

welcome = { -brand }へようこそ、{ $name }さん！
welcome-back = { welcome } 幸運を。
tooltip-of-save = { save.shortcut }でセーブします。
realm-name = { -realm }
inside = { -realm(case: "inner") }にいます。
sword-taken = { -sword.gender ->
    [neuter] あなたのものです
   *[other] あなたたちのものです
}

enemies-left = { $count ->
    [one] 使われないはずの語形
   *[other] 残りの敵：{ $count }体
}
lives = { $lives ->
    [0] 残機なし
    [1] 残機あと一つ
   *[other] 残機：{ $lives }
}
score = スコア：{ $points }
damage = { NUMBER($amount, minimumFractionDigits: 1, maximumFractionDigits: 1) }ダメージ
seed = シード { NUMBER($seed, useGrouping: "false") }
coins = ちょうど{ 1000 }コイン
price = 各{ NUMBER($price, minimumFractionDigits: 2) }
stance = { $stance ->
    [high] 上段の構え
    [low] 下段の構え
   *[other] 構えなし
}
braces = 記号 { "{" } と { "}" } と { "あ" }
