//! Pillar 5, counted: how often the real director, framing real fights in the real arena, shows what a
//! person would call a framing fault.
//!
//! Nobody has played the fight, so these numbers aren't how it feels: they catch regressions and say
//! where to look. Each [`Fight`] runs through [`ArenaActors`], the same `follow`, `drive_camera` and
//! `present` the app calls, at a phone's and a desktop's frame rate and shape. Every frame is read against
//! two worlds: the colliders the camera can see, and, when a cook is there, the triangles a player can
//! (the town's and the plantings', with [`VisibleTown`]), since the town draws more than it collides with.
//!
//! - `framing_stays_within_its_bounds` runs on every test run, on colliders alone, and holds the numbers:
//!   each fight's to what's asked of it, so one gone wrong has nowhere to hide among the rest, and all of
//!   them together.
//! - `framing_report` is the whole measurement, for someone changing the camera:
//!
//! ```text
//! cargo xtask content
//! SAPPHIRE_FRAMING_REPLAYS=<dir> cargo test -p sapphire_stage --lib framing_report -- --ignored --nocapture
//! ```
//!
//! With that variable set, each scripted fight is also written there as a replay, so a moment it reports can
//! be looked at: `--arena --arena-shot director --replay <file> --capture <png> --capture-ticks T
//! --capture-frames T+1 --capture-step 1`. A capture with no `--capture-step` draws one frame after all its
//! ticks, where the director has never seen the fight. `SAPPHIRE_FRAMING_TUNING=<file>` runs another camera
//! document, and `SAPPHIRE_FRAMING_BRIEF=1` prints only each condition's whole row, for comparing tunings.
//!
//! The camera can't move a fight, since the simulation never reads it: only a collider that a body or a
//! hit meets does, so tunings compare frame for frame on the same fights.

#![allow(clippy::print_stdout, reason = "with --nocapture, the measurement's report")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use sapphire_assets::mesh::{ArchivedMesh, Mesh};
use sapphire_assets::pack::PackReader;
use sapphire_camera::{CameraPose, CameraStats};
use sapphire_gameplay::TickActions;
use sapphire_gameplay::replay::{
    Player, Recording, Replay, SCENARIOS, Scenario, fight_back, find_scenario, find_script,
};
use sapphire_gameplay::{ActorActions, Stick};
use sapphire_physics::{ColliderId, Group, GroupMask, RayHit, Shape, StaticCollider, StaticShape};

use super::*;
use sapphire_stage::lookdev::{TOWN_STEM, TOWN_ZONE, plantings};

/// A desktop's shape and a phone's: an iPhone 17 Pro's 2622 by 1206.
const DESKTOP: f32 = 16.0 / 9.0;
const PHONE: f32 = 2622.0 / 1206.0;

/// A rate and a shape to run a fight at.
#[derive(Clone, Copy, Debug)]
struct Condition {
    label: &'static str,
    rate: u32,
    aspect: f32,
}

const CONDITIONS: [Condition; 4] = [
    Condition { label: "60 Hz 16:9", rate: 60, aspect: DESKTOP },
    Condition { label: "60 Hz phone", rate: 60, aspect: PHONE },
    Condition { label: "30 Hz 16:9", rate: 30, aspect: DESKTOP },
    Condition { label: "30 Hz phone", rate: 30, aspect: PHONE },
];

/// How much of the screen counts as framed, as a share of its half-size: past this a HUD or a thumb has
/// the fighter.
const MARGIN: f32 = 0.9;
/// The near plane, in metres (`OrbitCamera`'s), and how far its corners reach at the widest of these shapes.
const NEAR_REACH: f32 = 0.05 * 1.55;
/// A stretch of a fault shorter than this is a blink, and goes in the counts but not the list.
const WORTH_NAMING: f32 = 0.4;
/// An eye moving further than this in a frame, or turning further, reads as a cut.
const JUMP_METRES: f32 = 0.75;
const JUMP_RADIANS: f32 = 0.3;

/// Where the rays go on someone, how wide they draw. A fallen fighter is still measured as standing.
#[derive(Clone, Copy, Debug)]
struct Figure {
    head: f32,
    hips: f32,
    half_width: f32,
}

const PLAYER: Figure = Figure { head: 1.5, hips: 0.85, half_width: 0.3 };
static WARRIOR: LazyLock<Figure> = LazyLock::new(|| {
    let look = enemy::Look::warrior();
    Figure { head: look.crown - 0.1, hips: 1.0, half_width: look.framed_radius + 0.1 }
});

/// What each frame is checked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Fault {
    PlayerOut,
    EnemyOut,
    PlayerHidden,
    EnemyHidden,
    /// The enemy stands between the camera and the player and covers half of them or more.
    Covered,
    /// The enemy takes more than this much of the frame's height.
    Fills,
    /// Much of the frame is something standing in front of the fighters.
    Foreground,
    /// The near plane meets something.
    EyeTouches,
}

impl Fault {
    const ALL: [Self; 8] = [
        Self::PlayerOut,
        Self::EnemyOut,
        Self::PlayerHidden,
        Self::EnemyHidden,
        Self::Covered,
        Self::Fills,
        Self::Foreground,
        Self::EyeTouches,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::PlayerOut => "player out",
            Self::EnemyOut => "enemy out",
            Self::PlayerHidden => "player hidden",
            Self::EnemyHidden => "enemy hidden",
            Self::Covered => "enemy over player",
            Self::Fills => "enemy fills",
            Self::Foreground => "foreground",
            Self::EyeTouches => "eye touches",
        }
    }
}

/// Past this share of the frame's height, the enemy fills it.
const FILLS: f32 = 0.85;
/// Past this share of the frame, what's in front of the fighters is in the way. A capture of a stall each
/// side of a fight, a third of the frame between them, reads as a framed shot.
const FOREGROUND_FROM: f32 = 0.4;
/// The foreground's ray grid: across, then up, as shares of the screen's half-size.
const GRID_X: [f32; 5] = [-0.8, -0.4, 0.0, 0.4, 0.8];
const GRID_Y: [f32; 3] = [-0.5, 0.0, 0.5];

/// How far from the square's middle the visible town is read, in metres: past this nothing stands between
/// a camera in the square and a fighter.
const TOWN_REACH: f32 = 45.0;
/// How big a piece of it is one collider.
const TILE: f32 = 6.0;

/// What a player can see that a camera can't: the town's and the plantings' triangles, by the name of
/// each's material or plant, from a cook's pack.
struct VisibleTown {
    world: PhysicsWorld,
    names: Vec<String>,
    filter: QueryFilter,
}

