//! [`AudioSystem`]: cues, lines and music in, voices out, once a frame.

use std::collections::HashMap;
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Attenuation, Bus, Cue, HitstopResponse, SILENCE_DB};
use sapphire_assets::score::Score;
use sapphire_assets::sound::DecodedSound;
use sapphire_assets::voice::VoiceLines;
use sapphire_math::Vec3;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::backend::{AudioBackend, DeviceState, DeviceStatus, StartError, VoiceParams, VoiceSlot};
use crate::music::{Music, MusicRequest};
use crate::rng::CueRandom;
use crate::snapshot::{ClockSnapshot, SnapshotMix, SnapshotTuning};
use crate::voice::{LineKey, VoiceRoute, VoiceRouteError};

/// Voices playing at once. A duel's sounds are short and few; past this a request is dropped and
/// counted, rather than cutting off what is already playing.
pub const MAX_VOICES: usize = 32;

/// Emitters tracked at once: the actors a frame reports, and those gone with voices still ringing.
const MAX_EMITTERS: usize = 64;

/// A voice moves only when its gain changes by this many decibels, ...
const GAIN_STEP_DB: f32 = 0.05;
/// ... its pan by this, ...
const PAN_STEP: f32 = 0.005;
/// ... or its pitch by this: smaller steps are all smoothed away, and each costs the audio thread a
/// command.
const PITCH_STEP: f32 = 0.001;

/// How fast the music goes down under a voice line or the player's volume, ...
pub(crate) const DUCK_ATTACK: Duration = Duration::from_millis(80);
/// ... and comes back after it: slower, so that a pause between two lines doesn't pump.
pub(crate) const DUCK_RELEASE: Duration = Duration::from_millis(400);

/// An actor sounds can be attached to, named however the caller likes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EmitterId(pub u32);

/// An emitter this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emitter {
    pub id: EmitterId,
    /// Where it stands, in meters.
    pub position: Vec3,
    /// Whether its owner's clock is stopped by hitstop: attached voices do what their cue's
    /// `hitstop` says. An emitter no frame reports any more is out of hitstop.
    pub hitstop: bool,
}

/// Where the sound is heard from: the camera, usually.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub position: Vec3,
    /// The way it faces. Need not be normalized.
    pub forward: Vec3,
    /// Its up. Need not be normalized, or square to `forward`.
    pub up: Vec3,
}

impl Default for Listener {
    fn default() -> Self {
        Self { position: Vec3::ZERO, forward: Vec3::NEG_Z, up: Vec3::Y }
    }
}

/// Where a cue plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CueTarget {
    /// Nowhere: flat and centered, at its volume.
    Flat,
    /// At a point in the world, staying there.
    At(Vec3),
    /// On an emitter, following it and answering its hitstop. The emitter must be in this frame's
    /// [`AudioFrame::emitters`] or still be tracked for a voice on it.
    On(EmitterId),
}

/// One cue to play.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CueRequest {
    /// The cue, added with [`AudioSystem::add_cue`].
    pub cue: AssetId,
    pub target: CueTarget,
}

/// One voice line to speak, in the chosen voice language: a cue that language's
/// [`VoiceLines`] name for its key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineRequest {
    pub line: LineKey,
    pub target: CueTarget,
}

/// Everything a frame tells the audio system.
#[derive(Clone, Copy, Debug)]
pub struct AudioFrame<'a> {
    /// Real seconds since the last frame.
    pub dt: f32,
    pub listener: Listener,
    /// The world clock's speed: 1 is normal, and less drives the snapshot.
    pub world_time_scale: f32,
    /// The emitters that exist this frame.
    pub emitters: &'a [Emitter],
    /// The cues to start this frame.
    pub cues: &'a [CueRequest],
    /// The voice lines to start this frame, after the cues.
    pub lines: &'a [LineRequest],
    /// The score and phase to play, or none: what played fades out.
    pub music: Option<MusicRequest<'a>>,
}

/// The system's counters (rule R9). [`Self::update`] is what spec §3.2's "Audio update" row budgets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioStats {
    /// The last [`AudioSystem::update`] took this long, on the calling thread.
    pub update: Duration,
    /// The longest update since [`AudioSystem::take_peak`].
    pub update_peak: Duration,
    /// Voices live now.
    pub voices: u32,
    /// Cues that started a voice, all told.
    pub cues_played: u64,
    /// Cues dropped because the output isn't playing (suspended, no device).
    pub dropped_no_output: u64,
    /// Cues dropped because [`MAX_VOICES`] voices were live or the mixer was full.
    pub dropped_voice_limit: u64,
    /// Cues dropped for naming a cue, a sound or an emitter nobody added.
    pub dropped_unknown: u64,
    /// Lines asked for that the voice language has, all told: each plays as a cue does.
    pub lines_asked: u64,
    /// Lines the voice language lacks, or asked for with no language chosen.
    pub lines_missing: u64,
    /// Changes to a phase the score has, all told.
    pub music_phase_changes: u64,
    /// Changes to a phase the score lacks, which leave the stems as they were.
    pub music_unknown_phases: u64,
    /// The output's own counters.
    pub device: DeviceStatus,
}

/// A voice's position, as it was asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Place {
    Flat,
    At(Vec3),
    On(EmitterId),
}

/// The player's volumes (M4 A2), each a share of full: every sound's, and on top of it each kind's. The
/// effects' is the world's buses' and the interface's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volumes {
    pub all: f32,
    pub music: f32,
    pub effects: f32,
    pub voice: f32,
}

impl Volumes {
    /// Everything at full, as a mix starts.
    pub const FULL: Self = Self { all: 1.0, music: 1.0, effects: 1.0, voice: 1.0 };
}

impl Default for Volumes {
    fn default() -> Self {
        Self::FULL
    }
}

/// A live voice.
#[derive(Clone, Copy, Debug)]
struct Voice {
    slot: VoiceSlot,
    place: Place,
    bus: Bus,
    attenuation: Option<Attenuation>,
    hitstop: HitstopResponse,
    /// The gain and pitch the cue picked.
    gain_db: f32,
    pitch: f32,
    /// What the backend was last told.
    sent: VoiceParams,
    frozen: bool,
    /// Told to end, and only waiting to.
    stopped: bool,
}

/// A tracked emitter.
#[derive(Clone, Copy, Debug)]
struct Tracked {
    emitter: Emitter,
    /// In this frame's list.
    reported: bool,
}

