//! Scripted input as documents (M4 R2): `content/inputs/<name>.inputs.ron`, each a [`Script`] found by its
//! file's name. `build.rs` compiles every one in, as it does scenarios, so a headless runner still needs no
//! assets, and a new replay's input takes no Rust.
//!
//! **What a document says,** a tick at a time, for each of a scenario's cast by its place, the player first:
//!
//! - `tracks`, each driving one of the cast (`who: Cast(n)`, the player by default) or all of them
//!   (`who: Everyone(stagger: n)`, the cast's `i`-th member `n × i` ticks ahead of the first). A track's
//!   `presses` are schedules: buttons pressed at ticks counted `from` its start, `every` so many ticks over
//!   again if it says, each held `hold` ticks (1 by default). A track's `stick` is `Legs`, each a stick held
//!   for so many ticks, eased in over its first `ease` and with a jump `jump` ticks in if it says, then at rest;
//!   `Loop`, the same legs over and over; or `Sweep`, each axis counting up a step a tick as a signed byte,
//!   from 0 and over again every `x` and `y` ticks. A tick's buttons are every track's together; a member's
//!   stick is the one track that gives it one.
//! - `commands`, each before the step of its tick ([`ReplayCommand`]): a timeline, a skip, a zone entry, a
//!   return to a checkpoint, a dialogue's effect or an auto-target strength. They name what a scenario loaded
//!   by its number, as a replay's file does: its first timeline is 0.
//! - `like`, another document's name, whose tracks and commands come first: `cinematic-watched` is
//!   `melee-sweep`'s with a cinematic on top. A document named by `like` names no other.
//!
//! A script reads the tick and nothing else, so it can't aim at anyone: a walk's legs are fitted to the
//! ground by running them, and the replays' checksums say when a change to the controller or a collider
//! leaves one pressing into something it used to clear.
//!
//! [`Script::actions`] is integer arithmetic over what the document held, with no heap, so a source can drive
//! a fixed step.

use std::sync::LazyLock;

use bevy_reflect::TypePath;
use sapphire_dialogue::EffectId;
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use super::document::INPUTS_DOCUMENTS;
use super::file::{MAX_TICKS, ReplayCommand};
use crate::actions::{ActorActions, Buttons, Stick, TickActions};
use crate::actors::ActorId;
use crate::checkpoint::CheckpointId;
use crate::dialogue::{DialogueEffect, DialogueId};
use crate::targeting::AutoTarget;
use crate::timeline::TimelineId;
use crate::zone::{ZoneEntry, ZoneId};

/// A scripted input document (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::gameplay"]
#[serde(deny_unknown_fields)]
pub struct InputsDocument {
    /// A line on what it does, for listings.
    pub about: String,
    /// How many ticks to record when nothing says otherwise.
    pub ticks: u32,
    /// Another document, by name, whose tracks and commands come before this one's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub like: Option<String>,
    /// What the cast press and hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
    /// What it does between steps, each with the tick it comes before, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<(u32, Command)>,
}

impl Versioned for InputsDocument {
    /// 1: M4 R2.
    const VERSION: u32 = 1;
}

/// What one of the cast does, or all of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    #[serde(default)]
    pub who: Who,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presses: Vec<Schedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stick: Option<StickPath>,
}

/// Whom a [`Track`] drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Who {
    /// One of the cast, by its place: 0 is the player.
    Cast(u32),
    /// Each of the cast, the `i`-th `stagger × i` ticks ahead of the first.
    Everyone { stagger: u32 },
}

impl Default for Who {
    fn default() -> Self {
        Self::Cast(0)
    }
}

/// Buttons pressed on a schedule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    /// The tick it starts on, which is its tick 0.
    #[serde(default)]
    pub from: u32,
    /// It starts over this many ticks after each start, if it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<u32>,
    /// How many ticks each press is held.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub hold: u32,
    /// Its presses: the tick, counted from its start, and the buttons. Ticks go up.
    pub at: Vec<(u32, Vec<Button>)>,
}

fn one() -> u32 {
    1
}

#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde's skip_serializing_if takes a reference")]
fn is_one(value: &u32) -> bool {
    *value == 1
}

