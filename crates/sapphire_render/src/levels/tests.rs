use super::*;

/// The cook's default chain for a mesh 1 m across (spec §5.27): LOD0, then 0.2 cm, 1 cm and 3 cm.
const CHAIN: [f32; 4] = [0.0, 0.002, 0.01, 0.03];

#[test]
fn the_coarsest_level_within_the_budget_draws() {
    assert_eq!(coarsest_within(&CHAIN, 0.0), 0, "only LOD0 is exact");
    assert_eq!(coarsest_within(&CHAIN, 0.001_999), 0);
    assert_eq!(coarsest_within(&CHAIN, 0.002), 1, "an error at the budget is within it");
    assert_eq!(coarsest_within(&CHAIN, 0.029), 2);
    assert_eq!(coarsest_within(&CHAIN, 1.0), 3);
    // A mobile cook skips the first, so its LOD0 strays too: still the floor.
    assert_eq!(coarsest_within(&[0.002, 0.01], 0.0), 0);
    // A budget that isn't a number takes none; an error that isn't finite is never within one.
    assert_eq!(coarsest_within(&CHAIN, f32::NAN), 0);
    assert_eq!(coarsest_within(&[0.0, f32::NAN, f32::INFINITY], f32::MAX), 0);
}

#[test]
fn a_zero_error_chain_and_a_single_level() {
    // Levels that stray by nothing are good enough at any distance, the camera inside them too.
    assert_eq!(coarsest_within(&[0.0, 0.0, 0.0], 0.0), 2);
    assert_eq!(choose(&[0.0, 0.0, 0.0], 0.0, Some(0)), 2);
    // One level: always it.
    assert_eq!(coarsest_within(&[0.0], 1e9), 0);
    assert_eq!(choose(&[0.0], 1e9, Some(3)), 0, "a held level past the chain clamps");
    assert_eq!(coarsest_within(&[], 1.0), 0, "no levels: 0");
}

#[test]
fn the_fresh_choice_is_monotonic_in_the_budget() {
    // A budget grows with distance (meters per pixel); the level never goes finer as it grows.
    let mut last = 0;
    for step in 0..=400 {
        let budget = step as f32 * 1e-4;
        let level = choose(&CHAIN, budget, None);
        assert!(level >= last, "{budget}: {level} after {last}");
        last = level;
    }
    assert_eq!(last, 3);
}

#[test]
fn the_held_level_moves_only_past_the_band() {
    // Level 2 (1 cm) is good enough from a 1 cm budget, but a held 1 takes it only from 1.25 cm.
    assert_eq!(choose(&CHAIN, 0.011, None), 2, "fresh");
    assert_eq!(choose(&CHAIN, 0.011, Some(1)), 1, "held inside the band");
    assert_eq!(choose(&CHAIN, 0.013, Some(1)), 2, "past it");
    // A held 2 stays while 2 is good enough at all, and goes finer the moment it isn't.
    assert_eq!(choose(&CHAIN, 0.01, Some(2)), 2);
    assert_eq!(choose(&CHAIN, 0.009_99, Some(2)), 1);
    // A held level finer than the fresh one by two goes coarser past the band of the coarsest.
    assert_eq!(choose(&CHAIN, 0.04, Some(1)), 3);
    assert_eq!(choose(&CHAIN, 0.036, Some(1)), 2, "3 needs 3.75 cm held; 2 needs 1.25 cm");
}

#[test]
fn a_cascade_chooses_by_its_texel_and_the_stretch() {
    // A quarter of a 10 cm texel: 2.5 cm of error.
    assert_eq!(cascade_level(&CHAIN, 1.0, 0.1, 0.0), 2);
    // Twice as big: its errors are twice as big in the world.
    assert_eq!(cascade_level(&CHAIN, 2.0, 0.1, 0.0), 2);
    assert_eq!(cascade_level(&CHAIN, 4.0, 0.1, 0.0), 1);
    // A bias of 1 doubles the share; a near cascade's texels keep LOD0.
    assert_eq!(cascade_level(&CHAIN, 1.0, 0.1, 1.0), 3);
    assert_eq!(cascade_level(&CHAIN, 1.0, 0.005, 0.0), 0);
}

