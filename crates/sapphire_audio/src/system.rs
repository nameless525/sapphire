//! [`AudioSystem`]: cues, lines and music in, voices out, once a frame.

use std::collections::HashMap;
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Attenuation, Bus, Cue, HitstopResponse, LimitRule, SILENCE_DB};
use sapphire_assets::mix::{DuckFollow, Mix, MixError, OutputProfile, Reverb, VoiceGroup};
use sapphire_assets::mouth::MouthCurve;
use sapphire_assets::score::Score;
use sapphire_assets::sound::DecodedSound;
use sapphire_assets::voice::VoiceLines;
use sapphire_core::Name;
use sapphire_math::Vec3;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::backend::{
    AudioBackend, BusId, BusParams, DeviceState, DeviceStatus, STOP_FADE, StartError, VIRTUAL_FADE,
    VoiceParams, VoiceSlot, VoiceStart,
};
use crate::events::{CueParameters, CuePlan, ParameterValue};
use crate::master::DynamicRange;
use crate::mix::MixState;
use crate::music::{Music, MusicRequest};
use crate::rng::CueRandom;
use crate::rooms::{Rooms, Space};
use crate::voice::{LineKey, VoiceRoute, VoiceRouteError};

/// The most voices mixed at once (M6 U3, Desktop-High's real voices): [`VoiceCaps::real`] at most. Past it,
/// what ranks lowest goes virtual or ends.
pub const MAX_VOICES: usize = 64;

/// The most virtual voices kept at once (M6 U3, the desktop tiers'): [`VoiceCaps::virtual_voices`] at most.
pub const MAX_VIRTUAL_VOICES: usize = 256;

/// A one-shot shorter than this many seconds ends rather than going virtual (M6 U3): by the time it could come
/// back, it would mostly be over.
pub const SHORTEST_VIRTUAL_SECONDS: f64 = 2.0;

/// The mixer's slots: twice [`MAX_VOICES`], so as many voices as it mixes can fade out while as many start.
pub(crate) const MAX_SLOTS: usize = 2 * MAX_VOICES;

/// Voices live at once, real and virtual, and so the room the system keeps for them.
const MAX_LIVE: usize = MAX_VOICES + MAX_VIRTUAL_VOICES;

/// How many decibels a real voice counts above its gain against a virtual one of the same priority (M6 U3), so
/// two voices about as loud don't swap places frame after frame.
const KEEP_REAL_DB: f32 = 3.0;

/// How many voices the mixer mixes and keeps (M6 U3): the quality tier's, spec §5.15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceCaps {
    /// Voices mixed at once, 1 to [`MAX_VOICES`].
    pub real: u16,
    /// Voices kept at once without being mixed, waiting to come back, 0 to [`MAX_VIRTUAL_VOICES`].
    pub virtual_voices: u16,
}

impl VoiceCaps {
    /// The most there are room for: Desktop-High's.
    pub const MAX: Self = Self { real: MAX_VOICES as u16, virtual_voices: MAX_VIRTUAL_VOICES as u16 };

    /// The caps brought within what there's room for.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            real: self.real.clamp(1, MAX_VOICES as u16),
            virtual_voices: self.virtual_voices.min(MAX_VIRTUAL_VOICES as u16),
        }
    }
}

impl Default for VoiceCaps {
    fn default() -> Self {
        Self::MAX
    }
}

/// Emitters tracked at once: the actors a frame reports, and those gone with voices still ringing.
const MAX_EMITTERS: usize = 64;

/// A voice moves only when its gain changes by this many decibels, ...
const GAIN_STEP_DB: f32 = 0.05;
/// ... its pan by this, ...
const PAN_STEP: f32 = 0.005;
/// ... or its pitch by this: smaller steps are all smoothed away, and each costs the audio thread a
/// command.
const PITCH_STEP: f32 = 0.001;

/// How long a bus's gain takes to reach what a frame asks: enough to hide the steps between frames. How fast
/// a duck or a snapshot moves is the mix's own.
pub(crate) const BUS_FADE: Duration = Duration::from_millis(30);

/// The name a game asks for its pause snapshot by: the shipped mix's, which holds the world where it is.
pub const PAUSE_SNAPSHOT: Name = Name::new("pause");

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
    /// What its tree's switches and blends read (M6 U4a): a footstep's [`SURFACE`](crate::SURFACE).
    pub parameters: CueParameters,
}

impl CueRequest {
    /// `cue` at `target`, with no parameters.
    #[must_use]
    pub const fn new(cue: AssetId, target: CueTarget) -> Self {
        Self { cue, target, parameters: CueParameters::NONE }
    }

    /// The same, with `name` set to `value` if there's room ([`MAX_CUE_PARAMETERS`](crate::MAX_CUE_PARAMETERS)).
    #[must_use]
    pub fn with(self, name: Name, value: ParameterValue) -> Self {
        Self { parameters: self.parameters.with(name, value), ..self }
    }
}

/// One voice line to speak, in the chosen voice language: a cue that language's
/// [`VoiceLines`] name for its key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineRequest {
    pub line: LineKey,
    pub target: CueTarget,
}

/// The line a conversation says now (M5 C6), asked for every frame it's shown: one at a time, spoken in the
/// chosen voice language as a [`LineRequest`] is, and cut when another is asked for or none is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeechRequest {
    pub line: LineKey,
    /// Which showing of a line this is, however the caller counts: a new number starts its line from the
    /// beginning, even the same line again, and the same number goes on with the one speaking.
    pub serial: u64,
    pub target: CueTarget,
    /// Held where it is, as a pause holds a conversation: the voice waits, and its playhead with it.
    pub held: bool,
}

/// What a conversation's line is saying (M5 C6): the sound its voice plays, and how far into it the voice
/// is, by the system's own count of the real seconds it has played, never asked of the audio thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speaking {
    /// The [`SpeechRequest::serial`] it was asked by.
    pub serial: u64,
    pub sound: AssetId,
    /// Seconds into the sound.
    pub seconds: f32,
}

/// Everything a frame tells the audio system.
#[derive(Clone, Copy, Debug)]
pub struct AudioFrame<'a> {
    /// Real seconds since the last frame.
    pub dt: f32,
    pub listener: Listener,
    /// The world clock's speed: 1 is normal, and less drives the mix's snapshots that follow it
    /// ([`SnapshotDriver::WorldTimeScale`](sapphire_assets::mix::SnapshotDriver::WorldTimeScale)).
    pub world_time_scale: f32,
    /// The snapshots the game asks for by name this frame (M6 U2): [`PAUSE_SNAPSHOT`] while it's paused. A name
    /// the mix hasn't does nothing.
    pub snapshots: &'a [Name],
    /// The emitters that exist this frame.
    pub emitters: &'a [Emitter],
    /// The cues to start this frame.
    pub cues: &'a [CueRequest],
    /// The voice lines to start this frame, after the cues.
    pub lines: &'a [LineRequest],
    /// The line a conversation says, after the lines: none, and what one was saying is cut.
    pub speech: Option<SpeechRequest>,
    /// The score and phase to play, or none: what played fades out.
    pub music: Option<MusicRequest<'a>>,
    /// Where the listener and the emitters stand among the zone's rooms (M6 U5): the listener's chooses the reverb,
    /// and a voice reaches it from the same space. [`Rooms::NONE`] plays none.
    pub rooms: Rooms<'a>,
}

/// The system's counters (rule R9). [`Self::update`] is what spec §3.2's "Audio update" row budgets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioStats {
    /// The last [`AudioSystem::update`] took this long, on the calling thread.
    pub update: Duration,
    /// The longest update since [`AudioSystem::take_peak`].
    pub update_peak: Duration,
    /// Voices the mixer holds now: those it mixes, and those fading out that keep their slot until they've ended.
    pub voices: u32,
    /// Voices kept without being mixed now (M6 U3), each waiting to come back where it would be.
    pub virtual_voices: u32,
    /// Starts that played a voice, real or virtual, all told: one a start, however many sounds its tree picked (M6
    /// U4a), counted by the first of them the caps let in.
    pub cues_played: u64,
    /// Cues dropped because the output isn't playing (suspended, no device), one a request.
    pub dropped_no_output: u64,
    /// Voices that didn't play (M6 U3), one a sound picked: a start refused by its own limit or its group's or the
    /// mixer's room counts each of its sounds, and so does one ranked below everything the [`VoiceCaps`] keep,
    /// stolen before it began, or with the mixer full.
    pub dropped_voice_limit: u64,
    /// Cues dropped for naming a cue, a sound or an emitter nobody added, or asking for a loop off an emitter, one
    /// a request (a sound nobody added, one a voice).
    pub dropped_unknown: u64,
    /// Voices ended before their time to make room (M6 U3), one a voice: by a limit's rule, or outranked with no
    /// virtual room.
    pub voices_stolen: u64,
    /// Voices that went virtual, all told (M6 U3): those outranked as they played, and those asked for that had
    /// no real room yet.
    pub voices_virtualized: u64,
    /// Virtual voices that came back, all told (M6 U3).
    pub voices_revived: u64,
    /// Lines asked for that the voice language has, all told: each plays as a cue does.
    pub lines_asked: u64,
    /// Lines the voice language lacks, or asked for with no language chosen.
    pub lines_missing: u64,
    /// Changes to a phase the score has, all told.
    pub music_phase_changes: u64,
    /// Changes to a phase the score lacks, which leave the stems as they were.
    pub music_unknown_phases: u64,
    /// Changes of the reverb the listener's room plays (M6 U5), to one or to none, all told.
    pub room_changes: u64,
    /// Rooms entered naming a reverb the mix hasn't, which play none (M6 U5), all told.
    pub reverbs_unknown: u64,
    /// The output's own counters.
    pub device: DeviceStatus,
    /// The bytes of decoded sound the mixer holds (M6 U1): [`AudioBackend::decoded_bytes`], kept with each
    /// [`AudioSystem::add_sound`].
    pub decoded_bytes: u64,
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

/// A voice's serial, in the order they were asked for: its age, and how a conversation finds its line's voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct VoiceId(u64);

/// Where a voice stands with the mixer (M6 U3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Asked for this frame, and not yet placed.
    Asked,
    /// Mixed, in this slot.
    Real(VoiceSlot),
    /// Kept without being mixed, its playhead counted on.
    Virtual,
    /// Ended or refused this frame, and about to be forgotten.
    Gone,
}

/// A live voice: real, virtual or just asked for.
#[derive(Clone, Copy, Debug)]
struct Voice {
    id: VoiceId,
    mode: Mode,
    /// The start it's one of (M6 U4a): its first voice's serial. A start's voices are pushed together and only
    /// ever removed, so they sit side by side; a start counts once toward a limit while any of them lives, by its
    /// first live one ([`counts_start`]), and one made room for goes with all of them.
    start: VoiceId,
    /// The cue it plays, which its own limit counts by.
    cue: AssetId,
    sound: AssetId,
    place: Place,
    /// The cue's priority: higher keeps a real voice first.
    priority: u8,
    /// Its voice group, by place in the mix's groups.
    group: Option<u16>,
    /// Plays round and round until its emitter goes.
    looped: bool,
    /// Its sound's length in seconds.
    length: f64,
    /// The conversation's line: ranked first, never virtual, and outside every limit.
    protected: bool,
    /// What it sounds like now, real or not.
    params: VoiceParams,
    /// Its category, whose bus it plays on.
    bus: Bus,
    /// How far into its sound it has played, by the system's own count of the real seconds it has played,
    /// never asked of the audio thread: what a ducking rule reads its mouth curve at (M6 U2), and a
    /// conversation's playhead (M5 C6).
    seconds: f64,
    attenuation: Option<Attenuation>,
    hitstop: HitstopResponse,
    /// The gain and pitch the cue picked.
    gain_db: f32,
    pitch: f32,
    /// What the backend was last told, while it's real.
    sent: VoiceParams,
    /// Waiting where it is: what the backend was told, while it's real.
    frozen: bool,
    /// Held by its speech's caller ([`SpeechRequest::held`]), whatever hitstop says.
    held: bool,
}

impl Voice {
    fn live(&self) -> bool {
        self.mode != Mode::Gone
    }

    /// Whether it can wait unheard and come back: a loop, or a sound long enough to be worth it.
    fn can_wait(&self) -> bool {
        !self.protected && (self.looped || self.length >= SHORTEST_VIRTUAL_SECONDS)
    }

    /// Whether it's heard at all where it is.
    fn audible(&self) -> bool {
        self.params.gain_db > SILENCE_DB
    }

    fn playing(&self) -> bool {
        matches!(self.mode, Mode::Real(_) | Mode::Virtual)
    }
}

/// The voices before `index` that belong to its start, nearest first: at most a start's sounds less one.
fn start_before(voices: &[Voice], index: usize) -> impl Iterator<Item = &Voice> {
    let start = voices[index].start;
    voices[..index].iter().rev().take_while(move |voice| voice.start == start)
}