/// A button, as [`Buttons`] names them. Touch is the app's to set, never a script's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    Light,
    Heavy,
    Dodge,
    Skill,
    Ultimate,
    Swap,
    LockOn,
    Jump,
}

impl Button {
    const fn buttons(self) -> Buttons {
        match self {
            Self::Light => Buttons::LIGHT,
            Self::Heavy => Buttons::HEAVY,
            Self::Dodge => Buttons::DODGE,
            Self::Skill => Buttons::SKILL,
            Self::Ultimate => Buttons::ULTIMATE,
            Self::Swap => Buttons::SWAP,
            Self::LockOn => Buttons::LOCK_ON,
            Self::Jump => Buttons::JUMP,
        }
    }
}

/// Where a [`Track`] holds the stick, in the world's axes (x along +X, y along +Z): 127 is a run, 6 m/s by the
/// default profile, and 64 a walk, 2 m/s.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StickPath {
    /// Each leg in turn, then at rest.
    Legs(Vec<Leg>),
    /// Each leg in turn, over and over.
    Loop(Vec<Leg>),
    /// Each axis counts up a step a tick as a signed byte (0 to 127, then −128, read as −127, and up), from 0
    /// again every `x` and `y` ticks: every place the stick can be, on no tick the same as another.
    Sweep { x: u32, y: u32 },
}

/// A stick held for `ticks`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Leg {
    /// −127 to 127 on each axis.
    pub stick: (i8, i8),
    pub ticks: u32,
    /// Ticks it takes to reach `stick` from the middle, a thumb's way: on its `k`-th tick it's `(k + 1) / ease`
    /// of the way, in whole percent, each axis rounded toward the middle.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ease: u32,
    /// The tick, counted from the leg's start, it presses jump on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<u32>,
}

#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde's skip_serializing_if takes a reference")]
fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// A [`ReplayCommand`], by the numbers a replay's file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// The scenario's timeline of this number, counting from 0 in its `timelines`.
    PlayTimeline(u32),
    /// The timeline playing is skipped, this far in.
    SkipTimeline { at_ms: u32 },
    /// The player goes into the shelf's zone of this number, at its spawner of this number, counting each
    /// zone's spawners in the order of their IDs.
    EnterZone { zone: u16, spawner: u16 },
    /// The player goes back to the zone's checkpoint of this number, in the order of their names' hashes.
    ReturnTo(u32),
    /// The effect of this number in the scenario's dialogue of this number.
    Dialogue { dialogue: u16, effect: u16 },
    /// The player's auto-target strength.
    AutoTarget(AutoTarget),
}

impl Command {
    const fn replay(self) -> ReplayCommand {
        match self {
            Self::PlayTimeline(id) => ReplayCommand::PlayTimeline(TimelineId::from_raw(id)),
            Self::SkipTimeline { at_ms } => ReplayCommand::SkipTimeline { at_ms },
            Self::EnterZone { zone, spawner } => {
                ReplayCommand::EnterZone(ZoneEntry { zone: ZoneId::from_raw(zone), spawner })
            }
            Self::ReturnTo(checkpoint) => ReplayCommand::ReturnTo(CheckpointId::from_raw(checkpoint)),
            Self::Dialogue { dialogue, effect } => ReplayCommand::Dialogue(DialogueEffect {
                dialogue: DialogueId::from_raw(dialogue),
                effect: EffectId(effect),
            }),
            Self::AutoTarget(strength) => ReplayCommand::AutoTarget(strength),
        }
    }
}

/// A document that can't be a script.
#[derive(Debug, thiserror::Error)]
pub enum InputsError {
    #[error(transparent)]
    Document(#[from] sapphire_serialize::DocumentError),
    #[error("`{field}`: {problem}")]
    Invalid { field: String, problem: String },
}

fn invalid(field: impl Into<String>, problem: impl Into<String>) -> InputsError {
    InputsError::Invalid { field: field.into(), problem: problem.into() }
}

impl InputsDocument {
    /// Reads a document's text, and checks it as [`Self::check`] does.
    pub fn from_ron(text: &str) -> Result<Self, InputsError> {
        let document: Self = sapphire_serialize::from_ron(text)?;
        document.check()?;
        Ok(document)
    }

