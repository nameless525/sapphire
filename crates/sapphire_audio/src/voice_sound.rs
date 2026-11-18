//! [`Voice`]: the engine's own voice, on kira's public `Sound` trait (M6 U1).
//!
//! Every sound the mixer plays, a cue's or a score's stem, is one of these on its bus's track. Its
//! samples stay in the channels they were authored in, as 16-bit integers when they fit
//! ([`SoundStorage`]), so a mono second costs 96 KB where kira's own static sound held it as two `f32`s a
//! frame, 384 KB. On the way out a voice resamples for its pitch and the mixer's rate, by linear
//! interpolation; filters through a one-pole low-pass of its own, out of the signal while open; and pans
//! by equal power, as kira does. It can start part way in, wait for a clock's tick, and loop a region. What it
//! plays goes to the room's reverb too, by its own level (M6 U5, [`RoomTap`]).
//!
//! The audio thread allocates nothing for a voice: its parameters arrive through kira's command
//! buffers, made with the voice on the calling thread, and a voice that has played out is handed back
//! to that thread to be dropped. Parameters move once an internal buffer (about 3 ms), and inside it each
//! gain is a straight line, so a frame costs a few multiplies rather than a power and a square root.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use kira::clock::ClockTime;
use kira::command::{CommandReader, CommandWriter, command_writer_and_reader};
use kira::info::{Info, WhenToStart};
use kira::sound::{Sound, SoundData};
use kira::{Decibels, Frame, Parameter, Tween, Value};
use sapphire_assets::cue::SILENCE_DB;
use sapphire_assets::sound::DecodedSound;

use crate::backend::VoiceParams;
use crate::room_reverb::RoomTap;

/// What an integer sample's full scale is, either way.
const INTEGER_SCALE: f32 = 32_767.0;

/// The cutoff an open low-pass moves to, before it leaves the signal: the top of hearing.
const OPEN_CUTOFF_HZ: f32 = 20_000.0;
/// The lowest cutoff a voice takes.
const LOWEST_CUTOFF_HZ: f32 = 10.0;

/// A decoded sound as the mixer holds it, shared by every voice that plays it: its authored channels,
/// as 16-bit integers when every sample is within full scale, and as `f32` when one isn't, so a sound
/// decoded a little over full scale keeps its peaks rather than clipping.
pub(crate) struct SoundStorage {
    sample_rate: u32,
    stereo: bool,
    samples: Samples,
}

enum Samples {
    Integer(Box<[i16]>),
    Float(Box<[f32]>),
}

impl SoundStorage {
    pub(crate) fn new(sound: &DecodedSound) -> Self {
        let stereo = sound.channels != 1;
        let channels = if stereo { 2 } else { 1 };
        // A half frame at the end is no frame.
        let whole = sound.samples.len() / channels * channels;
        let samples = &sound.samples[..whole];
        let samples = if samples.iter().all(|sample| sample.abs() <= 1.0) {
            Samples::Integer(samples.iter().map(|&sample| (sample * INTEGER_SCALE).round() as i16).collect())
        } else {
            Samples::Float(samples.into())
        };
        Self { sample_rate: sound.sample_rate, stereo, samples }
    }

    /// The bytes the samples take.
    pub(crate) fn bytes(&self) -> u64 {
        match &self.samples {
            Samples::Integer(samples) => size_of_val::<[i16]>(samples) as u64,
            Samples::Float(samples) => size_of_val::<[f32]>(samples) as u64,
        }
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn frames(&self) -> usize {
        let samples = match &self.samples {
            Samples::Integer(samples) => samples.len(),
            Samples::Float(samples) => samples.len(),
        };
        if self.stereo { samples / 2 } else { samples }
    }
}

/// One sample, in whatever the storage holds.
trait Sample: Copy {
    fn amplitude(self) -> f32;
}

impl Sample for i16 {
    #[inline(always)]
    fn amplitude(self) -> f32 {
        f32::from(self) * (1.0 / INTEGER_SCALE)
    }
}

impl Sample for f32 {
    #[inline(always)]
    fn amplitude(self) -> f32 {
        self
    }
}

/// A storage's frames, as the mixing loop reads them: silence past the end.
trait Frames: Copy {
    fn len(self) -> usize;
    fn at(self, index: usize) -> Frame;
}

#[derive(Clone, Copy)]
struct Mono<'a, T>(&'a [T]);

