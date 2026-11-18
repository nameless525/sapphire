//! The weather system: the shipped states, choosing by layer, transitions in world seconds, and the
//! schedule as a pure function of the clock.

use sapphire_tod::TodGroup;
use sapphire_zone::{CookedWeatherRegion, WeatherChoice, WeatherSlot, ZoneShape};

use super::*;

const NAMES: [&str; 10] =
    ["clear", "overcast", "drizzle", "rain", "storm", "snow", "blizzard", "hail", "sandstorm", "mist"];

fn index(document: &WeatherDocument, name: &str) -> usize {
    document.state(name).unwrap_or_else(|| panic!("`{name}` is shipped"))
}

/// A day of `day_seconds` of world time from day 0's midnight: the clock at `seconds`.
fn clock(seconds: f64, day_seconds: f64) -> WeatherClock {
    let days = seconds / day_seconds;
    let hours = ((days - days.floor()) * 24.0) as f32;
    WeatherClock {
        seconds,
        day: days.floor() as i64,
        hours: if hours >= 24.0 { 0.0 } else { hours },
        day_length: Some(day_seconds as f32),
    }
}

#[test]
fn the_shipped_states_are_the_ten_and_read_back() {
    let document = default_weather();
    let names: Vec<&str> = document.states().iter().map(|state| state.name.as_str()).collect();
    assert_eq!(names, NAMES);
    assert_eq!(WeatherDocument::from_ron(&document.to_ron().expect("write")).expect("read"), *document);
    assert!(Arc::ptr_eq(&document, &default_weather()), "parsed once");
    for state in document.states() {
        match &state.look {
            None => assert_eq!(state.name, "clear", "only `clear` is the day as it is"),
            Some(look) => {
                assert!(!look.groups().contains(TodGroup::Weather), "{}: wetness accumulates", state.name);
                assert!(
                    !look.groups().contains(TodGroup::Sun),
                    "{}: a state never moves the sun",
                    state.name
                );
            }
        }
        assert!(state.accumulation.is_some(), "{}: every state says where surfaces go", state.name);
        assert_eq!(
            state.wind.is_none(),
            state.name == "clear",
            "{}: only `clear` blows the zone's wind",
            state.name
        );
    }
    let clear = document.frame(WeatherBlend::settled(index(&document, "clear")));
    assert_eq!(clear.wind_share, 0.0, "clear leaves the zone's wind bit for bit");
    // Rough sense: the storm is the darkest noon, and snow brings snow.
    let noon = |name: &str| {
        let mut look = crate::tod::default_day().evaluate(12.0);
        document.apply_looks(WeatherBlend::settled(index(&document, name)), &mut look, 12.0);
        look.sky.horizon.length()
    };
    assert!(noon("storm") < noon("overcast") && noon("overcast") < noon("clear"));
    let snow = document.frame(WeatherBlend::settled(index(&document, "snow")));
    assert!(snow.accumulation.expect("keyed").snow.target > 0.5);
    let storm = document.frame(WeatherBlend::settled(index(&document, "storm")));
    assert!(storm.lightning > 0.0 && storm.precipitation[0].is_some());
}

/// The weather a frame shows hangs on the clock alone: one system resolving every few seconds and
/// another made new at each instant agree on the state everywhere, and on the whole frame wherever the
/// first has settled.
#[test]
fn the_schedule_is_a_pure_function_of_the_world_clock() {
    let document = default_weather();
    let (clear, rain, snow, mist) = (
        index(&document, "clear"),
        index(&document, "rain"),
        index(&document, "snow"),
        index(&document, "mist"),
    );
    let schedule = WeatherSchedule::new(
        11,
        vec![(0.0, vec![(clear, 2), (mist, 1)]), (9.0, vec![(rain, 1), (clear, 1), (snow, 1)])],
        10,
    )
    .expect("valid");
    let plan = WeatherPlan::new(schedule.clone(), Vec::new());
    let day = 600.0;
    let mut stepped = WeatherSystem::new(Arc::clone(&document), Some(plan.clone()));
    let mut settled_frames = 0;
    let mut changes = 0;
    let mut last = None;
    for step in 0..(day as u32 * 12 / 4) {
        let at = clock(f64::from(step) * 4.0, day);
        let frame = *stepped.resolve(at, Vec3::ZERO, None);
        let stats = stepped.stats();
        assert_eq!(stats.state, Some(schedule.state_at(at.day, at.hours)), "{at:?}");
        let mut fresh = WeatherSystem::new(Arc::clone(&document), Some(plan.clone()));
        assert_eq!(fresh.resolve(at, Vec3::ZERO, None).live, 1, "a new system arrives settled");
        assert_eq!(fresh.stats().state, stats.state);
        if stats.transition >= 1.0 {
            assert_eq!(&frame, fresh.frame(), "{at:?}");
            settled_frames += 1;
        }
        changes += usize::from(last.is_some_and(|last| last != stats.state));
        last = Some(stats.state);
    }
    assert!(changes >= 4 && settled_frames > 1000, "{changes} changes, {settled_frames} settled");
}

