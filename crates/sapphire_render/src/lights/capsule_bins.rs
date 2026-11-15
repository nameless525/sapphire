//! Capsules by cluster (M5 L18): which of the characters' capsules can reach each cluster of the
//! main view, so that a pixel walks only those.
//!
//! Two lists of capsules reach the world's surfaces: the key light's soft shadows of the bodies
//! (`shadows::plan`, `capsule_visibility` in `frame.wgsl`) and capsule AO (`ambient`,
//! `capsule_occlusion` in `ambient.wgsl`). Each chooses and orders its own, by its own rules
//! (the shadows by group and the shadow distance, after the maps; AO every body, nearest first),
//! so they stay two lists over one grid: a texel per cluster holds a mask for each.
//!
//! # The grid and the masks
//!
//! - **The light clusters' grid** ([`super::clusters`]: 16 × 9 tiles, 24 slices in view depth),
//!   built here by the CPU every frame on every tier: phones have no light clusters, a frame has a
//!   few dozen capsules, and binning them takes microseconds with no compute pass.
//! - **A mask per list:** bit `i` of a cluster's mask is set when capsule `i` of the list can
//!   change any point inside the cluster, two 32-bit words a list for [`BINNED_CAPSULES`].
//! - **The texture:** one `Rgba32Uint` texel per cluster (shadows' words in r and g, AO's in b and
//!   a), [`TEXTURE_SIZE`] texels: tile x across, and tile y + 9 × slice down, so a row is 256 bytes
//!   and copies from a buffer without padding. Group 0 binds it at [`FRAME_BINDING`] for both frame
//!   groups (the anime character family's environment instances and outlines take the shadows),
//!   read with `textureLoad`, which every backend has without a feature or a raised limit. A uniform
//!   array was ruled out: 3,456 clusters × 16 bytes is 54 KiB, past the 16 KiB a downlevel device
//!   allows; so was a storage buffer, since the characters' fragment stage has none left (M5 L1).
//! - **Written** through the uploads pass, only on a frame whose main view has a capsule in either
//!   list; otherwise nothing is written and nothing read (`frame.capsule_bins` is 0). The CPU's
//!   masks are allocated once: a warm frame allocates nothing.
//!
//! # The bounds
//!
//! A capsule's factor is exactly 1.0 outside a convex volume, the hull of a few points grown by a
//! radius ([`Reach`]):
//!
//! - **The shadow** (`capsule_lookup`): the light's ray from a point must pass within `radius +
//!   soft` of the body's segment, `soft` growing by `CAPSULE_SPREAD` a metre from its contact share,
//!   at a distance along it below `CAPSULE_REACH`. So the volume is the segment swept away from the
//!   light by the reach (a parallelogram), grown by the widest the shadow gets: `radius × 1.35 +
//!   8 × 0.03`.
//! - **AO** (`capsule_occlusion`): `reach` is 0 from four radii past the body's surface, so the
//!   segment grown by five radii.
//!
//! Each grows by [`PAD_METRES`] and [`PAD_SHARE`] of its distance, which holds the shader's float
//! rounding, and a cluster's tiles by the frame's jitter and half a pixel, so the jittered main
//! view and the debug views' unjittered replay read the same masks. A cluster takes the bit unless
//! the volume lies wholly outside one of its six planes, tested against the hull clipped to the
//! cluster's slice (grown by the radius): too large costs a pixel a lookup that returns 1.0, too
//! small would be a missing shadow, so the tests hold every point a factor changes inside a marked
//! cluster.
//!
//! # The pixel
//!
//! A pixel finds its cluster from its own world position through its view (`capsule_bin` in
//! `frame.wgsl`), walks its masks' set bits in index order and multiplies the same factors in the
//! same order as the plain loop, leaving out only those that are exactly 1.0. So with the same
//! capsules a frame draws the same bytes binned or not ([`RenderSettings::capsule_bins`]); the
//! cap rises from [`UNBINNED_CAPSULES`] to [`BINNED_CAPSULES`] a list.
//!
//! Insets (their own frame constants) and any view but the main one read the plain loop.
//!
//! [`RenderSettings::capsule_bins`]: crate::RenderSettings::capsule_bins

use glam::{Vec3, Vec4, Vec4Swizzles};
use sapphire_rhi::wgpu;

use super::clusters::{CLUSTER_COUNT, SLICE_START, SLICES, TILES_X, TILES_Y, slices_per_octave};
use crate::CameraData;

