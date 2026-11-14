//! Per-tier caps: a plain table, since this crate has no edge to the renderer's quality tiers. An
//! app picks a row from its active tier; `tiers.ron`'s `max_particles` is §3.4's count of GPU
//! particles, which are a later package, and far above what the CPU path should simulate.

/// How much effects may hold at once. Everything is sized from it when a [`VfxWorld`](crate::VfxWorld)
/// starts: past a cap the simulation counts what it dropped and keeps drawing, never growing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VfxBudget {
    /// Live particles, and so the most births a step does: what doesn't fit is only counted.
    pub max_particles: u32,
    /// Ribbon points a frame, after smoothing. A ribbon over the cap loses its oldest points.
    pub max_ribbon_points: u32,
    /// Effects playing.
    pub max_effects: u32,
    /// Trails recording at once. A sword trail takes one per blade.
    pub max_trails: u32,
    /// The smoothed points between two socket samples: 1 draws the samples as they are.
    pub trail_subdivisions: u32,
    /// Effect meshes a frame draws (M5 L5). Past it the lowest priority aren't drawn that frame, and are
    /// counted: a mesh is a whole draw, so this caps the effects' draws, not their pixels.
    pub max_meshes: u32,
}

impl VfxBudget {
    /// A duel's worst moment (two sword trails, 200 sparks and 50 dust particles) fits every row,
    /// with room for a second hit landing on top of it.
    pub const DESKTOP_HIGH: Self = Self {
        max_particles: 4096,
        max_ribbon_points: 2048,
        max_effects: 128,
        max_trails: 16,
        trail_subdivisions: 4,
        max_meshes: 32,
    };
    pub const DESKTOP_LOW: Self = Self {
        max_particles: 2048,
        max_ribbon_points: 1024,
        max_effects: 96,
        max_trails: 12,
        trail_subdivisions: 3,
        max_meshes: 24,
    };
    pub const MOBILE_HIGH: Self = Self {
        max_particles: 1024,
        max_ribbon_points: 768,
        max_effects: 64,
        max_trails: 8,
        trail_subdivisions: 3,
        max_meshes: 16,
    };
    pub const MOBILE_LOW: Self = Self {
        max_particles: 512,
        max_ribbon_points: 384,
        max_effects: 48,
        max_trails: 6,
        trail_subdivisions: 2,
        max_meshes: 8,
    };
}

impl Default for VfxBudget {
    fn default() -> Self {
        Self::DESKTOP_LOW
    }
}