impl VisibleTown {
    /// From `target/content/desktop`'s packs, or `None` with no cook to read.
    fn load() -> Option<Self> {
        let packs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/content/desktop");
        let bytes = std::fs::read(packs.join("shared.spack")).ok()?;
        let pack = PackReader::new(&bytes).expect("the cook's pack reads");
        let name = sapphire_zone::pack_name(TOWN_STEM);
        let zone_bytes = std::fs::read(packs.join(format!("{name}.spack"))).ok()?;
        let zone_pack = PackReader::new(&zone_bytes).expect("the cook's pack reads");
        let mut statics = Vec::new();
        let mut names = Vec::new();
        // In tiles: a world's trees have to stay shallow, and one mesh of a whole town's materials doesn't.
        let mut add = |name: String, vertices: &[Vec3], faces: &[[u32; 3]]| {
            let mut tiles: BTreeMap<(i32, i32), Vec<[Vec3; 3]>> = BTreeMap::new();
            for face in faces {
                let corners = face.map(|index| vertices[index as usize]);
                let middle = (corners[0] + corners[1] + corners[2]) / 3.0;
                if middle.x.abs().max(middle.z.abs()) < TOWN_REACH {
                    let tile = ((middle.x / TILE).floor() as i32, (middle.z / TILE).floor() as i32);
                    tiles.entry(tile).or_default().push(corners);
                }
            }
            for corners in tiles.into_values() {
                let id = ColliderId(names.len() as u32);
                names.push(name.clone());
                statics.push(StaticCollider {
                    id,
                    shape: StaticShape::Mesh {
                        vertices: corners.iter().flatten().copied().collect(),
                        indices: (0..corners.len() as u32)
                            .map(|face| [face * 3, face * 3 + 1, face * 3 + 2])
                            .collect(),
                    },
                    pose: Pose::at(Vec3::ZERO),
                    memberships: Group::World.mask(),
                });
            }
        };
        let zone = zone_pack.read(TOWN_ZONE).expect("the pack has the town");
        let zone = sapphire_zone::Zone::read(&zone).expect("the town reads");
        let presentation = &zone.presentation;
        let backdrop = presentation.backdrop.as_ref().into_iter();
        for contents in backdrop.chain(presentation.cells.iter().map(|cell| &cell.contents)) {
            let Some(&mesh) = contents.mesh.as_ref() else { continue };
            let mesh = zone_pack.read(mesh).expect("the pack has the cell");
            let mesh = Mesh::read(&mesh).expect("the cell reads");
            for (slot, vertices, faces) in triangles(mesh, &Mat4::IDENTITY) {
                add(slot, &vertices, &faces);
            }
        }
        for (plant, place) in plantings() {
            let mesh = pack.read(plant.id()).expect("the pack has the plant");
            let mesh = Mesh::read(&mesh).expect("the plant reads");
            let at = place.w_axis;
            for (_, vertices, faces) in triangles(mesh, &place) {
                add(format!("{plant:?} at ({:.0}, {:.0})", at.x, at.z), &vertices, &faces);
            }
        }
        let world = PhysicsWorld::new(statics, 0).expect("the town builds");
        Some(Self { world, names, filter: QueryFilter::new(Group::World.mask()) })
    }

    fn name_of(&self, hit: &RayHit) -> &str {
        match hit.target {
            HitTarget::Static(id) => &self.names[id.0 as usize],
            HitTarget::Body(_) => "a body",
        }
    }
}

/// Each submesh of a cooked mesh's first LOD, placed by `place`, with its material's name.
fn triangles(mesh: &ArchivedMesh, place: &Mat4) -> Vec<(String, Vec<Vec3>, Vec<[u32; 3]>)> {
    let streams = &mesh.streams;
    let vertices: Vec<Vec3> = (0..streams.len())
        .map(|index| place.transform_point3(Vec3::from(streams.position(index).expect("in range"))))
        .collect();
    let mut out = Vec::new();
    for submesh in mesh.lods[0].submeshes.iter() {
        let (first, count) =
            (submesh.first_index.to_native() as usize, submesh.index_count.to_native() as usize);
        let name = mesh.materials[submesh.material.to_native() as usize].name.to_string();
        let (corners, _) = mesh.indices[first..first + count].as_chunks::<3>();
        let faces: Vec<[u32; 3]> = corners
            .iter()
            .map(|face| [face[0].to_native(), face[1].to_native(), face[2].to_native()])
            .collect();
        out.push((name, vertices.clone(), faces));
    }
    out
}

/// The world the camera reads, with each collider's name.
struct Colliders {
    ground: ArenaGround,
    names: Vec<String>,
}

impl Colliders {
    fn load() -> Self {
        let cooked = cooked_colliders().expect("the document cooks");
        let set = ColliderSet::read(&cooked).expect("it reads");
        let names = set.colliders.iter().map(|collider| collider.name.to_string()).collect();
        Self { ground: ArenaGround::new(set).expect("the stage stands on it"), names }
    }

    fn name_of(&self, hit: &RayHit) -> &str {
        match hit.target {
            HitTarget::Static(id) => &self.names[id.0 as usize],
            HitTarget::Body(_) => "a body",
        }
    }
}

/// What the camera's own casts hit.
const CAMERA_SEES: GroupMask =
    GroupMask::from_bits(Group::World.mask().bits() | Group::CameraBlocker.mask().bits());

/// A fault running over consecutive frames, once it's over.
#[derive(Clone, Debug)]
struct Stretch {
    fault: Fault,
    fight: String,
    segment: u32,
    from_tick: u64,
    to_tick: u64,
    seconds: f32,
    what: String,
}

/// What one fight at one condition came to.
#[derive(Clone, Debug, Default)]
struct Tally {
    frames: u32,
    /// Frames with the soft lock on someone: the enemy's only framed then.
    targeted: u32,
    faulty: BTreeMap<Fault, u32>,
    /// Frames with the head or the hips, not both, behind something: colliders, then what's drawn.
    player_part: [u32; 2],
    enemy_part: [u32; 2],
    /// Frames with both behind something: colliders, then what's drawn.
    player_hidden: [u32; 2],
    enemy_hidden: [u32; 2],
    /// Frames with the near plane in or at something: colliders, then what's drawn.
    eye_touching: [u32; 2],
    /// What stood in the way, by name, in frames.
    blockers: BTreeMap<String, u32>,
    /// Off the screen altogether, head and hips: the player, the enemy.
    gone: [u32; 2],
    speeds: Vec<f32>,
    turns: Vec<f32>,
    accelerations: Vec<f32>,
    /// Radians the view's turned, all told, and over how long.
    swing: f32,
    seconds: f32,
    jumps: u32,
    stats: CameraStats,
    /// How many stretches lasted past [`WORTH_NAMING`].
    stretches: Vec<Stretch>,
}

fn percentile(values: &[f32], share: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    sorted[((sorted.len() - 1) as f32 * share).round() as usize]
}

/// Where `point` lands on a screen of `aspect`: −1 to 1 each way, and its depth along the view.
fn on_screen(pose: &CameraPose, aspect: f32, point: Vec3) -> (f32, f32, f32) {
    let offset = point - pose.eye;
    let depth = offset.dot(pose.rotation * Vec3::Z);
    let half = (pose.fov_y * 0.5).tan();
    let x = offset.dot(pose.rotation * Vec3::NEG_X) / (depth * half * aspect);
    let y = offset.dot(pose.rotation * Vec3::Y) / (depth * half);
    (x, y, depth)
}

/// How someone draws on a screen, as a box: `min` and `max` in screen shares, and how deep.
#[derive(Clone, Copy, Debug)]
struct Drawn {
    min: Vec2,
    max: Vec2,
    depth: f32,
}

impl Drawn {
    fn of(pose: &CameraPose, aspect: f32, feet: Vec3, figure: &Figure) -> Option<Self> {
        let (x0, y0, d0) = on_screen(pose, aspect, feet);
        let (x1, y1, d1) = on_screen(pose, aspect, feet + Vec3::Y * figure.head);
        if d0 <= 0.05 || d1 <= 0.05 {
            return None;
        }
        let half = (pose.fov_y * 0.5).tan();
        let wide = figure.half_width / (d0.min(d1) * half * aspect);
        Some(Self {
            min: Vec2::new(x0.min(x1) - wide, y0.min(y1)),
            max: Vec2::new(x0.max(x1) + wide, y0.max(y1)),
            depth: d0.min(d1),
        })
    }

    fn area(&self) -> f32 {
        let size = (self.max - self.min).max(Vec2::ZERO);
        size.x * size.y
    }

    /// How much of this box `over` covers.
    fn covered_by(&self, over: &Self) -> f32 {
        let size = (self.max.min(over.max) - self.min.max(over.min)).max(Vec2::ZERO);
        size.x * size.y / self.area().max(1e-6)
    }
}

