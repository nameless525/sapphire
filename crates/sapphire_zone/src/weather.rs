//! A zone's weather plan (M6 W8a): which of a weather document's states its sky shows, and where, as
//! presentation alone.
//!
//! - **`states`**: a `WeatherDocument`, from this document's directory, as `day` is: the cook checks it,
//!   holds its text in the zone and holds every state the plan names to it. The shipped presets are
//!   `content/weather/presets.weather.ron`.
//! - **`schedule`**: the day in slots, each from its hour to the next slot's (the last wrapping past
//!   midnight to the first's), each a weighted choice of states. Which a slot shows on a day is a hash
//!   of `seed`, the day and the slot, so it's the same every time the clock reads that day and hour,
//!   and nothing is saved. One slot of one state is that weather all day.
//! - **`regions`**: places with weather of their own, each a [`ZoneShape`] about its position naming a
//!   state, which shows while the camera is inside it, over the schedule. Where regions overlap, the
//!   highest `priority` wins, and of equals the first by ID.
//!
//! ```ron
//! weather_plan: Some((
//!     states: "../../weather/presets.weather.ron",
//!     seed: 7,
//!     schedule: [
//!         (from: 6.0, states: [(state: "clear", weight: 3), (state: "overcast")]),
//!         (from: 15.0, states: [(state: "rain"), (state: "overcast")]),
//!         (from: 21.0, states: [(state: "mist")]),
//!     ],
//!     regions: [(name: "summit", position: (0.0, 80.0, -120.0), shape: Ellipsoid(radii: (60.0, 30.0, 40.0), yaw: 20.0), state: "snow")],
//! )),
//! ```
//!
//! **Presentation only:** the plan cooks into [`ZonePresentation::weather_plan`](crate::ZonePresentation),
//! which no digest reads, so no replay moves. A zone's fixed `weather` (its wet shares, M5 L4) is still
//! read as it was.

use bevy_reflect::Reflect;
use sapphire_core::Name;
use serde::{Deserialize, Serialize};

use crate::cooked::{ArchivedZonePresentation, check_ids};
use crate::document::{ZoneError, check_name, check_names, check_path, check_position};
use crate::shape::ZoneShape;

/// The most slots a zone's day may hold.
pub const MAX_SLOTS: usize = 48;
/// The most choices a slot may hold.
pub const MAX_CHOICES: usize = 64;
/// The most weather regions a zone may hold.
pub const MAX_REGIONS: usize = 64;

/// A zone's weather plan, as its document writes it (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct ZoneWeatherPlan {
    /// A `WeatherDocument`, from this document's directory.
    pub states: String,
    #[serde(default)]
    pub seed: u32,
    pub schedule: Vec<WeatherSlot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<WeatherRegion>,
}

/// One slot of a zone's day: from its hour to the next slot's, a weighted choice of states. The same
/// plain value in the document and in the cooked zone.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
#[rkyv(derive(Debug))]
pub struct WeatherSlot {
    /// Its first hour, `0 ≤ from < 24`, after the slot before it.
    pub from: f32,
    pub states: Vec<WeatherChoice>,
}

/// A state a slot may show, and how often against the slot's others.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
#[rkyv(derive(Debug))]
pub struct WeatherChoice {
    /// One of the weather document's states, by name.
    pub state: String,
    /// Above 0; 1 by default.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub weight: u32,
}

fn one() -> u32 {
    1
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_one(value: &u32) -> bool {
    *value == 1
}

/// A place with weather of its own (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct WeatherRegion {
    /// Unique among the zone's regions: its identity.
    pub name: String,
    /// The shape's centre, in metres.
    pub position: [f32; 3],
    pub shape: ZoneShape,
    /// One of the weather document's states, by name.
    pub state: String,
    /// Which wins where regions overlap: the highest.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: u8,
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_zero(value: &u8) -> bool {
    *value == 0
}

/// A zone's weather plan, cooked: the states' document as its text, the cook having checked it and held
/// every name here to it.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedWeatherPlan {
    /// The `WeatherDocument`, as RON text.
    pub states: String,
    pub seed: u32,
    pub schedule: Vec<WeatherSlot>,
    /// By ascending ID.
    pub regions: Vec<CookedWeatherRegion>,
}

