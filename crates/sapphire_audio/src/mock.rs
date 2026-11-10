//! [`MockBackend`]: records what was asked, so game logic is tested with no device and no mixer.

use std::collections::HashSet;
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::Bus;
use sapphire_assets::sound::DecodedSound;

use crate::backend::{
    AudioBackend, BusParams, DeviceState, DeviceStatus, StartError, StemStart, VoiceParams, VoiceSlot,
};

/// One thing asked of a [`MockBackend`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MockCall {
    AddSound(AssetId),
    RemoveSound(AssetId),
    StartVoice {
        slot: VoiceSlot,
        sound: AssetId,
        bus: Bus,
        params: VoiceParams,
        frozen: bool,
    },
    SetVoice {
        slot: VoiceSlot,
        params: VoiceParams,
    },
    FreezeVoice {
        slot: VoiceSlot,
        frozen: bool,
    },
    StopVoice {
        slot: VoiceSlot,
    },
    SetBus {
        bus: Bus,
        params: BusParams,
    },
    SetBusGain {
        bus: Bus,
        gain_db: f32,
        fade: Duration,
    },
    /// The stems are [`MockBackend::stems`].
    LoadMusic {
        tempo_bpm: f32,
        fade: Duration,
    },
    PlayMusic,
    FadeStem {
        stem: usize,
        gain_db: f32,
        beat: u64,
        fade: Duration,
    },
    StopMusic {
        fade: Duration,
    },
    Suspend,
    Resume,
    SetMono(bool),
}

/// A backend that plays nothing and writes everything down. Voices last until [`Self::finish`], and
/// the music's clock reads what [`Self::set_music_beats`] says.
#[derive(Debug, Default)]
pub struct MockBackend {
    calls: Vec<MockCall>,
    sounds: HashSet<AssetId>,
    finished: HashSet<VoiceSlot>,
    state: DeviceState,
    stems: Vec<StemStart>,
    music_beats: Option<f64>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self { state: DeviceState::Running, ..Self::default() }
    }

    /// What was asked since the last [`Self::take_calls`].
    pub fn calls(&self) -> &[MockCall] {
        &self.calls
    }

    /// What was asked since the last call to this.
    pub fn take_calls(&mut self) -> Vec<MockCall> {
        std::mem::take(&mut self.calls)
    }

    /// Reserves room for this many calls, so recording them doesn't allocate.
    pub fn reserve_calls(&mut self, calls: usize) {
        self.calls.reserve(calls);
    }

    /// Plays the voice in `slot` out.
    pub fn finish(&mut self, slot: VoiceSlot) {
        self.finished.insert(slot);
    }

    /// Makes the output report `state`: a device lost, say.
    pub fn set_state(&mut self, state: DeviceState) {
        self.state = state;
    }

    /// The stems of the last [`MockCall::LoadMusic`].
    pub fn stems(&self) -> &[StemStart] {
        &self.stems
    }

    /// Makes the music's clock read `beats`.
    pub fn set_music_beats(&mut self, beats: Option<f64>) {
        self.music_beats = beats;
    }
}

impl AudioBackend for MockBackend {
    fn add_sound(&mut self, id: AssetId, _sound: &DecodedSound) {
        self.sounds.insert(id);
        self.calls.push(MockCall::AddSound(id));
    }

    fn remove_sound(&mut self, id: AssetId) {
        self.sounds.remove(&id);
        self.calls.push(MockCall::RemoveSound(id));
    }

    fn start_voice(
        &mut self,
        slot: VoiceSlot,
        sound: AssetId,
        bus: Bus,
        params: VoiceParams,
        frozen: bool,
    ) -> Result<(), StartError> {
        if !self.sounds.contains(&sound) {
            return Err(StartError::UnknownSound(sound));
        }
        self.finished.remove(&slot);
        self.calls.push(MockCall::StartVoice { slot, sound, bus, params, frozen });
        Ok(())
    }

    fn set_voice(&mut self, slot: VoiceSlot, params: VoiceParams) {
        self.calls.push(MockCall::SetVoice { slot, params });
    }

    fn freeze_voice(&mut self, slot: VoiceSlot, frozen: bool) {
        self.calls.push(MockCall::FreezeVoice { slot, frozen });
    }

    fn stop_voice(&mut self, slot: VoiceSlot) {
        self.calls.push(MockCall::StopVoice { slot });
    }

    fn voice_finished(&mut self, slot: VoiceSlot) -> bool {
        self.finished.contains(&slot)
    }

    fn set_bus(&mut self, bus: Bus, params: BusParams) {
        self.calls.push(MockCall::SetBus { bus, params });
    }

    fn set_bus_gain(&mut self, bus: Bus, gain_db: f32, fade: Duration) {
        self.calls.push(MockCall::SetBusGain { bus, gain_db, fade });
    }

    fn load_music(&mut self, tempo_bpm: f32, stems: &[StemStart], fade: Duration) -> Result<(), StartError> {
        if let Some(stem) = stems.iter().find(|stem| !self.sounds.contains(&stem.sound)) {
            return Err(StartError::UnknownSound(stem.sound));
        }
        self.stems.clear();
        self.stems.extend_from_slice(stems);
        self.calls.push(MockCall::LoadMusic { tempo_bpm, fade });
        Ok(())
    }

    fn play_music(&mut self) {
        self.calls.push(MockCall::PlayMusic);
    }

    fn fade_stem(&mut self, stem: usize, gain_db: f32, beat: u64, fade: Duration) {
        self.calls.push(MockCall::FadeStem { stem, gain_db, beat, fade });
    }

    fn stop_music(&mut self, fade: Duration) {
        self.calls.push(MockCall::StopMusic { fade });
    }

    fn music_beats(&mut self) -> Option<f64> {
        self.music_beats
    }

    fn suspend(&mut self) {
        self.state = DeviceState::Suspended;
        self.calls.push(MockCall::Suspend);
    }

    fn resume(&mut self) {
        self.state = DeviceState::Running;
        self.calls.push(MockCall::Resume);
    }

    fn status(&mut self) -> DeviceStatus {
        DeviceStatus { state: self.state, ..DeviceStatus::default() }
    }

    fn set_mono(&mut self, mono: bool) {
        self.calls.push(MockCall::SetMono(mono));
    }
}
