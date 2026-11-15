//! A zone's ambient characters (M5 C10): the people standing at its posts and walking its walks
//! (`sapphire_zone::ambient`), as presentation alone.
//!
//! - **Where they come from.** The zone's `.sambient` is its own sub-asset (`zone.sub_asset("ambient")`), which
//!   the zone's pack holds with the cast's meshes and the graph. Once the zone is on stage the crowd loads it in
//!   the background, then the cast's meshes (on the renderer, with a copy kept for their skins), their
//!   skeletons, the graph and its clips, so no frame waits on any of it; a capture or a bench waits for all of
//!   it once, before its first frame ([`Scene::finish_ambient`]). A zone without the section has no crowd, and
//!   nothing here costs it anything but a lookup as it opens.
//! - **Who's there.** Each person belongs to the cell of the zone's grid their post stands in, or the middle
//!   of their walk lies in. On every frame the cells within the tier's reach of the focus the zone streams
//!   from are in ([`AmbientCap`]), and of their people the tier's count are drawn: by the ring of cells they're
//!   in round the focus's own, then by a rank the crowd's seed gives each person. Nothing is remembered from
//!   one frame to the next, so the same focus at the same tick draws the same people, whatever frames came
//!   before, at any frame rate; and who comes and goes does so with their cell.
//! - **Nothing the simulation reads.** A person is an entity with a mesh, a transform, a pose, a figure and
//!   a light rig, as a fighter is to the renderer, and an [`AmbientPerson`]: no actor, collider, nav agent or
//!   draw from the simulation's random numbers. The renderer gives them a crowd's shadows
//!   ([`ShadowRole::Ambient`](sapphire_render::ShadowRole)).
//! - **Posing.** One graph for all, on the world clock, through the tier's animation levels and the frame's
//!   jobs (M5 C1), never among a fight's characters that are never thinned. A walker's place and facing are
//!   functions of the tick: it walks its path at its pace, from a start its place on the walk gives. Its phase
//!   is the distance it has walked over the walk clip's stride: its first pose starts, cut, as far back as the
//!   distance since its stride last began, and the walk plays at a rate its speed sets, so a phase is a
//!   distance along the path (what C11 can bucket by). A post's person stands and breathes on a clock its rank
//!   shifts, and plays its gestures, the graph's moves, on a schedule that's a function of the tick. No one
//!   holds spring bones (`GraphDriver::new` without them, `springs_run` false), at any distance (M5 C11), and
//!   each draws and skins at the level its error gives ([`CharacterLevels`](super::CharacterLevels), M5 D5).
//! - **Making way.** A walker steps aside, to its right, from the character and from the other people ahead of
//!   it within a couple of metres, by how close they are, without slowing, so its phase stays its distance. It
//!   doesn't see walls, fighters or anyone the scene doesn't draw, and nobody turns to look: the graph has no
//!   look-at, and the cast no IK.

use std::sync::Arc;
use std::time::Duration;

use sapphire_anim::{
    AnimGraph, AnimStats, CharacterPose, Clip, ClipSource, CombatInput, GraphBinding, GraphInput,
    GraphPlayer, LocomotionInput, Skeleton, evaluate_all, reuse_list,
};
use sapphire_assets::graph::StateMotion;
use sapphire_assets::mesh::Mesh;
use sapphire_assets::{AssetId, Handle, LoadPriority, LoadState};
use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::{Affine3A, Mat4, Vec3};
use sapphire_render::quality::TierId;
use sapphire_render::{MeshHandle, Renderer};
use sapphire_rhi::GpuContext;
use sapphire_time::{Stopwatch, TICK_HZ};
use sapphire_world::light_rig::{CharacterLightRig, LightRigId};
use sapphire_zone::{AMBIENT_PART, Ambient, ArchivedAmbient};

use super::{Figure, LOG_TARGET, Scene};
use crate::anim_graph::{ClipTable, GraphDriver, GraphEvaluation};
use crate::anim_levels::{AnimLevels, AnimPace, LevelClock, LevelView, anim_levels};
use crate::content::Content;
use crate::extract::{RenderMesh, RenderPose, RenderTransform};
use crate::lookdev::FigureJoints;

/// How many people a frame poses before it hands them to jobs, as a fight's frame does (M5 C1).
const JOBS_FROM: usize = 12;

/// How far a walker on an open walk keeps to the right of its line, in metres: there along one side and
/// back along the other, turning round at each end.
const KEEP_RIGHT: f32 = 0.45;

/// How far either way of a walker's place its facing looks along its path, in metres: what rounds a corner.
const TURN_REACH: f32 = 0.6;

/// The most a walker steps aside, and from how near and how far ahead it starts to, in metres.
const STEP_ASIDE: f32 = 0.8;
const ASIDE_NEAR: f32 = 1.0;
const ASIDE_FAR: f32 = 2.5;

/// How many people a tier draws, and how far out from the focus their cells may lie (see the [module
/// docs](self)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmbientCap {
    /// The most people drawn.
    pub count: u16,
    /// How near the focus, in metres, a cell must come to be in.
    pub reach: f32,
}

impl AmbientCap {
    pub const DESKTOP_HIGH: Self = Self { count: 60, reach: 72.0 };
    pub const DESKTOP_LOW: Self = Self { count: 60, reach: 60.0 };
    pub const MOBILE_HIGH: Self = Self { count: 30, reach: 48.0 };
    pub const MOBILE_LOW: Self = Self { count: 30, reach: 40.0 };
}

/// How many ambient characters a tier draws, and how far out (M5 C10).
#[must_use]
pub fn ambient_cap(tier: TierId) -> AmbientCap {
    match tier {
        TierId::DesktopHigh => AmbientCap::DESKTOP_HIGH,
        TierId::DesktopLow => AmbientCap::DESKTOP_LOW,
        TierId::MobileHigh => AmbientCap::MOBILE_HIGH,
        TierId::MobileLow => AmbientCap::MOBILE_LOW,
    }
}