/// The most capsules a list holds with capsule bins on (WGSL `MAX_SHADOW_CAPSULES` and
/// `MAX_AMBIENT_CAPSULES`): a crowd at its tier's cap (60 on the desktop tiers) and a few named
/// characters.
pub const BINNED_CAPSULES: usize = 64;
/// The most a list holds with them off, as before M5 L18.
pub const UNBINNED_CAPSULES: usize = 16;
/// 32-bit words of one list's mask (`CAPSULE_BIN_WORDS` in `frame.wgsl`).
pub(crate) const WORDS: usize = BINNED_CAPSULES / 32;

/// Group 0's binding of the masks' texture.
pub(crate) const FRAME_BINDING: u32 = 22;
/// The masks' texture: a texel per cluster.
pub(crate) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;
pub(crate) const TEXTURE_SIZE: (u32, u32) = (TILES_X, TILES_Y * SLICES);
const TEXEL_BYTES: u32 = 16;

/// How far a capsule's shadow reaches along the light, and how its edge softens (frame.wgsl's
/// `CAPSULE_REACH`, `CAPSULE_SOFTNESS` and `CAPSULE_SPREAD`).
const CAPSULE_REACH: f32 = 8.0;
const CAPSULE_SOFTNESS: f32 = 0.35;
const CAPSULE_SPREAD: f32 = 0.03;
/// AO's reach: `reach` in `capsule_occlusion` is 0 from four radii past the body's surface.
const OCCLUSION_RADII: f32 = 5.0;

/// What every volume grows by, for the shader's rounding: a centimetre, and this share of the
/// larger of its distance from the camera and its coordinates' size.
const PAD_METRES: f32 = 0.01;
const PAD_SHARE: f32 = 1e-4;
/// Pixels a tile grows by beyond the frame's jitter.
const PAD_PIXELS: f32 = 0.5;

/// Which list a mask is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum List {
    Shadows = 0,
    Occlusion = 1,
}

/// A capsule's volume of influence: the convex hull of `count` points of `points` grown by
/// `radius`. Outside it the capsule's factor is exactly 1.0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reach {
    points: [Vec3; 4],
    count: usize,
    radius: f32,
}

impl Reach {
    /// Where the shadow of the capsule from `base` to `top`, `radius` thick, can fall, the key light
    /// toward `toward_light` (`capsule_lookup`): the segment swept away from the light by the
    /// reach, a parallelogram in order around, grown by the widest its soft edge gets.
    pub(crate) fn shadow(base: Vec3, top: Vec3, radius: f32, toward_light: Vec3) -> Self {
        let away = -toward_light * CAPSULE_REACH;
        let radius = radius.max(0.0);
        Self {
            points: [base, top, top + away, base + away],
            count: 4,
            radius: radius * (1.0 + CAPSULE_SOFTNESS) + CAPSULE_REACH * CAPSULE_SPREAD,
        }
    }

    /// Where the capsule darkens the ambient light (`capsule_occlusion`): its segment grown by
    /// five radii.
    pub(crate) fn occlusion(base: Vec3, top: Vec3, radius: f32) -> Self {
        Self { points: [base, top, top, base], count: 2, radius: radius.max(0.0) * OCCLUSION_RADII }
    }
}

/// Counters of the last frame binned (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapsuleBinStats {
    /// Capsule bins are on (`RenderSettings::capsule_bins`).
    pub enabled: bool,
    /// Capsules binned in each list: the shadows' and AO's.
    pub shadow_capsules: u32,
    pub occlusion_capsules: u32,
    /// Clusters with any bit set.
    pub marked_clusters: u32,
    /// Bits set over every cluster and both lists: what the clusters' pixels walk, summed.
    pub bits: u32,
    /// The most bits one cluster holds, both lists together: the most lookups a pixel walks.
    pub fullest: u32,
}

/// The main view's cluster planes for binning: each tile boundary's plane in world space,
/// grown outward by the pad, and the depth plane.
struct Grid {
    /// Per column: its left plane's coefficients (`clip.x − X·clip.w`) and normal length; column
    /// 0 has none (it reaches past the frame's edge, as the shader clamps).
    left: [(Vec4, f32); TILES_X as usize],
    right: [(Vec4, f32); TILES_X as usize],
    /// Per row, from the top: its top plane (`clip.y − Y·clip.w`, which a point in the row has at
    /// most 0) and its bottom plane.
    top: [(Vec4, f32); TILES_Y as usize],
    bottom: [(Vec4, f32); TILES_Y as usize],
    /// View depth, `clip.w`, and its normal's length.
    depth: (Vec4, f32),
    /// Per slice: its depth range, the first from −∞ and the last to ∞.
    slices: [(f32, f32); SLICES as usize],
}

