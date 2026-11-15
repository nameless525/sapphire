//! How finely the frame's views read each streamed texture (M5 M5, m5-plan §3 "Mip streaming"): what the
//! stage's streamer wants of a texture's levels, from the draw lists.
//!
//! - **Which textures stream.** A slot streams when its shaders sample it through the mesh's UVs with the
//!   hardware's own choice of level ([`texture_streams`]): base color, normal and expression maps, and an
//!   environment's mask and emission. A slot a shader reads an edge, a threshold or an ID from (the control
//!   map, the face's shadow map, the surface masks, ramps, the terminator band), or samples at a level of
//!   its own (matcaps, fog cards, the sky) never streams: the stage keeps it whole while a material binds
//!   it (m5-plan §2's pillar 1).
//! - **The estimate.** For each draw of the frame's main view, a lower bound on the UV distance a pixel
//!   spans along the axis the GPU's level comes from ([`TextureCoverage::uv_per_pixel`]): the mesh's
//!   least UV stretch, per object-space meter, over the triangles of its material slot at every level of
//!   detail ([`TexelDensity`], once at upload), shrunk by the instance's largest stretch, times the least
//!   distance in meters one pixel spans anywhere on the instance's bounds (their nearest point, the world
//!   box's for a static mesh, at the widest angle off the view's axis the frustum allows), over √2. The GPU's level is the log of the longer of a pixel's two UV steps in
//!   texels, at least the larger singular value of the screen-to-UV map over √2, and that is at least the
//!   UV map's larger singular value times the screen-to-surface map's smaller one, which is never under
//!   that distance: so the level a texture is read at is never finer than the bound says. With every
//!   wanted level resident, a frame is the frame with whole textures, as the readback tests check; the
//!   stage takes a margin for the GPU's rounding. The material sampler has no anisotropy, so a grazing
//!   surface only reads coarser.
//! - **The other views.** An inset reads whatever its draws bind whole, as a close-up must open sharp. A
//!   shadow view reads the base color of the alpha-tested anime character variants (the masked casters)
//!   and of foliage's cards: by the cascade's or character map's texel, which is orthographic. A cascade
//!   a frame draws reads its list's casters; one it holds (M5 D8, a cache that keeps: a still view holds
//!   every cascade for as long as nothing in it changes) reads what its map holds, as the shadow plan
//!   kept it ([`CoverageInputs::kept`]): its list is of the frame that drew it, which a rebuilt draw list
//!   no longer matches. So a held map's cut-outs stay wanted for as long as it's held, and the stage
//!   never lowers one below the level the held map reads it at.
//!   The lines read an outlined character's base color through hulls whose footprint isn't the surface's,
//!   so an outlined draw wants its base color whole.
//! - **Deformed meshes** stretch their surfaces as they pose: their bound is halved, one level finer.
//! - **Cost.** Once a recorded frame, when the stage asks ([`crate::Renderer::estimate_texture_coverage`],
//!   at the top of its loading, only while it streams), on the frame's thread, over the main view's list,
//!   the insets' and the shadow views' masked and foliage groups, and where a cascade is held, once over
//!   the draw list's masked and foliage draws, whatever the number of cascades held; no allocation once
//!   its vectors have grown.
//!
//! Levels of detail (m5-plan D4, D5): a draw of any level finds its submesh's density, the least over the
//! levels, so the need a level change leaves is the one the estimate gave before it.

use glam::{Mat4, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    AnimeCharacterMaterial, EnvironmentMaterial, FoliageMaterial, MaterialFamily,
};
use sapphire_assets::mesh::{ArchivedMesh, PositionFormat};

use crate::bounds::{Sphere, max_stretch};
use crate::draw::{DrawSet, Source, ViewList, ViewLists, Walk, has_level};
use crate::inset::MAX_INSETS;
use crate::materials::MaterialRecord;
use crate::mesh_pool::MeshRecord;
use crate::pipelines::{Shading, SurfaceVariant, format_index};
use crate::scene::{DeformedDraw, DrawCommand, DrawList, InstanceSlots};
use crate::shadows::plan::KeptCaster;
use crate::shadows::{MAX_CASCADES, MAX_CHARACTER_MAPS};

