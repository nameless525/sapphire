//! The foliage bake (M1 W6, look pillar 7): volume normals transferred onto leaf cards, and the
//! per-vertex data the foliage shader's wind and color gradient read.
//!
//! # Normal transfer
//!
//! Anime foliage reads as soft painted clumps, not noisy individual leaves: a canopy lights as
//! a rounded volume with one clean terminator and a soft colored shade. Each card's own normal
//! would light every leaf separately, so the cook replaces the normals of the slots
//! [`FoliageSettings::slots`] names with a volume's ("aggregate normals": DreamWorks' Peterson
//! and Lee; world_lighting_post.md §8), blended with the card's own by
//! [`FoliageSettings::blend`] so a little of each leaf's facing survives. The volume is one of
//! [`NormalTransfer`]'s:
//!
//! - **Sphere:** one ellipsoid around the slots' vertices (their bounds): radial normals.
//! - **Clumps:** `count` spheres, one per clump of cards: the cards (connected pieces of the
//!   slots' triangles, welded by position) are clustered by k-means, each clump lighting as its
//!   own rounded volume, the painted "cloud of blobs" canopy.
//! - **Hull:** the gradient of a smooth field over the same clumps (a metaball proxy of the
//!   canopy): one soft volume whose normals turn smoothly from clump to clump and card to card.
//! - **Up:** object space +Y: grass and ground cover light like the ground they stand on.
//!
//! A card's back faces share the volume normal: before the blend, the card's own normals are
//! flipped toward the volume, all of a face's at once (the transferred triangles connected
//! through shared vertices: a card, or one side of a card modeled with its two sides apart), by
//! the sign of their sum against the volume's, so a card standing edge-on to the volume isn't
//! flipped corner by corner. A two-sided card lights the same from either side.
//!
//! Kits that model a tree's trunk and its canopy in one material (blob canopies on a trunk, as
//! Kenney's do) can't name the canopy by slot: [`FoliageSettings::canopy_from`] leaves out the
//! pieces of the slots that start low on the mesh (the trunk), which then take the wood's
//! normals and weights, so the canopy alone rounds off and flutters. Pieces are welded at the
//! mesh's own weld tolerance (its sidecar's `outline.weld_tolerance`), so a source whose split
//! vertices drift further than the default doesn't fall apart at its seams.
//!
//! # Wind and gradient data
//!
//! Every vertex of the mesh gets a [`FoliageVertex`], wood included, so trunk, branches and
//! leaves move together:
//!
//! | Field | Cards and canopies (the slots) | Grass ([`NormalTransfer::Up`]) | Other slots (wood) |
//! |---|---|---|---|
//! | sway | (height above the mesh's root ÷ its height)², a cantilever's bend | the same | the same |
//! | bend | horizontal distance from the object's vertical axis ÷ the farthest vertex's, eased in from 0 at the root over the lowest [`BEND_RISE`] of the height | the height fraction | as the cards' |
//! | flutter | 1 | the height fraction | 0 |
//! | phase | its clump's: the phase field (below) at the clump's centroid, or at the whole canopy's for [`NormalTransfer::Sphere`] | its blade's, at the blade's centroid | the field at the vertex, or at its piece's centroid for a piece around the axis (a trunk) |
//! | gradient | depth in the canopy's ellipsoid: 0 at its center, 1 on its surface | the height fraction | the height fraction |
//!
//! **The phase field** is the azimuth around the vertical axis as a fraction of a turn, plus
//! 0.31 a meter outward, so branches pointing different ways swing in their own time. It wraps
//! (1 is 0), so the azimuth's wrap doesn't tear the swing, but around the axis it turns a whole
//! cycle: anything wrapped around the axis (a trunk, a crown) would move its sides in antiphase,
//! squashing and stretching rather than swaying. So a canopy's clump, a blade and a piece of
//! wood around the axis each take one phase; a branch, which points away from the axis, keeps
//! the field's per vertex, so it swings from its base to its tip into its clump's phase.
//!
//! **What stays joined:** sway and bend are continuous in the position, so vertices that meet
//! (split vertices, a card on its branch) sway and bend alike. Flutter (1 on cards, 0 on wood)
//! and the phases (one per clump, blade or trunk) change from piece to piece, so a card welded
//! into its wood would tear there: model cards as pieces of their own, hanging off the wood, as
//! card kits and the samples do. Objects are modeled with their origin at the root, as trees and
//! grass are: the shader sways them about it.

use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;

use glam::Vec3;
use sapphire_assets::mesh::FoliageVertex;
use serde::{Deserialize, Serialize};

use super::weld;
use crate::mesh::ImportedMesh;

/// The most clumps [`NormalTransfer::Clumps`] and [`NormalTransfer::Hull`] fit.
pub const MAX_CLUMPS: u32 = 64;

/// k-means iterations: the clusters of a canopy's cards settle long before.
const KMEANS_ITERATIONS: usize = 24;

/// Each clump's radius, the hull field's falloff, is its cards' RMS distance from its center
/// times this: wide enough that neighboring clumps' fields overlap and merge into one volume,
/// filling the waist between them rather than lighting each as a ball.
const CLUMP_RADIUS_SCALE: f32 = 2.0;

/// The smallest extent, as a fraction of the largest, an ellipsoid or clump keeps, so a flat
/// canopy (one layer of cards) still has a volume.
const MIN_EXTENT_RATIO: f32 = 0.05;

/// Phase added per meter away from the axis, in cycles, so inner and outer branches don't swing
/// in step either.
const PHASE_PER_METER: f32 = 0.31;

/// The share of the mesh's height over which branch bend eases in from 0 at the root, so a
/// trunk's foot stays planted on the ground rather than sliding with its branches.
pub const BEND_RISE: f32 = 0.3;

/// How short the mean of a group's horizontal directions from the axis is when the group
/// surrounds the axis (a trunk, a crown): 1 for a group off to one side, near 0 around it.
const AROUND_AXIS: f32 = 0.5;

