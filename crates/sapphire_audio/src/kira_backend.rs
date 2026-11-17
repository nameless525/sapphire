//! [`KiraBackend`]: the [`AudioBackend`] on kira's mixer, over whichever [`Output`] pulls its audio.
//!
//! The mix (M6 U2) is a sub-track for each of a [`Mix`]'s buses, each under its parent and the five
//! categories under a master track, each with room for every voice. A bus some snapshot low-passes carries
//! that filter first, out of the signal until the snapshot asks for it, then its own effects, and sends to
//! the mix's reverbs, which are kira's send tracks. Pausing a bus pauses its track, which holds every voice
//! under it where it is, tails and all. The master carries every output profile's volume and effects, each
//! effect switched in only for its own profile. A voice is the engine's own sound on kira's public `Sound`
//! trait (M6 U1, `voice_sound`), on its bus's track: it keeps each sound in its authored channels and filters
//! on its own.
//!
//! Music (M3 A1) is a score's stems, looping voices on the music track, all started by one
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
use kira::effect::compressor::CompressorBuilder;
use kira::effect::eq_filter::{EqFilterBuilder, EqFilterKind};
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::effect::reverb::ReverbBuilder;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::track::{MainTrackBuilder, SendTrackBuilder, SendTrackHandle, TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings, Capacities, Decibels, Frame, Mix as Share, StartTime, Tween};
use sapphire_assets::AssetId;
use sapphire_assets::cue::Bus;
use sapphire_assets::mix::{BusEffect, EqKind, Mix, OutputProfile};
use sapphire_assets::score::MAX_STEMS;
use sapphire_assets::sound::DecodedSound;

use crate::LOG_TARGET;
use crate::backend::{
    AudioBackend, BusId, BusParams, DeviceStatus, StartError, StemStart, VoiceParams, VoiceSlot,
};
use crate::output::Output;
use crate::system::MAX_VOICES;
use crate::voice_sound::{LoopRegion, SoundStorage, Start, Voice, VoiceHandle, VoiceSettings};

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
/// How long a change of output profile takes to cross: long enough not to click.
const PROFILE_EASE_SECONDS: f32 = 0.03;

/// A bus: its track and, where a snapshot can close one, its filter.
struct BusTrack {
    track: TrackHandle,
    filter: Option<FilterHandle>,
    open: bool,
}

/// What a [`Mix`] made: the master, every bus under it, the reverbs, and which profile each of the master's
/// effects belongs to. The handles keep their tracks: dropped, kira removes them, and everything under them.
struct MixTracks {
    /// Each profile, with its volume, by [`Mix::profiles`]' order.
    profiles: Vec<(OutputProfile, f32)>,
    /// The profile each of the master's effects is in, by place, and the switch it reads.
    switches: Vec<(usize, Arc<AtomicBool>)>,
    /// The profile playing, by place.
    profile: usize,
    /// Each reverb, with its own volume.
    sends: Vec<(SendTrackHandle, f32)>,
    /// By [`Mix::buses`]' order.
    buses: Vec<BusTrack>,
    master: TrackHandle,
}

/// The music: the clock its stems start on and their fades wait on, and the stems.
struct Music {
    clock: ClockHandle,
    stems: Vec<VoiceHandle>,
}

impl Music {
    /// Ends the stems over `fade`. A dropped handle would leave its stem looping.
    fn end(&mut self, fade: Duration) {
        for stem in &mut self.stems {
            stem.stop(fade);
        }
        self.stems.clear();
        self.clock.stop();
    }
}

/// The [`AudioBackend`] on kira. See the [crate docs](crate).
pub struct KiraBackend<O: Output> {
    manager: AudioManager<O>,
    mix: MixTracks,
    sounds: HashMap<AssetId, Arc<SoundStorage>>,
    /// What `sounds` hold, in bytes.
    decoded_bytes: u64,
    voices: Vec<Option<VoiceHandle>>,
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
        let Some(mix) = build_mix(&mut manager, &Mix::plain()) else {
            unreachable!("a new mixer has room for the plain mix")
        };
        let Ok(clock) = manager.add_clock(ClockSpeed::TicksPerMinute(120.0)) else {
            unreachable!("a new mixer has room for a clock")
        };
        Ok(Self {
            manager,
            mix,
            sounds: HashMap::new(),
            decoded_bytes: 0,
            voices: std::iter::repeat_with(|| None).take(MAX_VOICES).collect(),
            music: Music { clock, stems: Vec::with_capacity(MAX_STEMS) },
            mono,
        })
    }

    /// The output, for what only it can do: rendering, for an offline one.
    pub fn output_mut(&mut self) -> &mut O {
        self.manager.backend_mut()
    }

    fn voice(&mut self, slot: VoiceSlot) -> Option<&mut VoiceHandle> {
        self.voices.get_mut(slot.0 as usize)?.as_mut()
    }

    fn bus(&mut self, bus: BusId) -> Option<&mut BusTrack> {
        self.mix.buses.get_mut(bus.index())
    }
}