/// Where a list's draws come from, every source.
const SOURCES: [Source; 3] =
    [Source::Static(PositionFormat::ALL[0]), Source::Static(PositionFormat::ALL[1]), Source::Deformed];

/// How much a skinned or morphed surface may stretch from its bind pose: a deformed draw's bound is
/// divided by it.
const DEFORMED_STRETCH: f32 = 2.0;

/// Whether slot `slot` of a `family` material streams: its shaders sample it through the mesh's UVs with
/// the hardware's level, so a level coarser than the one the screen reads changes nothing. Every other
/// slot is wanted whole while a material binds it (see the module docs).
pub fn texture_streams(family: MaterialFamily, slot: usize) -> bool {
    match family {
        MaterialFamily::AnimeCharacter => matches!(
            slot,
            AnimeCharacterMaterial::BASE_COLOR
                | AnimeCharacterMaterial::NORMAL
                | AnimeCharacterMaterial::EXPRESSION
        ),
        MaterialFamily::Environment => matches!(
            slot,
            EnvironmentMaterial::BASE_COLOR
                | EnvironmentMaterial::NORMAL
                | EnvironmentMaterial::MASK
                | EnvironmentMaterial::EMISSION
        ),
        MaterialFamily::Foliage => matches!(slot, FoliageMaterial::BASE_COLOR | FoliageMaterial::MASK),
        MaterialFamily::Sky | MaterialFamily::FogCard => false,
    }
}

/// A texture the frame's views read through a mesh's UVs, and how finely.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureCoverage {
    pub texture: AssetId,
    /// A lower bound on the UV distance one pixel spans, along the axis the GPU's level comes from, in the
    /// view and draw that read the texture most finely: 0 for one an inset, an outline or a surface with
    /// no UV stretch reads, which wants it whole. A texture `size` texels on its shorter side is read at
    /// level `log2(uv_per_pixel × size)` or coarser.
    pub uv_per_pixel: f32,
}

/// The textures the last recorded frame read and how finely ([`crate::Renderer::texture_coverage`]).
#[derive(Clone, Copy, Debug)]
pub struct TextureCoverageFrame<'a> {
    /// Frames recorded with an estimate since the renderer was created: one the stage has seen already
    /// tells it nothing new.
    pub frame: u64,
    /// By texture, each once, in [`AssetId`] order.
    pub textures: &'a [TextureCoverage],
}

/// The least UV stretch per object-space meter over the triangles of indices `first..first + count` of
/// `mesh`: the larger singular value of the map from a triangle's plane to its UVs, the least over the
/// triangles, decoded as the GPU decodes them. 0 when a triangle's UVs meet at a point (its texels are
/// read at level 0 however far away it is) or when it has no triangle with area.
pub(crate) fn submesh_density(mesh: &ArchivedMesh, first: u32, count: u32) -> f32 {
    let streams = &mesh.streams;
    let (uv_offset, uv_scale) = streams.uv_transform();
    let surface = streams.surface_bytes();
    let uv = |vertex: usize| -> Option<[f32; 2]> {
        let bytes = surface.get(vertex * 8..vertex * 8 + 4)?;
        let q = [u16::from_le_bytes([bytes[0], bytes[1]]), u16::from_le_bytes([bytes[2], bytes[3]])];
        Some(std::array::from_fn(|axis| uv_offset[axis] + uv_scale[axis] * (f32::from(q[axis]) / 65535.0)))
    };
    let corner = |index: u32| -> Option<(Vec3, [f32; 2])> {
        let vertex = mesh.indices.get(index as usize)?.to_native() as usize;
        Some((Vec3::from(streams.position(vertex)?), uv(vertex)?))
    };
    let mut least = f32::INFINITY;
    let end = first.saturating_add(count);
    for triangle in (first..end).step_by(3) {
        let (Some(a), Some(b), Some(c)) = (corner(triangle), corner(triangle + 1), corner(triangle + 2))
        else {
            continue;
        };
        if let Some(stretch) = triangle_stretch([a.0, b.0, c.0], [a.1, b.1, c.1]) {
            least = least.min(stretch);
        }
    }
    if least.is_finite() { least } else { 0.0 }
}

