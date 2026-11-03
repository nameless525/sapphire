//! A fight played through the sandbox's own input, as `--arena --fight` plays one in a window, short of the
//! window.

use std::path::{Path, PathBuf};
use std::{fs, process};

use sapphire_gameplay::replay::{Replay, SCENARIOS};
use sapphire_gameplay::{ActorActions, Buttons};
use sapphire_input::RawInput;
use sapphire_platform::{Scancode, Size, Timestamp};
use sapphire_stage::combat::{Combat, CombatOption, CombatStatus, no_bindings, read_bindings};

use super::SandboxInput;

/// A file name of this process's own in the temp directory, deleted again when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let file = format!("sapphire-sandbox-input-{}-{name}.sreplay", process::id());
        let path = std::env::temp_dir().join(file);
        let _ = fs::remove_file(&path);
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn recording(path: &Path, script: Option<&str>) -> Combat {
    let option = CombatOption::Record {
        path: path.to_owned(),
        scenario: SCENARIOS[0].name.to_owned(),
        script: script.map(str::to_owned),
    };
    Combat::new(&option).expect("sets up")
}

fn fighting(script: Option<&str>) -> Combat {
    Combat::new(&CombatOption::Fight { script: script.map(str::to_owned), party: false }).expect("sets up")
}

/// A fight played on the keyboard, through the sandbox's own input, reaches the file and plays
/// back to the same checksums.
#[test]
fn a_player_is_recorded_through_the_bindings() {
    let scratch = Scratch::new("played");
    let mut combat = recording(&scratch.0, None);
    let cast = SCENARIOS[0].start(SCENARIOS[0].config).expect("sets up").1;
    let mut input = SandboxInput::new(false, true, Size { width: 640, height: 360 }).expect("binds");
    let frames: [&[(Scancode, bool)]; 4] = [
        &[(Scancode::W, true)],
        &[(Scancode::J, true), (Scancode::J, false)],
        &[(Scancode::W, false)],
        &[],
    ];
    // Looking along world +X, so forward is +x in the file.
    let camera_yaw = std::f32::consts::FRAC_PI_2;
    for (index, keys) in frames.iter().enumerate() {
        let frame = index as u64 * 16;
        for (offset, &(scancode, down)) in keys.iter().enumerate() {
            input.push(frame + 1 + offset as u64, RawInput::Key { scancode, down });
        }
        assert!(!input.update(Timestamp::from_nanos((frame + 16) * 1_000_000)));
        assert!(combat.step(&mut input.source(camera_yaw)), "it asks the bindings");
    }
    combat.finish().expect("writes");

    let replay = Replay::decode(&fs::read(&scratch.0).expect("written")).expect("reads");
    let read: Vec<_> = replay.actions().map(|tick| tick.get(cast[0])).collect();
    let forward = sapphire_gameplay::Stick::new(127, 0);
    assert_eq!(
        read,
        [
            ActorActions { buttons: Buttons::NONE, stick: forward },
            ActorActions { buttons: Buttons::LIGHT, stick: forward },
            ActorActions::NEUTRAL,
            ActorActions::NEUTRAL,
        ]
    );

    let mut played = Combat::new(&CombatOption::Replay { path: scratch.0.clone(), to_end: true })
        .expect("a recording reads as a replay");
    for _ in 0..read.len() {
        assert!(!played.step(&mut no_bindings), "a replay asks no one");
    }
    played.finish().expect("nothing recorded to differ from");
}

/// What `--arena --fight` does in a window, short of the window: the keys go through the sandbox's
/// own input to the same step, the player's verbs land, the brain thinks, nothing's written, and
/// the key that starts it over makes the same first tick again.
#[test]
fn a_fight_is_played_through_the_bindings_and_starts_over_on_its_key() {
    let mut combat = fighting(None);
    assert_eq!(combat.scenario(), sapphire_stage::arena::FIGHT_SCENARIO);
    sapphire_stage::arena::check_scenario(&combat, false).expect("it's the arena's");
    assert!(sapphire_stage::arena::check_scenario(&combat, true).is_err(), "it isn't a party's");
    let cast = combat.cast().to_vec();
    let at_start = combat.simulation().checksum();
    let enemy = |combat: &Combat| combat.simulation().actor(cast[1]).expect("alive");
    let player = |combat: &Combat| combat.simulation().actor(cast[0]).expect("alive");
    assert!(enemy(&combat).brain.is_on(), "someone's thinking for it");

    let mut input = SandboxInput::new(false, true, Size { width: 640, height: 360 }).expect("binds");
    input.listen_for_fight_restart().expect("binds");
    let mut now = 0;
    let mut frame = |input: &mut SandboxInput, combat: &mut Combat, keys: &[(Scancode, bool)]| {
        for &(scancode, down) in keys {
            now += 1;
            input.push(now, RawInput::Key { scancode, down });
        }
        now += 16;
        assert!(!input.update(Timestamp::from_nanos(now * 1_000_000)));
        let actions = input.take_actions();
        if actions.restart_fight {
            combat.restart();
        }
        read_bindings(Some(combat), &mut input.source(0.0), cast[0]);
    };

    frame(&mut input, &mut combat, &[(Scancode::J, true)]);
    let current = player(&combat).moves.current.expect("it started a move");
    assert_eq!(current.name, sapphire_core::Name::new("light_1"));
    frame(&mut input, &mut combat, &[(Scancode::J, false)]);
    for _ in 0..30 {
        frame(&mut input, &mut combat, &[]);
    }
    assert_ne!(enemy(&combat).position, sapphire_gameplay::arena::spawns().enemy.position(), "it walked");
    let CombatStatus::Fighting { fights } = combat.status() else { panic!("{:?}", combat.status()) };
    assert_eq!(fights, 1);

    frame(&mut input, &mut combat, &[(Scancode::R, true)]);
    // The key lands between frames, and that frame's step is the new fight's first.
    assert_eq!(combat.simulation().tick(), 1);
    let CombatStatus::Fighting { fights } = combat.status() else { panic!("{:?}", combat.status()) };
    assert_eq!(fights, 2);
    let mut fresh = fighting(None);
    fresh.step(&mut no_bindings);
    assert_eq!(combat.simulation().checksum(), fresh.simulation().checksum());
    assert_ne!(at_start, combat.simulation().checksum(), "a tick on");
    combat.finish().expect("nothing to write");
}