#[derive(Clone, Copy)]
struct Stereo<'a, T>(&'a [[T; 2]]);

impl<T: Sample> Frames for Mono<'_, T> {
    fn len(self) -> usize {
        self.0.len()
    }

    #[inline(always)]
    fn at(self, index: usize) -> Frame {
        self.0.get(index).map_or(Frame::ZERO, |&sample| Frame::from_mono(sample.amplitude()))
    }
}

impl<T: Sample> Frames for Stereo<'_, T> {
    fn len(self) -> usize {
        self.0.len()
    }

    #[inline(always)]
    fn at(self, index: usize) -> Frame {
        self.0
            .get(index)
            .map_or(Frame::ZERO, |&[left, right]| Frame::new(left.amplitude(), right.amplitude()))
    }
}

/// When a voice starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Start {
    /// From the mixer's next buffer.
    Now,
    /// From the first buffer at or after this time of a clock: a score's stems, all on its first beat.
    OnClock(ClockTime),
}

/// A part of a sound played over and over, in its frames: `start` up to, not including, `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoopRegion {
    pub start: usize,
    pub end: usize,
}

/// How a voice begins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VoiceSettings {
    pub params: VoiceParams,
    pub start: Start,
    /// The frame of the sound it starts from.
    pub from: usize,
    /// What it loops, if anything: then it plays until it's stopped. Clamped to the sound, and nothing
    /// if that leaves it empty.
    pub looped: Option<LoopRegion>,
    /// Whether it waits, silent, for [`VoiceHandle::freeze`] to let it go.
    pub frozen: bool,
    /// How long it takes to come in from silence, as a virtual voice coming back does (M6 U3): none, and it's
    /// heard whole from its first frame.
    pub fade_in: Duration,
}

impl VoiceSettings {
    /// A voice that plays its sound once through, from now.
    #[cfg(test)]
    pub(crate) fn once(params: VoiceParams, frozen: bool) -> Self {
        Self { params, start: Start::Now, from: 0, looped: None, frozen, fade_in: Duration::ZERO }
    }
}

/// A sound to play, as a kira track takes it: [`kira::track::TrackHandle::play`] returns its
/// [`VoiceHandle`].
pub(crate) struct Voice {
    pub storage: Arc<SoundStorage>,
    pub settings: VoiceSettings,
    /// Where it sends the room's reverb what it plays, by [`VoiceParams::send_db`].
    pub room: Option<Arc<RoomTap>>,
}

/// Where a voice is: what its handle reads, and the audio thread writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum State {
    /// Heard, or fading in.
    Playing,
    /// Fading to a stop it will go on from.
    Pausing,
    /// Silent, and holding its place.
    Paused,
    /// Fading out for good.
    Stopping,
    /// Played out or stopped: the track drops it.
    Stopped,
}

/// New parameters, and how they get there.
#[derive(Clone, Copy, Debug)]
struct Set {
    params: VoiceParams,
    tween: Tween,
}

/// A freeze or a thaw, over a fade.
#[derive(Clone, Copy, Debug)]
struct Freeze {
    frozen: bool,
    fade: Duration,
}

/// What the calling thread keeps of a playing voice.
pub(crate) struct VoiceHandle {
    set: CommandWriter<Set>,
    freeze: CommandWriter<Freeze>,
    stop: CommandWriter<Duration>,
    state: Arc<AtomicU8>,
}

impl VoiceHandle {
    /// Moves the voice to `params` by `tween`, which may wait for a clock's time. A change not yet read
    /// by the mixer is replaced.
    pub(crate) fn set(&mut self, params: VoiceParams, tween: Tween) {
        self.set.write(Set { params, tween });
    }

    /// Fades the voice to a stop where it is, or back from one, over `fade`.
    pub(crate) fn freeze(&mut self, frozen: bool, fade: Duration) {
        self.freeze.write(Freeze { frozen, fade });
    }

    /// Fades the voice out for good over `fade`: at once if it's silent, frozen or waiting for its clock.
    pub(crate) fn stop(&mut self, fade: Duration) {
        self.stop.write(fade);
    }

