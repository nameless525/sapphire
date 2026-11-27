//! A water body as drawn (M6 W3a): its surface's mesh and its info texture, cooked (`.swater`), a sub-asset of
//! its zone (`town.water.<id>.swater`) that the zone's presentation names
//! ([`CookedWaterSurface::mesh`](super::CookedWaterSurface::mesh)).
//!
//! # The mesh
//!
//! Triangles over the body's rest surface, each facing up, in the zone's space, with what the water family's
//! normal maps and flow scroll by in [`WaterVertex::uv`]:
//!
//! - **A lake's** is its outline grown by its `shore_margin`, triangulated and cut along a square grid laid on
//!   the zone's axes (whole squares inside, a square's share of the outline's triangles at its edge), so its
//!   vertices are as dense as the grid wherever it lies, for waves to displace (H1a), and its edge is the grown
//!   outline exactly. Its UV is its `(x, z)` in metres: still water, its maps tiled in the zone's space.
//! - **A river's** is a ribbon along its points, its banks grown by its margin, mitred at each point and cut
//!   into sections at most the grid's square apart, each the same number of columns across; beside a bend they
//!   stand clear of where its inner bank falls back, so no square turns over. Its surface falls
//!   with its points'. Its UV runs with the flow: `u` across in metres from the middle (the left bank, looking
//!   downstream, negative) and `v` along it in metres from its first point, rising downstream, its two ends
//!   grown by the margin too. The volume's rounded ends, half a width past its first and last points, aren't
//!   drawn: a river ends in a lake, the sea or the zone's edge.
//!
//! # The info texture
//!
//! Top-down over the drawn surface and a texel round it, `width × height` texels of `Rgba8Unorm` (never
//! sRGB, never block-compressed: they're data), rows along +X from `origin`, row after row along +Z; a texel's
//! centre is `origin + (i + 0.5, j + 0.5) × texel`. A renderer samples it with a linear clamped sampler at
//! `((x, z) − origin) / (size × texel)`. Each channel, as a byte `b` (`b = 255 × the sampled value`):
//!
//! - **R, the bed's depth:** how far under the rest surface its floor is, `b × DEPTH_STEP` metres, 255 for
//!   [`MAX_DEPTH`] or deeper or no floor under it at all, 0 on land. The floor is the highest face of the
//!   zone's static colliders that looks up from at or below the surface (terrain's once W12 lands), so a bridge
//!   over a river keeps its bed. What W3b's alpha by depth and its bands read.
//! - **G, the signed distance to the shore:** `(b − 128) × SHORE_STEP` metres, positive in the water and
//!   negative on land, from −32 to +31.75 m: the distance from a texel's centre to the nearest texel's of the
//!   other side, less half a texel, so the two texels either side of a shore read ± half a texel and the
//!   filtered value crosses 0 between them. Water is where the body's volume is (its outline, a river's
//!   banks), its floor lies under its surface and no closed solid (a rock's or an island's box, sphere or
//!   capsule) stands through it there; a bank with nothing under it is land, and under a bridge is water. What
//!   the foam line, the shore's wet film
//!   and shore waves read (W3b, H2).
//! - **B and A, the flow:** its `x` and `z` in metres a second, `(b − 128) / 127 × flow_scale`: the
//!   simulation's own current ([`WaterVolumes::at`](super::WaterVolumes::at)'s, downstream along a river's
//!   stretch) in the water, and the nearest water texel's on land, so the drawn margin flows with its river;
//!   still (128) on a lake. What H3b's two-phase flow reads beside the UV.
//!
//! Everything here is plain data: a cook makes it on `det`'s maths and integer arithmetic, so it's the same
//! bytes on every machine (the pipeline's `zone::water`).

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::AlignedVec;

use super::WaterKind;
use crate::document::{ZoneError, check_position};
use crate::grid::MAX_COORDINATE;

/// Conventional file extension for a cooked water body's surface.
pub const WATER_MESH_EXTENSION: &str = "swater";

