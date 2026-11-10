//! [`KiraBackend`]: the [`AudioBackend`] on kira's mixer, over whichever [`Output`] pulls its audio.
//!
//! The mix is a sub-track for each [`Bus`], each with room for every voice; the world's buses
//! carry a low-pass filter, out of the signal until a snapshot asks for it. A voice is a kira
//! static sound on its bus's track.
//!
//! Music (M3 A1) is a score's stems, looping static sounds on the music track, all started by one
//! kira clock that counts the score's beats: a stem's fade waits on the clock for its beat, so it
//! lands on the bar in the mixer's own time, to within one internal buffer, however late the frame
//! that asked.
//!
//! The main track ends in a fold to mono (M4 A2), out of the signal until the player asks for it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use kira::clock::{ClockHandle, ClockSpeed, ClockTime};
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::sound::PlaybackState;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings};
use kira::track::{MainTrackBuilder, TrackBuilder, TrackHandle};
use kira::{
    AudioManager, AudioManagerSettings, Capacities, Decibels, Frame, Mix, Panning, PlaybackRate, StartTime,
    Tween,
};
use sapphire_assets::AssetId;
use sapphire_assets::cue::Bus;
use sapphire_assets::score::MAX_STEMS;
use sapphire_assets::sound::DecodedSound;

use crate::LOG_TARGET;
use crate::backend::{AudioBackend, BusParams, DeviceStatus, StartError, StemStart, VoiceParams, VoiceSlot};
use crate::output::Output;
use crate::system::MAX_VOICES;

/// Frames between the mixer's parameter updates: kira's default, about 3 ms.
const INTERNAL_BUFFER_FRAMES: usize = 128;

/// How long a voice takes to reach new parameters: enough to hide the steps of a frame's updates.
const VOICE_TWEEN: Duration = Duration::from_millis(30);
/// How long a voice takes to freeze, go on or end. Hitstop is about a tenth of a second, so this stays
/// short of hearing as a fade, and long enough not to click.
const FREEZE_TWEEN: Duration = Duration::from_millis(5);
/// How long a bus's filter takes to move.
const FILTER_TWEEN: Duration = Duration::from_millis(30);
/// How long the fold to mono takes to come in or go: long enough not to click.
const MONO_EASE_SECONDS: f32 = 0.03;

/// A bus: its track and, where the world's clock can reach it, its filter.
struct BusTrack {
    track: TrackHandle,
    filter: Option<FilterHandle>,
    open: bool,
}

/// The music: the clock its stems start on and their fades wait on, and the stems.
struct Music {
    clock: ClockHandle,
    stems: Vec<StaticSoundHandle>,
}

impl Music {
    /// Ends the stems over `fade`. A dropped handle would leave its stem looping.
    fn end(&mut self, fade: Duration) {
        for stem in &mut self.stems {
            stem.stop(tween(fade));
        }
        self.stems.clear();
        self.clock.stop();
    }
}

/// The [`AudioBackend`] on kira. See the [crate docs](crate).
pub struct KiraBackend<O: Output> {
    manager: AudioManager<O>,
    buses: Vec<BusTrack>,
    sounds: HashMap<AssetId, StaticSoundData>,
    voices: Vec<Option<StaticSoundHandle>>,
    music: Music,
    /// Whether the main track folds to mono, which its effect reads in the mixer's own time.
    mono: Arc<AtomicBool>,
}

impl<O: Output> KiraBackend<O> {
    /// Starts the mixer on an output made with `settings`.
    pub fn new(settings: O::Settings) -> Result<Self, O::Error> {
        let mono = Arc::new(AtomicBool::new(false));
        let mut main_track_builder = MainTrackBuilder::default();
        main_track_builder.add_effect(MonoFoldBuilder(Arc::clone(&mono)));
        let mut manager = AudioManager::<O>::new(AudioManagerSettings {
            capacities: Capacities::default(),
            main_track_builder,
            internal_buffer_size: INTERNAL_BUFFER_FRAMES,
            backend_settings: settings,
        })?;
        let mut buses = Vec::with_capacity(Bus::ALL.len());
        for bus in Bus::ALL {
            // A score's stems, and the last one's fading out.
            let stems = if bus == Bus::Music { 2 * MAX_STEMS } else { 0 };
            let mut builder = TrackBuilder::new().sound_capacity(MAX_VOICES + stems).sub_track_capacity(0);
            let filter = bus.follows_world_clock().then(|| {
                builder
                    .add_effect(FilterBuilder::new().mode(FilterMode::LowPass).cutoff(20_000.0).mix(Mix::DRY))
            });
            match manager.add_sub_track(builder) {
                Ok(track) => buses.push(BusTrack { track, filter, open: true }),
                // There is room for far more than five in a new mixer.
                Err(_) => unreachable!("a new mixer has room for its buses"),
            }
        }
        let Ok(clock) = manager.add_clock(ClockSpeed::TicksPerMinute(120.0)) else {
            unreachable!("a new mixer has room for a clock")
        };
        Ok(Self {
            manager,
            buses,
            sounds: HashMap::new(),
            voices: std::iter::repeat_with(|| None).take(MAX_VOICES).collect(),
            music: Music { clock, stems: Vec::with_capacity(MAX_STEMS) },
            mono,
        })
    }