/// The larger singular value of the map from the plane of the triangle `positions` to its `uvs`, or `None`
/// for a triangle with no area, which draws nothing.
fn triangle_stretch(positions: [Vec3; 3], uvs: [[f32; 2]; 3]) -> Option<f32> {
    let (e1, e2) = (positions[1] - positions[0], positions[2] - positions[0]);
    let d1 = [uvs[1][0] - uvs[0][0], uvs[1][1] - uvs[0][1]];
    let d2 = [uvs[2][0] - uvs[0][0], uvs[2][1] - uvs[0][1]];
    let l1 = e1.length();
    if l1.is_nan() || l1 <= 0.0 {
        return None;
    }
    // The triangle in a frame of its plane: e1 along x, e2 at (a, b).
    let x = e1 / l1;
    let a = e2.dot(x);
    let b = (e2 - x * a).length();
    if b.is_nan() || b <= l1 * 1e-6 {
        return None;
    }
    // The UV map's columns: d1 = M (l1, 0), d2 = M (a, b).
    let c0 = [d1[0] / l1, d1[1] / l1];
    let c1 = [(d2[0] - c0[0] * a) / b, (d2[1] - c0[1] * a) / b];
    let frobenius = c0[0] * c0[0] + c0[1] * c0[1] + c1[0] * c1[0] + c1[1] * c1[1];
    let determinant = c0[0] * c1[1] - c1[0] * c0[1];
    let root = (frobenius * frobenius - 4.0 * determinant * determinant).max(0.0).sqrt();
    let largest = ((frobenius + root) * 0.5).sqrt();
    largest.is_finite().then_some(largest)
}

/// The meters one pixel spans, at least, anywhere on a sphere: from a camera's projection and the render
/// target's size.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Footprint {
    view: Mat4,
    eye: Vec3,
    /// The projection's focal length in pixels, the larger of its two axes'.
    focal: f32,
    /// `None` for an orthographic projection; for a perspective one, the near plane's distance and the
    /// tangent of the widest angle off the view's axis the frustum holds.
    perspective: Option<(f32, f32)>,
}

impl Footprint {
    /// For a camera with `view` and `proj` at `eye`, rendering `size` pixels.
    pub(crate) fn new(view: Mat4, proj: Mat4, eye: Vec3, size: (u32, u32)) -> Self {
        let (sx, sy) = (proj.x_axis.x.abs(), proj.y_axis.y.abs());
        let focal = (sx * size.0 as f32 * 0.5).max(sy * size.1 as f32 * 0.5);
        // An orthographic projection's w is 1; a perspective one's is the view depth.
        let perspective = (proj.w_axis.w == 0.0).then(|| {
            let near = proj.w_axis.z.abs();
            let corner = ((1.0 / sx).powi(2) + (1.0 / sy).powi(2)).sqrt();
            (near, corner)
        });
        Self { view, eye, focal, perspective }
    }

    /// The least meters one pixel spans on `sphere` (world space), in any direction, on a surface no nearer
    /// the eye than `nearest` too.
    pub(crate) fn meters_per_pixel(&self, sphere: Sphere, nearest: f32) -> f32 {
        let Some((near, corner)) = self.perspective else {
            return 1.0 / self.focal;
        };
        let offset = sphere.center - self.eye;
        let distance = offset.length();
        let nearest = (distance - sphere.radius).max(near).max(nearest);
        // The widest angle off the axis, as a tangent: the center's plus the sphere's half angle, no wider
        // than the frustum's corner.
        let tangent = if distance > sphere.radius {
            let local = self.view.transform_vector3(offset);
            let depth = -local.z;
            let across = local.truncate().length();
            let center = (across.atan2(depth)).max(0.0);
            let half = (sphere.radius / distance).asin();
            let angle = center + half;
            if angle >= std::f32::consts::FRAC_PI_2 { corner } else { angle.tan().min(corner) }
        } else {
            corner
        };
        // A pixel at angle α off the axis spans cos²α / f radians across its ray, at the least.
        let cos_squared = 1.0 / (1.0 + tangent * tangent);
        nearest * cos_squared / self.focal
    }
}