impl Grid {
    /// The grid of `camera` drawn at `size` pixels with temporal AA's `jitter` (pixels).
    fn new(camera: &CameraData, size: (u32, u32), jitter: [f32; 2]) -> Self {
        let m = camera.proj * camera.view;
        let row = |i: usize| Vec4::new(m.x_axis[i], m.y_axis[i], m.z_axis[i], m.w_axis[i]);
        let (rx, ry, rw) = (row(0), row(1), row(3));
        let plane = |r: Vec4, at: f32| {
            let plane = r - at * rw;
            (plane, plane.xyz().length())
        };
        let pad = |pixels: f32, side: u32| 2.0 * (pixels.abs() + PAD_PIXELS) / side.max(1) as f32;
        let (pad_x, pad_y) = (pad(jitter[0], size.0), pad(jitter[1], size.1));
        let x = |c: u32| c as f32 / TILES_X as f32 * 2.0 - 1.0;
        let y = |r: u32| 1.0 - r as f32 / TILES_Y as f32 * 2.0;
        let per_octave = slices_per_octave();
        let slice_depth = |k: u32| SLICE_START * (k as f32 / per_octave).exp2();
        Self {
            left: std::array::from_fn(|c| plane(rx, x(c as u32) - pad_x)),
            right: std::array::from_fn(|c| plane(rx, x(c as u32 + 1) + pad_x)),
            top: std::array::from_fn(|r| plane(ry, y(r as u32) + pad_y)),
            bottom: std::array::from_fn(|r| plane(ry, y(r as u32 + 1) - pad_y)),
            depth: (rw, rw.xyz().length()),
            slices: std::array::from_fn(|s| {
                let s = s as u32;
                let near = if s == 0 { f32::NEG_INFINITY } else { slice_depth(s - 1) };
                let far = if s == SLICES - 1 { f32::INFINITY } else { slice_depth(s) };
                (near, far)
            }),
        }
    }
}

/// Up to [`BINNED_CAPSULES`] of a list's capsules, `(base, top, radius)`, held on the stack for
/// [`CapsuleBins::bin`].
pub(crate) struct Gathered {
    capsules: [(Vec3, Vec3, f32); BINNED_CAPSULES],
    count: usize,
}

impl Gathered {
    pub(crate) fn as_slice(&self) -> &[(Vec3, Vec3, f32)] {
        &self.capsules[..self.count]
    }
}

/// The first [`BINNED_CAPSULES`] of `capsules`.
pub(crate) fn gather(capsules: impl Iterator<Item = (Vec3, Vec3, f32)>) -> Gathered {
    let mut gathered = Gathered { capsules: [(Vec3::ZERO, Vec3::ZERO, 0.0); BINNED_CAPSULES], count: 0 };
    for (slot, capsule) in gathered.capsules.iter_mut().zip(capsules) {
        *slot = capsule;
        gathered.count += 1;
    }
    gathered
}

/// A cluster's coordinates in the masks' texture.
pub(crate) fn texel(x: u32, y: u32, slice: u32) -> usize {
    ((slice * TILES_Y + y) * TILES_X + x) as usize
}

/// The capsules' masks per cluster on the CPU: what [`CapsuleBins`] uploads.
#[derive(Debug)]
pub(crate) struct ClusterMasks {
    /// Binning is on: the lists hold up to [`BINNED_CAPSULES`].
    enabled: bool,
    /// Per cluster, at [`texel`]: the shadows' words, then AO's.
    masks: Vec<[u32; 2 * WORDS]>,
    /// Some mask holds a bit: the next frame clears them first.
    marked: bool,
    /// The main view reads the masks this frame.
    read: bool,
    stats: CapsuleBinStats,
    /// The masks are left empty while the main view still reads them ([`Renderer::blank_capsule_masks`]):
    /// a test's knob, in debug builds only.
    ///
    /// [`Renderer::blank_capsule_masks`]: crate::Renderer::blank_capsule_masks
    #[cfg(debug_assertions)]
    blank: bool,
}