/// What a frame's crowd came to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AmbientStats {
    /// People the zone on stage holds, drawn or not, once its crowd has loaded.
    pub people: u32,
    /// People standing in the scene: within reach and the tier's count.
    pub spawned: u32,
    /// Of those, the ones a view or its shadow shows, by the animation levels' test.
    pub in_view: u32,
    /// Of those in view, the ones in the animation levels' middle band and in their far band and past
    /// (M5 C11): the rest are in the nearest.
    pub middle_band: u32,
    pub far_band: u32,
    /// Standing people holding spring bones' state: none ever do, as the [module docs](self) say (M5
    /// C11), and the bench's count row says so.
    pub springs: u32,
    /// What deciding who's there took, with the spawning and despawning: a cell's activation.
    pub activation: Duration,
}

/// One of a zone's ambient characters, by its place among the zone's people: posts first, then each walk's
/// walkers. What the renderer's crowd shadows and a frame's posing find them by.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmbientPerson {
    pub index: u32,
    /// Whether it has been posed since it appeared.
    posed: bool,
}

/// What a frame poses a person with.
type People = QueryState<(
    Entity,
    &'static mut AmbientPerson,
    &'static mut GraphDriver,
    &'static mut RenderPose,
    &'static mut Figure,
    &'static mut CharacterLightRig,
    &'static mut RenderTransform,
)>;

/// One person.
#[derive(Clone, Copy, Debug)]
struct Person {
    member: u16,
    /// The seed's pick for them: lower draws first.
    rank: u64,
    /// The cell they belong to.
    cell: [i32; 2],
    /// Ticks their clock runs ahead of the world's: what keeps a crowd from breathing together.
    clock: f64,
    what: What,
}

#[derive(Clone, Copy, Debug)]
enum What {
    Post {
        at: Vec3,
        yaw: f32,
        post: u16,
    },
    /// On walk `walk`, `start` metres along its path at tick 0.
    Walk {
        walk: u16,
        start: f64,
    },
}

/// A walk's path: a closed loop, an open walk's there and back.
#[derive(Clone, Debug)]
struct Path {
    points: Vec<Vec3>,
    /// From the first point to each, along the path.
    along: Vec<f32>,
    pace: f32,
}

impl Path {
    fn new(points: &[Vec3], closed: bool, pace: f32) -> Self {
        let mut round: Vec<Vec3> = if closed {
            points.to_vec()
        } else {
            let back: Vec<Vec3> = points.iter().rev().copied().collect();
            keep_right(points).into_iter().chain(keep_right(&back)).collect()
        };
        round.push(round[0]);
        let mut along = Vec::with_capacity(round.len());
        let mut walked = 0.0;
        for (at, point) in round.iter().enumerate() {
            if at > 0 {
                walked += point.distance(round[at - 1]);
            }
            along.push(walked);
        }
        Self { points: round, along, pace }
    }

    fn length(&self) -> f32 {
        self.along.last().copied().unwrap_or(0.0)
    }

    /// Where `s` metres along the path is, round and round.
    fn at(&self, s: f64) -> Vec3 {
        let length = f64::from(self.length());
        let s = if length > 0.0 { s.rem_euclid(length) as f32 } else { 0.0 };
        let leg = self.along.partition_point(|&along| along <= s).clamp(1, self.points.len() - 1);
        let (from, to) = (self.along[leg - 1], self.along[leg]);
        let t = if to > from { (s - from) / (to - from) } else { 0.0 };
        self.points[leg - 1].lerp(self.points[leg], t)
    }

    /// Which way a walker `s` metres along faces, on the ground: its path's way, rounded over corners.
    fn facing(&self, s: f64) -> Vec3 {
        let reach = f64::from(TURN_REACH);
        let way = self.at(s + reach) - self.at(s - reach);
        Vec3::new(way.x, 0.0, way.z).try_normalize().unwrap_or(Vec3::Z)
    }
}

/// `points` moved [`KEEP_RIGHT`] to the right of the way they go.
fn keep_right(points: &[Vec3]) -> Vec<Vec3> {
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
    (0..points.len())
        .map(|at| {
            let into = if at > 0 { flat(points[at] - points[at - 1]) } else { Vec3::ZERO };
            let out = if at + 1 < points.len() { flat(points[at + 1] - points[at]) } else { Vec3::ZERO };
            let way = (into + out).normalize_or_zero();
            points[at] + right_of(way) * KEEP_RIGHT
        })
        .collect()
}

/// The ground's direction to the right of facing `forward`: +X is a character's left as it faces +Z.
fn right_of(forward: Vec3) -> Vec3 {
    Vec3::new(-forward.z, 0.0, forward.x)
}

/// One kind of person, ready to stand.
struct Member {
    mesh: AssetId,
    on_renderer: MeshHandle,
    binding: Arc<GraphBinding>,
    pose: CharacterPose,
    joints: FigureJoints,
    render: RenderPose,
    figure: Figure,
}

/// A post's gestures, as the graph knows them.
#[derive(Clone, Debug)]
struct Gestures {
    moves: Vec<(Name, f64)>,
    /// Least and most apart, in ticks.
    every: [f64; 2],
}

/// A zone's crowd, loaded.
struct Cast {
    _ambient: Handle<Ambient>,
    _meshes: Vec<Handle<Mesh>>,
    _skeletons: Vec<Handle<Skeleton>>,
    _graph: Handle<AnimGraph>,
    clips: Vec<(AssetId, Handle<Clip>)>,
    members: Vec<Member>,
    paths: Vec<Path>,
    gestures: Vec<Gestures>,
    /// How far the walk clip's stride carries a walker, in metres: 0 for a graph with none.
    stride: f32,
}

/// What a zone's crowd is loading, in the order it can: the section, then its meshes and graph, then their
/// skeletons and clips.
struct Loading {
    ambient: Handle<Ambient>,
    meshes: Vec<(AssetId, Handle<Mesh>)>,
    skeletons: Vec<Option<Handle<Skeleton>>>,
    graph: Option<Handle<AnimGraph>>,
    clips: Option<Vec<(AssetId, Handle<Clip>)>>,
}

