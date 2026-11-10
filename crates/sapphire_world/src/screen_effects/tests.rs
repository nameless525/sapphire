use sapphire_math::{Vec2, Vec3};
use sapphire_render::{EffectCoverage, PostEffects, PostVariants};
use sapphire_serialize::DocumentError;
use sapphire_time::{ScaleSource, ScaledClock, TICK_HZ};

use super::stack::MAX_FRAMELESS_STEPS;
use super::*;

fn pulse(attack: f32, hold: f32, release: f32) -> Envelope {
    Envelope::Pulse { attack, hold, release }
}

fn keys(keys: &[(f32, f32)]) -> Envelope {
    Envelope::Keys(EnvelopeKeys::new(keys).expect("valid keys"))
}

fn blur(strength: f32, center: [f32; 2], envelope: Envelope) -> EffectLayer {
    EffectLayer {
        effect: ScreenEffect::RadialBlur(RadialBlurEffect { center, strength }),
        envelope,
        delay: 0.0,
        clock: EffectClock::World,
    }
}

fn preset(layers: &[EffectLayer]) -> EffectPreset {
    EffectPreset { name: "test".to_owned(), layers: layers.to_vec() }
}

/// A world clock and a stack on a clock it issued.
fn stack() -> (ScaledClock, EffectStack) {
    let mut world = ScaledClock::new();
    let clock = world.new_local_clock();
    (world, EffectStack::new(clock))
}

