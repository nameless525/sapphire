//! The graph pane's trace and its overrides against a fight that runs (M6 A9), with no GPU, no cook and no
//! window: a device of the test's own, on a real live link and a real socket, plays a recorded arena run's
//! simulation a step a frame and, beside it, the arena fighter's shipped graph on a presentation player, fed
//! from the fighter's actor view as the stage feeds it (`sapphire_stage::anim_graph`). Its clips are
//! stand-ins, since no cook is read: the states, transitions, blends and parameters are the shipped graph's.
//! The editor follows the graph, reads its trace (made as the sandbox makes it, `GraphTrace::of`), pins
//! `speed` and lets it go, through its commands. What this proves is the protocol and the editor's commands.
//! That no checksum can move is structural, not this test's: the fixed step's player lives inside its
//! `FixedGraphHistory`, which hands out no `&mut GraphPlayer`, and its canonical bytes write the input, never
//! an override (`sapphire_body_pose`'s `graph/debug.rs`). The run's checksums are still held to those of the
//! same run alone, tick for tick, so a device that ever fed the graph back would show here.

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use sapphire_anim::{GraphInput, GraphPlayer};
use sapphire_assets::MemoryPackSource;
use sapphire_assets::graph::clip_id;
use sapphire_assets::graph::v2::{AnimGraph, program::Target};
use sapphire_core::Name;
use sapphire_editor::commands::Command;
use sapphire_editor::{Editor, Options};
use sapphire_gameplay::replay::{Player, Replay, SCENARIOS};
use sapphire_livelink::protocol::{DeviceStatus, GraphTrace, ReplyBody, TracedParameter};
use sapphire_livelink::{LinkMode, LiveLink, LivePacks, Push};
use sapphire_stage::anim_graph::{combat_input, locomotion_of, time_ticks};
use sapphire_time::Stopwatch;

use sapphire_anim as body;

#[path = "../../../crates/sapphire_body_pose/tests/support/humanoid.rs"]
mod humanoid;

const HEROINE: &str = include_str!("../../../content/lookdev/character/character.animgraph.ron");
const GRAPH: &str = "character.animgraph.ron";
/// A recorded arena fight in which the player stands: what an override of `speed` shows plainly.
const RUN: &[u8] = include_bytes!("../../../tests/replays/arena-fight-still.sreplay");
/// Ticks the run steps between what the editor does.
const STRETCH: u32 = 90;
/// How long the editor has to see something it should, under a loaded machine.
const PATIENCE: Duration = Duration::from_secs(60);

/// The run's checksum after each step, alone.
fn checksums_alone() -> Vec<u64> {
    let mut run = Player::new(Replay::decode(RUN).expect("it decodes"), &SCENARIOS).expect("it plays");
    let mut checksums = vec![run.simulation().checksum()];
    while run.step() {
        checksums.push(run.simulation().checksum());
    }
    checksums
}

/// The device: the run and the fighter's graph beside it, stepping [`STRETCH`] ticks, then waiting for an
/// override, then [`STRETCH`] more with it, then waiting for it to go, then to the end. It returns the run's
/// checksums and the ticks it stepped with the override on.
struct Device {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<(Vec<u64>, u32)>>,
}