    /// Whether the voice has played out or stopped, as the mixer last said.
    pub(crate) fn finished(&self) -> bool {
        self.state.load(Ordering::Acquire) == State::Stopped as u8
    }
}

/// What a buffer's frames move between: a straight line from the last buffer's to this one's.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ramp {
    amplitude: f32,
    left: f32,
    right: f32,
    /// Frames of the sound a frame of the mix moves.
    step: f64,
    /// The low-pass's coefficient: 1 passes everything.
    coefficient: f32,
    /// The level to the room's reverb, an amplitude.
    send: f32,
}

/// Where a voice is in its sound, and what it carries from one frame to the next.
struct Playhead {
    /// In the sound's frames.
    position: f64,
    looped: Option<LoopRegion>,
    /// The low-pass's output, or while it's open the last frame that went by it.
    filtered: Frame,
    /// 0 silent to 1 heard, for freezes and stops.
    fade: f32,
}

impl Playhead {
    /// Mixes `out` from `frames`, ramping from `from` to `to` and fading toward `fade_to` by `fade_step` a
    /// frame; whether the sound played out.
    #[inline(always)]
    fn render<F: Frames, const FILTER: bool>(
        &mut self,
        frames: F,
        out: &mut [Frame],
        from: Ramp,
        to: Ramp,
        fade_to: f32,
        fade_step: f32,
    ) -> bool {
        let count = out.len() as f32;
        let along = |from: f32, to: f32| (to - from) / count;
        let (amplitude_step, left_step, right_step, coefficient_step) = (
            along(from.amplitude, to.amplitude),
            along(from.left, to.left),
            along(from.right, to.right),
            along(from.coefficient, to.coefficient),
        );
        let step_step = (to.step - from.step) / f64::from(count);
        let (mut amplitude, mut left, mut right, mut coefficient, mut step) =
            (from.amplitude, from.left, from.right, from.coefficient, from.step);
        let length = frames.len() as f64;
        let mut filtered = self.filtered;
        let mut fade = self.fade;
        let mut ended = false;
        for (n, frame) in out.iter_mut().enumerate() {
            amplitude += amplitude_step;
            left += left_step;
            right += right_step;
            coefficient += coefficient_step;
            step += step_step;
            fade = if fade_to > fade {
                (fade + fade_step).min(fade_to)
            } else {
                (fade - fade_step).max(fade_to)
            };

            let index = self.position as usize;
            let fraction = (self.position - index as f64) as f32;
            let next = match self.looped {
                Some(region) if index + 1 == region.end => region.start,
                _ => index + 1,
            };
            let (now, then) = (frames.at(index), frames.at(next));
            let sample = now + (then - now) * fraction;
            if FILTER {
                filtered += (sample - filtered) * coefficient;
            } else {
                filtered = sample;
            }
            let gain = amplitude * fade;
            *frame = Frame::new(filtered.left * left * gain, filtered.right * right * gain);

            self.position += step;
            match self.looped {
                Some(region) => {
                    let (start, end) = (region.start as f64, region.end as f64);
                    if self.position >= end {
                        self.position = start + (self.position - start) % (end - start);
                    }
                }
                None if self.position >= length => {
                    out[n + 1..].fill(Frame::ZERO);
                    ended = true;
                    break;
                }
                None => {}
            }
        }
        self.filtered = filtered;
        self.fade = fade;
        ended
    }
}

/// The audio thread's half of a voice.
struct VoiceSound {
    storage: Arc<SoundStorage>,
    start: Start,
    playhead: Playhead,
    gain_db: Parameter<f32>,
    pan: Parameter<f32>,
    pitch: Parameter<f32>,
    cutoff_hz: Parameter<f32>,
    send_db: Parameter<f32>,
    room: Option<Arc<RoomTap>>,
    /// Whether the low-pass is asked to open, and leaves the signal once its cutoff gets there.
    opening: bool,
    /// What the last buffer ended on, which the next one ramps from: none before the first.
    last: Option<Ramp>,
    /// How far the fade moves in a second, toward where `state` takes it.
    fade_rate: f32,
    state: State,
    shared: Arc<AtomicU8>,
    set: CommandReader<Set>,
    freeze: CommandReader<Freeze>,
    stop: CommandReader<Duration>,
}

impl SoundData for Voice {
    type Error = Infallible;
    type Handle = VoiceHandle;