/// Makes a checked `mix`'s tracks on `manager`: none if the mixer has no room for them, which kira's default
/// capacities leave for many mixes.
fn build_mix<O: Output>(manager: &mut AudioManager<O>, mix: &Mix) -> Option<MixTracks> {
    let home = mix.profile(OutputProfile::Home);
    let home_db = mix.profiles[home].volume_db;
    let mut sends = Vec::with_capacity(mix.sends.len());
    for send in &mix.sends {
        let reverb = ReverbBuilder::new()
            .feedback(f64::from(send.feedback))
            .damping(f64::from(send.damping))
            .stereo_width(f64::from(send.stereo_width))
            .mix(Share::WET);
        // A send track ends in the main track, not the master: it takes the profile's volume itself.
        let builder = SendTrackBuilder::new().volume(Decibels(send.volume_db + home_db)).with_effect(reverb);
        sends.push((manager.add_send_track(builder).ok()?, send.volume_db));
    }

    let mut master =
        TrackBuilder::new().volume(Decibels(home_db)).sound_capacity(0).sub_track_capacity(Bus::ALL.len());
    let mut switches = Vec::new();
    for (place, profile) in mix.profiles.iter().enumerate() {
        for effect in &profile.effects {
            let on = Arc::new(AtomicBool::new(place == home));
            master.add_built_effect(Box::new(Switched::new(build_effect(effect), Arc::clone(&on))));
            switches.push((place, on));
        }
    }
    let mut master = manager.add_sub_track(master).ok()?;

    let mut buses: Vec<BusTrack> = Vec::with_capacity(mix.buses.len());
    for (place, bus) in mix.buses.iter().enumerate() {
        let children = (place + 1..mix.buses.len()).filter(|&other| mix.parent(other) == Some(place)).count();
        // A score's stems, and the last one's fading out.
        let stems = if place == BusId::category(Bus::Music).index() { 2 * MAX_STEMS } else { 0 };
        let mut builder = TrackBuilder::new()
            .volume(Decibels(bus.volume_db))
            .sound_capacity(MAX_VOICES + stems)
            .sub_track_capacity(children);
        let filtered = mix
            .snapshots
            .iter()
            .flat_map(|snapshot| &snapshot.buses)
            .any(|change| change.bus == bus.name && change.low_pass_hz.is_some());
        let filter = filtered.then(|| {
            builder
                .add_effect(FilterBuilder::new().mode(FilterMode::LowPass).cutoff(20_000.0).mix(Share::DRY))
        });
        for effect in &bus.effects {
            builder.add_built_effect(build_effect(effect));
        }
        for send in &bus.sends {
            let to = &sends[mix.send(&send.to)?].0;
            builder = builder.with_send(to, Decibels(send.volume_db));
        }
        let parent = match mix.parent(place) {
            Some(parent) => &mut buses[parent].track,
            None => &mut master,
        };
        let track = parent.add_sub_track(builder).ok()?;
        buses.push(BusTrack { track, filter, open: true });
    }
    Some(MixTracks {
        profiles: mix.profiles.iter().map(|profile| (profile.profile, profile.volume_db)).collect(),
        switches,
        profile: home,
        sends,
        buses,
        master,
    })
}

/// kira's effect for one of the mix's.
fn build_effect(effect: &BusEffect) -> Box<dyn Effect> {
    match *effect {
        BusEffect::LowPass { cutoff_hz } => {
            FilterBuilder::new().mode(FilterMode::LowPass).cutoff(f64::from(cutoff_hz)).build().0
        }
        BusEffect::HighPass { cutoff_hz } => {
            FilterBuilder::new().mode(FilterMode::HighPass).cutoff(f64::from(cutoff_hz)).build().0
        }
        BusEffect::Eq { kind, frequency_hz, gain_db, q } => {
            let kind = match kind {
                EqKind::Bell => EqFilterKind::Bell,
                EqKind::LowShelf => EqFilterKind::LowShelf,
                EqKind::HighShelf => EqFilterKind::HighShelf,
            };
            EqFilterBuilder::new(kind, f64::from(frequency_hz), Decibels(gain_db), f64::from(q)).build().0
        }
        BusEffect::Compressor { threshold_db, ratio, attack_ms, release_ms, makeup_db } => {
            CompressorBuilder::new()
                .threshold(f64::from(threshold_db))
                .ratio(f64::from(ratio))
                .attack_duration(Duration::from_secs_f32(attack_ms / 1000.0))
                .release_duration(Duration::from_secs_f32(release_ms / 1000.0))
                .makeup_gain(Decibels(makeup_db))
                .build()
                .0
        }
    }
}