/// Reads one frame into a [`Tally`].
struct Reader<'a> {
    colliders: &'a Colliders,
    town: Option<&'a VisibleTown>,
    aspect: f32,
    rate: u32,
    fight: String,
    segment: u32,
    tally: Tally,
    /// When each fault's running stretch began, and what's behind it.
    open: BTreeMap<Fault, (u64, String, f32)>,
    last: Option<(Vec3, Quat)>,
    last_speed: Option<Vec3>,
}

/// What's in the way of `point` from the eye, in a world.
fn blocker(world: &PhysicsWorld, filter: &QueryFilter, eye: Vec3, point: Vec3) -> Option<RayHit> {
    let along = point - eye;
    let reach = along.length() - 0.05;
    if reach <= 0.0 {
        return None;
    }
    world.cast_ray(eye, along, reach, filter)
}

impl<'a> Reader<'a> {
    fn new(
        colliders: &'a Colliders,
        town: Option<&'a VisibleTown>,
        condition: Condition,
        fight: &str,
    ) -> Self {
        Self {
            colliders,
            town,
            aspect: condition.aspect,
            rate: condition.rate,
            fight: fight.to_owned(),
            segment: 0,
            tally: Tally::default(),
            open: BTreeMap::new(),
            last: None,
            last_speed: None,
        }
    }

    /// A new fight: what's running ends, and the view's jump is the cut it is.
    fn start_segment(&mut self, tick: u64) {
        self.close_all(tick);
        self.segment += 1;
        self.last = None;
        self.last_speed = None;
    }

    fn close_all(&mut self, tick: u64) {
        let open = std::mem::take(&mut self.open);
        for (fault, (from, what, worst)) in open {
            self.close(fault, from, tick, format!("{what} {worst:.2}"));
        }
    }

    fn close(&mut self, fault: Fault, from: u64, to: u64, what: String) {
        let seconds = (to - from) as f32 / TICK_HZ as f32;
        if seconds >= WORTH_NAMING {
            self.tally.stretches.push(Stretch {
                fault,
                fight: self.fight.clone(),
                segment: self.segment,
                from_tick: from,
                to_tick: to,
                seconds,
                what,
            });
        }
    }

    /// Counts a frame with `fault` on or off. `size` says how bad, and a stretch names its worst.
    fn mark(&mut self, fault: Fault, on: bool, tick: u64, size: f32, what: impl FnOnce() -> String) {
        if on {
            *self.tally.faulty.entry(fault).or_default() += 1;
            let open = self.open.entry(fault).or_insert_with(|| (tick, what(), 0.0));
            open.2 = f32::max(open.2, size.min(9.99));
        } else if let Some((from, what, worst)) = self.open.remove(&fault) {
            self.close(fault, from, tick, format!("{what} {worst:.2}"));
        }
    }

    fn blocked_by(&mut self, name: &str) {
        *self.tally.blockers.entry(name.to_owned()).or_default() += 1;
    }

    /// How someone's rays came out: colliders, then what's drawn, as how many of head and hips are hidden,
    /// and a name for it.
    fn rays(&mut self, eye: Vec3, feet: Vec3, figure: &Figure) -> ([u32; 2], String) {
        let mut hidden = [0, 0];
        let mut what = String::new();
        let solid = QueryFilter::new(CAMERA_SEES);
        for height in [figure.head, figure.hips] {
            let point = feet + Vec3::Y * height;
            let solid_hit = blocker(&self.colliders.ground.world, &solid, eye, point);
            if let Some(hit) = &solid_hit {
                hidden[0] += 1;
                let name = self.colliders.name_of(hit).to_owned();
                self.blocked_by(&format!("collider {name}"));
                if what.is_empty() {
                    what = name;
                }
            }
            if let Some(town) = self.town
                && let Some(hit) = blocker(&town.world, &town.filter, eye, point)
            {
                hidden[1] += 1;
                let name = town.name_of(&hit).to_owned();
                // What the colliders leave out: drawn in the way, with no collider on the ray.
                if solid_hit.is_none() {
                    self.blocked_by(&format!("drawn only {name}"));
                    if what.is_empty() {
                        what = format!("drawn {name}");
                    }
                }
            }
        }
        (hidden, what)
    }

    /// How much of the frame stands in front of the nearer fighter: a share of its ray grid.
    fn foreground(&mut self, pose: &CameraPose, nearest: f32) -> (f32, String) {
        let Some(town) = self.town else { return (0.0, String::new()) };
        if nearest <= 0.5 {
            return (0.0, String::new());
        }
        let half = (pose.fov_y * 0.5).tan();
        let (forward, right, up) =
            (pose.rotation * Vec3::Z, pose.rotation * Vec3::NEG_X, pose.rotation * Vec3::Y);
        let mut blocked = 0;
        let mut what = String::new();
        for x in GRID_X {
            for y in GRID_Y {
                let along = forward + right * (x * half * self.aspect) + up * (y * half);
                let reach = (nearest - 0.25) * along.length();
                if let Some(hit) = town.world.cast_ray(pose.eye, along, reach, &town.filter)
                    && hit.point.y > 0.2
                {
                    blocked += 1;
                    if what.is_empty() {
                        what = town.name_of(&hit).to_owned();
                    }
                }
            }
        }
        (blocked as f32 / (GRID_X.len() * GRID_Y.len()) as f32, what)
    }

