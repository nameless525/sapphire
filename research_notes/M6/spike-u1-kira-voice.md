# Spike U1: an engine voice on kira 0.12.6

What M6 U1 needed to know before building its voice, read from kira 0.12.6's source as cargo holds it and measured with small tests against `OfflineOutput` (the crate's own renderer, no device). The tests were throwaway and aren't in the tree; what they measured that matters for the kill line is now in `crates/sapphire_audio/tests/costs.rs`. Run on 2026-11-17 on an Apple M5, release build, with other agents building: load average 7 for the timings below unless said otherwise. **Every time here is relative only:** read the ratios, not the microseconds.

## Can a sound of our own play on a track?

Yes. `SoundData` (`into_sound() -> (Box<dyn Sound>, Handle)`) and `Sound` (`on_start_processing`, `process(&mut [Frame], dt, &Info)`, `finished`) are public, and `TrackHandle::play` takes any `SoundData`. A constant sound of our own played on a sub-track came out at its value. Everything a sound needs to be built well is public too: `Parameter` with tweens that wait on a clock, `command_writer_and_reader` (kira's triple buffers), `Info::when_to_start` for a clock start, and `MockInfoBuilder` for unit tests with a clock. What isn't public: kira's playback state machine and its resampler, so a voice writes its own (both small).

A finished sound leaves the audio thread through a ring back to the calling thread, which drops it the next time a sound is inserted, so a voice holding an `Arc` of shared samples never frees memory on the audio thread.

## Can a send's level change on a handle while it plays?

Yes, for a route the track was built with: `TrackHandle::set_send(send, volume, tween)`; it can't add a route. Measured: a sub-track feeding a send track at silence, then set to 0 dB on the handle, doubled the output (the dry path plus the send). Two things to know:

- The send level is applied once a buffer (`volume.value()`), not interpolated inside it, so a fast send move steps every 128 frames.
- Sends are taken after the track's own volume but don't pass through its parent: silencing a parent track left its child's send audible. A ducked bus doesn't duck reverb fed from tracks below it; sends should hang off the bus tracks, or the mix must move their levels itself.

## What do 64 sub-tracks cost?

Mix of 512 frames, median, one voice a track against the same voices on one bus track:

| | 32 voices | 64 voices |
|---|---|---|
| kira's static sounds on one track (v0.2.0's path) | 118 µs | 230 µs |
| a prototype engine voice on one track | 45 µs | 83 µs |
| a sub-track a voice, each with kira's filter and a send to one reverb | 251 µs | 494 µs |
| the same tracks with no sound playing | 128 µs | 246 µs |
| one reverb fed by one voice | 13 µs | 13 µs |

A track a slot costs about twice the whole of today's mix, and most of that is paid with nothing playing: kira runs a track's effects every buffer whether or not a sound plays on it, so a pool of 64 filtered tracks costs about as much idle as 64 voices do today. The engine voice with its filter inside it is the cheaper design by a factor of five against tracks, and doesn't hit U1's kill line (1.5× today's at 32 voices).

## What do kira's stream threads cost?

`StreamingSoundData::from_decoder` starts one OS thread a stream (8 streams added 8 threads, 16 added 16), each with a ring of 16,384 frames of 16 bytes (256 KiB). The thread decodes until the ring is full and then sleeps 1 ms at a time, so an idle stream still wakes a thousand times a second: 8 idle streams used about 0.08 s of CPU in 2 s, 16 used about 0.19 s, roughly half a percent of a core each. Starting one took 13 to 340 µs on the calling thread (the thread's creation). `Decoder::decode` returns a `Vec` a chunk, so the decode thread allocates as it goes. For a phone with four to eight stems, a line and some beds that is a dozen threads waking a thousand times a second each; U8's single audio-IO worker stays the plan.

## What was built from it

`VoiceSound` (`crates/sapphire_audio/src/voice_sound.rs`) on the public traits, playing every cue and stem. Spec §5.15's "As built (M6 U1)" has what it does and costs.

## Unverified

- Every time above was taken with other builds running; none is a budget figure. The orchestrator's quiet-window run of `tests/costs.rs` is the one to quote.
- Thread wake-up cost was read from the process's CPU time by `ps`, ten-millisecond resolution, on macOS only; a phone's scheduler may cost more or less.
- Nobody listened: the voice's linear interpolation and one-pole filter were judged by tests of frequency and level, not by ear.