    /// Checks everything that needs no other document, naming the field.
    pub fn check(&self) -> Result<(), InputsError> {
        if self.about.is_empty() {
            return Err(invalid("about", "say what it does"));
        }
        if self.ticks == 0 || self.ticks > MAX_TICKS {
            return Err(invalid("ticks", format!("1 to {MAX_TICKS}, a replay's most")));
        }
        for (index, track) in self.tracks.iter().enumerate() {
            track.check().map_err(|(field, problem)| invalid(format!("tracks[{index}].{field}"), problem))?;
        }
        let mut last = 0;
        for (index, &(tick, _)) in self.commands.iter().enumerate() {
            if tick < last {
                return Err(invalid(format!("commands[{index}]"), "commands go in the order of their ticks"));
            }
            if tick > self.ticks {
                return Err(invalid(format!("commands[{index}]"), "after the last tick"));
            }
            last = tick;
        }
        Ok(())
    }
}

impl Track {
    fn check(&self) -> Result<(), (String, String)> {
        let at = |field: &str, problem: &str| Err((field.to_owned(), problem.to_owned()));
        if let Who::Cast(place) = self.who
            && place as usize >= TickActions::CAPACITY
        {
            return at(
                "who",
                &format!("a tick holds input for the first {} of the cast", TickActions::CAPACITY),
            );
        }
        if self.presses.is_empty() && self.stick.is_none() {
            return at("presses", "a track with no presses and no stick does nothing");
        }
        for (index, schedule) in self.presses.iter().enumerate() {
            schedule.check().map_err(|problem| (format!("presses[{index}]"), problem))?;
        }
        match &self.stick {
            Some(StickPath::Legs(legs) | StickPath::Loop(legs)) => {
                if legs.is_empty() {
                    return at("stick", "a path of no legs");
                }
                for (index, leg) in legs.iter().enumerate() {
                    leg.check().map_err(|problem| (format!("stick[{index}]"), problem))?;
                }
            }
            Some(StickPath::Sweep { x, y }) if !(1..=256).contains(x) || !(1..=256).contains(y) => {
                return at("stick", "a sweep starts over every 1 to 256 ticks");
            }
            None | Some(StickPath::Sweep { .. }) => {}
        }
        Ok(())
    }

    /// Whether this track drives the cast's member at `place`, and if so, at what tick of its own.
    fn clock(&self, place: usize, tick: u64) -> Option<u64> {
        match self.who {
            Who::Cast(driven) => (driven as usize == place).then_some(tick),
            Who::Everyone { stagger } => Some(tick + place as u64 * u64::from(stagger)),
        }
    }

    /// Whether this track and `other` could both hold one member's stick.
    fn shares_a_stick_with(&self, other: &Self) -> bool {
        let overlap = match (self.who, other.who) {
            (Who::Cast(one), Who::Cast(two)) => one == two,
            _ => true,
        };
        overlap && self.stick.is_some() && other.stick.is_some()
    }

    fn buttons(&self, clock: u64) -> Buttons {
        let mut buttons = Buttons::NONE;
        for schedule in &self.presses {
            buttons = buttons | schedule.buttons(clock);
        }
        if let Some(StickPath::Legs(legs) | StickPath::Loop(legs)) = &self.stick
            && let Some((leg, into)) = leg_at(&self.stick, legs, clock)
            && leg.jump.map(u64::from) == Some(into)
        {
            buttons = buttons | Buttons::JUMP;
        }
        buttons
    }

    fn stick(&self, clock: u64) -> Stick {
        match &self.stick {
            None => Stick::CENTER,
            Some(StickPath::Legs(legs) | StickPath::Loop(legs)) => {
                leg_at(&self.stick, legs, clock).map_or(Stick::CENTER, |(leg, into)| leg.stick(into))
            }
            Some(StickPath::Sweep { x, y }) => {
                // A signed byte counting up: the `as` casts are the wrap.
                let axis = |period: &u32| (clock % u64::from(*period)) as u8 as i8;
                Stick::new(axis(x), axis(y))
            }
        }
    }
}

/// The leg of `legs` that `clock` falls in, and how far into it: none once `Legs` have run out.
fn leg_at<'a>(path: &Option<StickPath>, legs: &'a [Leg], clock: u64) -> Option<(&'a Leg, u64)> {
    let total: u64 = legs.iter().map(|leg| u64::from(leg.ticks)).sum();
    let mut left = match path {
        Some(StickPath::Loop(_)) => clock % total,
        _ => clock,
    };
    for leg in legs {
        if left < u64::from(leg.ticks) {
            return Some((leg, left));
        }
        left -= u64::from(leg.ticks);
    }
    None
}