/// Transitions take the document's seconds of world time, whatever the hour does, and ease in and out.
#[test]
fn transitions_run_in_world_seconds() {
    let document = default_weather();
    let (clear, rain, storm) =
        (index(&document, "clear"), index(&document, "rain"), index(&document, "storm"));
    for day in [86_400.0, 60.0] {
        let mut system = WeatherSystem::new(Arc::clone(&document), Some(WeatherPlan::always(clear)));
        let at = |seconds: f64| WeatherClock { hours: 10.0, ..clock(seconds, day) };
        system.resolve(at(100.0), Vec3::ZERO, None);
        assert_eq!(system.blend(), Some(WeatherBlend::settled(clear)), "the first frame arrives settled");
        system.set_override(Some(WeatherOverride { state: rain, seconds: None })).expect("a state");
        system.resolve(at(100.0), Vec3::ZERO, None);
        assert_eq!(system.blend(), Some(WeatherBlend { from: Some(clear), to: rain, weight: 0.0 }));
        system.resolve(at(115.0), Vec3::ZERO, None);
        let half = system.blend().expect("live");
        assert!((half.weight - 0.5).abs() < 1e-6, "halfway through the document's 30 s: {half:?}");
        assert_eq!(system.frame().live, 2);
        system.resolve(at(122.5), Vec3::ZERO, None);
        let eased = system.blend().expect("live").weight;
        assert!(eased > 0.75 && eased < 0.9, "smoothstep at three quarters: {eased}");
        system.resolve(at(130.0), Vec3::ZERO, None);
        assert_eq!(system.blend(), Some(WeatherBlend { from: Some(clear), to: rain, weight: 1.0 }));
        assert_eq!(system.frame().live, 1);
        // A pair's own seconds, and an override's own.
        assert_eq!(document.seconds(clear, storm), 90.0);
        system.set_override(Some(WeatherOverride { state: clear, seconds: Some(10.0) })).expect("a state");
        system.resolve(at(200.0), Vec3::ZERO, None);
        system.resolve(at(205.0), Vec3::ZERO, None);
        assert!((system.blend().expect("live").weight - 0.5).abs() < 1e-6);
    }
}

/// Turning back mid-transition runs it the other way from where it is; a third state waits for the
/// transition to finish, quickly, on its heavier state. Never a jump, and never more than two live.
#[test]
fn a_change_of_mind_never_jumps_and_never_shows_three() {
    let document = default_weather();
    let (clear, rain, snow) = (index(&document, "clear"), index(&document, "rain"), index(&document, "snow"));
    let mut system = WeatherSystem::new(Arc::clone(&document), Some(WeatherPlan::always(clear)));
    let at = |seconds: f64| clock(seconds, 86_400.0);
    system.resolve(at(0.0), Vec3::ZERO, None);
    system.set_override(Some(WeatherOverride { state: rain, seconds: None })).expect("a state");
    system.resolve(at(0.0), Vec3::ZERO, None);
    system.resolve(at(9.0), Vec3::ZERO, None);
    let before = system.blend().expect("live");
    system.set_override(None).expect("none");
    system.resolve(at(9.0), Vec3::ZERO, None);
    let after = system.blend().expect("live");
    assert_eq!((after.from, after.to), (Some(rain), clear), "turned round");
    assert!((after.weight - (1.0 - before.weight)).abs() < 1e-5, "{before:?} {after:?}");
    system.resolve(at(12.0), Vec3::ZERO, None);
    assert!(system.blend().expect("live").weight > after.weight, "and on toward clear");

    // A third state: the transition finishes on its heavier state, clear, quickly and from where it is,
    // then snow comes in from clear. Every state's weight moves a little a frame, and two at most live.
    let before = weights(&system);
    system.set_override(Some(WeatherOverride { state: snow, seconds: None })).expect("a state");
    system.resolve(at(12.0), Vec3::ZERO, None);
    assert!(step(&before, &weights(&system)) < 1e-4, "no jump at the change");
    let mut last = before;
    let mut seconds = 12.0;
    while seconds < 60.0 {
        seconds += 0.1;
        let frame = system.resolve(at(seconds), Vec3::ZERO, None);
        assert!(frame.live <= 2, "{seconds}");
        let now = weights(&system);
        assert!(step(&last, &now) < 0.03, "a jump at {seconds}: {last:?} to {now:?}");
        last = now;
    }
    assert_eq!(
        system.blend(),
        Some(WeatherBlend { from: Some(clear), to: snow, weight: 1.0 }),
        "snow in the end"
    );
    // Back to rain while finishing toward clear for snow: the queued state changes, nothing jumps.
    system.set_override(Some(WeatherOverride { state: rain, seconds: None })).expect("a state");
    system.resolve(at(70.0), Vec3::ZERO, None);
    system.set_override(Some(WeatherOverride { state: clear, seconds: None })).expect("a state");
    let mut last = weights(&system);
    let mut seconds = 70.0;
    while seconds < 140.0 {
        seconds += 0.1;
        system.resolve(at(seconds), Vec3::ZERO, None);
        let now = weights(&system);
        assert!(step(&last, &now) < 0.03, "a jump at {seconds}: {last:?} to {now:?}");
        last = now;
    }
    assert_eq!(system.stats().state, Some(clear));
}

