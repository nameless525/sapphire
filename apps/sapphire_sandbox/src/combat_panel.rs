//! What the overlay's combat panel shows (M2 T1): a fight as `sapphire_debug::CombatSnapshot`, plain
//! data, built here so that crate reads no simulation (`sapphire_debug`'s `combat` module says why).
//!
//! Only while the panel shows: it's text and lists, rebuilt every frame.

use sapphire_core::Name;
use sapphire_core::debug_draw::Color;
use sapphire_debug::{
    ActorSnapshot, CombatSnapshot, IntentSnapshot, MoveSnapshot, RecorderSnapshot, RecorderState,
    TrackSnapshot,
};
use sapphire_gameplay::replay::SCENARIOS;
use sapphire_gameplay::{ActorView, CurrentMove, LiveWindows, Simulation};
use sapphire_moves::{Move, MoveSet, Phase, Span};
use sapphire_time::{SUBTICKS_PER_TICK, SimDuration};

use crate::combat::{Combat, CombatStatus, SessionRecorder};
use crate::combat_views::{BodyState, palette};

/// Colours for what only the panel shows, so a cancel isn't taken for a state of the hurtbox.
const CANCEL: Color = Color::BLUE;
const WARP: Color = Color::ORANGE;
const ARMOUR: Color = Color::rgb(90, 200, 200);
const CUE: Color = Color::WHITE;

/// The fight as it stands, and the recorder. `can_record` says whether someone's there to play.
pub(crate) fn snapshot(
    combat: Option<&Combat>,
    recorder: &SessionRecorder,
    can_record: bool,
) -> CombatSnapshot {
    let sim = combat.map(Combat::simulation);
    CombatSnapshot {
        tick: sim.map_or(0, Simulation::tick),
        actors: sim.map_or_else(Vec::new, |sim| sim.actors().map(|actor| actor_of(sim, &actor)).collect()),
        recorder: RecorderSnapshot {
            state: combat.map_or(RecorderState::Idle, recorder_state),
            scenarios: SCENARIOS.iter().map(|scenario| scenario.name.to_owned()).collect(),
            can_record,
            note: recorder.note().map(str::to_owned),
        },
    }
}

fn recorder_state(combat: &Combat) -> RecorderState {
    let tick = combat.simulation().tick();
    match combat.status() {
        CombatStatus::Replaying { name, length } => {
            RecorderState::Replaying { name: name.to_owned(), tick, ticks: length }
        }
        CombatStatus::Recording { scenario, path, at_limit, scripted } => RecorderState::Recording {
            scenario: scenario.to_owned(),
            path: path.display().to_string(),
            ticks: tick,
            at_limit,
            scripted,
        },
    }
}

fn ticks_of(duration: SimDuration) -> f32 {
    duration.as_subticks() as f32 / SUBTICKS_PER_TICK as f32
}

fn actor_of(sim: &Simulation, actor: &ActorView) -> ActorSnapshot {
    let library = sim.move_library();
    let set = actor.moves.set.and_then(|set| library.current(set));
    let (intents, held) = sim.intents(actor.id).map_or_else(Default::default, |ring| {
        let intents = ring
            .iter()
            .map(|intent| IntentSnapshot {
                name: set
                    .and_then(|set| intent_label(set, intent.name))
                    .unwrap_or_else(|| name_of(intent.name)),
                priority: intent.priority,
                age_ticks: ticks_of(actor.clock_now.since(intent.pressed_at)),
                left_ticks: ticks_of(intent.expires_at.since(actor.clock_now)),
            })
            .collect();
        (intents, set.map_or_else(Vec::new, |set| held_of(set, ring.held())))
    });
    let current = actor.moves.current.and_then(|current| {
        let mv = library.get(current.at)?;
        Some(move_of(mv, &current))
    });
    let state = BodyState::of(&actor.moves);
    ActorSnapshot {
        id: actor.id.raw(),
        clock_ticks: actor.clock_now.as_subticks() as f32 / SUBTICKS_PER_TICK as f32,
        hitstop_ticks: u32::from(actor.hitstop_ticks),
        body: (body_text(state), state.color()),
        windows: windows_of(actor.moves.windows),
        intents,
        held,
        current,
    }
}

fn intent_label(set: &MoveSet, name: Name) -> Option<String> {
    set.intent(name).map(|(_, intent)| intent.label().to_owned())
}

fn held_of(set: &MoveSet, held: u16) -> Vec<String> {
    let bits = set.intents().iter().enumerate();
    bits.filter(|&(bit, _)| held & (1 << bit) != 0).map(|(_, intent)| intent.label().to_owned()).collect()
}

const fn body_text(state: BodyState) -> &'static str {
    match state {
        BodyState::Parry => "parrying",
        BodyState::PerfectDodge => "perfect dodge",
        BodyState::Off => "can't be hit",
        BodyState::Hurtbox => "can be hit",
    }
}

