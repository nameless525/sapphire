# Voice over

The game's voiced lines (M6 U11), one folder a language. Not third-party: every master here is the
project's own work, synthesized by [`tools/sapphire_soundgen`](../../tools/sapphire_soundgen) from a spec
each in [`tools/sounds/`](../../tools/sounds), with no recordings, samples, downloads or text-to-speech
voices. The voices are formant babble in each language's syllable shapes, with no words: placeholders
until a studio records the lines from each language's `script.csv`. Licensed with the rest of the
workspace.

| | |
|---|---|
| **Author** | The Sapphire project |
| **Made by** | `cargo run -p sapphire_soundgen` (the masters), then `cargo run -p sapphire_pipeline -- vo --content content` (everything else), from the workspace root |
| **Made on** | macOS on arm64; the lines first 2026-11-09 and 2026-11-13 (M3 A1, M5 C6), as masters 2026-11-17 (UTC) |
| **Format** | Masters: WAV, mono, 24 kHz, 32-bit float. Encoded: Ogg Vorbis, quality 0.3 (`vorbis_rs`, a fixed stream serial for each file) |
| **License** | The workspace's |

## What is here

- `script.ron`, written by hand: every voiced line, its text and speaker as Fluent messages, its context and
  direction, and which languages' takes are approved.
- `<language>/<key>.wav`: each line's master, which a studio's take replaces.
- Written by `sapphire_pipeline vo`, never by hand: each master's `<key>.ogg` and its line's
  `<key>.cue.ron`, with sidecars; `<language>.voice.ron`, the language's voice lines; `<language>/script.csv`,
  the recording script; `<language>/import.csv`, the import report, which records each master's and Ogg's
  BLAKE3.

A master is a pure function of its spec, its samples exactly as synthesized; its Ogg is libvorbis's encode
of it, which another OS or architecture can make a few bits apart, so the committed Oggs were made on macOS
on arm64 and the tests hold them to the byte there, and by their decoded samples elsewhere.

| File | What it is | SHA-256 |
|---|---|---|
| `en/perfect-dodge.wav` | The `perfect-dodge` line's English placeholder | `521e6019beea066256d85406b84a22dd7dced3d5651a84c042e496b53e362ae3` |
| `en/swap-in.wav` | The `swap-in` line's English placeholder | `b681e7291ba4a3a9b0f54f1bb5f9d433def576b3a406aed2472acb26b2e595f6` |
| `en/guide_greet.wav` | The ward guide's greeting, `guide_greet`, in English: a conversation's voiced line | `9446b157ece9065ec4deb6cc4650bed78d6e87612091bf462bd1fbee16ed6111` |
| `ja/perfect-dodge.wav` | The `perfect-dodge` line's Japanese placeholder | `35cb47b7d2844b6ff734e144a64cf0fe05c018e381c39a6104fceb55b88ad373` |
| `ja/swap-in.wav` | The `swap-in` line's Japanese placeholder | `b130b532054a8acb616a1e6c06f54b37dd09c8501dba657a519ef0e4fd0f3b4e` |
| `ja/guide_greet.wav` | The same greeting's Japanese placeholder | `af816747cd18262686e721881604741dde1fb75ec535702ad8eadfff5aa4faa0` |

Nobody has listened to these. They prove the voice path with numbers; every timbre and length is a guess
for someone who can hear them, and real takes replace them.