    fn into_sound(self) -> Result<(Box<dyn Sound>, VoiceHandle), Infallible> {
        let Self { storage, settings, room } = self;
        let (set_writer, set) = command_writer_and_reader();
        let (freeze_writer, freeze) = command_writer_and_reader();
        let (stop_writer, stop) = command_writer_and_reader();
        let state = if settings.frozen { State::Paused } else { State::Playing };
        let shared = Arc::new(AtomicU8::new(state as u8));
        let frames = storage.frames();
        let looped = settings
            .looped
            .map(|region| LoopRegion { start: region.start, end: region.end.min(frames) })
            .filter(|region| region.start < region.end);
        let params = settings.params;
        let fixed = |value: f32| Parameter::new(Value::Fixed(value), value);
        let sound = VoiceSound {
            storage,
            start: settings.start,
            playhead: Playhead {
                position: settings.from as f64,
                looped,
                filtered: Frame::ZERO,
                fade: if settings.frozen || !settings.fade_in.is_zero() { 0.0 } else { 1.0 },
            },
            gain_db: fixed(params.gain_db),
            pan: fixed(params.pan),
            pitch: fixed(params.pitch),
            cutoff_hz: fixed(cutoff_of(params.low_pass_hz)),
            send_db: fixed(params.send_db),
            room,
            opening: !params.low_pass_hz.is_finite(),
            last: None,
            fade_rate: fade_rate(settings.fade_in),
            state,
            shared: Arc::clone(&shared),
            set,
            freeze,
            stop,
        };
        let handle = VoiceHandle { set: set_writer, freeze: freeze_writer, stop: stop_writer, state: shared };
        Ok((Box::new(sound), handle))
    }
}

/// The cutoff a voice's low-pass moves to for `hz`: open (any value that isn't finite) moves it to the top
/// of hearing.
fn cutoff_of(hz: f32) -> f32 {
    if hz.is_finite() { hz.clamp(LOWEST_CUTOFF_HZ, OPEN_CUTOFF_HZ) } else { OPEN_CUTOFF_HZ }
}

/// How far a fade over `fade` moves in a second: at once, for none.
fn fade_rate(fade: Duration) -> f32 {
    let seconds = fade.as_secs_f32();
    if seconds > 0.0 { 1.0 / seconds } else { f32::INFINITY }
}

impl VoiceSound {
    fn set_state(&mut self, state: State) {
        self.state = state;
        self.shared.store(state as u8, Ordering::Release);
    }

    /// What the parameters ask of this buffer, at `rate` frames a second.
    fn ramp(&self, rate: f32) -> Ramp {
        let pan = self.pan.value().clamp(-1.0, 1.0);
        let cutoff = self.cutoff_hz.value().min(rate * 0.45);
        Ramp {
            amplitude: Decibels(self.gain_db.value()).as_amplitude(),
            // Equal power, as kira's own panning: both sides whole at the centre, and √2 on one side at an end.
            left: (1.0 - pan).sqrt(),
            right: (1.0 + pan).sqrt(),
            step: f64::from(self.storage.sample_rate) * f64::from(self.pitch.value().max(0.0))
                / f64::from(rate),
            coefficient: 1.0 - (-std::f32::consts::TAU * cutoff / rate).exp(),
            send: if self.send_db.value() > SILENCE_DB {
                Decibels(self.send_db.value()).as_amplitude()
            } else {
                0.0
            },
        }
    }
}

impl Sound for VoiceSound {
    fn on_start_processing(&mut self) {
        if let Some(Set { params, tween }) = self.set.read() {
            self.gain_db.set(Value::Fixed(params.gain_db), tween);
            self.pan.set(Value::Fixed(params.pan), tween);
            self.pitch.set(Value::Fixed(params.pitch), tween);
            self.cutoff_hz.set(Value::Fixed(cutoff_of(params.low_pass_hz)), tween);
            self.send_db.set(Value::Fixed(params.send_db), tween);
            self.opening = !params.low_pass_hz.is_finite();
        }
        if let Some(Freeze { frozen, fade }) = self.freeze.read() {
            match (frozen, self.state) {
                (true, State::Playing | State::Pausing) => {
                    self.fade_rate = fade_rate(fade);
                    self.set_state(if self.playhead.fade > 0.0 { State::Pausing } else { State::Paused });
                }
                (false, State::Pausing | State::Paused) => {
                    self.fade_rate = fade_rate(fade);
                    self.set_state(State::Playing);
                }
                _ => {}
            }
        }
        if let Some(fade) = self.stop.read()
            && self.state != State::Stopped
        {
            self.fade_rate = fade_rate(fade);
            let silent = self.playhead.fade == 0.0 || matches!(self.start, Start::OnClock(_));
            self.set_state(if silent { State::Stopped } else { State::Stopping });
        }
    }