    /// The output, for what only it can do: rendering, for an offline one.
    pub fn output_mut(&mut self) -> &mut O {
        self.manager.backend_mut()
    }

    fn voice(&mut self, slot: VoiceSlot) -> Option<&mut StaticSoundHandle> {
        self.voices.get_mut(slot.0 as usize)?.as_mut()
    }
}

/// Folds the mix to mono for a player who hears with one ear, or through one speaker (M4 A2): each
/// frame's two sides ease toward their average over [`MONO_EASE_SECONDS`], and back. Off, it leaves the
/// mix untouched, sample for sample.
struct MonoFold {
    on: Arc<AtomicBool>,
    /// How far folded, 0 to 1.
    share: f32,
    /// How far a frame moves `share`.
    step: f32,
}

impl MonoFold {
    fn ease_at(&mut self, sample_rate: u32) {
        self.step = 1.0 / (MONO_EASE_SECONDS * sample_rate as f32).max(1.0);
    }
}

impl Effect for MonoFold {
    fn init(&mut self, sample_rate: u32, _internal_buffer_size: usize) {
        self.ease_at(sample_rate);
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.ease_at(sample_rate);
    }

    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info<'_>) {
        let target = if self.on.load(Ordering::Relaxed) { 1.0 } else { 0.0 };
        if target == 0.0 && self.share == 0.0 {
            return;
        }
        for frame in input {
            self.share = if target > self.share {
                (self.share + self.step).min(target)
            } else {
                (self.share - self.step).max(target)
            };
            let middle = (frame.left + frame.right) * 0.5;
            frame.left += (middle - frame.left) * self.share;
            frame.right += (middle - frame.right) * self.share;
        }
    }
}

/// Builds a [`MonoFold`] that follows `0`.
struct MonoFoldBuilder(Arc<AtomicBool>);

impl EffectBuilder for MonoFoldBuilder {
    type Handle = ();

    fn build(self) -> (Box<dyn Effect>, ()) {
        (Box::new(MonoFold { on: self.0, share: 0.0, step: 1.0 }), ())
    }
}

fn tween(duration: Duration) -> Tween {
    Tween { duration, ..Tween::default() }
}

/// A decoded sound as kira's frames: left and right, mono in both.
fn frames_of(sound: &DecodedSound) -> Arc<[Frame]> {
    match sound.channels {
        1 => sound.samples.iter().map(|&sample| Frame::from_mono(sample)).collect(),
        _ => sound.samples.as_chunks::<2>().0.iter().map(|&[left, right]| Frame::new(left, right)).collect(),
    }
}

fn settings_of(params: VoiceParams) -> StaticSoundSettings {
    StaticSoundSettings::new()
        .volume(Decibels(params.gain_db))
        .panning(Panning(params.pan))
        .playback_rate(PlaybackRate(f64::from(params.pitch)))
}

impl<O: Output> AudioBackend for KiraBackend<O> {
    fn add_sound(&mut self, id: AssetId, sound: &DecodedSound) {
        let data = StaticSoundData {
            sample_rate: sound.sample_rate,
            frames: frames_of(sound),
            settings: StaticSoundSettings::new(),
            slice: None,
        };
        self.sounds.insert(id, data);
    }

    fn remove_sound(&mut self, id: AssetId) {
        self.sounds.remove(&id);
    }

    fn start_voice(
        &mut self,
        slot: VoiceSlot,
        sound: AssetId,
        bus: Bus,
        params: VoiceParams,
        frozen: bool,
    ) -> Result<(), StartError> {
        let data = self.sounds.get(&sound).ok_or(StartError::UnknownSound(sound))?;
        // A clone shares the frames.
        let mut data = data.clone();
        data.settings = settings_of(params);
        let track = &mut self.buses[bus.index()].track;
        let mut handle = track.play(data).map_err(|error| {
            tracing::warn!(target: LOG_TARGET, %error, "the mixer refused a sound");
            StartError::MixerFull
        })?;
        if frozen {
            handle.pause(tween(Duration::ZERO));
        }
        self.voices[slot.0 as usize] = Some(handle);
        Ok(())
    }