impl Device {
    fn start() -> Self {
        let free = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = free.local_addr().expect("its address");
        drop(free);
        let started = Stopwatch::start();
        let link = loop {
            let packs = Arc::new(LivePacks::new(MemoryPackSource::new()));
            match LiveLink::start(LinkMode::Listen(address), packs) {
                Ok(link) => break link,
                Err(error) => {
                    assert!(started.elapsed() < Duration::from_secs(10), "no listener: {error}");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let stop = Arc::clone(&stop);
            move || Self::run(&link, &stop)
        });
        Self { address, stop, thread: Some(thread) }
    }

    fn run(link: &LiveLink, stop: &AtomicBool) -> (Vec<u64>, u32) {
        let mut graph = AnimGraph::from_document(HEROINE).expect("the heroine's graph reads");
        graph.resolve(clip_id);
        // As the cook compiles every graph.
        let program = graph.compile_for(Target::COOKED).expect("it compiles");
        let stand_in = humanoid::Humanoid::new(program.clips.iter().map(|clip| clip.id));
        let clips = stand_in.source();
        let mut player = GraphPlayer::new(stand_in.bind(&program), stand_in.pose(), 1);

        let mut run = Player::new(Replay::decode(RUN).expect("it decodes"), &SCENARIOS).expect("it plays");
        let fighter = run.cast()[0];
        let mut checksums = vec![run.simulation().checksum()];
        let (mut stepped, mut overridden_ticks, mut cleared) = (0, 0, false);
        let mut ended = false;
        while !stop.load(Ordering::Relaxed) {
            let overridden = player.overrides().next().is_some();
            // Waiting for an override, or with one for it to go.
            let holds = (!overridden && !cleared && stepped >= STRETCH)
                || (overridden && overridden_ticks >= STRETCH);
            if !ended && !holds {
                if run.step() {
                    checksums.push(run.simulation().checksum());
                    stepped += 1;
                    overridden_ticks += u32::from(overridden);
                } else {
                    ended = true;
                }
            }
            if let Some(view) = run.simulation().actor(fighter) {
                let input = GraphInput {
                    ticks: time_ticks(view.clock_now),
                    locomotion: locomotion_of(&view),
                    combat: combat_input(&view, 0.0),
                    ..GraphInput::default()
                };
                if player.evaluate(&input, clips.as_slice(), 0) {
                    player.finish();
                }
            }
            while let Some(push) = link.poll() {
                match push {
                    Push::Status { reply } => reply.reply(ReplyBody::Status(DeviceStatus {
                        frame: u64::from(stepped),
                        ..Default::default()
                    })),
                    Push::GraphTrace { graph, reply } if graph == GRAPH => {
                        reply.reply(ReplyBody::GraphTrace(GraphTrace::of(&player, GRAPH, "the heroine", 1)));
                    }
                    Push::GraphOverride { graph, set, reply } if graph == GRAPH => match set {
                        Some((parameter, value)) => {
                            match player.set_override(Name::intern(&parameter), value) {
                                Ok(()) => reply.reply(ReplyBody::GraphOverridden { characters: 1 }),
                                Err(error) => reply.refuse(error.to_string()),
                            }
                        }
                        None => {
                            player.clear_overrides();
                            cleared = overridden_ticks > 0;
                            reply.reply(ReplyBody::GraphOverridden { characters: 1 });
                        }
                    },
                    Push::GraphTrace { graph, reply } | Push::GraphOverride { graph, reply, .. } => {
                        reply.refuse(format!("{graph}: no character plays it"));
                    }
                    _ => {}
                }
            }
            if ended && cleared {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        (checksums, overridden_ticks)
    }

    /// Waits for the run's end and returns what it says.
    fn finish(mut self) -> (Vec<u64>, u32) {
        let thread = self.thread.take().expect("running");
        let started = Stopwatch::start();
        while !thread.is_finished() {
            assert!(started.elapsed() < PATIENCE, "the run never ended");
            std::thread::sleep(Duration::from_millis(10));
        }
        thread.join().expect("the device ran")
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Polls the editor until `done` finds what it wants in the followed graph's trace.
fn until<T>(editor: &mut Editor, what: &str, done: impl Fn(&GraphTrace) -> Option<T>) -> T {
    let started = Stopwatch::start();
    loop {
        editor.poll(started.elapsed().as_secs_f64());
        if let Some(found) = editor.link().graph_trace().and_then(&done) {
            return found;
        }
        assert!(
            started.elapsed() < PATIENCE,
            "never saw {what}: trace {:?}, trouble {:?}, refusal {:?}, link {:?}",
            editor.link().graph_trace(),
            editor.link().trace_trouble(),
            editor.link().override_refusal(),
            editor.link().trouble(),
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn speed(trace: &GraphTrace) -> Option<&TracedParameter> {
    trace.parameters.iter().find(|parameter| parameter.name == "speed")
}

fn value(trace: &GraphTrace, name: &str) -> Option<f32> {
    trace.parameters.iter().find(|parameter| parameter.name == name).map(|parameter| parameter.value)
}

/// The base layer's weight on its blend's first sample, the heroine's idle.
fn idle(trace: &GraphTrace) -> Option<f32> {
    trace.layers.first()?.weights.first().copied()
}

/// The editor reads the arena fighter's trace, pins its `speed` and sees the graph move for it, has a parameter
/// it doesn't have refused, and lets it go; the run steps as it would have, tick for tick. The heroine's graph
/// names no `speed` (its conditions read `smoothed_speed`, its ground blend the smoothed velocity): the
/// override is the controller's input, which both follow.
#[test]
fn the_editor_traces_the_arena_fighters_graph_and_overrides_speed_with_no_checksum_moving() {
    let alone = checksums_alone();
    let device = Device::start();
    let mut editor = Editor::new(Options {
        file: None,
        content: None,
        link: Some(device.address),
        pipeline: None,
        game: None,
    });
    editor.run(Command::WatchGraph(Some(GRAPH.to_owned()))).expect("it follows the graph");

    // Standing, as the fighter does: its speed what the controller says, nothing overridden.
    let standing = until(&mut editor, "the fighter standing", |trace| {
        let speed = speed(trace)?;
        (trace.drawn.is_some() && !speed.overridden).then(|| trace.clone())
    });
    assert!(speed(&standing).is_some_and(|speed| speed.value < 0.5), "{standing:?}");
    assert!(idle(&standing).is_some_and(|weight| weight > 0.9), "standing is the idle: {standing:?}");

    // A parameter the graph doesn't have is refused, and leaves nothing pinned.
    let refused =
        Command::OverrideGraph { graph: GRAPH.to_owned(), parameter: "height".to_owned(), value: 1.0 };
    editor.run(refused).expect("it sends");
    let started = Stopwatch::start();
    while editor.link().override_refusal().is_none() {
        assert!(started.elapsed() < PATIENCE, "no refusal came");
        editor.poll(started.elapsed().as_secs_f64());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(editor.link().override_refusal().is_some_and(|why| why.contains("height")));

    // Pinned at a run's speed, the graph runs though the fighter stands: the smoothed speed follows the
    // override and the ground blend leaves the idle for the run's samples.
    let pinned =
        Command::OverrideGraph { graph: GRAPH.to_owned(), parameter: "speed".to_owned(), value: 5.0 };
    editor.run(pinned).expect("it sends");
    let running = until(&mut editor, "the graph running", |trace| {
        let speed = speed(trace)?;
        let smoothed = value(trace, "smoothed_speed")?;
        (speed.overridden && speed.value == 5.0 && smoothed > 4.0 && idle(trace)? < 0.1)
            .then(|| trace.clone())
    });
    assert!(running.parameters.iter().filter(|parameter| parameter.overridden).count() == 1, "{running:?}");

    editor.run(Command::ClearOverrides(GRAPH.to_owned())).expect("it sends");
    until(&mut editor, "the override gone", |trace| speed(trace).filter(|speed| !speed.overridden).map(drop));

    let (checksums, overridden) = device.finish();
    assert!(overridden > 0, "the run stepped with the override on");
    assert_eq!(checksums.len(), alone.len(), "the whole run");
    let moved = checksums.iter().zip(&alone).position(|(with, without)| with != without);
    assert_eq!(moved, None, "a checksum moved at that tick");
}
