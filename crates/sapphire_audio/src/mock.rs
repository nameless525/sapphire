//! [`MockBackend`]: records what was asked, so game logic is tested with no device and no mixer.

use std::collections::HashSet;

use sapphire_assets::AssetId;
use sapphire_assets::cue::Bus;
use sapphire_assets::sound::DecodedSound;

use crate::backend::{
    AudioBackend, BusParams, DeviceState, DeviceStatus, StartError, VoiceParams, VoiceSlot,
};

/// One thing asked of a [`MockBackend`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MockCall {
    AddSound(AssetId),
    RemoveSound(AssetId),
    StartVoice { slot: VoiceSlot, sound: AssetId, bus: Bus, params: VoiceParams, frozen: bool },
    SetVoice { slot: VoiceSlot, params: VoiceParams },
    FreezeVoice { slot: VoiceSlot, frozen: bool },
    StopVoice { slot: VoiceSlot },
    SetBus { bus: Bus, params: BusParams },
    Suspend,
    Resume,
}

/// A backend that plays nothing and writes everything down. Voices last until [`Self::finish`].
#[derive(Debug, Default)]
pub struct MockBackend {
    calls: Vec<MockCall>,
    sounds: HashSet<AssetId>,
    finished: HashSet<VoiceSlot>,
    state: DeviceState,
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
}