/// Whether the voice at `index` stands for its start (M6 U4a): it's live and the first of its start that is, so a
/// start counts toward its limits for as long as any of its voices lives, whichever of them ends first.
fn counts_start(voices: &[Voice], index: usize) -> bool {
    voices[index].live() && start_before(voices, index).all(|voice| !voice.live())
}

/// The start of the voice at `index` as one, judged against another (M6 U4a): its live voice with the best claim
/// ([`standing`]), as loud as the loudest of them, so a blend whose first voice is its quiet side counts as loud
/// as it's heard.
fn judged(voices: &[Voice], index: usize) -> Voice {
    let start = voices[index].start;
    let first = index - start_before(voices, index).count();
    let sides = voices[first..].iter().take_while(|voice| voice.start == start).filter(|voice| voice.live());
    let mut best = sides.clone().min_by(|a, b| standing(a, b)).copied().unwrap_or(voices[index]);
    best.params.gain_db = sides.map(|voice| voice.params.gain_db).fold(best.params.gain_db, f32::max);
    best
}

/// Whether `a` has a better claim than `b` on being mixed: the conversation's line first, then what's heard,
/// then the higher priority, then the louder (a real voice by [`KEEP_REAL_DB`] more), then the older.
fn standing(a: &Voice, b: &Voice) -> std::cmp::Ordering {
    let loudness = |voice: &Voice| {
        voice.params.gain_db + if matches!(voice.mode, Mode::Real(_)) { KEEP_REAL_DB } else { 0.0 }
    };
    b.protected
        .cmp(&a.protected)
        .then(b.audible().cmp(&a.audible()))
        .then(b.priority.cmp(&a.priority))
        .then(loudness(b).total_cmp(&loudness(a)))
        .then(a.id.cmp(&b.id))
}

/// The conversation's line, as asked and as spoken.
#[derive(Clone, Copy, Debug)]
struct Speech {
    serial: u64,
    /// The voice saying it and its sound, until it ends or is cut: none if it never started.
    voice: Option<(VoiceId, AssetId)>,
}

/// What a bus was last sent.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SentBus {
    gain_db: f32,
    low_pass_hz: f32,
    paused: bool,
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
    /// Each cue, with its tree laid out for starts and what that remembers (M6 U4a).
    cues: HashMap<AssetId, (Cue, CuePlan)>,
    /// Each sound's length in seconds, kept with [`Self::add_sound`].
    lengths: HashMap<AssetId, f64>,
    /// Real, virtual and just asked for, oldest first.
    voices: Vec<Voice>,
    /// The voices' places in [`Self::voices`], best claim first: room kept for the ranking each frame.
    order: Vec<u16>,
    /// Slots of voices told to end, kept until they have.
    ending: Vec<VoiceSlot>,
    /// Slots no voice holds, as a stack.
    free_slots: Vec<VoiceSlot>,
    next_id: u64,
    caps: VoiceCaps,
    /// The mix's voice groups, by place (M6 U3).
    groups: Vec<VoiceGroup>,
    emitters: Vec<Tracked>,
    /// The mix's snapshots and ducking rules, as they stand (M6 U2).
    mix: MixState,
    /// Each bus as last sent, by place.
    sent: Vec<SentBus>,
    /// Each ducking rule's level this frame, 0 to 1.
    levels: Vec<f32>,
    /// The mouth curves of the sounds that have one, which ducking rules follow (M6 U2).
    mouths: HashMap<AssetId, MouthCurve>,
    music: Music,
    voice: VoiceRoute,
    /// The conversation's line (M5 C6).
    speech: Option<Speech>,
    background: bool,
    interrupted: bool,
    /// What the player's volume takes off every voice.
    volume_db: f32,
    /// What the player's volume for each bus takes off its voices on top (M4 A2), by [`Bus::index`].
    bus_volume_db: [f32; Bus::ALL.len()],
    /// The mix's reverbs, which rooms name (M6 U5).
    reverbs: Vec<Reverb>,
    /// Each bus's level to the room's reverb, by place: its category's.
    room_db: Vec<Option<f32>>,
    /// The listener's room, as last placed: its reverb, by place in [`Self::reverbs`], none if it plays none, and
    /// its fade.
    room: Option<(usize, f32)>,
    /// Where the listener stood this frame, among the rooms: in which, and in what space.
    listener_room: Option<usize>,
    listener_space: Space,
    /// How many reverbs the room's plays on.
    reverb_slots: u8,
    /// The output profile asked for (M6 U10), kept through a change of mix.
    profile: OutputProfile,
    stats: AudioStats,
    /// A dropped cue's reason has been logged once.
    warned_unknown: bool,
    warned_no_output: bool,
}