    fn process(&mut self, out: &mut [Frame], dt: f64, info: &Info<'_>) {
        let length = dt * out.len() as f64;
        self.gain_db.update(length, info);
        self.pan.update(length, info);
        self.pitch.update(length, info);
        self.cutoff_hz.update(length, info);
        self.send_db.update(length, info);
        if let Start::OnClock(time) = self.start {
            match info.when_to_start(time) {
                WhenToStart::Now => self.start = Start::Now,
                WhenToStart::Later => {
                    out.fill(Frame::ZERO);
                    return;
                }
                WhenToStart::Never => {
                    self.set_state(State::Stopped);
                    out.fill(Frame::ZERO);
                    return;
                }
            }
        }
        if matches!(self.state, State::Paused | State::Stopped) {
            out.fill(Frame::ZERO);
            return;
        }

        let rate = (1.0 / dt) as f32;
        let to = self.ramp(rate);
        let from = self.last.replace(to).unwrap_or(to);
        let fade_to = if self.state == State::Playing { 1.0 } else { 0.0 };
        let fade_step = self.fade_rate * dt as f32;
        let filter = !(self.opening && self.cutoff_hz.value() >= OPEN_CUTOFF_HZ);
        let playhead = &mut self.playhead;
        let ended = match (&self.storage.samples, self.storage.stereo, filter) {
            (Samples::Integer(samples), false, true) => {
                playhead.render::<_, true>(Mono(samples), out, from, to, fade_to, fade_step)
            }
            (Samples::Integer(samples), false, false) => {
                playhead.render::<_, false>(Mono(samples), out, from, to, fade_to, fade_step)
            }
            (Samples::Integer(samples), true, true) => {
                playhead.render::<_, true>(Stereo(samples.as_chunks().0), out, from, to, fade_to, fade_step)
            }
            (Samples::Integer(samples), true, false) => {
                playhead.render::<_, false>(Stereo(samples.as_chunks().0), out, from, to, fade_to, fade_step)
            }
            (Samples::Float(samples), false, true) => {
                playhead.render::<_, true>(Mono(samples), out, from, to, fade_to, fade_step)
            }
            (Samples::Float(samples), false, false) => {
                playhead.render::<_, false>(Mono(samples), out, from, to, fade_to, fade_step)
            }
            (Samples::Float(samples), true, true) => {
                playhead.render::<_, true>(Stereo(samples.as_chunks().0), out, from, to, fade_to, fade_step)
            }
            (Samples::Float(samples), true, false) => {
                playhead.render::<_, false>(Stereo(samples.as_chunks().0), out, from, to, fade_to, fade_step)
            }
        };
        if let Some(room) = &self.room
            && (from.send > 0.0 || to.send > 0.0)
        {
            room.add(out, from.send, to.send);
        }
        let silent = self.playhead.fade == 0.0;
        if ended || (self.state == State::Stopping && silent) {
            self.set_state(State::Stopped);
        } else if self.state == State::Pausing && silent {
            self.set_state(State::Paused);
        }
    }

    fn finished(&self) -> bool {
        self.state == State::Stopped
    }
}

#[cfg(test)]
mod tests {
    use kira::info::MockInfoBuilder;

    use super::*;

    const RATE: u32 = 48_000;
    /// kira's internal buffer, as the mixer calls a sound.
    const BUFFER: usize = 128;

    fn storage(samples: Vec<f32>, channels: u32) -> Arc<SoundStorage> {
        Arc::new(SoundStorage::new(&DecodedSound { sample_rate: RATE, channels, samples }))
    }

    fn play(storage: &Arc<SoundStorage>, settings: VoiceSettings) -> (Box<dyn Sound>, VoiceHandle) {
        let Ok(played) = Voice { storage: Arc::clone(storage), settings, room: None }.into_sound();
        played
    }