#[test]
fn the_settings_check_the_bias_and_force_levels() {
    assert_eq!(LevelSettings::ON.validate(), Ok(()));
    for bias in [4.5, -4.5, f32::NAN, f32::INFINITY] {
        let settings = LevelSettings { bias, ..LevelSettings::ON };
        assert_eq!(settings.validate().map_err(|(field, _)| field), Err("levels.bias"), "{bias}");
    }
    assert_eq!(LevelSettings::ON.forced(4), None);
    assert_eq!(LevelSettings { mode: LevelsMode::Off, bias: 0.0 }.forced(4), Some(0));
    assert_eq!(LevelSettings { mode: LevelsMode::Fixed(2), bias: 0.0 }.forced(4), Some(2));
    assert_eq!(LevelSettings { mode: LevelsMode::Fixed(9), bias: 0.0 }.forced(4), Some(3), "its coarsest");
    assert!((allowance(0.5) - 2f32.sqrt()).abs() < 1e-6);
    assert_eq!(LevelStats { coarser: 2, finer: 3 }.switches(), 5);
}

/// A street's chain: the district's levels, 15, 40 and 100 mm.
const STREET_CHAIN: [f32; 4] = [0.0, 0.015, 0.04, 0.1];

/// A thousand instances of [`STREET_CHAIN`] from 2 to 200 m away, log-uniform.
fn street() -> Vec<f32> {
    (0..1000).map(|i| 2.0 * 100f32.powf(i as f32 / 999.0)).collect()
}

/// One frame of the main view over `street`'s instances, each `scale` times as far, at `bias`, seen
/// at 1080 rows through 0.8 rad: each chooses from the level it held, which it then holds. Returns
/// how many switched.
fn frame(levels: &mut [u8], street: &[f32], scale: f32, bias: f32) -> usize {
    let meters_per_pixel = 2.0 * 0.4f32.tan() / 1080.0;
    let mut switched = 0;
    for (level, distance) in levels.iter_mut().zip(street) {
        let budget = allowance(bias) * meters_per_pixel * distance * scale;
        let next = choose(&STREET_CHAIN, budget, Some(*level));
        switched += usize::from(next != *level);
        *level = next;
    }
    switched
}

/// `street`'s levels after the view has held still at `bias` until nothing switches.
fn settled(street: &[f32], bias: f32) -> Vec<u8> {
    let mut levels = vec![0; street.len()];
    for _ in 0..8 {
        if frame(&mut levels, street, 1.0, bias) == 0 {
            return levels;
        }
    }
    panic!("the street settles at {bias}");
}

#[test]
fn a_rung_bias_switches_no_more_than_a_dolly_does() {
    let street = street();
    // A dolly away by the ramp's step, 2^(1/32) or 2.2% (a doubling in 32 steps): what a camera's
    // step switches. Within half again: the grain of a thousand instances on three thresholds.
    let dolly = frame(&mut settled(&street, 0.0), &street, RUNG_BIAS_STEP.exp2(), 0.0);
    assert!(dolly > 0, "the dolly switches some");
    let most = dolly * 3 / 2;
    // The rung taken, then left, each frame at the bias it applies.
    let mut levels = settled(&street, 0.0);
    let mut rung = RungBias::default();
    let mut total = 0;
    for target in [1.0, 0.0] {
        rung.set(target);
        for at in 0..40 {
            rung.step();
            let switched = frame(&mut levels, &street, 1.0, rung.applied());
            assert!(switched <= most, "to {target}, frame {at}: {switched} switch, a dolly's {dolly}");
            total += switched;
        }
    }
    assert!(total > 10 * dolly, "the rung switches many over its frames: {total}");
}

#[test]
fn the_rung_bias_arrives_and_settles_where_one_step_would() {
    let street = street();
    for (from, to) in [(0.0, 1.0), (1.0, 0.0), (0.5, 1.5)] {
        let mut rung = RungBias::default();
        rung.set(from);
        (0..200).for_each(|_| rung.step());
        assert_eq!(rung.applied(), from);
        let mut ramped = settled(&street, from);
        let mut stepped = ramped.clone();
        rung.set(to);
        // A doubling in 32 frames: not before, and exactly at the target then.
        let frames = ((to - from) / RUNG_BIAS_STEP).abs().round() as u32;
        assert_eq!(frames, 32, "{from} to {to}");
        for at in 1..=frames {
            rung.step();
            frame(&mut ramped, &street, 1.0, rung.applied());
            assert_eq!(rung.applied() == to, at == frames, "{from} to {to}, frame {at}: {}", rung.applied());
        }
        // The ramp leaves every instance where the bias taken in one frame would, and settled there.
        frame(&mut stepped, &street, 1.0, to);
        assert_eq!(frame(&mut ramped, &street, 1.0, to), 0, "{from} to {to}: settled");
        assert_eq!(frame(&mut stepped, &street, 1.0, to), 0, "{from} to {to}: settled");
        assert!(ramped == stepped, "{from} to {to}: the ramp's levels are one step's");
    }
}
