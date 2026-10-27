//! GPU-friendly vertex and triangle order, and the LOD chain (spec §5.27), with
//! meshoptimizer.
//!
//! [`optimize`] runs after the outline bake, once per cook:
//!
//! 1. **Submeshes merge by material**, so each material is one draw.
//! 2. **The LOD chain** ([`LodSettings`]): each level simplifies the last one kept, submesh by
//!    submesh, with meshoptimizer's attribute-aware simplifier. It is deformation-aware (the
//!    review's AP-005), since the mobile cook draws a simplified level as its LOD0:
//!    - **Attributes** weigh in beside positions: normals, because the two-tone light split is
//!      a hard threshold on them, and a skinned mesh's positions in two test poses (every joint
//!      turned 45° from its parent, about X and about Z), so a level keeps the shape of bent
//!      joints as well as of the rest pose: where the weights change, and the blend's curve
//!      between. Vertex color doesn't weigh in: at a normal's weight it cost the far mountains a
//!      tenth more triangles on phones, so a painted spot off every morph can still simplify
//!      away.
//!    - **Locks:** every vertex a morph target moves never moves, and neither do its
//!      neighbors, so every triangle a target deforms stays LOD0's own at every level and the
//!      rest of the mesh doesn't deform: at any weight a level strays from LOD0 only as far as
//!      at rest. Vertices that two submeshes share (a material seam) are locked, so simplifying
//!      the submeshes independently can't open cracks between them. A sidecar's `lock_border`
//!      locks the mesh's open border as well, for a mesh cut from a larger surface whose edges
//!      must meet its neighbors' at every level (X1's far mountains, cut into sectors).
//!    - **Seams** where a vertex is split (hard normals, texture seams, material IDs) collapse
//!      only along themselves, so both sides stay sealed: meshoptimizer's own rule.
//!
//!    A level stops at its triangle target or its error budget, whichever comes first, and is
//!    dropped if it saves less than [`MIN_LOD_REDUCTION`] of the last kept level's triangles,
//!    or has none left. Since each level simplifies the one before, every LOD's vertices are a
//!    subset of the previous LOD's, and its error is the sum of the steps: an upper bound on
//!    its distance from LOD0. meshoptimizer's error estimate includes the weighted attributes'
//!    deviation, so the errors (and the budgets they are held to) are conservative as
//!    distances. A part that simplifies away (a speck smaller than the budget) still counts
//!    toward its level's error.
//! 3. **The cook picks its levels.** The mobile cook drops the first
//!    [`LodSettings::mobile_skip`] levels, so its LOD0 is lower-detail and it has fewer LODs.
//! 4. **Triangles are reordered** in every LOD for the post-transform vertex cache, then for
//!    overdraw, allowing the cache 5% worse use (meshoptimizer's recommendation).
//! 5. **Vertices are reordered** in the order the coarsest LOD first uses them, then the next
//!    finer one, and so on, so every LOD reads a prefix of the vertex streams (see the cooked
//!    format) and fetches within it are close to sequential. The price is some locality for
//!    LOD0, whose own vertices follow the coarser LODs'. Vertices no kept LOD uses are
//!    dropped; skin weights, morph deltas and foliage data follow the reorder.
//!
//! Every step is deterministic, so a cook of the same input writes the same bytes.

use glam::{Mat3, Mat4, Vec3};
use meshopt::{SimplifyOptions, VertexDataAdapter};
use sapphire_assets::mesh::{Lod, Submesh};
use serde::{Deserialize, Serialize};

use crate::bake;
use crate::cook::Cook;
use crate::mesh::{ImportedMesh, ImportedSkin};

/// A level must cut the triangle count by at least this fraction of the last kept level's,
/// or it is dropped: a smaller saving isn't worth a LOD switch.
pub const MIN_LOD_REDUCTION: f32 = 0.1;

/// meshoptimizer's overdraw optimizer may make vertex-cache use this much worse.
const OVERDRAW_THRESHOLD: f32 = 1.05;