/// Each shipped state's weight in the last resolve: the world's blend under a cutscene's state.
fn weights(system: &WeatherSystem) -> [f32; 10] {
    let mut weights = [0.0; 10];
    let shot = system.shot();
    let under = 1.0 - shot.map_or(0.0, |(_, weight)| weight);
    for (state, weight) in system.blend().into_iter().flat_map(|blend| blend.live()) {
        weights[state] += weight * under;
    }
    if let Some((state, weight)) = shot {
        weights[state] += weight;
    }
    weights
}

/// The largest change of any state's weight between two frames.
fn step(last: &[f32; 10], now: &[f32; 10]) -> f32 {
    last.iter().zip(now).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
}

/// A cutscene naming a state comes in and goes out by its weight over a world in mid-transition, the
/// world's transition crossing its midpoint under the shot: no state's weight jumps, at the shot's start,
/// its end or the midpoint, and the frame's wind follows.
#[test]
fn a_cutscene_comes_and_goes_over_a_transition_without_a_jump() {
    let document = default_weather();
    let (clear, rain) = (index(&document, "clear"), index(&document, "rain"));
    let mut system = WeatherSystem::new(Arc::clone(&document), Some(WeatherPlan::always(clear)));
    let at = |seconds: f64| clock(seconds, 86_400.0);
    system.resolve(at(0.0), Vec3::ZERO, None);
    system.set_override(Some(WeatherOverride { state: rain, seconds: None })).expect("a state");
    let shot_weight =
        |seconds: f64| ((seconds - 5.0) / 4.0).clamp(0.0, 1.0).min(((25.0 - seconds) / 4.0).clamp(0.0, 1.0));
    let mut last = None;
    let mut last_wind: Option<f32> = None;
    let mut seconds = 0.0;
    let mut most_live = 0;
    while seconds < 35.0 {
        let weight = shot_weight(seconds) as f32;
        let shot = (weight > 0.0).then_some(("storm", weight));
        let frame = *system.resolve(at(seconds), Vec3::ZERO, shot);
        most_live = most_live.max(frame.live);
        let now = weights(&system);
        if let Some(last) = last {
            assert!(step(&last, &now) < 0.03, "a jump at {seconds}: {last:?} to {now:?}");
        }
        let wind = frame.wind.strength * frame.wind_share + (1.0 - frame.wind_share);
        if let Some(last_wind) = last_wind {
            assert!((wind - last_wind).abs() < 0.05, "the wind jumps at {seconds}: {last_wind} to {wind}");
        }
        (last, last_wind) = (Some(now), Some(wind));
        seconds += 0.1;
    }
    assert_eq!(most_live, 3, "clear, rain and the shot's storm while it eases in over the transition");
    assert_eq!(system.shot(), None, "the shot has gone");
    assert_eq!(system.blend(), Some(WeatherBlend { from: Some(clear), to: rain, weight: 1.0 }));
}

/// The override and the tool hold their states by name: a new document finds them again, or drops them.
#[test]
fn an_override_and_the_tool_follow_their_names_into_a_new_document() {
    let document = default_weather();
    let snow = index(&document, "snow");
    let mut system = WeatherSystem::new(Arc::clone(&document), None);
    system.set_override(Some(WeatherOverride { state: snow, seconds: Some(3.0) })).expect("a state");
    system
        .set_tool(Some(WeatherBlend { from: Some(index(&document, "rain")), to: snow, weight: 0.5 }))
        .expect("states");
    let two = r#"(type: "sapphire::world::WeatherDocument", version: 1, data: (states: [
        (name: "snow", look: (keys: [(time: 0.0, exposure: 1.0)])),
        (name: "rain", look: (keys: [(time: 0.0, exposure: 2.0)])),
    ]))"#;
    system.set_plan(Arc::new(WeatherDocument::from_ron(two).expect("valid")), None);
    assert_eq!(
        system.override_state(),
        Some(WeatherOverride { state: 0, seconds: Some(3.0) }),
        "snow, found again"
    );
    assert_eq!(system.tool(), Some(WeatherBlend { from: Some(1), to: 0, weight: 0.5 }));
    let one = r#"(type: "sapphire::world::WeatherDocument", version: 1, data: (states: [
        (name: "rain", look: (keys: [(time: 0.0, exposure: 2.0)])),
    ]))"#;
    system.set_plan(Arc::new(WeatherDocument::from_ron(one).expect("valid")), None);
    assert_eq!((system.override_state(), system.tool()), (None, None), "no snow here: both dropped");
}