/// The longest an info texture's side may be, in texels.
pub const MAX_INFO_SIDE: u32 = 512;

/// The most vertices a body's mesh may hold.
pub const MAX_VERTICES: usize = 1 << 20;

/// The most triangles a body's mesh may hold.
pub const MAX_TRIANGLES: usize = 1 << 21;

/// What one step of the info texture's depth (R) is, in metres.
pub const DEPTH_STEP: f32 = 0.1;

/// The deepest the info texture's depth tells, in metres: 255 steps, or anything deeper.
pub const MAX_DEPTH: f32 = 25.5;

/// What one step of the info texture's shore distance (G) is, in metres.
pub const SHORE_STEP: f32 = 0.25;

/// The byte a signed channel (shore distance and flow) holds its zero at.
pub const SIGNED_ZERO: u8 = 128;

/// One of a body's mesh's vertices: 20 bytes, as a vertex stream reads it.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    bytemuck::Pod,
    bytemuck::Zeroable,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[repr(C)]
pub struct WaterVertex {
    /// On the rest surface, in the zone's space.
    pub position: [f32; 3],
    /// A lake's `(x, z)`; a river's `(across, along)`, in metres (see the [module docs](self#the-mesh)).
    pub uv: [f32; 2],
}

/// A body's info texture (see the [module docs](self#the-info-texture)).
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct InfoTexture {
    /// The corner of its first texel, `(x, z)`, in metres: a multiple of `texel`.
    pub origin: [f32; 2],
    /// A texel's side, in metres: a power of two.
    pub texel: f32,
    pub width: u32,
    pub height: u32,
    /// What the flow channels' full reach is, in metres a second: the body's fastest current, 0 for still
    /// water.
    pub flow_scale: f32,
    /// `width × height` texels, RGBA, row after row.
    pub texels: Vec<[u8; 4]>,
}

/// A water body as drawn, cooked (`.swater`): see the [module docs](self). An rkyv archive read in place.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::zone"]
pub struct WaterMesh {
    /// Its body's ID, its name's hash, as its volume's.
    pub body: u64,
    /// A lake or a river: the sea has no mesh.
    pub kind: WaterKind,
    pub vertices: Vec<WaterVertex>,
    /// Three a triangle, each facing up.
    pub indices: Vec<u32>,
    /// Around its vertices, in the zone's space.
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub info: InfoTexture,
}

impl Versioned for WaterMesh {
    /// 1: M6 W3a.
    const VERSION: u32 = 1;
}

impl WaterMesh {
    pub fn to_cooked(&self) -> Result<AlignedVec, ZoneError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value a reader leans on.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedWaterMesh, ZoneError> {
        let mesh = sapphire_serialize::access::<Self>(bytes)?;
        mesh.validate()?;
        Ok(mesh)
    }
}

impl ArchivedWaterMesh {
    /// Checks everything a reader leans on, whatever wrote these bytes: a lake or a river; a mesh of whole
    /// triangles within its counts, each index naming a vertex, each vertex finite, within the zone's reach
    /// and within its bounds; an info texture of a power-of-two texel, its origin on its own grid, its sides
    /// from 1 to [`MAX_INFO_SIDE`] and holding them exactly, and a flow scale that's a speed.
    pub fn validate(&self) -> Result<(), ZoneError> {
        let invalid = |problem: String| ZoneError::invalid("water mesh", problem);
        if self.kind == super::ArchivedWaterKind::Ocean {
            return Err(invalid("the sea has no mesh".to_owned()));
        }
        let (vertices, indices) = (self.vertices.len(), self.indices.len());
        if vertices > MAX_VERTICES || indices % 3 != 0 || indices / 3 > MAX_TRIANGLES {
            return Err(invalid(format!(
                "{vertices} vertices and {indices} indices aren't whole triangles within its counts"
            )));
        }
        if let Some(index) =
            self.indices.iter().map(|index| index.to_native()).find(|&i| i as usize >= vertices)
        {
            return Err(invalid(format!("index {index} names no vertex of {vertices}")));
        }
        let native3 = |values: &rkyv::Archived<[f32; 3]>| values.each_ref().map(|value| value.to_native());
        let (min, max) = (native3(&self.bounds_min), native3(&self.bounds_max));
        check_position(min).and_then(|()| check_position(max)).map_err(invalid)?;
        for (index, vertex) in self.vertices.iter().enumerate() {
            let position = native3(&vertex.position);
            let uv = vertex.uv.each_ref().map(|value| value.to_native());
            check_position(position).map_err(|problem| invalid(format!("vertex {index}: {problem}")))?;
            let inside = (0..3).all(|axis| min[axis] <= position[axis] && position[axis] <= max[axis]);
            if !inside || !uv.iter().all(|value| value.is_finite()) {
                return Err(invalid(format!("vertex {index} lies outside its bounds or has no UV")));
            }
        }
        self.info.validate().map_err(invalid)
    }

