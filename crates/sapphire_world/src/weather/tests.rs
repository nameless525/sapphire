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
    WeatherClock { seconds, day: days.floor() as i64, hours: if hours >= 24.0 { 0.0 } else { hours } }
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
