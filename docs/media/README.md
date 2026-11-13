# Media

The images on the [README](../README.md), and how each was made. Every one is a frame the engine drew. The sandbox rendered them offscreen on Metal at the Desktop-High tier, at 2560×1440 (3200×1372 for the hero), from `main` at 7845aeac. Each was then resized to 1800 or 1200 px wide and saved as a JPEG. Nothing was painted over.

To render one again, from the repository root:

```bash
cargo xtask content --cook desktop
cargo build --release -p sapphire_sandbox --locked
SAPPHIRE_CONTENT_DIR=$PWD/target/content/desktop/ target/release/sapphire_sandbox --capture <file>.png --size 2560x1440 <arguments below>
```

- **Ambient occlusion:** the tier's own. The goldens pin it off with `--ao off`, and these don't.
- **UI scale:** an offscreen capture draws the UI at one pixel per point. So `--ui-scale 2` and `--display-scale 2` give the menus and the HUD the size they'd have on a 2× display.

| File | Arguments | Shows |
|---|---|---|
| `hero.jpg` | `--size 3200x1372 --capture-ticks 90 --look-dev street --time-of-day 21 --tier desktop-high --capture-frames 32` | The lantern street at 21:00 |
| `feature-face.jpg` | `--capture-ticks 90 --look-dev face --time-of-day 17.5 --tier desktop-high --capture-frames 32` | Anime face shading: the SDF face shadow, eye and hair highlights, outlines |
| `feature-day.jpg` | `--capture-ticks 90 --look-dev wide --time-of-day <13, 18.5, 19.75, 23> --tier desktop-high --capture-frames 32` | One camera at four hours, labelled in Oxanium |
| `feature-combat.webp` | `--size 960x540 --capture-ticks 300 --cues --arena --replay tests/replays/arena-skirmish.sreplay --time-of-day 17.5 --tier desktop-high --blade on --capture-frames 300 --capture-step 1`, with `SAPPHIRE_CAPTURE_SEQUENCE=<dir>` | The skirmish at golden hour. Every second frame from 40 to 250, resized to 800 px and encoded with `img2webp -loop 0 -lossy -q 45 -m 6 -d 33` |
| `feature-story.jpg` | `--capture-ticks 90 --look-dev face --time-of-day 17.5 --tier desktop-high --capture-frames 32 --ui-game dialogue --ui-locale ja --ui-scale 2` | The dialogue panel and replies in Japanese |
| `feature-game.jpg` | `--capture-ticks 90 --look-dev hero --time-of-day 18 --tier desktop-high --capture-frames 32 --ui-game title --ui-locale ja --ui-scale 2` | The title menu in Japanese |
| `feature-mobile.jpg` | `--capture-ticks 670 --cues --hud live --arena --replay tests/replays/arena-boss.sreplay --time-of-day 10 --tier desktop-high --blade on --capture-frames 32 --display-scale 2` | The boss fight on the touch HUD, past its enrage phase |
| `gallery-aurora.jpg` | `--capture-ticks 90 --look-dev sky --time-of-day 23 --tier desktop-high --capture-frames 32` | The square at night, under the aurora |
| `gallery-effects.jpg` | `--capture-ticks 120 --look-dev hero --time-of-day 21 --tier desktop-high --capture-frames 16 --capture-step 1 --vfx duel` | The duel's sword arc, sparks, flares and glints |
| `gallery-impact.jpg` | `--capture-ticks 132 --look-dev hero --time-of-day 12 --tier desktop-high --capture-frames 16 --capture-step 1 --replay tests/replays/fight.sreplay --cues` | The impact frame of a guard-breaking hit |
| `sapphire-icon.png` | | Sapphire's icon, the maintainer's artwork. The app icon's master is [`platform/icon/icon.png`](../../platform/icon/icon.png) |