enum Load {
    None,
    Loading(Box<Loading>),
    Ready(Box<Cast>),
}

/// How a step asks for an asset: in the background, or at once for a capture.
type Wait<'a> = Option<(&'a mut Renderer, &'a GpuContext)>;

/// Asks for `id`: in the background, or until it's resident and on the renderer.
fn ask<T: sapphire_assets::Asset>(
    content: &mut Content,
    id: AssetId,
    wait: &mut Wait<'_>,
) -> Result<Handle<T>, String> {
    match wait {
        Some((renderer, gpu)) => content.hold(id, renderer, gpu).map_err(|error| error.to_string()),
        None => {
            let (server, _) = content.streaming_parts();
            server.load_with(id, LoadPriority::Prefetch).map_err(|error| error.to_string())
        }
    }
}

/// Whether `handle` has loaded, or why it can't.
fn ready<T: sapphire_assets::Asset>(content: &mut Content, handle: &Handle<T>) -> Result<bool, String> {
    let (server, _) = content.streaming_parts();
    match server.state(handle) {
        LoadState::Ready => Ok(true),
        LoadState::Loading => Ok(false),
        LoadState::Failed(error) => Err(error.to_string()),
    }
}

impl Loading {
    fn start(content: &mut Content, id: AssetId, wait: &mut Wait<'_>) -> Result<Self, String> {
        let ambient = ask(content, id, wait)?;
        Ok(Self { ambient, meshes: Vec::new(), skeletons: Vec::new(), graph: None, clips: None })
    }

    /// Takes the next steps that can be taken, and the cast once everything's in.
    fn advance(&mut self, content: &mut Content, wait: &mut Wait<'_>) -> Result<Option<Cast>, String> {
        if !ready(content, &self.ambient)? {
            return Ok(None);
        }
        if self.graph.is_none() {
            let section = content.read(&self.ambient).ok_or("the section isn't resident")?;
            let graph = section.graph;
            let meshes: Vec<AssetId> = section.cast.iter().map(|member| member.mesh).collect();
            for &mesh in &meshes {
                content.keep_mesh_copy(mesh);
            }
            for mesh in meshes {
                let handle =
                    ask(content, mesh, wait).map_err(|error| format!("cast mesh {mesh}: {error}"))?;
                self.meshes.push((mesh, handle));
                self.skeletons.push(None);
            }
            self.graph = Some(ask(content, graph, wait).map_err(|error| format!("graph {graph}: {error}"))?);
        }
        let graph = self.graph.as_ref().expect("asked for above");
        if self.clips.is_none() && ready(content, graph)? {
            let ids: Vec<AssetId> =
                content.read(graph).ok_or("the graph isn't resident")?.clip_ids().collect();
            let mut clips = Vec::with_capacity(ids.len());
            for id in ids {
                clips.push((id, ask(content, id, wait).map_err(|error| format!("clip {id}: {error}"))?));
            }
            self.clips = Some(clips);
        }
        let mut all = true;
        for ((id, mesh), skeleton) in self.meshes.iter().zip(&mut self.skeletons) {
            let landed = ready(content, mesh)? && content.mesh_on_renderer(*id).is_some();
            let copy = content.mesh_copy(*id);
            if !landed || copy.is_none() {
                all = false;
                continue;
            }
            if skeleton.is_none() {
                let skin = copy
                    .and_then(|mesh| mesh.skin.as_ref())
                    .ok_or_else(|| format!("cast mesh {id} isn't skinned"))?;
                let named = skin.skeleton.ok_or_else(|| format!("cast mesh {id} names no skeleton"))?;
                *skeleton = Some(ask(content, named, wait)?);
            }
            all &= ready(content, skeleton.as_ref().expect("asked for above"))?;
        }
        let Some(clips) = &self.clips else { return Ok(None) };
        for (_, clip) in clips {
            all &= ready(content, clip)?;
        }
        if !all {
            return Ok(None);
        }
        Cast::new(self, content).map(Some)
    }
}