/// The layers in order: the schedule, a region over it, an override over both, a cutscene's state by its
/// weight over the world's, and the tool's blend in place of everything.
#[test]
fn layers_choose_in_order() {
    let document = default_weather();
    let [clear, rain, snow, storm, mist] =
        ["clear", "rain", "snow", "storm", "mist"].map(|name| index(&document, name));
    let low = WeatherRegion {
        shape: VolumeShape::Sphere { center: Vec3::ZERO, radius: 10.0 },
        priority: 0,
        state: rain,
    };
    let high = WeatherRegion {
        shape: VolumeShape::Box { center: Vec3::ZERO, half_extents: Vec3::splat(2.0), yaw: 0.0 },
        priority: 3,
        state: mist,
    };
    let plan = WeatherPlan::new(WeatherSchedule::always(clear), vec![low, high]);
    assert_eq!(plan.regions()[0].state, mist, "highest priority first");
    let mut system = WeatherSystem::new(Arc::clone(&document), Some(plan));
    let at = clock(0.0, 86_400.0);
    let state = |system: &mut WeatherSystem, probe: Vec3| {
        system.resolve(at, probe, None);
        system.stats().state
    };
    assert_eq!(state(&mut system, Vec3::new(50.0, 0.0, 0.0)), Some(clear), "the schedule's");
    assert_eq!(state(&mut system, Vec3::new(5.0, 0.0, 0.0)), Some(rain), "a region's over it");
    assert_eq!(system.stats().region, Some(1));
    assert_eq!(state(&mut system, Vec3::ZERO), Some(mist), "the higher region where both are");
    system.set_override(Some(WeatherOverride { state: snow, seconds: Some(0.0) })).expect("a state");
    assert_eq!(state(&mut system, Vec3::ZERO), Some(snow), "an override over regions");
    assert_eq!(
        system.blend().map(|blend| (blend.to, blend.weight)),
        Some((snow, 1.0)),
        "at once: no seconds"
    );

    system.resolve(at, Vec3::ZERO, Some(("storm", 0.25)));
    assert_eq!(
        system.blend().map(|blend| (blend.to, blend.weight)),
        Some((snow, 1.0)),
        "the world's whole, under"
    );
    assert_eq!(system.shot(), Some((storm, 0.25)), "a cutscene's over it");
    assert_eq!(system.stats().cutscene_weight, 0.25);
    system.resolve(at, Vec3::ZERO, Some(("no such state", 1.0)));
    assert_eq!(system.shot(), None, "a name the document hasn't shows nothing");

    system.set_tool(Some(WeatherBlend::settled(clear))).expect("a state");
    system.resolve(at, Vec3::ZERO, Some(("storm", 1.0)));
    assert_eq!(
        (system.blend(), system.shot()),
        (Some(WeatherBlend::settled(clear)), None),
        "the tool's over everything"
    );
    assert!(system.set_tool(Some(WeatherBlend::settled(99))).is_err());
    assert!(system.set_override(Some(WeatherOverride { state: 99, seconds: None })).is_err());
    system.set_tool(None).expect("none");
    system.resolve(at, Vec3::ZERO, None);
    assert_eq!(system.blend().map(|blend| blend.to), Some(snow));

    // No plan: no weather, until something names a state.
    let mut bare = WeatherSystem::default();
    assert_eq!(bare.resolve(at, Vec3::ZERO, None).live, 0);
    assert_eq!(bare.blend(), None);
    bare.resolve(at, Vec3::ZERO, Some(("rain", 0.5)));
    assert_eq!((bare.blend(), bare.shot()), (None, Some((rain, 0.5))), "over the day as it is");
    assert_eq!(bare.frame().live, 1);
}

/// The world's weather lies in time of day's look through [`WeatherSystem::write`].
#[test]
fn the_system_hands_time_of_day_its_states() {
    let mut world = sapphire_time::ScaledClock::new();
    let mut tod = TimeOfDay::new(crate::tod::default_day(), world.new_local_clock(), 12.0).expect("complete");
    let base = tod.resolve(Vec3::ZERO, 1.0).look;
    let document = default_weather();
    let storm = index(&document, "storm");
    let mut system = WeatherSystem::new(document, Some(WeatherPlan::always(storm)));
    system.resolve(WeatherClock::of(&tod, 0.0, 1.0), Vec3::ZERO, None);
    system.write(&mut tod);
    let stormy = tod.resolve(Vec3::ZERO, 1.0).look;
    assert!(stormy.sky.horizon.length() < base.sky.horizon.length());
    assert_eq!(stormy.sun, base.sun);
}

#[test]
fn a_cooked_plan_reads_and_a_name_its_states_lack_is_refused() {
    let plan = |region_state: &str| CookedWeatherPlan {
        states: PRESETS.to_owned(),
        seed: 3,
        schedule: vec![
            WeatherSlot { from: 0.0, states: vec![WeatherChoice { state: "clear".into(), weight: 1 }] },
            WeatherSlot {
                from: 12.0,
                states: vec![
                    WeatherChoice { state: "rain".into(), weight: 2 },
                    WeatherChoice { state: "overcast".into(), weight: 1 },
                ],
            },
        ],
        regions: vec![CookedWeatherRegion {
            id: 1,
            name: "peak".into(),
            position: [0.0, 100.0, 0.0],
            shape: ZoneShape::Ellipsoid { radii: [40.0, 20.0, 40.0], yaw: 30.0 },
            state: region_state.into(),
            priority: 1,
        }],
    };
    let setup = WeatherSetup::read(&plan("snow")).expect("valid");
    assert_eq!(setup.document.states().len(), 10);
    assert_eq!(setup.plan.regions()[0].state, index(&setup.document, "snow"));
    assert_eq!(setup.plan.schedule().seed(), 3);
    assert_eq!(setup.plan.state_at(0, 10.0, Vec3::ZERO).0, index(&setup.document, "clear"));
    assert_eq!(
        setup.plan.state_at(0, 10.0, Vec3::new(0.0, 100.0, 0.0)),
        (index(&setup.document, "snow"), Some(0))
    );
    let error = WeatherSetup::read(&plan("monsoon")).expect_err("no such state");
    assert_eq!(error.field, "weather_plan.regions[0].state");
    let broken = CookedWeatherPlan { states: "(".into(), ..plan("snow") };
    assert_eq!(WeatherSetup::read(&broken).expect_err("doesn't read").field, "weather_plan.states");
}

