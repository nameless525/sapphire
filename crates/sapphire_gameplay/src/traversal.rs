//! Traversal moves (M6 A15): a ledge in front of a fighter, found by a few casts on the tick it presses jump,
//! starts a mantle onto it or a vault over it, in place of the jump. Climbing's presses (M6 A16a): a climbable
//! face in front of it grabbed, let go of, and topped out over by a mantle. And gliding's (M6 A17): a glide opened
//! in the air high enough, and dropped.
//!
//! # What finds a ledge
//!
//! `find_ledges` runs in [`CombatSet::Intents`](crate::CombatSet::Intents), after targeting, for every actor on
//! the field whose set names traversal moves ([`sapphire_moves::TraversalMoves`]) and that presses jump this tick,
//! grounded, with no move running and its clock moving. [`probe`] asks the collision world, in this order, and
//! stops at the first refusal:
//!
//! 1. **The face:** a ray from the axis at half the lowest ledge's height, along the facing, for the capsule's
//!    radius and [`REACH`] more. It must meet a face too steep to walk up within 45° of the facing, inclusive by
//!    a slack ([`APPROACH_COS`]).
//! 2. **The top:** a ray down from over the highest ledge, just past the face's edge, where the axis would
//!    stand. Its height over the feet must be within the profile's mantle heights, inclusive by
//!    [`HEIGHT_SLACK`] (A14's course puts its stations on those limits), and walkable. The slack never reaches
//!    into the step: a ledge no higher than `step_height` and the slack is stepped, never mantled.
//! 3. **Room to rise:** the capsule cast straight up by the ledge's height and [`CLEARANCE`], or for a vault
//!    (a running fighter, a set with one, an obstacle no taller than `vault_max_height`) by its table's apex
//!    if that's higher.
//! 4. For a vault, **the far side:** a ray down where the capsule's near side lands, [`ON_TOP`] past an
//!    obstacle [`VAULT_DEPTH`] deep, which must find ground lower than the top by at least the lowest ledge;
//!    without it, the press mantles instead. Then **room to cross**, and for a mantle **room to go in**: the
//!    capsule cast at the height it rose to, over the edge to where it lands or stands. Something in the way,
//!    such as a ceiling low over the top, refuses the press: a vault with no room doesn't fall back to a
//!    mantle, which would cost more casts.
//!
//! So a press costs at most five casts (two rays and two capsule casts for a mantle, a ray and a cast more
//! for a vault), and a tick with no press costs none: [`crate::SimStats::ledge_casts`] counts them. Only
//! static scenery in [`Group::World`] is a ledge; the capsule's own casts meet whatever it meets.
//!
//! # What it starts
//!
//! A found ledge forces its move ([`MoveRunner::force_toward`]) aimed at where the root should arrive: on the
//! top, [`CLEARANCE`] over it, for a mantle, and on the far ground for a vault. The aim is the move's own and
//! ends with it, so the actor's [`crate::WarpTarget`] is left as it was and no later move warps toward the
//! ledge. The move's warp windows bring the table there ([`sapphire_moves::WarpWindow::max_rise`] for the
//! height) and it flies ([`sapphire_moves::MoveDrive::Flight`]). A refused press jumps as it would have, unless
//! it grabs a face. None of this enters a [`crate::TraversalMode`]: a mantle is a move.
//!
//! # Climbing (M6 A16a)
//!
//! `climb_and_glide` runs after `find_ledges`, for the same fighters (a set that names mantles, since a climb's top
//! is a mantle's), on the field, standing or defeated not, no move running or forced, clock moving:
//!
//! - **A grab:** a press of jump that no ledge took, standing or in the air, casts one ray from the hands (the
//!   capsule's top) along the facing, `radius + climb_reach` long. A static face in [`Group::Climbable`],
//!   too steep to stand on, not a ceiling and within 45° of the facing ([`APPROACH_COS`]) is grabbed: the
//!   fighter enters [`crate::TraversalMode::Climbing`] holding it ([`crate::ClimbHold`]), and the controller
//!   climbs from its next move. A face only in [`Group::World`] is never grabbed, and nor is any face once
//!   its stamina is spent, until the feet stand on the ground again. Why a press, not a run into the face: the
//!   button is one the ledges already read, so a tick with no press costs nothing, and nobody fighting beside
//!   tagged scenery climbs it by walking into it.
//! - **A let-go:** a press of jump while climbing: the fighter falls from where it hangs.
//! - **A top-out:** once the hands have reached the face's lip ([`crate::Lip::Reached`]) and the stick asks
//!   for higher, [`probe`] runs from where it hangs, as from the ground, and a ledge it finds forces the mantle
//!   for its height (the heroine's lip is about 1.6 m over her feet: `mantle_high`). The move takes the stick,
//!   which lets go of the face. A refused top-out marks the lip [`crate::Lip::Refused`], and it hangs there
//!   until it climbs down or moves along the top. A16a reuses the set's mantles rather than adding a
//!   `climb_top` move: a hang's top is a mantle from the feet's height, which their warp already bends to,
//!   and a new move would change every set's format for no new motion. A16b may key one for the look, with
//!   the format's bump and every replay's re-stamp that brings. A set whose numbers can't top out from a hang
//!   is refused when it compiles or cooks.
//!
//! So a grab's press costs one cast more than a ledge's probe, a let-go none, and a top-out the probe's four.
//! [`crate::SimStats::climb_casts`] counts them with the controller's.
//!
//! # Gliding (M6 A17)
//!
//! `climb_and_glide` reads a press of jump in the air too, for a fighter whose set names traversal moves (a set
//! with none never glides, so no enemy opens one by a press), once the press has grabbed no face:
//!
//! - **Opening a glide:** a press in the air, with the coyote time after a ledge spent (in it, the press jumps),
//!   rising or falling, casts one ray down from the feet, the profile's `glide_min_height` long, against static
//!   scenery in [`Group::World`]. If it meets nothing, the ground is further down than that, or there is none in
//!   reach, and the fighter enters [`crate::TraversalMode::Gliding`]: the controller sinks it and steers it from
//!   this tick's move on. Ground within the height refuses it, and the press does what it did before (nothing, in
//!   the air). Why a press, not a held button: it's the reference games' feel, where a press in the air opens the
//!   glider and another folds it, and a press is what every other traversal reads, so a tick with no press costs
//!   nothing. A spent stamina opens none until the feet stand on the ground again.
//! - **Dropping it:** a press while gliding, unless it grabs a climbable face in front of the glider (the same
//!   ray from the hands as any grab, so a glide meets a climbable wall by a press, and only for a set that
//!   climbs): the fighter falls from there, with the speed it had. A fallen glider drops at once, as a fallen
//!   climber lets go.
//!
//! So an opening press costs one cast (two for a set that climbs, whose press first looks for a face), a drop
//! none of its own (a set that climbs first looks for a face, A16a's one ray, counted in `climb_casts`), and a
//! gliding tick none of its own: [`crate::SimStats::glide_casts`] counts the opening rays. The
//! controller ends a glide on landing, when a move takes the stick and when stamina runs out.

