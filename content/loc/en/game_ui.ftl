### Every string the game draws (M3 U7, gathered in C3), in English: what every other language follows.
###
### Adding one:
### 1. Add its key here and in `ja/game_ui.ftl`, in the same place. A key reads `<screen>-<what>`. Values a
###    model chooses at run time (a name, a line of dialogue, an objective, a subtitle) are whole keys under
###    one prefix, which the document's `key_from` names: `dialogue-`, `hud-objective-`.
### 2. Write the Japanese too, as well as you can. It stays under that file's machine-quality mark until a
###    Japanese reader has read every line: an agent's Japanese is owed a person's pass.
### 3. Name the key in a document (`text: (key: ...)`), or let a `key_from` prefix cover it. Nothing the game
###    draws is written anywhere else: no words in code, and a variable is only ever a number. A boss's name
###    reads `boss-<kind>`, and its `.enemy.ron` names it (`boss: Some((name: ...))`) for its boss bar.
### 4. Add any Han character the Japanese uses that `content/fonts/ui.charset.ron` lacks.
### 5. Run `cargo test -p game_ui --test strings`, which holds all of this, and `--test menus`, which walks
###    every screen in both languages and under pseudo-localization.
### A new file of strings goes beside this one, in both languages, each with its own `.meta` sidecar.
### A cutscene's lines and speakers go in `cutscenes.ftl` instead, named by its timeline's `subtitles`; the
### same test holds them.

title-name = Sapphire
title-tagline = The first fight
title-continue = Continue
title-new = New game
title-settings = Settings
title-leave = Quit

pause-title = Paused
pause-resume = Resume
pause-character = Character
pause-settings = Settings
pause-to-title = Back to the title
pause-leave = Quit

settings-title = Settings
settings-language = Language
settings-language-en = English
settings-language-ja = 日本語
settings-size = Interface size
settings-size-small = Small
settings-size-normal = Normal
settings-size-large = Large
settings-sound = Volume
settings-sound-value = { $percent }%
settings-rumble = Vibration
settings-rumble-on = On
settings-rumble-off = Off
settings-back = Back

character-title = Character
character-name-wanderer = Wanderer
character-name-skeleton = Skeleton Warrior
character-level = Lv. { $level }
character-hp = HP
character-hp-value = { NUMBER($hp) } / { NUMBER($max) }
character-attack = Attack
character-defense = Defense
character-crit = Critical rate
character-number = { NUMBER($value) }
character-percent = { $value }%
character-skill-attack = Combo
character-skill-dodge = Dodge
character-skill-art = Skill
character-detail-attack = Press to strike, again to carry on. Hold to wind up a heavy blow that breaks a guard.
character-detail-dodge = Slip out of the way. Timed just as a blow lands, the world slows for a moment.
character-detail-art = A strong strike with a wait before it comes back. The HUD counts the seconds down.
character-back = Back

dialogue-next = Next
dialogue-sample-keeper = Harbor Keeper
dialogue-sample-line = The storm passed in the night, and the first boats are out past the lighthouse. Will you stand watch with me until the bells?
dialogue-sample-stay = I'll stay until the bells.
dialogue-sample-ask = What are you watching for?
dialogue-sample-go = I have a fight to finish.
dialogue-sample-later = Ask me again tomorrow.

hud-level = Lv. { $level }
hud-objective = Objective
hud-objective-arena = Defeat the enemy
hud-pause = Pause
hud-skip = Skip
hud-banner-checkpoint = Checkpoint reached
hud-banner-victory = Victory
hud-banner-defeat = Defeated

boss-captain = Skeleton Captain
boss-knell-colossus = Knell Colossus
