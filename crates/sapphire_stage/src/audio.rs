//! The real audio device: the sandbox's `--cue <name>` plays a cue at the character (M2 E1, spec §5.15), and
//! a fight plays what its table asks (M2 E3, `game_combat`'s `cues`).
//!
//! A [`DeviceAudio`] is made only when an app asks. Without one, the app mounts no audio pack, starts no audio
//! thread and opens no device. The offscreen modes refuse `--cue` and never make one for `--cues`, and
//! [`stayed_shut`] holds capture and bench to it at the end of every run, golden scenes included: CI's
//! runners have no sound. A bench of a fight with `--cues` measures the real mixer through
//! [`OfflineAudio`], which has no device to open.
//!
//! A device that isn't there or goes away is a logged state of the mixer, never a failure, and so is
//! an OS that won't set up its audio session: the app runs on in silence. A cue that can't load is
//! the run's failure: it was asked for by name.
//!
//! Slow motion (`Space`) bends and darkens what's playing, and hitstop (`H`) freezes `--cue swing`,
//! both through the scene's own clocks ([`Scene::sound`](crate::scene::Scene::sound)). A sound
//! recooked while the app runs isn't picked up: the mixer holds what was decoded at startup.
//!
//! A fight's table can name a score and voice lines (M3 A1, [`ShowSounds`]): the score follows the
//! fight's phase ([`CueSounds::music_phase`]), or a boss's named phase where the score has one of that name
//! (M3 Z5a, [`CueSounds::boss_phase`]), and the lines are spoken in the voice language
//! ([`DeviceAudio::set_voice_language`]), which starts as `LocaleSettings`' default and is the
//! player's to change, apart from the text's.
//!
//! A conversation's line with a voice (M5 C6) comes as [`CueSounds::speech`]: the mixer starts it, holds it
//! through a pause and cuts it at the next line, and [`DeviceAudio::said`] reads the sound's cooked mouth
//! curve at the voice's playhead, which the mixer's caller counts in real seconds. With no device there's no
//! voice, and so no mouth: a face never moves to a line nobody can hear played.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::Cue;
use sapphire_assets::mouth::{MouthCurve, MouthSample};
use sapphire_assets::score::Score;
use sapphire_assets::sound::{DecodedSound, Sound};
use sapphire_assets::voice::VoiceLines;
pub use sapphire_audio::Volumes;
use sapphire_audio::{
    AudioBackend, AudioFrame, AudioStats, AudioSystem, CpalOutput, CueRequest, CueRng, CueTarget, Emitter,
    EmitterId, KiraBackend, LineRequest, Listener, MusicRequest, OfflineOutput, SpeechRequest,
};
use sapphire_core::Name;
use sapphire_loc::LocaleSettings;
use sapphire_math::Vec3;
use sapphire_platform::{AppInitError, Platform};
use sapphire_render::Renderer;
use sapphire_rhi::GpuContext;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::content::Content;

/// The cues `--cue` can name, by their sidecars' IDs in `content/audio/`. A game names cues in its
/// own data; a test holds this table to the sidecars.
const CUES: [(&str, AssetId); 3] = [
    ("click", AssetId::from_text("8aaaae94-35ae-4195-a7d4-f5d648ff401c")),
    ("hit", AssetId::from_text("16e787b3-260f-4873-a443-40c8c52ef5c2")),
    ("swing", AssetId::from_text("e595fa42-bdd3-4e6e-845c-52684b76f8ed")),
];

/// What a fight or whatever else fires sounds asks of the audio system this frame.
#[derive(Clone, Copy, Debug)]
pub struct CueSounds<'a> {
    pub requests: &'a [CueRequest],
    /// Voice lines, by key, spoken in the voice language.
    pub lines: &'a [LineRequest],
    /// The line a conversation says now, if it has a voice (M5 C6).
    pub speech: Option<SpeechRequest>,
    pub emitters: &'a [Emitter],
    /// The world clock's speed where they fire, which a slow motion turns down.
    pub world_time_scale: f32,
    /// A new fight began this frame, so what the last one left playing ends before these start.
    pub started_over: bool,
    /// The phase the fight's score plays, by name: none, and the music fades out.
    pub music_phase: Option<&'a str>,
    /// The named phase of a boss in the fight (M3 Z5a), which the score plays in place of [`RUNNING`] if it has
    /// a phase of that name. With any other `music_phase`, a game flow's `waiting` or `cleared` among them, it
    /// isn't heard.
    pub boss_phase: Option<Name>,
}

impl CueSounds<'_> {
    /// Nothing: there's no fight.
    pub const NONE: CueSounds<'static> = CueSounds {
        requests: &[],
        lines: &[],
        speech: None,
        emitters: &[],
        world_time_scale: 1.0,
        started_over: false,
        music_phase: None,
        boss_phase: None,
    };
}

/// The phase a fight's score plays while any of its encounters runs: the one a boss's phase can stand in for.
pub const RUNNING: &str = "running";