    /// Reads a frame's pose. `enemy` is who's framed, if anyone.
    fn frame(
        &mut self,
        tick: u64,
        pose: &CameraPose,
        player_feet: Vec3,
        enemy_feet: Vec3,
        framed: bool,
        stats: CameraStats,
    ) {
        self.tally.frames += 1;
        if framed {
            self.tally.targeted += 1;
        }
        let aspect = self.aspect;
        add_stats(&mut self.tally.stats, &stats);

        // In the frame.
        let score = |feet: Vec3, figure: &Figure| {
            let mut worst = 0.0_f32;
            let mut seen = false;
            for height in [figure.head, figure.hips] {
                let (x, y, depth) = on_screen(pose, aspect, feet + Vec3::Y * height);
                if depth <= 0.05 {
                    worst = f32::MAX;
                    continue;
                }
                let inside = x.abs().max(y.abs());
                worst = worst.max(inside);
                seen |= inside <= 1.0;
            }
            (worst, seen)
        };
        let (player_edge, player_seen) = score(player_feet, &PLAYER);
        let (enemy_edge, enemy_seen) = score(enemy_feet, &WARRIOR);
        self.tally.gone[0] += u32::from(!player_seen);
        self.tally.gone[1] += u32::from(framed && !enemy_seen);
        self.mark(Fault::PlayerOut, player_edge > MARGIN, tick, player_edge, || "reaches".to_owned());
        self.mark(Fault::EnemyOut, framed && enemy_edge > MARGIN, tick, enemy_edge, || "reaches".to_owned());

        // Behind something.
        let (player, what) = self.rays(pose.eye, player_feet, &PLAYER);
        for (index, hidden) in player.into_iter().enumerate() {
            self.tally.player_hidden[index] += u32::from(hidden == 2);
            self.tally.player_part[index] += u32::from(hidden == 1);
        }
        let hidden = player.contains(&2);
        self.mark(Fault::PlayerHidden, hidden, tick, 1.0, || what);
        if framed {
            let (enemy, what) = self.rays(pose.eye, enemy_feet, &WARRIOR);
            for (index, hidden) in enemy.into_iter().enumerate() {
                self.tally.enemy_hidden[index] += u32::from(hidden == 2);
                self.tally.enemy_part[index] += u32::from(hidden == 1);
            }
            self.mark(Fault::EnemyHidden, enemy.contains(&2), tick, 1.0, || what);
        } else {
            self.mark(Fault::EnemyHidden, false, tick, 0.0, String::new);
        }

        // One over the other, and how much of the frame the enemy takes.
        let drawn_player = Drawn::of(pose, aspect, player_feet, &PLAYER);
        let drawn_enemy = Drawn::of(pose, aspect, enemy_feet, &WARRIOR);
        let cover = match (drawn_player, drawn_enemy) {
            (Some(player), Some(enemy)) if enemy.depth < player.depth => player.covered_by(&enemy),
            _ => 0.0,
        };
        self.mark(Fault::Covered, framed && cover >= 0.5, tick, cover, || "covers".to_owned());
        let fill = drawn_enemy.map_or(0.0, |enemy| (enemy.max.y - enemy.min.y) * 0.5);
        self.mark(Fault::Fills, framed && fill > FILLS, tick, fill, || "of the height".to_owned());

        // In front of them both.
        let nearest = match (drawn_player, drawn_enemy, framed) {
            (Some(player), Some(enemy), true) => player.depth.min(enemy.depth),
            (Some(player), _, _) => player.depth,
            _ => 0.0,
        };
        let (front, what) = self.foreground(pose, nearest);
        self.mark(Fault::Foreground, front >= FOREGROUND_FROM, tick, front, || what);

        // The near plane.
        let near = Shape::Sphere { radius: NEAR_REACH };
        let mut out = [HitTarget::Static(ColliderId(0)); 1];
        let solid = QueryFilter::new(CAMERA_SEES);
        let at = Pose::at(pose.eye);
        let in_collider = self.colliders.ground.world.overlap(&near, at, &solid, &mut out).written > 0;
        let named = in_collider.then(|| match out[0] {
            HitTarget::Static(id) => self.colliders.names[id.0 as usize].clone(),
            HitTarget::Body(_) => "a body".to_owned(),
        });
        let mut drawn = None;
        if let Some(town) = self.town
            && town.world.overlap(&near, at, &town.filter, &mut out).written > 0
            && let HitTarget::Static(id) = out[0]
        {
            drawn = Some(town.names[id.0 as usize].clone());
        }
        self.tally.eye_touching[0] += u32::from(in_collider);
        self.tally.eye_touching[1] += u32::from(drawn.is_some());
        if let Some(name) = &drawn {
            let name = format!("eye at drawn {name}");
            self.blocked_by(&name);
        }
        self.mark(Fault::EyeTouches, in_collider || drawn.is_some(), tick, 1.0, || {
            named.or(drawn).unwrap_or_default()
        });

        // How it moved.
        let dt = 1.0 / self.rate as f32;
        let now = (pose.eye, pose.rotation);
        if let Some((eye, rotation)) = self.last {
            let moved = now.0 - eye;
            let speed = moved / dt;
            let change = now.1 * rotation.inverse();
            let change = if change.w < 0.0 { -change } else { change };
            let angle = Vec3::new(change.x, change.y, change.z).length() * 2.0;
            self.tally.speeds.push(speed.length());
            self.tally.turns.push(angle / dt);
            self.tally.swing += angle;
            if let Some(last) = self.last_speed {
                self.tally.accelerations.push((speed - last).length() / dt);
            }
            self.last_speed = Some(speed);
            if moved.length() > JUMP_METRES || angle > JUMP_RADIANS {
                self.tally.jumps += 1;
            }
        }
        self.last = Some(now);
        self.tally.seconds += dt;
    }

    fn finish(mut self, tick: u64) -> Tally {
        self.close_all(tick);
        self.tally
    }
}

fn add_stats(sum: &mut CameraStats, more: &CameraStats) {
    sum.frames += more.frames;
    sum.casts += more.casts;
    sum.thin_frames += more.thin_frames;
    sum.pulled_frames += more.pulled_frames;
    sum.contacts += more.contacts;
    sum.hops += more.hops;
    sum.cuts += more.cuts;
    sum.swings += more.swings;
}

/// A stop on a [`Tour`]: where to run to, and how long to fight there.
#[derive(Clone, Copy, Debug)]
struct Stop {
    at: [f32; 2],
    fight: u32,
}

const fn stop(x: f32, z: f32, fight: u32) -> Stop {
    Stop { at: [x, z], fight }
}

/// Someone wandering the square as a player does: running to a place, fighting there, and on. A
/// script can't: it reads a tick and nothing else, so it can't aim its stick.
#[derive(Clone, Copy, Debug)]
struct Tour {
    stops: &'static [Stop],
    /// Swings the view by hand, as a person's thumb does, on a schedule.
    looks: bool,
    /// Holds the lock-on from the first tick.
    locks: bool,
    /// Circles the enemy rather than running stop to stop, on this radius.
    circles: Option<f32>,
    /// Asks for the next party member now and then.
    swaps: bool,
}

/// Round the square's edge one way, by its corners, stalls and trees: what a fight's pushed into.
const ROUND_THE_EDGE: &[Stop] = &[
    stop(14.0, -13.0, 180),
    stop(14.8, -3.5, 120),
    stop(12.5, 3.3, 180),
    stop(15.0, 8.0, 90),
    stop(0.0, 18.0, 120),
    stop(-16.0, 18.0, 120),
    stop(-15.0, 6.0, 90),
    stop(-14.6, -8.0, 120),
    stop(-15.0, -13.5, 180),
    stop(-11.0, -15.6, 120),
    stop(-0.5, -16.5, 120),
    stop(7.0, -15.5, 120),
];

/// Through the square's clutter: in and behind the pavilion, behind each stall, and past the trees.
const THROUGH_THE_MIDDLE: &[Stop] = &[
    stop(-4.4, -7.0, 180),
    stop(-8.2, -9.5, 120),
    stop(-1.0, -4.0, 60),
    stop(6.6, -11.0, 150),
    stop(10.0, -13.0, 120),
    stop(14.0, -4.5, 150),
    stop(9.5, -1.0, 90),
    stop(11.0, 3.8, 150),
    stop(-1.5, 3.2, 120),
    stop(-3.0, 8.0, 90),
    stop(-9.2, 2.2, 150),
    stop(-7.8, -1.0, 90),
    stop(-10.8, -5.0, 120),
];

/// Every fight the report runs.
fn fights() -> Vec<Fight> {
    let tour = |stops, looks, locks| Tour { stops, looks, locks, circles: None, swaps: false };
    vec![
        Fight::replay("arena-fight-still"),
        Fight::replay("arena-fight-won").allowing(WANDERING),
        Fight::replay("party-swap").allowing(WANDERING),
        Fight::replay("arena-walk"),
        Fight::script("fight-back", FIGHT_SCENARIO, 1700).allowing(WANDERING),
        Fight::tour("edge", FIGHT_SCENARIO, 4800, 7, tour(ROUND_THE_EDGE, false, false)),
        Fight::tour("middle", FIGHT_SCENARIO, 4800, 7, tour(THROUGH_THE_MIDDLE, false, false)),
        Fight::tour("middle-seed", FIGHT_SCENARIO, 4800, 23, tour(THROUGH_THE_MIDDLE, false, false)),
        Fight::tour(
            "edge-party",
            PARTY_SCENARIO,
            4800,
            7,
            Tour { swaps: true, ..tour(ROUND_THE_EDGE, false, false) },
        ),
        Fight::tour("edge-looks", FIGHT_SCENARIO, 4800, 7, tour(ROUND_THE_EDGE, true, false))
            .allowing(UNDER_A_THUMB),
        Fight::tour("middle-locked", FIGHT_SCENARIO, 4800, 7, tour(THROUGH_THE_MIDDLE, false, true))
            .allowing(LOCKED_ON),
        Fight::tour(
            "circling",
            FIGHT_SCENARIO,
            3600,
            7,
            Tour { stops: &[], looks: false, locks: false, circles: Some(4.0), swaps: false },
        )
        .allowing(CIRCLING),
    ]
}

/// A fight to run.
struct Fight {
    name: &'static str,
    scenario: &'static str,
    ticks: u32,
    seed: u64,
    drive: Drive,
    /// What it may come to, in any condition.
    allowed: Bounds,
}