/// A weather region, cooked: see [`WeatherRegion`].
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedWeatherRegion {
    /// The name's hash ([`Name`]).
    pub id: u64,
    pub name: String,
    pub position: [f32; 3],
    pub shape: ZoneShape,
    pub state: String,
    pub priority: u8,
}

impl ArchivedCookedWeatherRegion {
    pub fn position(&self) -> [f32; 3] {
        self.position.each_ref().map(|value| value.to_native())
    }
}

impl ArchivedCookedWeatherPlan {
    /// The plan as plain values: what a reader takes off the frame's thread to parse its states.
    #[must_use]
    pub fn to_native(&self) -> CookedWeatherPlan {
        CookedWeatherPlan {
            states: self.states.as_str().to_owned(),
            seed: self.seed.to_native(),
            schedule: self.schedule.iter().map(ArchivedWeatherSlot::to_native).collect(),
            regions: self
                .regions
                .iter()
                .map(|region| CookedWeatherRegion {
                    id: region.id.to_native(),
                    name: region.name.as_str().to_owned(),
                    position: region.position(),
                    shape: region.shape.to_native(),
                    state: region.state.as_str().to_owned(),
                    priority: region.priority,
                })
                .collect(),
        }
    }
}

impl ArchivedWeatherSlot {
    #[must_use]
    pub fn to_native(&self) -> WeatherSlot {
        WeatherSlot {
            from: self.from.to_native(),
            states: self
                .states
                .iter()
                .map(|choice| WeatherChoice {
                    state: choice.state.as_str().to_owned(),
                    weight: choice.weight.to_native(),
                })
                .collect(),
        }
    }
}

impl ZoneWeatherPlan {
    /// Every state the plan names, the schedule's then the regions', each once in the order first
    /// named: what a cook holds to the states' document.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        let scheduled =
            self.schedule.iter().flat_map(|slot| slot.states.iter().map(|choice| choice.state.as_str()));
        for name in scheduled.chain(self.regions.iter().map(|region| region.state.as_str())) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }
}

/// Whether a document's plan is one a cook can take, naming the field of what isn't. Whether its states
/// are the document's is the cook's to check, which reads the document.
pub(crate) fn check(plan: &ZoneWeatherPlan) -> Result<(), ZoneError> {
    let invalid =
        |name: String| move |problem: String| ZoneError::invalid(format!("weather_plan.{name}"), problem);
    check_path(&plan.states).map_err(invalid("states".to_owned()))?;
    check_schedule(&plan.schedule).map_err(|(field, problem)| invalid(field)(problem))?;
    if plan.regions.len() > MAX_REGIONS {
        let problem =
            format!("{} regions is more than the {MAX_REGIONS} a zone may hold", plan.regions.len());
        return Err(ZoneError::invalid("weather_plan.regions", problem));
    }
    check_names("weather_plan.regions", plan.regions.iter().map(|region| region.name.as_str()))?;
    for (index, region) in plan.regions.iter().enumerate() {
        let field = |name: &str| format!("regions[{index}].{name}");
        check_position(region.position).map_err(invalid(field("position")))?;
        region.shape.check().map_err(invalid(field("shape")))?;
        check_name(&region.state).map_err(invalid(field("state")))?;
    }
    Ok(())
}

