//! M6 U10's loudness gate: committed fights heard as the game hears them, through the shipped cue table, mix, score
//! and voice lines on the real mixer with no device, metered by BS.1770 against each output profile's loudness.
//!
//! - `fight` is a string of hits on the warrior: effects over the score.
//! - `dodge-counter` is a perfect dodge and its counter: the dodge's line, which ducks the music, then hits.
//! - `melee-play` is a long played melee: steps, swings, hits and falls, a minute and more.
//!
//! The score plays its `running` phase throughout, as it does while an encounter runs, and the listener stands
//! where the game's camera does, behind the player and above. Each render is the replay's whole length. A render
//! that misses its profile is fixed in the mix's master gain for that profile (`content/audio/game.mix.ron`),
//! never here. `--nocapture` prints every reading.

#![allow(clippy::print_stdout, reason = "the readings are what a sound designer tunes the mix's gains by")]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Bus, Cue};
use sapphire_assets::mix::{LOUDNESS_TOLERANCE_LU, Mix, OutputProfile};
use sapphire_assets::mouth::MouthCurve;
use sapphire_assets::score::Score;
use sapphire_assets::sound::{DecodedSound, Sound};
use sapphire_assets::voice::VoiceLines;
use sapphire_audio::loudness::{Loudness, measure};
use sapphire_audio::{
    AudioFrame, AudioSystem, CueRng, DynamicRange, KiraBackend, LIMITER_CEILING_DB, Listener, MusicRequest,
    OfflineOutput, PAUSE_SNAPSHOT, Rooms,
};
use sapphire_camera::{CameraDirector, CameraTuning};
use sapphire_core::Name;
use sapphire_cues::{CueDispatcher, CueStep, CueTable, Stand, actor_emitter};
use sapphire_gameplay::replay::{Player, Replay, SCENARIOS};
use sapphire_math::{Mat4, Vec3};
use sapphire_time::ScaledClock;
use sapphire_vfx::{EffectLibrary as VfxLibrary, VfxBudget, VfxWorld};
use sapphire_world::screen_effects::{EffectLibrary as ScreenLibrary, EffectStack};

const RATE: u32 = 48_000;
const FRAME: f32 = 1.0 / 60.0;
/// A frame's samples, in stereo.
const BLOCK: usize = RATE as usize / 60 * 2;
const REPLAYS: [&str; 3] = ["fight", "dodge-counter", "melee-play"];
/// Where the game's camera hears from: this far behind the player, and this high.
const BEHIND: f32 = 4.0;
const ABOVE: f32 = 1.6;
/// How far Night must narrow a render's loudness range (m6-plan's U10).
const NIGHT_NARROWS_LU: f64 = 6.0;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every source under `folder` with a sidecar, by the ID its sidecar gives it.
fn sources_in(folder: &Path, found: &mut HashMap<AssetId, PathBuf>) {
    for entry in fs::read_dir(folder).expect("reads") {
        let path = entry.expect("reads").path();
        if path.is_dir() {
            sources_in(&path, found);
        } else if let Some(source) = path.to_str().and_then(|path| path.strip_suffix(".meta")) {
            let sidecar = fs::read_to_string(&path).expect("reads");
            let id = sidecar.split('"').nth(3).expect("type, then id").parse().expect("an ID");
            found.insert(id, PathBuf::from(source));
        }
    }
}

/// What the shipped table plays, read from its sources: the documents as they ship and the sounds decoded.
struct Shipped {
    table: CueTable,
    mix: Mix,
    score: (AssetId, Score),
    voices: Vec<VoiceLines>,
    cues: Vec<(AssetId, Cue)>,
    sounds: Vec<(AssetId, DecodedSound)>,
    mouths: Vec<(AssetId, MouthCurve)>,
}

static SHIPPED: LazyLock<Shipped> = LazyLock::new(|| {
    let mut sources = HashMap::new();
    for folder in ["content/audio", "content/vo", "content/third_party"] {
        sources_in(&repo().join(folder), &mut sources);
    }
    let text =
        |id: AssetId| fs::read_to_string(&sources[&id]).unwrap_or_else(|error| panic!("{id}: {error}"));
    let table = sapphire_cues::default_table();
    let mix = Mix::from_document(&text(table.mix().expect("the table names its mix"))).expect("the mix");
    let score_id = table.music().expect("the table names its score");
    let score = Score::from_document(&text(score_id)).expect("the score");
    let voices: Vec<VoiceLines> =
        table.voices().iter().map(|&id| VoiceLines::from_document(&text(id)).expect("voice lines")).collect();
    let mut cue_ids = table.sound_cues();
    cue_ids.extend(voices.iter().flat_map(|lines| lines.lines.iter().map(|line| line.cue)));
    cue_ids.sort_unstable();
    cue_ids.dedup();
    let cues: Vec<(AssetId, Cue)> =
        cue_ids.iter().map(|&id| (id, Cue::from_document(&text(id)).expect("a cue"))).collect();
    let mut sound_ids: Vec<(AssetId, bool)> = cues
        .iter()
        .flat_map(|(_, cue)| cue.sounds().into_iter().map(|sound| (sound, cue.bus == Bus::Voice)))
        .collect();
    sound_ids.extend(score.stems.iter().map(|stem| (stem.sound, false)));
    sound_ids.sort_unstable();
    sound_ids.dedup_by_key(|(id, _)| *id);
    let mut sounds = Vec::new();
    let mut mouths = Vec::new();
    for (id, voiced) in sound_ids {
        let sound = Sound::from_vorbis(fs::read(&sources[&id]).expect("reads")).expect("a sound");
        let decoded = sound.decode().expect("decodes");
        if voiced {
            mouths.push((id, MouthCurve::analyze(&decoded)));
        }
        sounds.push((id, decoded));
    }
    Shipped { table, mix, score: (score_id, score), voices, cues, sounds, mouths }
});