impl ClusterMasks {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            masks: vec![[0; 2 * WORDS]; CLUSTER_COUNT as usize],
            marked: false,
            read: false,
            stats: CapsuleBinStats { enabled, ..CapsuleBinStats::default() },
            #[cfg(debug_assertions)]
            blank: false,
        }
    }

    /// Bins the frame's capsules for the main view of `camera`, drawn at `size` with temporal AA's
    /// `jitter` (pixels): `shadows` the key light's capsules toward `toward_light`, `occlusion`
    /// AO's, each `(base, top, radius)` in its list's order. Returns whether the main view reads
    /// the masks (`frame.capsule_bins`): binning is on and some list has a capsule.
    pub(crate) fn bin(
        &mut self,
        camera: &CameraData,
        size: (u32, u32),
        jitter: [f32; 2],
        (shadows, toward_light): (&[(Vec3, Vec3, f32)], Vec3),
        occlusion: &[(Vec3, Vec3, f32)],
    ) -> bool {
        profiling::scope!("CapsuleBins::bin");
        if std::mem::take(&mut self.marked) {
            self.masks.fill([0; 2 * WORDS]);
        }
        let shadows = &shadows[..shadows.len().min(BINNED_CAPSULES)];
        let occlusion = &occlusion[..occlusion.len().min(BINNED_CAPSULES)];
        self.read = self.enabled && !(shadows.is_empty() && occlusion.is_empty());
        self.stats = CapsuleBinStats { enabled: self.enabled, ..CapsuleBinStats::default() };
        if !self.read {
            return false;
        }
        #[cfg(debug_assertions)]
        if self.blank {
            return true;
        }
        let grid = Grid::new(camera, size, jitter);
        for (index, &(base, top, radius)) in shadows.iter().enumerate() {
            self.mark(&grid, &Reach::shadow(base, top, radius, toward_light), List::Shadows, index);
        }
        for (index, &(base, top, radius)) in occlusion.iter().enumerate() {
            self.mark(&grid, &Reach::occlusion(base, top, radius), List::Occlusion, index);
        }
        self.marked = true;
        let (mut marked, mut bits, mut fullest) = (0, 0, 0);
        for mask in &self.masks {
            let count: u32 = mask.iter().map(|word| word.count_ones()).sum();
            marked += u32::from(count > 0);
            bits += count;
            fullest = fullest.max(count);
        }
        self.stats = CapsuleBinStats {
            enabled: true,
            shadow_capsules: shadows.len() as u32,
            occlusion_capsules: occlusion.len() as u32,
            marked_clusters: marked,
            bits,
            fullest,
        };
        true
    }

    /// Sets capsule `index` of `list` in every cluster `reach` may touch on `grid`.
    fn mark(&mut self, grid: &Grid, reach: &Reach, list: List, index: usize) {
        let points = &reach.points[..reach.count];
        let (depth_plane, depth_scale) = grid.depth;
        let depth = |p: Vec3| depth_plane.dot(p.extend(1.0));
        let far = points.iter().map(|p| depth(*p).abs()).fold(0.0, f32::max);
        let size = points.iter().map(|p| p.abs().max_element()).fold(0.0, f32::max);
        let radius = reach.radius + PAD_METRES + PAD_SHARE * far.max(size);
        if !radius.is_finite() || !points.iter().all(|p| p.is_finite()) {
            return;
        }
        let grow = radius * depth_scale;
        let (shallowest, deepest) =
            points.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), p| {
                (low.min(depth(*p)), high.max(depth(*p)))
            });
        let (word, bit) = (list as usize * WORDS + index / 32, 1u32 << (index % 32));
        for (slice, &(near, far)) in grid.slices.iter().enumerate() {
            if deepest + grow < near || shallowest - grow >= far {
                continue;
            }
            // The hull within the slice's depths grown by the radius: every point of the volume
            // in the slice lies within the radius of it.
            let mut clipped = [Vec3::ZERO; 8];
            let count = clip_to_slab(points, depth, near - grow, far + grow, &mut clipped);
            if count == 0 {
                continue;
            }
            let clipped = &clipped[..count];
            // Whether some point of the volume lies where `plane` is at least 0, or at most 0.
            let above = |(plane, scale): (Vec4, f32)| {
                let most = clipped.iter().map(|p| plane.dot(p.extend(1.0))).fold(f32::NEG_INFINITY, f32::max);
                most + radius * scale >= 0.0
            };
            let below = |(plane, scale): (Vec4, f32)| {
                let least = clipped.iter().map(|p| plane.dot(p.extend(1.0))).fold(f32::INFINITY, f32::min);
                least - radius * scale <= 0.0
            };
            let mut columns = 0u32;
            for c in 0..TILES_X as usize {
                let left = c == 0 || above(grid.left[c]);
                let right = c + 1 == TILES_X as usize || below(grid.right[c]);
                columns |= u32::from(left && right) << c;
            }
            if columns == 0 {
                continue;
            }
            for r in 0..TILES_Y as usize {
                let top = r == 0 || below(grid.top[r]);
                let bottom = r + 1 == TILES_Y as usize || above(grid.bottom[r]);
                if !(top && bottom) {
                    continue;
                }
                let mut rest = columns;
                while rest != 0 {
                    let c = rest.trailing_zeros();
                    rest &= rest - 1;
                    self.masks[texel(c, r as u32, slice as u32)][word] |= bit;
                }
            }
        }
    }

    /// The masks of the cluster at `texel`: the shadows' words, then AO's.
    #[cfg(test)]
    pub(crate) fn at(&self, texel: usize) -> [u32; 2 * WORDS] {
        self.masks[texel]
    }
}