/// Whether a schedule's slots make a day: 1 to [`MAX_SLOTS`], their hours within it and rising, each
/// slot 1 to [`MAX_CHOICES`] states, each named and weighing something. Returns the field and what's
/// wrong.
fn check_schedule(schedule: &[WeatherSlot]) -> Result<(), (String, String)> {
    if schedule.is_empty() || schedule.len() > MAX_SLOTS {
        return Err(("schedule".to_owned(), format!("{} slots isn't from 1 to {MAX_SLOTS}", schedule.len())));
    }
    let mut previous: Option<f32> = None;
    for (index, slot) in schedule.iter().enumerate() {
        let field = |name: &str| format!("schedule[{index}].{name}");
        if !(slot.from.is_finite() && (0.0..24.0).contains(&slot.from)) {
            return Err((field("from"), format!("{} isn't 0 ≤ hours < 24", slot.from)));
        }
        // Slots start on hundredths of an hour (`WeatherSchedule`), so two closer than that are one.
        if previous.is_some_and(|previous| (slot.from * 100.0).round() <= (previous * 100.0).round()) {
            return Err((
                field("from"),
                "hours must rise by a hundredth or more from slot to slot".to_owned(),
            ));
        }
        previous = Some(slot.from);
        if slot.states.is_empty() || slot.states.len() > MAX_CHOICES {
            return Err((
                field("states"),
                format!("{} states isn't from 1 to {MAX_CHOICES}", slot.states.len()),
            ));
        }
        for (choice_index, choice) in slot.states.iter().enumerate() {
            let choice_field = |name: &str| format!("schedule[{index}].states[{choice_index}].{name}");
            check_name(&choice.state).map_err(|problem| (choice_field("state"), problem))?;
            if choice.weight == 0 {
                return Err((choice_field("weight"), "it weighs nothing".to_owned()));
            }
        }
    }
    Ok(())
}

/// A checked document's plan, cooked with its states' document's `text`.
pub(crate) fn cook(plan: &ZoneWeatherPlan, text: String) -> CookedWeatherPlan {
    let mut regions: Vec<CookedWeatherRegion> = plan
        .regions
        .iter()
        .map(|region| CookedWeatherRegion {
            id: Name::new(&region.name).as_u64(),
            name: region.name.clone(),
            position: region.position,
            shape: region.shape,
            state: region.state.clone(),
            priority: region.priority,
        })
        .collect();
    regions.sort_unstable_by_key(|region| region.id);
    CookedWeatherPlan { states: text, seed: plan.seed, schedule: plan.schedule.clone(), regions }
}