impl<B: AudioBackend> AudioSystem<B> {
    /// A system with nothing loaded, on `backend`, under [`Mix::plain`] until [`Self::set_mix`].
    pub fn new(backend: B) -> Self {
        let plain = Mix::plain();
        Self {
            backend,
            cues: HashMap::new(),
            lengths: HashMap::new(),
            // The conversation's line is outside the room the rest share.
            voices: Vec::with_capacity(MAX_LIVE + 1),
            order: Vec::with_capacity(MAX_LIVE + 1),
            ending: Vec::with_capacity(MAX_SLOTS),
            free_slots: (0..MAX_SLOTS as u32).rev().map(VoiceSlot).collect(),
            next_id: 0,
            caps: VoiceCaps::MAX,
            groups: Vec::new(),
            emitters: Vec::with_capacity(MAX_EMITTERS),
            sent: sent_of(&plain),
            levels: Vec::new(),
            mix: MixState::new(&plain),
            mouths: HashMap::new(),
            music: Music::default(),
            voice: VoiceRoute::new(),
            speech: None,
            background: false,
            interrupted: false,
            volume_db: 0.0,
            bus_volume_db: [0.0; Bus::ALL.len()],
            reverbs: Vec::new(),
            room_db: vec![None; plain.buses.len()],
            room: None,
            listener_room: None,
            listener_space: Space::Outdoors,
            reverb_slots: 2,
            profile: OutputProfile::Home,
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

    /// Plays everything through `mix` from now on (M6 U2): its buses, snapshots, ducking rules and profiles,
    /// under the output profile asked for last. Load-time work, before the music and voices it's for: what plays
    /// ends, and music starts again at its next request. A mix that doesn't check changes nothing.
    pub fn set_mix(&mut self, mix: &Mix) -> Result<(), MixError> {
        mix.check()?;
        self.stop_voices();
        self.backend.set_mix(mix);
        if self.profile != OutputProfile::Home {
            self.backend.set_output_profile(self.profile);
        }
        self.music.forget_playing();
        self.mix = MixState::new(mix);
        self.sent = sent_of(mix);
        self.levels = vec![0.0; mix.ducking.len()];
        self.groups.clone_from(&mix.groups);
        self.reverbs.clone_from(&mix.reverbs);
        self.room_db = (0..mix.buses.len()).map(|bus| mix.room_db(bus)).collect();
        // The new mix's room plays nothing until the next update places the listener.
        (self.room, self.listener_room) = (None, None);
        Ok(())
    }

    /// How many reverbs the room's plays on (M6 U5), 1 or 2: the quality tier's. Two crossfade on a change of room;
    /// one is retuned, at half the cost.
    pub fn set_reverb_slots(&mut self, slots: u8) {
        let slots = slots.clamp(1, 2);
        if slots != self.reverb_slots {
            self.reverb_slots = slots;
            self.backend.set_reverb_slots(slots);
        }
    }

    /// How many voices are mixed and kept from the next update on (M6 U3), brought within [`VoiceCaps::MAX`]:
    /// the quality tier's. Lowering them sends what ranks lowest virtual, or ends it.
    pub fn set_voice_caps(&mut self, caps: VoiceCaps) {
        self.caps = caps.clamped();
    }

    pub fn voice_caps(&self) -> VoiceCaps {
        self.caps
    }

    /// Plays the mix through `profile`'s volume and effects (M6 U2), or `Home`'s if the mix has none for it, and
    /// its dynamic range relative to `profile`'s loudness (M6 U10). The profile stays through [`Self::set_mix`].
    pub fn set_output_profile(&mut self, profile: OutputProfile) {
        if profile != self.profile {
            self.profile = profile;
            self.backend.set_output_profile(profile);
        }
    }

    /// The output profile asked for last: `Home` until another is.
    #[must_use]
    pub const fn output_profile(&self) -> OutputProfile {
        self.profile
    }

    /// Plays the whole mix through `range` (M6 U10), the player's choice, relative to the output profile's
    /// loudness. A new system plays [`DynamicRange::Wide`], the mix as it is.
    pub fn set_dynamic_range(&mut self, range: DynamicRange) {
        self.backend.set_dynamic_range(range);
    }

    /// How late the mix leaves for the device past when it was asked (M6 U10): the master limiter's look-ahead,
    /// which a device's own latency adds to. What a face's mouth waits for, beside that (U15).
    #[must_use]
    pub fn mix_latency(&self) -> Duration {
        self.backend.mix_latency()
    }

    /// Gives a sound's mouth curve, for the ducking rules that follow a voice line's envelope (M6 U2). Load-time
    /// work, beside [`Self::add_sound`].
    pub fn add_mouth(&mut self, sound: AssetId, mouth: MouthCurve) {
        self.mouths.insert(sound, mouth);
    }

    /// The mouth curve given for `sound`, if any: what a face reads at [`Speaking::seconds`].
    #[must_use]
    pub fn mouth(&self, sound: AssetId) -> Option<&MouthCurve> {
        self.mouths.get(&sound)
    }

    /// Makes a cue playable, its random nodes and sequences remembering nothing yet. Load-time work, as is
    /// [`Self::add_sound`] for each of its sounds.
    pub fn add_cue(&mut self, id: AssetId, cue: Cue) {
        let plan = CuePlan::new(&cue.play);
        self.cues.insert(id, (cue, plan));
    }

    /// Makes a decoded sound playable.
    pub fn add_sound(&mut self, id: AssetId, sound: &DecodedSound) {
        let frames = sound.samples.len() / sound.channels.max(1) as usize;
        self.lengths.insert(id, frames as f64 / f64::from(sound.sample_rate.max(1)));
        self.backend.add_sound(id, sound);
        self.stats.decoded_bytes = self.backend.decoded_bytes();
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

    /// The player's volume, a share of full: the mix's last gain (M6 U10), after the dynamic range and the
    /// limiter, so the slider moves what's heard by exactly its decibels whatever the range. The kinds' volumes
    /// ([`Self::set_bus_volume`]) stay on their buses, before the range, which evens out a rebalanced mix as a
    /// night mode should.
    pub fn set_volume(&mut self, volume: f32) {
        let volume_db = decibels_of(volume);
        if volume_db != self.volume_db {
            self.volume_db = volume_db;
            self.backend.set_output_volume(volume_db);
        }
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

    /// The player's volume for one bus (M4 A2), a share of full under [`Self::set_volume`]'s: its voices, and for
    /// the music its stems, follow at the next update.
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
        for index in 0..self.voices.len() {
            self.end_voice(index, STOP_FADE);
        }
        self.voices.clear();
        // A conversation's line is cut too, and not started again while it's asked for by the same serial.
        if let Some(speech) = &mut self.speech {
            speech.voice = None;
        }
    }

    /// Ends the voice at `index` in [`Self::voices`] for good, fading over `fade` if it's real, and marks it
    /// gone: its slot waits in [`Self::ending`] until the mixer has let it go.
    fn end_voice(&mut self, index: usize, fade: Duration) {
        let voice = &mut self.voices[index];
        if let Mode::Real(slot) = voice.mode {
            self.backend.stop_voice(slot, fade);
            self.ending.push(slot);
        }
        voice.mode = Mode::Gone;
    }

    /// What the conversation's line is saying, as of the last [`Self::update`]: none once it has ended or
    /// been cut, or if it never started (no such line in the voice language, no output).
    #[must_use]
    pub fn speaking(&self) -> Option<Speaking> {
        let speech = self.speech?;
        let (id, sound) = speech.voice?;
        let voice = self.voices.iter().find(|voice| voice.id == id && voice.live())?;
        Some(Speaking { serial: speech.serial, sound, seconds: voice.seconds as f32 })
    }

    /// The counters, as of the last [`Self::update`].
    pub fn stats(&self) -> AudioStats {
        self.stats
    }

    /// Starts a new window for [`AudioStats::update_peak`].
    pub fn take_peak(&mut self) -> Duration {
        std::mem::take(&mut self.stats.update_peak)
    }

    /// One frame: takes the emitters, moves the mix's snapshots and ducks, starts the frame's cues and lines,
    /// follows the music's phase, and brings every voice and bus up to date. Picks come from `random`,
    /// in request order, lines after cues: one number for the sounds, one for the volume, one for the
    /// pitch, whatever the cue's tree and ranges.
    pub fn update(&mut self, frame: &AudioFrame<'_>, random: &mut impl CueRandom) {
        profiling::scope!("AudioSystem::update");
        let timer = Stopwatch::start();
        self.stats.device = self.backend.status();
        self.stats.decoded_bytes = self.backend.decoded_bytes();
        // What was asked comes first: `CpalOutput` stops on a thread of its own and reads `Running` for
        // a while after, when a voice started now would sound, or wait to sound on the way back. The
        // stats keep the output's own word.
        let suspended = self.background || self.interrupted;
        let state = if suspended { DeviceState::Suspended } else { self.stats.device.state };

        if state == DeviceState::Running {
            self.reap_voices();
        }
        self.advance_voices(frame.dt, state);
        self.track_emitters(frame.emitters);
        self.end_loops_left_behind();
        self.advance_speech();
        self.step_mix(frame);
        self.follow_room(&frame.rooms, &frame.listener);
        for request in frame.cues {
            if state.plays() {
                self.start_cue(request, &frame.listener, random, None);
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
                self.start_cue(&CueRequest::new(cue, request.target), &frame.listener, random, None);
            } else {
                self.stats.dropped_no_output += 1;
            }
        }
        self.follow_speech(frame.speech, state, &frame.listener, random);
        self.music.update(frame.music, state.plays(), &mut self.backend);
        self.stats.music_phase_changes = self.music.counts.phase_changes;
        self.stats.music_unknown_phases = self.music.counts.unknown_phases;
        if state.plays() {
            self.place_voices(&frame.listener, &frame.rooms);
        }
        self.follow_bus_gains(frame.dt);

        let real = self.voices.iter().filter(|voice| matches!(voice.mode, Mode::Real(_))).count();
        self.stats.voices = (real + self.ending.len()) as u32;
        self.stats.virtual_voices = (self.voices.len() - real) as u32;
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

    /// Frees the slots of the voices the mixer has let go: those played out, and those told to end.
    fn reap_voices(&mut self) {
        let (backend, free_slots) = (&mut self.backend, &mut self.free_slots);
        self.voices.retain(|voice| {
            let Mode::Real(slot) = voice.mode else { return true };
            let finished = backend.voice_finished(slot);
            if finished {
                free_slots.push(slot);
            }
            !finished
        });
        self.ending.retain(|&slot| {
            let finished = backend.voice_finished(slot);
            if finished {
                free_slots.push(slot);
            }
            !finished
        });
    }

    /// Moves every voice's playhead on by the real seconds since the last update, as fast as it plays, unless it
    /// waits: frozen or held, on a paused bus, or the output not running. A virtual one-shot whose playhead has
    /// passed its end has ended unheard; a virtual loop goes round.
    fn advance_voices(&mut self, dt: f32, state: DeviceState) {
        if state != DeviceState::Running || !(dt > 0.0 && dt.is_finite()) {
            return;
        }
        for voice in &mut self.voices {
            if voice.frozen || self.mix.now(BusId::category(voice.bus).index()).held {
                continue;
            }
            match voice.mode {
                Mode::Real(_) => voice.seconds += f64::from(dt) * f64::from(voice.sent.pitch),
                Mode::Virtual => {
                    voice.seconds += f64::from(dt) * f64::from(voice.params.pitch);
                    if voice.looped && voice.length > 0.0 {
                        voice.seconds %= voice.length;
                    } else if voice.seconds >= voice.length {
                        voice.mode = Mode::Gone;
                    }
                }
                Mode::Asked | Mode::Gone => {}
            }
        }
        self.voices.retain(Voice::live);
    }

    /// Ends each loop whose emitter this frame didn't report: it plays only while what makes it is there.
    fn end_loops_left_behind(&mut self) {
        for index in 0..self.voices.len() {
            let voice = self.voices[index];
            let Place::On(id) = voice.place else { continue };
            let reported =
                || self.emitters.iter().any(|tracked| tracked.emitter.id == id && tracked.reported);
            if voice.looped && !reported() {
                self.end_voice(index, STOP_FADE);
            }
        }
        self.voices.retain(Voice::live);
    }

    /// Ends the conversation's line once its voice has.
    fn advance_speech(&mut self) {
        let Some(speech) = &mut self.speech else { return };
        let Some((id, _)) = speech.voice else { return };
        if !self.voices.iter().any(|voice| voice.id == id) {
            speech.voice = None;
        }
    }

    /// Moves the mix's snapshots toward what the frame asks, and sends each bus's filter and pause where they
    /// changed: before the frame's voices start, so they start bent as the bus is.
    fn step_mix(&mut self, frame: &AudioFrame<'_>) {
        self.mix.step_snapshots(frame.snapshots, frame.world_time_scale, frame.dt);
        for (bus, sent) in self.sent.iter_mut().enumerate() {
            let now = self.mix.now(bus);
            if self.mix.filtered(bus) && now.low_pass_hz != sent.low_pass_hz {
                self.backend.set_bus(BusId(bus as u16), BusParams { low_pass_hz: now.low_pass_hz });
                sent.low_pass_hz = now.low_pass_hz;
            }
            if now.paused != sent.paused {
                self.backend.pause_bus(
                    BusId(bus as u16),
                    now.paused,
                    Duration::from_secs_f32(now.pause_fade),
                );
                sent.paused = now.paused;
            }
        }
    }

    /// Plays the reverb of the listener's room (M6 U5), crossfading from the last when it changes: over the fade of
    /// the room entered, or of the room left for none. A room whose reverb the mix hasn't plays none, counted.
    fn follow_room(&mut self, rooms: &Rooms<'_>, listener: &Listener) {
        let at = rooms.at(listener.position);
        self.listener_space = rooms.space_at(listener.position);
        let room = at.map(|index| &rooms.rooms[index]);
        let reverb = room
            .and_then(|room| self.reverbs.iter().position(|reverb| Name::new(&reverb.name) == room.reverb));
        if at != self.listener_room {
            self.listener_room = at;
            if room.is_some() && reverb.is_none() {
                self.stats.reverbs_unknown += 1;
                if self.stats.reverbs_unknown == 1 {
                    tracing::warn!(target: LOG_TARGET, "the listener's room names a reverb the mix hasn't: it plays none");
                }
            }
        }
        if reverb == self.room.map(|(playing, _)| playing) {
            return;
        }
        // The room entered's fade, or for none the room left's.
        let fade = match (reverb, room, self.room) {
            (Some(_), Some(room), _) => room.fade,
            (None, _, Some((_, fade))) => fade,
            _ => 0.0,
        };
        self.backend.set_room(reverb.map(|reverb| &self.reverbs[reverb]), seconds(fade));
        self.room = reverb.map(|reverb| (reverb, fade));
        self.stats.room_changes += 1;
    }

    /// `voice`'s level to the room's reverb (M6 U5): its bus's, moving with the bus's gain, if a room's reverb
    /// plays and the voice is in the listener's space; nothing otherwise. A voice in no place is at the listener.
    fn send_of(&self, voice: &Voice, rooms: &Rooms<'_>) -> f32 {
        let bus = BusId::category(voice.bus).index();
        let (Some(_), Some(level)) = (self.room, self.room_db[bus]) else { return SILENCE_DB };
        let space = self.position_of(voice).map_or(self.listener_space, |position| rooms.space_at(position));
        if space != self.listener_space {
            return SILENCE_DB;
        }
        (level + self.sent[bus].gain_db).max(SILENCE_DB)
    }

    /// Where `voice` sounds from: none for one played flat.
    fn position_of(&self, voice: &Voice) -> Option<Vec3> {
        match voice.place {
            Place::Flat => None,
            Place::At(position) => Some(position),
            Place::On(id) => self
                .emitters
                .iter()
                .find(|tracked| tracked.emitter.id == id)
                .map(|tracked| tracked.emitter.position),
        }
    }

    /// Starts the line a conversation asks for, cutting the one it said before, or holds or lets go of the
    /// one it's saying: every voice of its start (M6 U4a), the sounds layered with the line's own among them.
    fn follow_speech(
        &mut self,
        asked: Option<SpeechRequest>,
        state: DeviceState,
        listener: &Listener,
        random: &mut impl CueRandom,
    ) {
        if let (Some(speech), Some(asked)) = (&self.speech, asked)
            && speech.serial == asked.serial
        {
            // The line's voice is its start's first, so its serial is the start's.
            if let Some((start, _)) = speech.voice {
                for voice in self.voices.iter_mut().filter(|voice| voice.start == start) {
                    voice.held = asked.held;
                }
            }
            return;
        }
        if let Some((start, _)) = self.speech.take().and_then(|speech| speech.voice) {
            for index in (0..self.voices.len()).rev() {
                if self.voices[index].start == start {
                    self.end_voice(index, STOP_FADE);
                    self.voices.remove(index);
                }
            }
        }
        let Some(asked) = asked else { return };
        let mut speech = Speech { serial: asked.serial, voice: None };
        match self.voice.line(asked.line) {
            None => self.stats.lines_missing += 1,
            Some(cue) => {
                self.stats.lines_asked += 1;
                if state.plays() {
                    let request = CueRequest::new(cue, asked.target);
                    speech.voice = self.start_cue(&request, listener, random, Some(asked.held));
                } else {
                    self.stats.dropped_no_output += 1;
                }
            }
        }
        self.speech = Some(speech);
    }

    /// Asks for `request`'s voices, one a sound its tree picks, placed with the rest at the end of the update: the
    /// conversation's line if `speech` says whether it's held. Its own limit and its group's make room or refuse
    /// it first, counting the start once, and then the mixer's room takes all of its voices or none. The first
    /// voice's serial and the sound it picked, if it was let in. What its tree remembers, avoid-repeat's last
    /// picks and a sequence's next, moves on only if it was.
    fn start_cue(
        &mut self,
        request: &CueRequest,
        listener: &Listener,
        random: &mut impl CueRandom,
        speech: Option<bool>,
    ) -> Option<(VoiceId, AssetId)> {
        let started = self.start_picked(request, listener, random, speech);
        if let Some((_, plan)) = self.cues.get_mut(&request.cue) {
            plan.settle(started.is_some());
        }
        started
    }

    /// [`Self::start_cue`], its tree's memory left to settle.
    fn start_picked(
        &mut self,
        request: &CueRequest,
        listener: &Listener,
        random: &mut impl CueRandom,
        speech: Option<bool>,
    ) -> Option<(VoiceId, AssetId)> {
        let Some((cue, plan)) = self.cues.get_mut(&request.cue) else {
            self.count_unknown(|| format!("cue {} isn't loaded", request.cue));
            return None;
        };
        // Always the same three numbers, whatever the cue's tree and ranges, so one cue's shape moves no other's
        // picks.
        let (pick, volume, pitch) = (random.next_u32(), random.next_unit(), random.next_unit());
        let picks = plan.pick(pick, &request.parameters);
        let Some(&(sound, tree_db)) = picks.as_slice().first() else {
            self.count_unknown(|| format!("cue {} picked no sound", request.cue));
            return None;
        };
        let place = match request.target {
            CueTarget::Flat => Place::Flat,
            CueTarget::At(position) => Place::At(position),
            CueTarget::On(id) => Place::On(id),
        };
        let group =
            cue.group.as_deref().and_then(|name| self.groups.iter().position(|group| group.name == name));
        let [quiet, loud] = cue.volume_db;
        let [slow, fast] = cue.pitch;
        let gain_db = quiet + (loud - quiet) * volume;
        let id = VoiceId(self.next_id);
        let mut voice = Voice {
            id,
            mode: Mode::Asked,
            start: id,
            cue: request.cue,
            sound,
            place,
            priority: cue.priority,
            group: group.map(|group| group as u16),
            looped: cue.looped,
            length: self.lengths.get(&sound).copied().unwrap_or_default(),
            protected: speech.is_some(),
            params: VoiceParams::NEUTRAL,
            bus: cue.bus,
            seconds: 0.0,
            attenuation: cue.attenuation,
            hitstop: cue.hitstop,
            gain_db: gain_db + tree_db,
            pitch: slow + (fast - slow) * pitch,
            sent: VoiceParams::NEUTRAL,
            frozen: false,
            held: speech.unwrap_or(false),
        };
        let limit = cue.limit;
        if let Place::On(id) = place
            && !self.emitters.iter().any(|tracked| tracked.emitter.id == id)
        {
            self.count_unknown(|| format!("emitter {id:?} isn't in this frame"));
            return None;
        }
        if voice.looped && !matches!(place, Place::On(_)) {
            self.count_unknown(|| format!("cue {} loops, so it plays only on an emitter", request.cue));
            return None;
        }
        voice.frozen = self.is_frozen(&voice);
        voice.params = self.params_of(&voice, listener);
        // The start is judged by its loudest voice, as one already playing is (`judged`).
        let mut new = voice;
        for &(sound, tree_db) in &picks.as_slice()[1..] {
            let side = Voice { sound, gain_db: gain_db + tree_db, ..voice };
            new.params.gain_db = new.params.gain_db.max(self.params_of(&side, listener).gain_db);
        }
        // The conversation's line is outside the room the rest share; the sounds layered with it aren't.
        let sharing = picks.as_slice().len() - usize::from(voice.protected);
        let let_in = (voice.protected
            || (limit.is_none_or(|limit| {
                self.make_room(&new, limit.max.into(), Some(limit.rule), |other| other.cue == voice.cue)
            }) && group.is_none_or(|group| {
                let limit = self.groups[group].limit;
                self.make_room(&new, limit.max.into(), Some(limit.rule), |other| other.group == voice.group)
            })))
            && (sharing == 0 || self.make_room(&new, MAX_LIVE + 1 - sharing, None, |_| true));
        if !let_in {
            self.stats.dropped_voice_limit += picks.as_slice().len() as u64;
            return None;
        }
        for (index, &(sound, tree_db)) in picks.as_slice().iter().enumerate() {
            let mut voice = voice;
            if index > 0 {
                voice.id = VoiceId(self.next_id);
                (voice.protected, voice.sound) = (false, sound);
                voice.length = self.lengths.get(&sound).copied().unwrap_or_default();
                voice.gain_db = gain_db + tree_db;
                voice.params = self.params_of(&voice, listener);
            }
            self.next_id += 1;
            self.voices.push(voice);
        }
        Some((voice.id, voice.sound))
    }

    /// Makes room for `new` among the live voices `counts` picks, `max` at most, by `rule`: whether it may play.
    /// A limit's rule counts starts, each by the first of its voices still live; no rule is the room kept for live
    /// voices, counting each, where what has the worst claim goes if the new one's is better. A start is judged by
    /// its loudest voice, `new` as well, and whatever goes takes its whole start with it. The conversation's line
    /// counts toward no limit and is never what goes.
    fn make_room(
        &mut self,
        new: &Voice,
        max: usize,
        rule: Option<LimitRule>,
        counts: impl Fn(&Voice) -> bool,
    ) -> bool {
        let members = |voices: &[Voice], index: usize| {
            let voice = &voices[index];
            let stands = if rule.is_some() { counts_start(voices, index) } else { voice.live() };
            stands && !voice.protected && counts(voice)
        };
        while (0..self.voices.len()).filter(|&index| members(&self.voices, index)).count() >= max {
            // Each judged as its start: as loud as its loudest voice, and with the best claim any of them has.
            let candidates = (0..self.voices.len())
                .filter(|&index| members(&self.voices, index))
                .map(|index| (index, judged(&self.voices, index)));
            let victim = match rule {
                Some(LimitRule::RefuseNew) => return false,
                Some(LimitRule::StopOldest) => candidates.min_by_key(|(_, voice)| voice.start),
                Some(LimitRule::StopQuietest) => candidates.min_by(|(_, a), (_, b)| {
                    a.params.gain_db.total_cmp(&b.params.gain_db).then(a.start.cmp(&b.start))
                }),
                Some(LimitRule::StopLowestPriority) => {
                    candidates.min_by_key(|(_, voice)| (voice.priority, voice.start))
                }
                None => candidates.max_by(|(_, a), (_, b)| standing(a, b)),
            };
            let Some((index, ref victim)) = victim else { return false };
            let refused = match rule {
                Some(LimitRule::StopQuietest) => new.params.gain_db <= victim.params.gain_db,
                Some(LimitRule::StopLowestPriority) => new.priority < victim.priority,
                None => standing(new, victim).is_ge(),
                Some(LimitRule::RefuseNew | LimitRule::StopOldest) => false,
            };
            if refused {
                return false;
            }
            self.steal(index);
        }
        true
    }

    /// Ends the voice at `index` to make room, and the rest of its start with it but the conversation's line: one
    /// asked for this frame never starts.
    fn steal(&mut self, index: usize) {
        let start = self.voices[index].start;
        for index in (0..self.voices.len()).rev() {
            let voice = &self.voices[index];
            if voice.start != start || voice.protected {
                continue;
            }
            match voice.mode {
                Mode::Asked => self.stats.dropped_voice_limit += 1,
                Mode::Real(_) | Mode::Virtual => self.stats.voices_stolen += 1,
                Mode::Gone => {}
            }
            self.end_voice(index, STOP_FADE);
            self.voices.remove(index);
        }
    }

    /// Whether `voice` should wait where it is: held by its speech, or frozen by its emitter's hitstop.
    fn is_frozen(&self, voice: &Voice) -> bool {
        voice.held || (self.in_hitstop(voice) && voice.hitstop == HitstopResponse::Freeze)
    }

    fn count_unknown(&mut self, message: impl FnOnce() -> String) {
        self.stats.dropped_unknown += 1;
        if !self.warned_unknown {
            self.warned_unknown = true;
            tracing::warn!(target: LOG_TARGET, "dropping a cue: {}", message());
        }
    }

    /// Brings every voice up to date and decides which are mixed (M6 U3): ranked by [`standing`], the first
    /// [`VoiceCaps::real`] are real and the next [`VoiceCaps::virtual_voices`] that can wait are virtual, and the
    /// rest end. A voice going virtual fades out over [`VIRTUAL_FADE`] and one coming back fades in over it, where
    /// it would be. Then the backend hears of each change, oldest voice first.
    fn place_voices(&mut self, listener: &Listener, rooms: &Rooms<'_>) {
        for index in 0..self.voices.len() {
            let voice = self.voices[index];
            let send_db = self.send_of(&voice, rooms);
            self.voices[index].params = VoiceParams { send_db, ..self.params_of(&voice, listener) };
        }
        let voices = &self.voices;
        self.order.clear();
        self.order.extend(0..voices.len() as u16);
        self.order.sort_unstable_by(|&a, &b| standing(&voices[usize::from(a)], &voices[usize::from(b)]));
        let (mut real, mut waiting) = (0, 0);
        let mut wants = [false; MAX_LIVE + 1];
        let mut keeps = [false; MAX_LIVE + 1];
        for &index in &self.order {
            let voice = &self.voices[usize::from(index)];
            if real < self.caps.real {
                real += 1;
                wants[usize::from(index)] = true;
                keeps[usize::from(index)] = true;
            } else if voice.can_wait() && waiting < self.caps.virtual_voices {
                waiting += 1;
                keeps[usize::from(index)] = true;
            }
        }
        for index in 0..self.voices.len() {
            self.place(index, wants[index], keeps[index]);
        }
        self.voices.retain(Voice::live);
    }

    /// Takes the voice at `index` real if `real`, virtual if only `kept`, or ends it, telling the backend.
    fn place(&mut self, index: usize, real: bool, kept: bool) {
        let voice = self.voices[index];
        let frozen = self.is_frozen(&voice);
        match (voice.mode, real, kept) {
            (Mode::Real(slot), true, _) => {
                if frozen != voice.frozen {
                    self.backend.freeze_voice(slot, frozen);
                }
                if moved(voice.params, voice.sent) {
                    self.backend.set_voice(slot, voice.params);
                    self.voices[index].sent = voice.params;
                }
                self.voices[index].frozen = frozen;
            }
            (Mode::Real(_), false, true) => {
                self.end_voice(index, VIRTUAL_FADE);
                self.voices[index].mode = Mode::Virtual;
                self.voices[index].frozen = frozen;
                self.stats.voices_virtualized += 1;
            }
            (Mode::Real(_), false, false) => {
                self.end_voice(index, STOP_FADE);
                self.stats.voices_stolen += 1;
            }
            (Mode::Asked | Mode::Virtual, true, _) => self.start(index, frozen, kept),
            (Mode::Asked, false, true) => {
                self.voices[index].mode = Mode::Virtual;
                self.voices[index].frozen = frozen;
                self.stats.cues_played += u64::from(self.first_to_play(index));
                self.stats.voices_virtualized += 1;
            }
            (Mode::Virtual, false, true) => self.voices[index].frozen = frozen,
            (Mode::Asked, false, false) => {
                self.voices[index].mode = Mode::Gone;
                self.stats.dropped_voice_limit += 1;
            }
            (Mode::Virtual, false, false) => {
                self.voices[index].mode = Mode::Gone;
                self.stats.voices_stolen += 1;
            }
            (Mode::Gone, ..) => {}
        }
    }

    /// Whether the voice at `index`, asked for and now playing real or virtual, is its start's first to (M6 U4a):
    /// a start is one cue played, whichever of its voices the caps let in.
    fn first_to_play(&self, index: usize) -> bool {
        start_before(&self.voices, index).all(|voice| !voice.playing())
    }

    /// Starts the voice at `index` in the mixer: one just asked for from its sound's start, a virtual one where
    /// it would be by now, fading in. With no slot or no room in the mixer it waits virtual if it may, `kept`.
    fn start(&mut self, index: usize, frozen: bool, kept: bool) {
        let voice = self.voices[index];
        let revived = voice.mode == Mode::Virtual;
        let once = VoiceStart { looped: voice.looped, ..VoiceStart::once(voice.params, frozen) };
        let start = if revived {
            let from_seconds =
                if voice.looped && voice.length > 0.0 { voice.seconds % voice.length } else { voice.seconds };
            VoiceStart { from_seconds, fade_in: VIRTUAL_FADE, ..once }
        } else {
            once
        };
        let result = match self.free_slots.pop() {
            None => Err(StartError::MixerFull),
            Some(slot) => {
                let started = self.backend.start_voice(slot, voice.sound, BusId::category(voice.bus), start);
                if started.is_err() {
                    self.free_slots.push(slot);
                }
                started.map(|()| slot)
            }
        };
        self.voices[index].frozen = frozen;
        match result {
            Ok(slot) => {
                self.voices[index].mode = Mode::Real(slot);
                self.voices[index].sent = voice.params;
                if revived {
                    self.stats.voices_revived += 1;
                } else {
                    self.stats.cues_played += u64::from(self.first_to_play(index));
                }
            }
            Err(StartError::MixerFull) if kept && voice.can_wait() => {
                if !revived {
                    self.voices[index].mode = Mode::Virtual;
                    self.stats.cues_played += u64::from(self.first_to_play(index));
                    self.stats.voices_virtualized += 1;
                }
            }
            Err(error) => {
                self.voices[index].mode = Mode::Gone;
                match error {
                    StartError::UnknownSound(_) => self.count_unknown(|| error.to_string()),
                    StartError::MixerFull if revived => self.stats.voices_stolen += 1,
                    StartError::MixerFull => self.stats.dropped_voice_limit += 1,
                }
            }
        }
    }

    /// Moves the ducks toward the voices playing now, the frame's among them, and brings each bus's gain to its own
    /// volume with the snapshots' and the ducks' on top. The music's carries the
    /// player's volume too, as every voice does, since its stems' gains are the score's. A gain moving sends
    /// only steps a voice would; one come to rest is sent exactly.
    fn follow_bus_gains(&mut self, dt: f32) {
        let (mix, voices, mouths) = (&self.mix, &self.voices, &self.mouths);
        for (level, (when, follow)) in self.levels.iter_mut().zip(mix.rules()) {
            // The loudest of the rule's voices: a line by its mouth at its playhead, anything else by being there.
            *level = voices
                .iter()
                .filter(|voice| {
                    let bus = BusId::category(voice.bus).index();
                    matches!(voice.mode, Mode::Real(_))
                        && !voice.frozen
                        && mix.is_under(bus, when)
                        && !mix.now(bus).held
                })
                .map(|voice| match (follow, mouths.get(&voice.sound)) {
                    (DuckFollow::Envelope { full_at }, Some(mouth)) => {
                        (mouth.at(voice.seconds as f32).open / full_at).min(1.0)
                    }
                    _ => 1.0,
                })
                .fold(0.0, f32::max);
        }
        self.mix.step_ducks(&self.levels, dt);
        let music = BusId::category(Bus::Music).index();
        for (bus, sent) in self.sent.iter_mut().enumerate() {
            let player_db = if bus == music { self.bus_volume_db[Bus::Music.index()] } else { 0.0 };
            let rest_db = (self.mix.volume_db(bus) + player_db).max(SILENCE_DB);
            let gain_db = (rest_db + self.mix.now(bus).gain_db).max(SILENCE_DB);
            let moved = (gain_db - sent.gain_db).abs() > GAIN_STEP_DB
                || (gain_db == rest_db && gain_db != sent.gain_db);
            if moved {
                self.backend.set_bus_gain(BusId(bus as u16), gain_db, BUS_FADE);
                sent.gain_db = gain_db;
            }
        }
    }

    fn in_hitstop(&self, voice: &Voice) -> bool {
        let Place::On(id) = voice.place else { return false };
        self.emitters.iter().any(|tracked| tracked.emitter.id == id && tracked.emitter.hitstop)
    }

    /// What `voice` should sound like now.
    fn params_of(&self, voice: &Voice, listener: &Listener) -> VoiceParams {
        let position = self.position_of(voice);
        // The music bus carries the player's music volume itself, for the score's stems
        // ([`Self::follow_bus_gains`]), so a cue on it takes it there and not again here (M4 A8). The overall
        // volume is the master's (M6 U10).
        let volume_db = match voice.bus {
            Bus::Music => 0.0,
            bus => self.bus_volume_db[bus.index()],
        };
        let mut gain_db = voice.gain_db + volume_db;
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
        let pitch = voice.pitch * self.mix.now(BusId::category(voice.bus).index()).pitch;
        VoiceParams { gain_db, pan, pitch, ..VoiceParams::NEUTRAL }
    }
}

/// Each of `mix`'s buses as a new mixer has it: at its own volume, open and playing.
fn sent_of(mix: &Mix) -> Vec<SentBus> {
    mix.buses
        .iter()
        .map(|bus| SentBus {
            gain_db: bus.volume_db.max(SILENCE_DB),
            low_pass_hz: f32::INFINITY,
            paused: false,
        })
        .collect()
}

/// A share of full volume in decibels. Below [`SILENCE_DB`] the mixer's silent anyway, and it's finite
/// whatever it's given: see `VoiceParams`.
fn decibels_of(volume: f32) -> f32 {
    if volume > 0.0 { (20.0 * volume.log10()).clamp(SILENCE_DB, 0.0) } else { SILENCE_DB }
}

/// `seconds` as a duration, none for one that isn't a positive number.
fn seconds(seconds: f32) -> Duration {
    Duration::from_secs_f32(if seconds.is_finite() { seconds.max(0.0) } else { 0.0 })
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
        || (now.send_db - sent.send_db).abs() > GAIN_STEP_DB
}

#[cfg(test)]
mod tests {
    use sapphire_assets::cue::{AttenuationCurve, CueNode, Limit};

    use super::*;
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
            play: CueNode::one_of(vec![SOUND_A]),
            volume_db: [0.0, 0.0],
            pitch: [1.0, 1.0],
            attenuation: None,
            bus: Bus::Sfx,
            hitstop: HitstopResponse::Ignore,
            ..Cue::default()
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
            snapshots: &[],
            emitters,
            cues,
            lines: &[],
            speech: None,
            music: None,
            rooms: Rooms::NONE,
        }
    }

    /// The game's mix, as shipped.
    fn shipped() -> Mix {
        Mix::from_document(include_str!("../../../content/audio/game.mix.ron")).unwrap()
    }

    fn music_bus() -> BusId {
        BusId::category(Bus::Music)
    }

    fn play(target: CueTarget) -> [CueRequest; 1] {
        [CueRequest::new(CUE, target)]
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
        assert_eq!(started(&mut system), VoiceParams::NEUTRAL);
        assert_eq!(system.stats().cues_played, 1);
        assert_eq!(system.stats().voices, 1);
    }

    #[test]
    fn the_seed_pins_every_pick() {
        let cue = Cue {
            play: CueNode::one_of(vec![SOUND_A, SOUND_B]),
            volume_db: [-6.0, 0.0],
            pitch: [0.8, 1.2],
            ..plain_cue()
        };
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
        // The master's last gain (M6 U10), once: no voice and no bus moves for it.
        system.set_volume(0.5);
        system.set_volume(0.5);
        let calls = system.backend_mut().take_calls();
        let [MockCall::SetOutputVolume(volume_db)] = calls[..] else { panic!("{calls:?}") };
        assert!((volume_db + 6.02).abs() < 0.01, "{volume_db}");
        system.update(&frame(&[], &[]), &mut rng);
        assert_eq!(system.backend_mut().take_calls(), []);
        system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        assert_eq!(started(&mut system).gain_db, 0.0);
        // Off is silent, and nothing a setting says can reach the mixer as a bad number.
        for volume in [0.0, -1.0, f32::NAN] {
            system.set_volume(volume);
            assert_eq!(system.volume_db, SILENCE_DB, "{volume}");
        }
        system.set_volume(3.0);
        assert_eq!(system.volume_db, 0.0);
        let calls = system.backend_mut().take_calls();
        assert_eq!(calls, [MockCall::SetOutputVolume(SILENCE_DB), MockCall::SetOutputVolume(0.0)]);
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
        let [MockCall::SetBusGain { bus, gain_db, .. }] = calls[..] else { panic!("{calls:?}") };
        assert_eq!(bus, music_bus());
        assert!((gain_db + 20.0).abs() < 0.01, "{gain_db}");
        // The effects' is on the voice, every sound's on the master after it (M6 U10).
        system.set_volumes(Volumes { all: 0.5, effects: 0.5, ..Volumes::FULL });
        system.update(&frame(&[], &[]), &mut rng);
        let calls = system.backend_mut().take_calls();
        let [
            MockCall::SetOutputVolume(all_db),
            MockCall::SetVoice { params, .. },
            MockCall::SetBusGain { bus, gain_db, .. },
        ] = calls[..]
        else {
            panic!("{calls:?}")
        };
        assert!((all_db + 6.02).abs() < 0.01, "{all_db}");
        assert_eq!(bus, music_bus());
        assert!((params.gain_db + 6.02).abs() < 0.01, "{params:?}");
        assert_eq!(gain_db, 0.0, "the music back at full, under every sound's on the master");
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
        // The shipped mix's `slow_motion`: v0.2.0's numbers.
        let (floor_scale, floor_low_pass_hz, floor_pitch) = (0.2, 900.0, 0.75);
        let mut system = system_with(plain_cue());
        system.set_mix(&shipped()).unwrap();
        let ui = AssetId::from_text("22222222-2222-4333-8444-555555555555");
        system.add_cue(ui, Cue { bus: Bus::Ui, ..plain_cue() });
        let mut rng = CueRng::new(1);
        let both = [CueRequest::new(CUE, CueTarget::Flat), CueRequest::new(ui, CueTarget::Flat)];
        system.update(&frame(&[], &both), &mut rng);
        let calls = system.backend_mut().take_calls();
        assert!(!calls.iter().any(|call| matches!(call, MockCall::SetBus { .. })), "{calls:?}");

        let slow = AudioFrame { world_time_scale: floor_scale, ..frame(&[], &[]) };
        for _ in 0..600 {
            system.update(&slow, &mut rng);
        }
        let calls = system.backend_mut().take_calls();
        let last_of_bus = |bus: Bus| {
            calls.iter().rev().find_map(|call| match *call {
                MockCall::SetBus { bus: seen, params } if seen == BusId::category(bus) => Some(params),
                _ => None,
            })
        };
        let low_pass = last_of_bus(Bus::Sfx).expect("the world's bus dips").low_pass_hz;
        assert!((low_pass - floor_low_pass_hz).abs() < 1.0, "{low_pass}");
        assert_eq!(last_of_bus(Bus::Ambience).map(|params| params.low_pass_hz), Some(low_pass));
        assert_eq!(last_of_bus(Bus::Ui), None, "the interface stays as it is");

        let last_of_voice = |slot: VoiceSlot| {
            calls.iter().rev().find_map(|call| match *call {
                MockCall::SetVoice { slot: seen, params } if seen == slot => Some(params),
                _ => None,
            })
        };
        let world = last_of_voice(VoiceSlot(0)).expect("the world's voice bends");
        assert!((world.pitch - floor_pitch).abs() < 2e-3, "{world:?}");
        assert_eq!(last_of_voice(VoiceSlot(1)), None, "the interface's doesn't");

        // A voice started in the dip starts dipped.
        let cues = play(CueTarget::Flat);
        system.update(&AudioFrame { cues: &cues, ..slow }, &mut rng);
        assert!((started(&mut system).pitch - floor_pitch).abs() < 2e-3);

        // Back at speed, everything comes all the way back.
        for _ in 0..600 {
            system.update(&frame(&[], &[]), &mut rng);
        }
        let calls = system.backend_mut().take_calls();
        let open = MockCall::SetBus { bus: BusId::category(Bus::Sfx), params: BusParams::OPEN };
        assert!(calls.contains(&open), "{calls:?}");
    }

    /// M6 U2: a mix that doesn't check changes nothing, and one that does ends what played on the last.
    #[test]
    fn a_mix_is_checked_before_it_replaces_the_last() {
        let mut system = system_with(plain_cue());
        let mut rng = CueRng::new(1);
        system.update(&frame(&[], &play(CueTarget::Flat)), &mut rng);
        system.backend_mut().take_calls();
        let mut bad = shipped();
        bad.buses[0].volume_db = f32::NAN;
        assert!(system.set_mix(&bad).is_err());
        assert_eq!(system.backend_mut().take_calls(), []);
        system.set_mix(&shipped()).unwrap();
        let buses = shipped().buses.len();
        assert_eq!(
            system.backend_mut().take_calls(),
            [MockCall::StopVoice { slot: VoiceSlot(0), fade: STOP_FADE }, MockCall::SetMix { buses }]
        );
        system.set_output_profile(OutputProfile::Portable);
        assert_eq!(system.backend_mut().take_calls(), [MockCall::SetOutputProfile(OutputProfile::Portable)]);
    }

    /// M6 U2: the shipped `pause` holds the world's buses and the voices', fading as it says, and lets them go on;
    /// a voice on a held bus keeps its playhead where it is.
    #[test]
    fn the_pause_snapshot_holds_the_world_and_lets_it_go() {
        let mut system = speaker(1.0);
        system.set_mix(&shipped()).unwrap();
        let mut rng = CueRng::new(1);
        say(&mut system, greet(1, false), &mut rng);
        say(&mut system, greet(1, false), &mut rng);
        system.backend_mut().take_calls();
        let paused = [PAUSE_SNAPSHOT];
        system
            .update(&AudioFrame { snapshots: &paused, speech: greet(1, false), ..frame(&[], &[]) }, &mut rng);
        let calls = system.backend_mut().take_calls();
        let pauses: Vec<_> = calls
            .iter()
            .filter_map(|call| match *call {
                MockCall::PauseBus { bus, paused, fade } => Some((bus, paused, fade)),
                _ => None,
            })
            .collect();
        let attack = Duration::from_secs_f32(0.05);
        let world = [Bus::Sfx, Bus::Ambience, Bus::Voice].map(|bus| (BusId::category(bus), true, attack));
        assert_eq!(pauses, world, "{calls:?}");
        let held = seconds(&system).unwrap();
        for _ in 0..10 {
            system.update(
                &AudioFrame { snapshots: &paused, speech: greet(1, false), ..frame(&[], &[]) },
                &mut rng,
            );
        }
        assert_eq!(seconds(&system), Some(held), "the line waits with its bus");
        assert!(!system.backend().calls().iter().any(|call| matches!(call, MockCall::PauseBus { .. })));
        // The music goes down and darkens rather than stopping.
        let music = system.backend().calls().iter().rev().find_map(|call| match *call {
            MockCall::SetBus { bus, params } if bus == music_bus() => Some(params.low_pass_hz),
            _ => None,
        });
        assert!(music.is_some_and(|hz| hz < 2_500.0), "{music:?}");
        system.backend_mut().take_calls();
        say(&mut system, greet(1, false), &mut rng);
        let release = Duration::from_secs_f32(0.2);
        let resumed = system.backend_mut().take_calls();
        for bus in [Bus::Sfx, Bus::Ambience, Bus::Voice] {
            let call = MockCall::PauseBus { bus: BusId::category(bus), paused: false, fade: release };
            assert!(resumed.contains(&call), "{resumed:?}");
        }
        say(&mut system, greet(1, false), &mut rng);
        assert!(seconds(&system).unwrap() > held);
    }

    /// M6 U2: a ducking rule takes its buses down while a line's mouth is open, by how open, and lets them back up
    /// in a gap; a voice with no mouth curve ducks them all the way while it plays.
    #[test]
    fn a_line_ducks_the_music_by_its_mouth_and_lets_it_up_in_a_gap() {
        // A line of 0.5 s spoken, 0.5 s silent, 0.5 s spoken.
        let rate = 48_000;
        let samples: Vec<f32> = (0..rate * 3 / 2)
            .map(|frame| {
                let spoken = !(rate / 2..rate).contains(&frame);
                if spoken { 0.5 * (frame as f32 * 0.05).sin() } else { 0.0 }
            })
            .collect();
        let sound = DecodedSound { sample_rate: rate as u32, channels: 1, samples };
        let mouth = MouthCurve::analyze(&sound);
        let mut system = speaker(1.0);
        system.set_mix(&shipped()).unwrap();
        system.add_mouth(SOUND_A, mouth);
        let mut rng = CueRng::new(1);
        let mut music_gain = 0.0;
        let mut gains = Vec::new();
        for _ in 0..80 {
            say(&mut system, greet(1, false), &mut rng);
            for call in system.backend_mut().take_calls() {
                if let MockCall::SetBusGain { bus, gain_db, .. } = call
                    && bus == music_bus()
                {
                    music_gain = gain_db;
                }
            }
            gains.push(music_gain);
        }
        // Down by the rule's 9 dB while it speaks, back up in the gap, and down again.
        let at = |seconds: f32| gains[(seconds * 60.0) as usize];
        assert!((at(0.4) + 9.0).abs() < 0.2, "{gains:?}");
        assert!(at(0.9) > -0.5, "{gains:?}");
        assert!((at(1.3) + 9.0).abs() < 0.2, "{gains:?}");
        // With no curve, the line's presence is enough.
        let mut system = speaker(1.0);
        system.set_mix(&shipped()).unwrap();
        for _ in 0..30 {
            say(&mut system, greet(1, false), &mut rng);
        }
        let last = system.backend().calls().iter().rev().find_map(|call| match *call {
            MockCall::SetBusGain { bus, gain_db, .. } if bus == music_bus() => Some(gain_db),
            _ => None,
        });
        assert!(last.is_some_and(|gain| (gain + 9.0).abs() < 0.1), "{last:?}");
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
            CueRequest::new(CUE, CueTarget::On(ACTOR)),    // slot 0: plays on
            CueRequest::new(duck, CueTarget::On(ACTOR)),   // slot 1: ducks
            CueRequest::new(freeze, CueTarget::On(ACTOR)), // slot 2: freezes
            CueRequest::new(freeze, CueTarget::On(other)), // slot 3: not in hitstop
            CueRequest::new(freeze, CueTarget::Flat),      // slot 4: on no actor
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
                    params: VoiceParams { gain_db: -12.0, ..VoiceParams::NEUTRAL }
                },
                MockCall::FreezeVoice { slot: VoiceSlot(2), frozen: true },
            ]
        );
        // Nothing more while it lasts.
        system.update(&frame(&stopped, &[]), &mut rng);
        assert_eq!(system.backend().calls(), &[]);

