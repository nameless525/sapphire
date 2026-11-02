//! Sapphire's effects simulation (M2 E2, spec §5.16, `docs/m2-plan.md` §3 "VFX v1"): hit sparks,
//! sword trails and dust as documents, spawned on owners and simulated on the CPU, each on its
//! owner's clock, resolved every frame into plain sprites and ribbon points the renderer draws.
//!
//! ```
//! use glam::Vec3;
//! use sapphire_look::VfxFrame;
//! use sapphire_vfx::{EffectLibrary, OwnerId, OwnerSteps, Placement, VfxBudget, VfxWorld};
//!
//! let library = EffectLibrary::default_effects();
//! let spark = library.find("hit_spark").expect("a shipped effect");
//! let mut world = VfxWorld::new(library, VfxBudget::DESKTOP_LOW, 7);
//!
//! // A hit lands on a victim, and hitstop holds its attacker's and its victim's clocks at 0: their
//! // effects freeze where they are, while everything else keeps moving.
//! let victim = OwnerId(2);
//! world.spawn(spark, victim, Placement::at(Vec3::new(0.0, 1.0, 0.0)));
//! world.advance(&OwnerSteps { world: 1.0 / 60.0, owners: &[(victim, 0.0)] });
//!
//! // Each frame: what the renderer draws, in `FramePacket::vfx`.
//! let mut frame = VfxFrame::default();
//! world.write_frame(&mut frame);
//! ```
//!
//! # Owns
//!
//! - **Effect documents** ([`EffectLibrary`], schema 1): effects of emitters, each a set of modules
//!   (`spawn`, `source`, `motion`, `shape`, `facing`, `light`) and curves over a particle's life
//!   ([`Curve`]), and at most one trail, checked whole so an author's mistake names its field.
//!   [`EffectLibrary::default_effects`] compiles in a hit spark, a sword trail and a dust puff.
//! - **The simulation** ([`VfxWorld`]): CPU particles and trail samples, in pools sized from a
//!   [`VfxBudget`] when it starts. Past a cap it counts what it dropped and keeps drawing, and it
//!   allocates nothing per frame once warmed (`tests/allocations.rs`). Its random numbers are
//!   seeded, so a golden image's particles land the same way in every run.
//! - **Clocks** (spec §4.5): every effect runs on one [`OwnerId`]'s clock, through a plain time step
//!   per owner the caller passes ([`OwnerSteps`]). Hitstop is an owner's step of 0, which freezes
//!   its particles, emitters and trail exactly where they are, while world effects keep running;
//!   slow motion is a smaller step.
//! - **Sword trails** ([`VfxWorld::sample_trail`]): two sockets' samples, taken by the caller at
//!   sub-tick rate, smoothed by a centripetal Catmull-Rom spline through each socket's path (the
//!   `trail` module has why that one), the strip's width, color, opacity and bright core following
//!   each point's age.
//!
//! # Does not own
//!
//! - **Drawing:** the renderer's `vfx` module and `vfx.wgsl`. The two share only the plain types in
//!   [`sapphire_look::vfx`], so there's no edge between them (rule R4).
//! - **What triggers effects:** gameplay's cues reach this crate through the app (E3). Effects read
//!   cues and never affect them: this is presentation, outside the simulation's checksum, and nothing
//!   here feeds back (R3).
//! - **The sockets:** plain points a caller passes, from a pose once animation has them (N2).
//! - **The clock:** rule R2. The caller reads owners' clocks; this crate reads none, so it can't time
//!   itself either: [`sapphire_look::VfxStats::simulation`] is the app's measure.
//!
//! # Later milestones
//!
//! GPU particles, flipbooks and mesh particles, distortion, decals, and half-resolution smoke on
//! phones (spec §5.16); more ribbons than trails (E2's plan row cuts them first); effects in cooked
//! packs and hot reload. [`VfxWorld::set_library`] is the hook a reload takes.

mod budget;
mod curve;
mod document;
mod rng;
mod trail;
mod world;

pub use budget::VfxBudget;
pub use curve::{Curve, CurveValue, MAX_KEYS};
pub use document::{
    Effect, EffectId, EffectLibrary, EffectLibraryError, EmitterDesc, Facing, Light, MAX_EMITTERS,
    MAX_TRAIL_SAMPLES, Motion, Range, Shape, Source, Spawn, TrailCore, TrailDesc,
};
pub use world::{EffectHandle, OwnerId, OwnerSteps, Placement, VfxWorld};

#[cfg(test)]
mod tests;