// Accumulation (M6 Y2).

/// The scripted day: dry until six, then each coupling in turn (rain and a storm; snow over the puddles;
/// a freezing blizzard; a mild half hour melting it; a sandstorm and the drizzle that washes it; hail),
/// clear from half past eleven. Each slot one state.
fn scripted_day(document: &WeatherDocument) -> WeatherPlan {
    let slots = SCRIPT.iter().map(|&(from, name)| (from as f32, vec![(index(document, name), 1)])).collect();
    WeatherPlan::new(WeatherSchedule::new(5, slots, document.states().len()).expect("valid"), Vec::new())
}

/// The scripted day's slots: the game hour each starts and its one state.
const SCRIPT: [(f64, &str); 10] = [
    (0.0, "clear"),
    (6.0, "rain"),
    (7.0, "storm"),
    (7.5, "snow"),
    (8.5, "blizzard"),
    (9.5, "clear"),
    (10.0, "sandstorm"),
    (10.5, "drizzle"),
    (11.0, "hail"),
    (11.5, "clear"),
];

/// The scripted day's dominant state at `seconds` of world time, derived from its slots alone (not the
/// system's blend): each slot's state takes over halfway through the change into it, which starts at
/// the slot and lasts the document's seconds for the pair.
fn scripted_state(document: &WeatherDocument, seconds: f64) -> usize {
    let mut state = index(document, SCRIPT[0].1);
    for &(hours, name) in &SCRIPT[1..] {
        let next = index(document, name);
        if seconds >= hours * 900.0 + f64::from(document.seconds(state, next)) / 2.0 {
            state = next;
        }
    }
    state
}

/// A game hour of the scripted day takes 900 s of world time.
const DAY_SECONDS: f64 = 24.0 * 900.0;
/// Frames of 50 ms.
const FRAME: f64 = 0.05;

fn shares_of(weather: Weather) -> [f64; 5] {
    [weather.wetness, weather.puddles, weather.snow, weather.sand, weather.ice].map(f64::from)
}

/// The CPU reference: the shares' differential equations written out again from the module docs, each
/// share's rate `(target - share) / τ`, integrated by fourth-order Runge-Kutta in steps of 5 ms. It shares
/// no code with the integrator: not its couplings, its drying or its closed form.
struct Reference {
    shares: [f64; 5],
}

impl Reference {
    fn rates(state: Option<&sapphire_tod::weather::WeatherState>, shares: [f64; 5]) -> [f64; 5] {
        use sapphire_tod::weather::{ShareKey, TemperatureKey};
        let key = state.and_then(|state| state.accumulation).unwrap_or_default();
        let pair = |share: Option<ShareKey>| {
            share.map_or((0.0, 0.0), |share| (f64::from(share.target), f64::from(share.rise)))
        };
        let (mut wet, mut puddles, mut snow, sand, mut ice) =
            (pair(key.wetness), pair(key.puddles), pair(key.snow), pair(key.sand), pair(key.ice));
        let temperature = state.and_then(|state| state.temperature).unwrap_or(TemperatureKey::Mild);
        let freezing = temperature == TemperatureKey::Freezing;
        if matches!(temperature, TemperatureKey::Warm | TemperatureKey::Mild) {
            if snow.0 > wet.0 {
                wet = snow;
            }
            snow = (0.0, 0.0);
        }
        if freezing {
            for water in [wet, puddles] {
                if water.0 > ice.0 {
                    ice = water;
                }
            }
            (wet, puddles) = ((0.0, 0.0), (0.0, 0.0));
        }
        if shares[2] > 0.5 {
            puddles = (0.0, 0.0);
        }
        let sand_drying = if wet.0 > 0.0 { wet.1.min(1_800.0) } else { 1_800.0 };
        let targets = [wet, puddles, snow, sand, ice];
        let drying = [180.0, 900.0, 1_200.0, sand_drying, 900.0];
        std::array::from_fn(|share| {
            let ((target, rise), now) = (targets[share], shares[share]);
            if target > now {
                (target - now) / rise
            } else if target < now && !(freezing && (share == 2 || share == 4)) {
                (target - now) / drying[share]
            } else {
                0.0
            }
        })
    }