enum Drive {
    Replay,
    Script(&'static str),
    Tour(Tour),
}

impl Fight {
    fn replay(name: &'static str) -> Self {
        Self { name, scenario: "", ticks: 0, seed: 0, drive: Drive::Replay, allowed: ON_THE_CLEAR_FLOOR }
    }

    fn script(name: &'static str, scenario: &'static str, ticks: u32) -> Self {
        Self { name, scenario, ticks, seed: 7, drive: Drive::Script(name), allowed: ON_THE_CLEAR_FLOOR }
    }

    fn tour(name: &'static str, scenario: &'static str, ticks: u32, seed: u64, tour: Tour) -> Self {
        Self { name, scenario, ticks, seed, drive: Drive::Tour(tour), allowed: IN_THE_CLUTTER }
    }

    /// With what's asked of it, where that isn't what's asked of its kind.
    fn allowing(self, allowed: Bounds) -> Self {
        Self { allowed, ..self }
    }
}

/// How far from a stop counts as there, and how long without getting nearer before it's skipped.
const THERE: f32 = 1.2;
const STUCK_TICKS: u32 = 90;

/// Plays a [`Tour`]: reads where the player is, and says what a thumb would.
struct Walker {
    tour: Tour,
    stop: usize,
    /// Ticks left fighting at this stop.
    fighting: u32,
    fought: u64,
    best: f32,
    since_nearer: u32,
    side: f32,
}

impl Walker {
    fn new(tour: Tour) -> Self {
        Self { tour, stop: 0, fighting: 0, fought: 0, best: f32::MAX, since_nearer: 0, side: 1.0 }
    }

    fn actions(&mut self, tick: u64, sim: &Simulation, cast: &[ActorId]) -> TickActions {
        let mut actions = TickActions::NEUTRAL;
        let Some(&player) = cast.first() else { return actions };
        let Some(me) = sim.actor(sim.party().on_field(player)) else { return actions };
        let mut held = ActorActions::NEUTRAL;
        if self.tour.locks && tick == 1 {
            held.buttons = held.buttons | Buttons::LOCK_ON;
        }
        if self.tour.swaps && tick.is_multiple_of(420) && tick > 0 {
            held.buttons = held.buttons | Buttons::SWAP;
        }
        let at = Vec2::new(me.position.x, me.position.z);
        if let Some(radius) = self.tour.circles {
            if let Some(enemy) = cast.get(1).and_then(|enemy| sim.actor(*enemy)) {
                let to = Vec2::new(enemy.position.x, enemy.position.z) - at;
                let away = to.length() - radius;
                let around = Vec2::new(-to.y, to.x).normalize_or_zero() * self.side;
                let steer = (around + to.normalize_or_zero() * away.clamp(-1.0, 1.0)).normalize_or_zero();
                held.stick = stick_along(steer);
                if tick.is_multiple_of(480) {
                    self.side = -self.side;
                }
                held.buttons = held.buttons | fight_buttons(tick, cast);
            }
        } else if self.fighting > 0 {
            self.fighting -= 1;
            self.fought += 1;
            held.buttons = held.buttons | fight_buttons(self.fought, cast);
            if self.fighting == 0 {
                self.stop = (self.stop + 1) % self.tour.stops.len();
                self.best = f32::MAX;
                self.since_nearer = 0;
            }
        } else {
            let stop = self.tour.stops[self.stop];
            let to = Vec2::from(stop.at) - at;
            let left = to.length();
            if left < self.best - 0.02 {
                self.best = left;
                self.since_nearer = 0;
            } else {
                self.since_nearer += 1;
            }
            if left < THERE || self.since_nearer > STUCK_TICKS {
                self.fighting = stop.fight.max(1);
            } else {
                held.stick = stick_along(to / left);
            }
        }
        actions.set(player, held).expect("one actor fits a tick");
        actions
    }
}

fn stick_along(direction: Vec2) -> Stick {
    Stick::new((direction.x * 127.0) as i8, (direction.y * 127.0) as i8)
}

/// What the player presses while fighting: `fight-back`'s round.
fn fight_buttons(tick: u64, cast: &[ActorId]) -> Buttons {
    fight_back(tick, &cast[..1]).get(cast[0]).buttons
}

/// A thumb swinging the view: a second's push one way, every few seconds, in radians this frame.
fn thumb(tick: u64, dt: f32) -> Vec2 {
    let beat = tick % 360;
    let turn = if beat < 60 {
        1.6
    } else if (180..240).contains(&beat) {
        -1.6
    } else {
        0.0
    };
    Vec2::new(turn * dt, 0.0)
}

/// How long the app waits after someone falls before a new fight: `Combat`'s, 4 s.
const START_OVER_TICKS: u64 = 240;

/// Whatever steps a fight.
enum Stepper {
    Played(Box<Player>),
    Recorded { recording: Box<Recording>, script: Option<fn(u64, &[ActorId]) -> TickActions> },
}

impl Stepper {
    fn simulation(&self) -> &Simulation {
        match self {
            Self::Played(player) => player.simulation(),
            Self::Recorded { recording, .. } => recording.simulation(),
        }
    }

    fn cast(&self) -> Vec<ActorId> {
        match self {
            Self::Played(player) => player.cast().to_vec(),
            Self::Recorded { recording, .. } => recording.cast().to_vec(),
        }
    }

    /// One tick. `false` once a replay's over.
    fn step(&mut self, walker: Option<&mut Walker>) -> bool {
        match self {
            Self::Played(player) => player.step(),
            Self::Recorded { recording, script } => {
                let cast = recording.cast();
                let tick = recording.simulation().tick();
                let actions = match (script, walker) {
                    (Some(script), _) => script(tick, cast),
                    (None, Some(walker)) => walker.actions(tick, recording.simulation(), cast),
                    (None, None) => TickActions::NEUTRAL,
                };
                recording.step(&mut |_: u64, _: &[ActorId]| actions).expect("within a file's limit");
                true
            }
        }
    }

    /// Writes what it recorded, numbered after those before it, if asked to.
    fn save(self, name: &str, to: Option<&Path>, saved: &mut u32) {
        let (Some(dir), Self::Recorded { recording, .. }) = (to, self) else { return };
        let (replay, _) = recording.finish();
        let file = match *saved {
            0 => format!("{name}.sreplay"),
            part => format!("{name}.{part}.sreplay"),
        };
        std::fs::write(dir.join(file), replay.encode()).expect("the replay writes");
        *saved += 1;
    }
}

fn replays_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays")
}

impl Fight {
    /// The fight from its first tick.
    fn start(&self) -> Stepper {
        let recording = || {
            let scenario: &Scenario = find_scenario(&SCENARIOS, self.scenario).expect("listed");
            let mut config = scenario.config;
            config.seed = self.seed;
            Box::new(Recording::new(scenario, config).expect("it starts"))
        };
        match &self.drive {
            Drive::Replay => {
                let path = replays_dir().join(format!("{}.sreplay", self.name));
                let bytes = std::fs::read(&path).expect("a committed replay");
                let replay = Replay::decode(&bytes).expect("it decodes");
                Stepper::Played(Box::new(Player::new(replay, &SCENARIOS).expect("it starts")))
            }
            Drive::Script(name) => Stepper::Recorded {
                recording: recording(),
                script: Some(find_script(name).expect("listed").actions),
            },
            Drive::Tour(_) => Stepper::Recorded { recording: recording(), script: None },
        }
    }
}