impl Schedule {
    fn check(&self) -> Result<(), String> {
        if self.every == Some(0) {
            return Err("`every` 0 ticks".to_owned());
        }
        if self.hold == 0 {
            return Err("a press is held at least a tick".to_owned());
        }
        if self.at.is_empty() {
            return Err("a schedule of no presses".to_owned());
        }
        let mut after = None;
        for &(tick, ref buttons) in &self.at {
            if after.is_some_and(|after| tick <= after) {
                return Err(format!("{tick}: presses go up by tick, each tick once"));
            }
            after = Some(tick);
            if buttons.is_empty() {
                return Err(format!("{tick}: a press of no buttons"));
            }
            if let Some(every) = self.every
                && u64::from(tick) + u64::from(self.hold) > u64::from(every)
            {
                return Err(format!("{tick}: held past the schedule's start over at {every}"));
            }
        }
        Ok(())
    }

    fn buttons(&self, clock: u64) -> Buttons {
        let Some(since) = clock.checked_sub(u64::from(self.from)) else { return Buttons::NONE };
        let tick = self.every.map_or(since, |every| since % u64::from(every));
        let mut buttons = Buttons::NONE;
        for (at, pressed) in &self.at {
            if (u64::from(*at)..u64::from(*at) + u64::from(self.hold)).contains(&tick) {
                for button in pressed {
                    buttons = buttons | button.buttons();
                }
            }
        }
        buttons
    }
}

impl Leg {
    fn check(&self) -> Result<(), String> {
        if self.stick.0 == i8::MIN || self.stick.1 == i8::MIN {
            return Err("a stick reads −127 to 127".to_owned());
        }
        if self.ticks == 0 {
            return Err("a leg of no ticks".to_owned());
        }
        if self.ease > self.ticks {
            return Err("eased in over more ticks than it has".to_owned());
        }
        if self.jump.is_some_and(|jump| jump >= self.ticks) {
            return Err("a jump after the leg's end".to_owned());
        }
        Ok(())
    }

    fn stick(&self, into: u64) -> Stick {
        let (x, y) = self.stick;
        if into >= u64::from(self.ease) {
            return Stick::new(x, y);
        }
        // Whole percent of the way: `into` is under `ease`, so both fit an `i32`.
        let percent = (into as i32 + 1) * 100 / self.ease as i32;
        let eased = |axis: i8| (i32::from(axis) * percent / 100) as i8;
        Stick::new(eased(x), eased(y))
    }
}

/// A named source of a run's actions, from its document: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    /// Its name on the command line: its document's file's.
    pub name: String,
    /// A line on what it does, for listings.
    pub about: String,
    /// How many ticks to record when nothing says otherwise.
    pub ticks: u32,
    /// What the cast press and hold, `like`'s first.
    pub tracks: Vec<Track>,
    /// What it does between steps, each with the tick it comes before, in order.
    pub commands: Vec<(u32, ReplayCommand)>,
}

impl Script {
    /// The script `document` makes, under `name`, on `like`, the document its `like` names.
    pub fn new(
        name: &str,
        document: &InputsDocument,
        like: Option<&InputsDocument>,
    ) -> Result<Self, InputsError> {
        document.check()?;
        let mut tracks = Vec::new();
        let mut commands: Vec<(u32, ReplayCommand)> = Vec::new();
        match (&document.like, like) {
            (None, None) => {}
            (Some(named), Some(like)) => {
                if like.like.is_some() {
                    return Err(invalid("like", format!("`{named}` is like another itself")));
                }
                like.check().map_err(|error| invalid("like", format!("`{named}`: {error}")))?;
                tracks.extend(like.tracks.iter().cloned());
                commands.extend(like.commands.iter().map(|&(tick, command)| (tick, command.replay())));
            }
            (Some(named), None) => return Err(invalid("like", format!("there's no `{named}`"))),
            (None, Some(_)) => return Err(invalid("like", "it's like nothing")),
        }
        tracks.extend(document.tracks.iter().cloned());
        commands.extend(document.commands.iter().map(|&(tick, command)| (tick, command.replay())));
        // Stable: at one tick, `like`'s first.
        commands.sort_by_key(|&(tick, _)| tick);
        for (index, track) in tracks.iter().enumerate() {
            if tracks[..index].iter().any(|other| other.shares_a_stick_with(track)) {
                return Err(invalid("tracks", "two tracks hold one of the cast's stick"));
            }
        }
        Ok(Self {
            name: name.to_owned(),
            about: document.about.clone(),
            ticks: document.ticks,
            tracks,
            commands,
        })
    }