/// What an app loads for a fight's sound: the cues its table plays, and the score and voice lines it
/// names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShowSounds {
    pub cues: Vec<AssetId>,
    pub score: Option<AssetId>,
    /// One `VoiceLines` a language.
    pub voices: Vec<AssetId>,
}

/// What a conversation's line says this frame (M5 C6): its sound's mouth curve at the voice's playhead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Said {
    /// The [`SpeechRequest::serial`] that asked for the line.
    pub serial: u64,
    pub mouth: MouthSample,
}

/// The mouth curves of the sounds a show's voice lines play, by sound: load-time data a frame only reads.
#[derive(Debug, Default)]
struct Mouths(HashMap<AssetId, MouthCurve>);

impl Mouths {
    /// What `system`'s conversation line says now, if it's saying one. Allocates nothing.
    fn said<B: AudioBackend>(&self, system: &AudioSystem<B>) -> Option<Said> {
        let speaking = system.speaking()?;
        let curve = self.0.get(&speaking.sound)?;
        Some(Said { serial: speaking.serial, mouth: curve.at(speaking.seconds) })
    }
}

/// The emitter cues play on: the scene's character.
const CHARACTER: EmitterId = EmitterId(0);

/// Seconds between stats lines.
const REPORT_EVERY: f32 = 2.0;

/// The picks' seed: every run plays the same sequence, which is all a test bed wants.
const CUE_SEED: u64 = 0x5EED;

/// `--cue <name> [--cue-repeat <seconds>]`.
#[derive(Clone, Debug, PartialEq)]
pub struct CueOption {
    pub id: AssetId,
    /// Seconds between plays; once, without it.
    pub repeat: Option<f32>,
}

/// The cue `--cue` named, or an error listing the names it knows.
pub fn named_cue(name: &str) -> Result<AssetId, String> {
    let found = CUES.iter().find(|(known, _)| *known == name);
    found.map(|&(_, id)| id).ok_or_else(|| {
        let names = CUES.map(|(known, _)| known).join(", ");
        format!("--cue expects one of {names}, got {name:?}")
    })
}

/// What the audio system hears of the scene this frame.
#[derive(Clone, Copy, Debug)]
pub struct SceneSound {
    pub(crate) listener: Listener,
    /// The world clock's speed.
    pub(crate) world_time_scale: f32,
    /// Where the character stands.
    pub(crate) character: Vec3,
    /// Whether the character's clock is held by hitstop.
    pub(crate) hitstop: bool,
}

/// `--cue`'s one cue, and when it's due.
struct Repeating {
    cue: AssetId,
    repeat: Option<f32>,
    /// Seconds until the next play; none once a cue that doesn't repeat has played.
    next_play: Option<f32>,
}

/// The audio system on the real device, playing `--cue`'s one cue or a fight's: what a window hears.
pub struct DeviceAudio {
    system: AudioSystem<KiraBackend<CpalOutput>>,
    random: CueRng,
    /// The score a fight's phase plays.
    score: Option<AssetId>,
    /// `--cue`'s.
    repeating: Option<Repeating>,
    /// This frame's, the character's and a fight's.
    emitters: Vec<Emitter>,
    /// Seconds until the next stats line.
    report_in: f32,
    mouths: Mouths,
}

impl DeviceAudio {
    /// `--cue`'s cue on the device. `None`, logged, if the OS won't set up audio.
    pub fn new(
        option: &CueOption,
        platform: &mut Platform,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Option<Self>, AppInitError> {
        let sounds = ShowSounds { cues: vec![option.id], ..ShowSounds::default() };
        let audio = Self::for_show(&sounds, platform, content, renderer, gpu)?;
        Ok(audio.map(|audio| Self {
            repeating: Some(Repeating { cue: option.id, repeat: option.repeat, next_play: Some(0.0) }),
            ..audio
        }))
    }

    /// Loads `sounds` and opens the device, with nothing due to play. `None`, logged, if the OS won't
    /// set up audio. A window's game loads them off the frame instead ([`SoundsLoad`]).
    pub fn for_show(
        sounds: &ShowSounds,
        platform: &mut Platform,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Option<Self>, AppInitError> {
        let loaded = load(sounds, content, renderer, gpu)?;
        Self::open(ReadySounds { loaded, score: sounds.score }, platform)
    }

    /// Opens the device for sounds a [`SoundsLoad`] has made ready, with nothing due to play. `None`, logged,
    /// if the OS won't set up audio.
    pub fn open(ready: ReadySounds, platform: &mut Platform) -> Result<Option<Self>, AppInitError> {
        Self::prepare(platform).map(|prepared| Self::open_prepared(ready, prepared)).transpose()
    }

    /// Tells the OS the app plays sound ([`Platform::prepare_audio`]), on the platform's thread, the one it may
    /// be told on: what opening a device needs first, anywhere ([`Self::open_prepared`]). `None`, logged, if the
    /// OS won't set up audio.
    pub fn prepare(platform: &mut Platform) -> Option<AudioPrepared> {
        match platform.prepare_audio() {
            Ok(()) => Some(AudioPrepared(())),
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "no audio this run");
                None
            }
        }
    }