/// What the estimate reads of a mesh, made at its upload ([`crate::mesh_pool::MeshRecord::texel_density`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TexelDensity {
    /// Per level, LOD0 first, per submesh in LOD0's order (as the record's `lods`): its material slot's
    /// least UV stretch per object-space meter over the triangles of every level ([`submesh_density`]),
    /// how finely its textures can be read; 0 wants them whole. One value for a slot at every level, so a
    /// draw of any level (m5-plan D4) bounds the read of the slot's textures from below, whichever level
    /// drew them, and a texture's need doesn't move as a level changes.
    pub levels: Vec<Vec<f32>>,
    /// Its positions' box, in object space.
    pub local_box: Option<[Vec3; 2]>,
}

impl TexelDensity {
    /// `mesh`'s.
    pub(crate) fn of_mesh(mesh: &ArchivedMesh) -> Self {
        let mut least = vec![f32::INFINITY; mesh.materials.len()];
        let mut first = vec![f32::INFINITY; mesh.materials.len()];
        let densities: Vec<Vec<(usize, f32)>> = mesh
            .lods
            .iter()
            .map(|lod| {
                let submeshes = lod.submeshes.iter();
                submeshes
                    .map(|s| {
                        let slot = s.material.to_native() as usize;
                        let density =
                            submesh_density(mesh, s.first_index.to_native(), s.index_count.to_native());
                        (slot, density)
                    })
                    .collect()
            })
            .collect();
        for (level, lod) in densities.iter().enumerate() {
            for &(slot, density) in lod {
                if let Some(least) = least.get_mut(slot) {
                    *least = least.min(density);
                }
                if let Some(first) = first.get_mut(slot).filter(|_| level == 0) {
                    *first = first.min(density);
                }
            }
        }
        let coarser = least.iter().zip(&first).filter(|(least, first)| least < first).count();
        if coarser > 0 {
            // A coarser level's triangles stretch their UVs less than level 0's: the slot's textures want a
            // finer level wherever any level draws (counted for M5 M5's report on the district).
            let worst = least.iter().zip(&first).map(|(least, first)| least / first).fold(1.0, f32::min);
            tracing::debug!(target: crate::LOG_TARGET, slots = least.len(), coarser, worst, "texel density lowered by a level");
        }
        let levels = aligned_levels(&densities, &least);
        let bounds = mesh.streams.bounds.to_native();
        Self { levels, local_box: Some([bounds.min, bounds.max].map(Vec3::from)) }
    }

    /// The density a draw of `lods` starting at `first_index` takes, whichever level it is: 0, whole, for a
    /// draw that isn't one of them.
    pub(crate) fn of_draw(&self, lods: &[Vec<crate::mesh_pool::SubmeshDraw>], first_index: u32) -> f32 {
        lods.iter()
            .zip(&self.levels)
            .find_map(|(lod, densities)| {
                let submesh = lod.iter().position(|submesh| submesh.first_index == first_index)?;
                densities.get(submesh).copied()
            })
            .unwrap_or(0.0)
    }
}

/// Per level, per submesh in LOD0's order (as [`crate::mesh_pool::MeshRecord::lods`] aligns them, m5-plan
/// D4), its slot's `least` density: `densities` holds each level's submeshes as cooked, by slot. A slot
/// with no triangle of area, or none at all, wants its textures whole (0).
fn aligned_levels(densities: &[Vec<(usize, f32)>], least: &[f32]) -> Vec<Vec<f32>> {
    let Some(lod0) = densities.first() else { return Vec::new() };
    let row: Vec<f32> = lod0
        .iter()
        .map(|&(slot, _)| least.get(slot).copied().filter(|d| d.is_finite()).unwrap_or(0.0))
        .collect();
    vec![row; densities.len()]
}

