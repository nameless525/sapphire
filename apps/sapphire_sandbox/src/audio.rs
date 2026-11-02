//! `--cue <name>`: a cue played at the character on the real audio device (M2 E1, spec §5.15).
//!
//! Only that flag makes a [`SandboxAudio`]. Without it the app mounts no audio pack, starts no audio
//! thread and opens no device. The offscreen modes refuse the flag, and [`stayed_shut`] holds capture
//! and bench to it at the end of every run, golden scenes included: CI's runners have no sound.
//!
//! A device that isn't there or goes away is a logged state of the mixer, never a failure, and so is
//! an OS that won't set up its audio session: the app runs on in silence. A cue that can't load is
//! the run's failure: it was asked for by name.
//!
//! Slow motion (`Space`) bends and darkens what's playing, and hitstop (`H`) freezes `--cue swing`,
//! both through the scene's own clocks ([`Scene::sound`](crate::scene::Scene::sound)). A sound
//! recooked while the app runs isn't picked up: the mixer holds what was decoded at startup.

use sapphire_assets::AssetId;
use sapphire_audio::{
    AudioFrame, AudioSystem, CpalOutput, CueRequest, CueRng, CueTarget, Emitter, EmitterId, KiraBackend,
    Listener,
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

/// The emitter cues play on: the scene's character.
const CHARACTER: EmitterId = EmitterId(0);

/// Seconds between stats lines.
const REPORT_EVERY: f32 = 2.0;

/// The picks' seed: every run plays the same sequence, which is all a test bed wants.
const CUE_SEED: u64 = 0x5EED;

/// `--cue <name> [--cue-repeat <seconds>]`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CueOption {
    pub id: AssetId,
    /// Seconds between plays; once, without it.
    pub repeat: Option<f32>,
}

/// The cue `--cue` named, or an error listing the names it knows.
pub(crate) fn named_cue(name: &str) -> Result<AssetId, String> {
    let found = CUES.iter().find(|(known, _)| *known == name);
    found.map(|&(_, id)| id).ok_or_else(|| {
        let names = CUES.map(|(known, _)| known).join(", ");
        format!("--cue expects one of {names}, got {name:?}")
    })
}

/// What the audio system hears of the scene this frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SceneSound {
    pub listener: Listener,
    /// The world clock's speed.
    pub world_time_scale: f32,
    /// Where the character stands.
    pub character: Vec3,
    /// Whether the character's clock is held by hitstop.
    pub hitstop: bool,
}

/// The audio system on the real device, playing one cue.
pub(crate) struct SandboxAudio {
    system: AudioSystem<KiraBackend<CpalOutput>>,
    random: CueRng,
    cue: AssetId,
    repeat: Option<f32>,
    /// Seconds until the next play; none once a cue that doesn't repeat has played.
    next_play: Option<f32>,
    /// Seconds until the next stats line.
    report_in: f32,
}

impl SandboxAudio {
    /// Loads the cue and opens the device. `None`, logged, if the OS won't set up audio.
    pub fn new(
        option: &CueOption,
        platform: &mut Platform,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Option<Self>, AppInitError> {
        let stopwatch = Stopwatch::start();
        let (cue, sounds) = content.load_cue(option.id, renderer, gpu)?;
        let loaded = stopwatch.elapsed();
        if let Err(error) = platform.prepare_audio() {
            tracing::warn!(target: LOG_TARGET, %error, "no audio this run");
            return Ok(None);
        }
        let Ok(backend) = KiraBackend::<CpalOutput>::new(());
        let mut system = AudioSystem::new(backend);
        let frames: usize = sounds.iter().map(|(_, sound)| sound.frames()).sum();
        system.add_cue(option.id, cue);
        for (id, sound) in &sounds {
            system.add_sound(*id, sound);
        }
        tracing::info!(
            target: LOG_TARGET,
            cue = %option.id,
            sounds = sounds.len(),
            frames,
            load_and_decode = ?loaded,
            "cue ready"
        );
        Ok(Some(Self {
            system,
            random: CueRng::new(CUE_SEED),
            cue: option.id,
            repeat: option.repeat,
            next_play: Some(0.0),
            report_in: REPORT_EVERY,
        }))
    }

    /// One frame, `dt` real seconds after the last: plays the cue when it's due, and follows the OS's
    /// interruptions and the scene.
    pub fn update(&mut self, dt: f32, platform: &Platform, scene: SceneSound) {
        self.system.set_interrupted(platform.audio_interrupted());
        let due = self.next_play.as_mut().is_some_and(|left| {
            *left -= dt;
            *left <= 0.0
        });
        if due {
            self.next_play = self.repeat;
        }
        let requests = [CueRequest { cue: self.cue, target: CueTarget::On(CHARACTER) }];
        let emitters = [Emitter { id: CHARACTER, position: scene.character, hitstop: scene.hitstop }];
        let frame = AudioFrame {
            dt,
            listener: scene.listener,
            world_time_scale: scene.world_time_scale,
            emitters: &emitters,
            cues: if due { &requests } else { &[] },
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
    pub fn stats(&self) -> sapphire_audio::AudioStats {
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

/// An error unless a run that must open no audio device, having asked `opens` times, opened none.
pub(crate) fn stayed_shut(opens: u64) -> Result<(), String> {
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