/// Foliage import settings, part of a mesh's import settings
/// ([`MeshSettings::foliage`](crate::meta::MeshSettings::foliage)): a mesh with them cooks a
/// foliage stream. Loading checks them as [`Self::validate`] does, and a sidecar may leave any
/// of them out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, try_from = "FoliageFields")]
pub struct FoliageSettings {
    /// The volume the slots' normals are transferred from.
    pub normals: NormalTransfer,
    /// The material slots (by name) whose normals transfer: leaf cards, canopies, blades. Empty:
    /// every slot.
    pub slots: Vec<String>,
    /// How much of the volume normal replaces the source's, 0 to 1. Around 0.8 keeps a little of
    /// each card's own facing, so a canopy isn't a perfectly smooth ball.
    pub blend: f32,
    /// Pieces of the slots (their connected parts, welded by position) whose lowest point is below
    /// this fraction of the mesh's height are wood, not foliage: they keep their normals and
    /// never flutter. For meshes that model trunk and canopy in one material; 0 (the default)
    /// takes every piece of the slots. Written out only when set.
    #[serde(skip_serializing_if = "is_zero")]
    pub canopy_from: f32,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

/// [`FoliageSettings`] as written, before it is checked.
#[derive(Deserialize)]
#[serde(default)]
struct FoliageFields {
    normals: NormalTransfer,
    slots: Vec<String>,
    blend: f32,
    canopy_from: f32,
}

impl Default for FoliageFields {
    fn default() -> Self {
        let FoliageSettings { normals, slots, blend, canopy_from } = FoliageSettings::default();
        Self { normals, slots, blend, canopy_from }
    }
}

impl TryFrom<FoliageFields> for FoliageSettings {
    type Error = &'static str;

    fn try_from(fields: FoliageFields) -> Result<Self, Self::Error> {
        let FoliageFields { normals, slots, blend, canopy_from } = fields;
        let settings = Self { normals, slots, blend, canopy_from };
        settings.validate().map(|()| settings)
    }
}

impl Default for FoliageSettings {
    fn default() -> Self {
        Self { normals: NormalTransfer::Sphere, slots: Vec::new(), blend: 0.8, canopy_from: 0.0 }
    }
}

impl FoliageSettings {
    /// Checks the blend and `canopy_from` are in 0..=1 and clump counts in 1..=[`MAX_CLUMPS`].
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(0.0..=1.0).contains(&self.blend) {
            return Err("foliage blend must be within 0..=1");
        }
        if !(0.0..=1.0).contains(&self.canopy_from) {
            return Err("foliage canopy_from must be within 0..=1");
        }
        match self.normals {
            NormalTransfer::Clumps { count } | NormalTransfer::Hull { count }
                if !(1..=MAX_CLUMPS).contains(&count) =>
            {
                Err("foliage clump counts must be within 1..=64")
            }
            _ => Ok(()),
        }
    }
}

/// The volume a foliage mesh's normals come from (see the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NormalTransfer {
    /// Radial normals from one ellipsoid around the slots' vertices.
    #[default]
    Sphere,
    /// Radial normals from `count` clump spheres, each clump of cards its own rounded volume.
    Clumps { count: u32 },
    /// The gradient of a smooth field over `count` clumps: one soft volume.
    Hull { count: u32 },
    /// Straight up: blades light like the ground.
    Up,
}

/// A foliage bake that can't run.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum FoliageError {
    #[error("the foliage settings name material slot {name:?}, but the source's slots are {}", slots.join(", "))]
    UnknownSlot { name: String, slots: Vec<String> },
}

/// Transfers volume normals onto the slots `settings` names and computes every vertex's
/// [`FoliageVertex`] into [`ImportedMesh::foliage`] (see the [module docs](self)), welding pieces
/// at `weld_tolerance` (the mesh's outline weld tolerance). Runs after the outline bake (foliage
/// draws no outlines) and before optimization, which reorders the foliage data with the vertices.
///
/// # Panics
///
/// If an index is out of range; validate untrusted meshes first.
pub fn bake(
    mesh: &mut ImportedMesh,
    settings: &FoliageSettings,
    weld_tolerance: f32,
) -> Result<(), FoliageError> {
    let all = Extent::of(mesh.vertices.iter().map(|v| Vec3::from(v.position)));
    let transferred = transferred_vertices(mesh, &settings.slots)?;
    let floor = all.min.y + settings.canopy_from * all.size().y;
    let transferred = canopy_only(mesh, transferred, floor, weld_tolerance);
    let canopy = Extent::of(
        mesh.vertices.iter().zip(&transferred).filter(|(_, t)| **t).map(|(v, _)| Vec3::from(v.position)),
    );
    let volume = match settings.normals {
        NormalTransfer::Sphere => Volume::Ellipsoid,
        NormalTransfer::Up => Volume::Up,
        NormalTransfer::Clumps { count } => Volume::Clumps(clumps(mesh, &transferred, count, weld_tolerance)),
        NormalTransfer::Hull { count } => Volume::Hull(clumps(mesh, &transferred, count, weld_tolerance)),
    };
    transfer_normals(mesh, &transferred, &volume, &canopy, settings.blend);
    let phases = phases(mesh, &transferred, &volume, weld_tolerance);
    let up = settings.normals == NormalTransfer::Up;
    let reach = mesh.vertices.iter().map(|v| horizontal(Vec3::from(v.position)).length()).fold(0.0, f32::max);
    mesh.foliage = mesh
        .vertices
        .iter()
        .zip(&transferred)
        .zip(phases)
        .map(|((vertex, &leaf), phase)| {
            let p = Vec3::from(vertex.position);
            let height = ((p.y - all.min.y) / all.size().y.max(f32::EPSILON)).clamp(0.0, 1.0);
            let out = (horizontal(p).length() / reach.max(f32::EPSILON)).clamp(0.0, 1.0);
            FoliageVertex {
                sway: height * height,
                bend: if up { height } else { out * smoothstep(height / BEND_RISE) },
                flutter: match (leaf, up) {
                    (false, _) => 0.0,
                    (true, false) => 1.0,
                    (true, true) => height,
                },
                phase,
                gradient: if leaf && !up { canopy.depth(p) } else { height },
            }
        })
        .collect();
    Ok(())
}

/// `3x² − 2x³` of `x` held to 0..=1.
fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Replaces the transferred vertices' normals with the volume's, blended with their own by
/// `blend`. Each face of cards (see [`faces`]) turns its own normals toward the volume as one,
/// by the sign of their sum against the volume's.
fn transfer_normals(
    mesh: &mut ImportedMesh,
    transferred: &[bool],
    volume: &Volume,
    canopy: &Extent,
    blend: f32,
) {
    let targets: Vec<Vec3> = mesh
        .vertices
        .iter()
        .enumerate()
        .map(|(index, v)| {
            if transferred[index] { volume.normal(index, Vec3::from(v.position), canopy) } else { Vec3::ZERO }
        })
        .collect();
    let (face_of_vertex, faces) = faces(mesh, transferred);
    let mut facing = vec![0.0f32; faces];
    for (index, &face) in face_of_vertex.iter().enumerate() {
        if face != usize::MAX {
            facing[face] += Vec3::from(mesh.vertices[index].normal).dot(targets[index]);
        }
    }
    for (index, vertex) in mesh.vertices.iter_mut().enumerate() {
        if !transferred[index] {
            continue;
        }
        let target = targets[index];
        let source = Vec3::from(vertex.normal);
        // The face's sum decides; a vertex in no transferred triangle decides alone.
        let toward = match face_of_vertex[index] {
            usize::MAX => source.dot(target),
            face => facing[face],
        };
        let source = if toward < 0.0 { -source } else { source };
        let blended = target * blend + source * (1.0 - blend);
        vertex.normal = blended.try_normalize().unwrap_or(target).to_array();
    }
}