/// The least distance from `eye` to `local_box` placed by `transform`, at the least: the distance to the
/// world's axis-aligned box that holds the placed one, which needs no inverse.
fn box_distance(local_box: [Vec3; 2], transform: Mat4, eye: Vec3) -> f32 {
    let center = transform.transform_point3((local_box[0] + local_box[1]) * 0.5);
    let half = (local_box[1] - local_box[0]) * 0.5;
    let half = transform.x_axis.truncate().abs() * half.x
        + transform.y_axis.truncate().abs() * half.y
        + transform.z_axis.truncate().abs() * half.z;
    let distance = eye.distance(eye.clamp(center - half, center + half));
    if distance.is_finite() { distance } else { 0.0 }
}

/// The frame's estimate, and what it keeps across frames so a steady frame allocates nothing.
#[derive(Debug, Default)]
pub(crate) struct CoverageEstimate {
    /// Per material record: the least UV per pixel any draw with it reads its streamed slots at, and its
    /// base color's apart (an outlined draw reads it whole); infinite for one no draw read.
    materials: Vec<[f32; 2]>,
    /// The materials `materials` holds a value for this frame.
    touched: Vec<u32>,
    textures: Vec<TextureCoverage>,
    frame: u64,
}

/// The views a recorded frame drew, kept for the estimate the stage asks for after it
/// ([`crate::Renderer::estimate_texture_coverage`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CoverageViews {
    /// The main view's footprint.
    pub footprint: Footprint,
    /// The insets that drew, by index.
    pub insets: [bool; MAX_INSETS],
    /// One texel of each cascade drawn or held, in meters, and of each character map drawn (the first
    /// `maps`).
    pub cascades: [Option<f32>; MAX_CASCADES],
    /// Bit `i`: cascade `i` is held, not drawn (M5 D8): what it reads is in [`CoverageInputs::kept`].
    pub held: u8,
    pub map_texels: [f32; MAX_CHARACTER_MAPS],
    pub maps: usize,
}

/// What [`CoverageEstimate::estimate`] reads: the frame's views, and what they drew.
pub(crate) struct CoverageInputs<'a> {
    pub list: &'a DrawList,
    pub slots: &'a InstanceSlots,
    pub meshes: &'a [MeshRecord],
    pub materials: &'a [MaterialRecord],
    /// The views' lists.
    pub lists: &'a ViewLists,
    pub views: CoverageViews,
    /// Per instance slot: what the cascades' maps hold of it, as they were last drawn (M5 D8).
    pub kept: &'a [KeptCaster],
}