impl Cast {
    fn new(loading: &Loading, content: &Content) -> Result<Self, String> {
        let section = content.read(&loading.ambient).ok_or("the section isn't resident")?;
        let graph_handle = loading.graph.clone().expect("loaded");
        let graph = content.read(&graph_handle).ok_or("the graph isn't resident")?;
        let graph = graph.to_owned_graph().map_err(|error| error.to_string())?;
        let clips = loading.clips.clone().expect("loaded");
        let table = ClipTable::new(1, clips.iter().map(|(id, clip)| (*id, content.read(clip))));
        let mut members = Vec::with_capacity(section.cast.len());
        for (at, cooked) in section.cast.iter().enumerate() {
            let name = cooked.name.as_str();
            let failed = |problem: &dyn std::fmt::Display| format!("cast member {name:?}: {problem}");
            let (id, _) = &loading.meshes[at];
            let mesh = content.mesh_copy(*id).ok_or_else(|| failed(&"its mesh isn't kept"))?;
            let skin = mesh.skin.as_ref().ok_or_else(|| failed(&"its mesh isn't skinned"))?;
            let skeleton = loading.skeletons[at].as_ref().and_then(|skeleton| content.read(skeleton));
            let skeleton = skeleton.ok_or_else(|| failed(&"its skeleton isn't resident"))?;
            if skin.joint_count as usize != skeleton.len() {
                return Err(failed(&format!(
                    "its mesh is for {} joints, its skeleton has {}",
                    skin.joint_count,
                    skeleton.len()
                )));
            }
            let find = |joint: &str| {
                skeleton.find(joint).ok_or_else(|| failed(&format!("its skeleton has no {joint:?}")))
            };
            let head = find(cooked.head.as_str())?;
            let feet = [find(cooked.feet[0].as_str())?, find(cooked.feet[1].as_str())?];
            let binding =
                GraphBinding::new(&graph, skeleton, None, 0, &table).map_err(|error| failed(&error))?;
            let pose = CharacterPose::new(skeleton, &skin.binds, &[], Vec::new());
            let joints = FigureJoints::of_joints(&pose, head, 0.0, feet);
            let mut palette = vec![Affine3A::IDENTITY; pose.palette_len()];
            pose.write_palette(&mut palette);
            let figure = joints.moving(&pose);
            let on_renderer =
                content.mesh_on_renderer(*id).ok_or_else(|| failed(&"its mesh isn't on the renderer"))?;
            members.push(Member {
                mesh: *id,
                on_renderer,
                binding: Arc::new(binding),
                pose,
                joints,
                render: RenderPose { palette, morph_weights: Vec::new() },
                figure,
            });
        }
        let paths = section
            .walks
            .iter()
            .map(|walk| {
                let points: Vec<Vec3> = walk
                    .points
                    .iter()
                    .map(|point| Vec3::from(point.map(|value| value.to_native())))
                    .collect();
                Path::new(&points, walk.closed, walk.pace.to_native())
            })
            .collect();
        let seconds = |clip_name: &str| -> Option<f64> {
            let id = graph.clips.iter().find(|clip| clip.name == clip_name)?.id;
            let clip = table.clip(id)?;
            let rate = f64::from(clip.rate.to_native());
            (rate > 0.0).then(|| f64::from(clip.ticks.to_native()) / rate)
        };
        let gestures = section
            .posts
            .iter()
            .map(|post| {
                let moves = post
                    .gestures
                    .iter()
                    .filter_map(|gesture| {
                        let entry = graph.combat.moves.iter().find(|entry| entry.name == gesture.as_str());
                        let length = entry.and_then(|entry| seconds(&entry.clip));
                        if length.is_none() {
                            tracing::warn!(target: LOG_TARGET, post = %post.name, %gesture, "the ambient graph plays no such gesture");
                        }
                        length.map(|length| (Name::new(gesture.as_str()), length * f64::from(TICK_HZ)))
                    })
                    .collect();
                let every = post.every.map(|seconds| f64::from(seconds.to_native()) * f64::from(TICK_HZ));
                Gestures { moves, every }
            })
            .collect();
        // The walk's stride: the synced sample of the graph's first state that goes forward fastest, carried
        // over its clip at the speed it was authored at.
        let stride = graph
            .locomotion
            .states
            .iter()
            .find(|state| state.name == graph.locomotion.start)
            .and_then(|state| match &state.plays {
                StateMotion::Blend(space) => space
                    .samples
                    .iter()
                    .filter(|sample| sample.sync && sample.at[1] > 0.0)
                    .max_by(|a, b| a.at[1].total_cmp(&b.at[1]))
                    .and_then(|sample| Some(sample.at[1] * seconds(&sample.clip)? as f32)),
                _ => None,
            })
            .unwrap_or(0.0);
        Ok(Self {
            _ambient: loading.ambient.clone(),
            _meshes: loading.meshes.iter().map(|(_, mesh)| mesh.clone()).collect(),
            _skeletons: loading.skeletons.iter().flatten().cloned().collect(),
            _graph: graph_handle,
            clips,
            members,
            paths,
            gestures,
            stride,
        })
    }
}