    /// `frames` frames as the mixer asks for them: commands read, then a buffer at a time.
    fn render(sound: &mut dyn Sound, frames: usize, info: &Info<'_>) -> Vec<Frame> {
        let mut out = vec![Frame::ZERO; frames];
        sound.on_start_processing();
        for buffer in out.chunks_mut(BUFFER) {
            sound.process(buffer, 1.0 / f64::from(RATE), info);
        }
        out
    }

    fn sine(hz: f32, seconds: f32) -> Vec<f32> {
        let frames = (seconds * RATE as f32) as usize;
        (0..frames).map(|n| 0.5 * (std::f32::consts::TAU * hz * n as f32 / RATE as f32).sin()).collect()
    }

    fn rms(frames: &[Frame]) -> f32 {
        (frames.iter().map(|frame| frame.left * frame.left).sum::<f32>() / frames.len() as f32).sqrt()
    }

    #[test]
    fn a_sound_is_held_in_its_channels_as_integers_where_it_fits() {
        let mono = storage(vec![0.5; 1_000], 1);
        assert!(matches!(mono.samples, Samples::Integer(_)));
        assert_eq!((mono.frames(), mono.bytes()), (1_000, 2_000), "two bytes a frame, not eight");
        let stereo = storage(vec![-1.0; 2_001], 2);
        assert_eq!((stereo.frames(), stereo.bytes()), (1_000, 4_000), "and the half frame at the end goes");
        let loud = storage(vec![0.5, 1.2, -0.3], 1);
        assert!(matches!(loud.samples, Samples::Float(_)), "past full scale stays exact");
        assert_eq!(loud.bytes(), 12);
    }

    #[test]
    fn integers_play_back_what_was_decoded() {
        let samples: Vec<f32> = (0..1_000).map(|n| (n as f32 * 0.01).sin() * 0.9).collect();
        let (mut sound, handle) =
            play(&storage(samples.clone(), 1), VoiceSettings::once(VoiceParams::NEUTRAL, false));
        let out = render(&mut *sound, 1_100, &MockInfoBuilder::new().build());
        for (frame, sample) in out.iter().zip(&samples) {
            assert!((frame.left - sample).abs() < 1.0 / INTEGER_SCALE, "{} against {sample}", frame.left);
            assert_eq!(frame.left, frame.right);
        }
        assert!(out[1_000..].iter().all(|frame| *frame == Frame::ZERO));
        assert!(sound.finished() && handle.finished());
    }

    #[test]
    fn a_voice_starts_where_it_is_told() {
        let ramp = (0..1_000).map(|n| n as f32 / 1_000.0).collect();
        let settings = VoiceSettings { from: 500, ..VoiceSettings::once(VoiceParams::NEUTRAL, false) };
        let (mut sound, _) = play(&storage(ramp, 1), settings);
        let out = render(&mut *sound, 600, &MockInfoBuilder::new().build());
        assert!((out[0].left - 0.5).abs() < 1e-3, "{}", out[0].left);
        assert!(sound.finished(), "and plays what's left");
    }

    /// M6 U3: a voice coming back fades in over its fade, from where it's told, with no step bigger than the
    /// sound's own and the fade's.
    #[test]
    fn a_fade_in_comes_up_from_silence_over_its_length() {
        let samples = vec![0.5; 4_800];
        let fade = Duration::from_millis(20);
        let settings =
            VoiceSettings { from: 1_000, fade_in: fade, ..VoiceSettings::once(VoiceParams::NEUTRAL, false) };
        let (mut sound, _) = play(&storage(samples, 1), settings);
        let out = render(&mut *sound, 1_920, &MockInfoBuilder::new().build());
        let fade_frames = (fade.as_secs_f64() * f64::from(RATE)) as usize;
        assert!(out[0].left < 0.5 / fade_frames as f32 * 2.0, "from silence: {}", out[0].left);
        let steps = out.windows(2).map(|pair| (pair[1].left - pair[0].left).abs());
        let largest = steps.fold(0.0, f32::max);
        assert!(largest <= 0.5 / fade_frames as f32 * 1.01, "no step past the fade's own: {largest}");
        assert!((out[fade_frames].left - 0.5).abs() < 1e-3, "whole once it's in: {}", out[fade_frames].left);
        assert!(!sound.finished(), "3,800 frames were left from frame 1,000");
    }