/// Weight of each normal component in the simplifier's error, relative to position error as a
/// fraction of the mesh's extent. meshoptimizer's guidance for normals is 0.5–1.
const NORMAL_WEIGHT: f32 = 0.5;

/// How far each joint turns from its parent in the test poses a skinned mesh simplifies in
/// (radians). A level's error bounds how far its surface strays from LOD0's in them, so it
/// holds bends up to about this much within its budget; sharper ones stray further, about as
/// the bends' chords (`2 sin(θ / 2)`) grow.
const POSE_ANGLE: f32 = std::f32::consts::FRAC_PI_4;

/// The axes the test poses turn every joint about, one pose each: limbs along Y (legs, spine,
/// hair) bend both ways, and limbs along X (arms in a T pose) bend and twist.
const POSE_AXES: [Vec3; 2] = [Vec3::X, Vec3::Z];

/// Weight of each coordinate of a vertex's position in a test pose, as a fraction of the
/// extent as position error's is, over the mesh's typical vertex spacing ([`spacing`]): a
/// pose's surface then counts as the rest surface does.
const POSE_WEIGHT: f32 = 1.0;

/// LOD generation settings, part of a mesh's import settings
/// ([`MeshSettings`](crate::meta::MeshSettings)). Loading checks them as [`Self::new`] does,
/// and a sidecar may leave any of them out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "LodFields")]
pub struct LodSettings {
    levels: Vec<LodLevel>,
    mobile_skip: u32,
    /// Written only when set, so sidecars without it read and write as they did.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    lock_border: bool,
}

/// One simplified level of the chain.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LodLevel {
    /// The triangle count to aim for, as a fraction of LOD0's, in (0, 1).
    pub triangle_ratio: f32,
    /// The most the level may deviate from LOD0, accumulated over the chain, as a fraction of
    /// the mesh's extent (the longest side of its bounds).
    pub max_error: f32,
}

/// The mobile cook drops this many levels unless a sidecar says otherwise, or has fewer.
const DEFAULT_MOBILE_SKIP: u32 = 1;

/// [`LodSettings`] as written, before it is checked.
#[derive(Deserialize)]
#[serde(default)]
struct LodFields {
    levels: Vec<LodLevel>,
    /// Left out: [`DEFAULT_MOBILE_SKIP`], or every level if there are fewer, so `levels: []`
    /// alone turns LODs off on both cooks.
    mobile_skip: Option<u32>,
    lock_border: bool,
}

impl Default for LodFields {
    fn default() -> Self {
        Self { levels: LodSettings::default().levels, mobile_skip: None, lock_border: false }
    }
}

impl TryFrom<LodFields> for LodSettings {
    type Error = &'static str;

    fn try_from(fields: LodFields) -> Result<Self, Self::Error> {
        let levels = u32::try_from(fields.levels.len()).unwrap_or(u32::MAX);
        let mobile_skip = fields.mobile_skip.unwrap_or(DEFAULT_MOBILE_SKIP.min(levels));
        let settings =
            Self::new(fields.levels, mobile_skip).map(|settings| settings.lock_border(fields.lock_border));
        settings.ok_or(
            "LOD triangle_ratio must be in (0, 1) and decrease level by level, max_error must be \
             finite, at least 0 and never decrease, and mobile_skip can't exceed the levels",
        )
    }
}

impl Default for LodSettings {
    /// Three levels at ½, ¼ and ⅛ of the triangles, within 0.2%, 1% and 3% of the extent (4 mm,
    /// 2 cm and 6 cm on a 2 m character), and a mobile cook that starts at the first.
    fn default() -> Self {
        let level = |triangle_ratio, max_error| LodLevel { triangle_ratio, max_error };
        Self {
            levels: vec![level(0.5, 0.002), level(0.25, 0.01), level(0.125, 0.03)],
            mobile_skip: DEFAULT_MOBILE_SKIP,
            lock_border: false,
        }
    }
}