/// Plays cues on a backend: picks their variation, places them relative to the listener, and keeps
/// them in step with the world's clock and each actor's hitstop. See the [crate docs](crate).
pub struct AudioSystem<B: AudioBackend> {
    backend: B,
    cues: HashMap<AssetId, Cue>,
    voices: Vec<Voice>,
    /// Slots no voice holds, as a stack.
    free_slots: Vec<VoiceSlot>,
    emitters: Vec<Tracked>,
    snapshot: ClockSnapshot,
    mix: SnapshotMix,
    music: Music,
    voice: VoiceRoute,
    /// The music bus's gain, as last sent: the player's volume, and down under a voice line.
    music_gain_db: f32,
    background: bool,
    interrupted: bool,
    /// What the player's volume takes off every voice.
    volume_db: f32,
    /// What the player's volume for each bus takes off its voices on top (M4 A2), by [`Bus::index`].
    bus_volume_db: [f32; Bus::ALL.len()],
    stats: AudioStats,
    /// A dropped cue's reason has been logged once.
    warned_unknown: bool,
    warned_no_output: bool,
}

impl<B: AudioBackend> AudioSystem<B> {
    /// A system with nothing loaded, on `backend`, under the default snapshot.
    pub fn new(backend: B) -> Self {
        Self::with_tuning(backend, SnapshotTuning::default())
    }