/// Whether a cooked zone's plan is one a cook would write, whatever wrote it.
pub(crate) fn validate(presentation: &ArchivedZonePresentation) -> Result<(), ZoneError> {
    let Some(plan) = presentation.weather_plan.as_ref() else { return Ok(()) };
    let invalid =
        |field: String| move |problem: String| ZoneError::invalid(format!("weather_plan.{field}"), problem);
    if plan.states.is_empty() {
        return Err(ZoneError::invalid("weather_plan.states", "the document's text is empty"));
    }
    let schedule: Vec<WeatherSlot> = plan.schedule.iter().map(ArchivedWeatherSlot::to_native).collect();
    check_schedule(&schedule).map_err(|(field, problem)| invalid(field)(problem))?;
    let regions = &plan.regions;
    if regions.len() > MAX_REGIONS {
        let problem = format!("{} regions is more than the {MAX_REGIONS} a zone may hold", regions.len());
        return Err(ZoneError::invalid("weather_plan.regions", problem));
    }
    check_ids(
        "weather_plan.regions",
        regions.iter().map(|region| (region.id.to_native(), region.name.as_str())),
    )?;
    for region in regions.iter() {
        let field = format!("regions `{}`", region.name.as_str());
        check_position(region.position()).map_err(invalid(field.clone()))?;
        region.shape.to_native().check().map_err(invalid(field.clone()))?;
        check_name(region.state.as_str()).map_err(invalid(field))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{ZoneDocument, ZoneError};

    fn document(plan: &str) -> Result<ZoneDocument, ZoneError> {
        ZoneDocument::from_ron(&format!(
            r#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 32.0), weather_plan: Some(({plan}))))"#
        ))
    }

    const STATES: &str = r#"states: "../weather/presets.weather.ron""#;

    fn plan(more: &str) -> String {
        format!(
            r#"{STATES}, schedule: [(from: 6.0, states: [(state: "clear", weight: 3), (state: "rain")]), (from: 20.0, states: [(state: "mist")])]{more}"#
        )
    }

    fn summit(more: &str) -> String {
        plan(&format!(
            r#", regions: [(name: "summit", position: (0.0, 80.0, 0.0), shape: Sphere(radius: 40.0), state: "snow"{more})]"#
        ))
    }

    /// A plan's shares lie over the zone's fixed weather (M6 Y2): the cook warns of a zone that sets both.
    #[test]
    fn a_fixed_weather_beside_a_plan_is_warned_of() {
        let planned = document(&plan("")).expect("a good document");
        assert!(planned.warnings().is_empty(), "a plan alone");
        let both = ZoneDocument::from_ron(&format!(
            r#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 32.0), weather: Some((wetness: 0.5, puddles: 0.2)), weather_plan: Some(({}))))"#,
            plan("")
        ))
        .expect("a good document");
        let warnings = both.warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("weather:") && warnings[0].contains("shadowed"), "{warnings:?}");
        let fixed = ZoneDocument { weather_plan: None, ..both };
        assert!(fixed.warnings().is_empty(), "fixed weather alone");
    }

    #[test]
    fn a_plan_takes_its_defaults_and_names_its_states_once() {
        let document = document(&summit("")).expect("a good document");
        let plan = document.weather_plan.as_ref().expect("there");
        assert_eq!(plan.seed, 0);
        assert_eq!(plan.schedule[1].states[0].weight, 1, "a choice weighs 1 by default");
        assert_eq!(plan.regions[0].priority, 0);
        assert_eq!(plan.names(), ["clear", "rain", "mist", "snow"]);
        let text = sapphire_serialize::to_ron(&document).expect("writes");
        assert!(
            !text.contains("weight: 1") && text.contains("weight: 3") && !text.contains("priority"),
            "{text}"
        );
        assert_eq!(ZoneDocument::from_ron(&text).expect("reads back"), document);
    }

    #[test]
    fn a_bad_plan_is_refused_by_its_field() {
        let cases = [
            (format!("{STATES}, schedule: []"), "weather_plan.schedule"),
            (plan("").replace("from: 6.0", "from: 24.0"), "weather_plan.schedule[0].from"),
            (plan("").replace("from: 20.0", "from: 6.0"), "weather_plan.schedule[1].from"),
            (plan("").replace("[(state: \"mist\")]", "[]"), "weather_plan.schedule[1].states"),
            (plan("").replace("weight: 3", "weight: 0"), "weather_plan.schedule[0].states[0].weight"),
            (plan("").replace("\"rain\"", "\"\""), "weather_plan.schedule[0].states[1].state"),
            (plan("").replace("../weather", "/weather"), "weather_plan.states"),
            (summit("").replace("radius: 40.0", "radius: 0.0"), "weather_plan.regions[0].shape"),
            (summit("").replace("(0.0, 80.0, 0.0)", "(1e9, 80.0, 0.0)"), "weather_plan.regions[0].position"),
            (summit("").replace("\"snow\"", "\"\""), "weather_plan.regions[0].state"),
            (summit("").replace(r#"regions: ["#, r#"regions: [(name: "summit", position: (0.0, 0.0, 0.0), shape: Sphere(radius: 1.0), state: "snow"), "#), "weather_plan.regions[1].name"),
        ];
        for (plan, field) in cases {
            let message = document(&plan).expect_err(field).to_string();
            assert!(message.contains(field), "`{message}` must name `{field}`");
        }
        let message = document(&plan(", rain: 1.0")).expect_err("unknown").to_string();
        assert!(message.contains("rain"), "{message}");
    }
}
