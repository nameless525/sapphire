//! M3's exit test 3, with the player in front: run a cutscene and a cinematic as an app runs them, from a
//! real `TimelinePlayer`'s `holds_simulation` and `skip`, and skip at every frame it could be skipped at. The
//! fight that follows stays on the checksums of watching. Then the same with every presentation track gone
//! from the document, which is what "a replay plays the same with them ignored" means.

use std::sync::Arc;

use sapphire_gameplay::replay::{SCENARIOS, Scenario, find_scenario, melee_sweep};
use sapphire_gameplay::{ActorId, Simulation, TickActions, TimelineId};
use sapphire_sequencer::{Origin, Playing, StepClock, TimelinePlayer};
use sapphire_timeline::{Timeline, TimelineDocument};

const CUTSCENE: &str = include_str!("../../sapphire_timeline/test_content/cutscene.timeline.ron");
const CINEMATIC: &str = include_str!("../../sapphire_timeline/test_content/cinematic.timeline.ron");

const PLAY_AT: u64 = 90;
const AFTER: u64 = 90;
/// A display's frame, on no multiple of a step.
const FRAME: f32 = 1.0 / 73.0;

fn cook(text: &str) -> Arc<Timeline> {
    Arc::new(TimelineDocument::from_ron(text).expect("reads").cook().expect("cooks"))
}

fn scenario() -> &'static Scenario {
    find_scenario(SCENARIOS, "duel").expect("listed")
}

/// A fight, and a timeline, loaded as `timelines` does but free of its document: so a test can load one
/// with its presentation changed.
fn world(timeline: &Timeline) -> (Simulation, Vec<ActorId>, TimelineId) {
    let (mut sim, cast) = scenario().start(scenario().config).expect("sets up");
    let id = sim.add_timeline(timeline.simulation.clone()).expect("loads");
    (sim, cast, id)
}

struct Run {
    /// The checksum after every step.
    trail: Vec<u64>,
    frames_held: u32,
    /// The player's final state.
    state: Playing,
}

/// An app's loop, frame by frame: fixed steps on an accumulator unless the player holds them, and a skip at
/// frame `skip_at` of the timeline, if there is one.
fn run(timeline: &Arc<Timeline>, skip_at: Option<u32>) -> Run {
    let (mut sim, cast, id) = world(timeline);
    let mut trail = Vec::new();
    let actions = |tick: u64, cast: &[ActorId]| melee_sweep(tick, cast);
    for tick in 0..PLAY_AT {
        sim.step(&actions(tick, &cast));
        trail.push(sim.checksum());
    }
    sim.play_timeline(id).expect("loaded");
    let mut player = TimelinePlayer::new(timeline.clone(), Origin::default());
    let mut owed = 0.0_f32;
    let mut frames_held = 0;
    let mut frame = 0;
    while player.state() == Playing::Yes {
        let steps = sim.timeline().map(|run| StepClock { taken: run.tick, alpha: owed * 60.0 });
        player.advance(FRAME, steps);
        if player.holds_simulation() {
            frames_held += 1;
        } else if player.state() == Playing::Yes {
            owed += FRAME;
            while owed >= 1.0 / 60.0 {
                owed -= 1.0 / 60.0;
                sim.step(&actions(sim.tick(), &cast));
                trail.push(sim.checksum());
            }
        }
        if skip_at == Some(frame) {
            player.skip();
            for _ in 0..sim.timeline_steps_held() {
                sim.step(&TickActions::NEUTRAL);
                trail.push(sim.checksum());
            }
        }
        frame += 1;
    }
    let state = player.state();
    // Whatever a watch's last frame left unrun, by the steps it was owed.
    for _ in 0..sim.timeline_steps_held() {
        sim.step(&actions(sim.tick(), &cast));
        trail.push(sim.checksum());
    }
    for _ in 0..AFTER {
        sim.step(&actions(sim.tick(), &cast));
        trail.push(sim.checksum());
    }
    Run { trail, frames_held, state }
}

#[test]
fn a_cutscene_skipped_at_every_frame_reaches_the_checksums_of_watching_it() {
    let timeline = cook(CUTSCENE);
    let watch = run(&timeline, None);
    assert_eq!(watch.state, Playing::Finished);
    let frames = (6.0 / FRAME) as u32;
    assert!(watch.frames_held >= frames - 1, "the simulation was held: {} frames", watch.frames_held);
    assert_eq!(watch.trail.len() as u64, PLAY_AT + AFTER, "and took no step for it");
    let mut skips = 0;
    for skip_at in 0..frames - 1 {
        let skipped = run(&timeline, Some(skip_at));
        assert_eq!(skipped.state, Playing::Skipped, "at frame {skip_at}");
        assert_eq!(skipped.frames_held, skip_at + 1);
        assert_eq!(skipped.trail, watch.trail, "skipped at frame {skip_at}");
        skips += 1;
    }
    assert!(skips > 400, "{skips} skips");
}

#[test]
fn a_cinematic_skipped_at_every_frame_reaches_the_checksums_of_watching_it() {
    let timeline = cook(CINEMATIC);
    let watch = run(&timeline, None);
    assert_eq!(watch.frames_held, 0, "it runs the simulation");
    let frames = (2.5 / FRAME) as u32;
    let mut skips = 0;
    for skip_at in 0..frames - 2 {
        let skipped = run(&timeline, Some(skip_at));
        assert_eq!(skipped.state, Playing::Skipped, "at frame {skip_at}");
        assert_eq!(skipped.trail, watch.trail, "skipped at frame {skip_at}");
        skips += 1;
    }
    assert!(skips > 150, "{skips} skips");
}

/// With every presentation track out of the document, the simulation's half is the same bytes and the same
/// run. So nothing a track says can reach a checksum, which is the rule that shapes this package.
#[test]
fn a_run_is_the_same_with_every_presentation_track_gone() {
    for text in [CUTSCENE, CINEMATIC] {
        let mut stripped = TimelineDocument::from_ron(text).expect("reads");
        stripped.camera = None;
        stripped.characters.clear();
        stripped.cues.clear();
        stripped.post.clear();
        stripped.subtitles.clear();
        let stripped = Arc::new(stripped.cook().expect("cooks"));
        let whole = cook(text);
        assert_eq!(stripped.simulation.digest(), whole.simulation.digest());
        assert_eq!(run(&stripped, None).trail, run(&whole, None).trail);
    }
}