    #[test]
    fn a_loop_plays_its_region_until_stopped() {
        let ramp = (0..100).map(|n| n as f32 / 100.0).collect();
        let settings = VoiceSettings {
            looped: Some(LoopRegion { start: 20, end: 60 }),
            ..VoiceSettings::once(VoiceParams::NEUTRAL, false)
        };
        let (mut sound, mut handle) = play(&storage(ramp, 1), settings);
        let info = MockInfoBuilder::new().build();
        let out = render(&mut *sound, 1_000, &info);
        assert!(!sound.finished());
        // Up to the region's end once, and from there only the region, round and round.
        let looped = &out[60..];
        assert!(looped.iter().all(|frame| (0.2 - 1e-3..0.6).contains(&frame.left)), "only the region");
        assert!((looped[0].left - 0.2).abs() < 1e-3 && (looped[40].left - 0.2).abs() < 1e-3, "round again");
        handle.stop(Duration::from_millis(5));
        let _fading = render(&mut *sound, 480, &info);
        assert!(sound.finished() && handle.finished());
    }

    #[test]
    fn a_voice_on_a_clock_waits_for_its_time() {
        let mut waiting = MockInfoBuilder::new();
        let clock = waiting.add_clock(false, 0, 0.0);
        let waiting = waiting.build();
        let start = Start::OnClock(ClockTime { clock, ticks: 0, fraction: 0.0 });
        let settings = VoiceSettings { start, ..VoiceSettings::once(VoiceParams::NEUTRAL, false) };
        let tone = storage(vec![0.25; 4_800], 1);
        let (mut sound, _) = play(&tone, settings);
        assert!(render(&mut *sound, 1_000, &waiting).iter().all(|frame| *frame == Frame::ZERO));
        assert!(!sound.finished());
        let mut ticking = MockInfoBuilder::new();
        ticking.add_clock(true, 0, 0.0);
        let out = render(&mut *sound, 256, &ticking.build());
        assert!((out[0].left - 0.25).abs() < 1e-4, "from its first frame: {}", out[0].left);

        // Stopped before its time, it ends at once.
        let (mut sound, mut handle) = play(&tone, settings);
        handle.stop(Duration::from_secs(1));
        let _ = render(&mut *sound, BUFFER, &waiting);
        assert!(sound.finished());
    }

    /// One pole's gain at `hz` for a `cutoff`: `a / |1 - (1 - a)e^-jw|`.
    fn one_pole_gain(cutoff: f32, hz: f32) -> f32 {
        let a = 1.0 - (-std::f32::consts::TAU * cutoff / RATE as f32).exp();
        let w = std::f32::consts::TAU * hz / RATE as f32;
        let (re, im) = (1.0 - (1.0 - a) * w.cos(), (1.0 - a) * w.sin());
        a / (re * re + im * im).sqrt()
    }

    #[test]
    fn the_low_pass_falls_by_one_pole_and_opens_to_nothing() {
        let info = MockInfoBuilder::new().build();
        let heard = |hz: f32, low_pass_hz: f32| {
            let params = VoiceParams { low_pass_hz, ..VoiceParams::NEUTRAL };
            let (mut sound, _) = play(&storage(sine(hz, 0.5), 1), VoiceSettings::once(params, false));
            let out = render(&mut *sound, RATE as usize / 2, &info);
            // Past the filter's first moments.
            rms(&out[2_400..])
        };
        for hz in [100.0, 1_000.0, 8_000.0] {
            let open = heard(hz, f32::INFINITY);
            assert!((open - 0.5 / 2.0_f32.sqrt()).abs() < 1e-3, "{hz} Hz open: {open}");
            let gain = heard(hz, 1_000.0) / open;
            let expected = one_pole_gain(1_000.0, hz);
            assert!((gain / expected).log10().abs() * 20.0 < 0.2, "{hz} Hz: {gain} against {expected}");
        }
        assert!(20.0 * one_pole_gain(1_000.0, 8_000.0).log10() < -15.0, "8 kHz is well down");

        // Opened at once, the filter leaves the signal: the sound comes back sample for sample.
        let samples = sine(8_000.0, 0.1);
        let closed = VoiceParams { low_pass_hz: 1_000.0, ..VoiceParams::NEUTRAL };
        let (mut sound, mut handle) = play(&storage(samples.clone(), 1), VoiceSettings::once(closed, false));
        let _ = render(&mut *sound, 1_024, &info);
        handle.set(VoiceParams::NEUTRAL, Tween { duration: Duration::ZERO, ..Tween::default() });
        let out = render(&mut *sound, 1_024, &info);
        for (frame, sample) in out[BUFFER..].iter().zip(&samples[1_024 + BUFFER..]) {
            assert!((frame.left - sample).abs() < 1.0 / INTEGER_SCALE, "{} against {sample}", frame.left);
        }
    }