impl LodSettings {
    /// Settings with these levels, after LOD0, and the number the mobile cook drops. Returns
    /// `None` unless every ratio is in (0, 1) and below the previous level's, every error is
    /// finite, at least 0 and at least the previous level's, and `mobile_skip` is at most the
    /// number of levels.
    pub fn new(levels: Vec<LodLevel>, mobile_skip: u32) -> Option<Self> {
        let mut previous = LodLevel { triangle_ratio: 1.0, max_error: 0.0 };
        for &level in &levels {
            let ratio_ok = level.triangle_ratio > 0.0 && level.triangle_ratio < previous.triangle_ratio;
            let error_ok = level.max_error.is_finite() && level.max_error >= previous.max_error;
            if !(ratio_ok && error_ok) {
                return None;
            }
            previous = level;
        }
        (mobile_skip as usize <= levels.len()).then_some(Self { levels, mobile_skip, lock_border: false })
    }

    /// No LODs: LOD0 alone, on both cooks.
    pub fn none() -> Self {
        Self { levels: Vec::new(), mobile_skip: 0, lock_border: false }
    }

    /// These settings with the mesh's open border locked at every level (`lock`) or free to
    /// simplify along itself (the default).
    pub fn lock_border(self, lock: bool) -> Self {
        Self { lock_border: lock, ..self }
    }

    /// Whether every level keeps the mesh's open border where it is.
    pub fn locks_border(&self) -> bool {
        self.lock_border
    }

    pub fn levels(&self) -> &[LodLevel] {
        &self.levels
    }

    /// Levels the mobile cook drops from the top of the chain.
    pub fn mobile_skip(&self) -> u32 {
        self.mobile_skip
    }
}

/// One submesh of a level being built.
#[derive(Clone, Debug)]
struct Part {
    material: u32,
    /// LOD0's index count, which triangle ratios are relative to.
    lod0_indices: usize,
    indices: Vec<u32>,
    /// Accumulated relative error.
    error: f32,
    /// The simplifier's attribute weights for this part ([`attribute_weights`]).
    weights: Vec<f32>,
}

/// One level of the chain: its parts, and the largest of their errors.
#[derive(Clone, Debug)]
struct Level {
    parts: Vec<Part>,
    error: f32,
}

impl Level {
    fn index_count(&self) -> usize {
        self.parts.iter().map(|part| part.indices.len()).sum()
    }
}

/// Optimizes `mesh` (LOD0 only, as imported and baked) for `cook`, as the
/// [module docs](self) describe. `seam_tolerance` is the distance within which vertices of two
/// submeshes count as one seam (the outline weld tolerance).
pub fn optimize(mesh: &mut ImportedMesh, settings: &LodSettings, seam_tolerance: f32, cook: Cook) {
    debug_assert_eq!(mesh.lods.len(), 1, "optimize builds the LOD chain from LOD0 alone");
    merge_by_material(mesh);
    let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.position).collect();
    let adapter = VertexDataAdapter::new(bytemuck::cast_slice(&positions), size_of::<[f32; 3]>(), 0)
        .expect("tightly packed positions");
    let mut chain = lod_chain(mesh, settings, seam_tolerance, &adapter);
    let skip = match cook {
        Cook::Desktop => 0,
        Cook::Mobile => settings.mobile_skip as usize,
    };
    chain.drain(..skip.min(chain.len() - 1));
    for part in chain.iter_mut().flat_map(|level| &mut level.parts) {
        meshopt::optimize_vertex_cache_in_place(&mut part.indices, positions.len());
        meshopt::optimize_overdraw_in_place(&mut part.indices, &adapter, OVERDRAW_THRESHOLD);
    }
    let scale = meshopt::simplify_scale(&adapter);
    reorder_vertices(mesh, &chain, scale);
}

/// Rewrites LOD0 with one submesh per material, in material order, each holding that
/// material's triangles in their original order.
fn merge_by_material(mesh: &mut ImportedMesh) {
    let lod0 = &mut mesh.lods[0];
    let mut indices = Vec::with_capacity(mesh.indices.len());
    let mut submeshes = Vec::new();
    for material in 0..mesh.materials.len() as u32 {
        let first_index = indices.len() as u32;
        for submesh in lod0.submeshes.iter().filter(|submesh| submesh.material == material) {
            let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
            indices.extend_from_slice(&mesh.indices[range]);
        }
        let index_count = indices.len() as u32 - first_index;
        if index_count > 0 {
            submeshes.push(Submesh { first_index, index_count, material });
        }
    }
    mesh.indices = indices;
    lod0.submeshes = submeshes;
}