/// The faces of the transferred triangles: the triangles connected through shared vertices (not
/// welded by position, so a card modeled with its two sides apart is two faces). Each
/// transferred vertex's face, numbered in vertex order (`usize::MAX` for the rest), and how many
/// there are.
fn faces(mesh: &ImportedMesh, transferred: &[bool]) -> (Vec<usize>, usize) {
    let mut connected = UnionFind::new(mesh.vertices.len());
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|i| i as usize);
        if transferred[a] && transferred[b] && transferred[c] {
            connected.union(a, b);
            connected.union(a, c);
        }
    }
    number_groups(transferred, |index| connected.find(index))
}

/// Groups the vertices that are `members` by `root`, numbered in order of their first member;
/// the rest get `usize::MAX`. Returns each vertex's group and how many groups there are.
fn number_groups(members: &[bool], mut root: impl FnMut(usize) -> usize) -> (Vec<usize>, usize) {
    let mut group_of_root = BTreeMap::new();
    let groups = (0..members.len())
        .map(|index| {
            if !members[index] {
                return usize::MAX;
            }
            let next = group_of_root.len();
            *group_of_root.entry(root(index)).or_insert(next)
        })
        .collect();
    (groups, group_of_root.len())
}

/// The phase field (see the [module docs](self)): the azimuth around the vertical axis as a
/// fraction of a turn, plus [`PHASE_PER_METER`] a meter outward, wrapped.
fn phase_at(p: Vec3) -> f32 {
    (p.z.atan2(p.x) / TAU + PHASE_PER_METER * horizontal(p).length()).rem_euclid(1.0)
}

/// Each vertex's branch phase (see the [module docs](self)): a canopy clump's (the whole
/// canopy's for a sphere) at its centroid, a blade's at its centroid, a piece of wood around the
/// axis at its centroid, and other wood per vertex.
fn phases(mesh: &ImportedMesh, transferred: &[bool], volume: &Volume, weld_tolerance: f32) -> Vec<f32> {
    let positions: Vec<Vec3> = mesh.vertices.iter().map(|v| Vec3::from(v.position)).collect();
    let everything = vec![true; positions.len()];
    let (piece_of_vertex, pieces) = pieces(mesh, &everything, weld_tolerance);
    let pieces = Groups::of(&positions, &piece_of_vertex, pieces);
    let (clump_of_vertex, clumps) = match volume {
        Volume::Clumps(clumps) | Volume::Hull(clumps) => (clumps.of_vertex.clone(), clumps.clumps.len()),
        Volume::Ellipsoid => (transferred.iter().map(|&t| if t { 0 } else { usize::MAX }).collect(), 1),
        Volume::Up => (vec![usize::MAX; positions.len()], 0),
    };
    let clumps = Groups::of(&positions, &clump_of_vertex, clumps);
    positions
        .iter()
        .enumerate()
        .map(|(index, &p)| {
            let (clump, piece) = (clump_of_vertex[index], piece_of_vertex[index]);
            if clump != usize::MAX {
                phase_at(clumps.centroids[clump])
            } else if transferred[index] || pieces.around_axis[piece] {
                phase_at(pieces.centroids[piece])
            } else {
                phase_at(p)
            }
        })
        .collect()
}

/// Groups of vertices: each one's centroid, and whether it surrounds the vertical axis.
struct Groups {
    centroids: Vec<Vec3>,
    around_axis: Vec<bool>,
}

impl Groups {
    /// The `count` groups `group_of_vertex` numbers (`usize::MAX` for none) over `positions`.
    fn of(positions: &[Vec3], group_of_vertex: &[usize], count: usize) -> Self {
        // Per group: its positions' sum and count, and its directions from the axis' sum and
        // count (a vertex on the axis has none).
        let mut sums = vec![(Vec3::ZERO, 0.0f32, Vec3::ZERO, 0.0f32); count];
        for (&p, &group) in positions.iter().zip(group_of_vertex) {
            if group == usize::MAX {
                continue;
            }
            let sum = &mut sums[group];
            sum.0 += p;
            sum.1 += 1.0;
            if let Some(direction) = horizontal(p).try_normalize() {
                sum.2 += direction;
                sum.3 += 1.0;
            }
        }
        Self {
            centroids: sums.iter().map(|&(sum, n, ..)| sum / n.max(1.0)).collect(),
            around_axis: sums
                .iter()
                .map(|&(.., directions, n)| (directions / n.max(1.0)).length() < AROUND_AXIS)
                .collect(),
        }
    }
}

/// Per vertex, whether a triangle of LOD0 in one of `slots` (every slot when empty) uses it.
fn transferred_vertices(mesh: &ImportedMesh, slots: &[String]) -> Result<Vec<bool>, FoliageError> {
    let names: Vec<String> = mesh.materials.iter().map(|m| m.name.clone()).collect();
    if let Some(name) = slots.iter().find(|name| !names.contains(name)) {
        let slots = names.iter().map(|name| format!("{name:?}")).collect();
        return Err(FoliageError::UnknownSlot { name: name.clone(), slots });
    }
    let chosen: BTreeSet<u32> = (0..names.len() as u32)
        .filter(|&slot| slots.is_empty() || slots.contains(&names[slot as usize]))
        .collect();
    let mut transferred = vec![false; mesh.vertices.len()];
    for submesh in mesh.lods.first().map_or(&[][..], |lod| &lod.submeshes) {
        if !chosen.contains(&submesh.material) {
            continue;
        }
        let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
        for &index in &mesh.indices[range] {
            transferred[index as usize] = true;
        }
    }
    Ok(transferred)
}

/// `transferred` without the pieces whose lowest point is below `floor`: the trunk of a mesh that
/// models trunk and canopy in one slot ([`FoliageSettings::canopy_from`]).
fn canopy_only(mesh: &ImportedMesh, transferred: Vec<bool>, floor: f32, weld_tolerance: f32) -> Vec<bool> {
    if transferred.iter().zip(&mesh.vertices).all(|(&t, v)| !t || v.position[1] >= floor) {
        return transferred;
    }
    let (piece_of_vertex, pieces) = pieces(mesh, &transferred, weld_tolerance);
    let mut lowest = vec![f32::INFINITY; pieces];
    for (vertex, &piece) in mesh.vertices.iter().zip(&piece_of_vertex) {
        if piece != usize::MAX {
            lowest[piece] = lowest[piece].min(vertex.position[1]);
        }
    }
    piece_of_vertex.iter().map(|&piece| piece != usize::MAX && lowest[piece] >= floor).collect()
}