    #[test]
    fn panning_keeps_the_power() {
        let info = MockInfoBuilder::new().build();
        for pan in [-1.0, -0.5, 0.0, 0.3, 1.0] {
            let params = VoiceParams { pan, ..VoiceParams::NEUTRAL };
            let (mut sound, _) = play(&storage(vec![0.5; 512], 1), VoiceSettings::once(params, false));
            let frame = render(&mut *sound, 256, &info)[200];
            let power = frame.left * frame.left + frame.right * frame.right;
            assert!((power - 0.5).abs() < 1e-4, "pan {pan}: {power}");
        }
    }

    #[test]
    fn a_frozen_voice_holds_its_place() {
        let ramp = (0..4_800).map(|n| n as f32 / 4_800.0).collect();
        let (mut sound, mut handle) =
            play(&storage(ramp, 1), VoiceSettings::once(VoiceParams::NEUTRAL, true));
        let info = MockInfoBuilder::new().build();
        assert!(render(&mut *sound, 1_000, &info).iter().all(|frame| *frame == Frame::ZERO));
        handle.freeze(false, Duration::ZERO);
        let out = render(&mut *sound, 256, &info);
        assert!(out[0].left.abs() < 1e-3, "from the start: {}", out[0].left);
        handle.freeze(true, Duration::from_millis(1));
        let _fading = render(&mut *sound, 256, &info);
        let held = render(&mut *sound, 1_000, &info);
        assert!(held.iter().all(|frame| *frame == Frame::ZERO) && !sound.finished());
        handle.freeze(false, Duration::ZERO);
        // On from where the fade ended, within the buffer it ended in.
        let out = render(&mut *sound, 128, &info);
        let at = out[0].left * 4_800.0;
        assert!((256.0..=256.0 + BUFFER as f32).contains(&at), "{at}");
    }

    /// M6 U5: a voice sends the room what it plays, by its own level, and nothing at silence.
    #[test]
    fn a_voice_sends_the_room_what_it_plays_by_its_level() {
        let info = MockInfoBuilder::new().build();
        let tap = Arc::new(RoomTap::new(BUFFER));
        let params = VoiceParams { gain_db: -6.0, pan: 0.5, send_db: -12.0, ..VoiceParams::NEUTRAL };
        let settings = VoiceSettings::once(params, false);
        let Ok((mut sound, _)) =
            Voice { storage: storage(vec![0.5; 4_800], 1), settings, room: Some(Arc::clone(&tap)) }
                .into_sound();
        let heard = render(&mut *sound, BUFFER, &info);
        let mut sent = vec![Frame::ZERO; BUFFER];
        tap.take_into(&mut sent);
        let share = Decibels(-12.0).as_amplitude();
        for (sent, heard) in sent.iter().zip(&heard) {
            assert!(
                (sent.left - heard.left * share).abs() < 1e-6
                    && (sent.right - heard.right * share).abs() < 1e-6
            );
        }
        let mut again = vec![Frame::ZERO; BUFFER];
        tap.take_into(&mut again);
        assert!(again.iter().all(|frame| *frame == Frame::ZERO), "taken, it's clear");

        let quiet = VoiceSettings::once(VoiceParams { send_db: SILENCE_DB, ..params }, false);
        let Ok((mut sound, _)) =
            Voice { storage: storage(vec![0.5; 4_800], 1), settings: quiet, room: Some(Arc::clone(&tap)) }
                .into_sound();
        let _ = render(&mut *sound, BUFFER, &info);
        tap.take_into(&mut again);
        assert!(again.iter().all(|frame| *frame == Frame::ZERO), "silence sends nothing");
    }
}