/// LOD0 and every level `settings` keeps, LOD0 first, as index lists into `mesh.vertices`.
fn lod_chain(
    mesh: &ImportedMesh,
    settings: &LodSettings,
    seam_tolerance: f32,
    adapter: &VertexDataAdapter<'_>,
) -> Vec<Level> {
    let scale = meshopt::simplify_scale(adapter);
    let poses = if mesh.skin.is_some() { POSE_AXES.len() } else { 0 };
    let lod0 = Level {
        parts: mesh.lods[0]
            .submeshes
            .iter()
            .map(|submesh| {
                let range =
                    submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
                let indices = mesh.indices[range].to_vec();
                Part {
                    material: submesh.material,
                    lod0_indices: indices.len(),
                    weights: attribute_weights(poses, spacing(mesh, &indices, scale)),
                    indices,
                    error: 0.0,
                }
            })
            .collect(),
        error: 0.0,
    };
    let mut chain = vec![lod0];
    if settings.levels.is_empty() {
        return chain;
    }
    let attributes = simplifier_attributes(mesh, scale);
    let locks = locks(mesh, seam_tolerance);
    let options = if settings.lock_border { SimplifyOptions::LockBorder } else { SimplifyOptions::None };
    for level in &settings.levels {
        let last = chain.last().expect("LOD0 is always there");
        let parts: Vec<Part> = last
            .parts
            .iter()
            .map(|part| {
                let target = (part.lod0_indices as f32 * level.triangle_ratio) as usize / 3 * 3;
                let budget = level.max_error - part.error;
                if part.indices.len() <= target || budget <= 0.0 {
                    return part.clone();
                }
                let mut step_error = 0.0;
                let indices = meshopt::simplify_with_attributes_and_locks(
                    &part.indices,
                    adapter,
                    &attributes,
                    &part.weights,
                    size_of_val(part.weights.as_slice()),
                    &locks,
                    target,
                    budget,
                    options,
                    Some(&mut step_error),
                );
                Part { indices, error: part.error + step_error, ..part.clone() }
            })
            .collect();
        // Before parts that simplified away are dropped: their error still counts, and so does
        // every earlier level's, so the chain's errors never decrease.
        let error = parts.iter().map(|part| part.error).fold(last.error, f32::max);
        let candidate =
            Level { parts: parts.into_iter().filter(|part| !part.indices.is_empty()).collect(), error };
        let (count, last_count) = (candidate.index_count(), last.index_count());
        if count > 0 && count as f32 <= (1.0 - MIN_LOD_REDUCTION) * last_count as f32 {
            chain.push(candidate);
        }
    }
    chain
}

/// Per vertex, what the simplifier weighs beside its position: its normal and, for a skinned
/// mesh, its position in each test pose ([`POSE_AXES`]) over `scale` (the extent), as positions
/// are measured. [`attribute_weights`] gives their weights.
fn simplifier_attributes(mesh: &ImportedMesh, scale: f32) -> Vec<f32> {
    let poses: Vec<Vec<Vec3>> = match &mesh.skin {
        Some(skin) => POSE_AXES.iter().map(|&axis| posed_positions(mesh, skin, axis, POSE_ANGLE)).collect(),
        None => Vec::new(),
    };
    let mut attributes = Vec::with_capacity((3 + 3 * poses.len()) * mesh.vertices.len());
    for (index, vertex) in mesh.vertices.iter().enumerate() {
        attributes.extend(vertex.normal);
        attributes.extend(poses.iter().flat_map(|pose| (pose[index] / scale).to_array()));
    }
    attributes
}