    fn advance(&mut self, state: Option<&sapphire_tod::weather::WeatherState>, seconds: f64) {
        let steps = (seconds / 0.005).round().max(1.0) as u32;
        let h = seconds / f64::from(steps);
        let add = |a: [f64; 5], b: [f64; 5], scale: f64| -> [f64; 5] {
            std::array::from_fn(|i| a[i] + b[i] * scale)
        };
        for _ in 0..steps {
            let s = self.shares;
            let k1 = Self::rates(state, s);
            let k2 = Self::rates(state, add(s, k1, h / 2.0));
            let k3 = Self::rates(state, add(s, k2, h / 2.0));
            let k4 = Self::rates(state, add(s, k3, h));
            self.shares =
                std::array::from_fn(|i| s[i] + h / 6.0 * (k1[i] + 2.0 * k2[i] + 2.0 * k3[i] + k4[i]));
        }
    }
}

/// The scripted day played frame by frame from midnight to noon: the shares stay within 1e-4 of the CPU
/// reference at every frame, through every coupling; and at each check a zone arriving (`set_plan`) and a
/// load (a system made new) re-warm to the shares the played day reached, within 1e-4.
#[test]
fn the_shares_follow_the_reference_through_a_scripted_day_and_a_rewarm_lands_on_it() {
    let document = default_weather();
    let plan = scripted_day(&document);
    let mut played = WeatherSystem::new(Arc::clone(&document), Some(plan.clone()));
    let mut reference = Reference { shares: [0.0; 5] };
    let checks =
        [7.25, 8.0, 9.0, 9.75, 10.25, 10.75, 11.25, 12.0].map(|hours: f64| (hours * 900.0 / FRAME) as u64);
    let mut worst = 0.0f64;
    let (mut crossed, mut froze, mut melted, mut washed) = (false, false, false, false);
    let mut last = Weather::DRY;
    let mut before = 0;
    let (mut flips, mut last_state) = (0, None);
    for frame in 0..=(12 * 900 * 20u64) {
        let seconds = frame as f64 * FRAME;
        played.resolve(clock(seconds, DAY_SECONDS), Vec3::ZERO, None);
        if frame > 0 {
            reference.advance(document.states().get(before), FRAME);
        }
        // The state the shares follow from this frame on is the script's, and the system agrees.
        before = scripted_state(&document, seconds);
        let dominant = played.blend().and_then(|blend| blend.dominant());
        assert_eq!(dominant, Some(before), "the dominant state at {seconds} s");
        flips += usize::from(frame > 0 && Some(before) != last_state);
        last_state = Some(before);
        let shares = played.shares();
        for (got, want) in shares_of(shares).into_iter().zip(reference.shares) {
            worst = worst.max((got - want).abs());
        }
        crossed |= last.snow <= 0.5 && shares.snow > 0.5;
        froze |= shares.ice > 0.3;
        melted |= last.snow > 0.5 && shares.snow <= 0.5;
        washed |= shares.sand < last.sand && before == index(&document, "drizzle");
        last = shares;
        if checks.contains(&frame) {
            let at = clock(seconds, DAY_SECONDS);
            let mut arrived = WeatherSystem::new(Arc::clone(&document), None);
            arrived.resolve(clock(seconds - 1.0, DAY_SECONDS), Vec3::ZERO, None);
            arrived.set_plan(Arc::clone(&document), Some(plan.clone()));
            arrived.resolve(at, Vec3::ZERO, None);
            let mut loaded = WeatherSystem::new(Arc::clone(&document), Some(plan.clone()));
            loaded.resolve(at, Vec3::ZERO, None);
            for (what, warm) in [("a zone's arrival", arrived.shares()), ("a load", loaded.shares())] {
                for (got, want) in shares_of(warm).into_iter().zip(shares_of(shares)) {
                    assert!((got - want).abs() < 1e-4, "{what} at {}: {warm:?} against {shares:?}", at.hours);
                }
            }
            assert_eq!((arrived.stats().rewarms, loaded.stats().rewarms), (2, 1));
        }
    }
    assert!(worst < 1e-4, "the shares strayed {worst} from the reference");
    assert_eq!(flips, SCRIPT.len() - 1, "every slot's state took over");
    assert!(crossed && froze && melted && washed, "{crossed} {froze} {melted} {washed}: every coupling ran");
    assert_eq!(played.stats().rewarms, 1, "the played day re-warmed once, at its start");
    assert_eq!(played.stats().shares, played.shares());
}

