use super::*;
use crate::{CloudsKey, TodKey};

fn document(text: &str) -> Result<WeatherDocument, DocumentError> {
    WeatherDocument::from_ron(&format!(
        r#"(type: "sapphire::world::WeatherDocument", version: 1, data: ({text}))"#
    ))
}

fn clouds(coverage: f32) -> CloudsKey {
    CloudsKey { lit: [1.0, 1.0, 1.0], shade: [0.5, 0.5, 0.5], rim: [1.0, 1.0, 1.0], coverage, cirrus: 0.0 }
}

/// A partial day of one key at noon setting the clouds, and the exposure if given.
fn day(coverage: f32, exposure: Option<f32>) -> TodTimeline {
    TodTimeline::new(vec![TodKey {
        time: 12.0,
        clouds: Some(clouds(coverage)),
        exposure,
        ..TodKey::default()
    }])
    .expect("a valid partial day")
}

fn state(name: &str) -> WeatherState {
    WeatherState { name: name.to_owned(), ..WeatherState::default() }
}

/// Four states and a day of four slots, the table below pins.
fn schedule() -> WeatherSchedule {
    let slots = vec![
        (0.0, vec![(0, 1), (1, 1), (2, 2)]),
        (6.0, vec![(0, 3), (3, 1)]),
        (12.0, vec![(1, 1), (2, 1), (3, 1)]),
        (18.0, vec![(0, 1), (2, 1)]),
    ];
    WeatherSchedule::new(7, slots, 4).expect("a valid schedule")
}

/// The schedule's states at a fixed seed are these, on every machine: the hash is integer maths alone,
/// so a change here is a change to every zone's weather.
#[test]
fn the_schedules_states_at_a_fixed_seed_are_pinned() {
    let schedule = schedule();
    let week: Vec<[usize; 4]> =
        (0..7).map(|day| std::array::from_fn(|slot| schedule.pick(day, slot))).collect();
    let pinned =
        [[2, 0, 2, 2], [2, 3, 3, 0], [1, 0, 3, 0], [2, 3, 2, 0], [2, 0, 1, 2], [1, 0, 1, 0], [2, 3, 1, 0]];
    assert_eq!(week, pinned);
    assert_eq!(schedule.pick(-3, 2), schedule.pick(-3, 2), "days before the first are days too");
    let other = WeatherSchedule::new(
        8,
        schedule.slots.iter().map(|slot| (slot.from as f32 / 100.0, slot.choices.clone())).collect(),
        4,
    )
    .expect("valid");
    let differs = (0..64).any(|day| (0..4).any(|slot| other.pick(day, slot) != schedule.pick(day, slot)));
    assert!(differs, "another seed is another sequence");
}

/// Over many days each choice comes up about as often as its weight says.
#[test]
fn choices_come_up_by_their_weights() {
    let schedule = schedule();
    let mut counts = [0u32; 4];
    for day in 0..12_000 {
        counts[schedule.pick(day, 0)] += 1;
    }
    let shares = counts.map(|count| f64::from(count) / 12_000.0);
    assert!((shares[0] - 0.25).abs() < 0.02 && (shares[1] - 0.25).abs() < 0.02, "{shares:?}");
    assert!((shares[2] - 0.5).abs() < 0.02 && counts[3] == 0, "{shares:?}");
}

/// The schedule is a pure function of the day and the hour: the same answer however it's asked, in any
/// order, and an hour before the first slot belongs to the night before.
#[test]
fn the_schedule_is_a_pure_function_of_the_clock() {
    let schedule = schedule();
    let times: Vec<(i64, f32)> = (0..400).map(|step| (step / 48, (step % 48) as f32 * 0.5)).collect();
    let forward: Vec<usize> = times.iter().map(|&(day, hours)| schedule.state_at(day, hours)).collect();
    let backward: Vec<usize> =
        times.iter().rev().map(|&(day, hours)| schedule.state_at(day, hours)).collect();
    assert_eq!(forward, backward.into_iter().rev().collect::<Vec<_>>());
    assert_eq!(schedule.slot_at(3, 5.99), (3, 0));
    assert_eq!(schedule.slot_at(3, 6.0), (3, 1));
    assert_eq!(schedule.slot_at(3, 23.5), (3, 3));
    let late = WeatherSchedule::new(1, vec![(6.0, vec![(0, 1)]), (20.0, vec![(1, 1)])], 2).expect("valid");
    assert_eq!(late.slot_at(3, 2.0), (2, 1), "before the first slot is the night before's last");
    assert_eq!(late.state_at(3, 2.0), 1);
    assert_eq!(WeatherSchedule::always(2).state_at(-40, 13.0), 2);
}