/// The weights of [`simplifier_attributes`] with `poses` test poses, for a part whose vertices
/// lie `spacing` apart ([`spacing`]).
fn attribute_weights(poses: usize, spacing: f32) -> Vec<f32> {
    let mut weights = vec![NORMAL_WEIGHT; 3];
    weights.extend(std::iter::repeat_n(POSE_WEIGHT / spacing, 3 * poses));
    weights
}

/// How far apart the vertices of the triangles `indices` lie, on average, as a fraction of
/// `scale` (the extent): the square root of the triangles' area round each vertex they use.
/// meshoptimizer weighs an attribute's error by that area (a change of 1 across a region `d`
/// wide counts as a distance of `d`), which suits slopes such as normals. A position in a test
/// pose is a distance itself, so its weight is divided by this: it counts as the distance it
/// strays over a region of LOD0's size, and more over a coarser level's larger ones. 1 for
/// triangles without area.
fn spacing(mesh: &ImportedMesh, indices: &[u32], scale: f32) -> f32 {
    let triangles = indices.as_chunks::<3>().0;
    let area: f32 = triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.map(|index| Vec3::from(mesh.vertices[index as usize].position));
            (b - a).cross(c - a).length() / 2.0
        })
        .sum();
    let mut used = indices.to_vec();
    used.sort_unstable();
    used.dedup();
    // Every triangle lies round each of its three corners.
    let spacing = (3.0 * area / used.len() as f32).sqrt() / scale;
    if spacing.is_finite() && spacing > 0.0 { spacing } else { 1.0 }
}

/// Each vertex of `mesh`, skinned by `skin` in a test pose: every joint turned `angle` about
/// `axis` from its parent, round its own pivot. A joint's pivot is where its first palette
/// entry's inverse bind matrix puts it. Joints without an entry (an armature's root) don't
/// turn, so their children turn from the nearest ancestor that does.
fn posed_positions(mesh: &ImportedMesh, skin: &ImportedSkin, axis: Vec3, angle: f32) -> Vec<Vec3> {
    let mut pivots: Vec<Option<Vec3>> = vec![None; skin.joints.len()];
    for bind in &skin.binds {
        let pivot = Mat4::from_cols_array_2d(&bind.inverse_bind_matrix).inverse().w_axis.truncate();
        if let Some(first @ None) = pivots.get_mut(usize::from(bind.joint))
            && pivot.is_finite()
        {
            *first = Some(pivot);
        }
    }
    // Per joint, its posed transform in mesh space: `rotation · x + translation`. Parents come
    // first.
    let turn = Mat3::from_axis_angle(axis, angle);
    let mut posed: Vec<(Mat3, Vec3)> = Vec::with_capacity(skin.joints.len());
    for (joint, pivot) in skin.joints.iter().zip(&pivots) {
        let (rotation, translation) = joint
            .parent
            .and_then(|parent| posed.get(usize::from(parent)))
            .copied()
            .unwrap_or((Mat3::IDENTITY, Vec3::ZERO));
        posed.push(match *pivot {
            Some(pivot) => {
                let turned = rotation * turn;
                (turned, rotation * pivot + translation - turned * pivot)
            }
            None => (rotation, translation),
        });
    }
    mesh.vertices
        .iter()
        .enumerate()
        .map(|(index, vertex)| {
            let position = Vec3::from(vertex.position);
            // A vertex without weights, or bound to a palette entry or joint out of range (a mesh
            // the packer will refuse), stays put.
            let Some(influences) = skin.weights.get(index) else { return position };
            influences.binds.iter().zip(influences.weights).fold(Vec3::ZERO, |sum, (&bind, weight)| {
                let (rotation, translation) = skin
                    .binds
                    .get(usize::from(bind))
                    .and_then(|bind| posed.get(usize::from(bind.joint)))
                    .copied()
                    .unwrap_or((Mat3::IDENTITY, Vec3::ZERO));
                sum + weight * (rotation * position + translation)
            })
        })
        .collect()
}