/// Every rate is in world seconds: a world clock running twice as fast reaches the same shares in half
/// the frames, a paused one holds them, and time of day running faster shortens the six hours of
/// weather a re-warm reaches back through.
#[test]
fn an_accelerated_clock_accelerates_the_shares() {
    let document = default_weather();
    let (clear, rain) = (index(&document, "clear"), index(&document, "rain"));
    let run = |frames: u32, step: f64| {
        // Arriving dry from a clear day's last hours, under a story flag's rain.
        let mut system = WeatherSystem::new(Arc::clone(&document), Some(WeatherPlan::always(clear)));
        system.set_override(Some(WeatherOverride { state: rain, seconds: None })).expect("a state");
        system.resolve(clock(0.0, DAY_SECONDS), Vec3::ZERO, None);
        assert_eq!(system.shares(), Weather::DRY, "an override isn't replayed");
        for frame in 1..=frames {
            system.resolve(clock(f64::from(frame) * step, DAY_SECONDS), Vec3::ZERO, None);
        }
        system.shares()
    };
    let real = run(1_200, 0.05);
    let double = run(600, 0.1);
    assert!(real.wetness > 0.7 && (real.wetness - double.wetness).abs() < 1e-6, "{real:?} {double:?}");
    assert!((real.puddles - double.puddles).abs() < 1e-6);
    // Paused: the world's seconds stand, and so do the shares.
    let mut paused = WeatherSystem::new(Arc::clone(&document), Some(WeatherPlan::always(clear)));
    paused.resolve(clock(10.0, DAY_SECONDS), Vec3::ZERO, None);
    paused.set_override(Some(WeatherOverride { state: rain, seconds: Some(0.0) })).expect("a state");
    paused.resolve(clock(20.0, DAY_SECONDS), Vec3::ZERO, None);
    let held = paused.shares();
    for _ in 0..100 {
        paused.resolve(clock(20.0, DAY_SECONDS), Vec3::ZERO, None);
    }
    assert_eq!(paused.shares(), held);
    // Two game hours of rain before arriving at noon: under a day of real time the street is soaked and
    // the puddles nearly full; under a day of four minutes, ten world seconds a game hour, the rain began
    // 20 s before and weighed most 15 s into its 30 s transition, so it has wetted for 5 s.
    let plan = WeatherPlan::new(
        WeatherSchedule::new(1, vec![(0.0, vec![(clear, 1)]), (10.0, vec![(rain, 1)])], 10).expect("valid"),
        Vec::new(),
    );
    let arrive = |day: f64| {
        let mut system = WeatherSystem::new(Arc::clone(&document), Some(plan.clone()));
        system.resolve(WeatherClock { hours: 12.0, day: 3, ..clock(0.0, day) }, Vec3::ZERO, None);
        system.shares()
    };
    let slow = arrive(86_400.0);
    assert!(slow.wetness > 0.99 && slow.puddles > 0.59, "{slow:?}");
    let fast = arrive(240.0);
    let wet = 1.0 - (-5.0f64 / 45.0).exp();
    assert!((f64::from(fast.wetness) - wet).abs() < 1e-5, "{fast:?} against {wet}");
}

/// Time of day standing still, the weather has stood as it is: a re-warm settles the state the plan
/// shows for six hours of world time, across its 120 steps; and with no plan the world arrives dry.
#[test]
fn a_rewarm_with_time_standing_still_settles_the_state_as_it_is() {
    let document = default_weather();
    let snow = index(&document, "snow");
    let still = WeatherClock { seconds: 5.0, day: 0, hours: 9.0, day_length: None };
    let warm = rewarm(&document, &WeatherPlan::always(snow), still, Vec3::ZERO);
    let (toward, temperature) = accumulation(&document, Some(snow));
    let once = Weather::DRY.settle(toward, temperature, f64::from(REWARM_HOURS) * 3_600.0);
    for (got, want) in shares_of(warm).into_iter().zip(shares_of(once)) {
        assert!((got - want).abs() < 1e-6, "{warm:?} against {once:?}");
    }
    assert!(warm.snow > 0.79 && warm.puddles == 0.0, "settled snow over the puddles: {warm:?}");
    let mut system = WeatherSystem::default();
    system.resolve(still, Vec3::ZERO, None);
    assert_eq!((system.shares(), system.stats().rewarms), (Weather::DRY, 1));
}

/// The shares are the world's: the schedule's and an override's states move them, while a cutscene's
/// state and the tool's blend lay a look over the world and leave the street as the world made it.
#[test]
fn only_the_worlds_state_moves_the_shares() {
    let document = default_weather();
    let (clear, storm) = (index(&document, "clear"), index(&document, "storm"));
    let wet_after = |set: &dyn Fn(&mut WeatherSystem), cutscene: Option<(&str, f32)>| {
        let mut system = WeatherSystem::new(Arc::clone(&document), Some(WeatherPlan::always(clear)));
        let at = |seconds: f64| WeatherClock { hours: 10.0, ..clock(seconds, DAY_SECONDS) };
        system.resolve(at(0.0), Vec3::ZERO, None);
        set(&mut system);
        for frame in 1..=600 {
            system.resolve(at(f64::from(frame) * 0.5), Vec3::ZERO, cutscene);
        }
        system.shares().wetness
    };
    assert_eq!(wet_after(&|_| {}, None), 0.0, "a clear day stays dry");
    let flag = |system: &mut WeatherSystem| {
        system.set_override(Some(WeatherOverride { state: storm, seconds: None })).expect("a state");
    };
    assert!(wet_after(&flag, None) > 0.99, "a story flag's storm soaks the street");
    let tool = |system: &mut WeatherSystem| {
        system.set_tool(Some(WeatherBlend::settled(storm))).expect("a state");
    };
    assert_eq!(wet_after(&tool, None), 0.0, "the tool's storm is a look");
    assert_eq!(wet_after(&|_| {}, Some(("storm", 1.0))), 0.0, "and so is a cutscene's");
}