/// Runs a whole fight, new fights and all, in one condition.
fn run_fight(
    fight: &Fight,
    condition: Condition,
    colliders: &Colliders,
    town: Option<&VisibleTown>,
    tuning: &CameraTuning,
    save_to: Option<&Path>,
) -> Tally {
    let mut stepper = fight.start();
    let mut cast = stepper.cast();
    let mut walker = match &fight.drive {
        Drive::Tour(tour) => Some(Walker::new(*tour)),
        _ => None,
    };
    let looks = matches!(&fight.drive, Drive::Tour(tour) if tour.looks);

    let mut world = World::new();
    let mut placed = || world.spawn(RenderTransform::default()).id();
    let (player, warrior, blade) = (placed(), placed(), placed());
    let grip = enemy::Grip { hand: 0, at: Mat4::IDENTITY };
    let sizes = enemy::Sizes::of(&enemy::Look::warrior());
    let parts = ArenaEnemy { warrior, blade: Some((blade, grip)), kind: 0, sizes };
    let ground = ArenaGround { world: colliders.ground.share(), spawns: spawns() };
    let mut actors = ArenaActors::new(player, parts, ArenaShot::Director, ground);
    world.insert_resource(ArenaShot::Director.camera(spawns().player.position()));
    let director = actors.director().expect("this shot has one");
    director.set_tuning(tuning.clone()).expect("the tuning under test is valid");

    let steps_a_frame = TICK_HZ / condition.rate;
    let dt = 1.0 / condition.rate as f32;
    let mut reader = Reader::new(colliders, town, condition, fight.name);
    let mut played = 0_u64;
    let budget = if fight.ticks == 0 { u64::MAX } else { u64::from(fight.ticks) };
    let mut fallen_for = 0_u64;
    let mut saved = 0;

    // Its first frame, as the app's: before anything's stepped.
    actors.drive_camera(dt, Vec2::ZERO, 0.0);
    actors.present(&mut world, 1.0, Some(condition.aspect));
    actors.director().expect("this shot has one").take_stats();

    'fight: loop {
        for _ in 0..steps_a_frame {
            if played >= budget || !stepper.step(walker.as_mut()) {
                break 'fight;
            }
            played += 1;
            actors.follow(stepper.simulation(), &cast);
        }
        let tick = stepper.simulation().tick();
        let look = if looks { thumb(played, dt) } else { Vec2::ZERO };
        actors.drive_camera(dt, look, 0.0);
        actors.present(&mut world, 1.0, Some(condition.aspect));

        let framed = actors.framed().is_some();
        let (player_feet, enemy_feet) =
            (actors.drawn(0, 1.0).w_axis.truncate(), actors.drawn(1, 1.0).w_axis.truncate());
        let director = actors.director().expect("this shot has one");
        let (pose, stats) = (director.pose(), director.take_stats());
        reader.frame(tick, &pose, player_feet, enemy_feet, framed, stats);

        // Someone fell: as the app does, a new fight once it's been seen, going on with the same tour.
        let sim = stepper.simulation();
        let down = cast
            .iter()
            .any(|id| sim.actor(sim.party().on_field(*id)).is_some_and(|actor| actor.fighter.is_defeated()));
        fallen_for = if down { fallen_for + u64::from(steps_a_frame) } else { 0 };
        if fallen_for >= START_OVER_TICKS && !matches!(fight.drive, Drive::Replay) {
            fallen_for = 0;
            reader.start_segment(tick);
            std::mem::replace(&mut stepper, fight.start()).save(fight.name, save_to, &mut saved);
            cast = stepper.cast();
        }
    }
    let tick = stepper.simulation().tick();
    stepper.save(fight.name, save_to, &mut saved);
    reader.finish(tick)
}

fn share(count: u32, of: u32) -> f32 {
    if of == 0 { 0.0 } else { count as f32 * 100.0 / of as f32 }
}

/// One line of the table: what a fight came to.
fn table_row(label: &str, tally: &Tally) {
    let faults = |fault: Fault| share(tally.faulty.get(&fault).copied().unwrap_or(0), tally.frames);
    let enemy_out = share(tally.faulty.get(&Fault::EnemyOut).copied().unwrap_or(0), tally.targeted);
    println!(
        "{label:<26} {frames:>6} {targeted:>5.1} | {pout:>5.2} {eout:>5.2} {pgone:>5.2} {egone:>5.2} | {phc:>5.2} {phd:>5.2} {ppd:>5.2} {ehc:>5.2} {ehd:>5.2} {epd:>5.2} | {cover:>5.2} {fills:>5.2} {fore:>5.2} | {eyec:>5.2} {eyed:>5.2} | {sp50:>5.1} {sp99:>5.1} {spm:>5.1} {tu50:>5.2} {tu99:>5.2} {tum:>5.2} {ac99:>5.0} {swing:>5.1} {chose:>4} {jumps:>3} {hops:>3}",
        frames = tally.frames,
        targeted = share(tally.targeted, tally.frames),
        pout = faults(Fault::PlayerOut),
        eout = enemy_out,
        pgone = share(tally.gone[0], tally.frames),
        egone = share(tally.gone[1], tally.targeted),
        phc = share(tally.player_hidden[0], tally.frames),
        phd = share(tally.player_hidden[1], tally.frames),
        ppd = share(tally.player_part[1], tally.frames),
        ehc = share(tally.enemy_hidden[0], tally.targeted),
        ehd = share(tally.enemy_hidden[1], tally.targeted),
        epd = share(tally.enemy_part[1], tally.targeted),
        cover = share(tally.faulty.get(&Fault::Covered).copied().unwrap_or(0), tally.targeted),
        fills = share(tally.faulty.get(&Fault::Fills).copied().unwrap_or(0), tally.targeted),
        fore = faults(Fault::Foreground),
        eyec = share(tally.eye_touching[0], tally.frames),
        eyed = share(tally.eye_touching[1], tally.frames),
        sp50 = percentile(&tally.speeds, 0.5),
        sp99 = percentile(&tally.speeds, 0.99),
        spm = percentile(&tally.speeds, 1.0),
        tu50 = percentile(&tally.turns, 0.5),
        tu99 = percentile(&tally.turns, 0.99),
        tum = percentile(&tally.turns, 1.0),
        ac99 = percentile(&tally.accelerations, 0.99),
        swing = tally.swing / tally.seconds.max(1e-3),
        chose = tally.stats.swings,
        jumps = tally.jumps,
        hops = tally.stats.hops,
    );
}

fn table_header() {
    println!(
        "{:<26} {:>6} {:>5} | {:>5} {:>5} {:>5} {:>5} | {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} | {:>5} {:>5} {:>5} | {:>5} {:>5} | {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>4} {:>3} {:>3}",
        "fight",
        "frames",
        "targ%",
        "pOut",
        "eOut",
        "pGone",
        "eGone",
        "pHidC",
        "pHidD",
        "pPrtD",
        "eHidC",
        "eHidD",
        "ePrtD",
        "cover",
        "fills",
        "fore",
        "eyeC",
        "eyeD",
        "v50",
        "v99",
        "vMax",
        "w50",
        "w99",
        "wMax",
        "a99",
        "swing",
        "chose",
        "jmp",
        "hop"
    );
}

fn merge(sum: &mut Tally, more: Tally) {
    sum.frames += more.frames;
    sum.targeted += more.targeted;
    for (fault, count) in more.faulty {
        *sum.faulty.entry(fault).or_default() += count;
    }
    for index in 0..2 {
        sum.player_part[index] += more.player_part[index];
        sum.enemy_part[index] += more.enemy_part[index];
        sum.player_hidden[index] += more.player_hidden[index];
        sum.enemy_hidden[index] += more.enemy_hidden[index];
        sum.eye_touching[index] += more.eye_touching[index];
        sum.gone[index] += more.gone[index];
    }
    for (name, count) in more.blockers {
        *sum.blockers.entry(name).or_default() += count;
    }
    sum.speeds.extend(more.speeds);
    sum.turns.extend(more.turns);
    sum.accelerations.extend(more.accelerations);
    sum.swing += more.swing;
    sum.seconds += more.seconds;
    sum.jumps += more.jumps;
    add_stats(&mut sum.stats, &more.stats);
    sum.stretches.extend(more.stretches);
}

