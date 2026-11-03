//! A fight's sword trails: what `sapphire_vfx`'s trails need of a fight, read from the simulation and
//! the drawn blades, for [`CueShow`](crate::cues::CueShow) to run in the [`VfxWorld`] it owns.
//!
//! A row of the cue table can't start one, on purpose: a row keeps no handle to stop it with
//! (`Effect::is_endless`). So what a blade is doing decides, here, read after each step and changing
//! nothing in the simulation.
//!
//! - **When** ([`Cuts`]): each hit on the `blade` path has a cut, the run of ticks around that hit's
//!   fastest in which the path's middle moves at [`CUTTING_SPEED`] or better, and [`FOLLOW_TICKS`] more.
//!   A hit's window alone starts late on one move and early on another (`light_3`'s blade is flying two
//!   ticks before its window opens, `light_1`'s moves only from the second tick of it) and closes before
//!   the blade stops. A phase wouldn't do: a flinch or a dodge is `Active` too. The tables say so ahead of
//!   time, so a cut begins on the tick it should however the frames fall, and a cooked pose moves it with
//!   no code.
//! - **Where** ([`Rails`]): a blade's base and tip on its own mesh's axis, through the transform that
//!   draws it this frame, so the strip's head is the sword as drawn at any `alpha`, under IK and through a
//!   blend, where the baked table alone misses the drawn tip by up to 23 cm.
//! - **How often:** a sample with each frame's pose, as `VfxWorld::write_frame` reads back to it, and for
//!   the player one for each step a frame passed over, from the move's table. A strike's tip covers 1.5 m
//!   a tick, so at 30 Hz a frame's poses alone cut the corners by up to 82 cm and lose 18% of the arc.
//!   The table has the arc's shape, and [`Offset`] carries it onto the pose, by how far the table misses
//!   at the frames either side: 11 cm and 7% at most. The warrior's tables are hand-keyed apart from its
//!   clips, by up to 3 m, so its trail is its frames' alone, which its slower arms can bear: 21 cm.
//! - **Its clock:** an effect's age is its owner's, so a hitstop holds a trail exactly, and a frame's
//!   sample goes in `alpha` of the way between the last two steps, as the pose does.
//! - **How it ends:** any new playing, a new actor on the field or no move at all stops the running trail
//!   where it got to, so a cancel never stretches a strip from one move to the next: the next cut is a
//!   new effect with a new strip. What's already drawn fades where it was.
//!
//! No cut, no cost beyond a compare a step for each fighter: nothing is built for a fight with no trail
//! or a scene with no fight, and nothing here allocates. Tuning is in `default_effects.ron`, a save away
//! under `--cues`, and [`CUTTING_SPEED`] and [`FOLLOW_TICKS`] here.

use sapphire_core::Name;
use sapphire_cues::owner_of;
use sapphire_gameplay::{ActorId, ActorView, MoveLibrary};
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_moves::{Move, PathSample};
use sapphire_time::TICK_HZ;
use sapphire_vfx::{EffectHandle, EffectId, EffectLibrary, Placement, VfxWorld};

use sapphire_stage::anim_graph::duration_ticks;

/// The path a blade's hits ride.
const BLADE: Name = Name::new("blade");

/// How far a path's middle moves in a tick, in metres, to count as cutting: 21 m/s. The player's strikes
/// run at 39 to 112 cm a tick, the warrior's at 38 to 47, and the wind-ups beside them mostly under 30 (the
/// dash attack's throw touches 52, apart from its strike by a slower tick).
const CUTTING_SPEED: f32 = 0.35;
/// Ticks a trail runs on past a cut, as the blade slows to its mark.
const FOLLOW_TICKS: u32 = 1;
/// The most cuts a move has that trail apart. Past it they join the last.
const MAX_CUTS: usize = 4;
/// Steps a trail remembers of its table: more than a frame takes but for a hitch, and a whole cut, for a
/// capture that takes all its steps before its first frame.
const MAX_READINGS: usize = 12;

/// The player's sword, and what its trail plays.
const PLAYER_TRAIL: &str = "blade_trail";
/// The warrior's blade, and what its trail plays.
const WARRIOR_TRAIL: &str = "warrior_trail";

/// A blade's base and tip in its own mesh's space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rails {
    base: Vec3,
    tip: Vec3,
}

impl Rails {
    /// `sapphire_pipeline::combat::sword`'s `BLADE_ROOT` and `HALF_LENGTH`: where the guard ends, and the
    /// point.
    const PLAYER: Self = Self { base: Vec3::new(0.0, -0.364, 0.0), tip: Vec3::new(0.0, 0.45, 0.0) };
    /// `Skeleton_Blade.gltf`'s, on its long axis: above the guard, and the mesh's highest point.
    const WARRIOR: Self = Self { base: Vec3::new(0.0, 0.4, 0.0), tip: Vec3::new(0.0, 1.168, 0.0) };

    /// Where `place`, which draws the blade, has them.
    fn drawn(self, place: Mat4) -> Sockets {
        Sockets { base: place.transform_point3(self.base), tip: place.transform_point3(self.tip) }
    }

    /// Where a table's reading of its path has them, with the root at `view`. The path's transform is the
    /// mesh's in the root's space, by which a hit sweeps what's drawn (`blade::socket`).
    fn in_table(self, sample: PathSample, view: &ActorView) -> Sockets {
        let turn = Quat::from_rotation_y(view.yaw);
        let rotation = Quat::from_array(sample.rotation);
        let middle = Vec3::from_array(sample.translation);
        let world = |along: Vec3| view.position + turn * (middle + rotation * along);
        Sockets { base: world(self.base), tip: world(self.tip) }
    }
}

/// Where `place`, which draws the player's sword, has its base and tip.
#[cfg(test)]
pub(crate) fn player_rails(place: Mat4) -> (Vec3, Vec3) {
    let sockets = Rails::PLAYER.drawn(place);
    (sockets.base, sockets.tip)
}

/// Where `place`, which draws the warrior's blade, has its base and tip.
#[cfg(test)]
pub(crate) fn warrior_rails(place: Mat4) -> (Vec3, Vec3) {
    let sockets = Rails::WARRIOR.drawn(place);
    (sockets.base, sockets.tip)
}