/// One of an output profile's effects on the master (M6 U2): in the signal while its profile plays, crossing
/// over [`PROFILE_EASE_SECONDS`] each way, and out of it, unprocessed, otherwise. The dry signal it crosses
/// from goes in a buffer made before the mixer runs.
struct Switched {
    inner: Box<dyn Effect>,
    on: Arc<AtomicBool>,
    /// How far in, 0 to 1.
    share: f32,
    /// How far a frame moves `share`.
    step: f32,
    dry: Vec<Frame>,
}

impl Switched {
    fn new(inner: Box<dyn Effect>, on: Arc<AtomicBool>) -> Self {
        let share = if on.load(Ordering::Relaxed) { 1.0 } else { 0.0 };
        Self { inner, on, share, step: 1.0, dry: Vec::new() }
    }

    fn ease_at(&mut self, sample_rate: u32) {
        self.step = 1.0 / (PROFILE_EASE_SECONDS * sample_rate as f32).max(1.0);
    }
}

impl Effect for Switched {
    fn init(&mut self, sample_rate: u32, internal_buffer_size: usize) {
        self.ease_at(sample_rate);
        self.dry = vec![Frame::ZERO; internal_buffer_size];
        self.inner.init(sample_rate, internal_buffer_size);
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.ease_at(sample_rate);
        self.inner.on_change_sample_rate(sample_rate);
    }

    fn on_start_processing(&mut self) {
        self.inner.on_start_processing();
    }