    pub fn with_tuning(backend: B, tuning: SnapshotTuning) -> Self {
        Self {
            backend,
            cues: HashMap::new(),
            voices: Vec::with_capacity(MAX_VOICES),
            free_slots: (0..MAX_VOICES as u32).rev().map(VoiceSlot).collect(),
            emitters: Vec::with_capacity(MAX_EMITTERS),
            snapshot: ClockSnapshot::new(tuning),
            mix: SnapshotMix::NEUTRAL,
            music: Music::default(),
            voice: VoiceRoute::new(),
            music_gain_db: 0.0,
            background: false,
            interrupted: false,
            volume_db: 0.0,
            bus_volume_db: [0.0; Bus::ALL.len()],
            stats: AudioStats::default(),
            warned_unknown: false,
            warned_no_output: false,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Makes a cue playable. Load-time work, as is [`Self::add_sound`] for each of its sounds.
    pub fn add_cue(&mut self, id: AssetId, cue: Cue) {
        self.cues.insert(id, cue);
    }

    /// Makes a decoded sound playable.
    pub fn add_sound(&mut self, id: AssetId, sound: &DecodedSound) {
        self.backend.add_sound(id, sound);
    }

    /// Makes a score playable, once its stems' sounds are added. Load-time work.
    pub fn add_score(&mut self, id: AssetId, score: Score) {
        self.music.add_score(id, score);
    }

    /// Makes one language's voice lines speakable, once their cues are added. Load-time work.
    pub fn add_voice_lines(&mut self, lines: &VoiceLines) -> Result<(), VoiceRouteError> {
        self.voice.add(lines)
    }

    /// Chooses the voice language, apart from the text's; whether any lines are in it.
    pub fn set_voice_language(&mut self, locale: &str) -> bool {
        self.voice.set_language(locale)
    }

    /// The lines by language, and the one chosen.
    pub fn voice_route(&self) -> &VoiceRoute {
        &self.voice
    }

    /// The app went to the background or came back (spec §6.5): the mixer stops or starts asking
    /// the device for audio. Voices wait where they were, and cues asked for meanwhile are dropped, from
    /// this call on, whatever the output has got round to.
    pub fn set_background(&mut self, background: bool) {
        self.set_suspension(background, self.interrupted);
    }

    /// The player's volume, a share of full: every voice and the music follow at the next update.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume_db = decibels_of(volume);
    }

    /// The player's volumes (M4 A2): [`Self::set_volume`]'s, and each bus's.
    pub fn set_volumes(&mut self, volumes: Volumes) {
        self.set_volume(volumes.all);
        for bus in Bus::ALL {
            let volume = match bus {
                Bus::Sfx | Bus::Ambience | Bus::Ui => volumes.effects,
                Bus::Music => volumes.music,
                Bus::Voice => volumes.voice,
            };
            self.set_bus_volume(bus, volume);
        }
    }

    /// The player's volume for one bus (M4 A2), a share of full on top of [`Self::set_volume`]'s: its
    /// voices, and for the music its stems, follow at the next update.
    pub fn set_bus_volume(&mut self, bus: Bus, volume: f32) {
        self.bus_volume_db[bus.index()] = decibels_of(volume);
    }

    /// Folds the whole mix to mono, or back (M4 A2): for a player who hears with one ear, or through one
    /// speaker, so nothing panned away is lost.
    pub fn set_mono(&mut self, mono: bool) {
        self.backend.set_mono(mono);
    }

    /// The OS took audio away or gave it back (iOS: calls, Siri).
    pub fn set_interrupted(&mut self, interrupted: bool) {
        self.set_suspension(self.background, interrupted);
    }

    fn set_suspension(&mut self, background: bool, interrupted: bool) {
        let (was, now) = (self.background || self.interrupted, background || interrupted);
        (self.background, self.interrupted) = (background, interrupted);
        if was != now {
            tracing::info!(target: LOG_TARGET, background, interrupted, "audio {}", if now { "suspended" } else { "resumed" });
            if now {
                self.backend.suspend();
            } else {
                self.backend.resume();
            }
        }
    }

    /// Ends every voice playing, at once as far as anyone hears: a new fight has no use for the last
    /// one's sounds. Their slots come back as they finish, and cues go on playing as usual, this
    /// frame's among them. Suspended, they end as the mixer next runs.
    pub fn stop_voices(&mut self) {
        for voice in self.voices.iter_mut().filter(|voice| !voice.stopped) {
            voice.stopped = true;
            self.backend.stop_voice(voice.slot);
        }
    }

    /// The counters, as of the last [`Self::update`].
    pub fn stats(&self) -> AudioStats {
        self.stats
    }

    /// Starts a new window for [`AudioStats::update_peak`].
    pub fn take_peak(&mut self) -> Duration {
        std::mem::take(&mut self.stats.update_peak)
    }

    /// One frame: takes the emitters, moves the snapshot, starts the frame's cues and lines, follows
    /// the music's phase, and brings every voice and the ducking up to date. Picks come from `random`,
    /// in request order, lines after cues: one number for the sound, one for the volume, one for the
    /// pitch, whatever the cue's ranges.
    pub fn update(&mut self, frame: &AudioFrame<'_>, random: &mut impl CueRandom) {
        profiling::scope!("AudioSystem::update");
        let timer = Stopwatch::start();
        self.stats.device = self.backend.status();
        // What was asked comes first: `CpalOutput` stops on a thread of its own and reads `Running` for
        // a while after, when a voice started now would sound, or wait to sound on the way back. The
        // stats keep the output's own word.
        let suspended = self.background || self.interrupted;
        let state = if suspended { DeviceState::Suspended } else { self.stats.device.state };

        if state == DeviceState::Running {
            self.reap_voices();
        }
        self.track_emitters(frame.emitters);
        self.step_snapshot(frame);
        for request in frame.cues {
            if state.plays() {
                self.start_cue(request, &frame.listener, random);
            } else {
                self.stats.dropped_no_output += 1;
                if !self.warned_no_output {
                    self.warned_no_output = true;
                    tracing::warn!(target: LOG_TARGET, %state, "dropping cues while the output doesn't play");
                }
            }
        }
        for request in frame.lines {
            let Some(cue) = self.voice.line(request.line) else {
                self.stats.lines_missing += 1;
                continue;
            };
            self.stats.lines_asked += 1;
            if state.plays() {
                self.start_cue(&CueRequest { cue, target: request.target }, &frame.listener, random);
            } else {
                self.stats.dropped_no_output += 1;
            }
        }
        self.music.update(frame.music, state.plays(), &mut self.backend);
        self.stats.music_phase_changes = self.music.counts.phase_changes;
        self.stats.music_unknown_phases = self.music.counts.unknown_phases;
        if state.plays() {
            self.update_voices(&frame.listener);
        }
        self.follow_music_gain();

        self.stats.voices = self.voices.len() as u32;
        self.stats.update = timer.elapsed();
        self.stats.update_peak = self.stats.update_peak.max(self.stats.update);
    }

    fn track_emitters(&mut self, emitters: &[Emitter]) {
        for tracked in &mut self.emitters {
            tracked.reported = false;
        }
        for &emitter in emitters {
            if let Some(tracked) = self.emitters.iter_mut().find(|tracked| tracked.emitter.id == emitter.id) {
                *tracked = Tracked { emitter, reported: true };
            }
        }
        // Out of the frame's list: gone, though its voices ring on where it stood, unfrozen.
        let voices = &self.voices;
        self.emitters.retain_mut(|tracked| {
            if !tracked.reported {
                tracked.emitter.hitstop = false;
            }
            tracked.reported || voices.iter().any(|voice| voice.place == Place::On(tracked.emitter.id))
        });
        // Reclaim retired entries before admitting replacements. Preserve request order under
        // overload and update existing entries above even when no new entries will fit.
        for &emitter in emitters {
            if let Some(tracked) = self.emitters.iter_mut().find(|tracked| tracked.emitter.id == emitter.id) {
                // Repeated IDs retain the existing last-report-wins position/hitstop policy,
                // including newly admitted entries and reports later in an overloaded frame.
                *tracked = Tracked { emitter, reported: true };
            } else if self.emitters.len() < MAX_EMITTERS {
                self.emitters.push(Tracked { emitter, reported: true });
            }
        }
    }

    fn reap_voices(&mut self) {
        let (backend, free_slots) = (&mut self.backend, &mut self.free_slots);
        self.voices.retain(|voice| {
            let finished = backend.voice_finished(voice.slot);
            if finished {
                free_slots.push(voice.slot);
            }
            !finished
        });
    }

    fn step_snapshot(&mut self, frame: &AudioFrame<'_>) {
        let mix = self.snapshot.step(frame.world_time_scale, frame.dt);
        if mix.bus != self.mix.bus {
            for bus in Bus::ALL {
                if bus.follows_world_clock() {
                    self.backend.set_bus(bus, mix.bus);
                }
            }
        }
        self.mix = mix;
    }

    fn start_cue(&mut self, request: &CueRequest, listener: &Listener, random: &mut impl CueRandom) {
        let Some(cue) = self.cues.get(&request.cue) else {
            return self.count_unknown(|| format!("cue {} isn't loaded", request.cue));
        };
        // Always the same three numbers, so a change in one cue's ranges moves no other's picks.
        let (pick, volume, pitch) = (random.next_u32(), random.next_unit(), random.next_unit());
        let sound = cue.sounds[pick as usize % cue.sounds.len()];
        let place = match request.target {
            CueTarget::Flat => Place::Flat,
            CueTarget::At(position) => Place::At(position),
            CueTarget::On(id) => Place::On(id),
        };
        if let Place::On(id) = place
            && !self.emitters.iter().any(|tracked| tracked.emitter.id == id)
        {
            return self.count_unknown(|| format!("emitter {id:?} isn't in this frame"));
        }
        let Some(slot) = self.free_slots.pop() else {
            self.stats.dropped_voice_limit += 1;
            return;
        };
        let [quiet, loud] = cue.volume_db;
        let [slow, fast] = cue.pitch;
        let mut voice = Voice {
            slot,
            place,
            bus: cue.bus,
            attenuation: cue.attenuation,
            hitstop: cue.hitstop,
            gain_db: quiet + (loud - quiet) * volume,
            pitch: slow + (fast - slow) * pitch,
            sent: VoiceParams { gain_db: 0.0, pan: 0.0, pitch: 1.0 },
            frozen: false,
            stopped: false,
        };
        let in_hitstop = self.in_hitstop(&voice);
        voice.frozen = in_hitstop && voice.hitstop == HitstopResponse::Freeze;
        voice.sent = self.params_of(&voice, listener);
        match self.backend.start_voice(slot, sound, voice.bus, voice.sent, voice.frozen) {
            Ok(()) => {
                self.voices.push(voice);
                self.stats.cues_played += 1;
            }
            Err(error) => {
                self.free_slots.push(slot);
                match error {
                    StartError::UnknownSound(_) => self.count_unknown(|| error.to_string()),
                    StartError::MixerFull => self.stats.dropped_voice_limit += 1,
                }
            }
        }
    }

    fn count_unknown(&mut self, message: impl FnOnce() -> String) {
        self.stats.dropped_unknown += 1;
        if !self.warned_unknown {
            self.warned_unknown = true;
            tracing::warn!(target: LOG_TARGET, "dropping a cue: {}", message());
        }
    }

    fn update_voices(&mut self, listener: &Listener) {
        for index in 0..self.voices.len() {
            let voice = self.voices[index];
            // Thawing one would resume it, which cancels its stop.
            if voice.stopped {
                continue;
            }
            let frozen = self.in_hitstop(&voice) && voice.hitstop == HitstopResponse::Freeze;
            if frozen != voice.frozen {
                self.backend.freeze_voice(voice.slot, frozen);
                self.voices[index].frozen = frozen;
            }
            let params = self.params_of(&voice, listener);
            if moved(params, voice.sent) {
                self.backend.set_voice(voice.slot, params);
                self.voices[index].sent = params;
            }
        }
    }

    /// Keeps the music at the player's volume, as every voice is, and takes it further down while a voice
    /// line plays. Its stems' gains are the score's, so the volume is the bus's.
    fn follow_music_gain(&mut self) {
        let speaking = self.voices.iter().any(|voice| voice.bus == Bus::Voice && !voice.stopped);
        let duck_db = if speaking { self.music.duck_db() } else { 0.0 };
        let volume_db = self.volume_db + self.bus_volume_db[Bus::Music.index()];
        let gain_db = (volume_db + duck_db).max(SILENCE_DB);
        if gain_db != self.music_gain_db {
            let fade = if gain_db < self.music_gain_db { DUCK_ATTACK } else { DUCK_RELEASE };
            self.backend.set_bus_gain(Bus::Music, gain_db, fade);
            self.music_gain_db = gain_db;
        }
    }

    fn in_hitstop(&self, voice: &Voice) -> bool {
        let Place::On(id) = voice.place else { return false };
        self.emitters.iter().any(|tracked| tracked.emitter.id == id && tracked.emitter.hitstop)
    }

    /// What `voice` should sound like now.
    fn params_of(&self, voice: &Voice, listener: &Listener) -> VoiceParams {
        let position = match voice.place {
            Place::Flat => None,
            Place::At(position) => Some(position),
            Place::On(id) => self
                .emitters
                .iter()
                .find(|tracked| tracked.emitter.id == id)
                .map(|tracked| tracked.emitter.position),
        };
        let mut gain_db = voice.gain_db + self.volume_db + self.bus_volume_db[voice.bus.index()];
        let mut pan = 0.0;
        if let (Some(position), Some(attenuation)) = (position, voice.attenuation) {
            let (distance, side) = relative(listener, position);
            gain_db += attenuation.gain_db(distance);
            // Inside the near radius the sound surrounds the listener, so it eases to the center.
            let inside = if attenuation.near > 0.0 { (distance / attenuation.near).min(1.0) } else { 1.0 };
            pan = side * inside;
        }
        if let HitstopResponse::Duck(db) = voice.hitstop
            && self.in_hitstop(voice)
        {
            gain_db += db;
        }
        let pitch = if voice.bus.follows_world_clock() { voice.pitch * self.mix.pitch } else { voice.pitch };
        VoiceParams { gain_db, pan, pitch }
    }
}

/// A share of full volume in decibels. Below [`SILENCE_DB`] the mixer's silent anyway, and it's finite
/// whatever it's given: see `VoiceParams`.
fn decibels_of(volume: f32) -> f32 {
    if volume > 0.0 { (20.0 * volume.log10()).clamp(SILENCE_DB, 0.0) } else { SILENCE_DB }
}

/// Distance to `position` and how far to the listener's right it lies, -1 to 1.
fn relative(listener: &Listener, position: Vec3) -> (f32, f32) {
    let offset = position - listener.position;
    let distance = offset.length();
    let right = listener.forward.cross(listener.up).normalize_or_zero();
    if distance <= f32::EPSILON || right == Vec3::ZERO {
        return (distance, 0.0);
    }
    (distance, (offset.dot(right) / distance).clamp(-1.0, 1.0))
}

fn moved(now: VoiceParams, sent: VoiceParams) -> bool {
    (now.gain_db - sent.gain_db).abs() > GAIN_STEP_DB
        || (now.pan - sent.pan).abs() > PAN_STEP
        || (now.pitch - sent.pitch).abs() > PITCH_STEP
}

#[cfg(test)]
mod tests {
    use sapphire_assets::cue::AttenuationCurve;

