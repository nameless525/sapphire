//! Walks through zones (M3 B4): the player by the stick and the jump button alone, over a zone's real
//! ground, and back to its checkpoints. Exit test 1's replays are of these.
//!
//! A walk is legs, each a stick held for so many ticks with perhaps a jump in it, in the world's axes (x
//! along +X, y along +Z): 127 is a run, 6 m/s by the default profile, and 64 a walk, 2 m/s. The legs were
//! fitted to the ground by running them, so a change to the controller or a collider can leave a leg
//! pushing into something it used to clear; the replays' checksums say so, and `tests/walks.rs` says where
//! each walk has to get to.

use super::file::ReplayCommand;
use crate::actions::{ActorActions, Buttons, Stick, TickActions};
use crate::actors::ActorId;
use crate::checkpoint::CheckpointId;

/// A stick held for `ticks`, pressing jump `jump` ticks in, if it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leg {
    pub stick: (i8, i8),
    pub ticks: u32,
    pub jump: Option<u32>,
}

const fn leg(x: i8, y: i8, ticks: u32) -> Leg {
    Leg { stick: (x, y), ticks, jump: None }
}

const fn jump(x: i8, y: i8, ticks: u32, at: u32) -> Leg {
    Leg { stick: (x, y), ticks, jump: Some(at) }
}

/// How many ticks `legs` take.
#[must_use]
pub const fn length(legs: &[Leg]) -> u32 {
    let mut ticks = 0;
    let mut index = 0;
    while index < legs.len() {
        ticks += legs[index].ticks;
        index += 1;
    }
    ticks
}

/// The player's actions at `tick` of `legs`, and nothing once they're done.
#[must_use]
pub fn walk(legs: &[Leg], tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    let Some(&player) = cast.first() else { return actions };
    let mut left = tick;
    for leg in legs {
        if left < u64::from(leg.ticks) {
            let buttons = if leg.jump.map(u64::from) == Some(left) { Buttons::JUMP } else { Buttons::NONE };
            let stick = Stick::new(leg.stick.0, leg.stick.1);
            actions.set(player, ActorActions { buttons, stick }).expect("one actor fits a tick");
            break;
        }
        left -= u64::from(leg.ticks);
    }
    actions
}

/// The proving ground's `platform` checkpoint, by its place in the zone's list, which goes by its name's
/// hash: `tests/walks.rs` holds it there.
pub const PLATFORM: CheckpointId = CheckpointId::from_raw(0);

/// The proving ground from its start (1, 1) to the far end of its walking course and back to the
/// platform, by `walk-the-ground`:
///
/// 1. A run toward −Z, out to the course's lane at z −20.
/// 2. A run toward −X along it: up the 20° ramp, over the platform, down its four steps, and a jump from
///    their foot that comes down at the 15 cm curb.
/// 3. On, and a jump over the 60 cm wall.
/// 4. A jump into the 60° face, too steep to stand on, which it slides back down.
/// 5. A run toward +Z along the face's foot, and past its end.
/// 6. Standing.
///
/// Then [`PLATFORM`]'s return, and [`GROUND_ONWARD_LEGS`].
pub const GROUND_OUT_LEGS: [Leg; 6] = [
    leg(0, -127, 208),
    jump(-127, 0, 230, 199),
    jump(-127, 0, 70, 33),
    jump(-127, 0, 60, 5),
    leg(-127, 60, 60),
    leg(0, 0, 30),
];

/// From the platform, facing −X, as `walk-the-ground` goes on after its return and `from-the-platform`
/// starts:
///
/// 1. A run down the steps, toward −X.
/// 2. A run back up them, toward +X, which takes each as a step up.
/// 3. A walk toward +Z, off the platform's side, a drop of a metre.
/// 4. A run toward +X, back level with the start.
/// 5. A run toward +Z, back to the start.
/// 6. Standing.
pub const GROUND_ONWARD_LEGS: [Leg; 6] =
    [leg(-127, 0, 50), leg(127, 0, 60), leg(0, 64, 90), leg(127, 0, 133), leg(0, 127, 175), leg(0, 0, 60)];

/// When `walk-the-ground` returns to the platform: after [`GROUND_OUT_LEGS`].
pub const GROUND_RETURN_AT: u32 = length(&GROUND_OUT_LEGS);

/// `walk-the-ground`'s command: the return to the platform.
pub const GROUND_RETURN: [(u32, ReplayCommand); 1] = [(GROUND_RETURN_AT, ReplayCommand::ReturnTo(PLATFORM))];

/// `walk-the-ground`'s length.
pub const GROUND_WALK_TICKS: u32 = GROUND_RETURN_AT + length(&GROUND_ONWARD_LEGS);