/// A blade's two sockets, in the world.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Sockets {
    base: Vec3,
    tip: Vec3,
}

/// How far a table's reading falls from the drawn sword at a moment: IK's and a blend's share of a pose.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Offset {
    base: Vec3,
    tip: Vec3,
}

impl Offset {
    fn of(drawn: Sockets, table: Sockets) -> Self {
        Self { base: drawn.base - table.base, tip: drawn.tip - table.tip }
    }

    fn lerp(self, other: Self, share: f32) -> Self {
        Self { base: self.base.lerp(other.base, share), tip: self.tip.lerp(other.tip, share) }
    }

    fn on(self, table: Sockets) -> Sockets {
        Sockets { base: table.base + self.base, tip: table.tip + self.tip }
    }
}

/// A stretch of a move in which its blade cuts, in ticks, its last read as well.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cut {
    pub(crate) from: u32,
    pub(crate) to: u32,
}

/// The cuts of a move's playing. See the [module docs](self).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cuts {
    cuts: [Cut; MAX_CUTS],
    len: usize,
}

impl Cuts {
    /// The cuts of `moved`, one for each hit on its `blade` path, joined where they meet. A hit whose path
    /// never reaches [`CUTTING_SPEED`] is a slow cut, and its window says when. Anything with no hit on
    /// that path has none: a dodge or a flinch.
    pub(crate) fn of(moved: &Move) -> Self {
        let mut cuts = Self::default();
        let Some(path) = moved.tables.path(BLADE) else { return cuts };
        let samples = path.samples();
        let speed = |tick: usize| {
            let at = |tick: usize| Vec3::from_array(samples[tick].translation);
            at(tick).distance(at(tick + 1))
        };
        for hit in moved.hitboxes.iter().filter(|hit| hit.path() == BLADE) {
            let last = samples.len().saturating_sub(1);
            let span = hit.span.from as usize..(hit.span.to as usize).min(last);
            let fastest = span.clone().max_by(|&a, &b| speed(a).total_cmp(&speed(b)));
            let cut = match fastest.filter(|&tick| speed(tick) >= CUTTING_SPEED) {
                Some(tick) => {
                    let mut from = tick;
                    while from > 0 && speed(from - 1) >= CUTTING_SPEED {
                        from -= 1;
                    }
                    let mut to = tick + 1;
                    while to < last && speed(to) >= CUTTING_SPEED {
                        to += 1;
                    }
                    Cut { from: from as u32, to: (to as u32 + FOLLOW_TICKS).min(last as u32) }
                }
                None => Cut { from: hit.span.from, to: hit.span.to },
            };
            cuts.add(cut);
        }
        cuts
    }

    /// Adds a cut in its place by when it starts, which isn't always when its hit does: a long hit's
    /// fastest run can come after a shorter hit's that opens later.
    fn add(&mut self, cut: Cut) {
        let count = self.len;
        let place = self.as_slice().partition_point(|other| other.from <= cut.from);
        let mut all = [Cut::default(); MAX_CUTS + 1];
        all[..place].copy_from_slice(&self.cuts[..place]);
        all[place] = cut;
        all[place + 1..=count].copy_from_slice(&self.cuts[place..count]);
        self.len = 0;
        for next in all.into_iter().take(count + 1) {
            if self.len > 0 && (self.len == MAX_CUTS || next.from <= self.cuts[self.len - 1].to) {
                let last = &mut self.cuts[self.len - 1];
                last.to = last.to.max(next.to);
            } else {
                self.cuts[self.len] = next;
                self.len += 1;
            }
        }
    }

    /// The list, in the move's order.
    pub(crate) fn as_slice(&self) -> &[Cut] {
        &self.cuts[..self.len]
    }

    /// Which is open at `ticks` into the move.
    fn open_at(&self, ticks: f64) -> Option<usize> {
        self.as_slice().iter().position(|cut| f64::from(cut.from) <= ticks && ticks <= f64::from(cut.to))
    }
}

/// A step's reading of the table.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Reading {
    /// Ticks into the move.
    position: f64,
    sockets: Sockets,
}

/// The last steps' readings, oldest first.
#[derive(Clone, Copy, Debug, Default)]
struct Readings {
    list: [Reading; MAX_READINGS],
    len: usize,
}

impl Readings {
    fn push(&mut self, reading: Reading) {
        if self.len == MAX_READINGS {
            self.list.copy_within(1.., 0);
            self.len -= 1;
        }
        self.list[self.len] = reading;
        self.len += 1;
    }

    fn as_slice(&self) -> &[Reading] {
        &self.list[..self.len]
    }

    /// The table at `position`, on a quadratic through the readings about it: a frame between two of a
    /// strike's steps has its pose on the arc the joints bend through, not on the chord. `None` outside them.
    fn at(&self, position: f64) -> Option<Sockets> {
        let list = self.as_slice();
        let after = list.iter().position(|reading| reading.position >= position)?;
        if list[after].position == position {
            return Some(list[after].sockets);
        }
        if after == 0 {
            return None;
        }
        if list.len() < 3 {
            let (a, b) = (list[after - 1], list[after]);
            let share = ((position - a.position) / (b.position - a.position)) as f32;
            return Some(Sockets {
                base: a.sockets.base.lerp(b.sockets.base, share),
                tip: a.sockets.tip.lerp(b.sockets.tip, share),
            });
        }
        let first = after.saturating_sub(2).min(list.len() - 3);
        let [a, b, c] = [list[first], list[first + 1], list[first + 2]];
        let weight = |p: f64, q: f64, r: f64| ((position - q) * (position - r) / ((p - q) * (p - r))) as f32;
        let (wa, wb, wc) = (
            weight(a.position, b.position, c.position),
            weight(b.position, a.position, c.position),
            weight(c.position, a.position, b.position),
        );
        Some(Sockets {
            base: a.sockets.base * wa + b.sockets.base * wb + c.sockets.base * wc,
            tip: a.sockets.tip * wa + b.sockets.tip * wb + c.sockets.tip * wc,
        })
    }
}

/// What the latest step said of a fighter's move.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct LatestStep {
    /// Ticks into its move.
    elapsed: f64,
    /// Ticks its clock moved.
    delta: f64,
    /// Seconds its clock moved, as its effects' clock did.
    delta_seconds: f32,
}