/// How the player hears it: the output profile, the dynamic range and the overall volume, a share of full.
#[derive(Clone, Copy, Debug)]
struct Heard {
    profile: OutputProfile,
    range: DynamicRange,
    volume: f32,
}

impl Heard {
    const fn at(profile: OutputProfile, range: DynamicRange) -> Self {
        Self { profile, range, volume: 1.0 }
    }
}

/// The mixer, with everything the shipped table plays, heard as `heard` says.
fn mixer(heard: Heard) -> AudioSystem<KiraBackend<OfflineOutput>> {
    let shipped = &*SHIPPED;
    let Ok(backend) = KiraBackend::<OfflineOutput>::new(RATE);
    let mut audio = AudioSystem::new(backend);
    audio.set_mix(&shipped.mix).expect("the shipped mix checks");
    audio.set_output_profile(heard.profile);
    audio.set_dynamic_range(heard.range);
    audio.set_volume(heard.volume);
    for (id, sound) in &shipped.sounds {
        audio.add_sound(*id, sound);
    }
    for (id, cue) in &shipped.cues {
        audio.add_cue(*id, cue.clone());
    }
    for (id, mouth) in &shipped.mouths {
        audio.add_mouth(*id, mouth.clone());
    }
    let (id, score) = &shipped.score;
    audio.add_score(*id, score.clone());
    for lines in &shipped.voices {
        audio.add_voice_lines(lines).expect("voice lines");
    }
    assert!(audio.set_voice_language("en"));
    audio
}

/// How long a session waits in the score's calm before the fight, and how long it's paused after.
const CALM_SECONDS: usize = 20;

/// `tests/replays/<replay>.sreplay`'s sound, from its first step to its last, interleaved stereo; as a session,
/// with [`CALM_SECONDS`] of the score's `waiting` before it, as before an encounter, and as long paused after it,
/// the score's `running` held under the mix's `pause` as a player's menu holds it.
fn render(replay: &str, heard: Heard, session: bool) -> Vec<f32> {
    let shipped = &*SHIPPED;
    let bytes = fs::read(repo().join("tests/replays").join(format!("{replay}.sreplay"))).expect("committed");
    let mut player = Player::new(Replay::decode(&bytes).expect("reads"), &SCENARIOS).expect("sets up");
    let effects = VfxLibrary::default_effects();
    let presets = ScreenLibrary::default_effects();
    let tuning = CameraTuning::default_tuning();
    let mut cues = CueDispatcher::new(&shipped.table, &effects, &presets, &tuning).expect("resolves");
    let mut vfx = VfxWorld::new(effects, VfxBudget::DESKTOP_HIGH, 7);
    let mut world = ScaledClock::new();
    let mut screen = EffectStack::new(world.new_local_clock());
    let mut camera = CameraDirector::new(tuning, 7).expect("valid");
    let mut audio = mixer(heard);
    let mut random = CueRng::new(7);
    let mut emitters = Vec::with_capacity(16);
    let mut samples = Vec::new();
    let mut block = vec![0.0; BLOCK];
    let phase = |phase| Some(MusicRequest { score: shipped.score.0, phase, instead: None });
    let calm = |phase,
                snapshots: &[Name],
                audio: &mut AudioSystem<KiraBackend<OfflineOutput>>,
                random: &mut CueRng,
                samples: &mut Vec<f32>| {
        let mut block = vec![0.0; BLOCK];
        for _ in 0..CALM_SECONDS * 60 {
            let frame = AudioFrame {
                dt: FRAME,
                listener: Listener::default(),
                world_time_scale: 1.0,
                snapshots,
                emitters: &[],
                cues: &[],
                lines: &[],
                speech: None,
                music: phase,
                rooms: Rooms::NONE,
            };
            audio.update(&frame, random);
            audio.backend_mut().output_mut().render_into(&mut block);
            samples.extend_from_slice(&block);
        }
    };
    if session {
        calm(phase("waiting"), &[], &mut audio, &mut random, &mut samples);
    }
    let music = phase("running");
    while player.step() {
        world.tick();
        screen.tick(&world);
        let sim = player.simulation();
        let stand_of = |actor| Stand::of(sim, actor);
        let mut step = CueStep {
            vfx: &mut vfx,
            screen: &mut screen,
            presets: &presets,
            shakes: &mut camera,
            view_projection: Mat4::IDENTITY,
            stand_of: &stand_of,
        };
        cues.read_step(sim.tick(), sim.cues(), &mut step);
        cues.advance_effects(sim, &mut vfx);
        emitters.clear();
        emitters.extend(sim.actors().map(|actor| actor_emitter(&actor)));
        let lead = player.cast().first().copied();
        let stands =
            sim.actors().find(|actor| Some(actor.id) == lead).map_or(Vec3::ZERO, |actor| actor.position);
        let listener =
            Listener { position: stands + Vec3::new(0.0, ABOVE, -BEHIND), forward: Vec3::Z, up: Vec3::Y };
        let frame = AudioFrame {
            dt: FRAME,
            listener,
            world_time_scale: 1.0,
            snapshots: &[],
            emitters: &emitters,
            cues: cues.sounds(),
            lines: cues.lines(),
            speech: None,
            music,
            rooms: Rooms::NONE,
        };
        audio.update(&frame, &mut random);
        audio.backend_mut().output_mut().render_into(&mut block);
        samples.extend_from_slice(&block);
        cues.end_frame(FRAME, &mut ());
    }
    if session {
        calm(music, &[PAUSE_SNAPSHOT], &mut audio, &mut random, &mut samples);
    }
    samples
}

