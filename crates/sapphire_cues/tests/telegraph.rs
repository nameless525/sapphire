//! The telegraph's ground decal (M5 L7) in a real fight: it fills for exactly its wind-up, whole on the step its
//! move's strike starts and not before, and it is the telegraph mark's colour, one more reader of it.

mod support;

use std::collections::HashMap;

use sapphire_cues::{CueTable, EffectPlace};
use sapphire_gameplay::{ActorId, Simulation};
use sapphire_look::{VfxDecalBlend, VfxDecalShape, VfxFrame};
use sapphire_math::Vec3;
use sapphire_vfx::EffectLibrary;
use support::Fight;

/// Each actor's running move's playing and what had elapsed of it, in sub-ticks.
type Elapsed = HashMap<ActorId, (u32, u64)>;

/// The actors whose running move's next attack started in the step just taken, its start past what had elapsed
/// before the step and no later than what has after it, as `Stand` reads the strike; and what has elapsed now.
fn strikes(sim: &Simulation, before: &Elapsed) -> (Vec<ActorId>, Elapsed) {
    let (mut struck, mut now) = (Vec::new(), Elapsed::new());
    for view in sim.actors() {
        let Some(current) = view.moves.current else { continue };
        let elapsed = current.elapsed.as_subticks();
        now.insert(view.id, (current.serial, elapsed));
        let was =
            before.get(&view.id).filter(|(serial, _)| *serial == current.serial).map_or(0, |&(_, at)| at);
        let Some(mv) = sim.move_library().get(current.at) else { continue };
        let starts =
            mv.hitboxes.iter().filter(|hitbox| hitbox.attack.is_some()).map(|hitbox| hitbox.span.start());
        if starts.map(|start| start.as_subticks()).any(|start| start > was && start <= elapsed) {
            struck.push(view.id);
        }
    }
    (struck, now)
}

/// The fill of the telegraphs on the ground: sectors of light.
fn telegraph_fills(frame: &VfxFrame) -> Vec<f32> {
    let telegraphs = frame.decals.iter().filter(|decal| {
        decal.blend == VfxDecalBlend::Light && matches!(decal.shape, VfxDecalShape::Sector { .. })
    });
    telegraphs.map(|decal| decal.fill).collect()
}

/// Over the whole arena recording, every telegraph on the ground fills to whole on the step its warrior's
/// strike starts and is short of whole the step before: the fill's length is the wind-up's, on the actor's own
/// clock, from the cue to the strike. A telegraph that never reaches its strike (its move cut short) never fills.
#[test]
fn a_telegraph_fills_on_the_step_its_strike_starts() {
    let mut fight = Fight::new("arena-fight-still");
    let mut frame = VfxFrame::default();
    let (mut elapsed, mut previous) = (Elapsed::new(), Vec::new());
    let (mut struck_with_telegraph, mut checked) = (0, 0);
    while fight.step().is_some() {
        let sim = fight.player.simulation();
        let (struck, now) = strikes(sim, &elapsed);
        elapsed = now;
        fight.vfx.write_frame(&mut frame, 1.0);
        let fills = telegraph_fills(&frame);
        let whole = fills.iter().filter(|&&fill| fill >= 1.0).count();
        let was_short = previous.iter().any(|&fill: &f32| fill < 1.0);
        // A telegraph turns whole only on a strike's step, and a strike's step turns one whole.
        let turned_whole = whole > previous.iter().filter(|&&fill: &&f32| fill >= 1.0).count();
        if turned_whole {
            assert!(
                !struck.is_empty(),
                "tick {}: a telegraph turned whole on no strike: {fills:?}",
                sim.tick()
            );
            assert!(was_short, "tick {}: and was short of it the step before: {previous:?}", sim.tick());
            checked += 1;
        }
        if !struck.is_empty() && was_short {
            assert!(turned_whole, "tick {}: a strike started and its telegraph stayed {fills:?}", sim.tick());
            struck_with_telegraph += 1;
        }
        previous = fills;
    }
    assert!(checked > 0, "the recording telegraphs");
    assert_eq!(checked, struck_with_telegraph, "each telegraph that filled, filled on its strike");
}

/// The telegraph's cue lays its decal on the ground on its subject's clock, beside its mark over the head, and
/// the decal shows the mark's colour: its colour at its brightest, scaled so its strongest channel is 1, is the
/// mark's at its start.
#[test]
fn the_telegraphs_decal_is_its_marks_colour() {
    let (table, effects) = (CueTable::default_table(), EffectLibrary::default_effects());
    let row = table.rows().iter().find(|row| row.cue == "telegraph" && row.weight.is_none()).expect("a row");
    let effect = |at: EffectPlace| {
        let fire = row.effects.iter().find(|fire| fire.at == at).unwrap_or_else(|| panic!("fires at {at:?}"));
        effects.find(&fire.effect).and_then(|id| effects.effect(id)).expect("an effect")
    };
    let shown = |color: [f32; 3]| {
        let color = Vec3::from(color);
        color / color.max_element()
    };
    let mark = shown(effect(EffectPlace::Crown).emitters[0].color.sample(0.0));
    let ground = effect(EffectPlace::Ground);
    let [decal] = &ground.decals[..] else { panic!("one decal: {:?}", ground.decals) };
    assert!(decal.until_strike, "it fills until the strike");
    assert!((shown(decal.color.sample(0.0)) - mark).abs().max_element() < 1e-3, "{mark}");
}