    use super::*;
    use crate::backend::BusParams;
    use crate::mock::{MockBackend, MockCall};
    use crate::rng::CueRng;

    const FRAME: f32 = 1.0 / 60.0;
    const SOUND_A: AssetId = AssetId::from_text("4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c");
    const SOUND_B: AssetId = AssetId::from_text("0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b");
    const CUE: AssetId = AssetId::from_text("11111111-2222-4333-8444-555555555555");
    const ACTOR: EmitterId = EmitterId(7);

    fn decoded() -> DecodedSound {
        DecodedSound { sample_rate: 48_000, channels: 1, samples: vec![0.5; 480] }
    }

    fn plain_cue() -> Cue {
        Cue {
            sounds: vec![SOUND_A],
            volume_db: [0.0, 0.0],
            pitch: [1.0, 1.0],
            attenuation: None,
            bus: Bus::Sfx,
            hitstop: HitstopResponse::Ignore,
        }
    }

    fn placed_cue() -> Cue {
        Cue {
            attenuation: Some(Attenuation { near: 1.0, far: 21.0, curve: AttenuationCurve::Linear }),
            ..plain_cue()
        }
    }

    fn system_with(cue: Cue) -> AudioSystem<MockBackend> {
        let mut system = AudioSystem::new(MockBackend::new());
        system.add_sound(SOUND_A, &decoded());
        system.add_sound(SOUND_B, &decoded());
        system.add_cue(CUE, cue);
        system.backend_mut().take_calls();
        system
    }

    /// A frame at normal speed, with a listener at the origin facing -Z.
    fn frame<'a>(emitters: &'a [Emitter], cues: &'a [CueRequest]) -> AudioFrame<'a> {
        AudioFrame {
            dt: FRAME,
            listener: Listener::default(),
            world_time_scale: 1.0,
            emitters,
            cues,
            lines: &[],
            music: None,
        }
    }

    fn play(target: CueTarget) -> [CueRequest; 1] {
        [CueRequest { cue: CUE, target }]
    }

    fn actor_at(position: Vec3) -> Emitter {
        Emitter { id: ACTOR, position, hitstop: false }
    }

    /// The parameters of the first voice started since the last `take_calls`.
    fn started(system: &mut AudioSystem<MockBackend>) -> VoiceParams {
        system
            .backend_mut()
            .take_calls()
            .into_iter()
            .find_map(|call| match call {
                MockCall::StartVoice { params, .. } => Some(params),
                _ => None,
            })
            .expect("a voice started")
    }

    #[test]
    fn a_flat_cue_plays_as_authored() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        assert_eq!(started(&mut system), VoiceParams { gain_db: 0.0, pan: 0.0, pitch: 1.0 });
        assert_eq!(system.stats().cues_played, 1);
        assert_eq!(system.stats().voices, 1);
    }

