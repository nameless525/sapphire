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

use std::collections::HashSet;
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::Cue;
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::{
    AudioFrame, AudioStats, AudioSystem, CpalOutput, CueRequest, CueRng, CueTarget, Emitter, EmitterId,
    KiraBackend, Listener, OfflineOutput,
};
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
    pub emitters: &'a [Emitter],
    /// The world clock's speed where they fire, which a slow motion turns down.
    pub world_time_scale: f32,
    /// A new fight began this frame, so what the last one left playing ends before these start.
    pub started_over: bool,
}

impl CueSounds<'_> {
    /// Nothing: there's no fight.
    pub const NONE: CueSounds<'static> =
        CueSounds { requests: &[], emitters: &[], world_time_scale: 1.0, started_over: false };
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
    /// `--cue`'s.
    repeating: Option<Repeating>,
    /// This frame's, the character's and a fight's.
    emitters: Vec<Emitter>,
    /// Seconds until the next stats line.
    report_in: f32,
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
        let audio = Self::for_cues(&[option.id], platform, content, renderer, gpu)?;
        Ok(audio.map(|audio| Self {
            repeating: Some(Repeating { cue: option.id, repeat: option.repeat, next_play: Some(0.0) }),
            ..audio
        }))
    }

    /// Loads `cues` and opens the device, with nothing due to play. `None`, logged, if the OS won't set
    /// up audio.
    pub fn for_cues(
        cues: &[AssetId],
        platform: &mut Platform,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Option<Self>, AppInitError> {
        let loaded = load_cues(cues, content, renderer, gpu)?;
        if let Err(error) = platform.prepare_audio() {
            tracing::warn!(target: LOG_TARGET, %error, "no audio this run");
            return Ok(None);
        }
        let Ok(backend) = KiraBackend::<CpalOutput>::new(());
        let mut system = AudioSystem::new(backend);
        add_cues(&mut system, loaded);
        Ok(Some(Self {
            system,
            random: CueRng::new(CUE_SEED),
            repeating: None,
            emitters: Vec::with_capacity(8),
            report_in: REPORT_EVERY,
        }))
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
        };
        self.system.update(&frame, &mut self.random);
        self.report_in -= dt;
        if self.report_in <= 0.0 {
            self.report_in = REPORT_EVERY;
            self.report();
        }
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
            voices = stats.voices,
            played = stats.cues_played,
            dropped_no_output = stats.dropped_no_output,
            dropped_voice_limit = stats.dropped_voice_limit,
            dropped_unknown = stats.dropped_unknown,
            update_us = stats.update.as_micros() as u64,
            update_peak_us = peak.as_micros() as u64,
            mix_load = format_args!("{:.4}", device.mix_load()),
            mix_max_us = device.mix_nanos_max / 1_000,
            "audio stats"
        );
    }
}

/// Loads `cues` and decodes their sounds: what a mixer holds from then on.
fn load_cues(
    cues: &[AssetId],
    content: &mut Content,
    renderer: &mut Renderer,
    gpu: &GpuContext,
) -> Result<Vec<LoadedCue>, AppInitError> {
    let stopwatch = Stopwatch::start();
    let mut decoded = HashSet::new();
    let loaded = cues
        .iter()
        .map(|&id| content.load_cue(id, &mut decoded, renderer, gpu).map(|(cue, sounds)| (id, cue, sounds)))
        .collect::<Result<Vec<_>, _>>()?;
    let sounds = loaded.iter().map(|(_, _, sounds)| sounds.len()).sum::<usize>();
    let frames =
        loaded.iter().flat_map(|(_, _, sounds)| sounds).map(|(_, sound)| sound.frames()).sum::<usize>();
    let load_and_decode = stopwatch.elapsed();
    tracing::info!(target: LOG_TARGET, cues = cues.len(), sounds, frames, ?load_and_decode, "cues ready");
    Ok(loaded)
}

/// Gives `system` what [`load_cues`] loaded.
fn add_cues<B: sapphire_audio::AudioBackend>(system: &mut AudioSystem<B>, loaded: Vec<LoadedCue>) {
    for (id, cue, sounds) in loaded {
        system.add_cue(id, cue);
        for (id, sound) in &sounds {
            system.add_sound(*id, sound);
        }
    }
}

/// A cue and those of its sounds no cue before it brought, decoded.
type LoadedCue = (AssetId, Cue, Vec<(AssetId, DecodedSound)>);

/// The rate [`OfflineAudio`] mixes at: a phone's and most devices' own.
const OFFLINE_RATE: u32 = 48_000;

/// A fight's sound through the real mixer and no device (`--bench` with `--cues`, M2 Z1): the update a
/// frame pays, and the mix a phone pays on its callback, each measured where it happens, in a run that
/// must open no device.
pub struct OfflineAudio {
    system: AudioSystem<KiraBackend<OfflineOutput>>,
    random: CueRng,
    emitters: Vec<Emitter>,
    /// A frame's samples, interleaved, which nothing reads.
    block: Vec<f32>,
}

impl OfflineAudio {
    /// Loads `cues`, as the windowed app does for a fight.
    pub fn for_cues(
        cues: &[AssetId],
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Self, AppInitError> {
        let loaded = load_cues(cues, content, renderer, gpu)?;
        let Ok(backend) = KiraBackend::<OfflineOutput>::new(OFFLINE_RATE);
        let mut system = AudioSystem::new(backend);
        add_cues(&mut system, loaded);
        Ok(Self {
            system,
            random: CueRng::new(CUE_SEED),
            emitters: Vec::with_capacity(8),
            block: vec![0.0; OFFLINE_RATE as usize],
        })
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
        };
        self.system.update(&frame, &mut self.random);
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

    #[test]
    fn an_opened_device_fails_an_offscreen_run() {
        assert_eq!(stayed_shut(0), Ok(()));
        assert!(stayed_shut(1).unwrap_err().contains("audio device"));
    }
}