    /// The actions of `tick`, which counts from 0, for `cast`, the player first. Allocates nothing.
    #[must_use]
    pub fn actions(&self, tick: u64, cast: &[ActorId]) -> TickActions {
        let mut actions = TickActions::NEUTRAL;
        for (place, &actor) in cast.iter().enumerate() {
            let mut held = ActorActions::NEUTRAL;
            for track in &self.tracks {
                let Some(clock) = track.clock(place, tick) else { continue };
                held.buttons = held.buttons | track.buttons(clock);
                if track.stick.is_some() {
                    held.stick = track.stick(clock);
                }
            }
            actions.set(actor, held).expect("a tick holds the cast's input");
        }
        actions
    }

    /// What the cast's first, the player, does on `tick`: how a test finds when a script presses what.
    #[must_use]
    pub fn player(&self, tick: u64) -> ActorActions {
        let player = ActorId::from_raw(1).expect("not zero");
        self.actions(tick, &[player]).get(player)
    }

    /// The first tick, within its length, the player presses all of `buttons` on.
    #[must_use]
    pub fn first_press(&self, buttons: Buttons) -> Option<u64> {
        (0..u64::from(self.ticks)).find(|&tick| self.player(tick).buttons.contains(buttons))
    }

    /// The tick `command` first comes before.
    #[must_use]
    pub fn command_at(&self, command: ReplayCommand) -> Option<u32> {
        self.commands.iter().find(|&&(_, at)| at == command).map(|&(tick, _)| tick)
    }
}

/// Every document `build.rs` compiled in, as scripts, by name.
pub static SCRIPTS: LazyLock<Vec<Script>> = LazyLock::new(|| {
    let read = |name: &str, text: &str| {
        InputsDocument::from_ron(text)
            .unwrap_or_else(|error| panic!("content/inputs/{name}.inputs.ron: {error}"))
    };
    let documents: Vec<(&str, InputsDocument)> =
        INPUTS_DOCUMENTS.iter().map(|&(name, text)| (name, read(name, text))).collect();
    documents
        .iter()
        .map(|(name, document)| {
            let like = document.like.as_deref().and_then(|like| {
                documents.iter().find(|(other, _)| *other == like).map(|(_, document)| document)
            });
            Script::new(name, document, like)
                .unwrap_or_else(|error| panic!("content/inputs/{name}.inputs.ron: {error}"))
        })
        .collect()
});

/// The script named `name`.
#[must_use]
pub fn find_script(name: &str) -> Option<&'static Script> {
    SCRIPTS.iter().find(|script| script.name == name)
}

#[cfg(test)]
mod tests {
    use super::super::is_valid_name;
    use super::*;

    fn document(data: &str) -> Result<InputsDocument, InputsError> {
        InputsDocument::from_ron(&format!(
            "(type: \"sapphire::gameplay::InputsDocument\", version: 1, data: ({data}))"
        ))
    }

    fn script(data: &str) -> Script {
        Script::new("test", &document(data).expect("reads"), None).expect("a script")
    }

    fn cast(size: u32) -> Vec<ActorId> {
        (1..=size).map(|raw| ActorId::from_raw(raw).expect("a real ID")).collect()
    }

    #[test]
    fn every_script_has_a_good_unique_name_and_a_length() {
        assert_eq!(SCRIPTS.len(), INPUTS_DOCUMENTS.len());
        for script in SCRIPTS.iter() {
            assert!(is_valid_name(&script.name), "{}", script.name);
            assert!(!script.about.is_empty(), "{}", script.name);
            assert!(script.ticks > 0, "{}", script.name);
            assert!(std::ptr::eq(find_script(&script.name).expect("found"), script), "{} twice", script.name);
        }
        assert!(find_script("no-such-script").is_none());
    }

