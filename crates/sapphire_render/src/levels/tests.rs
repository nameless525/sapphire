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