/// The capsules' masks per cluster, on the CPU and in their texture.
#[derive(Debug)]
pub(crate) struct CapsuleBins {
    cpu: ClusterMasks,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Where the uploads pass stages the masks before copying them into the texture.
    staging: wgpu::Buffer,
}

impl CapsuleBins {
    pub(crate) fn new(device: &wgpu::Device, enabled: bool) -> Self {
        let (width, height) = TEXTURE_SIZE;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capsule bins"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capsule bins"),
            size: u64::from(width * height * TEXEL_BYTES),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self { cpu: ClusterMasks::new(enabled), texture, view, staging }
    }

    /// The most capsules each list holds.
    pub(crate) fn capacity(&self) -> usize {
        if self.cpu.enabled { BINNED_CAPSULES } else { UNBINNED_CAPSULES }
    }

    /// [`ClusterMasks::bin`].
    pub(crate) fn bin(
        &mut self,
        camera: &CameraData,
        size: (u32, u32),
        jitter: [f32; 2],
        shadows: (&[(Vec3, Vec3, f32)], Vec3),
        occlusion: &[(Vec3, Vec3, f32)],
    ) -> bool {
        self.cpu.bin(camera, size, jitter, shadows, occlusion)
    }

    pub(crate) fn stats(&self) -> CapsuleBinStats {
        self.cpu.stats
    }

    /// Leaves the masks empty while the main view still reads them, or bins again (debug builds only).
    #[cfg(debug_assertions)]
    pub(crate) fn blank(&mut self, blank: bool) {
        self.cpu.blank = blank;
    }

    /// Group 0's binding of the masks.
    pub(crate) fn frame_entry(&self) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding: FRAME_BINDING,
            resource: wgpu::BindingResource::TextureView(&self.view),
        }
    }

    /// The masks' share of a frame's uploads pass, on a frame that reads them: into the staging
    /// buffer through `belt`, then into the texture.
    pub(crate) fn upload(&self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        if !self.cpu.read {
            return;
        }
        crate::buffer::stage(belt, encoder, &self.staging, 0, bytemuck::cast_slice(&self.cpu.masks));
        let (width, height) = TEXTURE_SIZE;
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &self.staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * TEXEL_BYTES),
                    rows_per_image: Some(height),
                },
            },
            self.texture.as_image_copy(),
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }
}

/// The convex polygon (or segment) `points` clipped to `low ≤ depth ≤ high` into `out`
/// (Sutherland–Hodgman, both planes): the number of points written, 0 if nothing is left.
fn clip_to_slab(
    points: &[Vec3],
    depth: impl Fn(Vec3) -> f32,
    low: f32,
    high: f32,
    out: &mut [Vec3; 8],
) -> usize {
    let mut first = [Vec3::ZERO; 8];
    let kept = clip_to_plane(points, |p| depth(p) - low, &mut first);
    clip_to_plane(&first[..kept], |p| high - depth(p), out)
}

/// `points` clipped to where `side` is at least 0, into `out`.
fn clip_to_plane(points: &[Vec3], side: impl Fn(Vec3) -> f32, out: &mut [Vec3; 8]) -> usize {
    let mut count = 0;
    let mut push = |p: Vec3| {
        if count < out.len() {
            out[count] = p;
            count += 1;
        }
    };
    for (i, &a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        let (sa, sb) = (side(a), side(b));
        if sa >= 0.0 {
            push(a);
        }
        if (sa >= 0.0) != (sb >= 0.0) {
            // Infinite planes (the first and last slices) keep or drop whole edges above.
            let t = sa / (sa - sb);
            if t.is_finite() {
                push(a + (b - a) * t);
            }
        }
    }
    count
}

/// The cluster texel the shader reads for world `position` through `clip_from_world`
/// (`capsule_bin` in `frame.wgsl`), as `(x, y, slice)`.
#[cfg(test)]
pub(crate) fn cluster_of(clip_from_world: glam::Mat4, position: Vec3) -> (u32, u32, u32) {
    let clip = clip_from_world * position.extend(1.0);
    let ndc = clip.xy() / clip.w;
    let tiles = glam::Vec2::new(TILES_X as f32, TILES_Y as f32);
    let at = glam::Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * tiles;
    let tile = at.clamp(glam::Vec2::ZERO, tiles - 1.0);
    (tile.x as u32, tile.y as u32, super::clusters::reference::slice_of_depth(clip.w))
}

#[cfg(test)]
mod tests {
    use glam::Mat4;

    use super::*;
    use crate::lights::Rng;