/// What a run may come to, as shares of frames in percent, on colliders alone. Each sits over the camera's
/// numbers by a margin, since a fight's chaotic to a nudge and x86-64 plays a different one, and the whole
/// measurement's sit under what the director did before M2 G5's framing pass where that pass moved it. A
/// fight's sit about a third over the worst of 56 seeds in all four conditions, a little over nothing where
/// nothing went wrong, and further where fresh seeds passed that, until 16 more in a row didn't. Raising one
/// means the framing got worse, or x86-64's fight is one more seed to allow for: say which.
#[derive(Clone, Copy, Debug)]
struct Bounds {
    player_out: f32,
    enemy_out: f32,
    player_gone: f32,
    enemy_gone: f32,
    player_hidden: f32,
    enemy_hidden: f32,
    covered: f32,
    fills: f32,
    /// The eye's speed, in metres a second, at the 99th percentile.
    speed: f32,
    /// The view's turn, in radians a second, at the 99th percentile.
    turn: f32,
    /// Radians the view's turned, all told, a second.
    swing: f32,
    /// Frames that read as a cut, a thousand seconds. At 30 Hz a fast swing counts, 0.75 m a frame being
    /// only 22 m/s.
    jumps: f32,
    /// Times the eye hopped.
    hops: f32,
    /// How long each fault may run without a break, in seconds: a share spread out is a nuisance, and the
    /// same share all at once is a fighter lost.
    player_out_for: f32,
    enemy_out_for: f32,
    player_hidden_for: f32,
    enemy_hidden_for: f32,
    covered_for: f32,
    fills_for: f32,
}

const BOUNDS: Bounds = Bounds {
    player_out: 7.0,
    enemy_out: 10.0,
    player_gone: 4.0,
    enemy_gone: 9.0,
    player_hidden: 6.0,
    enemy_hidden: 8.0,
    covered: 5.0,
    fills: 4.5,
    speed: 22.0,
    turn: 3.2,
    swing: 0.5,
    jumps: 150.0,
    hops: 12.0,
    // As long as any fight's may run.
    player_out_for: 9.0,
    enemy_out_for: 5.0,
    player_hidden_for: 3.6,
    enemy_hidden_for: 11.5,
    covered_for: 5.2,
    fills_for: 5.2,
};

/// A fight that stays where it starts, on the clear floor right of the pavilion: there's little in the way,
/// and a camera has no excuse.
const ON_THE_CLEAR_FLOOR: Bounds = Bounds {
    player_out: 2.0,
    enemy_out: 2.0,
    player_gone: 1.0,
    enemy_gone: 1.0,
    player_hidden: 5.5,
    enemy_hidden: 3.5,
    covered: 2.0,
    fills: 2.0,
    speed: 14.0,
    turn: 2.6,
    swing: 0.25,
    jumps: 200.0,
    hops: 1.0,
    player_out_for: 0.5,
    enemy_out_for: 0.5,
    player_hidden_for: 2.1,
    enemy_hidden_for: 1.3,
    covered_for: 0.8,
    fills_for: 0.5,
};

/// Scripted fights: nothing much goes wrong as they're played here, but their dash attacks and swaps carry them
/// off the clear floor, on some seeds past the pavilion's columns.
const WANDERING: Bounds = Bounds {
    player_out: 4.0,
    player_hidden: 13.0,
    enemy_hidden: 16.0,
    player_gone: 2.0,
    enemy_gone: 2.0,
    covered: 5.5,
    fills: 3.0,
    speed: 19.0,
    turn: 4.5,
    swing: 0.35,
    hops: 2.0,
    player_out_for: 1.5,
    enemy_out_for: 1.5,
    player_hidden_for: 2.3,
    enemy_hidden_for: 4.2,
    covered_for: 3.2,
    fills_for: 0.9,
    ..ON_THE_CLEAR_FLOOR
};

/// Circling round the enemy on the clear floor: the enemy passes in front, and on some seeds they stray.
const CIRCLING: Bounds = Bounds {
    enemy_hidden: 9.0,
    covered: 15.0,
    speed: 16.0,
    swing: 0.45,
    player_out_for: 1.5,
    enemy_out_for: 1.5,
    enemy_hidden_for: 3.2,
    covered_for: 4.0,
    ..ON_THE_CLEAR_FLOOR
};

/// The tours: running a fight round the square's edge and through its clutter, where the framing's whole job
/// is, and it can't always be done.
const IN_THE_CLUTTER: Bounds = Bounds {
    player_out: 17.0,
    enemy_out: 15.0,
    player_gone: 12.0,
    enemy_gone: 13.0,
    player_hidden: 16.0,
    enemy_hidden: 23.0,
    covered: 15.0,
    fills: 10.5,
    speed: 31.0,
    turn: 9.0,
    swing: 0.7,
    jumps: 540.0,
    hops: 3.0,
    player_out_for: 6.5,
    enemy_out_for: 4.5,
    player_hidden_for: 3.6,
    enemy_hidden_for: 11.5,
    covered_for: 5.0,
    fills_for: 4.5,
};

/// Lock-on holds its line through what's in the way, with only a lean to get round it.
const LOCKED_ON: Bounds = Bounds {
    player_out: 18.5,
    player_gone: 14.5,
    player_hidden: 18.5,
    enemy_hidden: 32.0,
    fills: 11.5,
    swing: 0.9,
    covered_for: 5.2,
    fills_for: 5.2,
    ..IN_THE_CLUTTER
};

/// A thumb swinging the view on a schedule, past what the rigs would frame: much of what goes wrong is its
/// doing, and clearing keeps its hands off for `after_look`.
const UNDER_A_THUMB: Bounds = Bounds {
    player_out: 27.0,
    enemy_out: 26.0,
    player_gone: 22.0,
    enemy_gone: 23.0,
    enemy_hidden: 30.0,
    covered: 18.5,
    fills: 22.0,
    turn: 13.5,
    swing: 1.45,
    hops: 6.0,
    player_out_for: 9.0,
    enemy_out_for: 5.0,
    ..IN_THE_CLUTTER
};

impl Tally {
    /// How long `fault` ran without a break at most, in seconds, if that's long enough to be named.
    fn longest(&self, fault: Fault) -> f32 {
        let stretches = self.stretches.iter().filter(|stretch| stretch.fault == fault);
        stretches.map(|stretch| stretch.seconds).fold(0.0, f32::max)
    }

    /// Whatever's past `bounds`, each saying what and by how much.
    fn past(&self, bounds: &Bounds) -> Vec<String> {
        let faults = |fault: Fault, of: u32| share(self.faulty.get(&fault).copied().unwrap_or(0), of);
        let (frames, targeted) = (self.frames, self.targeted);
        let seconds = self.seconds.max(1e-3);
        let rows = [
            ("player out", faults(Fault::PlayerOut, frames), bounds.player_out),
            ("enemy out", faults(Fault::EnemyOut, targeted), bounds.enemy_out),
            ("player gone", share(self.gone[0], frames), bounds.player_gone),
            ("enemy gone", share(self.gone[1], targeted), bounds.enemy_gone),
            ("player hidden", share(self.player_hidden[0], frames), bounds.player_hidden),
            ("enemy hidden", share(self.enemy_hidden[0], targeted), bounds.enemy_hidden),
            ("enemy over player", faults(Fault::Covered, targeted), bounds.covered),
            ("enemy fills", faults(Fault::Fills, targeted), bounds.fills),
            ("eye speed", percentile(&self.speeds, 0.99), bounds.speed),
            ("view turn", percentile(&self.turns, 0.99), bounds.turn),
            ("swing", self.swing / seconds, bounds.swing),
            ("jumps", self.jumps as f32 * 1000.0 / seconds, bounds.jumps),
            ("hops", self.stats.hops as f32, bounds.hops),
            ("player out for", self.longest(Fault::PlayerOut), bounds.player_out_for),
            ("enemy out for", self.longest(Fault::EnemyOut), bounds.enemy_out_for),
            ("player hidden for", self.longest(Fault::PlayerHidden), bounds.player_hidden_for),
            ("enemy hidden for", self.longest(Fault::EnemyHidden), bounds.enemy_hidden_for),
            ("enemy over player for", self.longest(Fault::Covered), bounds.covered_for),
            ("enemy fills for", self.longest(Fault::Fills), bounds.fills_for),
            // Never, in anything.
            ("the eye in a collider", self.eye_touching[0] as f32, 0.0),
        ];
        rows.into_iter()
            .filter(|(_, value, bound)| value > bound)
            .map(|(name, value, bound)| format!("{name} is {value:.2}, over {bound}"))
            .collect()
    }
}