/// A trail playing, and how far the frames have taken it.
#[derive(Clone, Copy, Debug)]
struct Running {
    handle: EffectHandle,
    /// The cut it's for, among its move's.
    cut: usize,
    /// The cut's over: it stops once the frame has drawn up to there, or at the next step if none comes
    /// first.
    closing: bool,
    /// How far into the move the last frame drew, and how far its table fell from its pose then.
    drawn: Option<(f64, Option<Offset>)>,
    readings: Readings,
}

/// One fighter's blade.
#[derive(Clone, Copy, Debug)]
struct FighterTrail {
    effect: Option<EffectId>,
    rails: Rails,
    /// Whether its table has the shape of what's drawn, to fill the steps between frames with.
    table_is_its_pose: bool,
    /// Who plays what.
    playing: Option<(ActorId, u32)>,
    cuts: Cuts,
    latest: LatestStep,
    running: Option<Running>,
}

impl FighterTrail {
    fn new(effect: Option<EffectId>, rails: Rails, table_is_its_pose: bool) -> Self {
        Self {
            effect,
            rails,
            table_is_its_pose,
            playing: None,
            cuts: Cuts::default(),
            latest: LatestStep::default(),
            running: None,
        }
    }

    /// Takes what a step said of it, `None` for a fighter with no blade to draw a trail from.
    fn read_step(&mut self, view: Option<&ActorView>, library: &MoveLibrary, world: &mut VfxWorld) {
        let current = view.and_then(|view| view.moves.current.map(|current| (view, current)));
        let playing = current.map(|(view, current)| (view.id, current.serial));
        if playing != self.playing {
            self.end(world);
            self.playing = playing;
            let moved = current.and_then(|(_, current)| library.get(current.at));
            self.cuts = moved.map_or_else(Cuts::default, Cuts::of);
        }
        let Some((view, current)) = current else { return };
        let elapsed = duration_ticks(current.elapsed);
        let open = self.cuts.open_at(elapsed);
        let another =
            open.is_some() && self.running.as_ref().is_some_and(|running| Some(running.cut) != open);
        // A cut that closed on an earlier step has had no frame since: what follows is its recovery.
        let closed = open.is_none() && self.running.as_ref().is_some_and(|running| running.closing);
        if another || closed {
            self.end(world);
        }
        self.latest = LatestStep {
            elapsed,
            delta: duration_ticks(view.clock_delta),
            delta_seconds: view.clock_delta.as_secs_f32(),
        };
        if let Some(running) = &mut self.running {
            running.closing = open.is_none();
        }
        if let Some(cut) = open
            && self.running.is_none()
        {
            let spawned = self
                .effect
                .and_then(|effect| world.spawn_trail(effect, owner_of(view.id), Placement::default()));
            self.running = spawned.map(|handle| Running {
                handle,
                cut,
                closing: false,
                drawn: None,
                readings: Readings::default(),
            });
        }
        // A frozen step has nothing new to read.
        let Some(running) =
            self.running.as_mut().filter(|_| self.table_is_its_pose && self.latest.delta > 0.0)
        else {
            return;
        };
        let path = library.get(current.at).and_then(|moved| moved.tables.path(BLADE));
        if let Some(path) = path {
            let sockets = self.rails.in_table(path.at(current.elapsed), view);
            running.readings.push(Reading { position: elapsed, sockets });
        }
    }

    /// Takes the frame's drawn blade, `unplayed` of its last step behind the latest.
    fn follow_frame(&mut self, place: Option<Mat4>, unplayed: f32, world: &mut VfxWorld) {
        let Some(running) = &mut self.running else { return };
        let Some(place) = place else { return };
        let drawn = self.rails.drawn(place);
        let LatestStep { elapsed, delta, delta_seconds } = self.latest;
        let position = elapsed - f64::from(unplayed) * delta;
        let offset = running.readings.at(position).map(|table| Offset::of(drawn, table));
        let (last_position, last_offset) =
            running.drawn.map_or((None, None), |(at, offset)| (Some(at), offset));
        for reading in running.readings.as_slice() {
            let after_last = last_position.is_none_or(|last| reading.position > last);
            if !after_last || reading.position >= position {
                continue;
            }
            let share = last_position.map_or(1.0, |last| (reading.position - last) / (position - last));
            let moved = match (last_offset, offset) {
                (Some(last), Some(now)) => last.lerp(now, share as f32),
                (one, other) => one.or(other).unwrap_or_default(),
            };
            let sockets = moved.on(reading.sockets);
            let ago = (elapsed - reading.position) as f32 / TICK_HZ as f32;
            world.sample_trail(running.handle, sockets.base, sockets.tip, ago);
        }
        // Just what `VfxWorld::write_frame` reads back by, so the strip ends on this sample.
        world.sample_trail(running.handle, drawn.base, drawn.tip, unplayed * delta_seconds);
        running.drawn = Some((last_position.map_or(position, |last| last.max(position)), offset));
        if running.closing {
            world.stop(running.handle);
            self.running = None;
        }
    }

    /// Ends what's running at a step, before its clock moves, leaving what it drew to fade. Steps no frame
    /// reached go in first: a cancel on a frame's second step, or a capture that takes all its steps first,
    /// still has the arc up to there.
    fn end(&mut self, world: &mut VfxWorld) {
        let Some(running) = self.running.take() else { return };
        let (last, offset) = running.drawn.unwrap_or((f64::NEG_INFINITY, None));
        for reading in running.readings.as_slice().iter().filter(|reading| reading.position > last) {
            let sockets = offset.unwrap_or_default().on(reading.sockets);
            let ago = (self.latest.elapsed - reading.position) as f32 / TICK_HZ as f32;
            world.sample_trail(running.handle, sockets.base, sockets.tip, ago);
        }
        world.stop(running.handle);
    }
}

/// Every fighter's blade, the player first. It starts with room for a duel, grows as it's told of more, and
/// they stay.
#[derive(Clone, Debug)]
pub(crate) struct BladeTrails {
    fighters: Vec<FighterTrail>,
    /// What a warrior's blade draws, for those who come later.
    warrior: Option<EffectId>,
}

