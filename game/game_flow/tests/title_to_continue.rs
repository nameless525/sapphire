//! The game's flow over real runs with no window (M3 G1): from the title a new game, a trip out of the flow's
//! zone and back, a checkpoint reached and saved, the pause's way back to the title under the wipe, and the
//! title's continue landing where the save says, here and in a game started over from the same save. Each of
//! the run's steps allocates nothing, nor does the flow after a step that saves nothing. Installs
//! `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;
use std::path::PathBuf;

use game_combat::combat::{Combat, CombatOption};
use game_flow::document::FlowDocument;
use game_flow::flow::{FlowCommand, FlowState, Run};
use game_flow::game::{FlowContent, Game, GameSetup};
use game_flow::progress::{self, progress_store};
use sapphire_core::Name;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_gameplay::{ActorId, TickActions};
use sapphire_serialize::AssetId;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// content/zones/proving_ground/proving_ground.zone.ron
const GROUND: AssetId = AssetId::from_text("e8c21831-7d98-4a79-b592-912b806cf3e2");
/// content/zones/proving_hub/proving_hub.zone.ron
const HUB: AssetId = AssetId::from_text("5f3ac6de-0b7c-4f21-9d3e-7a1e64c2b805");
/// The proving ground's platform checkpoint, where the save puts a continued run.
const PLATFORM: [f32; 3] = [-13.5, 1.0, -20.0];

/// `ground-trip` in the proving ground, which its cart yard's clearing wins.
const FLOW: &str = r#"(type: "sapphire::game::FlowDocument", version: 1, data: (
    scenario: "ground-trip",
    zone: "e8c21831-7d98-4a79-b592-912b806cf3e2",
    beats: [(when: Cleared("5d3a9e21-7c44-4b0e-9f16-0a8ce3b7d214"), then: Victory)],
    music: (title: "waiting", victory: "cleared", defeat: "waiting"),
    holds: (banner: 2.0, defeat: 3.0, victory: 5.0),
))"#;

/// A directory of the test's own, gone when it is.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("game_flow-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory of its own");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn game(dir: &Scratch) -> Game {
    let content =
        FlowContent { document: FlowDocument::from_ron(FLOW).expect("reads"), registered: Vec::new() };
    let store = progress_store(&dir.0, |path, bytes| std::fs::write(path, bytes));
    Game::new(content, GameSetup { store: Some(store), ..GameSetup::default() })
        .expect("the game has what it names")
}

/// `steps` of the run with nobody pressing anything, each followed by the flow, as the show does: none
/// allocates. Returns whether the flow allocated after any of them.
fn steps(game: &mut Game, combat: &mut Combat, steps: u32) -> bool {
    let mut neutral = |_: u64, _: &[ActorId]| TickActions::NEUTRAL;
    let mut flow_allocated = false;
    for _ in 0..steps {
        assert!(game.flow().runs_steps(), "the flow runs the run's steps");
        combat.step(&mut neutral);
        if PER_THREAD_COUNTS {
            assert_eq!(combat.last_step_allocations(), 0, "a step allocates nothing");
        }
        let (opened, counts) = alloc::count_in_scope(|| game.after_step(combat));
        assert_eq!(opened, None, "no beat opens a dialogue here");
        flow_allocated |= counts.allocations > 0;
    }
    flow_allocated
}

fn player(combat: &Combat) -> [f32; 3] {
    let id = combat.cast()[0];
    combat.simulation().actor(id).expect("the player stands").position.to_array()
}

fn near(at: [f32; 3], to: [f32; 3]) -> bool {
    at.iter().zip(to).all(|(at, to)| (at - to).abs() < 2.0)
}

#[test]
fn a_new_game_goes_out_and_back_saves_at_a_checkpoint_and_continues_there() {
    let dir = Scratch::new("title-to-continue");
    let mut game = game(&dir);
    assert_eq!(game.flow().state(), FlowState::Title { stood: Run::New });
    assert!(!game.flow().can_continue(), "nothing saved yet");
    let mut combat = Combat::new(&game.first_run()).expect("the run behind the title stands");

    // The title's new game plays the run that's standing, and saves where a new game starts.
    assert_eq!(game.command(FlowCommand::NewGame), None);
    assert_eq!(game.flow().state(), FlowState::Playing);
    assert_eq!(game.progress().map(|saved| saved.zone), Some(GROUND));
    let allocated = steps(&mut game, &mut combat, 30);
    assert!(!allocated || !PER_THREAD_COUNTS, "the flow after a step that saves nothing allocates nothing");
    let start = game.progress().cloned();

    // Out through an exit to the hub, where nothing's saved, and back.
    combat.enter_zone(HUB, "", Name::new("player_start").as_u64(), |_| None).expect("the hub's on the shelf");
    steps(&mut game, &mut combat, 10);
    let sim = combat.simulation();
    let row = sim.current_zone().and_then(|id| sim.zone_ground(id)).expect("in a zone");
    assert_eq!(row.asset(), HUB);
    assert_eq!(game.progress().cloned(), start, "away from the flow's zone, nothing's saved");
    combat.enter_zone(GROUND, "", Name::new("player_start").as_u64(), |_| None).expect("and back");
    steps(&mut game, &mut combat, 10);
    assert_eq!(game.progress().cloned(), start, "back where it started, nothing's new");

    // The platform, as a beat's align brings the player there: reached on the next step, and saved.
    combat.return_to_checkpoint("platform").expect("the ground has one");
    steps(&mut game, &mut combat, 5);
    let saved = game.progress().cloned().expect("a save");
    assert_eq!(saved.start.checkpoint.as_deref(), Some("platform"), "the checkpoint's saved");

    // The pause goes back to the title: the wipe covers, the run the save says stands, the wipe clears.
    assert_eq!(game.command(FlowCommand::Pause), None);
    assert_eq!(game.command(FlowCommand::ToTitle), None);
    assert!(!game.flow().runs_steps(), "nothing steps under the wipe");
    assert_eq!(game.wipe(false, false), None, "not covered yet");
    let option = game.wipe(true, false).expect("covered: the saved run stands");
    let CombatOption::Run { start, .. } = &option else { panic!("a run: {option:?}") };
    assert_eq!(start.zone.as_ref().and_then(|zone| zone.checkpoint.as_deref()), Some("platform"));
    combat.finish().expect("nothing kept");
    let mut combat = Combat::new(&option).expect("the saved run stands");
    assert_eq!(game.wipe(true, true), None);
    assert_eq!(game.flow().state(), FlowState::Title { stood: Run::Saved });
    assert!(game.flow().can_continue());

    // The title's continue plays it, from the platform.
    assert_eq!(game.command(FlowCommand::Continue), None);
    assert_eq!(game.flow().state(), FlowState::Playing);
    assert!(near(player(&combat), PLATFORM), "the run starts where the save says: {:?}", player(&combat));
    steps(&mut game, &mut combat, 5);
    assert_eq!(game.progress().cloned(), Some(saved.clone()), "standing there again moves nothing on");
    game.finish();
    assert_eq!(game.stats().saves.failed, 0);

    // And a game started over from the same save, as the next launch is.
    let store = progress_store(&dir.0, |path, bytes| std::fs::write(path, bytes));
    assert_eq!(progress::load(&store), Some(saved));
    let mut next = self::game(&dir);
    assert_eq!(next.flow().state(), FlowState::Title { stood: Run::Saved });
    let combat = Combat::new(&next.first_run()).expect("the saved run stands behind the title");
    assert!(near(player(&combat), PLATFORM), "the next launch's continue starts there too");
}