    #[test]
    fn the_seed_pins_every_pick() {
        let cue =
            Cue { sounds: vec![SOUND_A, SOUND_B], volume_db: [-6.0, 0.0], pitch: [0.8, 1.2], ..plain_cue() };
        let picks = |seed| {
            let mut system = system_with(cue.clone());
            let mut rng = CueRng::new(seed);
            let mut seen = Vec::new();
            for _ in 0..8 {
                system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
                seen.extend(system.backend_mut().take_calls().into_iter().filter_map(|call| match call {
                    MockCall::StartVoice { sound, params, .. } => Some((sound, params.gain_db, params.pitch)),
                    _ => None,
                }));
            }
            seen
        };
        let first = picks(3);
        assert_eq!(first.len(), 8);
        assert_eq!(first, picks(3), "one seed, one sequence");
        assert_ne!(first, picks(4));
        assert!(first.iter().any(|pick| pick.0 == SOUND_A) && first.iter().any(|pick| pick.0 == SOUND_B));
        for (_, gain, pitch) in &first {
            assert!((-6.0..=0.0).contains(gain), "{gain}");
            assert!((0.8..=1.2).contains(pitch), "{pitch}");
        }
    }

    #[test]
    fn a_placed_cue_fades_with_distance() {
        // near 1, far 21, linear in decibels: at 11 m, halfway, -30 dB.
        let rows =
            [(0.5, 0.0), (1.0, 0.0), (11.0, SILENCE_DB * 0.5), (21.0, SILENCE_DB), (500.0, SILENCE_DB)];
        for (meters, gain) in rows {
            let mut system = system_with(placed_cue());
            let mut rng = CueRng::new(1);
            let at = Vec3::new(0.0, 0.0, -meters);
            system.update(&frame(&[], &play(CueTarget::At(at))), &mut rng);
            let params = started(&mut system);
            assert!((params.gain_db - gain).abs() < 1e-3, "{meters} m: {params:?}");
            assert_eq!(params.pan, 0.0, "straight ahead");
        }
    }

    #[test]
    fn the_listeners_right_is_the_pan_whichever_way_it_faces() {
        let mut rng = CueRng::new(1);
        let start = |listener: Listener, position: Vec3, rng: &mut CueRng| {
            let mut system = system_with(placed_cue());
            let cues = play(CueTarget::At(position));
            system.update(&AudioFrame { listener, ..frame(&[], &cues) }, rng);
            started(&mut system).pan
        };
        let facing_away = Listener::default();
        assert!((start(facing_away, Vec3::new(5.0, 0.0, 0.0), &mut rng) - 1.0).abs() < 1e-5);
        assert!((start(facing_away, Vec3::new(-5.0, 0.0, 0.0), &mut rng) + 1.0).abs() < 1e-5);
        // Turned round, what was right is left.
        let facing_back = Listener { forward: Vec3::Z, ..facing_away };
        assert!((start(facing_back, Vec3::new(5.0, 0.0, 0.0), &mut rng) + 1.0).abs() < 1e-5);
        // Moved beyond the sound, it is on the other side.
        let moved_past = Listener { position: Vec3::new(10.0, 0.0, 0.0), ..facing_away };
        assert!((start(moved_past, Vec3::new(5.0, 0.0, 0.0), &mut rng) + 1.0).abs() < 1e-5);
        // Within the near radius it eases to the center; on top of the listener it is centered.
        let close = start(facing_away, Vec3::new(0.5, 0.0, 0.0), &mut rng);
        assert!((close - 0.5).abs() < 1e-5, "{close}");
        assert_eq!(start(facing_away, Vec3::ZERO, &mut rng), 0.0);
    }