/// A schedule's slots as [`WeatherSchedule::new`] takes them.
type Slots = Vec<(f32, Vec<(usize, u32)>)>;

#[test]
fn a_bad_schedule_is_refused_by_its_field() {
    let cases: [(Slots, &str); 6] = [
        (vec![], "schedule"),
        (vec![(24.0, vec![(0, 1)])], "schedule[0].from"),
        (vec![(f32::NAN, vec![(0, 1)])], "schedule[0].from"),
        (vec![(6.0, vec![(0, 1)]), (6.0, vec![(1, 1)])], "schedule[1].from"),
        (vec![(0.0, vec![(5, 1)])], "schedule[0].states[0]"),
        (vec![(0.0, vec![(0, 0)])], "schedule[0].states[0]"),
    ];
    for (slots, field) in cases {
        assert_eq!(WeatherSchedule::new(0, slots, 2).expect_err(field).field, field);
    }
}

#[test]
fn a_document_reads_writes_and_takes_its_defaults() {
    let document = document(
        r#"states: [
            (name: "clear"),
            (name: "rain", precipitation: Some((kind: Rain, density: 0.7)), wind: Some((strength: 2.0)),
             accumulation: Some((wetness: (target: 1.0, rise: 30.0))), temperature: Some(Mild)),
        ], transitions: (pairs: [(from: "rain", to: "clear", seconds: 90.0)])"#,
    )
    .expect("a good document");
    assert_eq!(document.states().len(), 2);
    assert_eq!(document.state("rain"), Some(1));
    assert_eq!(document.state("snow"), None);
    let rain = &document.states()[1];
    let falling = rain.precipitation.expect("set");
    assert_eq!((falling.size, falling.speed), (1.0, 1.0), "sizes and speeds default to the kind's");
    assert_eq!(rain.wind.expect("set").gustiness, 0.0);
    assert_eq!(document.seconds(0, 1), 90.0, "a pair holds either way");
    assert_eq!(document.seconds(1, 0), 90.0);
    assert_eq!(document.seconds(0, 0), 30.0, "half a minute by default");
    let text = document.to_ron().expect("writes");
    assert_eq!(WeatherDocument::from_ron(&text).expect("reads back"), document);
}