use std::f32::consts::FRAC_1_SQRT_2;

use sapphire_ecs::prelude::*;
use sapphire_math::{Vec3, det};
use sapphire_moves::{Move, MoveSet};
use sapphire_physics::{BodyId, Group, PhysicsWorld, QueryFilter};
use sapphire_time::SimDuration;

use crate::TraversalMode;
use crate::actions::{Buttons, Controls};
use crate::actors::ActorTable;
use crate::controller::{
    CharacterController, CharacterWorld, GroundMode, Lip, ResolvedProfile, actor_body, climbable_face,
    drop_glide, grab, hands, into_face, let_go, open_glide, wants_up,
};
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveRunner};
use crate::party::Party;
use crate::resolve::Fighter;

/// How far past the capsule's side a ledge's face may be, in metres.
pub const REACH: f32 = 0.6;
/// The approach a ledge takes: its face within 45° of the facing, by the cosine, with a slack for the turn a
/// stick leaves a hair short.
pub const APPROACH_COS: f32 = FRAC_1_SQRT_2 - 1e-4;
/// How far outside the profile's mantle heights a ledge still counts, in metres, so a ledge built on a limit
/// takes it.
pub const HEIGHT_SLACK: f32 = 0.01;
/// How far over a ledge's top the feet come before they go in, in metres: the root's warp target.
pub const CLEARANCE: f32 = 0.05;
/// How far past a ledge's edge, beyond the capsule's radius and two skins, a mantle stands, in metres.
pub const ON_TOP: f32 = 0.05;
/// How deep an obstacle a vault crosses, in metres from its face: the far ray looks down [`ON_TOP`] past that,
/// where the capsule's near side lands, so an obstacle up to 0.85 m deep is vaulted and a deeper one mantled.
pub const VAULT_DEPTH: f32 = 0.8;