impl BladeTrails {
    /// The trails `effects` has for a fight's blades. A library without them, as a test's may be, plays
    /// none.
    pub(crate) fn new(effects: &EffectLibrary) -> Self {
        Self {
            fighters: vec![
                FighterTrail::new(effects.find(PLAYER_TRAIL), Rails::PLAYER, true),
                FighterTrail::new(effects.find(WARRIOR_TRAIL), Rails::WARRIOR, false),
            ],
            warrior: effects.find(WARRIOR_TRAIL),
        }
    }

    /// What the latest step said of each fighter, the player first, `None` for one with no blade drawn:
    /// call it after each step, before the effects' clocks move, so a cut's first tick ages from its start.
    /// Allocates nothing, but for a fighter it hasn't been told of before.
    pub(crate) fn read_step<'a>(
        &mut self,
        fighters: impl IntoIterator<Item = Option<&'a ActorView>>,
        library: &MoveLibrary,
        world: &mut VfxWorld,
    ) {
        for (place, view) in fighters.into_iter().enumerate() {
            if place == self.fighters.len() {
                self.fighters.push(FighterTrail::new(self.warrior, Rails::WARRIOR, false));
            }
            self.fighters[place].read_step(view, library, world);
        }
    }

    /// Gets ready for this many fighters, so that the next ones' first steps allocate nothing.
    pub(crate) fn stand_by(&mut self, fighters: usize) {
        while self.fighters.len() < fighters {
            self.fighters.push(FighterTrail::new(self.warrior, Rails::WARRIOR, false));
        }
    }

    /// Whether a frame has anything to tell a trail.
    pub(crate) fn are_running(&self) -> bool {
        self.fighters.iter().any(|trail| trail.running.is_some())
    }

    /// Whether the trail of the fighter at `place` plays now.
    #[cfg(test)]
    pub(crate) fn runs_at(&self, place: usize) -> bool {
        self.fighters[place].running.is_some()
    }

    /// The frame's blades as drawn, `alpha` of the way from the step before the latest to the latest, as
    /// each one's transform and pose are: call it after they're placed and before the frame's effects are
    /// written. Allocates nothing.
    pub(crate) fn follow_frame(
        &mut self,
        drawn: impl IntoIterator<Item = Option<Mat4>>,
        alpha: f32,
        world: &mut VfxWorld,
    ) {
        // Written so that NaN draws the previous state, as `VfxWorld::write_frame` reads it.
        let unplayed = 1.0 - if alpha >= 0.0 { alpha.min(1.0) } else { 0.0 };
        for (trail, place) in self.fighters.iter_mut().zip(drawn) {
            trail.follow_frame(place, unplayed, world);
        }
    }

    /// A new fight: what the last one had running is gone with its world's effects.
    pub(crate) fn start_over(&mut self) {
        for trail in &mut self.fighters {
            trail.running = None;
            trail.playing = None;
            trail.cuts = Cuts::default();
        }
    }

    /// New effects (a reload), which end all that's playing. A library without a trail's effect plays
    /// none of that trail until it has again.
    #[cfg(any(test, feature = "dev"))]
    pub(crate) fn take_effects(&mut self, effects: &EffectLibrary) {
        self.warrior = effects.find(WARRIOR_TRAIL);
        for (place, trail) in self.fighters.iter_mut().enumerate() {
            trail.effect = if place == 0 { effects.find(PLAYER_TRAIL) } else { self.warrior };
            trail.running = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use sapphire_gameplay::Simulation;
    use sapphire_moves::{BakedPath, BakedTables, RootSample, Span};
    use sapphire_render::VfxFrame;
    use sapphire_time::SimDuration;
    use sapphire_vfx::{OwnerSteps, VfxBudget};

    use super::*;
    use crate::combat::CombatOption;
    use crate::combat::{Combat, no_bindings};

    /// A frame's reads between two steps, in the order frames come.
    const ALPHAS: [f32; 3] = [0.25, 0.5, 1.0];
    /// How far these tests' drawn blades stand from their tables, as look IK would have them.
    const OFF_THE_TABLE: Vec3 = Vec3::new(0.05, -0.1, 0.08);

    /// A recorded fight beside its trails, with blades drawn off the moves' tables by [`OFF_THE_TABLE`].
    struct Bout {
        combat: Combat,
        trails: BladeTrails,
        world: VfxWorld,
        frame: VfxFrame,
        owners: Vec<(sapphire_vfx::OwnerId, f32)>,
    }

    impl Bout {
        fn new(replay: &str) -> Self {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/replays")
                .join(format!("{replay}.sreplay"));
            let combat =
                Combat::new(&CombatOption::Replay { path, to_end: true }).expect("a committed recording");
            let effects = EffectLibrary::default_effects();
            Self {
                combat,
                trails: BladeTrails::new(&effects),
                world: VfxWorld::new(effects, VfxBudget::DESKTOP_HIGH, 1),
                frame: VfxFrame::default(),
                owners: Vec::new(),
            }
        }

        fn simulation(&self) -> &Simulation {
            self.combat.simulation()
        }

        /// Who's on the field for each of the cast's places, as the arena has them.
        fn views(&self) -> [Option<ActorView>; 2] {
            let sim = self.simulation();
            let cast = self.combat.cast();
            [0, 1].map(|place| cast.get(place).and_then(|id| sim.actor(sim.party().on_field(*id))))
        }

        /// One step, as `CueShow::step` runs its trails, the actors' clocks moving their effects.
        fn step(&mut self) {
            self.combat.step(&mut no_bindings);
            let [player, enemy] = self.views();
            let sim = self.combat.simulation();
            self.trails.read_step([player.as_ref(), enemy.as_ref()], sim.move_library(), &mut self.world);
            self.owners.clear();
            self.owners
                .extend(sim.actors().map(|actor| (owner_of(actor.id), actor.clock_delta.as_secs_f32())));
            self.world.advance(&OwnerSteps { world: 1.0 / TICK_HZ as f32, owners: &self.owners });
        }

        /// The blade `view` draws at `alpha`.
        fn drawn(&self, view: Option<ActorView>, alpha: f32) -> Option<Mat4> {
            let view = view?;
            let current = view.moves.current?;
            let path = self.simulation().move_library().get(current.at)?.tables.path(BLADE)?;
            let behind = (1.0 - alpha) * view.clock_delta.as_secs_f32();
            let elapsed = current.elapsed.as_secs_f32() - behind;
            let sample =
                path.at(SimDuration::from_duration(std::time::Duration::from_secs_f32(elapsed.max(0.0))));
            Some(
                Mat4::from_translation(view.position + OFF_THE_TABLE)
                    * Mat4::from_rotation_y(view.yaw)
                    * Mat4::from_rotation_translation(
                        Quat::from_array(sample.rotation),
                        Vec3::from_array(sample.translation),
                    ),
            )
        }

        /// One frame at `alpha`, what it draws in `self.frame`.
        fn draw(&mut self, alpha: f32) {
            let [player, enemy] = self.views();
            let drawn = [self.drawn(player, alpha), self.drawn(enemy, alpha)];
            self.trails.follow_frame(drawn, alpha, &mut self.world);
            self.world.write_frame(&mut self.frame, alpha);
        }

        /// Whether the place's trail plays now.
        fn running(&self, place: usize) -> bool {
            self.trails.runs_at(place)
        }

        fn ribbons(&self) -> usize {
            self.frame.ribbon_points.iter().filter(|point| point.ends).count()
        }
    }

    /// Each move's cuts, in the set the fighter at `place` first plays from.
    fn cuts_by_label(replay: &str, place: usize) -> Vec<(String, Vec<Cut>)> {
        let mut bout = Bout::new(replay);
        for _ in 0..4000 {
            bout.step();
            let [player, enemy] = bout.views();
            if let Some(current) = [player, enemy][place].and_then(|view| view.moves.current) {
                let library = bout.simulation().move_library();
                let set = library.current(current.at.set).expect("its set");
                return set
                    .moves()
                    .iter()
                    .map(|moved| (moved.label().to_owned(), Cuts::of(moved).as_slice().to_vec()))
                    .collect();
            }
        }
        panic!("nobody moved");
    }

    fn cut(from: u32, to: u32) -> Cut {
        Cut { from, to }
    }

    /// A cut starts when the blade flies, which is before or after its hit opens by move, and ends as it
    /// slows, and only what cuts has one. These are the shipped moves' numbers: a pose that moves them
    /// by more than a tick wants a look at how its trail reads.
    #[test]
    fn a_cut_is_where_the_blade_flies_about_its_hit() {
        let player = cuts_by_label("arena-fight-won", 0);
        let of = |label: &str, moves: &[(String, Vec<Cut>)]| {
            moves.iter().find(|(name, _)| name == label).unwrap_or_else(|| panic!("{label}")).1.clone()
        };
        // The first cut brakes into its low follow-through on tick 12.
        assert_eq!(of("light_1", &player), [cut(8, 12)]);
        assert_eq!(of("light_2", &player), [cut(8, 12)]);
        assert_eq!(of("light_3", &player), [cut(13, 20)]);
        assert_eq!(of("dash_attack", &player), [cut(5, 12)]);
        assert_eq!(of("skill", &player), [cut(12, 15), cut(21, 26)]);
        assert_eq!(of("dodge", &player), []);
        assert_eq!(of("flinch", &player), []);
        assert_eq!(of("knockdown", &player), []);
        let enemy = cuts_by_label("arena-fight-won", 1);
        assert_eq!(of("swipe", &enemy), [cut(24, 30)]);
        assert_eq!(of("overhead", &enemy), [cut(45, 52)]);
        assert_eq!(of("lunge", &enemy), [cut(30, 38)]);
        assert_eq!(of("stagger_light", &enemy), []);
        assert_eq!(of("death", &enemy), []);
    }

    /// A shipped move, to hang tables and hits of a test's own on: `samples` along +X, one a tick, and a
    /// hit on the blade for each of `hits`, as a move's hits are, sorted by when they open.
    fn a_move_of(samples: &[f32], hits: &[(u32, u32)]) -> Move {
        let mut bout = Bout::new("arena-fight-won");
        while bout.views()[1].and_then(|view| view.moves.current).is_none() {
            bout.step();
        }
        let current = bout.views()[1].and_then(|view| view.moves.current).expect("just seen");
        let library = bout.simulation().move_library();
        let set = library.current(current.at.set).expect("its set");
        let swipe = set.moves().iter().find(|moved| moved.label() == "swipe").expect("the warrior's");
        let mut moved = swipe.clone();
        let path = samples.iter().map(|&x| PathSample { translation: [x, 0.0, 0.0], ..PathSample::IDENTITY });
        let path = BakedPath::new(BLADE, path.collect());
        moved.tables =
            BakedTables::new(vec![RootSample::ZERO; samples.len()], vec![path]).expect("sound tables");
        let hit = swipe.hitboxes.iter().find(|hit| hit.path() == BLADE).expect("its hit");
        moved.hitboxes = hits
            .iter()
            .map(|&(from, to)| {
                let mut one = hit.clone();
                one.span = Span { from, to };
                one
            })
            .collect();
        moved
    }

    /// A hit's window says when for a blade that never flies, however little of the move its table has:
    /// a pose can swing where its table holds still, and a table can be shorter than its move.
    #[test]
    fn a_slow_cuts_window_outlasts_a_short_table() {
        assert_eq!(Cuts::of(&a_move_of(&[0.0], &[(10, 14)])).as_slice(), [cut(10, 14)]);
        assert_eq!(Cuts::of(&a_move_of(&[0.0; 12], &[(10, 14)])).as_slice(), [cut(10, 14)]);
        assert_eq!(Cuts::of(&a_move_of(&[0.0; 30], &[(10, 14)])).as_slice(), [cut(10, 14)]);
    }

    /// A long hit's fastest run can come after a shorter hit's that opens later, and each has its cut.
    #[test]
    fn a_later_hits_earlier_cut_is_kept() {
        let mut samples = vec![0.0_f32; 31];
        let mut x = 0.0;
        for (tick, sample) in samples.iter_mut().enumerate() {
            *sample = x;
            x += match tick {
                5..8 => 0.5,
                20..24 => 1.0,
                _ => 0.0,
            };
        }
        let moved = a_move_of(&samples, &[(4, 26), (5, 8)]);
        assert_eq!(Cuts::of(&moved).as_slice(), [cut(5, 9), cut(20, 25)]);
    }

    /// Cuts that meet are one, and a list too long joins its last.
    #[test]
    fn cuts_that_meet_are_one() {
        let mut cuts = Cuts::default();
        cuts.add(cut(2, 5));
        cuts.add(cut(5, 8));
        assert_eq!(cuts.as_slice(), [cut(2, 8)]);
        for from in 1..9 {
            cuts.add(cut(from * 10, from * 10 + 2));
        }
        assert_eq!(cuts.as_slice().len(), MAX_CUTS);
        assert_eq!(cuts.as_slice()[MAX_CUTS - 1], cut(30, 82));
    }

    /// Through whole fights, on either fighter, a trail plays for just the ticks of a cut (an effect's
    /// last frame closing it), and every way a cut ends early leaves nothing behind on a later frame.
    /// A strip's neighbouring points never jump as far as one move's blade to another's would.
    #[test]
    fn a_trail_plays_only_in_its_cut_and_any_new_move_ends_it() {
        let mut endings = BTreeSet::new();
        for replay in ["arena-fight-won", "fight", "cancel-windows", "dodge-counter", "party-swap", "string"]
        {
            let mut bout = Bout::new(replay);
            let mut seen = [None; 2];
            for _ in 0..bout.combat.ticks_left().expect("a recording") {
                let was: [Option<(ActorId, u32)>; 2] = seen;
                let was_running = [bout.running(0), bout.running(1)];
                bout.step();
                let views = bout.views();
                for place in 0..2 {
                    let view = views[place];
                    let current = view.and_then(|view| view.moves.current.map(|current| (view, current)));
                    seen[place] = current.map(|(view, current)| (view.id, current.serial));
                    let open = current.and_then(|(_, current)| {
                        let moved = bout.simulation().move_library().get(current.at)?;
                        Cuts::of(moved).open_at(duration_ticks(current.elapsed))
                    });
                    let trail = &bout.trails.fighters[place];
                    if open.is_some() {
                        assert!(bout.running(place), "{replay}, tick {}", bout.simulation().tick());
                    }
                    if seen[place] != was[place] && was_running[place] {
                        let label = current
                            .and_then(|(_, current)| {
                                let moved = bout.simulation().move_library().get(current.at)?;
                                Some(moved.label().to_owned())
                            })
                            .unwrap_or_else(|| "nothing".to_owned());
                        let swapped = seen[place].zip(was[place]).is_some_and(|(now, then)| now.0 != then.0);
                        endings.insert(if swapped { format!("swap to {label}") } else { label });
                        if open.is_none() {
                            assert!(trail.running.is_none(), "{replay}: {trail:?}");
                        }
                    }
                }
                bout.draw(1.0);
                let points = &bout.frame.ribbon_points;
                for (a, b) in points.iter().zip(points.iter().skip(1)).filter(|(a, _)| !a.ends) {
                    assert!(
                        a.tip.distance(b.tip) < 1.6,
                        "{replay} at {}: {a:?} {b:?}",
                        bout.simulation().tick()
                    );
                }
                for place in 0..2 {
                    let trail = &bout.trails.fighters[place];
                    if let Some(running) = trail.running {
                        assert!(!running.closing, "a frame closes it: {trail:?}");
                    }
                }
            }
            // What's left fades within its lifetime on the owner's clock, on and off the field.
            for _ in 0..60 {
                bout.step();
                bout.draw(1.0);
            }
        }
        // The recordings have these end a cut before its time: a cancel into the next attack, reactions
        // on either fighter, and a swap. `every_way_out_of_a_cut_ends_it` has the rest.
        for ending in ["light_2", "light_3", "flinch", "knockdown", "stagger_light", "swap to swap_in"] {
            assert!(endings.contains(ending), "{ending} in {endings:?}");
        }
    }

    /// Mid-cut, whatever comes next, on a step: the running trail stops there, and takes no more of the
    /// blade, wherever the blade is drawn next. So no strip stretches from one move to what follows.
    #[test]
    fn every_way_out_of_a_cut_ends_it() {
        type Next<'a> = &'a dyn Fn(ActorView, &MoveLibrary) -> Option<ActorView>;
        let into = |label: &'static str| {
            move |mut view: ActorView, library: &MoveLibrary| {
                let current = view.moves.current.as_mut().expect("in a move");
                let set = library.current(current.at.set).expect("its set");
                let index = set.moves().iter().position(|moved| moved.label() == label).expect("in the set");
                current.at.index = index as u32;
                current.serial += 1;
                current.elapsed = SimDuration::ZERO;
                Some(view)
            }
        };
        let (dodge, flinch, knockdown, next) =
            (into("dodge"), into("flinch"), into("knockdown"), into("light_2"));
        let none = |mut view: ActorView, _: &MoveLibrary| {
            view.moves.current = None;
            Some(view)
        };
        let gone = |_: ActorView, _: &MoveLibrary| None;
        let cases: [(&str, Next<'_>); 6] = [
            ("a dodge", &dodge),
            ("a flinch", &flinch),
            ("a defeat's fall", &knockdown),
            ("the next attack", &next),
            ("no move at all", &none),
            ("gone from the table", &gone),
        ];
        for (what, way_out) in cases {
            let mut bout = Bout::new("arena-fight-won");
            while !bout.running(0) {
                bout.step();
                bout.draw(1.0);
            }
            bout.step();
            bout.draw(1.0);
            bout.step();
            bout.draw(1.0);
            assert!(bout.running(0), "{what}");
            let strip = bout.frame.ribbon_points.len();
            assert!(strip >= 2, "{what}");
            let head = bout.frame.ribbon_points[strip - 1].tip;

            let [player, enemy] = bout.views();
            let sim = bout.combat.simulation();
            let after = way_out(player.expect("the player"), sim.move_library());
            bout.trails.read_step([after.as_ref(), enemy.as_ref()], sim.move_library(), &mut bout.world);
            assert!(!bout.running(0), "{what}");

            let away = Mat4::from_translation(Vec3::splat(5.0));
            bout.trails.follow_frame([Some(away), None], 1.0, &mut bout.world);
            bout.world.write_frame(&mut bout.frame, 1.0);
            assert!(!bout.frame.ribbon_points.is_empty(), "{what}: it's left to fade");
            for point in &bout.frame.ribbon_points {
                assert!(point.tip.distance(head) < 3.0, "{what}: {point:?} reached out from {head}");
            }
        }
    }

    /// After every cut, on the owner's clock, a strip's gone: a stopped effect isn't left in the air, even
    /// for a member a swap took off the field.
    #[test]
    fn what_a_trail_drew_fades_on_its_owners_clock() {
        for replay in ["arena-fight-won", "party-swap"] {
            let mut bout = Bout::new(replay);
            let mut drew = 0;
            let mut since = 0;
            for _ in 0..bout.combat.ticks_left().expect("a recording") {
                bout.step();
                bout.draw(1.0);
                let frozen = bout.views().iter().flatten().any(|view| view.clock_delta == SimDuration::ZERO);
                let trails = bout.running(0) || bout.running(1);
                drew += usize::from(bout.ribbons() > 0);
                since = if trails || bout.ribbons() == 0 || frozen { 0 } else { since + 1 };
                assert!(
                    since < 60,
                    "{replay} tick {}: a strip with nothing to feed it",
                    bout.simulation().tick()
                );
            }
            assert!(drew > 0, "{replay} drew one");
        }
    }

    /// A frame at any alpha ends the strip on the blade it draws, and a frozen clock holds all of it
    /// to the bit, whatever the frame's alpha.
    #[test]
    fn a_strip_ends_on_the_drawn_blade_and_a_hitstop_holds_it_exactly() {
        let mut bout = Bout::new("arena-fight-won");
        let (mut ends, mut held) = (0, 0);
        let mut first = None;
        for _ in 0..bout.combat.ticks_left().expect("a recording") {
            bout.step();
            let view = bout.views()[0];
            let frozen = view.is_some_and(|view| view.clock_delta == SimDuration::ZERO);
            let enemy_still = bout.views()[1].is_none_or(|view| view.clock_delta == SimDuration::ZERO);
            let before = bout.frame.ribbon_points.clone();
            for alpha in ALPHAS {
                let drawn = bout.drawn(view, alpha);
                bout.draw(alpha);
                // A strip has two samples once the blade has moved from where its trail began. Until a
                // frame closes it, it ends where the blade's drawn, among any others still fading.
                let sockets = drawn.map(|place| Rails::PLAYER.drawn(place));
                let handle = bout.trails.fighters[0].running.map(|running| running.handle);
                first = first.filter(|(from, _)| Some(*from) == handle).or(handle.zip(sockets));
                if let Some((_, from)) = first
                    && let Some(sockets) = sockets
                    && from.tip.distance(sockets.tip) > 2e-3
                {
                    let is_head = |point: &&sapphire_render::VfxRibbonPoint| {
                        point.ends
                            && point.tip.distance(sockets.tip) < 2e-3
                            && point.base.distance(sockets.base) < 2e-3
                    };
                    let head = bout.frame.ribbon_points.iter().find(is_head);
                    assert!(head.is_some(), "{sockets:?} in {:?}", bout.frame.ribbon_points);
                    ends += 1;
                }
                if frozen && enemy_still && !before.is_empty() {
                    assert_eq!(bout.frame.ribbon_points, before, "alpha {alpha}");
                    held += 1;
                }
            }
        }
        assert!(ends > 50, "{ends}");
        assert!(held > 10, "{held}");
    }

    /// With no steps, as in a pause, frames add nothing, however many there are.
    #[test]
    fn a_pause_holds_a_trail() {
        let mut bout = Bout::new("arena-fight-won");
        while !bout.running(0) {
            bout.step();
            bout.draw(1.0);
        }
        for _ in 0..3 {
            bout.step();
            bout.draw(1.0);
        }
        assert!(bout.running(0), "mid-cut");
        let held = bout.frame.ribbon_points.clone();
        assert!(!held.is_empty());
        for _ in 0..600 {
            bout.draw(1.0);
            assert_eq!(bout.frame.ribbon_points, held);
        }
    }

    /// A new fight's first cut starts clean, however the last ended: the same actor and serial play again,
    /// so it can't be told by what they play.
    #[test]
    fn starting_over_forgets_the_last_fight() {
        let mut bout = Bout::new("arena-fight-won");
        while !bout.running(0) {
            bout.step();
            bout.draw(1.0);
        }
        bout.world.clear();
        bout.trails.start_over();
        assert!(!bout.trails.are_running());
        bout.draw(1.0);
        assert_eq!(bout.ribbons(), 0);
        // Its fighters say the same as they did: it has only forgotten.
        let [player, enemy] = bout.views();
        let sim = bout.combat.simulation();
        bout.trails.read_step([player.as_ref(), enemy.as_ref()], sim.move_library(), &mut bout.world);
        assert!(bout.running(0), "they're still in the cut, on a new strip");
        bout.draw(1.0);
        assert_eq!(bout.ribbons(), 0, "with one sample so far");
    }

    /// New effects end what's playing, and the next cut plays them. Effects without the trails play none.
    #[test]
    fn new_effects_reach_the_next_cut() {
        let mut bout = Bout::new("arena-fight-won");
        while !bout.running(0) {
            bout.step();
        }
        let none = EffectLibrary::from_ron(
            "(type: \"sapphire::vfx::EffectLibrary\", version: 1, data: (effects: [(name: \"other\", \
             emitters: [(name: \"flies\", spawn: Burst(count: 1), lifetime: (1.0, 1.0), \
             motion: (speed: (1.0, 1.0)), size: [(0.0, 0.1)], color: [(0.0, (1.0, 1.0, 1.0))])])]))",
        )
        .expect("valid");
        bout.trails.take_effects(&none);
        bout.world.set_library(none);
        assert!(!bout.trails.are_running());
        for _ in 0..200 {
            bout.step();
            bout.draw(1.0);
            assert!(!bout.trails.are_running());
            assert_eq!(bout.ribbons(), 0);
        }
        let shipped = EffectLibrary::default_effects();
        bout.trails.take_effects(&shipped);
        bout.world.set_library(shipped);
        let mut ran = false;
        for _ in 0..400 {
            bout.step();
            ran |= bout.trails.are_running();
        }
        assert!(ran, "back on the next cut");
    }

    /// A phone's frames come every other step, and what's between comes from the table, so its arc keeps
    /// all but a sliver of what a frame on each step has.
    #[test]
    fn frames_every_other_step_keep_the_arc() {
        // The strip's tip, the moment a cut closes: as long as its points go.
        let arcs = |every: usize| -> Vec<f32> {
            let mut bout = Bout::new("arena-fight-won");
            let mut arcs = Vec::new();
            let mut longest = 0.0_f32;
            for tick in 0..bout.combat.ticks_left().expect("a recording").min(400) as usize {
                let was = bout.running(0);
                bout.step();
                if was && !bout.running(0) {
                    arcs.push(longest);
                    longest = 0.0;
                }
                if tick % every == 0 {
                    bout.draw(1.0);
                    let points = &bout.frame.ribbon_points;
                    let arc: f32 = points
                        .iter()
                        .zip(points.iter().skip(1))
                        .filter(|(a, _)| !a.ends)
                        .map(|(a, b)| a.tip.distance(b.tip))
                        .sum();
                    longest = longest.max(arc);
                }
            }
            arcs
        };
        let (each, every_other) = (arcs(1), arcs(2));
        assert!(!each.is_empty());
        let (each_total, other_total): (f32, f32) = (each.iter().sum(), every_other.iter().sum());
        assert!(other_total > each_total * 0.9, "{each:?} against {every_other:?}");
    }

    /// A capture takes all its steps before its first frame, and a hitch several: a cut that closed with
    /// no frame since ends where it closed, its arc kept, not on what the blade does in its recovery.
    #[test]
    fn a_cut_closed_with_no_frame_keeps_its_arc() {
        let mut bout = Bout::new("arena-fight-won");
        let serial = |bout: &Bout| bout.views()[0].and_then(|view| view.moves.current).map(|at| at.serial);
        let at_its_close = 'a_cut: loop {
            assert!(bout.combat.ticks_left().expect("a recording") > 8, "no cut closed with a recovery");
            bout.step();
            if !bout.trails.fighters[0].running.is_some_and(|running| running.closing) {
                continue;
            }
            let view = bout.views()[0].expect("on the field");
            let current = view.moves.current.expect("in its move");
            let library = bout.simulation().move_library();
            let path = library.get(current.at).and_then(|moved| moved.tables.path(BLADE));
            let at = Rails::PLAYER.in_table(path.expect("a cut's path").at(current.elapsed), &view);
            bout.step();
            assert!(!bout.running(0), "past its close: {:?}", bout.trails.fighters[0].running);
            for _ in 0..5 {
                bout.step();
                if serial(&bout) != Some(current.serial) {
                    continue 'a_cut;
                }
            }
            break at;
        };
        bout.draw(1.0);
        let heads = bout.frame.ribbon_points.iter().filter(|point| point.ends);
        let nearest = heads.map(|point| point.tip.distance(at_its_close.tip)).fold(f32::INFINITY, f32::min);
        assert!(nearest < 0.05, "the strip's head is {nearest} m from the blade at the close");
    }

    /// A fight's frames allocate nothing, nor do its steps, once warm: a whole fight's, in its own world,
    /// with its trails running.
    /// A warrior who wasn't there when the trails were made has a blade of its own, whichever place it's in.
    #[test]
    fn every_warrior_has_a_blade_of_its_own() {
        let mut bout = Bout::new("arena-skirmish");
        let cast = bout.combat.cast().to_vec();
        let mut cut = vec![false; cast.len()];
        for _ in 0..1200 {
            bout.combat.step(&mut no_bindings);
            let sim = bout.combat.simulation();
            let views: Vec<_> = cast.iter().map(|id| sim.actor(*id)).collect();
            bout.trails.read_step(views.iter().map(Option::as_ref), sim.move_library(), &mut bout.world);
            for (place, cut) in cut.iter_mut().enumerate() {
                *cut |= bout.trails.runs_at(place);
            }
        }
        assert_eq!(bout.trails.fighters.len(), cast.len());
        assert!(cut[1..].iter().filter(|cut| **cut).count() >= 3, "{cut:?}");
    }

    #[test]
    fn trails_allocate_nothing() {
        let mut bout = Bout::new("arena-fight-won");
        let (mut allocations, mut ran) = (0, 0);
        for _ in 0..bout.combat.ticks_left().expect("a recording") {
            bout.combat.step(&mut no_bindings);
            let [player, enemy] = bout.views();
            let drawn = [bout.drawn(player, 0.5), bout.drawn(enemy, 0.5)];
            let Bout { trails, world, combat, .. } = &mut bout;
            let library = combat.simulation().move_library();
            let ((), counts) = sapphire_core::alloc::count_in_scope(|| {
                trails.read_step([player.as_ref(), enemy.as_ref()], library, world);
                trails.follow_frame(drawn, 0.5, world);
            });
            allocations += counts.allocations;
            ran += usize::from(trails.are_running());
            world.advance(&OwnerSteps { world: 1.0 / TICK_HZ as f32, owners: &[] });
        }
        assert!(ran > 100, "{ran} ticks with a trail running");
        if sapphire_core::alloc::PER_THREAD_COUNTS {
            assert_eq!(allocations, 0);
        }
    }

    /// The rails are where the meshes say: the player's sword's source, and the warrior's blade's file.
    #[test]
    fn the_rails_are_on_the_blades_as_modelled() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let sword = std::fs::read_to_string(root.join("tools/sapphire_pipeline/src/combat/sword.rs"))
            .expect("the sword's source");
        assert!(sword.contains(&format!("const BLADE_ROOT: f32 = {};", Rails::PLAYER.base.y)), "base");
        assert!(sword.contains(&format!("pub const HALF_LENGTH: f32 = {};", Rails::PLAYER.tip.y)), "tip");
        let blade = root.join("content/third_party/kaykit_skeletons/assets/Skeleton_Blade.gltf");
        let blade: String = std::fs::read_to_string(blade)
            .expect("the warrior's blade")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let top = blade.split("\"max\":[").nth(1).expect("a bound").split(',').nth(1).expect("its y");
        let top: f32 = top.parse().expect("a number");
        assert!((Rails::WARRIOR.tip.y - top).abs() < 1e-3, "{top} against {:?}", Rails::WARRIOR);
    }
}