    /// Opens the device for `ready`, on any thread, once [`Self::prepare`] has: the default output found, the
    /// mixer made and the sounds handed to it, which a [`DeviceOpening`] does on the warm-up pool.
    pub fn open_prepared(ready: ReadySounds, _: AudioPrepared) -> Result<Self, AppInitError> {
        let ReadySounds { loaded, score } = ready;
        let Ok(backend) = KiraBackend::<CpalOutput>::new(());
        let mut system = AudioSystem::new(backend);
        let mouths = add_loaded(&mut system, loaded)?;
        Ok(Self {
            system,
            random: CueRng::new(CUE_SEED),
            score,
            repeating: None,
            emitters: Vec::with_capacity(8),
            report_in: REPORT_EVERY,
            mouths,
        })
    }

    /// What the conversation's line says as of the last [`Self::update`] (M5 C6): its sound's mouth curve at
    /// its voice's playhead, until the voice ends or is cut. Allocates nothing.
    #[must_use]
    pub fn said(&self) -> Option<Said> {
        self.mouths.said(&self.system)
    }

    /// One frame, `dt` real seconds after the last: plays `--cue`'s cue when it's due and what a fight
    /// asks, and follows the OS's interruptions and the scene.
    pub fn update(&mut self, dt: f32, platform: &Platform, scene: SceneSound, fight: CueSounds<'_>) {
        self.system.set_interrupted(platform.audio_interrupted());
        if fight.started_over {
            self.system.stop_voices();
        }
        let mut due = None;
        if let Some(repeating) = &mut self.repeating {
            let is_due = repeating.next_play.as_mut().is_some_and(|left| {
                *left -= dt;
                *left <= 0.0
            });
            if is_due {
                repeating.next_play = repeating.repeat;
                due = Some(repeating.cue);
            }
        }
        let requests = due.map(|cue| [CueRequest { cue, target: CueTarget::On(CHARACTER) }]);
        self.emitters.clear();
        self.emitters.push(Emitter { id: CHARACTER, position: scene.character, hitstop: scene.hitstop });
        self.emitters.extend_from_slice(fight.emitters);
        let frame = AudioFrame {
            dt,
            listener: scene.listener,
            world_time_scale: scene.world_time_scale.min(fight.world_time_scale),
            emitters: &self.emitters,
            cues: requests.as_ref().map_or(fight.requests, |requests| requests.as_slice()),
            lines: fight.lines,
            speech: fight.speech,
            music: music(self.score, &fight),
        };
        self.system.update(&frame, &mut self.random);
        self.report_in -= dt;
        if self.report_in <= 0.0 {
            self.report_in = REPORT_EVERY;
            self.report();
        }
    }

    /// The player's volume, a share of full.
    pub fn set_volume(&mut self, volume: f32) {
        self.system.set_volume(volume);
    }

    /// The player's volumes (M4 A2): every sound's, and each kind's under it.
    pub fn set_volumes(&mut self, volumes: Volumes) {
        self.system.set_volumes(volumes);
    }

    /// Whether every sound plays in one channel (M4 A2).
    pub fn set_mono(&mut self, mono: bool) {
        self.system.set_mono(mono);
    }

    /// Chooses the language voice lines are spoken in, a BCP 47 tag, apart from the text's: whether the
    /// fight's lines are in it.
    pub fn set_voice_language(&mut self, locale: &str) -> bool {
        self.system.set_voice_language(locale)
    }

    /// The app went to the background, or came back, where it asks the OS for audio again first.
    pub fn set_background(&mut self, background: bool, platform: &mut Platform) {
        if !background {
            platform.activate_audio();
            self.system.set_interrupted(platform.audio_interrupted());
        }
        self.system.set_background(background);
    }

    /// The counters, for the overlay.
    #[cfg(feature = "dev")]
    pub fn stats(&self) -> AudioStats {
        self.system.stats()
    }

    /// The stats line, which is all a phone with no one listening can say about its sound.
    fn report(&mut self) {
        let stats = self.system.stats();
        let peak = self.system.take_peak();
        let device = stats.device;
        tracing::info!(
            target: LOG_TARGET,
            state = %device.state,
            sample_rate = device.sample_rate,
            channels = device.channels,
            callbacks = device.callbacks,
            frames = device.frames,
            xruns = device.xruns,
            streams = device.streams_opened,
            low_latency_refused = device.low_latency_refused,
            voices = stats.voices,
            played = stats.cues_played,
            dropped_no_output = stats.dropped_no_output,
            dropped_voice_limit = stats.dropped_voice_limit,
            dropped_unknown = stats.dropped_unknown,
            lines = stats.lines_asked,
            lines_missing = stats.lines_missing,
            phase_changes = stats.music_phase_changes,
            update_us = stats.update.as_micros() as u64,
            update_peak_us = peak.as_micros() as u64,
            mix_load = format_args!("{:.4}", device.mix_load()),
            mix_max_us = device.mix_nanos_max / 1_000,
            "audio stats"
        );
    }
}

