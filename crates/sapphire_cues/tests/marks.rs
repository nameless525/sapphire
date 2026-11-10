//! Cues by shape (M4 A5, m4-plan §3 "Colour"): what a fight warns of reads whatever colours a player
//! sees. Each warning is a mark over a fighter, and each pair of them stays apart under protanopia,
//! deuteranopia and tritanopia, as Machado et al. (2009) simulate them.
//!
//! **Apart** is measured: two marks are apart under a dichromacy if their simulated colours are at
//! least [`APART`] from each other in Oklab, or if their shapes share less than [`SHAPES_APART`] of
//! their outlines. The marks hold the stronger rule too, that every pair differs by shape, so colour is
//! never what alone tells them apart; the colour distances are measured and printed, to say where
//! colour alone would have failed.

mod support;

use sapphire_core::Name;
use sapphire_cues::{CueMarks, CueTable, DODGE_WINDOW, EffectPlace, NAMED_PHASE};
use sapphire_gameplay::ActorId;
use sapphire_look::VfxShape;
use sapphire_look::color_vision::{APART, Dichromacy, distance};
use sapphire_look::vfx::SHAPES_APART;
use sapphire_math::Vec3;
use sapphire_vfx::{EffectLibrary, Facing};
use support::Fight;

/// A mark as a player sees it: its shape, and its colour at its brightest, scaled so its strongest
/// channel is display white (how an emissive colour past 1 shows after the tone curve, near enough
/// for its hue).
#[derive(Debug)]
struct Mark {
    effect: String,
    shape: VfxShape,
    color: Vec3,
}

/// Its colour as it shows: the brightest channel at 1.
fn shown(color: [f32; 3]) -> Vec3 {
    let color = Vec3::from(color);
    color / color.max_element()
}

/// The mark `cue`'s row fires: one effect, over its subject's head, a mark with one upright emitter.
fn mark_of(table: &CueTable, effects: &EffectLibrary, cue: &str) -> Mark {
    let row = table.rows().iter().find(|row| row.cue == cue && row.weight.is_none());
    let row = row.unwrap_or_else(|| panic!("`{cue}` has a row"));
    let [fire] = &row.effects[..] else { panic!("`{cue}` fires one effect: {:?}", row.effects) };
    assert_eq!(fire.at, EffectPlace::Crown, "`{cue}` marks over its subject's head");
    let effect = effects.find(&fire.effect).and_then(|id| effects.effect(id));
    let effect = effect.unwrap_or_else(|| panic!("`{}` is an effect", fire.effect));
    assert!(effect.mark, "`{}` draws a mark's random numbers", fire.effect);
    let [emitter] = &effect.emitters[..] else { panic!("`{}` is one mark", fire.effect) };
    assert_eq!(emitter.facing, Facing::Upright, "`{}` stands upright, so its shape reads", fire.effect);
    Mark { effect: fire.effect.clone(), shape: emitter.shape.into(), color: shown(emitter.color.sample(0.0)) }
}

/// Every warning a fight gives has its mark: a heavy's wind-up, a perfect dodge's window, and an enemy
/// crossing a named phase.
fn marks() -> Vec<Mark> {
    let (table, effects) = (CueTable::default_table(), EffectLibrary::default_effects());
    ["telegraph", DODGE_WINDOW, NAMED_PHASE].into_iter().map(|cue| mark_of(&table, &effects, cue)).collect()
}

/// What [`CueMarks`] fires over a whole recording: each cue's tick and actor, and the ticks the simulation
/// published `also` on.
fn marks_fired(name: &str, also: &str) -> (Vec<(u64, Name, ActorId)>, Vec<u64>) {
    let mut fight = Fight::new(name);
    let mut marks = CueMarks::default();
    let (mut fired, mut seen) = (Vec::new(), Vec::new());
    while fight.player.step() {
        let sim = fight.player.simulation();
        let tick = sim.tick();
        marks.read(sim, fight.player.cast().first().copied(), |cue, actor| fired.push((tick, cue, actor)));
        seen.extend(sim.cues().filter(|cue| cue.cue == Name::new(also)).map(|_| tick));
    }
    (fired, seen)
}

#[test]
fn every_pair_of_marks_stays_apart_under_each_dichromacy() {
    let marks = marks();
    for (index, a) in marks.iter().enumerate() {
        for b in &marks[index + 1..] {
            let overlap = a.shape.overlap(b.shape);
            let by_shape = overlap < SHAPES_APART;
            assert!(
                by_shape,
                "{} and {} differ by shape, not colour alone: they share {overlap}",
                a.effect, b.effect
            );
            for dichromacy in Dichromacy::ALL {
                let seen = distance(dichromacy.simulate(a.color), dichromacy.simulate(b.color));
                eprintln!(
                    "{} / {} under {}: colour {seen:.3}, shapes share {overlap:.2}",
                    a.effect,
                    b.effect,
                    dichromacy.name()
                );
                assert!(
                    seen >= APART || by_shape,
                    "{} and {} under {}",
                    a.effect,
                    b.effect,
                    dichromacy.name()
                );
            }
        }
    }
}

/// Two fighters' trails share a shape, a crescent off the blade, and are told apart by whose blade draws
/// them; their colours stay apart under each dichromacy as well.
#[test]
fn the_players_and_a_warriors_trails_stay_apart_by_colour() {
    let effects = EffectLibrary::default_effects();
    let glow = |name: &str| {
        let effect = effects.find(name).and_then(|id| effects.effect(id)).expect("a trail");
        shown(effect.trail.as_ref().expect("a trail").color.sample(0.0))
    };
    let (player, warrior) = (glow("blade_trail"), glow("warrior_trail"));
    for dichromacy in Dichromacy::ALL {
        let seen = distance(dichromacy.simulate(player), dichromacy.simulate(warrior));
        eprintln!("blade_trail / warrior_trail under {}: colour {seen:.3}", dichromacy.name());
        assert!(seen >= APART, "under {}: {seen}", dichromacy.name());
    }
}

/// The boss's recording crosses the phase where it enrages: the mark fires once, on the boss, on the step
/// the simulation says the phase's name, with no row of the table naming it.
#[test]
fn a_named_phase_is_marked_on_the_enemy_that_crossed_it() {
    let (fired, enraged) = marks_fired("arena-boss", "enraged");
    let phases: Vec<_> = fired.iter().filter(|(_, cue, _)| *cue == Name::new(NAMED_PHASE)).collect();
    assert_eq!(enraged.len(), 1, "the recording crosses it once");
    assert_eq!(phases.len(), 1, "{phases:?}");
    assert_eq!(phases[0].0, enraged[0], "on the step it crossed");
    assert!(!CueTable::default_table().rows().iter().any(|row| row.cue == "enraged"), "no row of its own");
}

/// Every perfect dodge in the recording that dodges and counters had its window marked first, at most a
/// second before, and no strike is announced twice on one step.
#[test]
fn every_perfect_dodge_had_its_window_marked_first() {
    let (fired, perfect) = marks_fired("dodge-counter", "perfect_dodge");
    let windows: Vec<_> = fired.iter().filter(|(_, cue, _)| *cue == Name::new(DODGE_WINDOW)).collect();
    assert!(!perfect.is_empty(), "the recording dodges perfectly");
    for &tick in &perfect {
        let before = windows.iter().any(|&&(at, ..)| at <= tick && tick - at <= 60);
        assert!(before, "the perfect dodge at {tick} had no window marked first: {windows:?}");
    }
    for pair in windows.windows(2) {
        assert!(pair[0].0 != pair[1].0 || pair[0].2 != pair[1].2, "announced twice: {pair:?}");
    }
}