/// splitmix64: a person's picks, from the crowd's seed and what's picked.
fn pick(seed: u32, person: u64, what: u64) -> u64 {
    let mut z = ((u64::from(seed) << 32) ^ person.wrapping_mul(0x9e37_79b9) ^ what.wrapping_mul(0xbf58_476d))
        .wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A pick as a fraction from 0 up to 1.
fn fraction(pick: u64) -> f64 {
    (pick >> 11) as f64 / (1u64 << 53) as f64
}

/// The zone's people, from its section, on its grid's `cell_size`.
fn people_of(section: &ArchivedAmbient, cast: &Cast, cell_size: f32) -> Vec<Person> {
    let seed = section.seed.to_native();
    let members = cast.members.len().max(1) as u64;
    let cell = |at: Vec3| [(at.x / cell_size).floor() as i32, (at.z / cell_size).floor() as i32];
    let member = |who: u16, index: u64| {
        if who == sapphire_zone::ambient::ANYONE { (pick(seed, index, 1) % members) as u16 } else { who }
    };
    let mut people = Vec::new();
    for (at, post) in section.posts.iter().enumerate() {
        let index = people.len() as u64;
        let position = Vec3::from(post.position.map(|value| value.to_native()));
        people.push(Person {
            member: member(post.cast.to_native(), index),
            rank: pick(seed, index, 0),
            cell: cell(position),
            clock: (pick(seed, index, 2) % 600) as f64,
            what: What::Post { at: position, yaw: post.yaw.to_native(), post: at as u16 },
        });
    }
    for (at, walk) in section.walks.iter().enumerate() {
        let path = &cast.paths[at];
        let middle = walk
            .points
            .iter()
            .fold(Vec3::ZERO, |sum, point| sum + Vec3::from(point.map(|value| value.to_native())))
            / walk.points.len().max(1) as f32;
        let count = walk.count.to_native();
        for walker in 0..count {
            let index = people.len() as u64;
            let spacing = f64::from(path.length()) / f64::from(count);
            let start = spacing * (f64::from(walker) + 0.3 * fraction(pick(seed, index, 3)));
            people.push(Person {
                member: member(walk.cast.to_native(), index),
                rank: pick(seed, index, 0),
                cell: cell(middle),
                clock: (pick(seed, index, 2) % 600) as f64,
                what: What::Walk { walk: at as u16, start },
            });
        }
    }
    people
}

/// Where a person stands at tick `ticks`, in the zone's space before any stepping aside, which way they
/// face, how fast they go, and how far they've walked.
#[derive(Clone, Copy, Debug)]
struct Place {
    at: Vec3,
    forward: Vec3,
    speed: f32,
    walked: f64,
}

fn place_of(person: &Person, cast: &Cast, ticks: f64) -> Place {
    match person.what {
        What::Post { at, yaw, .. } => {
            Place { at, forward: Vec3::new(yaw.sin(), 0.0, yaw.cos()), speed: 0.0, walked: 0.0 }
        }
        What::Walk { walk, start } => {
            let path = &cast.paths[usize::from(walk)];
            let walked = start + f64::from(path.pace) * ticks / f64::from(TICK_HZ);
            Place { at: path.at(walked), forward: path.facing(walked), speed: path.pace, walked }
        }
    }
}

/// The gesture a post's person plays at `ticks`, if any: one starts in each of `every`'s most, at least its
/// least after the last, which and when picked by the seed.
fn gesture_at(gestures: &Gestures, seed: u32, index: u64, ticks: f64) -> Option<CombatInput> {
    if gestures.moves.is_empty() {
        return None;
    }
    let [least, most] = gestures.every;
    let slot = (ticks / most).floor();
    let picked = pick(seed, index, 4 ^ ((slot as i64 as u64) << 8));
    let start = slot * most + fraction(picked) * (most - least).max(0.0);
    let (name, length) = gestures.moves[(picked % gestures.moves.len() as u64) as usize];
    let position = ticks - start;
    (0.0..length).contains(&position).then_some(CombatInput { name, serial: slot as i64 as u32, position })
}

/// A zone's crowd on stage: see the [module docs](self).
pub(super) struct Crowd {
    on: bool,
    cap: AmbientCap,
    /// The tier's animation levels, which the crowd is posed through whether or not anyone else is.
    levels: AnimLevels,
    /// The zone it's for, while one's on stage and the crowd is on.
    zone: Option<AssetId>,
    load: Load,
    seed: u32,
    /// The zone's grid's cells' size, which people belong to cells of.
    cell_size: f32,
    people: Vec<Person>,
    /// Those standing, by index, with their entities, and the list the next frame's are made in.
    shown: Vec<(u32, Entity)>,
    kept: Vec<(u32, Entity)>,
    /// This frame's choice, by index: kept from frame to frame.
    chosen: Vec<u32>,
    candidates: Vec<(i32, u64, u32)>,
    places: Vec<Place>,
    /// How far each walker steps aside this frame.
    asides: Vec<f32>,
    query: Option<People>,
    gathered: Vec<Gathered<'static>>,
    jobs: Vec<AmbientJob<'static>>,
    stats: AmbientStats,
    anim: AnimStats,
}

impl Default for Crowd {
    fn default() -> Self {
        Self {
            on: true,
            cap: AmbientCap::DESKTOP_HIGH,
            levels: AnimLevels::DESKTOP_HIGH,
            zone: None,
            load: Load::None,
            seed: 0,
            cell_size: 32.0,
            people: Vec::new(),
            shown: Vec::new(),
            kept: Vec::new(),
            chosen: Vec::new(),
            candidates: Vec::new(),
            places: Vec::new(),
            asides: Vec::new(),
            query: None,
            gathered: Vec::new(),
            jobs: Vec::new(),
            stats: AmbientStats::default(),
            anim: AnimStats::default(),
        }
    }
}

impl Crowd {
    /// What animation did for the crowd in the last frame.
    pub(super) fn anim_stats(&self) -> AnimStats {
        self.anim
    }

    /// Takes everyone away and lets go of what they're made of.
    fn clear(&mut self, world: &mut World) {
        for (_, entity) in self.shown.drain(..) {
            world.despawn(entity);
        }
        self.load = Load::None;
        self.people.clear();
        self.stats = AmbientStats::default();
        self.anim = AnimStats::default();
    }

    /// Stands whoever this frame's choice has that isn't standing, and takes away whoever's standing that
    /// it hasn't. Both lists are by index.
    fn follow_choice(&mut self, world: &mut World, to_world: Mat4) {
        let Load::Ready(cast) = &self.load else { return };
        let mut kept = std::mem::take(&mut self.kept);
        kept.clear();
        let mut next = 0;
        for &(index, entity) in &self.shown {
            while next < self.chosen.len() && self.chosen[next] < index {
                kept.push(spawn(world, cast, &self.people, self.chosen[next], to_world));
                next += 1;
            }
            if next < self.chosen.len() && self.chosen[next] == index {
                kept.push((index, entity));
                next += 1;
            } else {
                world.despawn(entity);
            }
        }
        for &index in &self.chosen[next..] {
            kept.push(spawn(world, cast, &self.people, index, to_world));
        }
        self.kept = std::mem::replace(&mut self.shown, kept);
    }
}

/// Stands person `index` where tick 0 has them; its first pose places it.
fn spawn(world: &mut World, cast: &Cast, people: &[Person], index: u32, to_world: Mat4) -> (u32, Entity) {
    let person = &people[index as usize];
    let member = &cast.members[usize::from(person.member)];
    let seed = (person.rank >> 32) as u32;
    let player = GraphPlayer::new(member.binding.clone(), member.pose.clone(), seed);
    let place = place_of(person, cast, 0.0);
    let transform = to_world * Mat4::from_translation(place.at);
    let entity = world
        .spawn((
            RenderMesh(member.on_renderer),
            RenderTransform(transform),
            member.render.clone(),
            member.figure,
            CharacterLightRig { rig: LightRigId::DEFAULT, head: member.figure.head },
            GraphDriver::new(player, member.joints),
            AmbientPerson { index, posed: false },
        ))
        .id();
    (index, entity)
}

/// One person's components for a frame.
struct Gathered<'w> {
    index: u32,
    person: Mut<'w, AmbientPerson>,
    driver: Mut<'w, GraphDriver>,
    render: Mut<'w, RenderPose>,
    figure: Mut<'w, Figure>,
    rig: Mut<'w, CharacterLightRig>,
    transform: Mut<'w, RenderTransform>,
}

/// One person's frame: its graph's, after a first pose that starts it where its phase is its distance.
struct AmbientJob<'w> {
    evaluation: GraphEvaluation<'w>,
    first: Option<GraphInput>,
    clips: &'w ClipTable<'w>,
}

impl AmbientJob<'_> {
    fn run(&mut self) {
        if let Some(first) = &self.first {
            self.evaluation.driver.pose_frame(first, self.clips, self.evaluation.place);
        }
        self.evaluation.run(self.clips);
    }
}