impl CoverageEstimate {
    /// The last estimate.
    pub(crate) fn frame(&self) -> TextureCoverageFrame<'_> {
        TextureCoverageFrame { frame: self.frame, textures: &self.textures }
    }

    /// Estimates the frame from `inputs` (see the module docs).
    pub(crate) fn estimate(&mut self, inputs: &CoverageInputs<'_>) {
        profiling::scope!("texture coverage");
        for &material in &self.touched {
            self.materials[material as usize] = [f32::INFINITY; 2];
        }
        self.touched.clear();
        if self.materials.len() < inputs.materials.len() {
            self.materials.resize(inputs.materials.len(), [f32::INFINITY; 2]);
        }
        let views = inputs.views;
        for (walk, view) in reads(&views) {
            if let Some(list) = inputs.lists.list(walk) {
                self.walk(inputs, list, view);
            }
        }
        if views.held != 0 {
            self.walk_held(inputs);
        }
        self.textures.clear();
        for &material in &self.touched {
            let Some(record) = inputs.materials.get(material as usize) else { continue };
            let [mapped, base] = self.materials[material as usize];
            for (slot, texture) in record.textures() {
                if !texture_streams(record.family, slot) {
                    continue;
                }
                let base_color = match record.family {
                    MaterialFamily::AnimeCharacter => slot == AnimeCharacterMaterial::BASE_COLOR,
                    MaterialFamily::Environment => slot == EnvironmentMaterial::BASE_COLOR,
                    MaterialFamily::Foliage => slot == FoliageMaterial::BASE_COLOR,
                    MaterialFamily::Sky | MaterialFamily::FogCard => false,
                };
                let uv_per_pixel = if base_color { mapped.min(base) } else { mapped };
                if uv_per_pixel.is_finite() {
                    self.textures.push(TextureCoverage { texture, uv_per_pixel });
                }
            }
        }
        self.textures.sort_unstable_by(|a, b| {
            a.texture.cmp(&b.texture).then(a.uv_per_pixel.total_cmp(&b.uv_per_pixel))
        });
        self.textures.dedup_by(|later, kept| later.texture == kept.texture);
        self.frame += 1;
    }

    /// Takes the surfaces `list` holds that `view` reads textures in.
    fn walk(&mut self, inputs: &CoverageInputs<'_>, list: &ViewList, view: View) {
        for variant in SurfaceVariant::ALL.into_iter().filter(|&variant| view.reads(variant)) {
            for source in SOURCES {
                // Whichever level of detail an entry draws, its slot's density is the least over the levels
                // (`TexelDensity::levels`): the entry's own level changes nothing.
                for entry in list.group(source, variant, DrawSet::Surfaces) {
                    let draw = match source {
                        Source::Static(format) => inputs.list.commands[format_index(format)][variant.index()]
                            .get(entry.at as usize)
                            .map(Drawn::of_static),
                        Source::Deformed => inputs.list.deformed[variant.index()]
                            .get(entry.at as usize)
                            .map(Drawn::of_deformed),
                    };
                    if let Some(draw) = draw {
                        self.take(inputs, draw, view);
                    }
                }
            }
        }
    }

    /// Takes the masked and foliage draws the held cascades' maps hold ([`CoverageViews::held`]), each
    /// by the finest texel of those holding it at the level it was drawn at: one walk of the draw list's
    /// surfaces a shadow view reads, whatever the number of cascades held.
    fn walk_held(&mut self, inputs: &CoverageInputs<'_>) {
        let views = inputs.views;
        let shadow = View::Shadow(0.0);
        for variant in SurfaceVariant::ALL.into_iter().filter(|&variant| shadow.reads(variant)) {
            let index = variant.index();
            let statics = inputs.list.commands.iter().flat_map(|commands| &commands[index]);
            let statics = statics.map(|command| (Drawn::of_static(command), command.levels));
            let deformed =
                inputs.list.deformed[index].iter().map(|draw| (Drawn::of_deformed(draw), draw.levels));
            for (draw, levels) in statics.chain(deformed) {
                let Some(kept) = inputs.kept.get(draw.slot as usize) else { continue };
                let held = kept.maps & views.held;
                if held == 0 {
                    continue;
                }
                // A submesh simplified away at a cascade's level wasn't drawn into it.
                let texel = (0..MAX_CASCADES)
                    .filter(|&cascade| held & (1 << cascade) != 0 && has_level(levels, kept.levels[cascade]))
                    .filter_map(|cascade| views.cascades[cascade])
                    .reduce(f32::min);
                if let Some(texel) = texel {
                    self.take(inputs, draw, View::Shadow(texel));
                }
            }
        }
    }

    /// Takes one draw's bound into its material's.
    fn take(&mut self, inputs: &CoverageInputs<'_>, draw: Drawn, view: View) {
        let Some(entry) = self.materials.get_mut(draw.material) else { return };
        let bound = view.bound_of(|| {
            let record = inputs.meshes.get(draw.mesh);
            let density =
                record.map_or(0.0, |record| record.texel_density.of_draw(&record.lods, draw.first_index));
            let transform = inputs.slots.transform(draw.slot).unwrap_or(Mat4::IDENTITY);
            let stretch = max_stretch(transform);
            let meters = match view {
                View::Shadow(texel) => texel,
                _ => match inputs.slots.world_bounds(draw.slot, inputs.meshes) {
                    Some(sphere) => {
                        // A static draw's box can be nearer than its sphere says it can be: a street
                        // whose sphere holds the camera, seen from above it.
                        let local_box = record.and_then(|record| record.texel_density.local_box);
                        let floor = match local_box.filter(|_| !draw.deformed) {
                            Some(local_box) => box_distance(local_box, transform, inputs.views.footprint.eye),
                            None => 0.0,
                        };
                        inputs.views.footprint.meters_per_pixel(sphere, floor)
                    }
                    None => 0.0,
                },
            };
            let deformed = if draw.deformed { DEFORMED_STRETCH } else { 1.0 };
            let bound = density / (stretch * deformed) * meters * std::f32::consts::FRAC_1_SQRT_2;
            if bound.is_finite() && bound > 0.0 { bound } else { 0.0 }
        });
        if entry[0].is_infinite() && entry[1].is_infinite() {
            self.touched.push(draw.material as u32);
        }
        entry[0] = entry[0].min(bound);
        entry[1] = entry[1].min(if draw.outline { 0.0 } else { bound });
    }
}