/// The connected pieces (cards, blobs, a trunk) of the triangles whose vertices are all
/// `transferred`, welded by position at `weld_tolerance`: each transferred vertex's piece,
/// numbered in vertex order (`usize::MAX` for the rest), and how many there are.
fn pieces(mesh: &ImportedMesh, transferred: &[bool], weld_tolerance: f32) -> (Vec<usize>, usize) {
    let (group_of, groups) = weld(&mesh.vertices, weld_tolerance);
    let mut connected = UnionFind::new(groups);
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|i| i as usize);
        if transferred[a] && transferred[b] && transferred[c] {
            connected.union(group_of[a], group_of[b]);
            connected.union(group_of[a], group_of[c]);
        }
    }
    number_groups(transferred, |index| connected.find(group_of[index]))
}

/// The horizontal part of `p`: its offset from the object's vertical axis.
fn horizontal(p: Vec3) -> Vec3 {
    Vec3::new(p.x, 0.0, p.z)
}

/// An axis-aligned box of points, and the ellipsoid inscribed in it.
#[derive(Clone, Copy, Debug)]
struct Extent {
    min: Vec3,
    max: Vec3,
}

impl Extent {
    fn of(points: impl Iterator<Item = Vec3>) -> Self {
        points
            .fold(None, |extent: Option<Self>, p| {
                Some(
                    extent.map_or(Self { min: p, max: p }, |e| Self { min: e.min.min(p), max: e.max.max(p) }),
                )
            })
            .unwrap_or(Self { min: Vec3::ZERO, max: Vec3::ZERO })
    }

    fn size(&self) -> Vec3 {
        self.max - self.min
    }

    fn center(&self) -> Vec3 {
        (self.min + self.max) / 2.0
    }

    /// The ellipsoid's semi-axes, none below [`MIN_EXTENT_RATIO`] of the largest.
    fn radii(&self) -> Vec3 {
        let half = self.size() / 2.0;
        half.max(Vec3::splat((half.max_element() * MIN_EXTENT_RATIO).max(1e-4)))
    }

    /// The ellipsoid's outward normal through `p`: the gradient of `|(p − c) / r|²`.
    fn normal(&self, p: Vec3) -> Vec3 {
        let radii = self.radii();
        ((p - self.center()) / (radii * radii)).try_normalize().unwrap_or(Vec3::Y)
    }

    /// How deep `p` is in the ellipsoid: 0 at its center, 1 on (and past) its surface.
    fn depth(&self, p: Vec3) -> f32 {
        ((p - self.center()) / self.radii()).length().min(1.0)
    }
}

/// A clump of cards: a sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Clump {
    center: Vec3,
    radius: f32,
}

/// The clumps a canopy's cards fall into, and which clump each vertex's card is in.
#[derive(Clone, Debug, PartialEq)]
struct Clumps {
    clumps: Vec<Clump>,
    /// Per vertex: its card's clump; `usize::MAX` for vertices not transferred.
    of_vertex: Vec<usize>,
}

enum Volume {
    Ellipsoid,
    Up,
    Clumps(Clumps),
    Hull(Clumps),
}

impl Volume {
    /// The volume's normal at `p`, vertex `index`'s position.
    fn normal(&self, index: usize, p: Vec3, canopy: &Extent) -> Vec3 {
        match self {
            Self::Ellipsoid => canopy.normal(p),
            Self::Up => Vec3::Y,
            Self::Clumps(clumps) => {
                let clump = clumps.clumps[clumps.of_vertex[index]];
                (p - clump.center).try_normalize().unwrap_or_else(|| canopy.normal(p))
            }
            Self::Hull(clumps) => field_normal(&clumps.clumps, p).unwrap_or_else(|| canopy.normal(p)),
        }
    }
}

/// The outward normal of the field `Σ exp(−|p − c|² / r²)` over `clumps` at `p`: its negated,
/// normalized gradient. Exponents are taken relative to the largest, so points far from every
/// clump keep their direction. `None` where the gradient vanishes (a point at a lone clump's
/// center).
fn field_normal(clumps: &[Clump], p: Vec3) -> Option<Vec3> {
    let exponent = |clump: &Clump| -(p - clump.center).length_squared() / (clump.radius * clump.radius);
    let top = clumps.iter().map(exponent).fold(f32::NEG_INFINITY, f32::max);
    clumps
        .iter()
        .map(|clump| (p - clump.center) * ((exponent(clump) - top).exp() / (clump.radius * clump.radius)))
        .sum::<Vec3>()
        .try_normalize()
}

/// Groups the transferred vertices' cards into at most `count` clumps: cards are the connected
/// pieces of the transferred triangles, welded by position; their centroids are clustered by
/// k-means from a farthest-point start, deterministically.
fn clumps(mesh: &ImportedMesh, transferred: &[bool], count: u32, weld_tolerance: f32) -> Clumps {
    // Each card's centroid, cards numbered in vertex order.
    let (card_of_vertex, cards) = pieces(mesh, transferred, weld_tolerance);
    let mut sums = vec![(Vec3::ZERO, 0.0f32); cards];
    for (vertex, &card) in mesh.vertices.iter().zip(&card_of_vertex) {
        if card != usize::MAX {
            sums[card].0 += Vec3::from(vertex.position);
            sums[card].1 += 1.0;
        }
    }
    let centroids: Vec<Vec3> = sums.iter().map(|(sum, n)| *sum / *n).collect();
    let card_clump = kmeans(&centroids, count as usize);
    let clump_count = card_clump.iter().copied().max().map_or(0, |max| max + 1);
    // Centers and RMS radii over the clump's vertices.
    let mut centers = vec![(Vec3::ZERO, 0.0f32); clump_count];
    for (index, &card) in card_of_vertex.iter().enumerate() {
        if card != usize::MAX {
            let clump = &mut centers[card_clump[card]];
            clump.0 += Vec3::from(mesh.vertices[index].position);
            clump.1 += 1.0;
        }
    }
    let centers: Vec<Vec3> = centers.iter().map(|(sum, n)| *sum / n.max(1.0)).collect();
    let mut spread = vec![(0.0f32, 0.0f32); clump_count];
    for (index, &card) in card_of_vertex.iter().enumerate() {
        if card != usize::MAX {
            let clump = card_clump[card];
            spread[clump].0 += Vec3::from(mesh.vertices[index].position).distance_squared(centers[clump]);
            spread[clump].1 += 1.0;
        }
    }
    let floor = (Extent::of(
        mesh.vertices.iter().zip(transferred).filter(|(_, t)| **t).map(|(v, _)| Vec3::from(v.position)),
    )
    .size()
    .max_element()
        * MIN_EXTENT_RATIO)
        .max(1e-4);
    let clumps = centers
        .iter()
        .zip(&spread)
        .map(|(&center, &(sum, n))| Clump {
            center,
            radius: ((sum / n.max(1.0)).sqrt() * CLUMP_RADIUS_SCALE).max(floor),
        })
        .collect();
    let of_vertex = card_of_vertex
        .iter()
        .map(|&card| if card == usize::MAX { usize::MAX } else { card_clump[card] })
        .collect();
    Clumps { clumps, of_vertex }
}