fn run(world: &mut ScaledClock, stack: &mut EffectStack, steps: u32) {
    for _ in 0..steps {
        world.tick();
        stack.tick(world);
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn name_lookup_preserves_row_and_layer_error_precedence() {
    let good = preset(&[blur(0.1, [0.5, 0.5], pulse(0.0, 0.1, 0.1))]);
    let mut bad = good.clone();
    bad.layers.clear();
    assert!(matches!(
        EffectLibrary::new(vec![bad.clone(), good.clone()]),
        Err(EffectLibraryError::LayerCount(_))
    ));
    assert!(
        matches!(EffectLibrary::new(vec![good, bad]), Err(EffectLibraryError::DuplicateName(name)) if name == "test")
    );
}

#[test]
fn the_sample_presets_load_and_round_trip() {
    let library = EffectLibrary::default_effects();
    let names: Vec<&str> = library.names().collect();
    assert_eq!(
        names,
        [
            "radial_blur",
            "chromatic_aberration",
            "inversion",
            "duotone",
            "speed_lines",
            "impact_frame",
            "light_hit",
            "medium_hit",
            "heavy_hit",
            "poise_break",
            "finish",
            "perfect_dodge",
            "skill_flash",
        ]
    );
    let kinds = |name: &str| -> Vec<EffectKind> {
        let preset = library.preset(library.find(name).expect("shipped")).expect("in the library");
        preset.layers.iter().map(|layer| layer.effect.kind()).collect()
    };
    assert_eq!(kinds("duotone"), [EffectKind::Duotone]);
    assert_eq!(
        kinds("impact_frame"),
        [
            EffectKind::Duotone,
            EffectKind::SpeedLines,
            EffectKind::RadialBlur,
            EffectKind::ChromaticAberration
        ]
    );
    let text = library.to_ron().expect("writes");
    assert_eq!(EffectLibrary::from_ron(&text).expect("reads"), library);
    // What a layer leaves out is left out when written, and defaults when read.
    assert!(!text.contains("delay"), "{text}");
    assert!(text.contains("clock: Real"));
}

#[test]
fn presets_say_which_post_variants_they_draw_through() {
    // What the renderer prewarms for them: the lens effects' and the color effects' variants.
    let library = EffectLibrary::default_effects();
    let variants =
        |name: &str| library.preset(library.find(name).expect("shipped")).expect("there").post_variants();
    let lens = PostVariants { lens_effects: true, ..PostVariants::NONE };
    let color = PostVariants { color_effects: true, ..PostVariants::NONE };
    assert_eq!(variants("radial_blur"), lens);
    assert_eq!(variants("chromatic_aberration"), lens);
    for name in ["inversion", "duotone", "speed_lines"] {
        assert_eq!(variants(name), color, "{name}");
    }
    assert_eq!(variants("impact_frame"), PostVariants::SCREEN_EFFECTS);
    assert_eq!(library.post_variants(), PostVariants::SCREEN_EFFECTS);
    // Each layer counts alone: a later layer of the same kind drawing nothing hides nothing, and a
    // preset whose layers draw nothing needs no variant.
    let still = blur(0.0, [0.5, 0.5], pulse(0.0, 0.1, 0.1));
    assert_eq!(preset(&[blur(0.1, [0.5, 0.5], pulse(0.0, 0.1, 0.1)), still]).post_variants(), lens);
    assert_eq!(preset(&[still]).post_variants(), PostVariants::NONE);
}

/// A library document with `layers` as the one preset's layers.
fn document(layers: &str) -> String {
    format!(
        "(type: \"sapphire::world::EffectLibrary\", version: 1, data: (presets: [(name: \"a\", layers: [{layers}])]))"
    )
}

fn error(text: &str) -> String {
    match EffectLibrary::from_ron(text) {
        Ok(library) => panic!("accepted: {library:?}"),
        Err(error @ DocumentError::Parse(_)) => error.to_string(),
        Err(error) => panic!("not a parse error: {error}"),
    }
}

#[test]
fn layers_set_one_effect_and_errors_name_their_field() {
    let ok = document("(inversion: (), envelope: Pulse(hold: 0.1))");
    let library = EffectLibrary::from_ron(&ok).expect("a valid layer");
    let layer = library.presets()[0].layers[0];
    assert_eq!(layer.effect, ScreenEffect::Inversion(InversionEffect::default()), "defaults fill it");
    assert_eq!((layer.delay, layer.clock), (0.0, EffectClock::World));

    let two = error(&document("(inversion: (), duotone: (), envelope: Pulse(hold: 0.1))"));
    assert!(two.contains("exactly one effect"), "{two}");
    let none = error(&document("(envelope: Pulse(hold: 0.1))"));
    assert!(none.contains("exactly one effect"), "{none}");
    let no_envelope = error(&document("(inversion: ())"));
    assert!(no_envelope.contains("envelope"), "{no_envelope}");
    let typo = error(&document("(radial_blur: (strenght: 0.1), envelope: Pulse(hold: 0.1))"));
    assert!(typo.contains("strenght"), "{typo}");
    let envelope_typo = error(&document("(inversion: (), envelope: Pulse(hlod: 0.1))"));
    assert!(envelope_typo.contains("hlod"), "{envelope_typo}");
    let range = error(&document("(radial_blur: (strength: 2.0), envelope: Pulse(hold: 0.1))"));
    assert!(range.contains("radial_blur.strength"), "{range}");
    let lines = error(&document("(speed_lines: (count: 0), envelope: Pulse(hold: 0.1))"));
    assert!(lines.contains("speed_lines.count"), "{lines}");
    let instant = error(&document("(inversion: (), envelope: Pulse())"));
    assert!(instant.contains("last some time"), "{instant}");
    let backwards = error(&document("(inversion: (), envelope: Keys([(0.1, 1.0), (0.05, 0.0)]))"));
    assert!(backwards.contains("key 1"), "{backwards}");
    let heavy = error(&document("(inversion: (), envelope: Keys([(0.0, 2.0), (0.05, 0.0)]))"));
    assert!(heavy.contains("weight"), "{heavy}");
    let nine = ["(inversion: (), envelope: Pulse(hold: 0.1))"; MAX_PRESET_LAYERS + 1].join(", ");
    assert!(error(&document(&nine)).contains("between 1 and 8 layers"));
    let many_keys: Vec<(f32, f32)> = (0..=MAX_ENVELOPE_KEYS).map(|key| (key as f32 * 0.1, 1.0)).collect();
    assert_eq!(EnvelopeKeys::new(&many_keys), Err(EnvelopeError::KeyCount(MAX_ENVELOPE_KEYS + 1)));
    let twice = EffectLibrary::new(vec![preset(&[layer]), preset(&[layer])]);
    assert_eq!(twice, Err(EffectLibraryError::DuplicateName("test".to_owned())));
}

#[test]
fn envelopes_rise_hold_fall_and_end() {
    let envelope = pulse(0.1, 0.2, 0.4);
    let weight = |time: f32| envelope.weight(time, None);
    assert_eq!((weight(-0.01), weight(0.0)), (0.0, 0.0), "nothing before the start");
    assert!(close(weight(0.05), 0.5), "halfway up the smoothstep");
    assert_eq!((weight(0.1), weight(0.25)), (1.0, 1.0), "held");
    assert!(close(weight(0.5), 0.5), "halfway down");
    assert_eq!((weight(0.7), weight(5.0)), (0.0, 0.0), "ended");
    assert!(close(envelope.end(None), 0.7));
    // No attack: full from the first instant.
    assert_eq!(pulse(0.0, 0.1, 0.0).weight(0.0, None), 1.0);

    // Held until released, then falling from wherever it was, released mid-rise or held.
    let sustain = Envelope::Sustain { attack: 0.2, release: 0.4 };
    assert_eq!(sustain.weight(100.0, None), 1.0);
    assert_eq!(sustain.end(None), f32::INFINITY);
    assert!(close(sustain.weight(1.2, Some(1.0)), 0.5));
    assert_eq!(sustain.weight(1.4, Some(1.0)), 0.0);
    let early = sustain.weight(0.1, None);
    assert!(close(sustain.weight(0.1, Some(0.1)), early), "falls from mid-rise");
    assert!(sustain.weight(0.3, Some(0.1)) < early);

    // Keys: linear between, a cut where two share a time, ending at the last.
    let flash = keys(&[(0.0, 1.0), (0.05, 1.0), (0.05, 0.0), (0.1, 0.0), (0.1, 1.0), (0.2, 0.5)]);
    let weight = |time: f32| flash.weight(time, None);
    assert_eq!((weight(0.0), weight(0.049)), (1.0, 1.0));
    assert_eq!((weight(0.05), weight(0.09)), (0.0, 0.0), "cut off at 0.05");
    assert_eq!(weight(0.1), 1.0, "back on at 0.1");
    assert!(close(weight(0.15), 0.75));
    assert_eq!(weight(0.2), 0.0, "ended at the last key");
    let late = keys(&[(0.1, 0.5), (0.2, 1.0)]);
    assert_eq!(late.weight(0.05, None), 0.5, "before the first key, its weight");
}

#[test]
fn layers_play_on_their_clocks() {
    let (mut world, mut stack) = stack();
    let long = pulse(0.0, 30.0, 0.0);
    let scaled = blur(0.1, [0.5, 0.5], long);
    let real = EffectLayer { clock: EffectClock::Real, ..scaled };
    stack.push(&preset(&[scaled, real]));
    let times = |stack: &EffectStack| -> Vec<f32> { stack.playing().map(|effect| effect.time).collect() };
    run(&mut world, &mut stack, TICK_HZ);
    assert!(times(&stack).iter().all(|time| close(*time, 1.0)), "a second each: {:?}", times(&stack));

    // Slow motion stretches the world's time, not real time.
    let slow = world.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
    run(&mut world, &mut stack, TICK_HZ);
    let [world_time, real_time] = times(&stack)[..] else { panic!("two layers") };
    assert!(close(world_time, 1.2) && close(real_time, 2.0), "{world_time} {real_time}");
    world.pop_scale(slow);

    // A stopped world holds both, as it holds hit-stop's countdown.
    let pause = world.push_scale(ScaleSource::Pause, 0).expect("room");
    run(&mut world, &mut stack, TICK_HZ);
    assert_eq!(times(&stack), [world_time, real_time]);
    world.pop_scale(pause);

    // Hit-stop on an entity's clock doesn't reach the stack's: the effects play through it.
    let mut combatant = world.new_local_clock();
    combatant.apply_hitstop(30);
    for _ in 0..30 {
        world.tick();
        combatant.tick(&world);
        stack.tick(&world);
    }
    assert!(close(times(&stack)[0], world_time + 0.5), "{:?}", times(&stack));

    // Resolving interpolates between steps.
    let mut fading = EffectStack::new(world.new_local_clock());
    fading.push(&preset(&[blur(0.2, [0.5, 0.5], pulse(0.0, 0.0, 1.0))]));
    world.tick();
    fading.tick(&world);
    let at = |alpha: f32, fading: &mut EffectStack| fading.resolve(alpha).radial_blur.strength;
    let (previous, latest) = (at(0.0, &mut fading), at(1.0, &mut fading));
    let between = at(0.5, &mut fading);
    assert!(previous > between && between > latest, "{previous} {between} {latest}");
}

#[test]
fn shown_time_never_steps_back_entering_a_pause_or_slow_motion() {
    // Review WA-3: clocks ran up to a tick ahead of the latest step, by the last step's delta. When
    // that delta shrank, on a pause or into slow motion, what was shown stepped back.
    let (mut world, mut stack) = stack();
    stack.push(&preset(&[blur(0.2, [0.5, 0.5], pulse(0.0, 0.0, 2.0))]));
    run(&mut world, &mut stack, 30);
    let mut shown = stack.resolve(0.9).radial_blur.strength;

    let pause = world.push_scale(ScaleSource::Pause, 0).expect("room");
    run(&mut world, &mut stack, 1);
    for alpha in [0.0, 0.5, 1.0] {
        let strength = stack.resolve(alpha).radial_blur.strength;
        assert!(strength <= shown, "paused, alpha {alpha}: {strength} after {shown}");
        shown = strength;
    }
    assert_eq!(stack.resolve(0.0).radial_blur.strength, stack.resolve(1.0).radial_blur.strength);
    world.pop_scale(pause);

    run(&mut world, &mut stack, 30);
    shown = stack.resolve(0.9).radial_blur.strength;
    world.push_scale(ScaleSource::PerfectDodge, 100).expect("room");
    run(&mut world, &mut stack, 1);
    let slowed = stack.resolve(0.0).radial_blur.strength;
    assert!(slowed <= shown, "slowed: {slowed} after {shown}");
}

#[test]
fn a_push_shows_from_its_start_whatever_the_alpha() {
    let (mut world, mut stack) = stack();
    run(&mut world, &mut stack, 10);
    stack.push(&preset(&[blur(0.2, [0.5, 0.5], pulse(0.0, 1.0, 0.0))]));
    // Pushed on the latest step: there's no step before it to draw from.
    for alpha in [0.0, 0.5, 1.0] {
        assert!(close(stack.resolve(alpha).radial_blur.strength, 0.2), "alpha {alpha}");
    }
    // One with a delay waits that delay out, however early in a step the frame falls.
    let (mut world, mut stack) = self::stack();
    let delayed = EffectLayer { delay: 0.5, ..blur(0.1, [0.5, 0.5], pulse(0.0, 0.5, 0.0)) };
    run(&mut world, &mut stack, 10);
    stack.push(&preset(&[delayed]));
    for alpha in [0.0, 0.5, 1.0] {
        assert_eq!(stack.resolve(alpha), PostEffects::NONE, "alpha {alpha}");
    }
    assert_eq!(stack.len(), 1);
}

#[test]
fn layers_start_after_their_delay_and_end_by_themselves() {
    let (mut world, mut stack) = stack();
    let delayed = EffectLayer { delay: 0.5, ..blur(0.1, [0.5, 0.5], pulse(0.0, 0.5, 0.0)) };
    stack.push(&preset(&[delayed]));
    run(&mut world, &mut stack, 15);
    assert_eq!(stack.resolve(1.0), PostEffects::NONE, "still waiting");
    assert_eq!((stack.stats().depth, stack.stats().active), (1, 0));
    run(&mut world, &mut stack, 30);
    assert!(close(stack.resolve(1.0).radial_blur.strength, 0.1));
    assert_eq!(stack.stats().active, 1);
    run(&mut world, &mut stack, 30);
    assert!(stack.is_empty(), "ended a second after the push");
    assert_eq!(stack.resolve(1.0), PostEffects::NONE, "an empty stack draws nothing");
}

#[test]
fn a_sustained_layer_released_in_its_delay_never_shows() {
    // Review PW-004: released while it still waits out its delay, a sustained layer is cancelled.
    // It used to wait, then show at its full weight when it started (with no attack) and fade.
    // The preset's other layers play on by their own envelopes.
    for clock in [EffectClock::World, EffectClock::Real] {
        for attack in [0.0, 0.1] {
            let (mut world, mut stack) = stack();
            let sustain = Envelope::Sustain { attack, release: 0.2 };
            let charge = EffectLayer { delay: 1.0, clock, ..blur(0.1, [0.5, 0.5], sustain) };
            let lines = EffectLayer {
                effect: ScreenEffect::SpeedLines(SpeedLinesEffect {
                    rate: 0.0,
                    ..SpeedLinesEffect::default()
                }),
                clock,
                ..blur(0.0, [0.5, 0.5], pulse(0.0, 1.0, 0.0))
            };
            let handle = stack.push(&preset(&[charge, lines]));
            run(&mut world, &mut stack, 1);
            stack.release(handle);
            assert_eq!((stack.len(), stack.stats().depth), (1, 1), "{clock:?}, attack {attack}: cancelled");
            stack.release(handle);
            assert_eq!(stack.len(), 1, "releasing again changes nothing");
            assert_eq!(stack.resolve(1.0).speed_lines.opacity, 1.0, "the pulse plays on");
            for step in 0..90 {
                run(&mut world, &mut stack, 1);
                let blur = stack.resolve(0.5).radial_blur;
                assert_eq!(blur, PostEffects::NONE.radial_blur, "{clock:?}, attack {attack}, step {step}");
            }
            assert!(stack.is_empty());
        }
    }

    // Once started, a delayed one is released as any other: it falls from where it was, mid-rise
    // here (halfway up a 0.4 s attack, 0.2 s in).
    let (mut world, mut stack) = stack();
    let sustain = Envelope::Sustain { attack: 0.4, release: 0.5 };
    let handle = stack.push(&preset(&[EffectLayer { delay: 0.5, ..blur(0.1, [0.5, 0.5], sustain) }]));
    run(&mut world, &mut stack, 42);
    let before = stack.resolve(1.0).radial_blur.strength;
    assert!(close(before, 0.05), "{before}");
    stack.release(handle);
    assert!(close(stack.resolve(1.0).radial_blur.strength, before), "falls from where it was");
    run(&mut world, &mut stack, 15);
    let falling = stack.resolve(1.0).radial_blur.strength;
    assert!(close(falling, 0.025), "halfway down: {falling}");
    run(&mut world, &mut stack, 16);
    assert!(stack.is_empty(), "ended with its release");
}

#[test]
fn later_pushes_show_over_earlier_ones_kind_by_kind() {
    let (mut world, mut stack) = stack();
    let held = pulse(0.0, 10.0, 0.0);
    stack.push(&preset(&[blur(0.2, [0.25, 0.5], held)]));
    // Fading in over a second: a quarter of the way after 15 steps.
    let rising = blur(0.1, [0.75, 0.5], keys(&[(0.0, 0.0), (1.0, 1.0), (10.0, 1.0)]));
    let lines = EffectLayer {
        effect: ScreenEffect::SpeedLines(SpeedLinesEffect { rate: 0.0, ..SpeedLinesEffect::default() }),
        ..blur(0.0, [0.5, 0.5], held)
    };
    stack.push(&preset(&[rising, lines]));
    run(&mut world, &mut stack, 15);
    let effects = stack.resolve(1.0);
    let blur = effects.radial_blur;
    assert!(close(blur.strength, 0.2 * 0.75 + 0.1 * 0.25), "{blur:?}");
    // Its center moves by its share of the result: 0.025 of 0.175.
    let share = 0.025 / 0.175;
    assert!(blur.center.abs_diff_eq(Vec2::new(0.25 + 0.5 * share, 0.5), 1e-4), "{blur:?}");
    assert_eq!(effects.speed_lines.opacity, 1.0, "another kind, untouched by the blurs");
    run(&mut world, &mut stack, 60);
    let top = stack.resolve(1.0).radial_blur;
    assert!(
        close(top.strength, 0.1) && top.center.abs_diff_eq(Vec2::new(0.75, 0.5), 1e-5),
        "the top replaces: {top:?}"
    );

    // Over nothing, a layer keeps its own values as it fades in.
    let (mut world, mut stack) = self::stack();
    stack.push(&preset(&[rising]));
    run(&mut world, &mut stack, 6);
    let alone = stack.resolve(1.0).radial_blur;
    assert!(alone.strength > 0.0 && alone.center == Vec2::new(0.75, 0.5), "{alone:?}");
}

#[test]
fn colors_and_masks_blend_and_centers_follow_the_push() {
    let (_, mut stack) = stack();
    let library = EffectLibrary::default_effects();
    let impact = library.preset(library.find("impact_frame").expect("shipped")).expect("there");
    stack.push_at(impact, Vec2::new(0.3, 0.6));
    let effects = stack.resolve(1.0);
    assert_eq!(effects.duotone.coverage, EffectCoverage::WORLD, "characters keep their colors");
    assert_eq!(effects.duotone.amount, 1.0);
    assert_eq!(effects.duotone.dark, Vec3::new(0.005, 0.004, 0.012));
    let center = Vec2::new(0.3, 0.6);
    assert_eq!((effects.radial_blur.center, effects.speed_lines.center), (center, center));
    assert_eq!(effects.chromatic_aberration.center, center);
    assert_eq!(effects.inversion, PostEffects::NONE.inversion, "the preset has none");
}

#[test]
fn speed_lines_are_redrawn_at_their_rate() {
    let (mut world, mut stack) = stack();
    let lines = EffectLayer {
        effect: ScreenEffect::SpeedLines(SpeedLinesEffect { rate: 10.0, ..SpeedLinesEffect::default() }),
        ..blur(0.0, [0.5, 0.5], pulse(0.0, 5.0, 0.0))
    };
    stack.push(&preset(&[lines]));
    let mut seeds = Vec::new();
    // Up to just before a second: 0.1 s a drawing.
    for _ in 0..59 {
        run(&mut world, &mut stack, 1);
        let seed = stack.resolve(1.0).speed_lines.seed;
        if seeds.last() != Some(&seed) {
            seeds.push(seed);
        }
    }
    assert_eq!(seeds.len(), 10, "ten drawings a second: {seeds:?}");
    // Another push draws other lines: the top of two shows its own.
    let (_, mut one) = self::stack();
    one.push(&preset(&[lines]));
    let (_, mut two) = self::stack();
    two.push(&preset(&[lines]));
    two.push(&preset(&[lines]));
    assert_ne!(one.resolve(1.0).speed_lines.seed, two.resolve(1.0).speed_lines.seed);
}

#[test]
fn a_new_run_matches_fresh_patterns_and_old_handles_cannot_change_it() {
    let lines = preset(&[EffectLayer {
        effect: ScreenEffect::SpeedLines(SpeedLinesEffect { rate: 10.0, ..Default::default() }),
        ..blur(0.0, [0.5, 0.5], pulse(0.0, 1.0, 0.1))
    }]);
    let (mut world, mut restarted) = stack();
    let (mut fresh_world, mut fresh) = stack();
    let old = restarted.push(&lines);
    run(&mut world, &mut restarted, 3);
    assert!(restarted.resolve(1.0).speed_lines.opacity > 0.0);
    let pushed = restarted.stats().pushed;
    restarted.start_over();
    assert_eq!(restarted.stats().active, 0);
    assert_eq!(restarted.stats().pushed, pushed, "lifetime telemetry remains cumulative");
    assert!(restarted.is_empty());
    let current = restarted.push(&lines);
    fresh.push(&lines);
    assert_ne!(old, current, "a reset never reuses a handle");
    restarted.hold(old, Some(0.7));
    restarted.release(old);
    restarted.stop(old);
    assert_eq!(restarted.len(), 1, "stale handles leave the new run alone");
    for _ in 0..12 {
        for alpha in [0.0, 0.5, 1.0] {
            assert_eq!(restarted.resolve(alpha), fresh.resolve(alpha));
        }
        run(&mut world, &mut restarted, 1);
        run(&mut fresh_world, &mut fresh, 1);
    }
}

#[test]
fn a_full_stack_drops_what_has_least_time_left() {
    let (_, mut stack) = stack();
    // Sixteen layers ending 1 to 16 seconds from now.
    for seconds in 1..=STACK_CAPACITY {
        stack.push(&preset(&[blur(0.1, [0.5, 0.5], pulse(0.0, seconds as f32, 0.0))]));
    }
    assert_eq!(stack.len(), STACK_CAPACITY);
    let ends = |stack: &EffectStack| -> Vec<f32> { stack.playing().map(|effect| effect.end).collect() };
    // A layer that would end before every other is the one dropped.
    stack.push(&preset(&[blur(0.1, [0.5, 0.5], pulse(0.0, 0.5, 0.0))]));
    assert_eq!(stack.stats().dropped, 1);
    assert_eq!(ends(&stack)[0], 1.0, "the playing ones are kept");
    // A longer one takes the place of the one ending first, and goes on top.
    stack.push(&preset(&[blur(0.3, [0.5, 0.5], pulse(0.0, 20.0, 0.0))]));
    assert_eq!(stack.stats().dropped, 2);
    let after = ends(&stack);
    assert_eq!((after[0], after[STACK_CAPACITY - 1], after.len()), (2.0, 20.0, STACK_CAPACITY));
    assert!(close(stack.resolve(1.0).radial_blur.strength, 0.3), "the latest shows on top");
    // Sustained layers that weren't released stay; held ones too.
    stack.clear();
    for _ in 0..STACK_CAPACITY {
        stack.push(&preset(&[blur(0.1, [0.5, 0.5], Envelope::Sustain { attack: 0.0, release: 0.1 })]));
    }
    stack.push(&preset(&[blur(0.1, [0.5, 0.5], pulse(0.0, 30.0, 0.0))]));
    assert_eq!(stack.stats().dropped, 3);
    assert!(ends(&stack).iter().all(|end| end.is_infinite()));
}

/// A frame that runs `steps` fixed steps, then resolves, and what it drew of the inversion.
fn inversion_frame(world: &mut ScaledClock, stack: &mut EffectStack, steps: u32) -> f32 {
    run(world, stack, steps);
    stack.resolve(0.5).inversion.amount
}

#[test]
fn an_effect_shorter_than_a_frame_still_draws_once() {
    // Review WA-2: the sample inversion lasts three steps, and a frame can run four. It used to end
    // inside the frame, so no frame ever drew it. Those that drew it before draw as they did.
    let library = EffectLibrary::default_effects();
    let inversion = library.preset(library.find("inversion").expect("shipped")).expect("there");
    let frames = [
        (1, [1.0, 1.0, 0.0, 0.0]),
        (2, [1.0, 0.0, 0.0, 0.0]),
        (3, [1.0, 0.0, 0.0, 0.0]),
        (4, [1.0, 0.0, 0.0, 0.0]),
    ];
    for (steps, expected) in frames {
        let (mut world, mut stack) = stack();
        stack.push(inversion);
        let drawn = [0; 4].map(|_| inversion_frame(&mut world, &mut stack, steps));
        assert_eq!(drawn, expected, "{steps} steps a frame");
        assert!(stack.is_empty(), "{steps} steps a frame");
    }

    // Pushed by a hit on the first of a frame's four steps.
    let (mut world, mut stack) = stack();
    run(&mut world, &mut stack, 1);
    stack.push(inversion);
    assert_eq!(inversion_frame(&mut world, &mut stack, 3), 1.0);
    assert_eq!(stack.stats().active, 1);

    // That frame was its one: the next step drops it, and a second frame before any step shows
    // no more of it.
    assert_eq!(stack.resolve(0.5).inversion.amount, 0.0, "one frame");
    assert_eq!(stack.stats().active, 0);
    assert_eq!(stack.len(), 1, "until the next step");
    run(&mut world, &mut stack, 1);
    assert!(stack.is_empty());
    assert_eq!(stack.resolve(0.5), PostEffects::NONE);
}

#[test]
fn an_undrawn_effect_shows_at_its_peak() {
    // Its whole envelope went by unseen. The tail of a fall would hide the hit as surely as losing
    // it, so the frame it is owed shows the most it would have shown.
    let shown = |envelope: Envelope, delay: f32, released: bool| {
        let (mut world, mut stack) = stack();
        let handle = stack.push(&preset(&[EffectLayer { delay, ..blur(0.2, [0.5, 0.5], envelope) }]));
        if released {
            run(&mut world, &mut stack, 1);
            stack.release(handle);
        }
        run(&mut world, &mut stack, 8);
        let strength = stack.resolve(0.5).radial_blur.strength;
        run(&mut world, &mut stack, 1);
        assert!(stack.is_empty(), "{envelope:?}");
        strength
    };
    assert!(close(shown(pulse(0.0, 0.0, 0.05), 0.0, false), 0.2), "only a fall");
    assert!(close(shown(pulse(0.05, 0.0, 0.0), 0.0, false), 0.2), "only a rise");
    assert!(close(shown(pulse(0.01, 0.01, 0.01), 0.0, false), 0.2), "the lot");
    assert!(close(shown(pulse(0.0, 0.02, 0.0), 0.05, false), 0.2), "after a delay");
    assert!(
        close(shown(keys(&[(0.0, 0.25), (0.02, 0.5), (0.05, 0.1)]), 0.0, false), 0.1),
        "its greatest key"
    );
    assert!(close(shown(keys(&[(0.0, 0.0), (0.05, 1.0)]), 0.0, false), 0.2), "a key at its very end");

    // Released mid-rise, it never got further than that.
    let sustain = Envelope::Sustain { attack: 0.1, release: 0.02 };
    let rise = sustain.weight(1.0 / TICK_HZ as f32, None);
    assert!(close(shown(sustain, 0.0, true), 0.2 * rise));

    // Nothing to show, nothing drawn, and it still goes.
    assert_eq!(shown(keys(&[(0.0, 0.0), (0.05, 0.0)]), 0.0, false), 0.0);

    // Past its end only by the frame's own time, which reads ahead of the fixed step: this frame
    // is its last chance, since the next step would end it having drawn nothing.
    let (mut world, mut stack) = stack();
    stack.push(&preset(&[blur(0.2, [0.5, 0.5], pulse(0.0, 0.02, 0.0))]));
    run(&mut world, &mut stack, 1);
    assert!(close(stack.resolve(0.5).radial_blur.strength, 0.2));
    run(&mut world, &mut stack, 1);
    assert!(stack.is_empty());
}

#[test]
fn an_effect_drawn_before_it_ended_owes_no_frame() {
    let (mut world, mut stack) = stack();
    stack.push(&preset(&[blur(0.2, [0.5, 0.5], pulse(0.0, 0.05, 0.0))]));
    run(&mut world, &mut stack, 1);
    assert!(close(stack.resolve(1.0).radial_blur.strength, 0.2));
    run(&mut world, &mut stack, 4);
    assert!(stack.is_empty(), "gone with the step that ended it");
    assert_eq!(stack.resolve(0.5), PostEffects::NONE);

    // Stopped or cleared, it goes at once, drawn or not.
    let hit = preset(&[blur(0.2, [0.5, 0.5], pulse(0.0, 0.01, 0.0))]);
    let stopped = stack.push(&hit);
    run(&mut world, &mut stack, 4);
    assert_eq!(stack.len(), 1, "waiting for its frame");
    stack.stop(stopped);
    assert_eq!(stack.resolve(0.5), PostEffects::NONE);
    stack.push(&hit);
    run(&mut world, &mut stack, 4);
    stack.clear();
    assert_eq!(stack.resolve(0.5), PostEffects::NONE);
}

#[test]
fn a_layer_waiting_for_its_frame_counts_and_stays_in_a_full_stack() {
    // A hit that a hitch ran past, over fifteen layers that have been showing.
    let flash = preset(&[EffectLayer {
        effect: ScreenEffect::Inversion(InversionEffect::default()),
        ..blur(0.0, [0.5, 0.5], pulse(0.0, 0.02, 0.0))
    }]);
    let long = |seconds: f32| preset(&[blur(0.1, [0.5, 0.5], pulse(0.0, seconds, 0.0))]);
    let (mut world, mut stack) = stack();
    for seconds in 0..STACK_CAPACITY - 1 {
        stack.push(&long(5.0 + seconds as f32));
    }
    stack.resolve(0.5);
    stack.push(&flash);
    run(&mut world, &mut stack, 4);
    assert_eq!((stack.len(), stack.stats().depth), (STACK_CAPACITY, STACK_CAPACITY as u32), "it counts");

    // The stack is full. The next push takes the place of the layer with the least time left
    // that isn't owed a frame, where otherwise the flash, with the least of all, would go.
    stack.push(&long(30.0));
    assert_eq!(stack.stats().dropped, 1);
    assert_eq!(stack.len(), STACK_CAPACITY);
    let ends: Vec<f32> = stack.playing().map(|effect| effect.end).collect();
    assert_eq!(ends[0], 6.0, "the first long layer went");
    assert_eq!(stack.resolve(0.5).inversion.amount, 1.0, "the flash shows");
    run(&mut world, &mut stack, 1);
    assert_eq!(stack.len(), STACK_CAPACITY - 1, "and goes");

    // With nothing else to give up, a new layer is the one dropped, as among held layers.
    let (mut world, mut stack) = self::stack();
    for _ in 0..STACK_CAPACITY {
        stack.push(&flash);
    }
    run(&mut world, &mut stack, 4);
    stack.push(&long(30.0));
    assert_eq!((stack.len(), stack.stats().dropped), (STACK_CAPACITY, 1));
    stack.resolve(0.5);
    run(&mut world, &mut stack, 1);
    assert!(stack.is_empty(), "all drawn, all gone: the slots were only held for a frame");
}

#[test]
fn without_frames_layers_end_as_they_always_did() {
    // A stack nothing resolves (a simulation without a renderer, a capture's warmup steps) isn't
    // in a hitch. Layers can't wait for a frame that isn't coming: they would fill the stack, and
    // all flash at once at their peaks on the first frame there is.
    let hit = preset(&[blur(0.2, [0.5, 0.5], pulse(0.0, 0.05, 0.0))]);
    let (mut world, mut stack) = stack();
    stack.push(&hit);
    run(&mut world, &mut stack, MAX_FRAMELESS_STEPS);
    assert_eq!(stack.len(), 1, "as long as a hitch could be");
    assert!(close(stack.resolve(0.5).radial_blur.strength, 0.2));

    let (mut world, mut stack) = self::stack();
    stack.push(&hit);
    run(&mut world, &mut stack, MAX_FRAMELESS_STEPS + 1);
    assert!(stack.is_empty());
    assert_eq!(stack.resolve(0.5), PostEffects::NONE);

    // It goes on so for as long as no frame comes, for layers pushed along the way too.
    run(&mut world, &mut stack, MAX_FRAMELESS_STEPS + 1);
    for _ in 0..3 * STACK_CAPACITY {
        stack.push(&hit);
        run(&mut world, &mut stack, 4);
    }
    assert!(stack.is_empty());
    assert_eq!(stack.stats().dropped, 0);

    // And the first frame back starts the wait afresh.
    assert_eq!(stack.resolve(0.5), PostEffects::NONE);
    stack.push(&hit);
    run(&mut world, &mut stack, MAX_FRAMELESS_STEPS);
    assert!(close(stack.resolve(0.5).radial_blur.strength, 0.2));
}

#[test]
fn sustained_layers_wait_for_their_release_and_handles_stop_and_hold() {
    let (mut world, mut stack) = stack();
    let charge =
        stack.push(&preset(&[blur(0.1, [0.5, 0.5], Envelope::Sustain { attack: 0.1, release: 0.5 })]));
    stack.push(&preset(&[blur(0.2, [0.5, 0.5], pulse(0.0, 1.0, 1.0))]));
    run(&mut world, &mut stack, 600);
    assert_eq!(stack.len(), 1, "the hit ended; the charge holds");
    stack.release(charge);
    run(&mut world, &mut stack, 15);
    assert!(close(stack.resolve(1.0).radial_blur.strength, 0.05), "halfway down its release");
    run(&mut world, &mut stack, 16);
    assert!(stack.is_empty());

    // Held at a time, a push shows the same frame however long it runs: captures and benches.
    let hit_preset = preset(&[blur(0.2, [0.5, 0.5], pulse(0.0, 0.1, 1.0))]);
    let held = stack.push(&hit_preset);
    stack.hold(held, Some(0.6));
    run(&mut world, &mut stack, 600);
    assert!(close(stack.resolve(0.5).radial_blur.strength, 0.1), "held halfway down");
    stack.hold(held, None);
    run(&mut world, &mut stack, 60);
    assert!(stack.is_empty(), "played on to its end once let go");
    let stopped = stack.push(&hit_preset);
    stack.stop(stopped);
    assert!(stack.is_empty());
}

/// What the cue dispatcher counts as a full-screen flash (M4 A3): a preset with an inversion or a duotone, for
/// as long as its last one lasts after its delay. Blurs, splits and lines aren't.
#[test]
fn a_flash_is_an_inversion_or_a_duotone_for_as_long_as_its_last() {
    let library = EffectLibrary::default_effects();
    let seconds = |name: &str| library.preset(library.find(name).expect(name)).expect(name).flash_seconds();
    for quiet in
        ["radial_blur", "chromatic_aberration", "speed_lines", "light_hit", "medium_hit", "skill_flash"]
    {
        assert_eq!(seconds(quiet), None, "{quiet}");
    }
    assert!(close(seconds("heavy_hit").expect("a flash"), 0.05));
    assert!(close(seconds("poise_break").expect("a flash"), 0.1), "the longer of its duotone and inversion");
    assert!(close(seconds("finish").expect("a flash"), 0.7), "to the end of its fall");
    assert!(close(seconds("perfect_dodge").expect("a flash"), 0.75));

    let invert = |delay, envelope| EffectLayer {
        effect: ScreenEffect::Inversion(InversionEffect::default()),
        envelope,
        delay,
        clock: EffectClock::Real,
    };
    let held = preset(&[invert(0.1, Envelope::Sustain { attack: 0.2, release: 0.3 })]);
    assert!(close(held.flash_seconds().expect("a flash"), 0.6), "a sustained one, what's known of it");
    let both = preset(&[invert(0.0, pulse(0.0, 0.1, 0.0)), blur(0.2, [0.5, 0.5], pulse(0.0, 2.0, 0.0))]);
    assert!(close(both.flash_seconds().expect("a flash"), 0.1), "the blur's length isn't the flash's");
}
