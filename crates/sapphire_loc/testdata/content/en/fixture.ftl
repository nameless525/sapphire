# Test strings for `sapphire_loc`, exercising every feature the evaluator runs. Not a game's: C3 writes
# those. They live here rather than under content/, so no pack holds them.

-brand = Sapphire
-realm = { $case ->
    [inner] the inner realm
   *[other] Realm
}
-sword = Sword
    .gender = neuter

title = Main Menu
save = Save
    .tooltip = Save your progress
    .shortcut = Ctrl+S
only-attributes =
    .label = Only an attribute

welcome = Welcome to { -brand }, { $name }!
welcome-back = { welcome } Good luck.
tooltip-of-save = Press { save.shortcut } to save.
realm-name = { -realm }
inside = You are in { -realm(case: "inner") }.
sword-taken = { -sword.gender ->
    [neuter] It is yours
   *[other] They are yours
}

enemies-left = { $count ->
    [one] One enemy left
   *[other] { $count } enemies left
}
lives = { $lives ->
    [0] No lives left
    [1] Last life
   *[other] { $lives } lives
}
score = Score: { $points }
damage = { NUMBER($amount, minimumFractionDigits: 1, maximumFractionDigits: 1) } damage
seed = Seed { NUMBER($seed, useGrouping: "false") }
coins = Exactly { 1000 } coins
price = { NUMBER($price, minimumFractionDigits: 2) } each
stance = { $stance ->
    [high] High guard
    [low] Low guard
   *[other] No guard
}
braces = Literal { "{" } and { "}" } and { "あ" }

only-english = This has no Japanese yet
only-english-count = { $count ->
    [one] { $count } line, in English
   *[other] { $count } lines, in English
}
only-english-brand = { -brand } stays English here