impl Scene {
    /// Draws a zone's ambient characters, or none, from the next frame on (M5 C10, `--ambient`): on unless
    /// told otherwise. Nothing the simulation reads moves either way.
    pub fn set_ambient_on(&mut self, on: bool) {
        self.ambient.on = on;
    }

    /// Draws as many ambient characters as `cap` says, from as far out, from the next frame on.
    pub fn set_ambient_cap(&mut self, cap: AmbientCap) {
        self.ambient.cap = cap;
    }

    /// Draws and poses ambient characters as `tier` does, from the next frame on: its cap and its animation
    /// levels, which hold for the crowd whether or not the rest of the scene's motion follows the tier
    /// ([`Scene::set_motion_tier`]), so a zone's walk thins its people as the tier says.
    pub fn set_ambient_tier(&mut self, tier: TierId) {
        self.ambient.cap = ambient_cap(tier);
        self.ambient.levels = anim_levels(tier);
    }

    /// What the zone's crowd came to in the last frame.
    #[must_use]
    pub fn ambient_stats(&self) -> AmbientStats {
        self.ambient.stats
    }

    /// Loads the crowd of the zone on stage until it stands, and stands it: a capture's or a bench's, which
    /// must see the same people on its first frame as on any. A crowd that can't load says so and stays away.
    pub fn finish_ambient(&mut self, content: &mut Content, renderer: &mut Renderer, gpu: &GpuContext) {
        self.follow_ambient_waiting(content, Some((renderer, gpu)));
    }

    /// This frame's crowd, after the zone's streaming: what loads, and who stands.
    pub(super) fn follow_ambient(&mut self, content: &mut Content) {
        self.follow_ambient_waiting(content, None);
    }

    fn follow_ambient_waiting(&mut self, content: &mut Content, mut wait: Wait<'_>) {
        let wanted = if self.ambient.on { self.zone_on_stage() } else { None };
        if self.ambient.zone != wanted {
            self.ambient.clear(self.ecs.world_mut());
            self.ambient.zone = wanted;
            if let Some(zone) = wanted {
                let id = zone.sub_asset(AMBIENT_PART);
                let (server, _) = content.streaming_parts();
                if server.contains(id) {
                    match Loading::start(content, id, &mut wait) {
                        Ok(loading) => self.ambient.load = Load::Loading(Box::new(loading)),
                        Err(error) => {
                            tracing::warn!(target: LOG_TARGET, %zone, error, "the zone's crowd stays away")
                        }
                    }
                }
            }
        }
        if let Load::Loading(loading) = &mut self.ambient.load {
            match loading.advance(content, &mut wait) {
                Ok(None) => return,
                Ok(Some(cast)) => self.take_cast(cast, content),
                Err(error) => {
                    let zone = self.ambient.zone;
                    tracing::warn!(target: LOG_TARGET, ?zone, error, "the zone's crowd stays away");
                    self.ambient.load = Load::None;
                    return;
                }
            }
        }
        self.choose_ambient();
    }

    /// The crowd's cast has loaded: its people, from its section on the zone's grid.
    fn take_cast(&mut self, cast: Cast, content: &mut Content) {
        let Some(zone) = &self.zone else { return };
        let (server, _) = content.streaming_parts();
        let cell_size =
            server.get(zone.streamer.zone()).map_or(32.0, |zone| zone.presentation.cell_size.to_native());
        let Some(section) = server.get(&cast._ambient) else { return };
        self.ambient.seed = section.seed.to_native();
        self.ambient.cell_size = cell_size;
        self.ambient.people = people_of(section, &cast, cell_size);
        self.ambient.stats.people = self.ambient.people.len() as u32;
        self.ambient.load = Load::Ready(Box::new(cast));
        self.extract_poses();
    }

    /// Who stands this frame, and standing them: the cells in reach of the focus, the tier's count of their
    /// people, by ring and rank (the [module docs](self)). Allocates nothing once its lists have held a crowd,
    /// but for the people it stands.
    fn choose_ambient(&mut self) {
        let stopwatch = Stopwatch::start();
        let Some(zone) = &self.zone else { return };
        let crowd = &mut *self.ambient;
        if !matches!(crowd.load, Load::Ready(_)) {
            return;
        }
        let eye = self.ecs.world().resource::<crate::orbit::OrbitCamera>().eye();
        let focus = zone.held_focus.map_or(eye + zone.stand, Vec3::from);
        let to_world = Mat4::from_translation(-zone.stand);
        let cell_size = crowd.cell_size;
        let own = [(focus.x / cell_size).floor() as i32, (focus.z / cell_size).floor() as i32];
        crowd.candidates.clear();
        for (index, person) in crowd.people.iter().enumerate() {
            let near = |cell: i32, at: f32| {
                let least = cell as f32 * cell_size;
                (least - at).max(at - (least + cell_size)).max(0.0)
            };
            let (dx, dz) = (near(person.cell[0], focus.x), near(person.cell[1], focus.z));
            if dx * dx + dz * dz > crowd.cap.reach * crowd.cap.reach {
                continue;
            }
            let ring = (person.cell[0] - own[0]).abs().max((person.cell[1] - own[1]).abs());
            crowd.candidates.push((ring, person.rank, index as u32));
        }
        let count = usize::from(crowd.cap.count);
        if crowd.candidates.len() > count {
            crowd.candidates.select_nth_unstable(count);
            crowd.candidates.truncate(count);
        }
        crowd.chosen.clear();
        crowd.chosen.extend(crowd.candidates.iter().map(|&(_, _, index)| index));
        crowd.chosen.sort_unstable();
        crowd.follow_choice(self.ecs.world_mut(), to_world);
        crowd.stats.spawned = crowd.shown.len() as u32;
        crowd.stats.activation = stopwatch.elapsed();
    }