/// Per vertex: whether it must not move. That is a vertex on a seam between two submeshes
/// (within `tolerance` of a vertex another submesh uses), and a vertex a morph target moves or
/// that shares a triangle with one (by position, so a split vertex's copies count as one).
fn locks(mesh: &ImportedMesh, tolerance: f32) -> Vec<bool> {
    let (group_of, group_count) = bake::weld(&mesh.vertices, tolerance);
    // Per weld group: the first submesh that uses it, and whether a second one does.
    let mut owner: Vec<Option<usize>> = vec![None; group_count];
    let mut locked = vec![false; group_count];
    for (submesh, range) in mesh.lods[0].submeshes.iter().enumerate() {
        let indices =
            &mesh.indices[range.first_index as usize..(range.first_index + range.index_count) as usize];
        for &index in indices {
            let group = group_of[index as usize];
            match owner[group] {
                None => owner[group] = Some(submesh),
                Some(first) if first != submesh => locked[group] = true,
                Some(_) => {}
            }
        }
    }
    let mut moved = vec![false; group_count];
    for delta in mesh.morph_targets.iter().flat_map(|target| &target.deltas) {
        moved[group_of[delta.vertex as usize]] = true;
    }
    if moved.contains(&true) {
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let groups = triangle.map(|index| group_of[index as usize]);
            if groups.iter().any(|&group| moved[group]) {
                groups.iter().for_each(|&group| locked[group] = true);
            }
        }
    }
    group_of.iter().map(|&group| locked[group]).collect()
}

/// Renumbers the vertices coarsest LOD first, drops the ones no level uses, and writes the
/// chain into `mesh`. `scale` turns relative errors into object-space meters.
fn reorder_vertices(mesh: &mut ImportedMesh, chain: &[Level], scale: f32) {
    // What `meshopt_optimizeVertexFetchRemap` computes, over the levels coarsest first. (The
    // `meshopt` crate's wrapper truncates the remap table when vertices go unused, as they do
    // when the mobile cook drops LOD0.)
    let mut remap = vec![u32::MAX; mesh.vertices.len()];
    let mut next = 0u32;
    let mut prefixes = vec![0u32; chain.len()];
    for (level, prefix) in chain.iter().zip(&mut prefixes).rev() {
        for &index in level.parts.iter().flat_map(|part| &part.indices) {
            let new = &mut remap[index as usize];
            if *new == u32::MAX {
                *new = next;
                next += 1;
            }
        }
        *prefix = next;
    }

    reorder_by(&mut mesh.vertices, &remap, next as usize);
    reorder_by(&mut mesh.authored_outline, &remap, next as usize);
    if !mesh.foliage.is_empty() {
        reorder_by(&mut mesh.foliage, &remap, next as usize);
    }
    if let Some(skin) = mesh.skin.as_mut() {
        reorder_by(&mut skin.weights, &remap, next as usize);
    }
    for target in &mut mesh.morph_targets {
        target.deltas.retain_mut(|delta| {
            delta.vertex = remap[delta.vertex as usize];
            delta.vertex != u32::MAX
        });
        target.deltas.sort_by_key(|delta| delta.vertex);
    }

    mesh.indices.clear();
    mesh.lods = chain
        .iter()
        .zip(prefixes)
        .map(|(level, vertex_count)| {
            let submeshes = level
                .parts
                .iter()
                .map(|part| {
                    let first_index = mesh.indices.len() as u32;
                    mesh.indices.extend(part.indices.iter().map(|&index| remap[index as usize]));
                    Submesh { first_index, index_count: part.indices.len() as u32, material: part.material }
                })
                .collect();
            Lod { vertex_count, error: level.error * scale, submeshes }
        })
        .collect();
}

/// Moves `values[old]` to `remap[old]` for every vertex kept, into a vector of `len`.
fn reorder_by<T: Clone + Default>(values: &mut Vec<T>, remap: &[u32], len: usize) {
    let mut reordered = vec![T::default(); len];
    for (value, &new) in values.iter().zip(remap) {
        if new != u32::MAX {
            reordered[new as usize] = value.clone();
        }
    }
    *values = reordered;
}

#[cfg(test)]
mod tests;