    #[test]
    fn a_schedule_presses_from_its_start_every_so_often_for_its_hold() {
        let script = script(
            "about: \"a\", ticks: 100, tracks: [(presses: [(from: 10, every: 20, hold: 2, at: [(3, [Light, Jump])])])]",
        );
        let pressed: Vec<u64> = (0..60).filter(|&tick| !script.player(tick).buttons.is_empty()).collect();
        assert_eq!(pressed, [13, 14, 33, 34, 53, 54]);
        assert_eq!(script.player(13).buttons, Buttons::LIGHT | Buttons::JUMP);
        assert_eq!(script.player(13).stick, Stick::CENTER, "a track with no stick leaves it at rest");
    }

    #[test]
    fn legs_ease_in_jump_and_end_at_rest_and_a_loop_goes_round() {
        let legs = "[(stick: (127, -100), ticks: 4, ease: 4, jump: 1), (stick: (0, 64), ticks: 2)]";
        let once = script(&format!("about: \"a\", ticks: 10, tracks: [(stick: Legs({legs}))]"));
        let sticks: Vec<Stick> = (0..7).map(|tick| once.player(tick).stick).collect();
        let expected = [(31, -25), (63, -50), (95, -75), (127, -100), (0, 64), (0, 64), (0, 0)];
        assert_eq!(
            sticks,
            expected.map(|(x, y)| Stick::new(x, y)),
            "a quarter of the way each tick, toward 0"
        );
        let jumped: Vec<u64> = (0..7).filter(|&tick| once.player(tick).buttons == Buttons::JUMP).collect();
        assert_eq!(jumped, [1]);

        let round = script(&format!("about: \"a\", ticks: 10, tracks: [(stick: Loop({legs}))]"));
        for tick in 0..6 {
            assert_eq!(round.player(tick + 6), once.player(tick), "tick {tick}");
        }
    }

    #[test]
    fn a_sweep_counts_up_as_a_signed_byte() {
        let script = script("about: \"a\", ticks: 10, tracks: [(stick: Sweep(x: 255, y: 101))]");
        let at = |tick| script.player(tick).stick;
        assert_eq!(at(0), Stick::new(0, 0));
        assert_eq!(at(127), Stick::new(127, 26));
        assert_eq!(at(128), Stick::new(-127, 27), "−128 reads as −127");
        assert_eq!(at(254), Stick::new(-2, 52));
        assert_eq!(at(255), Stick::new(0, 53), "and over again");
    }