/// What a probe found in front of a fighter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ledge {
    pub kind: LedgeKind,
    /// The top's height over the feet, in metres.
    pub height: f32,
    /// Where the move's warp brings the root: on the top for a mantle, on the far ground for a vault.
    pub target: Vec3,
}

/// Which move a ledge takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgeKind {
    Mantle,
    Vault,
}

/// Why a press found nothing to mantle or vault: it jumps instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// No face in reach, or one gentle enough to walk up.
    NoFace,
    /// A face further than 45° from the facing.
    Approach,
    /// No walkable top within the mantle's heights over the face's edge.
    Height,
    /// Something in the way of the rise, of going in, or of crossing.
    NoRoom,
}

/// The traversal moves' counters, deterministic but not state: nothing reads them back.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TraversalStats {
    pub(crate) ledge_casts: u64,
    pub(crate) mantles_started: u64,
    pub(crate) vaults_started: u64,
    pub(crate) ledges_refused: u64,
    pub(crate) climb_casts: u64,
    pub(crate) climbs_started: u64,
    pub(crate) climb_top_outs: u64,
    pub(crate) glide_casts: u64,
    pub(crate) glides_started: u64,
}

/// Static scenery is a ledge; characters and blockers aren't.
fn ledge_filter() -> QueryFilter {
    QueryFilter::new(Group::World.mask())
}

const fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// Looks for a ledge in front of a fighter whose feet are at `feet`, facing `yaw` and moving at `velocity`:
/// see the [module docs](self). `vault_apex` is how high its set's vault lifts the root, if it has one. Adds
/// each cast it makes to `casts`, refused or not.
#[allow(clippy::too_many_arguments, reason = "a probe reads the fighter's whole standing")]
pub fn probe(
    world: &PhysicsWorld,
    body: BodyId,
    feet: Vec3,
    yaw: f32,
    velocity: Vec3,
    profile: &ResolvedProfile,
    vault_apex: Option<f32>,
    casts: &mut u32,
) -> Result<Ledge, Refused> {
    let p = profile.authored();
    let forward = det::direction_from_yaw(yaw);
    let lowest = p.mantle_min_height.max(p.step_height);
    let highest = p.mantle_max_height;

    *casts += 1;
    let knee = feet + Vec3::Y * (lowest * 0.5);
    let face = world.cast_ray(knee, forward, p.radius + REACH, &ledge_filter()).ok_or(Refused::NoFace)?;
    let into = flat(-face.normal).try_normalize().ok_or(Refused::NoFace)?;
    if profile.walkable(face.normal) {
        return Err(Refused::NoFace);
    }
    if forward.dot(into) < APPROACH_COS {
        return Err(Refused::Approach);
    }

    // Where the axis stands on the top: just past the edge, so the whole capsule is over it.
    let edge = flat(face.point);
    let past = p.radius + 2.0 * p.skin + ON_TOP;
    let stand = edge + into * past;
    let above = highest + HEIGHT_SLACK + CLEARANCE;
    *casts += 1;
    let from = Vec3::new(stand.x, feet.y + above, stand.z);
    let top = world.cast_ray(from, Vec3::NEG_Y, above - lowest * 0.5, &ledge_filter());
    // A ray starting inside something has met a ledge taller than any mantle.
    let top = top.filter(|top| top.distance > 0.0 && profile.walkable(top.normal)).ok_or(Refused::Height)?;
    let height = above - top.distance;
    // The slack widens the mantle's heights, never into the step's: a ledge the feet step onto, to within
    // the slack, is stepped.
    if height < p.mantle_min_height - HEIGHT_SLACK
        || height <= p.step_height + HEIGHT_SLACK
        || height > highest + HEIGHT_SLACK
    {
        return Err(Refused::Height);
    }

    let lift = height + CLEARANCE;
    let speed = flat(velocity).length();
    let running = speed >= p.walk_speed + (p.run_speed - p.walk_speed) * 0.5;
    // A vault rises to its table's apex, over the obstacle or not: the room is asked for that.
    let vault_apex = vault_apex.filter(|_| running && height <= p.vault_max_height + HEIGHT_SLACK);
    let rise = vault_apex.map_or(lift, |apex| lift.max(apex));
    *casts += 1;
    if !matches!(world.cast_body(body, Vec3::ZERO, Vec3::Y, rise), Ok(None)) {
        return Err(Refused::NoRoom);
    }

    if vault_apex.is_some() {
        // The axis lands a capsule's radius past where the far ray looks, just beyond an obstacle
        // VAULT_DEPTH deep, so the capsule's near side comes down on the ground past it.
        let beyond = edge + into * (VAULT_DEPTH + past);
        let near = beyond - into * (p.radius + 2.0 * p.skin);
        let from = Vec3::new(near.x, feet.y + lift, near.z);
        *casts += 1;
        let far = world.cast_ray(from, Vec3::NEG_Y, lift + p.step_height, &ledge_filter());
        let landing = far
            .filter(|far| far.distance > 0.0 && profile.walkable(far.normal))
            .map(|far| from.y - far.distance);
        if let Some(landing) = landing.filter(|&landing| landing <= feet.y + height - lowest + HEIGHT_SLACK) {
            go_over(world, body, feet, beyond, rise, casts)?;
            return Ok(Ledge {
                kind: LedgeKind::Vault,
                height,
                target: Vec3::new(beyond.x, landing, beyond.z),
            });
        }
    }
    go_over(world, body, feet, stand, lift, casts)?;
    let target = Vec3::new(stand.x, feet.y + lift, stand.z);
    Ok(Ledge { kind: LedgeKind::Mantle, height, target })
}

/// Whether the capsule, lifted by `lift`, has room to go across to over `to`.
fn go_over(
    world: &PhysicsWorld,
    body: BodyId,
    feet: Vec3,
    to: Vec3,
    lift: f32,
    casts: &mut u32,
) -> Result<(), Refused> {
    let across = to - flat(feet);
    *casts += 1;
    match world.cast_body(body, Vec3::Y * lift, across, across.length()) {
        Ok(None) => Ok(()),
        _ => Err(Refused::NoRoom),
    }
}

/// How high a vault's table lifts the root over where it starts: the room it needs over the fighter.
fn vault_apex(vault: &Move) -> f32 {
    vault.tables.root_samples().iter().fold(0.0, |apex, sample| det::max(apex, sample.translation[1]))
}