/// The cluster of each of `points` after k-means with at most `k` clusters, started from
/// farthest-point sampling (the point farthest from the centroid, then each next farthest from
/// those chosen; ties to the lower index). Clusters are numbered by their first point, and
/// empty ones dropped, so the result is deterministic.
fn kmeans(points: &[Vec3], k: usize) -> Vec<usize> {
    if points.is_empty() {
        return Vec::new();
    }
    let k = k.clamp(1, points.len());
    let centroid = points.iter().sum::<Vec3>() / points.len() as f32;
    let farthest_from = |chosen: &[Vec3]| {
        let distance = |p: Vec3| chosen.iter().map(|c| c.distance_squared(p)).fold(f32::INFINITY, f32::min);
        (0..points.len())
            .fold(0, |best, i| if distance(points[i]) > distance(points[best]) { i } else { best })
    };
    let mut centers = vec![points[farthest_from(&[centroid])]];
    while centers.len() < k {
        let next = points[farthest_from(&centers)];
        centers.push(next);
    }
    let nearest = |centers: &[Vec3], p: Vec3| {
        (0..centers.len()).fold(0, |best, i| {
            if centers[i].distance_squared(p) < centers[best].distance_squared(p) { i } else { best }
        })
    };
    let mut assignment: Vec<usize> = points.iter().map(|&p| nearest(&centers, p)).collect();
    for _ in 0..KMEANS_ITERATIONS {
        let mut sums = vec![(Vec3::ZERO, 0.0f32); centers.len()];
        for (p, &cluster) in points.iter().zip(&assignment) {
            sums[cluster].0 += *p;
            sums[cluster].1 += 1.0;
        }
        for (center, (sum, n)) in centers.iter_mut().zip(&sums) {
            if *n > 0.0 {
                *center = *sum / *n;
            }
        }
        let next: Vec<usize> = points.iter().map(|&p| nearest(&centers, p)).collect();
        if next == assignment {
            break;
        }
        assignment = next;
    }
    // Renumber by first appearance, dropping empty clusters.
    let mut renumber = vec![usize::MAX; centers.len()];
    let mut next = 0;
    for cluster in &mut assignment {
        if renumber[*cluster] == usize::MAX {
            renumber[*cluster] = next;
            next += 1;
        }
        *cluster = renumber[*cluster];
    }
    assignment
}