    /// Poses the crowd for this frame, after everyone else (see the [module docs](self)).
    pub(super) fn animate_ambient(&mut self, content: &Content, alpha: f32) {
        self.ambient.anim = AnimStats::default();
        let ambient = &mut self.ambient.stats;
        (ambient.in_view, ambient.middle_band, ambient.far_band, ambient.springs) = (0, 0, 0, 0);
        if self.ambient.shown.is_empty() {
            return;
        }
        let Some(stand) = self.zone.as_ref().map(|zone| zone.stand) else { return };
        profiling::scope!("animate ambient");
        let started = Stopwatch::start();
        let camera = self.camera().camera_data(self.aspect);
        let levels =
            LevelView::new(&camera, self.packet().sun.direction, self.ambient.levels, self.anim_levels_on);
        let jobs_on = self.anim_jobs;
        let world = self.ecs.world_mut();
        let ticks = world.resource::<WorldClock>().0.presented_ticks(alpha);
        let character = world
            .get::<RenderTransform>(self.character)
            .filter(|_| world.get::<RenderMesh>(self.character).is_some())
            .map(|transform| transform.0.w_axis.truncate() + stand);
        let to_world = Mat4::from_translation(-stand);
        let crowd = &mut *self.ambient;
        let Load::Ready(cast) = &crowd.load else { return };
        let cast = &**cast;
        let seed = crowd.seed;

        // Where everyone is, then who steps aside for whom, from where they'd be.
        crowd.places.clear();
        crowd.places.extend(
            crowd.shown.iter().map(|&(index, _)| place_of(&crowd.people[index as usize], cast, ticks)),
        );
        crowd.asides.clear();
        for (at, place) in crowd.places.iter().enumerate() {
            crowd.asides.push(0.0);
            if place.speed <= 0.0 {
                continue;
            }
            let right = right_of(place.forward);
            let others =
                crowd.places.iter().enumerate().filter(|&(other, _)| other != at).map(|(_, other)| other.at);
            let mut aside = 0.0f32;
            for other in others.chain(character) {
                let to = other - place.at;
                let ahead = to.dot(place.forward);
                let side = to.dot(right);
                if ahead < -0.3 || ahead > ASIDE_FAR || side.abs() > ASIDE_NEAR + STEP_ASIDE {
                    continue;
                }
                let near = (1.0
                    - (Vec3::new(to.x, 0.0, to.z).length() - ASIDE_NEAR) / (ASIDE_FAR - ASIDE_NEAR))
                    .clamp(0.0, 1.0);
                let weight = near * near * (3.0 - 2.0 * near);
                // Away from them: to its right, unless they're on its right.
                let way = (1.0 - side * 4.0).clamp(-1.0, 1.0);
                if weight * way.abs() > aside.abs() {
                    aside = weight * way;
                }
            }
            crowd.asides[at] = aside * STEP_ASIDE;
        }
        for (place, aside) in crowd.places.iter_mut().zip(&crowd.asides) {
            place.at += right_of(place.forward) * *aside;
        }

        let table = ClipTable::new(1, cast.clips.iter().map(|(id, clip)| (*id, content.read(clip))));
        let table = &table;
        let query = crowd.query.get_or_insert_with(|| world.query());
        let mut gathered: Vec<Gathered<'_>> = reuse_list(std::mem::take(&mut crowd.gathered));
        for (_, person, driver, render, figure, rig, transform) in query.iter_mut(world) {
            let index = person.index;
            gathered.push(Gathered { index, person, driver, render, figure, rig, transform });
        }
        gathered.sort_unstable_by_key(|person| person.index);

        let mut stats = AnimStats::default();
        let (mut in_view, mut bands, mut springs) = (0, [0u32; 3], 0);
        let mut work: Vec<AmbientJob<'_>> = reuse_list(std::mem::take(&mut crowd.jobs));
        for mut person in gathered.drain(..) {
            let Ok(at) = crowd.shown.binary_search_by_key(&person.index, |&(index, _)| index) else {
                continue;
            };
            let who = &crowd.people[person.index as usize];
            let place = crowd.places[at];
            let yaw = place.forward.x.atan2(place.forward.z);
            let transform = to_world * Mat4::from_translation(place.at) * Mat4::from_rotation_y(yaw);
            person.transform.set_if_neq(RenderTransform(transform));
            stats.characters += 1;
            let clock =
                LevelClock { tick: ticks.max(0.0) as u64, place: person.index, cut: !person.person.posed };
            let pace = levels.pace(person.driver.level_mut(), transform, &person.figure, clock);
            springs += u32::from(person.driver.has_springs());
            if !matches!(pace, AnimPace::Hold { off_screen: true }) {
                in_view += 1;
                bands[usize::from(person.driver.level_mut().band()).min(2)] += 1;
            }
            let level = match pace {
                AnimPace::Update(level) => level,
                AnimPace::Hold { off_screen } => {
                    stats.held += 1;
                    stats.thinned += 1;
                    stats.off_screen += u32::from(off_screen);
                    continue;
                }
            };
            stats.thinned += u32::from(level.is_thinned());
            // A level past the nearest poses on a whole tick; the nearest between ticks, where transforms draw.
            let at_ticks = if level.interval <= 1 { ticks } else { ticks.floor() };
            let combat = match who.what {
                What::Post { post, .. } => {
                    gesture_at(&cast.gestures[usize::from(post)], seed, u64::from(person.index), at_ticks)
                }
                What::Walk { .. } => None,
            };
            let locomotion = LocomotionInput { speed: place.speed, ..LocomotionInput::default() };
            let input =
                GraphInput { ticks: at_ticks + who.clock, locomotion, combat, ..GraphInput::default() };
            // Its first pose starts it as far back as it has walked since its stride last began, cut.
            let first = (!person.person.posed).then(|| {
                let back = if place.speed > 0.0 && cast.stride > 0.0 {
                    let walked = place_of(who, cast, at_ticks).walked;
                    walked.rem_euclid(f64::from(cast.stride)) / f64::from(place.speed) * f64::from(TICK_HZ)
                } else {
                    1.0
                };
                let earlier = input.ticks - back;
                let combat = match who.what {
                    What::Post { post, .. } => gesture_at(
                        &cast.gestures[usize::from(post)],
                        seed,
                        u64::from(person.index),
                        at_ticks - back,
                    ),
                    What::Walk { .. } => None,
                };
                GraphInput { ticks: earlier, combat, cut: true, ..input }
            });
            person.person.posed = true;
            let evaluation = GraphEvaluation {
                driver: person.driver,
                render: person.render,
                figure: person.figure,
                rig: person.rig,
                input,
                world_from_model: Affine3A::from_mat4(transform),
                place: transform,
                springs_run: false,
                floor: false,
                evaluated: false,
                spring_work: sapphire_anim::SpringWork::default(),
                spring_time: Duration::ZERO,
            };
            work.push(AmbientJob { evaluation, first, clips: table });
        }
        if jobs_on && work.len() >= JOBS_FROM {
            stats.jobs = u32::try_from(evaluate_all(&mut work, AmbientJob::run)).unwrap_or(u32::MAX);
        } else {
            work.iter_mut().for_each(AmbientJob::run);
        }
        for job in work.drain(..) {
            if job.evaluation.evaluated {
                stats.evaluated += 1;
                stats.joints += job.evaluation.driver.player.pose().joint_count() as u32;
            }
        }
        crowd.gathered = reuse_list(gathered);
        crowd.jobs = reuse_list(work);
        stats.time = started.elapsed();
        crowd.anim = stats;
        crowd.stats.in_view = in_view;
        (crowd.stats.middle_band, crowd.stats.far_band, crowd.stats.springs) = (bands[1], bands[2], springs);
    }