/// One draw, as the estimate reads it.
#[derive(Clone, Copy)]
struct Drawn {
    slot: u32,
    mesh: usize,
    first_index: u32,
    material: usize,
    outline: bool,
    deformed: bool,
}

impl Drawn {
    fn of_static(command: &DrawCommand) -> Self {
        Self {
            slot: command.slot,
            mesh: command.mesh.index(),
            first_index: command.indices.start,
            material: command.material.index(),
            outline: command.outline,
            deformed: false,
        }
    }

    fn of_deformed(draw: &DeformedDraw) -> Self {
        Self {
            slot: draw.slot,
            mesh: draw.mesh.index(),
            first_index: draw.indices.start,
            material: draw.material.index(),
            outline: draw.outline,
            deformed: true,
        }
    }
}

/// The lists a frame's estimate reads, each with how its view reads its draws' textures: the main view by
/// its footprint, each inset that drew whole, and each shadow view drawn by its texel (a held cascade's
/// casters are walked apart, [`CoverageEstimate::walk_held`]).
fn reads(views: &CoverageViews) -> impl Iterator<Item = (Walk, View)> + '_ {
    let main = std::iter::once((Walk::Main, View::Main));
    let insets = views.insets.iter().enumerate().filter(|(_, drew)| **drew);
    // A held cascade's list is of the frame that drew it: what it reads is walked apart.
    let drawn = views.cascades.iter().enumerate().filter(|(index, _)| views.held & (1 << index) == 0);
    let cascades = drawn.filter_map(|(index, texel)| Some((index, (*texel)?)));
    let maps = views.map_texels[..views.maps].iter().enumerate();
    main.chain(insets.map(|(index, _)| (Walk::InsetMain(index), View::Whole)))
        .chain(cascades.map(|(index, texel)| (Walk::Cascade(index), View::Shadow(texel))))
        .chain(maps.map(|(index, texel)| (Walk::CharacterMap(index), View::Shadow(*texel))))
}

/// How a view reads its draws' textures.
#[derive(Clone, Copy, Debug, PartialEq)]
enum View {
    /// The main view: by its footprint.
    Main,
    /// An inset: whole.
    Whole,
    /// A shadow view of this texel, in meters: only the masked casters and foliage read a texture.
    Shadow(f32),
}

impl View {
    /// The bound a draw in this view takes: 0, whole, in a whole view, and `surface`'s in the others.
    fn bound_of(self, surface: impl FnOnce() -> f32) -> f32 {
        match self {
            Self::Whole => 0.0,
            Self::Main | Self::Shadow(_) => surface(),
        }
    }

    fn reads(self, variant: SurfaceVariant) -> bool {
        match self {
            Self::Main | Self::Whole => true,
            Self::Shadow(_) => {
                SurfaceVariant::CHARACTER_ALPHA_TEST.contains(&variant) || variant.shading == Shading::Foliage
            }
        }
    }
}

#[cfg(test)]
mod tests;
