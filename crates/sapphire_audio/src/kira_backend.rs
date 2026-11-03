//! [`KiraBackend`]: the [`AudioBackend`] on kira's mixer, over whichever [`Output`] pulls its audio.
//!
//! The mix is three sub-tracks, one a [`Bus`], each with room for every voice; the world's buses
//! carry a low-pass filter, out of the signal until a snapshot asks for it. A voice is a kira
//! static sound on its bus's track.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::sound::PlaybackState;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings};
use kira::track::{MainTrackBuilder, TrackBuilder, TrackHandle};
use kira::{
    AudioManager, AudioManagerSettings, Capacities, Decibels, Frame, Mix, Panning, PlaybackRate, Tween,
};
use sapphire_assets::AssetId;
use sapphire_assets::cue::Bus;
use sapphire_assets::sound::DecodedSound;

use crate::LOG_TARGET;
use crate::backend::{AudioBackend, BusParams, DeviceStatus, StartError, VoiceParams, VoiceSlot};
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

/// A bus: its track and, where the world's clock can reach it, its filter.
struct BusTrack {
    track: TrackHandle,
    filter: Option<FilterHandle>,
    open: bool,
}

/// The [`AudioBackend`] on kira. See the [crate docs](crate).
pub struct KiraBackend<O: Output> {
    manager: AudioManager<O>,
    buses: Vec<BusTrack>,
    sounds: HashMap<AssetId, StaticSoundData>,
    voices: Vec<Option<StaticSoundHandle>>,
}

impl<O: Output> KiraBackend<O> {
    /// Starts the mixer on an output made with `settings`.
    pub fn new(settings: O::Settings) -> Result<Self, O::Error> {
        let mut manager = AudioManager::<O>::new(AudioManagerSettings {
            capacities: Capacities::default(),
            main_track_builder: MainTrackBuilder::default(),
            internal_buffer_size: INTERNAL_BUFFER_FRAMES,
            backend_settings: settings,
        })?;
        let mut buses = Vec::with_capacity(Bus::ALL.len());
        for bus in Bus::ALL {
            let mut builder = TrackBuilder::new().sound_capacity(MAX_VOICES).sub_track_capacity(0);
            let filter = bus.follows_world_clock().then(|| {
                builder
                    .add_effect(FilterBuilder::new().mode(FilterMode::LowPass).cutoff(20_000.0).mix(Mix::DRY))
            });
            match manager.add_sub_track(builder) {
                Ok(track) => buses.push(BusTrack { track, filter, open: true }),
                // There is room for far more than three in a new mixer.
                Err(_) => unreachable!("a new mixer has room for its buses"),
            }
        }
        Ok(Self {
            manager,
            buses,
            sounds: HashMap::new(),
            voices: std::iter::repeat_with(|| None).take(MAX_VOICES).collect(),
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

    fn suspend(&mut self) {
        self.manager.backend_mut().suspend();
    }

    fn resume(&mut self) {
        self.manager.backend_mut().resume();
    }

    fn status(&mut self) -> DeviceStatus {
        self.manager.backend_mut().status()
    }
}