/// A clock set back (a tool's, never the world's own) holds the shares where they were and moves them
/// on from there: drying never runs backward into wetter streets, and a storm's soaking never undoes.
#[test]
fn a_clock_set_back_holds_the_shares() {
    let document = default_weather();
    let (clear, storm) = (index(&document, "clear"), index(&document, "storm"));
    let at = |seconds: f64| WeatherClock { hours: 10.0, ..clock(seconds, DAY_SECONDS) };
    let mut system = WeatherSystem::new(Arc::clone(&document), Some(WeatherPlan::always(clear)));
    system.resolve(at(1_000.0), Vec3::ZERO, None);
    system.set_override(Some(WeatherOverride { state: storm, seconds: None })).expect("a state");
    system.resolve(at(1_000.0), Vec3::ZERO, None);
    system.resolve(at(1_600.0), Vec3::ZERO, None);
    system.set_override(None).expect("cleared");
    // Clear again: drying from about 1,600 s, frame by frame.
    for second in 1_600..=1_800 {
        system.resolve(at(f64::from(second)), Vec3::ZERO, None);
    }
    let drying = system.shares();
    assert!(drying.wetness > 0.1 && drying.wetness < 0.9, "{drying:?}");
    // Back to 1,750 s: the shares hold, and dry on from there, 100 s on from 1,750 s as from 1,800 s.
    system.resolve(at(1_750.0), Vec3::ZERO, None);
    assert_eq!(system.shares(), drying);
    system.resolve(at(1_850.0), Vec3::ZERO, None);
    let on = drying.settle(Accumulation::default(), Temperature::Mild, 100.0);
    assert!((system.shares().wetness - on.wetness).abs() < 1e-6 && on.wetness < drying.wetness);
    // Back to 1,000 s, before the soaking began: they hold again, never running back to dry.
    let held = system.shares();
    system.resolve(at(1_000.0), Vec3::ZERO, None);
    assert_eq!(system.shares(), held);
}

/// What may go wet, so what declares the wet surfaces' pipelines: a plan with a state that wets, sleet
/// included, or a region's; never one of dry or freezing states alone; an override that wets.
#[test]
fn a_plan_may_go_wet_by_its_states() {
    let document = default_weather();
    let plan_of = |names: &[&str]| {
        let slots =
            names.iter().enumerate().map(|(slot, name)| (slot as f32, vec![(index(&document, name), 1)]));
        WeatherPlan::new(WeatherSchedule::new(0, slots.collect(), 10).expect("valid"), Vec::new())
    };
    let wets = |names: &[&str]| plan_of(names).may_be_wet(&document);
    assert!(!wets(&["clear", "overcast", "sandstorm"]), "dry states");
    assert!(!wets(&["blizzard"]), "at freezing, water is ice");
    assert!(wets(&["clear", "rain"]) && wets(&["snow"]) && wets(&["mist"]));
    let region = WeatherRegion {
        shape: VolumeShape::Sphere { center: Vec3::ZERO, radius: 5.0 },
        priority: 0,
        state: index(&document, "storm"),
    };
    let regioned = WeatherPlan::new(WeatherSchedule::always(index(&document, "clear")), vec![region]);
    assert!(regioned.may_be_wet(&document), "a region's state counts");
    let mut system = WeatherSystem::new(Arc::clone(&document), Some(plan_of(&["clear"])));
    assert!(!system.may_be_wet());
    let rain = index(&document, "rain");
    system.set_override(Some(WeatherOverride { state: rain, seconds: None })).expect("a state");
    assert!(system.may_be_wet(), "a story flag's rain");
}

/// The rainbow's condition reads the world's shares and the frame's rain: none while it pours, a bow
/// a minute after it stops under a low sun, none under a closed sky.
#[test]
fn a_rainbow_comes_as_the_rain_ends() {
    let document = default_weather();
    let (clear, rain) = (index(&document, "clear"), index(&document, "rain"));
    let slots = vec![(0.0, vec![(rain, 1)]), (12.0, vec![(clear, 1)])];
    let plan = WeatherPlan::new(WeatherSchedule::new(1, slots, 10).expect("valid"), Vec::new());
    let mut system = WeatherSystem::new(Arc::clone(&document), Some(plan));
    let low_sun = Vec3::new(0.3, 0.3, -1.0);
    let at = |seconds: f64| clock(seconds, DAY_SECONDS);
    let mut seconds = 11.0 * 900.0;
    system.resolve(at(seconds), Vec3::ZERO, None);
    assert_eq!(system.rainbow(low_sun, 0.3).strength, 0.0, "pouring");
    while seconds < 12.0 * 900.0 + 60.0 {
        seconds += 0.5;
        system.resolve(at(seconds), Vec3::ZERO, None);
    }
    let bow = system.rainbow(low_sun, 0.3);
    assert!(bow.strength > 0.9, "{bow:?}");
    assert!((bow.antisolar + low_sun.normalize()).length() < 1e-6);
    assert_eq!(system.rainbow(low_sun, 0.95).strength, 0.0, "overcast hides the sun");
}