    #[test]
    fn the_players_volume_reaches_a_voice_playing_and_one_to_come() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        assert_eq!(started(&mut system).gain_db, 0.0);
        system.set_volume(0.5);
        system.update(&frame(&[], &[]), &mut rng);
        let calls = system.backend_mut().take_calls();
        let [MockCall::SetVoice { params, .. }, MockCall::SetBusGain { bus: Bus::Music, gain_db, .. }] =
            calls[..]
        else {
            panic!("{calls:?}")
        };
        assert!((params.gain_db + 6.02).abs() < 0.01, "{params:?}");
        assert_eq!(gain_db, params.gain_db, "the music with it");
        system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        assert!((started(&mut system).gain_db + 6.02).abs() < 0.01);
        // Off is silent, and nothing a setting says can reach the mixer as a bad number.
        for volume in [0.0, -1.0, f32::NAN] {
            system.set_volume(volume);
            assert_eq!(system.volume_db, SILENCE_DB, "{volume}");
        }
        system.set_volume(3.0);
        assert_eq!(system.volume_db, 0.0);
    }

    #[test]
    fn each_kinds_volume_reaches_its_buses_on_top_of_every_sounds() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        assert_eq!(started(&mut system).gain_db, 0.0);
        // The voices' and the music's leave an effect alone.
        system.set_volumes(Volumes { voice: 0.1, music: 0.1, ..Volumes::FULL });
        system.update(&frame(&[], &[]), &mut rng);
        let calls = system.backend_mut().take_calls();
        let [MockCall::SetBusGain { bus: Bus::Music, gain_db, .. }] = calls[..] else { panic!("{calls:?}") };
        assert!((gain_db + 20.0).abs() < 0.01, "{gain_db}");
        // The effects' and every sound's add up.
        system.set_volumes(Volumes { all: 0.5, effects: 0.5, ..Volumes::FULL });
        system.update(&frame(&[], &[]), &mut rng);
        let calls = system.backend_mut().take_calls();
        let [MockCall::SetVoice { params, .. }, MockCall::SetBusGain { bus: Bus::Music, gain_db, .. }] =
            calls[..]
        else {
            panic!("{calls:?}")
        };
        assert!((params.gain_db + 12.04).abs() < 0.01, "{params:?}");
        assert!((gain_db + 6.02).abs() < 0.01, "the music under every sound's alone: {gain_db}");
        assert_eq!(Volumes::default(), Volumes::FULL);
    }

    #[test]
    fn mono_reaches_the_mixer() {
        let mut system = system_with(plain_cue());
        system.set_mono(true);
        system.set_mono(false);
        assert_eq!(system.backend_mut().take_calls(), [MockCall::SetMono(true), MockCall::SetMono(false)]);
    }

    #[test]
    fn a_voice_follows_its_emitter() {
        let mut system = system_with(placed_cue());
        let mut rng = CueRng::new(1);
        let near = [actor_at(Vec3::new(0.0, 0.0, -1.0))];
        system.update(&frame(&near, &play(CueTarget::On(ACTOR))), &mut rng);
        assert_eq!(started(&mut system).gain_db, 0.0);
        let far = [actor_at(Vec3::new(0.0, 0.0, -11.0))];
        system.update(&frame(&far, &[]), &mut rng);
        let calls = system.backend_mut().take_calls();
        let [MockCall::SetVoice { params, .. }] = calls[..] else { panic!("{calls:?}") };
        assert!((params.gain_db - SILENCE_DB * 0.5).abs() < 1e-3, "{params:?}");
        // A frame in which nothing moved sends nothing.
        system.update(&frame(&far, &[]), &mut rng);
        assert_eq!(system.backend().calls(), &[]);
    }

    #[test]
    fn an_emitter_gone_leaves_its_voices_ringing_where_it_stood() {
        let mut system = system_with(placed_cue());
        let mut rng = CueRng::new(1);
        let there = [actor_at(Vec3::new(0.0, 0.0, -11.0))];
        system.update(&frame(&there, &play(CueTarget::On(ACTOR))), &mut rng);
        system.backend_mut().take_calls();
        system.update(&frame(&[], &[]), &mut rng);
        assert_eq!(system.backend().calls(), &[], "no change of place or gain");
        assert_eq!(system.stats().voices, 1);
        // Played out, it is forgotten: a new cue on it has no emitter to follow.
        system.backend_mut().finish(VoiceSlot(0));
        system.update(&frame(&[], &[]), &mut rng);
        system.update(&frame(&[], &play(CueTarget::On(ACTOR))), &mut rng);
        assert_eq!(system.stats().voices, 0);
        assert_eq!(system.stats().dropped_unknown, 1);
    }

    fn emitter_set(start: u32) -> Vec<Emitter> {
        (start..start + MAX_EMITTERS as u32)
            .map(|id| Emitter { id: EmitterId(id), position: Vec3::X, hitstop: false })
            .collect()
    }

    #[test]
    fn replacement_emitters_can_play_on_their_first_frame_at_capacity() {
        for overlap in [0, 32] {
            let mut system = system_with(plain_cue());
            let mut rng = CueRng::new(1);
            system.update(&frame(&emitter_set(0), &[]), &mut rng);
            let replacement = emitter_set(MAX_EMITTERS as u32 - overlap);
            system
                .update(&frame(&replacement, &play(CueTarget::On(replacement.last().unwrap().id))), &mut rng);
            assert_eq!((system.stats().cues_played, system.stats().dropped_unknown), (1, 0));
            assert_eq!(system.emitters.len(), MAX_EMITTERS);
        }
    }

    #[test]
    fn ringing_emitters_keep_their_slots_until_the_voice_can_be_reaped() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        let old = emitter_set(0);
        system.update(&frame(&old, &play(CueTarget::On(old[0].id))), &mut rng);
        let replacement = emitter_set(MAX_EMITTERS as u32);
        system.update(&frame(&replacement, &[]), &mut rng);
        assert!(system.emitters.iter().any(|tracked| tracked.emitter.id == old[0].id));
        assert!(!system.emitters.iter().any(|tracked| tracked.emitter.id == replacement.last().unwrap().id));
        system.backend_mut().finish(VoiceSlot(0));
        system.backend_mut().set_state(DeviceState::Suspended);
        system.update(&frame(&replacement, &[]), &mut rng);
        assert!(system.emitters.iter().any(|tracked| tracked.emitter.id == old[0].id));
        system.backend_mut().set_state(DeviceState::Running);
        system.update(&frame(&replacement, &play(CueTarget::On(replacement.last().unwrap().id))), &mut rng);
        assert_eq!((system.stats().cues_played, system.stats().dropped_unknown), (2, 0));
        assert!(!system.emitters.iter().any(|tracked| tracked.emitter.id == old[0].id));
    }

    #[test]
    fn repeated_emitter_reports_keep_the_last_state_even_at_capacity() {
        let mut system = system_with(placed_cue());
        let mut rng = CueRng::new(1);
        let mut emitters = emitter_set(0);
        let last = Emitter { position: Vec3::NEG_X, hitstop: true, ..emitters[0] };
        emitters.push(last);
        system.update(&frame(&emitters, &play(CueTarget::On(last.id))), &mut rng);
        assert_eq!(started(&mut system).pan, -1.0);
        assert_eq!(
            system.emitters.iter().find(|tracked| tracked.emitter.id == last.id).unwrap().emitter,
            last
        );
    }

    #[test]
    fn excess_emitters_are_admitted_in_frame_order() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        let mut emitters = emitter_set(0);
        emitters.push(Emitter { id: EmitterId(1000), ..emitters[0] });
        system.update(&frame(&emitters, &play(CueTarget::On(EmitterId(1000)))), &mut rng);
        assert_eq!(system.stats().dropped_unknown, 1);
        assert_eq!(system.emitters.len(), MAX_EMITTERS);
    }

    #[test]
    fn a_time_scale_below_one_dips_the_world_but_not_the_interface() {
        let tuning = SnapshotTuning::default();
        let mut system = system_with(plain_cue());
        let ui = AssetId::from_text("22222222-2222-4333-8444-555555555555");
        system.add_cue(ui, Cue { bus: Bus::Ui, ..plain_cue() });
        let mut rng = CueRng::new(1);
        let both = [
            CueRequest { cue: CUE, target: CueTarget::Flat },
            CueRequest { cue: ui, target: CueTarget::Flat },
        ];
        system.update(&frame(&[], &both), &mut rng);
        let calls = system.backend_mut().take_calls();
        assert!(!calls.iter().any(|call| matches!(call, MockCall::SetBus { .. })), "{calls:?}");

        let slow = AudioFrame { world_time_scale: tuning.floor_scale, ..frame(&[], &[]) };
        for _ in 0..600 {
            system.update(&slow, &mut rng);
        }
        let calls = system.backend_mut().take_calls();
        let last_of_bus = |bus: Bus| {
            calls.iter().rev().find_map(|call| match *call {
                MockCall::SetBus { bus: seen, params } if seen == bus => Some(params),
                _ => None,
            })
        };
        let low_pass = last_of_bus(Bus::Sfx).expect("the world's bus dips").low_pass_hz;
        assert!((low_pass - tuning.floor_low_pass_hz).abs() < 1.0, "{low_pass}");
        assert_eq!(last_of_bus(Bus::Ambience).map(|params| params.low_pass_hz), Some(low_pass));
        assert_eq!(last_of_bus(Bus::Ui), None, "the interface stays as it is");

        let last_of_voice = |slot: VoiceSlot| {
            calls.iter().rev().find_map(|call| match *call {
                MockCall::SetVoice { slot: seen, params } if seen == slot => Some(params),
                _ => None,
            })
        };
        let world = last_of_voice(VoiceSlot(0)).expect("the world's voice bends");
        assert!((world.pitch - tuning.floor_pitch).abs() < 2e-3, "{world:?}");
        assert_eq!(last_of_voice(VoiceSlot(1)), None, "the interface's doesn't");

        // A voice started in the dip starts dipped.
        let cues = play(CueTarget::Flat);
        system.update(&AudioFrame { cues: &cues, ..slow }, &mut rng);
        assert!((started(&mut system).pitch - tuning.floor_pitch).abs() < 2e-3);

        // Back at speed, everything comes all the way back.
        for _ in 0..600 {
            system.update(&frame(&[], &[]), &mut rng);
        }
        let calls = system.backend_mut().take_calls();
        assert!(calls.contains(&MockCall::SetBus { bus: Bus::Sfx, params: BusParams::OPEN }), "{calls:?}");
    }

    #[test]
    fn hitstop_ducks_or_freezes_only_its_own_actors_voices() {
        let duck = AssetId::from_text("33333333-2222-4333-8444-555555555555");
        let freeze = AssetId::from_text("44444444-2222-4333-8444-555555555555");
        let mut system = system_with(plain_cue());
        system.add_cue(duck, Cue { hitstop: HitstopResponse::Duck(-12.0), ..plain_cue() });
        system.add_cue(freeze, Cue { hitstop: HitstopResponse::Freeze, ..plain_cue() });
        let mut rng = CueRng::new(1);
        let other = EmitterId(8);
        let at = Vec3::ZERO;
        let calm = [actor_at(at), Emitter { id: other, position: at, hitstop: false }];
        let cues = [
            CueRequest { cue: CUE, target: CueTarget::On(ACTOR) }, // slot 0: plays on
            CueRequest { cue: duck, target: CueTarget::On(ACTOR) }, // slot 1: ducks
            CueRequest { cue: freeze, target: CueTarget::On(ACTOR) }, // slot 2: freezes
            CueRequest { cue: freeze, target: CueTarget::On(other) }, // slot 3: not in hitstop
            CueRequest { cue: freeze, target: CueTarget::Flat },   // slot 4: on no actor
        ];
        system.update(&frame(&calm, &cues), &mut rng);
        assert_eq!(system.stats().voices, 5);
        system.backend_mut().take_calls();

        let stopped = [Emitter { hitstop: true, ..calm[0] }, calm[1]];
        system.update(&frame(&stopped, &[]), &mut rng);
        assert_eq!(
            system.backend_mut().take_calls(),
            [
                MockCall::SetVoice {
                    slot: VoiceSlot(1),
                    params: VoiceParams { gain_db: -12.0, pan: 0.0, pitch: 1.0 }
                },
                MockCall::FreezeVoice { slot: VoiceSlot(2), frozen: true },
            ]
        );
        // Nothing more while it lasts.
        system.update(&frame(&stopped, &[]), &mut rng);
        assert_eq!(system.backend().calls(), &[]);

        // A cue that fires during the hitstop starts as its response says.
        let fired = [
            CueRequest { cue: duck, target: CueTarget::On(ACTOR) },
            CueRequest { cue: freeze, target: CueTarget::On(ACTOR) },
        ];
        system.update(&frame(&stopped, &fired), &mut rng);
        let calls = system.backend_mut().take_calls();
        assert!(matches!(
            calls[0],
            MockCall::StartVoice { params: VoiceParams { gain_db: -12.0, .. }, frozen: false, .. }
        ));
        assert!(matches!(calls[1], MockCall::StartVoice { frozen: true, .. }));

        // The hitstop ends: every voice comes back as it was.
        system.update(&frame(&calm, &[]), &mut rng);
        let calls = system.backend_mut().take_calls();
        assert!(calls.contains(&MockCall::FreezeVoice { slot: VoiceSlot(2), frozen: false }), "{calls:?}");
        assert!(calls.contains(&MockCall::SetVoice {
            slot: VoiceSlot(1),
            params: VoiceParams { gain_db: 0.0, pan: 0.0, pitch: 1.0 }
        }));
    }

    #[test]
    fn a_frozen_voice_thaws_when_its_actor_is_gone() {
        let mut system = system_with(Cue { hitstop: HitstopResponse::Freeze, ..plain_cue() });
        let mut rng = CueRng::new(1);
        let stopped = [Emitter { hitstop: true, ..actor_at(Vec3::ZERO) }];
        system.update(&frame(&stopped, &play(CueTarget::On(ACTOR))), &mut rng);
        system.backend_mut().take_calls();
        system.update(&frame(&[], &[]), &mut rng);
        assert_eq!(
            system.backend_mut().take_calls(),
            [MockCall::FreezeVoice { slot: VoiceSlot(0), frozen: false }]
        );
    }

    /// A fight starts over, and what the last one left ringing goes. Nothing brings it back: a frozen
    /// voice told to thaw would resume, and cancel its stop.
    #[test]
    fn stopping_voices_ends_them_for_good_and_new_cues_play() {
        let freeze = AssetId::from_text("44444444-2222-4333-8444-555555555555");
        let mut system = system_with(placed_cue());
        system.add_cue(freeze, Cue { hitstop: HitstopResponse::Freeze, ..placed_cue() });
        let mut rng = CueRng::new(1);
        let stopped = [Emitter { hitstop: true, ..actor_at(Vec3::new(0.0, 0.0, -1.0)) }];
        let frozen_too = [
            CueRequest { cue: CUE, target: CueTarget::On(ACTOR) },
            CueRequest { cue: freeze, target: CueTarget::On(ACTOR) },
        ];
        system.update(&frame(&stopped, &frozen_too), &mut rng);
        assert_eq!(system.stats().voices, 2);
        system.backend_mut().take_calls();

        system.stop_voices();
        assert_eq!(
            system.backend_mut().take_calls(),
            [MockCall::StopVoice { slot: VoiceSlot(0) }, MockCall::StopVoice { slot: VoiceSlot(1) }]
        );
        system.stop_voices();
        assert_eq!(system.backend().calls(), &[], "told once");

        // The actor thaws and moves far away, and a cue of the new fight plays in the same frame.
        let thawed = [actor_at(Vec3::new(0.0, 0.0, -11.0))];
        system.update(&frame(&thawed, &play(CueTarget::Flat)), &mut rng);
        let calls = system.backend_mut().take_calls();
        assert!(matches!(calls[..], [MockCall::StartVoice { slot: VoiceSlot(2), .. }]), "{calls:?}");
        assert_eq!(system.stats().voices, 3, "they hold their slots until they've ended");

        system.backend_mut().finish(VoiceSlot(0));
        system.backend_mut().finish(VoiceSlot(1));
        system.update(&frame(&thawed, &[]), &mut rng);
        assert_eq!(system.stats().voices, 1);
        assert_eq!(system.stats().cues_played, 3);
    }

    #[test]
    fn slots_come_back_when_voices_finish_and_excess_cues_are_counted() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        for _ in 0..MAX_VOICES {
            system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        }
        assert_eq!(system.stats().voices as usize, MAX_VOICES);
        system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        assert_eq!((system.stats().voices as usize, system.stats().dropped_voice_limit), (MAX_VOICES, 1));
        system.backend_mut().finish(VoiceSlot(3));
        system.backend_mut().take_calls();
        system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        let calls = system.backend_mut().take_calls();
        assert!(matches!(calls[..], [MockCall::StartVoice { slot: VoiceSlot(3), .. }]), "{calls:?}");
        assert_eq!(system.stats().dropped_voice_limit, 1);
    }

    #[test]
    fn what_nobody_added_is_counted_not_fatal() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        let missing = AssetId::from_text("99999999-2222-4333-8444-555555555555");
        system.update(&frame(&[], &[CueRequest { cue: missing, target: CueTarget::Flat }]), &mut rng);
        assert_eq!(system.stats().dropped_unknown, 1);
        // A cue naming a sound that never loaded, and an emitter nobody reported.
        system.add_cue(missing, Cue { sounds: vec![missing], ..plain_cue() });
        system.update(&frame(&[], &[CueRequest { cue: missing, target: CueTarget::Flat }]), &mut rng);
        assert_eq!(system.stats().dropped_unknown, 2);
        system.update(&frame(&[], &play(CueTarget::On(EmitterId(99)))), &mut rng);
        assert_eq!(system.stats().dropped_unknown, 3);
        assert_eq!(system.stats().voices, 0);
        // Failing to start kept the slot.
        for _ in 0..MAX_VOICES {
            system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        }
        assert_eq!(system.stats().voices as usize, MAX_VOICES);
    }

    #[test]
    fn background_and_interruptions_suspend_once_and_resume_when_both_end() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        system.set_background(true);
        assert_eq!(system.backend_mut().take_calls(), [MockCall::Suspend]);
        system.set_background(true);
        system.set_interrupted(true);
        assert_eq!(system.backend().calls(), &[], "already suspended");
        // A call ends while the app is still in the background.
        system.set_interrupted(false);
        assert_eq!(system.backend().calls(), &[]);
        system.set_background(false);
        assert_eq!(system.backend_mut().take_calls(), [MockCall::Resume]);
        system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        assert_eq!(system.stats().cues_played, 1);
    }

    /// `CpalOutput`'s worker stops the stream when it gets to it, so its status reads `Running` for a
    /// while after it was asked to stop. What may play goes by what was asked.
    #[test]
    fn a_suspension_counts_from_the_asking_not_from_the_output_catching_up() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        let cues = play(CueTarget::Flat);
        system.update(&frame(&[], &cues), &mut rng);
        assert_eq!(system.stats().cues_played, 1);

        // An interruption, in the foreground, and an output that hasn't got there.
        system.set_interrupted(true);
        system.backend_mut().set_state(DeviceState::Running);
        system.backend_mut().take_calls();
        let rng_before = rng.clone();
        for _ in 0..3 {
            system.update(&frame(&[], &cues), &mut rng);
        }
        assert_eq!(system.stats().cues_played, 1, "no new voice");
        assert_eq!(system.stats().dropped_no_output, 3);
        assert_eq!(system.backend().calls(), &[], "and what's there is left alone");
        assert_eq!(rng, rng_before, "a dropped cue takes no numbers");
        assert_eq!(system.stats().device.state, DeviceState::Running, "the output's own word is kept");

        // It ends before the output caught up: the output is asked to run, and that's enough.
        system.set_interrupted(false);
        system.backend_mut().set_state(DeviceState::Running);
        system.update(&frame(&[], &cues), &mut rng);
        assert_eq!(system.stats().cues_played, 2);

        // Going back, the output's word counts too: it hasn't started yet.
        system.set_interrupted(true);
        system.set_interrupted(false);
        system.backend_mut().set_state(DeviceState::Suspended);
        system.update(&frame(&[], &cues), &mut rng);
        assert_eq!(system.stats().dropped_no_output, 4);
        system.backend_mut().set_state(DeviceState::Running);
        system.update(&frame(&[], &cues), &mut rng);
        assert_eq!(system.stats().cues_played, 3);

        // Both at once: one ending leaves the other in force.
        system.set_background(true);
        system.set_interrupted(true);
        system.set_background(false);
        system.backend_mut().set_state(DeviceState::Running);
        system.update(&frame(&[], &cues), &mut rng);
        assert_eq!(system.stats().dropped_no_output, 5);
        system.set_background(true);
        system.set_interrupted(false);
        system.backend_mut().set_state(DeviceState::Running);
        system.update(&frame(&[], &cues), &mut rng);
        assert_eq!(system.stats().dropped_no_output, 6);
        system.set_background(false);
        system.update(&frame(&[], &cues), &mut rng);
        assert_eq!(system.stats().cues_played, 4);
    }

    #[test]
    fn cues_are_dropped_and_voices_left_alone_while_the_output_is_not_playing() {
        for state in [DeviceState::Suspended, DeviceState::NoDevice] {
            let mut system = system_with(plain_cue());
            let mut rng = CueRng::new(1);
            system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
            system.backend_mut().take_calls();
            system.backend_mut().set_state(state);
            // Even a finished-looking voice stays until the mixer runs again.
            system.backend_mut().finish(VoiceSlot(0));
            let mut rng_after = rng.clone();
            let cues = play(CueTarget::Flat);
            let slow = AudioFrame { world_time_scale: 0.2, ..frame(&[], &cues) };
            system.update(&slow, &mut rng_after);
            assert_eq!(system.stats().dropped_no_output, 1, "{state}");
            assert_eq!(system.stats().voices, 1, "{state}");
            assert_eq!(rng_after, rng, "a dropped cue takes no numbers, so later picks don't move");
            assert!(system.backend().calls().iter().all(|call| matches!(call, MockCall::SetBus { .. })));
            // Back up: it plays again, and the voice is reaped.
            system.backend_mut().set_state(DeviceState::Running);
            system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng_after);
            assert_eq!((system.stats().voices, system.stats().cues_played), (1, 2), "{state}");
        }
    }

    #[test]
    fn the_update_is_timed_and_its_peak_kept_until_taken() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        let stats = system.stats();
        assert!(stats.update > Duration::ZERO);
        assert!(stats.update_peak >= stats.update);
        assert_eq!(system.take_peak(), stats.update_peak);
        assert_eq!(system.stats().update_peak, Duration::ZERO);
    }
}