    #[test]
    fn everyone_is_staggered_and_a_place_drives_one() {
        let script = script(
            "about: \"a\", ticks: 10, tracks: [
                (who: Everyone(stagger: 5), presses: [(every: 10, at: [(0, [Light])])]),
                (who: Cast(1), presses: [(at: [(0, [Heavy])])]),
            ]",
        );
        let cast = cast(3);
        let at = |tick| {
            cast.iter().map(|&actor| script.actions(tick, &cast).get(actor).buttons).collect::<Vec<_>>()
        };
        assert_eq!(at(0), [Buttons::LIGHT, Buttons::HEAVY, Buttons::LIGHT]);
        assert_eq!(at(5), [Buttons::NONE, Buttons::LIGHT, Buttons::NONE]);
        assert_eq!(script.actions(0, &cast[..1]).len(), 1, "a place nobody stands in is nobody's");
    }

    #[test]
    fn like_comes_first_and_brings_its_commands() {
        let base = document(
            "about: \"base\", ticks: 50, tracks: [(presses: [(at: [(1, [Light])])])], commands: [(10, PlayTimeline(0))]",
        )
        .expect("reads");
        let top = document(
            "about: \"top\", ticks: 80, like: \"base\", tracks: [(presses: [(at: [(1, [Heavy])])])],
             commands: [(5, SkipTimeline(at_ms: 9)), (10, AutoTarget(Weak))]",
        )
        .expect("reads");
        let script = Script::new("top", &top, Some(&base)).expect("a script");
        assert_eq!((script.about.as_str(), script.ticks), ("top", 80));
        assert_eq!(
            script.commands,
            [
                (5, ReplayCommand::SkipTimeline { at_ms: 9 }),
                (10, ReplayCommand::PlayTimeline(TimelineId::from_raw(0))),
                (10, ReplayCommand::AutoTarget(AutoTarget::Weak)),
            ]
        );
        assert_eq!(script.player(1).buttons, Buttons::LIGHT | Buttons::HEAVY);

        let error = |result: Result<Script, InputsError>| result.expect_err("refused").to_string();
        assert!(error(Script::new("top", &top, None)).contains("there's no `base`"));
        assert!(error(Script::new("base", &base, Some(&top))).contains("like nothing"));
        let deeper = Script::new(
            "deeper",
            &document("about: \"a\", ticks: 1, like: \"top\"").expect("reads"),
            Some(&top),
        );
        assert!(error(deeper).contains("like another itself"));
    }

    /// What can't be read as a script is refused with the field it's in.
    #[test]
    fn what_cant_be_a_script_is_refused_naming_its_field() {
        let refused = [
            ("about: \"a\", ticks: 1, speed: 2", "speed"),
            ("about: \"\", ticks: 1", "`about`"),
            ("about: \"a\", ticks: 0", "`ticks`"),
            ("about: \"a\", ticks: 1, tracks: [(presses: [(every: 0, at: [(0, [Light])])])]", "presses[0]"),
            (
                "about: \"a\", ticks: 1, tracks: [(presses: [(at: [(4, [Light]), (4, [Heavy])])])]",
                "each tick once",
            ),
            (
                "about: \"a\", ticks: 1, tracks: [(presses: [(every: 4, hold: 2, at: [(3, [Light])])])]",
                "held past",
            ),
            ("about: \"a\", ticks: 1, tracks: [(presses: [(at: [(0, [])])])]", "no buttons"),
            ("about: \"a\", ticks: 1, tracks: [(presses: [(at: [])])]", "no presses"),
            ("about: \"a\", ticks: 1, tracks: [(presses: [(at: [(0, [Touch])])])]", "Touch"),
            ("about: \"a\", ticks: 1, tracks: [(stick: Legs([(stick: (-128, 0), ticks: 1)]))]", "stick[0]"),
            (
                "about: \"a\", ticks: 1, tracks: [(stick: Legs([(stick: (1, 0), ticks: 2, ease: 3)]))]",
                "eased",
            ),
            ("about: \"a\", ticks: 1, tracks: [(stick: Legs([(stick: (1, 0), ticks: 2, jump: 2)]))]", "jump"),
            ("about: \"a\", ticks: 1, tracks: [(stick: Loop([]))]", "no legs"),
            ("about: \"a\", ticks: 1, tracks: [(stick: Sweep(x: 0, y: 1))]", "sweep"),
            ("about: \"a\", ticks: 1, tracks: [(who: Cast(8), stick: Sweep(x: 1, y: 1))]", "`tracks[0].who`"),
            ("about: \"a\", ticks: 1, tracks: [()]", "does nothing"),
            ("about: \"a\", ticks: 9, commands: [(5, PlayTimeline(0)), (4, PlayTimeline(1))]", "commands[1]"),
            ("about: \"a\", ticks: 9, commands: [(10, PlayTimeline(0))]", "after the last tick"),
        ];
        for (data, field) in refused {
            let error = document(data).expect_err(data).to_string();
            assert!(error.contains(field), "{data}: {error}");
        }

        let two_sticks = document(
            "about: \"a\", ticks: 1, tracks: [(who: Everyone(stagger: 0), stick: Sweep(x: 1, y: 1)),
             (who: Cast(3), stick: Legs([(stick: (1, 0), ticks: 1)]))]",
        )
        .expect("reads");
        let error = Script::new("two", &two_sticks, None).expect_err("refused").to_string();
        assert!(error.contains("one of the cast's stick"), "{error}");
        let apart = document(
            "about: \"a\", ticks: 1, tracks: [(who: Cast(0), stick: Sweep(x: 1, y: 1)),
             (who: Cast(1), stick: Sweep(x: 1, y: 1)), (who: Cast(0), presses: [(at: [(0, [Light])])])]",
        )
        .expect("reads");
        assert!(Script::new("apart", &apart, None).is_ok(), "one stick each, and presses beside one");
    }
}