    /// frame.wgsl's `capsule_lookup`, step for step.
    fn shadow_factor(base: Vec3, top: Vec3, radius: f32, l: Vec3, position: Vec3) -> f32 {
        let a = base;
        let axis = top - a;
        let w = position - a;
        let e = axis.dot(axis);
        let along = l.dot(axis);
        let c = l.dot(w);
        let f = axis.dot(w);
        let denominator = e - along * along;
        let mut u = if denominator > 1e-8 { (f - c * along) / denominator } else { 0.0 };
        u = u.clamp(0.0, 1.0);
        let t = (u * along - c).max(0.0);
        u = if e > 1e-8 { ((f + t * along) / e).clamp(0.0, 1.0) } else { 0.0 };
        let d = (w + t * l - u * axis).length();
        let soft = radius * CAPSULE_SOFTNESS + t * CAPSULE_SPREAD;
        let occlusion = 1.0 - smoothstep(radius - soft, radius + soft, d);
        let fade = 1.0 - smoothstep(0.75 * CAPSULE_REACH, CAPSULE_REACH, t);
        if t <= 0.0 { 1.0 } else { 1.0 - occlusion * fade }
    }

    /// ambient.wgsl's `capsule_occlusion` for one capsule, step for step.
    fn occlusion_factor(base: Vec3, top: Vec3, radius: f32, position: Vec3, n: Vec3) -> f32 {
        let axis = top - base;
        let along = if axis.dot(axis) > 1e-8 {
            ((position - base).dot(axis) / axis.dot(axis)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let to_center = base + along * axis - position;
        let distance = to_center.length().max(1e-4);
        let facing = n.dot(to_center / distance).clamp(0.0, 1.0);
        let ratio = radius / distance.max(radius);
        let reach = 1.0 - smoothstep(2.0 * radius, 4.0 * radius, distance - radius);
        1.0 - facing * ratio * ratio * reach
    }

    fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
        let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    fn unit(rng: &mut Rng) -> Vec3 {
        loop {
            let v = Vec3::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
            if (0.01..1.0).contains(&v.length_squared()) {
                return v.normalize();
            }
        }
    }

    fn camera(rng: &mut Rng) -> CameraData {
        let eye = Vec3::new(rng.range(-20.0, 20.0), rng.range(0.3, 12.0), rng.range(-20.0, 20.0));
        let target = eye + unit(rng) * 5.0;
        let aspect = rng.range(0.5, 2.4);
        CameraData::look_at(eye, target, Vec3::Y, rng.range(0.5, 1.6), aspect, rng.range(0.03, 0.3))
    }

    /// The camera's projection moved by `jitter` pixels at `size`, as temporal AA moves it.
    fn jittered(camera: &CameraData, size: (u32, u32), jitter: [f32; 2]) -> Mat4 {
        let offset = Mat4::from_translation(Vec3::new(
            2.0 * jitter[0] / size.0 as f32,
            -2.0 * jitter[1] / size.1 as f32,
            0.0,
        ));
        offset * camera.proj * camera.view
    }

    #[test]
    fn every_point_a_capsule_changes_lies_in_a_cluster_with_its_bit() {
        let mut rng = Rng(0x0005_eed1);
        let mut bins = ClusterMasks::new(true);
        let (mut changed, mut checked, mut walked) = (0u64, 0u64, 0u64);
        for round in 0..120 {
            let camera = camera(&mut rng);
            let size = [(1920, 1080), (640, 360), (1366, 768), (480, 270), (1170, 2532)][round % 5];
            let jitter = if round % 2 == 0 { [rng.range(-0.5, 0.5), rng.range(-0.5, 0.5)] } else { [0.0; 2] };
            let toward = {
                // Grazing lights now and then: long shadows across many clusters.
                let mut l = unit(&mut rng);
                l.y = if round % 4 == 0 { rng.range(0.02, 0.1) } else { l.y.abs().max(0.05) };
                l.normalize()
            };
            let forward = (camera.view.inverse().transform_vector3(Vec3::NEG_Z)).normalize();
            let capsules: Vec<(Vec3, Vec3, f32)> = (0..BINNED_CAPSULES)
                .map(|index| {
                    // In front, beside, behind and across the near plane.
                    let along = match index % 4 {
                        0 => rng.range(-2.0, 0.5),
                        1 => rng.range(0.0, 3.0),
                        _ => rng.range(1.0, 40.0),
                    };
                    let base = camera.position + forward * along + unit(&mut rng) * rng.range(0.0, 8.0);
                    let lying = index % 5 == 0;
                    let up = if lying {
                        unit(&mut rng) * rng.range(0.0, 2.0)
                    } else {
                        Vec3::Y * rng.range(0.0, 1.8)
                    };
                    (base, base + up, rng.range(0.0, 0.6))
                })
                .collect();
            let occluders = &capsules[..capsules.len() - round % 7];
            assert!(bins.bin(&camera, size, jitter, (&capsules, toward), occluders));
            let clip_from_world = jittered(&camera, size, jitter);
            for _ in 0..1000 {
                let (index, &(base, top, radius)) = {
                    let index = (rng.next() * capsules.len() as f32) as usize % capsules.len();
                    (index, &capsules[index])
                };
                // Near the capsule or in its shadow's reach.
                let point = base + (top - base) * rng.next() - toward * rng.range(0.0, 9.0)
                    + unit(&mut rng) * rng.range(0.0, 3.0);
                let clip = clip_from_world * point.extend(1.0);
                if clip.w < camera.proj.w_axis.z {
                    continue; // Nearer than the near plane: no pixel is.
                }
                let ndc = clip.xy() / clip.w;
                if ndc.abs().max_element() > 1.2 {
                    continue;
                }
                checked += 1;
                let (x, y, slice) = cluster_of(clip_from_world, point);
                let masks = bins.at(texel(x, y, slice));
                walked += masks.iter().map(|word| word.count_ones()).sum::<u32>() as u64;
                let (word, bit) = (index / 32, 1u32 << (index % 32));
                if shadow_factor(base, top, radius, toward, point) != 1.0 {
                    changed += 1;
                    assert!(
                        masks[word] & bit != 0,
                        "round {round}: shadow {index} at {point} in {x},{y},{slice}"
                    );
                }
                if index < occluders.len() {
                    let n = (base + (top - base) * 0.5 - point).normalize_or(Vec3::Y);
                    if occlusion_factor(base, top, radius, point, n) != 1.0 {
                        changed += 1;
                        assert!(
                            masks[WORDS + word] & bit != 0,
                            "round {round}: occluder {index} at {point} in {x},{y},{slice}"
                        );
                    }
                }
            }
        }
        assert!(changed > 10_000, "the points sample the volumes: {changed} of {checked}");
        // Binning bins: a pixel walks far fewer than every capsule of both lists.
        let mean = walked as f64 / checked as f64;
        assert!(mean < (2 * BINNED_CAPSULES) as f64 * 0.5, "mean bits walked {mean}");
    }

    #[test]
    fn the_same_frame_bins_the_same_masks_and_no_capsules_bin_nothing() {
        let mut rng = Rng(77);
        let camera = camera(&mut rng);
        let capsules: Vec<(Vec3, Vec3, f32)> = (0..40)
            .map(|_| {
                let base = camera.position + unit(&mut rng) * rng.range(1.0, 20.0);
                (base, base + Vec3::Y * 1.6, 0.3)
            })
            .collect();
        let toward = Vec3::new(0.3, 0.8, 0.2).normalize();
        let mut first = ClusterMasks::new(true);
        assert!(first.bin(&camera, (1920, 1080), [0.2, -0.1], (&capsules, toward), &capsules));
        let mut second = ClusterMasks::new(true);
        // A different frame in between leaves nothing behind.
        second.bin(&camera, (1920, 1080), [0.0; 2], (&capsules[..3], -toward), &[]);
        assert!(second.bin(&camera, (1920, 1080), [0.2, -0.1], (&capsules, toward), &capsules));
        assert!(first.masks == second.masks, "the same masks");
        assert_eq!(first.stats, second.stats);
        assert!(first.stats.bits > 0 && first.stats.fullest > 0);
        // No capsules: nothing marked, nothing read.
        assert!(!second.bin(&camera, (1920, 1080), [0.0; 2], (&[], toward), &[]));
        assert!(second.masks.iter().all(|mask| *mask == [0; 2 * WORDS]));
        assert!(!second.marked && !second.read);
        // Off: never read.
        let mut off = ClusterMasks::new(false);
        assert!(!off.bin(&camera, (1920, 1080), [0.0; 2], (&capsules, toward), &capsules));
        assert_eq!(off.stats, CapsuleBinStats::default());
    }

    /// What the frame constants say (M5 L18): the main view reads the masks on a frame with a capsule and
    /// bins on, walks the plain loops otherwise, at the lists' capacity; an inset never reads them and
    /// walks 16.
    #[test]
    fn the_main_view_reads_the_masks_with_a_capsule_and_bins_on_and_an_inset_never() {
        use crate::gpu_types::FrameUniforms;
        let camera = CameraData::look_at(Vec3::new(0.0, 1.6, 4.0), Vec3::ZERO, Vec3::Y, 0.9, 16.0 / 9.0, 0.1);
        let capsule = [(Vec3::new(0.0, 0.2, -2.0), Vec3::new(0.0, 1.6, -2.0), 0.3)];
        let world: FrameUniforms = bytemuck::Zeroable::zeroed();
        let none: &[(Vec3, Vec3, f32)] = &[];
        for (enabled, capsules, reads) in [(true, &capsule[..], 1), (true, none, 0), (false, &capsule[..], 0)]
        {
            let mut masks = ClusterMasks::new(enabled);
            let read = masks.bin(&camera, (640, 360), [0.0; 2], (capsules, Vec3::Y), capsules);
            let capacity = if enabled { BINNED_CAPSULES } else { UNBINNED_CAPSULES };
            let frame = world.with_capsules(read, capacity);
            assert_eq!((frame.capsule_bins, frame.capsules[0]), (reads, capacity as u32), "on {enabled}");
            let inset = frame.inset_capsules();
            assert_eq!((inset.capsule_bins, inset.capsules[0]), (0, UNBINNED_CAPSULES as u32));
        }
    }

    #[test]
    fn a_warm_frame_allocates_nothing() {
        let mut rng = Rng(3);
        let camera = camera(&mut rng);
        let capsules: Vec<(Vec3, Vec3, f32)> = (0..BINNED_CAPSULES)
            .map(|_| {
                let base = camera.position + unit(&mut rng) * rng.range(1.0, 30.0);
                (base, base + Vec3::Y * 1.7, 0.25)
            })
            .collect();
        let toward = Vec3::new(-0.4, 0.5, 0.3).normalize();
        let mut bins = ClusterMasks::new(true);
        bins.bin(&camera, (1920, 1080), [0.0; 2], (&capsules, toward), &capsules);
        let (read, allocated) = sapphire_core::alloc::count_in_scope(|| {
            bins.bin(&camera, (1920, 1080), [0.3, 0.3], (&capsules, toward), &capsules)
        });
        assert!(read);
        assert_eq!(allocated.allocations, 0, "a warm frame's binning allocated: {allocated:?}");
    }

    #[test]
    fn a_capsule_far_off_marks_only_clusters_about_it() {
        // One standing capsule 10 m ahead, centred: AO marks a few tiles around the screen's
        // centre in the slices about 10 m deep, and its shadow, cast away from a light behind the
        // camera, reaches deeper but not nearer.
        let camera = CameraData::look_at(
            Vec3::new(0.0, 1.0, 10.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::Y,
            0.9,
            16.0 / 9.0,
            0.1,
        );
        let capsule = [(Vec3::ZERO, Vec3::new(0.0, 1.8, 0.0), 0.3)];
        let toward = Vec3::new(0.0, 0.5, 1.0).normalize();
        let mut bins = ClusterMasks::new(true);
        assert!(bins.bin(&camera, (1920, 1080), [0.0; 2], (&capsule, toward), &capsule));
        let slice = super::super::clusters::reference::slice_of_depth(10.0);
        let mut occlusion_slices = Vec::new();
        let mut shadow_slices = Vec::new();
        for s in 0..SLICES {
            for y in 0..TILES_Y {
                for x in 0..TILES_X {
                    let masks = bins.at(texel(x, y, s));
                    if masks[WORDS] != 0 {
                        occlusion_slices.push(s);
                        assert!((5..=10).contains(&x), "AO at column {x}");
                    }
                    if masks[0] != 0 {
                        shadow_slices.push(s);
                    }
                }
            }
        }
        assert!(occlusion_slices.contains(&slice));
        assert!(occlusion_slices.iter().all(|s| s.abs_diff(slice) <= 2), "{occlusion_slices:?}");
        assert!(shadow_slices.iter().all(|&s| s + 2 >= slice), "nothing nearer: {shadow_slices:?}");
        assert!(shadow_slices.iter().any(|&s| s > slice + 1), "deeper: {shadow_slices:?}");
        let marked = bins.stats.marked_clusters;
        assert!(marked > 0 && marked < CLUSTER_COUNT / 40, "{marked} clusters");
    }

    #[test]
    fn slabs_clip_polygons_and_keep_infinite_slices_whole() {
        let square = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 4.0),
            Vec3::new(1.0, 0.0, 4.0),
            Vec3::new(1.0, 0.0, 0.0),
        ];
        let mut out = [Vec3::ZERO; 8];
        let count = clip_to_slab(&square, |p| p.z, 1.0, 2.0, &mut out);
        assert_eq!(count, 4);
        assert!(out[..count].iter().all(|p| (1.0..=2.0).contains(&p.z)));
        assert_eq!(clip_to_slab(&square, |p| p.z, 5.0, 6.0, &mut out), 0);
        assert_eq!(clip_to_slab(&square, |p| p.z, f32::NEG_INFINITY, f32::INFINITY, &mut out), 4);
        let segment = [Vec3::ZERO, Vec3::new(0.0, 0.0, 4.0)];
        let count = clip_to_slab(&segment, |p| p.z, 1.0, 3.0, &mut out);
        assert!(count >= 2 && out[..count].iter().all(|p| (1.0..=3.0).contains(&p.z)));
    }
}