/// Disjoint sets with path halving and union by index (the lower root wins), so the result
/// doesn't depend on the order of unions.
struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(len: usize) -> Self {
        Self((0..len).collect())
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        let (low, high) = (a.min(b), a.max(b));
        self.0[high] = low;
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::mesh::{Lod, Submesh, Vertex};

    use crate::bake::DEFAULT_WELD_TOLERANCE;

    use super::*;

    /// The bake at the default weld tolerance, as most tests' meshes need.
    fn bake(mesh: &mut ImportedMesh, settings: &FoliageSettings) -> Result<(), FoliageError> {
        super::bake(mesh, settings, DEFAULT_WELD_TOLERANCE)
    }
    use crate::test_util::material;

    /// A square card of side `size` at `center` facing `normal`, two-sided (back faces as a
    /// second pair of triangles over the same four vertices).
    fn card(mesh: &mut ImportedMesh, center: Vec3, normal: Vec3, size: f32) {
        let normal = normal.normalize();
        let tangent = normal.any_orthonormal_vector();
        let bitangent = normal.cross(tangent);
        let first = mesh.vertices.len() as u32;
        for (u, v) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            let position = center + (tangent * u + bitangent * v) * size;
            mesh.vertices.push(Vertex {
                position: position.to_array(),
                normal: normal.to_array(),
                outline_normal: normal.to_array(),
                uv: [u + 0.5, 0.5 - v],
                ..Vertex::default()
            });
        }
        mesh.indices.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|i| first + i));
    }

    /// `cards` meshes each in its own slot, then a trunk of one triangle in slot "Bark".
    fn tree(cards: &[(Vec3, Vec3)]) -> ImportedMesh {
        let mut mesh = ImportedMesh::default();
        for &(center, normal) in cards {
            card(&mut mesh, center, normal, 0.3);
        }
        let leaves = mesh.indices.len() as u32;
        let first = mesh.vertices.len() as u32;
        for position in [[-0.1, 0.0, 0.0], [0.1, 0.0, 0.0], [0.0, 2.0, 0.0]] {
            mesh.vertices.push(Vertex { position, normal: [0.0, 0.0, 1.0], ..Vertex::default() });
        }
        mesh.indices.extend([first, first + 1, first + 2]);
        let vertex_count = mesh.vertices.len() as u32;
        mesh.lods = vec![Lod {
            vertex_count,
            error: 0.0,
            submeshes: vec![
                Submesh { first_index: 0, index_count: leaves, material: 0 },
                Submesh { first_index: leaves, index_count: 3, material: 1 },
            ],
        }];
        mesh.materials = vec![material("Leaves"), material("Bark")];
        mesh.authored_outline = vec![false; mesh.vertices.len()];
        mesh
    }

    fn settings(normals: NormalTransfer, blend: f32) -> FoliageSettings {
        FoliageSettings { normals, slots: vec!["Leaves".into()], blend, canopy_from: 0.0 }
    }

    /// Cards on a sphere of radius 1 around (0, 2, 0), each facing a random way.
    fn sphere_canopy() -> Vec<(Vec3, Vec3)> {
        let mut cards = Vec::new();
        let mut seed = 7u32;
        let mut random = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        for _ in 0..60 {
            let (z, angle) = (random() * 2.0 - 1.0, random() * TAU);
            let r = (1.0 - z * z).sqrt();
            let direction = Vec3::new(r * angle.cos(), z, r * angle.sin());
            let facing = Vec3::new(random() - 0.5, random() - 0.5, random() - 0.5);
            cards.push((Vec3::new(0.0, 2.0, 0.0) + direction, facing));
        }
        cards
    }

    #[test]
    fn a_sphere_fit_gives_radial_normals() {
        let mut mesh = tree(&sphere_canopy());
        let trunk_normals: Vec<_> =
            mesh.vertices[mesh.vertices.len() - 3..].iter().map(|v| v.normal).collect();
        bake(&mut mesh, &settings(NormalTransfer::Sphere, 1.0)).unwrap();
        let canopy =
            Extent::of(mesh.vertices[..mesh.vertices.len() - 3].iter().map(|v| Vec3::from(v.position)));
        let center = canopy.center();
        assert!(center.distance(Vec3::new(0.0, 2.0, 0.0)) < 0.15, "{center}");
        // Every leaf vertex faces away from the canopy's center (exactly radial for a round
        // canopy; the fit's ellipsoid is within a few percent of round).
        let mut worst = 0.0f32;
        for vertex in &mesh.vertices[..mesh.vertices.len() - 3] {
            let radial = (Vec3::from(vertex.position) - center).normalize();
            worst = worst.max(Vec3::from(vertex.normal).angle_between(radial).to_degrees());
        }
        assert!(worst < 8.0, "worst {worst}° from radial");
        // The trunk keeps its normals.
        let after: Vec<_> = mesh.vertices[mesh.vertices.len() - 3..].iter().map(|v| v.normal).collect();
        assert_eq!(after, trunk_normals);
    }

    #[test]
    fn the_blend_keeps_a_share_of_each_cards_facing_on_both_sides() {
        // Six cards around (0, 2, 0), each facing along the canopy's surface rather than out of
        // it: the volume normal is outward, and each card keeps a fifth of its own facing, its
        // back faces (the same vertices) turned toward the volume first.
        let cards = [
            (Vec3::new(0.5, 2.0, 0.0), Vec3::Z),
            (Vec3::new(-0.5, 2.0, 0.0), -Vec3::Z),
            (Vec3::new(0.0, 2.5, 0.0), Vec3::X),
            (Vec3::new(0.0, 1.5, 0.0), -Vec3::X),
            (Vec3::new(0.0, 2.0, 0.5), Vec3::Y),
            (Vec3::new(0.0, 2.0, -0.5), -Vec3::Y),
        ];
        let mut mesh = tree(&cards);
        bake(&mut mesh, &settings(NormalTransfer::Sphere, 0.8)).unwrap();
        for (card, &(center, facing)) in cards.iter().enumerate() {
            let outward = (center - Vec3::new(0.0, 2.0, 0.0)).normalize();
            for vertex in &mesh.vertices[card * 4..card * 4 + 4] {
                let n = Vec3::from(vertex.normal);
                assert!((n.length() - 1.0).abs() < 1e-5, "{n}");
                assert!(n.dot(outward) > 0.8, "card {card}: {n} is mostly the volume's");
                assert!(n.dot(facing).abs() > 0.1, "card {card}: {n} keeps some of its own facing");
            }
        }
    }

    #[test]
    fn the_hull_is_smooth_across_cards_where_clumps_split() {
        // Two clumps of cards side by side, 0.6 m apart where they meet.
        let mut cards = Vec::new();
        for (i, x) in [-1.2f32, -0.9, -0.6, -0.3, 0.3, 0.6, 0.9, 1.2].iter().enumerate() {
            for y in [1.7f32, 2.0, 2.3] {
                cards.push((Vec3::new(*x, y, 0.1 * (i % 3) as f32), Vec3::new(0.3, 1.0, 0.2)));
            }
        }
        let normals = |transfer| {
            let mut mesh = tree(&cards);
            bake(&mut mesh, &settings(transfer, 1.0)).unwrap();
            mesh.vertices.iter().map(|v| (Vec3::from(v.position), Vec3::from(v.normal))).collect::<Vec<_>>()
        };
        // The largest angle between the normals of leaf vertices facing each other across the
        // gap between the clumps (the inner cards' corners, 0.3 m apart).
        let across = |vertices: &[(Vec3, Vec3)]| {
            let side = |sign: f32| vertices.iter().filter(move |(p, _)| p.x * sign > 0.0 && p.x * sign < 0.2);
            side(-1.0)
                .flat_map(|(p, n)| {
                    side(1.0)
                        .filter(move |(q, _)| p.distance(*q) < 0.35)
                        .map(move |(_, m)| n.angle_between(*m))
                })
                .fold(0.0f32, f32::max)
                .to_degrees()
        };
        let hull = across(&normals(NormalTransfer::Hull { count: 2 }));
        let split = across(&normals(NormalTransfer::Clumps { count: 2 }));
        // Each clump is a sphere of its own, so normals face apart across the gap; the hull's
        // field turns them through it.
        assert!(split > 140.0 && hull < split - 30.0, "clumps {split}°, hull {hull}°");

        // And the field is continuous: along a line through both clumps, 1 cm steps turn the
        // normal by under 3°.
        let mesh = tree(&cards);
        let transferred = transferred_vertices(&mesh, &["Leaves".to_owned()]).unwrap();
        let fitted = clumps(&mesh, &transferred, 2, DEFAULT_WELD_TOLERANCE).clumps;
        assert_eq!(fitted.len(), 2);
        let mut previous = field_normal(&fitted, Vec3::new(-1.5, 2.45, 0.1)).unwrap();
        for step in 1..=300 {
            let p = Vec3::new(-1.5 + 0.01 * step as f32, 2.45, 0.1);
            let n = field_normal(&fitted, p).unwrap();
            assert!(n.angle_between(previous).to_degrees() < 3.0, "at {p}: {previous} to {n}");
            previous = n;
        }
    }

    #[test]
    fn clumps_split_a_canopy_and_each_lights_as_its_own_ball() {
        let mut cards = Vec::new();
        for center in [Vec3::new(-2.0, 2.0, 0.0), Vec3::new(2.0, 2.0, 0.0)] {
            for offset in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
                cards.push((center + offset * 0.5, offset));
            }
        }
        let mut mesh = tree(&cards);
        bake(&mut mesh, &settings(NormalTransfer::Clumps { count: 2 }, 1.0)).unwrap();
        for (card, &(center, offset)) in cards.iter().enumerate() {
            let ball = if center.x < 0.0 { Vec3::new(-2.0, 2.0, 0.0) } else { Vec3::new(2.0, 2.0, 0.0) };
            for vertex in &mesh.vertices[card * 4..card * 4 + 4] {
                let radial = (Vec3::from(vertex.position) - ball).normalize();
                let n = Vec3::from(vertex.normal);
                assert!(
                    n.angle_between(radial).to_degrees() < 1.0,
                    "card {card} ({offset}): {n} vs {radial}"
                );
            }
        }
    }

    #[test]
    fn grass_faces_up_and_its_tips_bend_and_flutter() {
        let mut mesh = ImportedMesh::default();
        for x in [-0.5f32, 0.0, 0.5] {
            card(&mut mesh, Vec3::new(x, 0.15, 0.0), Vec3::Z, 0.3);
        }
        let vertex_count = mesh.vertices.len() as u32;
        let submesh = Submesh { first_index: 0, index_count: mesh.indices.len() as u32, material: 0 };
        mesh.lods = vec![Lod { vertex_count, error: 0.0, submeshes: vec![submesh] }];
        mesh.materials = vec![material("Grass")];
        let settings =
            FoliageSettings { normals: NormalTransfer::Up, slots: Vec::new(), blend: 1.0, canopy_from: 0.0 };
        bake(&mut mesh, &settings).unwrap();
        for (vertex, data) in mesh.vertices.iter().zip(&mesh.foliage) {
            assert_eq!(vertex.normal, [0.0, 1.0, 0.0]);
            let tip = vertex.position[1] > 0.2;
            let expected = if tip { 1.0 } else { 0.0 };
            assert_eq!(
                (data.sway, data.bend, data.flutter, data.gradient),
                (expected, expected, expected, expected)
            );
        }
    }

    #[test]
    fn wood_sways_with_its_leaves_and_never_flutters() {
        let mut mesh = tree(&sphere_canopy());
        bake(&mut mesh, &settings(NormalTransfer::Sphere, 1.0)).unwrap();
        assert_eq!(mesh.foliage.len(), mesh.vertices.len());
        let trunk = &mesh.foliage[mesh.foliage.len() - 3..];
        assert_eq!(trunk.iter().map(|f| f.flutter).collect::<Vec<_>>(), [0.0; 3]);
        assert_eq!((trunk[0].sway, trunk[0].gradient), (0.0, 0.0), "the root holds still");
        let top = mesh.vertices.iter().map(|v| v.position[1]).fold(0.0, f32::max);
        assert!((trunk[2].sway - (2.0 / top).powi(2)).abs() < 1e-5);
        let leaves = &mesh.foliage[..mesh.foliage.len() - 3];
        assert!(leaves.iter().all(|f| f.flutter == 1.0 && (0.0..1.0).contains(&f.phase)));
        // Leaves on the canopy's surface take the gradient's tip, the inner ones less.
        assert!(leaves.iter().all(|f| f.gradient > 0.6), "cards lie near the surface");
        // A sphere's canopy sways as one: every card takes one phase, so it doesn't breathe.
        assert!(leaves.iter().all(|f| f.phase == leaves[0].phase), "one phase for the canopy");
        // Vertices at one position sway and bend alike, a card's and its wood's included.
        let mut split = tree(&[(Vec3::new(0.7, 2.0, 0.0), Vec3::Z)]);
        let duplicate = split.vertices[0];
        split.vertices.push(duplicate);
        split.authored_outline.push(false);
        bake(&mut split, &settings(NormalTransfer::Sphere, 1.0)).unwrap();
        let (first, last) = (split.foliage[0], *split.foliage.last().unwrap());
        assert_eq!((first.sway, first.bend), (last.sway, last.bend));
    }

    #[test]
    fn a_card_edge_on_to_the_volume_flips_as_one() {
        // A flat canopy (wide and low, so its ellipsoid's normals turn fast), and a card on its
        // side turned edge-on to it: two of its corners face the volume's normals and two face
        // away. With no blend the card keeps its own normal, turned toward the volume by the
        // card's sum, so all four corners keep one normal.
        let tilted = Vec3::new(0.93, -0.36, 0.0).normalize();
        let mut cards: Vec<(Vec3, Vec3)> = [
            Vec3::new(2.0, 2.0, 0.0),
            Vec3::new(-2.0, 2.0, 0.0),
            Vec3::new(0.0, 2.0, 2.0),
            Vec3::new(0.0, 2.0, -2.0),
            Vec3::new(0.0, 2.5, 0.0),
            Vec3::new(0.0, 1.5, 0.0),
        ]
        .map(|center| (center, Vec3::Y))
        .to_vec();
        let first = cards.len() * 4;
        cards.push((Vec3::new(1.0, 2.2, 0.0), tilted));
        let mut mesh = tree(&cards);
        let canopy = Extent::of(mesh.vertices[..first + 4].iter().map(|v| Vec3::from(v.position)));
        let facing: Vec<f32> = (first..first + 4)
            .map(|i| tilted.dot(canopy.normal(Vec3::from(mesh.vertices[i].position))))
            .collect();
        assert!(facing.iter().any(|&d| d > 0.0) && facing.iter().any(|&d| d < 0.0), "mixed: {facing:?}");
        bake(&mut mesh, &settings(NormalTransfer::Sphere, 0.0)).unwrap();
        let normals: Vec<Vec3> =
            mesh.vertices[first..first + 4].iter().map(|v| Vec3::from(v.normal)).collect();
        let sum: f32 = facing.iter().sum();
        let expected = if sum < 0.0 { -tilted } else { tilted };
        assert!(normals.iter().all(|n| n.abs_diff_eq(expected, 1e-5)), "one flip for the card: {normals:?}");
    }

    #[test]
    fn pieces_weld_at_the_mesh_s_own_tolerance() {
        // A kit's tree in one slot: a blob canopy, and a trunk in two segments whose seam drifts
        // 0.5 mm apart. At 1 mm (the mesh's weld tolerance) the upper segment is welded to its
        // foot and stays wood; at the default 0.1 mm it starts high and turns into canopy.
        let tree = || {
            let mut mesh = blob_tree();
            mesh.vertices.truncate(6);
            mesh.indices.truncate(24);
            let mut solid = |corners: &[Vec3], faces: &[[u32; 3]]| {
                let first = mesh.vertices.len() as u32;
                for &corner in corners {
                    let normal = Vec3::new(corner.x, 0.0, corner.z).normalize_or(Vec3::Y);
                    mesh.vertices.push(Vertex {
                        position: corner.to_array(),
                        normal: normal.to_array(),
                        outline_normal: normal.to_array(),
                        ..Vertex::default()
                    });
                }
                mesh.indices.extend(faces.iter().flatten().map(|&i| first + i));
            };
            let ring = |y: f32, drift: f32| {
                [
                    Vec3::new(0.12 + drift, y, 0.0),
                    Vec3::new(-0.08 + drift, y, 0.1),
                    Vec3::new(-0.08 + drift, y, -0.1),
                ]
            };
            let [a, b, c] = ring(0.0, 0.0);
            let [d, e, f] = ring(0.7, 0.0);
            let prism =
                [[0, 2, 1], [3, 4, 5], [0, 1, 4], [0, 4, 3], [1, 2, 5], [1, 5, 4], [2, 0, 3], [2, 3, 5]];
            solid(&[a, b, c, d, e, f], &prism);
            let [g, h, i] = ring(0.7, 5e-4);
            solid(&[g, h, i, Vec3::new(0.0, 1.4, 0.0)], &[[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]]);
            mesh.lods[0].vertex_count = mesh.vertices.len() as u32;
            mesh.lods[0].submeshes[0].index_count = mesh.indices.len() as u32;
            mesh.authored_outline = vec![false; mesh.vertices.len()];
            mesh
        };
        let settings =
            FoliageSettings { slots: Vec::new(), blend: 1.0, canopy_from: 0.1, ..FoliageSettings::default() };
        let mut welded = tree();
        let upper = welded.vertices.len() - 4..welded.vertices.len();
        super::bake(&mut welded, &settings, 1e-3).unwrap();
        assert!(welded.foliage[upper.clone()].iter().all(|f| f.flutter == 0.0), "welded to its foot: wood");
        let mut apart = tree();
        super::bake(&mut apart, &settings, DEFAULT_WELD_TOLERANCE).unwrap();
        assert!(apart.foliage[upper].iter().all(|f| f.flutter == 1.0), "apart: canopy");
    }

    /// A kit's tree: a closed blob canopy (an octahedron of radius 1 around (0, 2, 0)) on a
    /// trunk (a closed tetrahedron from the ground to 1.4 m), in one slot.
    fn blob_tree() -> ImportedMesh {
        let mut mesh = ImportedMesh::default();
        let mut solid = |corners: &[Vec3], faces: &[[u32; 3]]| {
            let first = mesh.vertices.len() as u32;
            let center = corners.iter().sum::<Vec3>() / corners.len() as f32;
            for &corner in corners {
                let normal = (corner - center).normalize();
                mesh.vertices.push(Vertex {
                    position: corner.to_array(),
                    normal: normal.to_array(),
                    outline_normal: normal.to_array(),
                    ..Vertex::default()
                });
            }
            mesh.indices.extend(faces.iter().flatten().map(|&i| first + i));
        };
        let crown = Vec3::new(0.0, 2.0, 0.0);
        let octahedron =
            [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z].map(|d| crown + d);
        let faces = [[0, 2, 4], [4, 2, 1], [1, 2, 5], [5, 2, 0], [4, 3, 0], [1, 3, 4], [5, 3, 1], [0, 3, 5]];
        solid(&octahedron, &faces);
        let trunk = [
            Vec3::new(0.15, 0.0, 0.0),
            Vec3::new(-0.1, 0.0, 0.12),
            Vec3::new(-0.1, 0.0, -0.12),
            Vec3::new(0.0, 1.4, 0.0),
        ];
        solid(&trunk, &[[0, 1, 2], [0, 3, 1], [1, 3, 2], [2, 3, 0]]);
        let vertex_count = mesh.vertices.len() as u32;
        let index_count = mesh.indices.len() as u32;
        mesh.lods = vec![Lod {
            vertex_count,
            error: 0.0,
            submeshes: vec![Submesh { first_index: 0, index_count, material: 0 }],
        }];
        mesh.materials = vec![material("colormap")];
        mesh.authored_outline = vec![false; mesh.vertices.len()];
        mesh
    }

    #[test]
    fn a_blob_canopy_in_its_trunks_slot_rounds_off_and_its_trunk_stays_wood() {
        let mut mesh = blob_tree();
        let trunk = 6..10;
        let before: Vec<[f32; 3]> = mesh.vertices[trunk.clone()].iter().map(|v| v.normal).collect();
        let settings =
            FoliageSettings { slots: Vec::new(), blend: 1.0, canopy_from: 0.1, ..FoliageSettings::default() };
        bake(&mut mesh, &settings).unwrap();
        // The canopy's normals are its own ball's, centered on the crown, not the whole tree's.
        for vertex in &mesh.vertices[..6] {
            let radial = (Vec3::from(vertex.position) - Vec3::new(0.0, 2.0, 0.0)).normalize();
            assert!(Vec3::from(vertex.normal).dot(radial) > 0.999, "{vertex:?}");
        }
        assert!(mesh.foliage[..6].iter().all(|f| f.flutter == 1.0), "leaves flutter");
        // The trunk keeps its normals, never flutters, and its foot holds still.
        let after: Vec<[f32; 3]> = mesh.vertices[trunk.clone()].iter().map(|v| v.normal).collect();
        assert_eq!(after, before);
        assert!(mesh.foliage[trunk.clone()].iter().all(|f| f.flutter == 0.0));
        assert!(mesh.foliage[6..9].iter().all(|f| f.sway == 0.0));
        // Without `canopy_from`, the slot's every piece is foliage, the trunk too.
        let mut whole = blob_tree();
        bake(&mut whole, &FoliageSettings { canopy_from: 0.0, ..settings }).unwrap();
        assert!(whole.foliage[trunk].iter().all(|f| f.flutter == 1.0));
    }

    #[test]
    fn unknown_slots_are_errors_and_settings_are_checked() {
        let mut mesh = tree(&sphere_canopy());
        let error =
            bake(&mut mesh, &FoliageSettings { slots: vec!["Leaf".into()], ..FoliageSettings::default() })
                .unwrap_err();
        assert_eq!(
            error,
            FoliageError::UnknownSlot {
                name: "Leaf".into(),
                slots: vec!["\"Leaves\"".into(), "\"Bark\"".into()]
            }
        );
        assert!(FoliageSettings { blend: 1.5, ..FoliageSettings::default() }.validate().is_err());
        assert!(FoliageSettings { canopy_from: -0.1, ..FoliageSettings::default() }.validate().is_err());
        let zero =
            FoliageSettings { normals: NormalTransfer::Hull { count: 0 }, ..FoliageSettings::default() };
        assert!(zero.validate().is_err());
        let many =
            FoliageFields { normals: NormalTransfer::Clumps { count: 99 }, ..FoliageFields::default() };
        assert!(FoliageSettings::try_from(many).is_err());
        assert!(FoliageSettings::default().validate().is_ok());
    }

    #[test]
    fn kmeans_is_deterministic_and_drops_empty_clusters() {
        let points = [Vec3::ZERO, Vec3::X * 0.1, Vec3::X * 10.0, Vec3::X * 10.1];
        assert_eq!(kmeans(&points, 2), [0, 0, 1, 1]);
        assert_eq!(kmeans(&points, 9), [0, 1, 2, 3], "no more clusters than points");
        assert_eq!(kmeans(&points, 1), [0; 4]);
        assert!(kmeans(&[], 3).is_empty());
    }
}