fn windows_of(windows: LiveWindows) -> Vec<(&'static str, Color)> {
    let LiveWindows { invulnerable, perfect_dodge, parry, cancel, warp, armour } = windows;
    [
        (invulnerable, "invulnerable", palette::OFF),
        (perfect_dodge, "perfect dodge", palette::PERFECT_DODGE),
        (parry, "parry", palette::PARRY),
        (cancel, "cancel", CANCEL),
        (warp, "warp", WARP),
        (armour > 0, "armour", ARMOUR),
    ]
    .into_iter()
    .filter(|(live, ..)| *live)
    .map(|(_, name, color)| (name, color))
    .collect()
}

fn move_of(mv: &Move, current: &CurrentMove) -> MoveSnapshot {
    MoveSnapshot {
        name: mv.label().to_owned(),
        serial: current.serial,
        phase: match current.phase {
            Phase::Startup => "startup",
            Phase::Active => "active",
            Phase::Recovery => "recovery",
        },
        elapsed_ticks: ticks_of(current.elapsed),
        startup: mv.startup,
        active: mv.active,
        recovery: mv.recovery,
        tracks: tracks_of(mv, current.elapsed),
    }
}

/// `name`'s text. Only debug builds keep a name's text, so elsewhere this is its hash, and a track is
/// better known by what kind it is.
fn name_of(name: Name) -> String {
    name.to_string()
}

fn known_or(name: Name, kind: &str) -> String {
    let text = name_of(name);
    if text.starts_with("Name(") { kind.to_owned() } else { text }
}