    /// How many triangles its mesh holds.
    #[must_use]
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

impl ArchivedInfoTexture {
    fn validate(&self) -> Result<(), String> {
        let texel = self.texel.to_native();
        let (width, height) = (self.width.to_native(), self.height.to_native());
        let power_of_two =
            texel.is_finite() && texel > 0.0 && texel <= MAX_COORDINATE && texel.to_bits() & 0x007f_ffff == 0;
        if !power_of_two {
            return Err(format!("its info texture's texel {texel} isn't a power of two metres"));
        }
        if !(1..=MAX_INFO_SIDE).contains(&width) || !(1..=MAX_INFO_SIDE).contains(&height) {
            return Err(format!(
                "its info texture's {width} × {height} isn't from 1 to {MAX_INFO_SIDE} a side"
            ));
        }
        if self.texels.len() != width as usize * height as usize {
            return Err(format!(
                "its info texture holds {} texels for {width} × {height}",
                self.texels.len()
            ));
        }
        let origin = self.origin.each_ref().map(|value| value.to_native());
        check_position([origin[0], 0.0, origin[1]])?;
        // A texel's side is a power of two, so the division and the floor are exact.
        if origin.iter().any(|&value| (value / texel).floor() * texel != value) {
            return Err(format!("its info texture's origin {origin:?} isn't on its {texel} m grid"));
        }
        let flow_scale = self.flow_scale.to_native();
        if !(flow_scale.is_finite() && (0.0..=super::MAX_RIVER_SPEED).contains(&flow_scale)) {
            return Err(format!("its flow scale {flow_scale} isn't a current's speed"));
        }
        Ok(())
    }

    /// The texel at `(i, j)`: along +X, then +Z.
    #[must_use]
    pub fn texel(&self, i: u32, j: u32) -> [u8; 4] {
        self.texels[(j * self.width.to_native() + i) as usize]
    }

    /// The bed's depth a byte of R says, in metres: [`MAX_DEPTH`] for 255, deeper or no floor.
    #[must_use]
    pub fn depth(byte: u8) -> f32 {
        f32::from(byte) * DEPTH_STEP
    }

    /// The signed distance to the shore a byte of G says, in metres: positive in the water.
    #[must_use]
    pub fn shore(byte: u8) -> f32 {
        (f32::from(byte) - f32::from(SIGNED_ZERO)) * SHORE_STEP
    }

    /// The flow a byte of B or A says, in metres a second.
    #[must_use]
    pub fn flow(&self, byte: u8) -> f32 {
        (f32::from(byte) - f32::from(SIGNED_ZERO)) / 127.0 * self.flow_scale.to_native()
    }
}

/// The part a body's surface is among its zone's sub-assets: `water/<its ID in hex>`. Never changed: its ID is
/// derived from it.
#[must_use]
pub fn water_part(body: u64) -> String {
    format!("water/{body:016x}")
}