/// [`crate::CombatSet::Intents`], after targeting: a press of jump in front of a ledge starts a mantle or a
/// vault. See the [module docs](self).
#[allow(clippy::type_complexity, reason = "a system's query reads as what it touches")]
pub(crate) fn find_ledges(
    table: Res<'_, ActorTable>,
    party: Res<'_, Party>,
    library: Res<'_, MoveLibrary>,
    characters: Res<'_, CharacterWorld>,
    mut stats: ResMut<'_, TraversalStats>,
    mut actors: Query<
        '_,
        '_,
        (&Controls, &EntityClock, &mut MoveRunner, &CharacterController, &ActorTransform, &Fighter),
    >,
) {
    for (id, entity) in table.iter() {
        let Ok((controls, clock, mut runner, controller, transform, fighter)) = actors.get_mut(entity) else {
            continue;
        };
        if !controls.pressed().contains(Buttons::JUMP)
            || party.is_off_field(id)
            || fighter.is_defeated()
            || runner.is_running()
            || runner.forced.is_some()
            || controller.state.mode != GroundMode::Grounded
            || clock.0.last_delta() == SimDuration::ZERO
        {
            continue;
        }
        let Some(set) = runner.set() else { continue };
        let Some(moves) = library.current(set).and_then(|current| current.traversal().map(|t| (current, t)))
        else {
            continue;
        };
        let Some(spawned) = controller.profile().and_then(|profile| characters.profile(profile)) else {
            continue;
        };
        let profile = library.movement(set).unwrap_or(spawned);
        let (current, traversal) = moves;
        let mut casts = 0;
        let found = probe(
            characters.physics(),
            actor_body(id),
            transform.position,
            transform.yaw,
            controller.state.velocity,
            profile,
            traversal.vault.and_then(|at| current.move_at(at)).map(vault_apex),
            &mut casts,
        );
        stats.ledge_casts += u64::from(casts);
        let chosen = found.ok().and_then(|ledge| {
            let at = match ledge.kind {
                LedgeKind::Mantle => traversal.mantle_for(ledge.height - HEIGHT_SLACK)?,
                LedgeKind::Vault => traversal.vault?,
            };
            Some((ledge, current.move_at(at)?.name()))
        });
        let Some((ledge, name)) = chosen else {
            stats.ledges_refused += 1;
            continue;
        };
        // The move holds the ledge's point as its own aim, so nothing after it warps toward the ledge.
        runner.force_toward(name, ledge.target);
        match ledge.kind {
            LedgeKind::Mantle => stats.mantles_started += 1,
            LedgeKind::Vault => stats.vaults_started += 1,
        }
    }
}

/// [`crate::CombatSet::Intents`], after [`find_ledges`]: a press of jump grabs a climbable face in front of a
/// fighter or lets go of the one it holds, and a climber at a face's top tops out; a press in the air that grabs
/// nothing opens a glide or drops it. See the [module docs](self).
#[allow(clippy::type_complexity, reason = "a system's query reads as what it touches")]
pub(crate) fn climb_and_glide(
    table: Res<'_, ActorTable>,
    party: Res<'_, Party>,
    library: Res<'_, MoveLibrary>,
    characters: Res<'_, CharacterWorld>,
    mut stats: ResMut<'_, TraversalStats>,
    mut actors: Query<
        '_,
        '_,
        (&Controls, &EntityClock, &mut MoveRunner, &mut CharacterController, &ActorTransform, &Fighter),
    >,
) {
    for (id, entity) in table.iter() {
        let Ok((controls, clock, mut runner, mut controller, transform, fighter)) = actors.get_mut(entity)
        else {
            continue;
        };
        if fighter.is_defeated() && controller.state.climb.is_some() {
            // A fallen climber lets go at once, whatever drives its fall, so nothing waits on it to.
            let_go(&mut controller.state);
            continue;
        }
        if fighter.is_defeated() && controller.state.traversal == TraversalMode::Gliding {
            // A fallen glider drops, as a fallen climber lets go.
            drop_glide(&mut controller.state);
            continue;
        }
        if party.is_off_field(id)
            || fighter.is_defeated()
            || runner.is_running()
            || runner.forced.is_some()
            || clock.0.last_delta() == SimDuration::ZERO
        {
            continue;
        }
        let pressed = controls.pressed().contains(Buttons::JUMP);
        let Some(hold) = controller.state.climb else {
            let free = matches!(controller.state.traversal, TraversalMode::None | TraversalMode::Gliding);
            if pressed
                && free
                && !grab_ahead(&characters, &library, &runner, &mut controller, transform, &mut stats)
            {
                glide_press(&characters, &library, &runner, &mut controller, transform, &mut stats);
            }
            continue;
        };
        if pressed {
            let_go(&mut controller.state);
            continue;
        }
        if hold.lip != Lip::Reached || !wants_up(controls.current.stick.to_vec2(), &hold) {
            continue;
        }
        let Some((profile, traversal, current)) = climber(&characters, &library, &runner, &controller) else {
            continue;
        };
        let mut casts = 0;
        let found = probe(
            characters.physics(),
            actor_body(id),
            transform.position,
            transform.yaw,
            Vec3::ZERO,
            profile,
            None,
            &mut casts,
        );
        stats.climb_casts += u64::from(casts);
        let mantle = found.ok().filter(|ledge| ledge.kind == LedgeKind::Mantle).and_then(|ledge| {
            let at = traversal.mantle_for(ledge.height - HEIGHT_SLACK)?;
            Some((ledge, current.move_at(at)?.name()))
        });
        match mantle {
            Some((ledge, name)) => {
                // The mantle takes the stick, which lets go of the face as it starts.
                runner.force_toward(name, ledge.target);
                stats.climb_top_outs += 1;
            }
            None => {
                if let Some(hold) = &mut controller.state.climb {
                    hold.lip = Lip::Refused;
                }
            }
        }
    }
}

