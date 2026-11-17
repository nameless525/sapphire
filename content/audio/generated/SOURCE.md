# Generated audio

The fight's placeholder score (M3 A1); its voice lines moved to [`content/vo/`](../../vo) as masters
(M6 U11). Not third-party: every sound here is the project's own work, synthesized by
[`tools/sapphire_soundgen`](../../../tools/sapphire_soundgen) from a spec each in
[`tools/sounds/`](../../../tools/sounds) (M4 R4), with no recordings, samples, downloads or
text-to-speech voices. Licensed with the rest of the workspace.

| | |
|---|---|
| **Author** | The Sapphire project |
| **Made by** | `cargo run -p sapphire_soundgen`, from the workspace root: `tools/sounds/*.sound.ron` into `content/` |
| **Made on** | macOS on arm64, 2026-11-09 (UTC) |
| **Format** | Ogg Vorbis, mono, 24 kHz, quality 0.3 (`vorbis_rs`, a fixed stream serial for each file) |
| **License** | The workspace's |

Each file is a pure function of its spec: its noise is a seeded xorshift and each stream's serial
is fixed, so a run writes the same bytes as the last on the same machine. Another machine's floating
point may differ in the last bits, so the generator writes only missing files unless given `--force`,
and its tests hold these to their specs byte for byte on macOS on arm64. A new sound needs its row
here: the tests check that this table names every file a spec makes.

## What is here

| File | What it is | SHA-256 |
|---|---|---|
| `music/lantern_pad.ogg` | Held chords (A minor, F, C, G), four bars of 4/4 at 120 beats a minute (192,000 frames) | `0fcec9e374a4234ed23c39abd1ccb73531f5f5f8d21bbbd9ac470a8105f40992` |
| `music/lantern_pulse.ogg` | An eighth-note bass pulse, the same length | `8bd2fd94a433a123fda343681eba5a6e15d204a69c117f9fa8aeecaf0d3a2a06` |
| `music/lantern_drums.ogg` | Synthesized kick, snare and hat, the same length | `adaf9706ed47cdedba1c6d8f82c02283c6056a2109f69f1b860ea3899e1fdf3d` |
| `music/lantern_bells.ogg` | A pentatonic bell tune, the same length | `e22fb1e2c6386c24aa7f421be216c28e4a321f981a7fa8c243d98b3b09c810d5` |
| `music/lantern_toll.ogg` | A great bell tolled on each bar's root, the same length, for a boss's `tolling` phase (M3 Z5a) | `7670287d921863973dca1c948a3994a3b6fe4e91748d5ab27ef35817828ed2d8` |

The score's `.ron` document beside them and every `.meta` sidecar are written by hand or by the cook, not
by the generator.

Nobody has listened to these. They prove the music path with numbers; every level, note and timbre is a
guess for someone who can hear them, and a real score replaces them.