    /// Draws the crowd with its cast's meshes on a new renderer: after device loss or for a new tier.
    pub(super) fn set_ambient_meshes(&mut self, content: &Content) {
        let Load::Ready(cast) = &mut self.ambient.load else { return };
        for member in &mut cast.members {
            if let Some(handle) = content.mesh_on_renderer(member.mesh) {
                member.on_renderer = handle;
            }
        }
        let world = self.ecs.world_mut();
        for &(index, entity) in &self.ambient.shown {
            let person = &self.ambient.people[index as usize];
            let member = &cast.members[usize::from(person.member)];
            world.entity_mut(entity).insert(RenderMesh(member.on_renderer));
        }
    }
}

/// Both frames' animation together.
pub(super) fn sum(a: AnimStats, b: AnimStats) -> AnimStats {
    AnimStats {
        characters: a.characters + b.characters,
        evaluated: a.evaluated + b.evaluated,
        held: a.held + b.held,
        thinned: a.thinned + b.thinned,
        off_screen: a.off_screen + b.off_screen,
        jobs: a.jobs + b.jobs,
        joints: a.joints + b.joints,
        springs: a.springs + b.springs,
        spring_substeps: a.spring_substeps + b.spring_substeps,
        spring_time: a.spring_time + b.spring_time,
        teleports: a.teleports + b.teleports,
        ik: a.ik + b.ik,
        time: a.time + b.time,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_goes_round_and_an_open_one_comes_back_on_its_right() {
        let line = [Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0)];
        let path = Path::new(&line, false, 1.0);
        // There along one side and back along the other, 0.9 m apart, crossing over at each end.
        assert!((path.length() - (20.0 + 4.0 * KEEP_RIGHT)).abs() < 1e-4, "{}", path.length());
        let out = path.at(5.0);
        assert!((out.x + KEEP_RIGHT).abs() < 1e-4 && (out.z - 5.0).abs() < 1e-4, "{out}");
        let back = path.at(f64::from(path.length()) - 5.0);
        assert!((back.x - KEEP_RIGHT).abs() < 1e-4, "{back}");
        assert!(path.facing(5.0).abs_diff_eq(Vec3::Z, 1e-4));
        assert!(path.facing(f64::from(path.length()) - 5.0).abs_diff_eq(Vec3::NEG_Z, 1e-4));
        // Round and round.
        assert!(path.at(3.0).abs_diff_eq(path.at(3.0 + f64::from(path.length()) * 4.0), 1e-3));
    }

    #[test]
    fn a_corner_turns_over_its_reach() {
        let square =
            [Vec3::ZERO, Vec3::new(0.0, 0.0, 4.0), Vec3::new(4.0, 0.0, 4.0), Vec3::new(4.0, 0.0, 0.0)];
        let path = Path::new(&square, true, 1.0);
        assert!((path.length() - 16.0).abs() < 1e-4);
        let corner = path.facing(4.0);
        assert!((corner - Vec3::new(1.0, 0.0, 1.0).normalize()).length() < 1e-3, "{corner}");
        assert!(path.facing(4.0 - f64::from(TURN_REACH) - 0.01).abs_diff_eq(Vec3::Z, 1e-4));
    }

    #[test]
    fn gestures_come_by_the_tick_alone_within_their_spacing() {
        let gestures = Gestures {
            moves: vec![(Name::new("look_around"), 90.0), (Name::new("shift"), 60.0)],
            every: [8.0 * 60.0, 16.0 * 60.0],
        };
        let mut starts = Vec::new();
        let mut last = None;
        for tick in 0..(16 * 60 * 40) {
            let playing = gesture_at(&gestures, 7, 3, f64::from(tick));
            assert_eq!(playing, gesture_at(&gestures, 7, 3, f64::from(tick)), "the same every time");
            if let Some(playing) = playing
                && last.is_none_or(|serial| serial != playing.serial)
            {
                starts.push(f64::from(tick) - playing.position);
                last = Some(playing.serial);
            }
        }
        assert!(starts.len() >= 39, "one in each slot: {}", starts.len());
        for pair in starts.windows(2) {
            assert!(pair[1] - pair[0] >= 8.0 * 60.0 - 1.0, "{pair:?}");
        }
        let none = Gestures { moves: Vec::new(), every: [60.0, 120.0] };
        assert_eq!(gesture_at(&none, 7, 3, 100.0), None);
    }

    #[test]
    fn picks_spread_and_repeat() {
        assert_eq!(pick(7, 3, 0), pick(7, 3, 0));
        assert_ne!(pick(7, 3, 0), pick(7, 4, 0));
        assert_ne!(pick(7, 3, 0), pick(8, 3, 0));
        let mean = (0..1000).map(|index| fraction(pick(1, index, 0))).sum::<f64>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05, "{mean}");
    }
}