/// What a fighter climbs by, if it climbs: its numbers, its set's traversal moves and the set.
fn climber<'a>(
    characters: &'a CharacterWorld,
    library: &'a MoveLibrary,
    runner: &MoveRunner,
    controller: &CharacterController,
) -> Option<(&'a ResolvedProfile, &'a sapphire_moves::TraversalMoves, &'a MoveSet)> {
    let set = runner.set()?;
    let current = library.current(set)?;
    let traversal = current.traversal().filter(|traversal| !traversal.mantles.is_empty())?;
    let spawned = characters.profile(controller.profile()?)?;
    Some((library.movement(set).unwrap_or(spawned), traversal, current))
}

/// A press's grab: the face along the facing, from the hands. Counts its cast, grabbed or not, and says whether it
/// grabbed.
fn grab_ahead(
    characters: &CharacterWorld,
    library: &MoveLibrary,
    runner: &MoveRunner,
    controller: &mut CharacterController,
    transform: &ActorTransform,
    stats: &mut TraversalStats,
) -> bool {
    let Some((profile, ..)) = climber(characters, library, runner, controller) else { return false };
    if spent(profile, controller) {
        return false;
    }
    let forward = det::direction_from_yaw(transform.yaw);
    let center = transform.position + profile.feet_to_center();
    let reach = profile.authored().radius + profile.authored().climb_reach;
    stats.climb_casts += 1;
    let Some(face) = climbable_face(characters.physics(), profile, hands(profile, center), forward, reach)
    else {
        return false;
    };
    if forward.dot(into_face(face.normal)) < APPROACH_COS {
        return false;
    }
    grab(&mut controller.state, face.normal);
    stats.climbs_started += 1;
    true
}

/// Whether a fighter's stamina is spent, so it can neither grab a face nor open a glide.
fn spent(profile: &ResolvedProfile, controller: &CharacterController) -> bool {
    profile.stamina().is_some_and(|stamina| controller.state.stamina_spent >= stamina)
}

/// A press that grabbed nothing: in a glide it drops it, and in the air high enough, for a set that names
/// traversal moves, it opens one. Counts the opening's ray.
fn glide_press(
    characters: &CharacterWorld,
    library: &MoveLibrary,
    runner: &MoveRunner,
    controller: &mut CharacterController,
    transform: &ActorTransform,
    stats: &mut TraversalStats,
) {
    let state = &controller.state;
    if state.traversal == TraversalMode::Gliding {
        drop_glide(&mut controller.state);
        return;
    }
    if state.mode != GroundMode::Airborne || state.coyote > SimDuration::ZERO {
        return;
    }
    let Some(set) = runner.set() else { return };
    if library.current(set).and_then(MoveSet::traversal).is_none() {
        return;
    }
    let Some(spawned) = controller.profile().and_then(|profile| characters.profile(profile)) else { return };
    let profile = library.movement(set).unwrap_or(spawned);
    if spent(profile, controller) {
        return;
    }
    stats.glide_casts += 1;
    let height = profile.authored().glide_min_height;
    if characters.physics().cast_ray(transform.position, Vec3::NEG_Y, height, &ledge_filter()).is_some() {
        return;
    }
    open_glide(&mut controller.state, profile);
    stats.glides_started += 1;
}