#[test]
fn a_bad_document_is_refused_by_its_field() {
    let cases = [
        (r#"states: []"#, "states"),
        (r#"states: [(name: "")]"#, "states[0].name"),
        (r#"states: [(name: "rain"), (name: "rain")]"#, "states[1].name"),
        (
            r#"states: [(name: "rain", precipitation: Some((kind: Rain, density: 1.5)))]"#,
            "states[0].precipitation.density",
        ),
        (
            r#"states: [(name: "rain", precipitation: Some((kind: Hail, density: 0.5, size: 0.0)))]"#,
            "states[0].precipitation.size",
        ),
        (r#"states: [(name: "gale", wind: Some((strength: 9.0)))]"#, "states[0].wind.strength"),
        (
            r#"states: [(name: "gale", wind: Some((strength: 2.0, turbulence: -0.1)))]"#,
            "states[0].wind.turbulence",
        ),
        (
            r#"states: [(name: "storm", lightning: Some((per_minute: 61.0)))]"#,
            "states[0].lightning.per_minute",
        ),
        (
            r#"states: [(name: "snow", accumulation: Some((snow: (target: 1.0, rise: 0.0))))]"#,
            "states[0].accumulation.snow.rise",
        ),
        (r#"states: [(name: "storm", sea: Some(2.0))]"#, "states[0].sea"),
        (r#"states: [(name: "storm", audio: Some((thunder: 1.1)))]"#, "states[0].audio.thunder"),
        (
            r#"states: [(name: "mist", environment: Some((fog_density: 17.0)))]"#,
            "states[0].environment.fog_density",
        ),
        (r#"states: [(name: "rain")], transitions: (seconds: 601.0)"#, "transitions.seconds"),
        (
            r#"states: [(name: "rain")], transitions: (pairs: [(from: "rain", to: "snow", seconds: 1.0)])"#,
            "transitions.pairs[0].to",
        ),
        (
            r#"states: [(name: "rain", look: Some((keys: [(time: 12.0, weather: Some((wetness: 1.0)))])))]"#,
            "states[0].look",
        ),
    ];
    for (text, field) in cases {
        let message = document(text).expect_err(field).to_string();
        assert!(message.contains(&format!("`{field}`")), "`{message}` must name `{field}`");
    }
    let message = document(r#"states: [(name: "rain", hail: 1.0)]"#).expect_err("unknown").to_string();
    assert!(message.contains("hail"), "{message}");
}

/// At most two states, and each group blends as `WeatherFrame` says.
#[test]
fn a_blend_resolves_each_group_as_its_kind_does() {
    let mut clear = state("clear");
    clear.temperature = Some(TemperatureKey::Mild);
    let mut storm = state("storm");
    storm.precipitation =
        Some(PrecipitationKey { kind: PrecipitationKindKey::Rain, density: 1.0, size: 1.0, speed: 2.0 });
    storm.wind = Some(WeatherWindKey { strength: 3.0, gustiness: 0.8, turbulence: 0.6 });
    storm.lightning = Some(LightningKey { per_minute: 4.0 });
    storm.accumulation = Some(AccumulationKey {
        wetness: Some(ShareKey { target: 1.0, rise: 20.0 }),
        ..AccumulationKey::default()
    });
    storm.temperature = Some(TemperatureKey::Cold);
    storm.sea = Some(1.0);
    storm.environment =
        Some(WeatherEnvironmentKey { rings: 1.0, fog_density: 3.0, ..WeatherEnvironmentKey::default() });
    let mut drizzle = state("drizzle");
    drizzle.precipitation =
        Some(PrecipitationKey { kind: PrecipitationKindKey::Rain, density: 0.3, size: 0.5, speed: 1.0 });
    let mut snow = state("snow");
    snow.precipitation =
        Some(PrecipitationKey { kind: PrecipitationKindKey::Snow, density: 0.8, size: 1.0, speed: 1.0 });
    let document =
        WeatherDocument::new(vec![clear, storm, drizzle, snow], Transitions::default()).expect("valid");

    assert_eq!(WeatherDocument::frame(&document, WeatherBlend::settled(0)).live, 1);
    let quarter = document.frame(WeatherBlend { from: Some(0), to: 1, weight: 0.25 });
    assert_eq!((quarter.live, quarter.transition), (2, 0.25));
    assert_eq!(quarter.wind_share, 0.25, "only the storm sets a wind");
    assert_eq!(
        quarter.wind,
        WeatherWind { strength: 3.0, gustiness: 0.8, turbulence: 0.6 },
        "its own, by its share"
    );
    assert_eq!(quarter.lightning, 1.0);
    assert_eq!(quarter.sea, 0.25);
    assert_eq!(quarter.environment.rings, 0.25);
    assert_eq!(quarter.environment.fog_density, 1.5, "from the neutral 1 a quarter of the way to 3");
    let rain = quarter.precipitation[0].expect("the storm's rain");
    assert_eq!((rain.kind, rain.density), (PrecipitationKind::Rain, 0.25));
    assert_eq!(quarter.precipitation[1], None);
    assert_eq!(quarter.accumulation, None, "the clear sky dominates, and accumulates nothing");
    assert_eq!(quarter.temperature, Some(Temperature::Mild));
    let half = document.frame(WeatherBlend { from: Some(0), to: 1, weight: 0.5 });
    assert_eq!(half.temperature, Some(Temperature::Cold), "the incoming state from an even split");
    assert_eq!(half.accumulation.map(|a| a.wetness.target), Some(1.0));

    let same_kind = document.frame(WeatherBlend { from: Some(2), to: 1, weight: 0.5 });
    let rain = same_kind.precipitation[0].expect("one layer");
    assert_eq!(same_kind.precipitation[1], None, "the same kind blends into one layer");
    assert!((rain.density - 0.65).abs() < 1e-6);
    assert!((rain.size - (0.5 + 0.5 * (1.0 / 1.3))).abs() < 1e-6, "by the two's densities: {}", rain.size);
    let two_kinds = document.frame(WeatherBlend { from: Some(1), to: 3, weight: 0.75 });
    let [Some(outgoing), Some(incoming), None] = two_kinds.precipitation else {
        panic!("two layers: {:?}", two_kinds.precipitation);
    };
    assert_eq!((outgoing.kind, outgoing.density), (PrecipitationKind::Rain, 0.25));
    assert_eq!((incoming.kind, incoming.density), (PrecipitationKind::Snow, 0.6));

    assert_eq!(
        document.frame(WeatherBlend { from: Some(1), to: 1, weight: 0.3 }),
        document.frame(WeatherBlend::settled(1))
    );
    assert_eq!(
        document.frame(WeatherBlend { from: Some(1), to: 0, weight: f32::NAN }),
        document.frame(WeatherBlend::settled(1))
    );

    // Over no state at all, as a cutscene's state blends in over a zone with no plan: the rest is the day's.
    let coming = document.frame(WeatherBlend { from: None, to: 1, weight: 0.25 });
    assert_eq!((coming.live, coming.transition, coming.wind_share), (1, 0.25, 0.25));
    assert_eq!(coming.environment.fog_density, 1.5, "a quarter of the way from the day's 1 to 3");
    assert_eq!(coming.precipitation[0].map(|rain| rain.density), Some(0.25));
    assert_eq!((coming.accumulation, coming.temperature), (None, None), "the day still outweighs it");
    assert_eq!(document.frame(WeatherBlend { from: None, to: 1, weight: 0.0 }), WeatherFrame::default());
}

/// A group both states key reads exactly between them, the look beneath never showing through; a group
/// one keys fades over what's beneath; a settled state's look is its own, bit for bit.
#[test]
fn looks_blend_between_states_never_through_to_the_day() {
    let mut overcast = state("overcast");
    overcast.look = Some(day(0.8, Some(-0.5)));
    let mut storm = state("storm");
    storm.look = Some(day(1.0, None));
    let document = WeatherDocument::new(vec![overcast, storm], Transitions::default()).expect("valid");
    let base = || {
        let mut look = TodLook::default();
        look.clouds.coverage = 0.0;
        look.exposure = 1.0;
        look
    };
    let mut settled = base();
    document.apply_looks(WeatherBlend::settled(1), &mut settled, 12.0);
    assert_eq!(settled.clouds.coverage, 1.0);
    assert_eq!(settled.exposure, 1.0, "the storm keys no exposure, so the day's stands");
    let mut look = base();
    document.apply_looks(WeatherBlend { from: Some(0), to: 1, weight: 0.25 }, &mut look, 12.0);
    assert!((look.clouds.coverage - 0.85).abs() < 1e-6, "between the two states: {}", look.clouds.coverage);
    assert!(
        (look.exposure - (1.0 - 1.5 * 0.75)).abs() < 1e-6,
        "overcast's exposure by its weight over the day's"
    );
    let mut out = base();
    document.apply_looks(WeatherBlend { from: Some(1), to: 0, weight: 1.0 }, &mut out, 12.0);
    let mut alone = base();
    document.apply_looks(WeatherBlend::settled(0), &mut alone, 12.0);
    assert_eq!(out, alone);
}