fn read(replay: &str, heard: Heard, session: bool) -> Loudness {
    let loudness = measure(&render(replay, heard, session), RATE);
    let what = if session { "a session of " } else { "" };
    let Heard { profile, range, volume } = heard;
    println!(
        "{what}{replay}, {profile:?}, {range:?}, volume {volume}: {:.2} LUFS, range {:.2} LU, true peak {:.2} dBTP, \
         {:.1} s",
        loudness.integrated_lufs,
        loudness.range_lu,
        loudness.true_peak_db,
        (loudness.blocks + 3) as f64 / 10.0
    );
    loudness
}

/// Each render plays within ±2 LU of its profile's loudness under every dynamic range, with no true peak over the
/// limiter's ceiling. `Wide` is the mix as authored, which the profiles' gains are set by: a range compresses
/// toward the profile's loudness, so a miss under it alone would hide a mix twice as far off.
#[test]
fn each_fight_plays_at_its_profiles_loudness() {
    let mut misses = Vec::new();
    for replay in REPLAYS {
        for profile in OutputProfile::ALL {
            for range in DynamicRange::ALL {
                let heard = read(replay, Heard::at(profile, range), false);
                let target = f64::from(profile.loudness_lufs());
                if (heard.integrated_lufs - target).abs() > f64::from(LOUDNESS_TOLERANCE_LU) {
                    misses.push(format!(
                        "{replay} through {profile:?} under {range:?} reads {:.2} LUFS against {target}",
                        heard.integrated_lufs
                    ));
                }
                if heard.true_peak_db > f64::from(LIMITER_CEILING_DB) {
                    misses.push(format!(
                        "{replay} through {profile:?} under {range:?} peaks at {:.2} dBTP",
                        heard.true_peak_db
                    ));
                }
            }
        }
    }
    assert!(misses.is_empty(), "set the profile's `volume_db` in game.mix.ron:\n{}", misses.join("\n"));
}

/// Night narrows a session's loudness range by at least 6 LU from Wide's, at home: the calm before the longest
/// fight, the fight, and a pause after it. A fight alone, under its whole score, has little range to narrow.
#[test]
fn night_narrows_a_sessions_range() {
    let replay = REPLAYS[2];
    let wide = read(replay, Heard::at(OutputProfile::Home, DynamicRange::Wide), true);
    let night = read(replay, Heard::at(OutputProfile::Home, DynamicRange::Night), true);
    assert!(
        wide.range_lu - night.range_lu >= NIGHT_NARROWS_LU,
        "{replay}: Wide's range is {:.2} LU and Night's {:.2}",
        wide.range_lu,
        night.range_lu
    );
    assert!(night.true_peak_db <= f64::from(LIMITER_CEILING_DB), "{night:?}");
}

/// The player's overall volume moves the fight by exactly its decibels under every range that compresses, within
/// 0.5 LU: it's the mix's last gain, after the range and the limiter, so neither undoes the slider.
#[test]
fn the_volume_moves_the_fight_by_its_decibels() {
    let replay = REPLAYS[0];
    let mut misses = Vec::new();
    for range in [DynamicRange::Standard, DynamicRange::Night] {
        let full = read(replay, Heard::at(OutputProfile::Home, range), false).integrated_lufs;
        for volume in [0.5_f32, 0.2] {
            let heard = Heard { volume, ..Heard::at(OutputProfile::Home, range) };
            let moved = read(replay, heard, false).integrated_lufs - full;
            let wanted = 20.0 * f64::from(volume).log10();
            if (moved - wanted).abs() > 0.5 {
                misses.push(format!("{range:?} at {volume}: {moved:.2} LU, not {wanted:.2}"));
            }
        }
    }
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}