/// Every window of `mv`, in the order a designer reads them: what it hits with, what can be done to
/// it, what it can become, then its cues.
fn tracks_of(mv: &Move, elapsed: SimDuration) -> Vec<TrackSnapshot> {
    let mut tracks = Vec::new();
    let mut add = |name: String, span: Span, color: Color| {
        tracks.push(TrackSnapshot {
            name,
            from: span.from,
            to: span.to,
            live: span.contains(elapsed),
            color,
        });
    };
    for hitbox in &mv.hitboxes {
        add(known_or(hitbox.name(), "hit"), hitbox.span, palette::HIT);
    }
    for change in &mv.hurtboxes {
        add(known_or(change.name(), "hurtbox"), change.span, palette::OFF);
    }
    let spans: [(&str, &[Span], Color); 3] = [
        ("invulnerable", &mv.invulnerable, palette::OFF),
        ("perfect dodge", &mv.perfect_dodge, palette::PERFECT_DODGE),
        ("parry", &mv.parry, palette::PARRY),
    ];
    for (name, spans, color) in spans {
        for span in spans {
            add(name.to_owned(), *span, color);
        }
    }
    for window in &mv.cancels {
        add("cancel".to_owned(), window.span, CANCEL);
    }
    for warp in &mv.warps {
        add("warp".to_owned(), warp.span, WARP);
    }
    for cue in &mv.cues {
        let span = Span { from: cue.at, to: cue.at };
        add(known_or(cue.name(), "cue"), span, CUE);
    }
    tracks
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sapphire_gameplay::replay::find_scenario;
    use sapphire_gameplay::{ActorActions, Buttons, TickActions};

    use super::*;
    use crate::options::CombatOption;

    fn recorder() -> SessionRecorder {
        SessionRecorder::new(std::env::temp_dir().join("sapphire-sandbox-combat-panel-unused"))
    }

    /// The test content's duel, as a replay of it stands, with `actions` through `ticks` steps.
    fn fight(ticks: u32, actions: impl FnMut(u64, &[sapphire_gameplay::ActorId]) -> TickActions) -> Combat {
        let path =
            std::env::temp_dir().join(format!("sapphire-sandbox-panel-{}.sreplay", std::process::id()));
        let option = CombatOption::Record { path, scenario: "duel".to_owned(), script: None };
        let mut combat = Combat::new(&option).expect("sets up");
        let mut actions = actions;
        for _ in 0..ticks {
            combat.step(&mut actions);
        }
        combat
    }

    fn pressing(buttons: Buttons, at: u64) -> impl FnMut(u64, &[sapphire_gameplay::ActorId]) -> TickActions {
        move |tick, cast| {
            let mut actions = TickActions::NEUTRAL;
            if tick == at {
                let pressed = ActorActions { buttons, ..ActorActions::NEUTRAL };
                actions.set(cast[0], pressed).expect("room");
            }
            actions
        }
    }

    /// A dodge 3 ticks in: `invulnerable: 1..14`, `perfect_dodge: 1..7`, its hurtbox off for the same.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_dodge_reads_as_its_windows_and_its_tracks_say() {
        let combat = fight(4, pressing(Buttons::DODGE, 0));
        let snapshot = snapshot(Some(&combat), &recorder(), true);
        assert_eq!(snapshot.tick, 4);
        assert_eq!(snapshot.actors.len(), 2);
        let actor = &snapshot.actors[0];
        assert_eq!((actor.id, actor.hitstop_ticks), (1, 0));
        assert_eq!(actor.body, ("perfect dodge", palette::PERFECT_DODGE));
        assert!(actor.windows.contains(&("invulnerable", palette::OFF)), "{:?}", actor.windows);
        assert!(actor.windows.contains(&("perfect dodge", palette::PERFECT_DODGE)), "{:?}", actor.windows);

        let current = actor.current.as_ref().expect("dodging");
        assert_eq!((current.name.as_str(), current.phase, current.serial), ("dodge", "active", 1));
        assert_eq!(current.elapsed_ticks, 3.0);
        let track = |name: &str| {
            current.tracks.iter().find(|track| track.name == name).unwrap_or_else(|| panic!("{name}"))
        };
        let invulnerable = track("invulnerable");
        assert_eq!((invulnerable.from, invulnerable.to, invulnerable.live), (1, 14, true));
        let perfect = track("perfect dodge");
        assert_eq!((perfect.from, perfect.to, perfect.live), (1, 7, true));

        // The other just stands there.
        let other = &snapshot.actors[1];
        assert_eq!(other.body, ("can be hit", palette::HURTBOX));
        assert!(other.current.is_none() && other.windows.is_empty() && other.intents.is_empty());
    }

    /// A press mid-move waits in the ring, with its age and what's left of it on the owner's clock.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_buffered_press_shows_its_age_and_whats_left() {
        // Light's first hit starts on the first step, and a second press waits for its cancel window.
        let mut pressed = pressing(Buttons::LIGHT, 0);
        let mut second = pressing(Buttons::LIGHT, 2);
        let combat = fight(5, move |tick, cast| {
            let first = pressed(tick, cast);
            if first == TickActions::NEUTRAL { second(tick, cast) } else { first }
        });
        let snapshot = snapshot(Some(&combat), &recorder(), true);
        let actor = &snapshot.actors[0];
        assert_eq!(actor.current.as_ref().expect("swinging").name, "light_1");
        assert_eq!(actor.intents.len(), 1, "{:?}", actor.intents);
        let intent = &actor.intents[0];
        assert_eq!((intent.name.as_str(), intent.priority), ("light", 1));
        // Pressed on the third step, two steps ago, with a buffer of 12.
        assert_eq!((intent.age_ticks, intent.left_ticks), (2.0, 10.0));
    }

    /// What's held is named through the set's intents, as a press is.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_held_button_is_named() {
        let combat = fight(2, |_, cast: &[sapphire_gameplay::ActorId]| {
            let mut actions = TickActions::NEUTRAL;
            let held = ActorActions { buttons: Buttons::SKILL, ..ActorActions::NEUTRAL };
            actions.set(cast[0], held).expect("room");
            actions
        });
        let snapshot = snapshot(Some(&combat), &recorder(), true);
        assert_eq!(snapshot.actors[0].held, ["skill"]);
    }

    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn every_track_of_the_test_content_is_listed_with_its_window() {
        let scenario = find_scenario(SCENARIOS, "duel").expect("there");
        let (sim, cast) = scenario.start(scenario.config).expect("sets up");
        let set = sim.move_library().current(sim.actor(cast[0]).unwrap().moves.set.unwrap()).unwrap();
        let (_, skill) = set.find(Name::new("skill")).expect("a skill");
        let tracks = tracks_of(skill, SimDuration::from_ticks(3));
        let count = skill.hitboxes.len()
            + skill.hurtboxes.len()
            + skill.invulnerable.len()
            + skill.perfect_dodge.len()
            + skill.parry.len()
            + skill.cancels.len()
            + skill.warps.len()
            + skill.cues.len();
        assert_eq!(tracks.len(), count);
        let parry = tracks.iter().find(|track| track.name == "parry").expect("a skill parries");
        assert_eq!((parry.from, parry.to, parry.live, parry.color), (2, 6, true, palette::PARRY));
    }

    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn the_recorder_reads_as_what_is_running() {
        let idle = snapshot(None, &recorder(), true);
        assert_eq!(idle.recorder.state, RecorderState::Idle);
        assert!(idle.actors.is_empty());
        assert_eq!(idle.tick, 0);
        assert!(idle.recorder.scenarios.iter().any(|name| name == "duel"));
        assert!(idle.recorder.can_record);

        let combat = fight(3, |_, _: &[sapphire_gameplay::ActorId]| TickActions::NEUTRAL);
        let running = snapshot(Some(&combat), &recorder(), false);
        let RecorderState::Recording { scenario, path, ticks, at_limit, scripted } = running.recorder.state
        else {
            panic!("{:?}", running.recorder.state);
        };
        assert_eq!((scenario.as_str(), ticks, at_limit, scripted), ("duel", 3, false, false));
        assert!(Path::new(&path).extension().is_some_and(|extension| extension == "sreplay"), "{path}");
        assert!(!running.recorder.can_record);
    }
}