/// [`super::SCRIPTS`]' `walk-the-ground`.
#[must_use]
pub fn walk_the_ground(tick: u64, cast: &[ActorId]) -> TickActions {
    match tick.checked_sub(u64::from(GROUND_RETURN_AT)) {
        None => walk(&GROUND_OUT_LEGS, tick, cast),
        Some(onward) => from_the_platform(onward, cast),
    }
}

/// [`super::SCRIPTS`]' `from-the-platform`: [`GROUND_ONWARD_LEGS`] alone.
#[must_use]
pub fn from_the_platform(tick: u64, cast: &[ActorId]) -> TickActions {
    walk(&GROUND_ONWARD_LEGS, tick, cast)
}

/// The arena's square from its `south_paving` checkpoint (0, 12), its encounter cleared, as a save of
/// someone who has fought it starts, by `walk-the-square`:
///
/// 1. A run toward −Z, a jump over the bench at (0.4, −9.5), and on into the gate wall.
/// 2. A run toward +X along the wall, into the corner the shop's front makes with it.
/// 3. A run toward +Z, a jump over the planter at (10.2, −14.6), and on into the lantern post
///    at (8, −2.5).
/// 4. A run toward −X, past the pavilion.
/// 5. A run toward +Z, past the fruit stall and the front planter.
/// 6. A run toward +X, back to the checkpoint, and standing.
pub const SQUARE_LEGS: [Leg; 7] = [
    jump(0, -127, 300, 200),
    leg(127, 0, 90),
    jump(0, 127, 150, 6),
    leg(-127, 0, 140),
    leg(0, 127, 150),
    leg(127, 0, 100),
    leg(0, 0, 60),
];

/// [`super::SCRIPTS`]' `walk-the-square`.
#[must_use]
pub fn walk_the_square(tick: u64, cast: &[ActorId]) -> TickActions {
    walk(&SQUARE_LEGS, tick, cast)
}

/// The Lantern Ward's `court` checkpoint, by its place in the zone's list: `tests/walks.rs` holds it there.
pub const WARD_COURT: CheckpointId = CheckpointId::from_raw(0);

/// The Lantern Ward end to end from `player_start` (0, 14), along its `tour` spline, by `walk-the-ward`: a
/// leg a point of it, each aimed from where the last one ended, so none presses into a wall.
///
/// 1. Standing, then toward −Z and east to the Market Court, through its `market` checkpoint, to the
///    court's far end and back.
/// 2. West to the Blossom Garden, through `garden`, to its far end and back.
/// 3. South down the temple's lane, through the court's gate and its `court` checkpoint, the length of
///    the court and up the terrace's steps to where a boss would stand.
///
/// Then [`WARD_COURT`]'s return, and [`WARD_ONWARD_LEGS`].
pub const WARD_OUT_LEGS: [Leg; 25] = [
    leg(0, 0, 30),
    leg(0, -127, 81),
    leg(118, -48, 91),
    leg(124, -27, 119),
    leg(127, 0, 260),
    leg(127, 0, 180),
    leg(5, 127, 87),
    leg(86, -94, 125),
    leg(-127, 0, 278),
    leg(-127, 2, 255),
    leg(-123, 30, 124),
    leg(-127, 0, 160),
    leg(-123, -30, 123),
    leg(-127, 0, 260),
    leg(-127, 0, 180),
    leg(-4, 127, 87),
    leg(116, -52, 213),
    leg(127, -2, 253),
    leg(114, -55, 134),
    leg(71, -105, 145),
    leg(0, -127, 420),
    leg(0, -127, 365),
    leg(-3, -127, 43),
    leg(-1, -127, 45),
    leg(0, 0, 60),
];

/// From the court's checkpoint, facing up the court: a run of two seconds toward −Z, and standing.
pub const WARD_ONWARD_LEGS: [Leg; 2] = [leg(0, -127, 120), leg(0, 0, 60)];

/// When `walk-the-ward` returns to the court: after [`WARD_OUT_LEGS`].
pub const WARD_RETURN_AT: u32 = length(&WARD_OUT_LEGS);

/// `walk-the-ward`'s command: the return to the court.
pub const WARD_RETURN: [(u32, ReplayCommand); 1] = [(WARD_RETURN_AT, ReplayCommand::ReturnTo(WARD_COURT))];

/// `walk-the-ward`'s length.
pub const WARD_WALK_TICKS: u32 = WARD_RETURN_AT + length(&WARD_ONWARD_LEGS);

/// [`super::SCRIPTS`]' `walk-the-ward`.
#[must_use]
pub fn walk_the_ward(tick: u64, cast: &[ActorId]) -> TickActions {
    match tick.checked_sub(u64::from(WARD_RETURN_AT)) {
        None => walk(&WARD_OUT_LEGS, tick, cast),
        Some(onward) => walk(&WARD_ONWARD_LEGS, onward, cast),
    }
}