/// The music `score` plays for `fight`, if there's both a score and a phase: a boss's named phase in place of
/// [`RUNNING`] only.
fn music<'a>(score: Option<AssetId>, fight: &CueSounds<'a>) -> Option<MusicRequest<'a>> {
    let phase = fight.music_phase?;
    let instead = fight.boss_phase.filter(|_| phase == RUNNING);
    Some(MusicRequest { score: score?, phase, instead })
}

/// A show's sounds on their way to a [`DeviceAudio`] (M4 H4): read out of their pack on the frame that asks,
/// a few milliseconds, and decoded on the warm-up pool, the 50 ms that was once the window's first title frame.
pub struct SoundsLoad {
    score: Option<AssetId>,
    decoding: mpsc::Receiver<Result<Loaded, String>>,
}

impl SoundsLoad {
    /// Reads `sounds`, the voice lines' cues with the table's, and starts decoding them off the frame.
    pub fn begin(
        sounds: &ShowSounds,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Self, AppInitError> {
        let read = read(sounds, content, renderer, gpu)?;
        let (sender, decoding) = mpsc::channel();
        sapphire_jobs::spawn_warm_up(move || {
            let _ = sender.send(read.decode());
        });
        Ok(Self { score: sounds.score, decoding })
    }

    /// The sounds once they're decoded, without waiting: `None` while they aren't, and then once.
    pub fn take(&mut self) -> Option<Result<ReadySounds, AppInitError>> {
        let loaded = match self.decoding.try_recv() {
            Ok(loaded) => loaded.map_err(AppInitError::msg),
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(AppInitError::msg("decoding the sounds panicked")),
        };
        Some(loaded.map(|loaded| ReadySounds { loaded, score: self.score }))
    }
}

/// That the OS has been told the app plays sound ([`DeviceAudio::prepare`]), which a device opened anywhere needs.
#[derive(Clone, Copy, Debug)]
pub struct AudioPrepared(());

/// A show's sounds on their way to the device (M4 H8): read on the frame that asks, as a [`SoundsLoad`] reads
/// them, then decoded and the device opened for them on the warm-up pool. Finding the default output and making
/// the mixer take tens of milliseconds on some OSes, which were a title frame's.
pub struct DeviceOpening {
    opening: mpsc::Receiver<Result<DeviceAudio, String>>,
}

impl DeviceOpening {
    /// Reads `sounds`, and starts decoding them and opening the device for them off the frame.
    pub fn begin(
        sounds: &ShowSounds,
        prepared: AudioPrepared,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Self, AppInitError> {
        let read = read(sounds, content, renderer, gpu)?;
        let score = sounds.score;
        let (sender, opening) = mpsc::channel();
        sapphire_jobs::spawn_warm_up(move || {
            let opened = read.decode().and_then(|loaded| {
                let stopwatch = Stopwatch::start();
                let audio = DeviceAudio::open_prepared(ReadySounds { loaded, score }, prepared);
                let audio = audio.map_err(|error| error.to_string())?;
                let open = stopwatch.elapsed();
                tracing::info!(target: LOG_TARGET, ?open, "audio set up off the frame: its device asked for and its mixer made");
                Ok(audio)
            });
            let _ = sender.send(opened);
        });
        Ok(Self { opening })
    }

    /// The device once it's open, without waiting: `None` while it isn't, and then once.
    pub fn take(&mut self) -> Option<Result<DeviceAudio, AppInitError>> {
        match self.opening.try_recv() {
            Ok(opened) => Some(opened.map_err(AppInitError::msg)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err(AppInitError::msg("opening the audio device panicked")))
            }
        }
    }
}

/// A show's sounds, decoded, for [`DeviceAudio::open`].
pub struct ReadySounds {
    loaded: Loaded,
    score: Option<AssetId>,
}

impl ReadySounds {
    /// How many sounds there are, decoded.
    #[must_use]
    pub fn sounds(&self) -> usize {
        let cues = self.loaded.cues.iter().map(|(_, _, sounds)| sounds.len());
        cues.sum::<usize>() + self.loaded.score.as_ref().map_or(0, |(_, _, stems)| stems.len())
    }
}

/// Loads `sounds` and decodes theirs, the voice lines' cues with the table's: what a mixer holds from
/// then on.
fn load(
    sounds: &ShowSounds,
    content: &mut Content,
    renderer: &mut Renderer,
    gpu: &GpuContext,
) -> Result<Loaded, AppInitError> {
    read(sounds, content, renderer, gpu)?.decode().map_err(AppInitError::msg)
}

/// [`load`]'s first half: `sounds` read, and a copy of each of theirs to decode.
fn read(
    sounds: &ShowSounds,
    content: &mut Content,
    renderer: &mut Renderer,
    gpu: &GpuContext,
) -> Result<Read, AppInitError> {
    let stopwatch = Stopwatch::start();
    let mut read = HashSet::new();
    let voices = sounds
        .voices
        .iter()
        .map(|&id| content.load_voice_lines(id, renderer, gpu))
        .collect::<Result<Vec<_>, _>>()?;
    let mut cues = sounds.cues.clone();
    cues.extend(voices.iter().flat_map(|lines| lines.lines.iter().map(|line| line.cue)));
    cues.sort_unstable();
    cues.dedup();
    let cues = cues
        .iter()
        .map(|&id| content.load_cue(id, &mut read, renderer, gpu).map(|(cue, sounds)| (id, cue, sounds)))
        .collect::<Result<Vec<_>, _>>()?;
    let score = match sounds.score {
        Some(id) => {
            Some(content.load_score(id, &mut read, renderer, gpu).map(|(score, stems)| (id, score, stems))?)
        }
        None => None,
    };
    Ok(Read { cues, score, voices, took: stopwatch.elapsed() })
}

/// What [`read`] read: the cues, the score and the voice lines, each sound still to decode.
struct Read {
    cues: Vec<ReadCue>,
    score: Option<ReadScore>,
    voices: Vec<VoiceLines>,
    /// How long reading them took.
    took: Duration,
}

impl Read {
    /// Each sound decoded: [`load`]'s second half, wherever it runs.
    fn decode(self) -> Result<Loaded, String> {
        let stopwatch = Stopwatch::start();
        let decode = |sounds: Vec<(AssetId, Sound)>| {
            sounds
                .into_iter()
                .map(|(id, sound)| {
                    sound
                        .decode()
                        .map(|decoded| (id, decoded))
                        .map_err(|error| format!("the sound {id}: {error}"))
                })
                .collect::<Result<Vec<_>, String>>()
        };
        let mut mouths = Mouths::default();
        let cues = self
            .cues
            .into_iter()
            .map(|(id, cue, mut sounds)| {
                mouths.0.extend(sounds.iter_mut().filter_map(|(id, sound)| Some((*id, sound.mouth.take()?))));
                Ok((id, cue, decode(sounds)?))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let score =
            self.score.map(|(id, score, stems)| Ok::<_, String>((id, score, decode(stems)?))).transpose()?;
        let all =
            cues.iter().flat_map(|(_, _, sounds)| sounds).chain(score.iter().flat_map(|(_, _, stems)| stems));
        let (count, frames) =
            all.fold((0, 0), |(count, frames), (_, sound)| (count + 1, frames + sound.frames()));
        let (read, decode) = (self.took, stopwatch.elapsed());
        tracing::info!(
            target: LOG_TARGET,
            cues = cues.len(),
            score = score.is_some(),
            languages = self.voices.len(),
            sounds = count,
            frames,
            ?read,
            ?decode,
            "sounds ready"
        );
        Ok(Loaded { cues, score, voices: self.voices, mouths })
    }
}

/// Gives `system` what [`load`] loaded, and hands back the mouth curves, which a frame reads beside it.
fn add_loaded<B: AudioBackend>(system: &mut AudioSystem<B>, loaded: Loaded) -> Result<Mouths, AppInitError> {
    for (id, cue, sounds) in loaded.cues {
        system.add_cue(id, cue);
        for (id, sound) in &sounds {
            system.add_sound(*id, sound);
        }
    }
    if let Some((id, score, stems)) = loaded.score {
        for (id, stem) in &stems {
            system.add_sound(*id, stem);
        }
        system.add_score(id, score);
    }
    for lines in &loaded.voices {
        system.add_voice_lines(lines).map_err(|error| AppInitError::msg(format!("voice lines: {error}")))?;
    }
    system.set_voice_language(&LocaleSettings::default().voice);
    Ok(loaded.mouths)
}

/// What [`load`] loaded.
struct Loaded {
    cues: Vec<LoadedCue>,
    score: Option<LoadedScore>,
    voices: Vec<VoiceLines>,
    /// The voice sounds' mouth curves, out of their sounds.
    mouths: Mouths,
}

/// A cue and those of its sounds no cue before it brought, decoded.
type LoadedCue = (AssetId, Cue, Vec<(AssetId, DecodedSound)>);

/// A score and its stems, decoded.
type LoadedScore = (AssetId, Score, Vec<(AssetId, DecodedSound)>);

/// A cue and those of its sounds no cue before it brought, read to decode.
type ReadCue = (AssetId, Cue, Vec<(AssetId, Sound)>);

/// A score and its stems, read to decode.
type ReadScore = (AssetId, Score, Vec<(AssetId, Sound)>);

/// The rate [`OfflineAudio`] mixes at: a phone's and most devices' own.
const OFFLINE_RATE: u32 = 48_000;

/// A fight's sound through the real mixer and no device (`--bench` with `--cues`, M2 Z1): the update a
/// frame pays, and the mix a phone pays on its callback, each measured where it happens, in a run that
/// must open no device.
pub struct OfflineAudio {
    system: AudioSystem<KiraBackend<OfflineOutput>>,
    random: CueRng,
    score: Option<AssetId>,
    emitters: Vec<Emitter>,
    /// A frame's samples, interleaved, which nothing reads.
    block: Vec<f32>,
    mouths: Mouths,
}

impl OfflineAudio {
    /// Loads `sounds`, as the windowed app does for a fight.
    pub fn for_show(
        sounds: &ShowSounds,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Self, AppInitError> {
        Self::with_loaded(load(sounds, content, renderer, gpu)?, sounds.score)
    }

    fn with_loaded(loaded: Loaded, score: Option<AssetId>) -> Result<Self, AppInitError> {
        let Ok(backend) = KiraBackend::<OfflineOutput>::new(OFFLINE_RATE);
        let mut system = AudioSystem::new(backend);
        let mouths = add_loaded(&mut system, loaded)?;
        Ok(Self {
            system,
            random: CueRng::new(CUE_SEED),
            score,
            emitters: Vec::with_capacity(8),
            block: vec![0.0; OFFLINE_RATE as usize],
            mouths,
        })
    }

    /// As [`DeviceAudio::said`].
    #[must_use]
    pub fn said(&self) -> Option<Said> {
        self.mouths.said(&self.system)
    }

    /// One frame, `dt` seconds after the last, as [`DeviceAudio::update`] plays a fight's.
    pub fn update(&mut self, dt: f32, scene: SceneSound, fight: CueSounds<'_>) {
        if fight.started_over {
            self.system.stop_voices();
        }
        self.emitters.clear();
        self.emitters.push(Emitter { id: CHARACTER, position: scene.character, hitstop: scene.hitstop });
        self.emitters.extend_from_slice(fight.emitters);
        let frame = AudioFrame {
            dt,
            listener: scene.listener,
            world_time_scale: scene.world_time_scale.min(fight.world_time_scale),
            emitters: &self.emitters,
            cues: fight.requests,
            lines: fight.lines,
            speech: fight.speech,
            music: music(self.score, &fight),
        };
        self.system.update(&frame, &mut self.random);
    }

    /// As [`DeviceAudio::set_voice_language`].
    pub fn set_voice_language(&mut self, locale: &str) -> bool {
        self.system.set_voice_language(locale)
    }

    /// As [`DeviceAudio::set_volumes`].
    pub fn set_volumes(&mut self, volumes: Volumes) {
        self.system.set_volumes(volumes);
    }

    /// As [`DeviceAudio::set_mono`].
    pub fn set_mono(&mut self, mono: bool) {
        self.system.set_mono(mono);
    }

    /// Mixes `dt` seconds of sound, and says how long that took.
    pub fn mix(&mut self, dt: f32) -> Duration {
        let samples = ((dt * OFFLINE_RATE as f32) as usize * 2).min(self.block.len());
        let stopwatch = Stopwatch::start();
        self.system.backend_mut().output_mut().render_into(&mut self.block[..samples]);
        stopwatch.elapsed()
    }

    pub fn stats(&self) -> AudioStats {
        self.system.stats()
    }
}

/// An error unless a run that must open no audio device, having asked `opens` times, opened none.
pub fn stayed_shut(opens: u64) -> Result<(), String> {
    if opens == 0 {
        return Ok(());
    }
    Err(format!("this run opened an audio device {opens} time(s), which offscreen runs must never do"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cue_is_named_or_the_error_lists_the_names() {
        assert_eq!(named_cue("swing"), Ok(CUES[2].1));
        let error = named_cue("hti").unwrap_err();
        for name in ["hti", "click", "hit", "swing"] {
            assert!(error.contains(name), "{error}");
        }
    }

    #[test]
    fn the_cue_table_matches_the_documents_sidecars() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/audio");
        for (name, id) in CUES {
            let sidecar = folder.join(format!("{name}.cue.ron.meta"));
            let text = std::fs::read_to_string(&sidecar).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(text.contains(&id.to_string()), "{name}'s sidecar has another ID:\n{text}");
        }
    }

    /// A boss's phase stands in only for `running`, so a game's flow saying `waiting` over a defeat, with the
    /// boss still up in its second phase, is heard as the flow says.
    #[test]
    fn a_boss_phase_stands_in_for_running_alone() {
        const SCORE: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000f0");
        let tolling = Name::new("tolling");
        let fight =
            |phase| CueSounds { music_phase: Some(phase), boss_phase: Some(tolling), ..CueSounds::NONE };
        let running = fight(RUNNING);
        assert_eq!(music(Some(SCORE), &running).and_then(|music| music.instead), Some(tolling));
        let waiting = fight("waiting");
        assert_eq!(
            music(Some(SCORE), &waiting).map(|music| (music.phase, music.instead)),
            Some(("waiting", None))
        );
        assert_eq!(music(None, &running), None, "no score, no music");
    }

    #[test]
    fn an_opened_device_fails_an_offscreen_run() {
        assert_eq!(stayed_shut(0), Ok(()));
        assert!(stayed_shut(1).unwrap_err().contains("audio device"));
    }

    /// A game's settings, as far as languages go.
    #[derive(Debug, Default, PartialEq, bevy_reflect::TypePath, serde::Serialize, serde::Deserialize)]
    #[type_path = "sapphire::stage_test"]
    #[serde(default)]
    struct Settings {
        locale: LocaleSettings,
    }

    impl sapphire_reflect::Versioned for Settings {
        const VERSION: u32 = 1;
    }

    #[test]
    fn the_voice_language_survives_a_save_apart_from_the_texts() {
        use sapphire_assets::cue::{Bus, HitstopResponse};
        use sapphire_assets::voice::VoiceLine;
        use sapphire_audio::{LineKey, MockBackend, MockCall};
        use sapphire_save::{SettingsFile, SettingsOutcome};

        let dir = std::env::temp_dir().join(format!("sapphire_stage_voice_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = SettingsFile::<Settings>::new(&dir, "settings", sapphire_platform::durable_replace);
        let chosen = Settings { locale: LocaleSettings { text: "en".to_owned(), voice: "ja".to_owned() } };
        file.write(&file.snapshot(&chosen).unwrap()).unwrap();
        let loaded = file.load();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(loaded.outcome, SettingsOutcome::Read), "{:?}", loaded.outcome);
        assert_eq!(loaded.value, chosen, "English menus over Japanese voices, as saved");

        // A fight with the line in both languages, each its own cue and sound.
        let ids = ["5c0e0000-0000-4000-8000-0000000000e0", "5c0e0000-0000-4000-8000-0000000000e1"];
        let [english, japanese] = ids.map(AssetId::from_text);
        let sound = DecodedSound { sample_rate: 48_000, channels: 1, samples: vec![0.25; 480] };
        let line = |id| {
            let cue = Cue {
                sounds: vec![id],
                volume_db: [0.0, 0.0],
                pitch: [1.0, 1.0],
                attenuation: None,
                bus: Bus::Voice,
                hitstop: HitstopResponse::Ignore,
            };
            (id, cue, vec![(id, sound.clone())])
        };
        let voices = [("en", english), ("ja", japanese)].map(|(locale, cue)| VoiceLines {
            locale: locale.to_owned(),
            lines: vec![VoiceLine { key: "perfect-dodge".to_owned(), cue }],
        });
        let mut system = AudioSystem::new(MockBackend::new());
        let fight = Loaded {
            cues: vec![line(english), line(japanese)],
            score: None,
            voices: voices.to_vec(),
            mouths: Mouths::default(),
        };
        add_loaded(&mut system, fight).unwrap();
        assert_eq!(system.voice_route().language(), Some("en"), "the default until the player's is read");

        assert!(system.set_voice_language(&loaded.value.locale.voice));
        let dodge = [LineRequest { line: LineKey::of("perfect-dodge"), target: CueTarget::Flat }];
        let frame = AudioFrame {
            dt: 1.0 / 60.0,
            listener: Listener::default(),
            world_time_scale: 1.0,
            emitters: &[],
            cues: &[],
            lines: &dodge,
            speech: None,
            music: None,
        };
        system.backend_mut().take_calls();
        system.update(&frame, &mut CueRng::new(1));
        let spoken = system.backend().calls().iter().find_map(|call| match *call {
            MockCall::StartVoice { sound, .. } => Some(sound),
            _ => None,
        });
        assert_eq!(spoken, Some(japanese), "the line is in the voice language, not the text's");
        assert_eq!(loaded.value.locale.text, "en");
    }

    /// M5 C6, through the real mixer with no device: a conversation's voiced line, the committed `guide_greet`
    /// with its cooked curve, sounds from the frame that asks for it, its mouth is read at the voice's own
    /// playhead, and the curve's loudest moment is the mix's within a frame. A pause holds both.
    #[test]
    fn a_conversations_voice_plays_where_its_line_says_and_its_mouth_keeps_time() {
        use sapphire_assets::cue::{Bus, HitstopResponse};
        use sapphire_assets::voice::VoiceLine;
        use sapphire_audio::LineKey;

        const DT: f32 = 1.0 / 60.0;
        const ASKED: usize = 10;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/audio/generated/voice/en/guide_greet.ogg");
        let sound = Sound::from_vorbis(std::fs::read(path).unwrap()).unwrap().with_mouth().unwrap();
        let curve = sound.mouth.clone().unwrap();
        let [sound_id, cue_id] =
            ["5c0e0000-0000-4000-8000-0000000000c6", "5c0e0000-0000-4000-8000-0000000000c7"]
                .map(AssetId::from_text);
        let cue = Cue {
            sounds: vec![sound_id],
            volume_db: [-4.0, -4.0],
            pitch: [1.0, 1.0],
            attenuation: None,
            bus: Bus::Voice,
            hitstop: HitstopResponse::Ignore,
        };
        let decoded = sound.decode().unwrap();
        let length = curve.frames.len();
        let scene = SceneSound {
            listener: Listener::default(),
            world_time_scale: 1.0,
            character: Vec3::ZERO,
            hitstop: false,
        };
        // Each frame: what the mouth read after the update, and the mix's power over the frame after it, for a
        // line asked for from `ASKED` on and held over `held`.
        let play = |held: std::ops::Range<usize>| {
            let loaded = Loaded {
                cues: vec![(cue_id, cue.clone(), vec![(sound_id, decoded.clone())])],
                score: None,
                voices: vec![VoiceLines {
                    locale: "en".to_owned(),
                    lines: vec![VoiceLine { key: "guide_greet".to_owned(), cue: cue_id }],
                }],
                mouths: Mouths(HashMap::from([(sound_id, curve.clone())])),
            };
            let mut audio = OfflineAudio::with_loaded(loaded, None).unwrap();
            let mut heard = Vec::new();
            for frame in 0..ASKED + length + held.len() + 10 {
                let speech = (frame >= ASKED).then(|| SpeechRequest {
                    line: LineKey::of("guide_greet"),
                    serial: 1,
                    target: CueTarget::Flat,
                    held: held.contains(&frame),
                });
                audio.update(DT, scene, CueSounds { speech, ..CueSounds::NONE });
                let said = audio.said();
                audio.mix(DT);
                let samples = (DT * OFFLINE_RATE as f32) as usize * 2;
                let power =
                    audio.block[..samples].iter().map(|&sample| f64::from(sample).powi(2)).sum::<f64>();
                heard.push((said, power / samples as f64));
            }
            heard
        };
        let heard = play(0..0);
        // The line starts with a little silence, so the mix is first heard that far after the asking.
        let lead =
            decoded.samples.iter().position(|&sample| sample != 0.0).unwrap() / decoded.channels as usize;
        let lead = lead * sapphire_assets::mouth::MOUTH_RATE as usize / decoded.sample_rate as usize;
        let first = heard.iter().position(|&(_, power)| power > 0.0).unwrap();
        assert!(
            (ASKED + lead).abs_diff(first) <= 1,
            "the voice sounds from frame {first}, asked at {ASKED} with {lead} frames of silence first"
        );
        assert!(heard[..ASKED].iter().all(|(said, _)| said.is_none()));
        for (frame, (said, _)) in heard.iter().enumerate().skip(ASKED).take(length - 2) {
            let said = said.unwrap_or_else(|| panic!("frame {frame} says nothing"));
            assert_eq!(said.serial, 1);
            // The playhead is a sum of frames' seconds, so it may be an ulp from a product of them.
            let expected = curve.at((frame - ASKED) as f32 * DT);
            let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
            assert!(near(said.mouth.open, expected.open), "frame {frame}: {said:?} against {expected:?}");
            assert!(
                said.mouth.visemes.iter().zip(expected.visemes).all(|(&a, b)| near(a, b)),
                "frame {frame}"
            );
        }
        assert!(heard.last().unwrap().0.is_none(), "played out, the mouth's shut");
        let loudest = |of: &dyn Fn(usize) -> f64| {
            (ASKED..heard.len()).max_by(|&a, &b| of(a).total_cmp(&of(b))).unwrap()
        };
        let mouth_peak = loudest(&|frame| heard[frame].0.map_or(0.0, |said| f64::from(said.mouth.open)));
        let mix_peak = loudest(&|frame| heard[frame].1);
        assert!(
            mouth_peak.abs_diff(mix_peak) <= 1,
            "the mouth peaks at frame {mouth_peak}, the mix at {mix_peak}"
        );

        // Again, held by a pause partway: the mouth stays where it was, the mix falls silent, and both go on
        // from there.
        const HELD: std::ops::Range<usize> = ASKED + 30..ASKED + 50;
        let paused = play(HELD);
        let at_pause = paused[HELD.start].0.unwrap().mouth;
        assert_eq!(at_pause, heard[HELD.start].0.unwrap().mouth);
        for frame in HELD {
            assert_eq!(paused[frame].0.unwrap().mouth, at_pause, "frame {frame}");
        }
        assert!(
            paused[HELD.start + 5..HELD.end].iter().all(|&(_, power)| power == 0.0),
            "a held voice is silent"
        );
        let after = HELD.end + 1;
        assert_eq!(paused[after].0.unwrap().mouth, heard[after - HELD.len()].0.unwrap().mouth);
    }
}