    fn process(&mut self, input: &mut [Frame], dt: f64, info: &Info<'_>) {
        let target = if self.on.load(Ordering::Relaxed) { 1.0 } else { 0.0 };
        if target == 0.0 && self.share == 0.0 {
            return;
        }
        // Wholly in, or a buffer longer than the one it was made for, which kira never gives.
        if (target == 1.0 && self.share == 1.0) || input.len() > self.dry.len() {
            self.share = target;
            self.inner.process(input, dt, info);
            return;
        }
        let dry = &mut self.dry[..input.len()];
        dry.copy_from_slice(input);
        self.inner.process(input, dt, info);
        for (frame, dry) in input.iter_mut().zip(dry.iter()) {
            self.share = if target > self.share {
                (self.share + self.step).min(target)
            } else {
                (self.share - self.step).max(target)
            };
            *frame = *dry + (*frame - *dry) * self.share;
        }
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

impl<O: Output> AudioBackend for KiraBackend<O> {
    fn set_mix(&mut self, mix: &Mix) {
        // What plays on the last mix's buses goes with them.
        self.music.end(Duration::ZERO);
        for mut voice in self.voices.iter_mut().filter_map(Option::take) {
            voice.stop(Duration::ZERO);
        }
        match build_mix(&mut self.manager, mix) {
            Some(tracks) => self.mix = tracks,
            None => {
                tracing::error!(target: LOG_TARGET, "the mixer has no room for the mix, and keeps the last")
            }
        }
    }

    fn add_sound(&mut self, id: AssetId, sound: &DecodedSound) {
        let storage = SoundStorage::new(sound);
        self.decoded_bytes += storage.bytes();
        if let Some(old) = self.sounds.insert(id, Arc::new(storage)) {
            self.decoded_bytes -= old.bytes();
        }
    }

    fn remove_sound(&mut self, id: AssetId) {
        if let Some(old) = self.sounds.remove(&id) {
            self.decoded_bytes -= old.bytes();
        }
    }

    fn start_voice(
        &mut self,
        slot: VoiceSlot,
        sound: AssetId,
        bus: BusId,
        params: VoiceParams,
        frozen: bool,
    ) -> Result<(), StartError> {
        let storage = Arc::clone(self.sounds.get(&sound).ok_or(StartError::UnknownSound(sound))?);
        let voice = Voice { storage, settings: VoiceSettings::once(params, frozen) };
        let Some(BusTrack { track, .. }) = self.mix.buses.get_mut(bus.index()) else {
            tracing::warn!(target: LOG_TARGET, bus = bus.0, "a sound asked for a bus the mix hasn't");
            return Err(StartError::MixerFull);
        };
        let handle = track.play(voice).map_err(|error| {
            tracing::warn!(target: LOG_TARGET, %error, "the mixer refused a sound");
            StartError::MixerFull
        })?;
        self.voices[slot.0 as usize] = Some(handle);
        Ok(())
    }

    fn set_voice(&mut self, slot: VoiceSlot, params: VoiceParams) {
        let Some(voice) = self.voice(slot) else { return };
        voice.set(params, tween(VOICE_TWEEN));
    }

    fn freeze_voice(&mut self, slot: VoiceSlot, frozen: bool) {
        let Some(voice) = self.voice(slot) else { return };
        voice.freeze(frozen, FREEZE_TWEEN);
    }

    fn stop_voice(&mut self, slot: VoiceSlot) {
        if let Some(voice) = self.voice(slot) {
            voice.stop(FREEZE_TWEEN);
        }
    }

    fn voice_finished(&mut self, slot: VoiceSlot) -> bool {
        let finished = self.voice(slot).is_none_or(|voice| voice.finished());
        if finished {
            self.voices[slot.0 as usize] = None;
        }
        finished
    }

    fn set_bus(&mut self, bus: BusId, params: BusParams) {
        let Some(bus) = self.bus(bus) else { return };
        let Some(filter) = &mut bus.filter else { return };
        let moving = tween(FILTER_TWEEN);
        // Open, the filter is out of the signal: it colors even at the top of the range.
        let open = !params.low_pass_hz.is_finite();
        if open != bus.open {
            bus.open = open;
            filter.set_mix(if open { Share::DRY } else { Share::WET }, moving);
        }
        if !open {
            filter.set_cutoff(f64::from(params.low_pass_hz), moving);
        }
    }

    fn set_bus_gain(&mut self, bus: BusId, gain_db: f32, fade: Duration) {
        if let Some(bus) = self.bus(bus) {
            bus.track.set_volume(Decibels(gain_db), tween(fade));
        }
    }

    fn pause_bus(&mut self, bus: BusId, paused: bool, fade: Duration) {
        let Some(bus) = self.bus(bus) else { return };
        if paused {
            bus.track.pause(tween(fade));
        } else {
            bus.track.resume(tween(fade));
        }
    }

    fn set_output_profile(&mut self, profile: OutputProfile) {
        let mix = &mut self.mix;
        let find = |wanted: OutputProfile| mix.profiles.iter().position(|(each, _)| *each == wanted);
        let Some(place) = find(profile).or_else(|| find(OutputProfile::Home)) else { return };
        if place == mix.profile {
            return;
        }
        mix.profile = place;
        let volume_db = mix.profiles[place].1;
        let moving = tween(Duration::from_secs_f32(PROFILE_EASE_SECONDS));
        mix.master.set_volume(Decibels(volume_db), moving);
        for (send, own_db) in &mut mix.sends {
            send.set_volume(Decibels(*own_db + volume_db), moving);
        }
        for (of, on) in &mix.switches {
            on.store(*of == place, Ordering::Relaxed);
        }
    }

    fn load_music(&mut self, tempo_bpm: f32, stems: &[StemStart], fade: Duration) -> Result<(), StartError> {
        // Stopped before the stems go in, so none can start on the last score's clock.
        self.music.end(fade);
        self.music.clock.set_speed(ClockSpeed::TicksPerMinute(f64::from(tempo_bpm)), tween(Duration::ZERO));
        let first_beat = Start::OnClock(ClockTime::from_ticks_u64(&self.music.clock, 0));
        for stem in stems {
            let Some(storage) = self.sounds.get(&stem.sound) else {
                self.music.end(Duration::ZERO);
                return Err(StartError::UnknownSound(stem.sound));
            };
            let voice = Voice {
                storage: Arc::clone(storage),
                settings: VoiceSettings {
                    params: stem_params(stem.gain_db),
                    start: first_beat,
                    from: 0,
                    looped: Some(LoopRegion { start: 0, end: storage.frames() }),
                    frozen: false,
                },
            };
            match self.mix.buses[BusId::category(Bus::Music).index()].track.play(voice) {
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
            stem.set(stem_params(gain_db), Tween { start_time: at, duration: fade, ..Tween::default() });
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

    fn decoded_bytes(&self) -> u64 {
        self.decoded_bytes
    }
}

/// A stem at `gain_db`, as authored otherwise.
fn stem_params(gain_db: f32) -> VoiceParams {
    VoiceParams { gain_db, ..VoiceParams::NEUTRAL }
}