        // A cue that fires during the hitstop starts as its response says.
        let fired =
            [CueRequest::new(duck, CueTarget::On(ACTOR)), CueRequest::new(freeze, CueTarget::On(ACTOR))];
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
        assert!(calls.contains(&MockCall::SetVoice { slot: VoiceSlot(1), params: VoiceParams::NEUTRAL }));
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
        let frozen_too =
            [CueRequest::new(CUE, CueTarget::On(ACTOR)), CueRequest::new(freeze, CueTarget::On(ACTOR))];
        system.update(&frame(&stopped, &frozen_too), &mut rng);
        assert_eq!(system.stats().voices, 2);
        system.backend_mut().take_calls();

        system.stop_voices();
        assert_eq!(
            system.backend_mut().take_calls(),
            [
                MockCall::StopVoice { slot: VoiceSlot(0), fade: STOP_FADE },
                MockCall::StopVoice { slot: VoiceSlot(1), fade: STOP_FADE }
            ]
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
        system.update(&frame(&[], &[CueRequest::new(missing, CueTarget::Flat)]), &mut rng);
        assert_eq!(system.stats().dropped_unknown, 1);
        // A cue naming a sound that never loaded, and an emitter nobody reported.
        system.add_cue(missing, Cue { play: CueNode::one_of(vec![missing]), ..plain_cue() });
        system.update(&frame(&[], &[CueRequest::new(missing, CueTarget::Flat)]), &mut rng);
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

    /// A system that speaks `greet` in English as [`CUE`], a voice at `pitch`.
    fn speaker(pitch: f32) -> AudioSystem<MockBackend> {
        let mut system = system_with(Cue { pitch: [pitch, pitch], bus: Bus::Voice, ..plain_cue() });
        let line = sapphire_assets::voice::VoiceLine { key: "greet".to_owned(), cue: CUE };
        system.add_voice_lines(&VoiceLines { locale: "en".to_owned(), lines: vec![line] }).unwrap();
        assert!(system.set_voice_language("en"));
        system
    }

    fn say(system: &mut AudioSystem<MockBackend>, speech: Option<SpeechRequest>, rng: &mut CueRng) {
        system.update(&AudioFrame { speech, ..frame(&[], &[]) }, rng);
    }

    fn greet(serial: u64, held: bool) -> Option<SpeechRequest> {
        Some(SpeechRequest { line: LineKey::of("greet"), serial, target: CueTarget::Flat, held })
    }

    fn seconds(system: &AudioSystem<MockBackend>) -> Option<f32> {
        system.speaking().map(|speaking| speaking.seconds)
    }

    /// M5 C6: a line starts once, its playhead runs with the real seconds at the voice's pitch, a pause holds
    /// both, and the next line, or none, cuts it.
    #[test]
    fn a_conversations_line_speaks_once_holds_on_a_pause_and_is_cut_by_the_next() {
        let mut system = speaker(1.25);
        let mut rng = CueRng::new(1);
        say(&mut system, greet(1, false), &mut rng);
        let calls = system.backend_mut().take_calls();
        assert!(matches!(
            calls[..],
            [MockCall::StartVoice { slot: VoiceSlot(0), sound: SOUND_A, frozen: false, .. }]
        ));
        let speaking = system.speaking().unwrap();
        assert_eq!((speaking.serial, speaking.sound, speaking.seconds), (1, SOUND_A, 0.0));
        for _ in 0..30 {
            say(&mut system, greet(1, false), &mut rng);
        }
        assert!(
            system.backend_mut().take_calls().iter().all(|call| !matches!(call, MockCall::StartVoice { .. }))
        );
        let played = seconds(&system).unwrap();
        assert!((played - 30.0 * FRAME * 1.25).abs() < 1e-5, "{played}");

        // A pause: the voice waits, and so does its playhead, over the frames it's held.
        say(&mut system, greet(1, true), &mut rng);
        assert!(
            system.backend().calls().contains(&MockCall::FreezeVoice { slot: VoiceSlot(0), frozen: true })
        );
        let held = seconds(&system).unwrap();
        for _ in 0..10 {
            say(&mut system, greet(1, true), &mut rng);
        }
        assert_eq!(seconds(&system), Some(held));
        say(&mut system, greet(1, false), &mut rng);
        assert!(
            system.backend().calls().contains(&MockCall::FreezeVoice { slot: VoiceSlot(0), frozen: false })
        );
        assert_eq!(seconds(&system), Some(held));
        system.backend_mut().take_calls();

        // The same line again, as a new showing: the old voice stops and a new one starts from nothing.
        say(&mut system, greet(2, false), &mut rng);
        let calls = system.backend_mut().take_calls();
        assert!(calls.contains(&MockCall::StopVoice { slot: VoiceSlot(0), fade: STOP_FADE }));
        assert!(calls.iter().any(|call| matches!(call, MockCall::StartVoice { .. })));
        assert_eq!(system.speaking().map(|speaking| (speaking.serial, speaking.seconds)), Some((2, 0.0)));
        // The conversation's end cuts it.
        say(&mut system, None, &mut rng);
        assert!(
            system.backend_mut().take_calls().iter().any(|call| matches!(call, MockCall::StopVoice { .. }))
        );
        assert_eq!(system.speaking(), None);
        assert_eq!((system.stats().lines_asked, system.stats().lines_missing), (2, 0));
    }

    /// A suspended output holds the playhead; a voice played out, or stopped by a start over, ends the line,
    /// and the same showing never starts again. A line with no voice is counted, once.
    #[test]
    fn a_line_ends_with_its_voice_and_waits_out_a_suspension() {
        let mut system = speaker(1.0);
        let mut rng = CueRng::new(1);
        say(&mut system, greet(1, false), &mut rng);
        say(&mut system, greet(1, false), &mut rng);
        let before = seconds(&system).unwrap();
        system.backend_mut().set_state(DeviceState::Suspended);
        for _ in 0..5 {
            say(&mut system, greet(1, false), &mut rng);
        }
        assert_eq!(seconds(&system), Some(before));
        system.backend_mut().set_state(DeviceState::Running);
        say(&mut system, greet(1, false), &mut rng);
        system.backend_mut().finish(VoiceSlot(0));
        say(&mut system, greet(1, false), &mut rng);
        assert_eq!(system.speaking(), None);
        say(&mut system, greet(1, false), &mut rng);
        assert_eq!((system.speaking(), system.stats().lines_asked), (None, 1));

        say(&mut system, greet(2, false), &mut rng);
        assert!(system.speaking().is_some());
        system.stop_voices();
        say(&mut system, greet(2, false), &mut rng);
        assert_eq!((system.speaking(), system.stats().lines_asked), (None, 2));

        let missing = Some(SpeechRequest { line: LineKey::of("farewell"), ..greet(3, false).unwrap() });
        say(&mut system, missing, &mut rng);
        say(&mut system, missing, &mut rng);
        assert_eq!((system.speaking(), system.stats().lines_missing), (None, 1));
    }

    /// Three seconds of sound: long enough to wait virtual.
    const LONG: AssetId = AssetId::from_text("55555555-2222-4333-8444-555555555555");
    const OTHER: AssetId = AssetId::from_text("66666666-2222-4333-8444-555555555555");

    /// A system with [`CUE`] as `cue` and [`OTHER`] as `other`, both on [`LONG`] if `long`.
    fn rivals(cue: Cue, other: Cue, long: bool) -> AudioSystem<MockBackend> {
        let mut system = system_with(plain_cue());
        system
            .add_sound(LONG, &DecodedSound { sample_rate: 48_000, channels: 1, samples: vec![0.5; 144_000] });
        let sounds = |cue: Cue| if long { Cue { play: CueNode::one_of(vec![LONG]), ..cue } } else { cue };
        system.add_cue(CUE, sounds(cue));
        system.add_cue(OTHER, sounds(other));
        system.backend_mut().take_calls();
        system
    }

    fn ask(system: &mut AudioSystem<MockBackend>, cue: AssetId, target: CueTarget, rng: &mut CueRng) {
        system.update(&frame(&[], &[CueRequest::new(cue, target)]), rng);
    }

    fn stopped(calls: &[MockCall]) -> Vec<(VoiceSlot, Duration)> {
        calls
            .iter()
            .filter_map(|call| match *call {
                MockCall::StopVoice { slot, fade } => Some((slot, fade)),
                _ => None,
            })
            .collect()
    }

    /// M6 U3: a cue's own limit, by each of its rules.
    #[test]
    fn a_cues_limit_makes_room_by_its_rule() {
        let limited = |rule| Cue { limit: Some(Limit { max: 2, rule }), ..placed_cue() };
        let ahead = |meters: f32| CueTarget::At(Vec3::new(0.0, 0.0, -meters));
        let mut rng = CueRng::new(1);

        // Two play; the third ends the first, or doesn't play.
        for (rule, stolen, refused) in [(LimitRule::StopOldest, 1, 0), (LimitRule::RefuseNew, 0, 1)] {
            let mut system = system_with(limited(rule));
            for _ in 0..3 {
                ask(&mut system, CUE, CueTarget::Flat, &mut rng);
            }
            let calls = system.backend_mut().take_calls();
            let ends: &[_] = if stolen == 1 { &[(VoiceSlot(0), STOP_FADE)] } else { &[] };
            assert_eq!(stopped(&calls), ends, "{rule:?}");
            let stats = system.stats();
            assert_eq!(
                (stats.voices_stolen, stats.dropped_voice_limit, stats.cues_played),
                (stolen, refused, 3 - refused)
            );
        }

        // The quietest goes, unless it's the new one: at 1 m and 11 m, then 5 m, then 15 m.
        let mut system = system_with(limited(LimitRule::StopQuietest));
        for meters in [1.0, 11.0, 5.0] {
            ask(&mut system, CUE, ahead(meters), &mut rng);
        }
        assert_eq!(stopped(&system.backend_mut().take_calls()), [(VoiceSlot(1), STOP_FADE)]);
        ask(&mut system, CUE, ahead(15.0), &mut rng);
        assert_eq!((system.stats().voices_stolen, system.stats().dropped_voice_limit), (1, 1));
    }

    /// M6 U4a: a layer's sounds are one start's voices. They start together, count once toward a limit, and go
    /// together when the limit makes room; the start counts as one cue played.
    #[test]
    fn a_layers_voices_count_once_and_go_together() {
        let layered = Cue {
            play: CueNode::Layer(vec![CueNode::Sound(SOUND_A), CueNode::Sound(SOUND_B)]),
            limit: Some(Limit { max: 2, rule: LimitRule::StopOldest }),
            ..plain_cue()
        };
        let mut system = system_with(layered);
        let mut rng = CueRng::new(1);
        let starts = |calls: &[MockCall]| {
            calls
                .iter()
                .filter_map(|call| match *call {
                    MockCall::StartVoice { slot, sound, .. } => Some((slot, sound)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        let first = starts(&system.backend_mut().take_calls());
        assert_eq!(first, [(VoiceSlot(0), SOUND_A), (VoiceSlot(1), SOUND_B)]);
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        assert_eq!(starts(&system.backend_mut().take_calls()).len(), 2, "two starts are within a limit of 2");
        assert_eq!(system.stats().voices, 4);
        // The third ends the first start, both its voices.
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        let calls = system.backend_mut().take_calls();
        assert_eq!(stopped(&calls), [(VoiceSlot(1), STOP_FADE), (VoiceSlot(0), STOP_FADE)]);
        assert_eq!(starts(&calls).len(), 2);
        let stats = system.stats();
        assert_eq!((stats.cues_played, stats.voices_stolen, stats.dropped_voice_limit), (3, 2, 0));
    }

    fn started_sounds(calls: &[MockCall]) -> Vec<AssetId> {
        calls
            .iter()
            .filter_map(|call| match *call {
                MockCall::StartVoice { sound, .. } => Some(sound),
                _ => None,
            })
            .collect()
    }

    /// M6 U4a: a start counts toward its limit while any of its voices lives, so a layer's short sound ending
    /// leaves its long one still counted, and still what a rule makes room by.
    #[test]
    fn a_start_counts_toward_its_limit_until_its_last_voice_ends() {
        let layered = |rule| Cue {
            play: CueNode::Layer(vec![CueNode::Sound(SOUND_A), CueNode::Sound(LONG)]),
            limit: Some(Limit { max: 1, rule }),
            ..plain_cue()
        };
        let mut rng = CueRng::new(1);
        for rule in [LimitRule::RefuseNew, LimitRule::StopOldest] {
            let mut system = rivals(layered(rule), plain_cue(), false);
            ask(&mut system, CUE, CueTarget::Flat, &mut rng);
            assert_eq!(started_sounds(&system.backend_mut().take_calls()), [SOUND_A, LONG]);
            // The short one plays out, and 30 frames on the long one still plays.
            system.backend_mut().finish(VoiceSlot(0));
            for _ in 0..30 {
                system.update(&frame(&[], &[]), &mut rng);
            }
            assert_eq!(system.stats().voices, 1);
            system.backend_mut().take_calls();
            ask(&mut system, CUE, CueTarget::Flat, &mut rng);
            let calls = system.backend_mut().take_calls();
            let stats = system.stats();
            if matches!(rule, LimitRule::RefuseNew) {
                assert!(started_sounds(&calls).is_empty(), "{calls:?}");
                assert_eq!((stats.cues_played, stats.dropped_voice_limit), (1, 2), "a voice a sound refused");
            } else {
                assert_eq!(stopped(&calls), [(VoiceSlot(1), STOP_FADE)]);
                assert_eq!(started_sounds(&calls), [SOUND_A, LONG]);
                assert_eq!((stats.cues_played, stats.voices_stolen), (2, 1));
            }
        }
    }

    /// M6 U4a: a blend's start is one cue played by whichever of its voices the caps let in, and its limit holds
    /// by that one when the first is ranked out.
    #[test]
    fn a_blends_start_counts_by_whichever_voice_plays() {
        use sapphire_assets::cue::{BlendNode, BlendPoint};

        let point = |at, sound| BlendPoint { at, play: CueNode::Sound(sound) };
        let blended = Cue {
            play: CueNode::Blend(BlendNode {
                on: "speed".to_owned(),
                points: vec![point(0.0, SOUND_A), point(1.0, SOUND_B)],
            }),
            limit: Some(Limit { max: 1, rule: LimitRule::RefuseNew }),
            ..plain_cue()
        };
        let mut system = system_with(blended);
        system.set_voice_caps(VoiceCaps { real: 1, virtual_voices: 0 });
        let mut rng = CueRng::new(1);
        // Near the second point the second is much the louder, so the first is the one ranked out.
        let fast =
            CueRequest::new(CUE, CueTarget::Flat).with(Name::new("speed"), ParameterValue::Number(0.9));
        system.update(&frame(&[], &[fast]), &mut rng);
        assert_eq!(started_sounds(&system.backend_mut().take_calls()), [SOUND_B]);
        let stats = system.stats();
        assert_eq!((stats.cues_played, stats.dropped_voice_limit), (1, 1));
        system.update(&frame(&[], &[fast]), &mut rng);
        assert!(started_sounds(&system.backend_mut().take_calls()).is_empty());
        let stats = system.stats();
        assert_eq!((stats.cues_played, stats.dropped_voice_limit), (1, 3));
    }

    /// M6 U4a: a layered line holds, lets go and is cut as one, every voice of its start and not its own alone.
    #[test]
    fn a_layered_lines_voices_hold_and_end_with_it() {
        let layered = Cue {
            play: CueNode::Layer(vec![CueNode::Sound(SOUND_A), CueNode::Sound(SOUND_B)]),
            bus: Bus::Voice,
            ..plain_cue()
        };
        let mut system = system_with(layered);
        let line = sapphire_assets::voice::VoiceLine { key: "greet".to_owned(), cue: CUE };
        system.add_voice_lines(&VoiceLines { locale: "en".to_owned(), lines: vec![line] }).unwrap();
        assert!(system.set_voice_language("en"));
        let mut rng = CueRng::new(1);
        say(&mut system, greet(1, false), &mut rng);
        assert_eq!(started_sounds(&system.backend_mut().take_calls()), [SOUND_A, SOUND_B]);
        for held in [true, false] {
            say(&mut system, greet(1, held), &mut rng);
            let calls = system.backend_mut().take_calls();
            for slot in [VoiceSlot(0), VoiceSlot(1)] {
                assert!(calls.contains(&MockCall::FreezeVoice { slot, frozen: held }), "{held}: {calls:?}");
            }
        }
        // The next line cuts both, and the conversation's end both of the next.
        say(&mut system, greet(2, false), &mut rng);
        let calls = system.backend_mut().take_calls();
        let mut ends: Vec<_> = stopped(&calls).into_iter().map(|(slot, _)| slot.0).collect();
        ends.sort_unstable();
        assert_eq!(ends, [0, 1]);
        assert_eq!(started_sounds(&calls), [SOUND_A, SOUND_B]);
        say(&mut system, None, &mut rng);
        let mut ends: Vec<_> =
            stopped(&system.backend_mut().take_calls()).into_iter().map(|(slot, _)| slot.0).collect();
        ends.sort_unstable();
        assert_eq!(ends, [2, 3]);
        assert_eq!(system.speaking(), None);
    }

    /// M6 U4a: a start is as loud as its loudest voice to a limit, so a blend near its second point, its quiet side
    /// first, isn't refused for that side, nor stolen for it.
    #[test]
    fn a_start_is_as_loud_as_its_loudest_voice() {
        use sapphire_assets::cue::{BlendNode, BlendPoint};

        let point = |at, sound| BlendPoint { at, play: CueNode::Sound(sound) };
        let blended = |max| Cue {
            play: CueNode::Blend(BlendNode {
                on: "speed".to_owned(),
                points: vec![point(0.0, SOUND_A), point(1.0, SOUND_B)],
            }),
            limit: Some(Limit { max, rule: LimitRule::StopQuietest }),
            ..plain_cue()
        };
        let at = |speed| {
            CueRequest::new(CUE, CueTarget::Flat).with(Name::new("speed"), ParameterValue::Number(speed))
        };
        let mut rng = CueRng::new(1);
        // Halfway each side is 3 dB down; at 0.9 the first is 16 dB down and the second 0.1.
        let mut system = system_with(blended(1));
        system.update(&frame(&[], &[at(0.5)]), &mut rng);
        system.backend_mut().take_calls();
        system.update(&frame(&[], &[at(0.9)]), &mut rng);
        let calls = system.backend_mut().take_calls();
        assert_eq!(stopped(&calls).len(), 2, "the even start gives way: {calls:?}");
        assert_eq!(started_sounds(&calls), [SOUND_A, SOUND_B]);
        // With two playing, the even one goes, not the one whose quiet side is first.
        let mut system = system_with(blended(2));
        system.update(&frame(&[], &[at(0.9)]), &mut rng);
        system.update(&frame(&[], &[at(0.5)]), &mut rng);
        system.backend_mut().take_calls();
        system.update(&frame(&[], &[at(0.9)]), &mut rng);
        let mut ends: Vec<_> =
            stopped(&system.backend_mut().take_calls()).into_iter().map(|(slot, _)| slot.0).collect();
        ends.sort_unstable();
        assert_eq!(ends, [2, 3]);
    }

    /// M6 U4a: the mixer's room takes all of a start's voices or none: with one place left among the live voices,
    /// a layer of two that outranks nothing is refused whole, and a single sound fits.
    #[test]
    fn the_mixers_room_takes_all_of_a_starts_voices_or_none() {
        const SINGLE: AssetId = AssetId::from_text("77777777-2222-4333-8444-555555555555");
        let mut system = system_with(plain_cue());
        system
            .add_sound(LONG, &DecodedSound { sample_rate: 48_000, channels: 1, samples: vec![0.5; 144_000] });
        let long = |play, priority| Cue { play, priority, ..plain_cue() };
        system.add_cue(OTHER, long(CueNode::Sound(LONG), 200));
        system.add_cue(CUE, long(CueNode::Layer(vec![CueNode::Sound(LONG), CueNode::Sound(LONG)]), 10));
        system.add_cue(SINGLE, long(CueNode::Sound(LONG), 10));
        let mut rng = CueRng::new(1);
        let live = |system: &AudioSystem<MockBackend>| {
            let stats = system.stats();
            (stats.voices + stats.virtual_voices) as usize
        };
        system.update(&frame(&[], &vec![CueRequest::new(OTHER, CueTarget::Flat); MAX_LIVE - 1]), &mut rng);
        assert_eq!(live(&system), MAX_LIVE - 1);
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        assert_eq!((live(&system), system.stats().dropped_voice_limit), (MAX_LIVE - 1, 2), "neither voice");
        ask(&mut system, SINGLE, CueTarget::Flat, &mut rng);
        assert_eq!((live(&system), system.stats().dropped_voice_limit), (MAX_LIVE, 2));
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        assert_eq!((live(&system), system.stats().dropped_voice_limit), (MAX_LIVE, 4));
    }

    /// M6 U4a: a start refused by its limit moves its sequence on by nothing, so asking every frame while one
    /// plays skips no step.
    #[test]
    fn a_refused_start_moves_no_sequence_on() {
        let stepped = Cue {
            play: CueNode::Sequence(vec![CueNode::Sound(SOUND_A), CueNode::Sound(SOUND_B)]),
            limit: Some(Limit { max: 1, rule: LimitRule::RefuseNew }),
            ..plain_cue()
        };
        let mut system = system_with(stepped);
        let mut rng = CueRng::new(1);
        let mut played = Vec::new();
        for frame_number in 0..9 {
            // The one playing ends every third frame.
            if frame_number % 3 == 2 {
                let slot = system.voices.iter().find_map(|voice| match voice.mode {
                    Mode::Real(slot) => Some(slot),
                    _ => None,
                });
                system.backend_mut().finish(slot.expect("one plays"));
            }
            ask(&mut system, CUE, CueTarget::Flat, &mut rng);
            played.extend(started_sounds(&system.backend_mut().take_calls()));
        }
        assert_eq!(played, [SOUND_A, SOUND_B, SOUND_A, SOUND_B]);
        assert_eq!(system.stats().dropped_voice_limit, 5);
    }

    /// M6 U4a: a start's tree reads the request's parameters, and the draws stay three whatever it reaches.
    #[test]
    fn a_switch_reads_the_requests_parameter_and_the_draws_stay_three() {
        use sapphire_assets::cue::{SwitchCase, SwitchNode};

        let switched = Cue {
            play: CueNode::Switch(SwitchNode {
                on: "surface".to_owned(),
                cases: vec![
                    SwitchCase { when: "stone".to_owned(), play: CueNode::Sound(SOUND_A) },
                    SwitchCase { when: "grass".to_owned(), play: CueNode::one_of(vec![SOUND_B, SOUND_A]) },
                ],
                default: "stone".to_owned(),
            }),
            ..plain_cue()
        };
        let mut system = system_with(switched);
        let mut rng = CueRng::new(3);
        let grass = CueRequest::new(CUE, CueTarget::Flat)
            .with(crate::SURFACE, ParameterValue::Case(Name::new("grass")));
        for request in [CueRequest::new(CUE, CueTarget::Flat), grass] {
            let mut copy = rng.clone();
            let (pick, ..) = (copy.next_u32(), copy.next_unit(), copy.next_unit());
            system.update(&frame(&[], &[request]), &mut rng);
            assert_eq!(rng, copy, "three numbers a start");
            let sound = system.backend_mut().take_calls().into_iter().find_map(|call| match call {
                MockCall::StartVoice { sound, .. } => Some(sound),
                _ => None,
            });
            let expected = if request == grass { [SOUND_B, SOUND_A][pick as usize % 2] } else { SOUND_A };
            assert_eq!(sound, Some(expected));
        }
    }

    /// M6 U3: a voice group's limit spans its cues, and the lowest priority goes, the newest only if it's lowest.
    #[test]
    fn a_groups_limit_spans_its_cues_and_keeps_the_higher_priority() {
        let grouped = |priority| Cue { priority, group: Some("steps".to_owned()), ..plain_cue() };
        let mut system = rivals(grouped(10), grouped(200), false);
        let mut mix = Mix::plain();
        let limit = Limit { max: 1, rule: LimitRule::StopLowestPriority };
        mix.groups.push(VoiceGroup { name: "steps".to_owned(), limit });
        system.set_mix(&mix).unwrap();
        system.backend_mut().take_calls();
        let mut rng = CueRng::new(1);
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        ask(&mut system, OTHER, CueTarget::Flat, &mut rng);
        assert_eq!(stopped(&system.backend_mut().take_calls()), [(VoiceSlot(0), STOP_FADE)]);
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        let stats = system.stats();
        assert_eq!((stats.voices_stolen, stats.dropped_voice_limit, stats.cues_played), (1, 1, 2));
        // A group the mix hasn't limits nothing.
        system.set_mix(&Mix::plain()).unwrap();
        for _ in 0..3 {
            ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        }
        assert_eq!(system.stats().cues_played, 5);
    }

    /// M6 U3: past the real cap the lowest-ranked voice goes virtual, its playhead counted on, and comes back where
    /// it would be when there's room, fading in; a short one-shot outranked ends instead.
    #[test]
    fn past_the_cap_a_voice_waits_virtual_and_comes_back_where_it_would_be() {
        let mut system =
            rivals(Cue { priority: 10, ..plain_cue() }, Cue { priority: 200, ..plain_cue() }, true);
        system.set_voice_caps(VoiceCaps { real: 1, virtual_voices: 4 });
        let mut rng = CueRng::new(1);
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        ask(&mut system, OTHER, CueTarget::Flat, &mut rng);
        let calls = system.backend_mut().take_calls();
        assert_eq!(stopped(&calls), [(VoiceSlot(0), VIRTUAL_FADE)], "{calls:?}");
        assert!(calls.iter().any(|call| matches!(call, MockCall::StartVoice { slot: VoiceSlot(1), .. })));
        let stats = system.stats();
        assert_eq!((stats.voices, stats.virtual_voices, stats.voices_virtualized), (2, 1, 1));

        // A virtual voice costs the mixer nothing while it waits.
        system.backend_mut().finish(VoiceSlot(0));
        for _ in 0..30 {
            system.update(&frame(&[], &[]), &mut rng);
        }
        let calls = system.backend_mut().take_calls();
        assert!(calls.is_empty(), "{calls:?}");
        assert_eq!(system.stats().voices, 1, "the faded slot is free");

        // The high one ends, and the low one comes back 32 frames in.
        system.backend_mut().finish(VoiceSlot(1));
        system.update(&frame(&[], &[]), &mut rng);
        let calls = system.backend_mut().take_calls();
        let [MockCall::StartVoice { slot, sound: LONG, from_seconds, fade_in, looped: false, .. }] =
            calls[..]
        else {
            panic!("{calls:?}")
        };
        assert_eq!((slot, fade_in), (VoiceSlot(1), VIRTUAL_FADE));
        assert!((from_seconds - 32.0 * f64::from(FRAME)).abs() < 1e-6, "{from_seconds}");
        let stats = system.stats();
        assert_eq!((stats.voices, stats.virtual_voices, stats.voices_revived), (1, 0, 1));

        // A one-shot too short to wait ends when it's outranked.
        let mut system =
            rivals(Cue { priority: 10, ..plain_cue() }, Cue { priority: 200, ..plain_cue() }, false);
        system.set_voice_caps(VoiceCaps { real: 1, virtual_voices: 4 });
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        ask(&mut system, OTHER, CueTarget::Flat, &mut rng);
        assert_eq!(stopped(&system.backend_mut().take_calls()), [(VoiceSlot(0), STOP_FADE)]);
        assert_eq!((system.stats().voices_stolen, system.stats().virtual_voices), (1, 0));
        // Nor does a low one asked for with no room play at all.
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        assert_eq!(system.stats().dropped_voice_limit, 1);
    }

    /// M6 U3: a real voice keeps its place against a virtual one of the same priority until the other is clearly
    /// louder, so two about as loud don't trade places frame by frame.
    #[test]
    fn a_real_voice_keeps_its_place_against_one_about_as_loud() {
        let mut system = rivals(placed_cue(), placed_cue(), true);
        system.set_voice_caps(VoiceCaps { real: 1, virtual_voices: 4 });
        let mut rng = CueRng::new(1);
        let (first, second) = (EmitterId(1), EmitterId(2));
        let at = |id, meters: f32| Emitter { id, position: Vec3::new(0.0, 0.0, -meters), hitstop: false };
        let both =
            [CueRequest::new(CUE, CueTarget::On(first)), CueRequest::new(OTHER, CueTarget::On(second))];
        system.update(&frame(&[at(first, 5.0), at(second, 6.0)], &both), &mut rng);
        system.backend_mut().take_calls();
        // 1 dB louder isn't enough; 6 dB is.
        let close = [at(first, 5.0), at(second, 4.7)];
        system.update(&frame(&close, &[]), &mut rng);
        assert!(stopped(&system.backend_mut().take_calls()).is_empty());
        let louder = [at(first, 5.0), at(second, 2.0)];
        system.update(&frame(&louder, &[]), &mut rng);
        assert_eq!(stopped(&system.backend_mut().take_calls()), [(VoiceSlot(0), VIRTUAL_FADE)]);
    }

    /// M6 U3: the conversation's line is mixed whatever else is asked for, and no limit counts it.
    #[test]
    fn the_conversations_line_outranks_everything() {
        let mut system = speaker(1.0);
        system.add_cue(OTHER, Cue { priority: u8::MAX, ..plain_cue() });
        system.set_voice_caps(VoiceCaps { real: 1, virtual_voices: 0 });
        let mut rng = CueRng::new(1);
        say(&mut system, greet(1, false), &mut rng);
        system.update(
            &AudioFrame { speech: greet(1, false), ..frame(&[], &[CueRequest::new(OTHER, CueTarget::Flat)]) },
            &mut rng,
        );
        assert!(system.speaking().is_some());
        assert_eq!((system.stats().voices, system.stats().dropped_voice_limit), (1, 1));
    }

    /// M6 U3: a loop plays on an emitter, round and round, and ends when the emitter goes.
    #[test]
    fn a_loop_plays_on_its_emitter_until_it_goes() {
        let mut system = system_with(Cue { looped: true, ..plain_cue() });
        let mut rng = CueRng::new(1);
        ask(&mut system, CUE, CueTarget::Flat, &mut rng);
        assert_eq!((system.stats().dropped_unknown, system.stats().voices), (1, 0));
        let here = [actor_at(Vec3::ZERO)];
        system.update(&frame(&here, &play(CueTarget::On(ACTOR))), &mut rng);
        let calls = system.backend_mut().take_calls();
        assert!(matches!(calls[..], [MockCall::StartVoice { looped: true, .. }]), "{calls:?}");
        system.update(&frame(&here, &[]), &mut rng);
        system.update(&frame(&[], &[]), &mut rng);
        assert_eq!(stopped(&system.backend_mut().take_calls()), [(VoiceSlot(0), STOP_FADE)]);
        assert_eq!(system.stats().voices, 1, "fading out");
    }

    /// M6 U3: the caps are held to what there's room for.
    #[test]
    fn the_caps_stay_within_the_room() {
        let mut system = system_with(plain_cue());
        assert_eq!(system.voice_caps(), VoiceCaps::MAX);
        system.set_voice_caps(VoiceCaps { real: 0, virtual_voices: 1_000 });
        assert_eq!(system.voice_caps(), VoiceCaps { real: 1, virtual_voices: MAX_VIRTUAL_VOICES as u16 });
    }

    /// M6 U5: a room 5 m ahead, of the shipped `shelter` unless named otherwise.
    fn shelter(reverb: &str) -> [crate::Room; 1] {
        [crate::Room {
            position: Vec3::new(0.0, 0.0, -5.0),
            shape: crate::RoomShape::Box { half_extents: Vec3::splat(2.0) },
            priority: 0,
            reverb: Name::new(reverb),
            outdoor: false,
            fade: 0.4,
        }]
    }

    fn at<'a>(
        listener: Vec3,
        rooms: &'a [crate::Room],
        emitters: &'a [Emitter],
        cues: &'a [CueRequest],
    ) -> AudioFrame<'a> {
        AudioFrame {
            listener: Listener { position: listener, ..Listener::default() },
            rooms: Rooms { rooms, origin: Vec3::ZERO },
            ..frame(emitters, cues)
        }
    }

    fn rooms_asked(calls: &[MockCall]) -> Vec<(Option<f32>, Duration)> {
        calls
            .iter()
            .filter_map(|call| match *call {
                MockCall::SetRoom { feedback, fade } => Some((feedback, fade)),
                _ => None,
            })
            .collect()
    }

    /// M6 U5: the listener's room plays its reverb from the frame it's entered, over its fade, once; leaving
    /// fades it out over the same.
    #[test]
    fn the_listeners_room_plays_its_reverb_and_leaving_fades_it_out() {
        let mut system = system_with(plain_cue());
        system.set_mix(&shipped()).unwrap();
        system.backend_mut().take_calls();
        let mut rng = CueRng::new(1);
        let rooms = shelter("shelter");
        let fade = Duration::from_secs_f32(0.4);
        system.update(&at(Vec3::ZERO, &rooms, &[], &[]), &mut rng);
        assert_eq!(rooms_asked(&system.backend_mut().take_calls()), []);
        for _ in 0..3 {
            system.update(&at(Vec3::new(0.5, 0.0, -5.0), &rooms, &[], &[]), &mut rng);
        }
        assert_eq!(rooms_asked(&system.backend_mut().take_calls()), [(Some(0.72), fade)]);
        system.update(&at(Vec3::ZERO, &rooms, &[], &[]), &mut rng);
        assert_eq!(rooms_asked(&system.backend_mut().take_calls()), [(None, fade)]);
        assert_eq!((system.stats().room_changes, system.stats().reverbs_unknown), (2, 0));
    }

    /// M6 U5: a room naming a reverb the mix hasn't plays none, counted once a visit.
    #[test]
    fn a_room_of_a_reverb_the_mix_hasnt_plays_none_and_is_counted() {
        let mut system = system_with(plain_cue());
        system.set_mix(&shipped()).unwrap();
        system.backend_mut().take_calls();
        let mut rng = CueRng::new(1);
        let rooms = shelter("cellar");
        for _ in 0..3 {
            system.update(&at(Vec3::new(0.0, 0.0, -5.0), &rooms, &[], &[]), &mut rng);
        }
        assert_eq!(rooms_asked(&system.backend_mut().take_calls()), []);
        assert_eq!((system.stats().room_changes, system.stats().reverbs_unknown), (0, 1));
    }

    /// M6 U5: a voice reaches the room's reverb by its bus's level, and only from the listener's space; with no
    /// room playing, none does.
    #[test]
    fn a_voice_reaches_the_reverb_from_the_listeners_space_by_its_bus() {
        let mut system = system_with(placed_cue());
        system.set_mix(&shipped()).unwrap();
        let mut rng = CueRng::new(1);
        let rooms = shelter("shelter");
        let mut send = |listener: Vec3, actor: Vec3| {
            let here = [actor_at(actor)];
            system.backend_mut().take_calls();
            system.update(&at(listener, &rooms, &here, &play(CueTarget::On(ACTOR))), &mut rng);
            let send = started(&mut system).send_db;
            system.stop_voices();
            send
        };
        let (room, inside) = (Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, -4.0));
        let sfx = shipped().room_db(Bus::Sfx.index()).unwrap();
        assert!((send(room, inside) - sfx).abs() < 0.01);
        assert_eq!(send(room, Vec3::new(0.0, 0.0, 3.0)), SILENCE_DB, "from outdoors");
        assert_eq!(send(Vec3::ZERO, inside), SILENCE_DB, "no room plays outdoors");
    }

    /// M6 U5: the tier's reverb count reaches the backend once a change, held to one or two.
    #[test]
    fn the_reverb_slots_reach_the_backend_once_a_change() {
        let mut system = system_with(plain_cue());
        system.set_reverb_slots(0);
        system.set_reverb_slots(1);
        system.set_reverb_slots(5);
        let calls = system.backend_mut().take_calls();
        assert_eq!(calls, [MockCall::SetReverbSlots(1), MockCall::SetReverbSlots(2)]);
    }
}
