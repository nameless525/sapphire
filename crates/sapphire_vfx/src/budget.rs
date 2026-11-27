//! Per-tier caps: a plain table, since this crate has no edge to the renderer's quality tiers. An
//! app picks a row from its active tier; `tiers.ron`'s `max_particles` is §3.4's count of GPU
//! particles, which are a later package, and far above what the CPU path should simulate.

/// How much effects may hold at once. Everything is sized from it when a [`VfxWorld`](crate::VfxWorld)
/// starts: past a cap the simulation counts what it dropped and keeps drawing, never growing.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Ground decals a frame draws (M5 L7). Past it the lowest priority aren't drawn that frame, and are
    /// counted: a telegraph should outrank a scorch, so a crowded frame keeps its warnings.
    pub max_decals: u32,
    /// Effect meshes bending the picture a frame draws (M5 L6). Past it the lowest priority bend nothing that
    /// frame, and are counted. 0 writes none, so the renderer makes nothing of them: Mobile-Low's.
    pub max_distortions: u32,
    /// Whether a decal's edge is broken by its noise: off, a scorch draws a clean edge.
    pub decal_noise: bool,
    /// What a decal's fixed life is multiplied by, 0 to 1: under 1, scorches go sooner. A telegraph's life,
    /// till its strike, is never shortened.
    pub decal_life: f32,
    /// Whether a noise force runs only every other step of its owner's clock, over both steps' time (M6 N2's
    /// switch, should its measure find noise too dear at a tier's cap): its swirl then turns on twos, as an
    /// anime drawing does. Off on every row.
    pub noise_on_alternate_steps: bool,
    /// Events a step fires (M6 N3, [`ParticleEvent`](crate::ParticleEvent)): births, deaths and first meetings
    /// with the ground that start a sub-emitter's burst. Past it they're dropped and counted, never kept for a
    /// later step, so a step's cost has a bound however many particles die in it.
    pub max_events: u32,
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
        max_decals: 24,
        max_distortions: 8,
        decal_noise: true,
        decal_life: 1.0,
        noise_on_alternate_steps: false,
        max_events: 64,
    };
    pub const DESKTOP_LOW: Self = Self {
        max_particles: 2048,
        max_ribbon_points: 1024,
        max_effects: 96,
        max_trails: 12,
        trail_subdivisions: 3,
        max_meshes: 24,
        max_decals: 16,
        max_distortions: 8,
        decal_noise: true,
        decal_life: 1.0,
        noise_on_alternate_steps: false,
        max_events: 48,
    };
    pub const MOBILE_HIGH: Self = Self {
        max_particles: 1024,
        max_ribbon_points: 768,
        max_effects: 64,
        max_trails: 8,
        trail_subdivisions: 3,
        max_meshes: 16,
        max_decals: 12,
        max_distortions: 4,
        decal_noise: true,
        decal_life: 1.0,
        noise_on_alternate_steps: false,
        max_events: 32,
    };
    pub const MOBILE_LOW: Self = Self {
        max_particles: 512,
        max_ribbon_points: 384,
        max_effects: 48,
        max_trails: 6,
        trail_subdivisions: 2,
        max_meshes: 8,
        max_decals: 8,
        max_distortions: 0,
        decal_noise: false,
        decal_life: 0.6,
        noise_on_alternate_steps: false,
        max_events: 16,
    };
}

impl Default for VfxBudget {
    fn default() -> Self {
        Self::DESKTOP_LOW
    }
}