    fn set_voice(&mut self, slot: VoiceSlot, params: VoiceParams) {
        let Some(voice) = self.voice(slot) else { return };
        let moving = tween(VOICE_TWEEN);
        voice.set_volume(Decibels(params.gain_db), moving);
        voice.set_panning(Panning(params.pan), moving);
        voice.set_playback_rate(PlaybackRate(f64::from(params.pitch)), moving);
    }

    fn freeze_voice(&mut self, slot: VoiceSlot, frozen: bool) {
        let Some(voice) = self.voice(slot) else { return };
        if frozen {
            voice.pause(tween(FREEZE_TWEEN));
        } else {
            voice.resume(tween(FREEZE_TWEEN));
        }
    }

    fn stop_voice(&mut self, slot: VoiceSlot) {
        if let Some(voice) = self.voice(slot) {
            voice.stop(tween(FREEZE_TWEEN));
        }
    }

    fn voice_finished(&mut self, slot: VoiceSlot) -> bool {
        let finished = self.voice(slot).is_none_or(|voice| voice.state() == PlaybackState::Stopped);
        if finished {
            self.voices[slot.0 as usize] = None;
        }
        finished
    }

    fn set_bus(&mut self, bus: Bus, params: BusParams) {
        let bus = &mut self.buses[bus.index()];
        let Some(filter) = &mut bus.filter else { return };
        let moving = tween(FILTER_TWEEN);
        // Open, the filter is out of the signal: it colors even at the top of the range.
        let open = !params.low_pass_hz.is_finite();
        if open != bus.open {
            bus.open = open;
            filter.set_mix(if open { Mix::DRY } else { Mix::WET }, moving);
        }
        if !open {
            filter.set_cutoff(f64::from(params.low_pass_hz), moving);
        }
    }

    fn set_bus_gain(&mut self, bus: Bus, gain_db: f32, fade: Duration) {
        self.buses[bus.index()].track.set_volume(Decibels(gain_db), tween(fade));
    }

    fn load_music(&mut self, tempo_bpm: f32, stems: &[StemStart], fade: Duration) -> Result<(), StartError> {
        // Stopped before the stems go in, so none can start on the last score's clock.
        self.music.end(fade);
        self.music.clock.set_speed(ClockSpeed::TicksPerMinute(f64::from(tempo_bpm)), tween(Duration::ZERO));
        let first_beat = StartTime::ClockTime(ClockTime::from_ticks_u64(&self.music.clock, 0));
        for stem in stems {
            let Some(data) = self.sounds.get(&stem.sound) else {
                self.music.end(Duration::ZERO);
                return Err(StartError::UnknownSound(stem.sound));
            };
            let data = data.loop_region(..).start_time(first_beat).volume(Decibels(stem.gain_db));
            match self.buses[Bus::Music.index()].track.play(data) {
                Ok(handle) => self.music.stems.push(handle),
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %error, "the mixer refused a stem");
                    self.music.end(Duration::ZERO);
                    return Err(StartError::MixerFull);
                }
            }
        }
        Ok(())
    }

    fn play_music(&mut self) {
        self.music.clock.start();
    }

    fn fade_stem(&mut self, stem: usize, gain_db: f32, beat: u64, fade: Duration) {
        let at = StartTime::ClockTime(ClockTime::from_ticks_u64(&self.music.clock, beat));
        if let Some(stem) = self.music.stems.get_mut(stem) {
            stem.set_volume(Decibels(gain_db), Tween { start_time: at, duration: fade, ..Tween::default() });
        }
    }

    fn stop_music(&mut self, fade: Duration) {
        self.music.end(fade);
    }

    fn music_beats(&mut self) -> Option<f64> {
        let clock = &self.music.clock;
        clock.ticking().then(|| {
            let time = clock.time();
            time.ticks as f64 + time.fraction
        })
    }

    fn suspend(&mut self) {
        self.manager.backend_mut().suspend();
    }

    fn resume(&mut self) {
        self.manager.backend_mut().resume();
    }

    fn status(&mut self) -> DeviceStatus {
        self.manager.backend_mut().status()
    }

    fn set_mono(&mut self, mono: bool) {
        self.mono.store(mono, Ordering::Relaxed);
    }
}