/// Pillar 5, held: every fight in every condition stays within what's asked of it, its eye is never in a
/// collider, and all of them together stay within [`BOUNDS`]. `framing_report` says what moved, if this fails.
#[test]
fn framing_stays_within_its_bounds() {
    let colliders = Colliders::load();
    let tuning = CameraTuning::default_tuning();
    let mut past = Vec::new();
    for condition in CONDITIONS {
        let label = condition.label;
        let mut sum = Tally::default();
        for fight in fights() {
            let tally = run_fight(&fight, condition, &colliders, None, &tuning, None);
            let name = fight.name;
            past.extend(tally.past(&fight.allowed).into_iter().map(|row| format!("{label}, {name}: {row}")));
            merge(&mut sum, tally);
        }
        table_header();
        table_row(label, &sum);
        past.extend(sum.past(&BOUNDS).into_iter().map(|row| format!("{label}, every fight: {row}")));
    }
    assert!(past.is_empty(), "{}", past.join("\n"));
}

/// A tally of a fight of `frames` frames at 60 Hz, all of them framing someone, with nothing wrong yet.
fn tally_of(frames: u32) -> Tally {
    Tally { frames, targeted: frames, seconds: frames as f32 / 60.0, ..Tally::default() }
}

/// With `count` of its frames, in one stretch, showing `fault`.
fn with_fault(mut tally: Tally, fault: Fault, count: u32) -> Tally {
    tally.faulty.insert(fault, count);
    tally.stretches.push(Stretch {
        fault,
        fight: "short".to_owned(),
        segment: 0,
        from_tick: 0,
        to_tick: u64::from(count),
        seconds: count as f32 / 60.0,
        what: String::new(),
    });
    tally
}

/// A short fight entirely wrong, in a measurement mostly right: however little it weighs in all of them, its
/// own bounds say so, by name.
#[test]
fn one_fight_gone_wrong_shows_however_well_the_rest_go() {
    let short = with_fault(tally_of(300), Fault::PlayerOut, 300);
    let mut sum = tally_of(43_500);
    merge(&mut sum, short.clone());
    assert!(sum.past(&BOUNDS).is_empty(), "what all of them can't see: {:?}", sum.past(&BOUNDS));

    let past = short.past(&ON_THE_CLEAR_FLOOR);
    assert_eq!(past.len(), 2, "{past:?}");
    assert!(past[0].starts_with("player out is 100.00"), "{past:?}");
    assert!(past[1].starts_with("player out for is 5.00"), "{past:?}");
}

/// The same share, spread out or all at once: only one of them is a fighter lost.
#[test]
fn a_fault_all_at_once_is_past_a_share_that_would_let_it_by() {
    let hidden = 720;
    let spread = Tally { enemy_hidden: [hidden, 0], ..tally_of(4800) };
    assert!(spread.past(&IN_THE_CLUTTER).is_empty(), "{:?}", spread.past(&IN_THE_CLUTTER));

    let lost = with_fault(spread, Fault::EnemyHidden, hidden);
    assert_eq!(lost.past(&IN_THE_CLUTTER), ["enemy hidden for is 12.00, over 11.5"]);
}

/// Every number a fight can come to reaches what's checked: a row that never fails holds nothing.
#[test]
fn every_bound_can_be_passed() {
    let mut tally = tally_of(1000);
    tally.gone = [1000, 1000];
    tally.player_hidden = [1000, 0];
    tally.enemy_hidden = [1000, 0];
    tally.eye_touching = [1, 0];
    tally.speeds = vec![1000.0; 2];
    tally.turns = vec![1000.0; 2];
    tally.swing = 1e6;
    tally.jumps = 1000;
    tally.stats.hops = 1000;
    for fault in Fault::ALL {
        tally = with_fault(tally, fault, 1000);
    }
    let past = tally.past(&UNDER_A_THUMB);
    assert_eq!(past.len(), 20, "{past:?}");
}

/// The whole measurement: every fight in every condition, a table per condition, what stood in the way,
/// and the longest stretches of each fault.
#[test]
#[ignore = "the whole measurement, a few seconds; read it with --nocapture"]
fn framing_report() {
    let colliders = Colliders::load();
    let town = VisibleTown::load();
    if town.is_none() {
        println!("no cook in target/content/desktop: read against the colliders alone");
    }
    let tuning =
        std::env::var_os("SAPPHIRE_FRAMING_TUNING").map_or_else(CameraTuning::default_tuning, |path| {
            let text = std::fs::read_to_string(path).expect("the tuning to try");
            CameraTuning::from_ron(&text).expect("it's valid")
        });
    let brief = std::env::var_os("SAPPHIRE_FRAMING_BRIEF").is_some();
    let save_to = std::env::var_os("SAPPHIRE_FRAMING_REPLAYS").map(PathBuf::from);
    if let Some(dir) = &save_to {
        std::fs::create_dir_all(dir).expect("the replays' folder");
    }
    for (index, condition) in CONDITIONS.iter().enumerate() {
        println!("\n=== {} ===", condition.label);
        table_header();
        let mut sum = Tally::default();
        for fight in fights() {
            let save = if index == 0 { save_to.as_deref() } else { None };
            let tally = run_fight(&fight, *condition, &colliders, town.as_ref(), &tuning, save);
            if !brief {
                table_row(fight.name, &tally);
            }
            merge(&mut sum, tally);
        }
        table_row("ALL", &sum);
        println!(
            "seconds of fault, with stretches past {WORTH_NAMING} s: {}",
            Fault::ALL
                .map(|fault| {
                    let frames = sum.faulty.get(&fault).copied().unwrap_or(0);
                    let stretches = sum.stretches.iter().filter(|stretch| stretch.fault == fault).count();
                    format!("{} {:.1} s ({stretches})", fault.name(), frames as f32 / condition.rate as f32)
                })
                .join(", ")
        );
        if (index == 0 || index == 1) && !brief {
            let mut blockers: Vec<_> = sum.blockers.iter().collect();
            blockers.sort_by_key(|(_, frames)| std::cmp::Reverse(**frames));
            println!("what stood in the way, in ray-frames:");
            for (name, frames) in blockers.iter().take(40) {
                println!("  {frames:>7} {name}");
            }
            for fault in Fault::ALL {
                let mut stretches: Vec<&Stretch> =
                    sum.stretches.iter().filter(|stretch| stretch.fault == fault).collect();
                stretches.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
                println!("longest {}:", fault.name());
                for stretch in stretches.iter().take(6) {
                    println!(
                        "  {:>5.2} s {} part {} ticks {}..{}: {}",
                        stretch.seconds,
                        stretch.fight,
                        stretch.segment,
                        stretch.from_tick,
                        stretch.to_tick,
                        stretch.what,
                    );
                }
            }
        }
    }
}
