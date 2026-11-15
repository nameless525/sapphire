//! Hashes of a frame's CPU-side presentation, by stage (M5, a capture's diagnostic): what a capture logs
//! beside its picture so that two machines' runs of one scene say where they part, ahead of the GPU.
//!
//! - **What's hashed:** each value's bit pattern, in a fixed order, by FNV-1a over 64 bits. Nothing hashes
//!   an address, a time or a thread's work, so a hash is the same at any job-thread count.
//! - **The stages** ([`PresentationHashes`]), each over every character in the order of its entity
//!   (spawn order, the same on any machine for the same run):
//!   - `camera`: the main view's view and projection matrices and eye, as the frame packet hands them over;
//!   - `graph`: each [`GraphDriver`](crate::anim_graph::GraphDriver)'s joint locals as its graph leaves
//!     them, before IK and springs; a body the simulation solved has no graph of its own here, so its
//!     `graph` is the pose it publishes;
//!   - `ik`: the pose as it reaches the springs, after IK (the feet, the look, the hands) or the published
//!     body;
//!   - `springs`: the pose after the secondary-motion step, the springs' and cloth's bones with it;
//!   - each of those three as the stage last ran: a frame drawn again at the same tick (a capture's frames
//!     for temporal AA's history) poses nothing, so the line holds the last frame that did;
//!   - `palettes` and `morphs`: every [`RenderPose`](crate::RenderPose)'s skinning palette and morph
//!     weights, in entity order, as extraction hands them to the GPU (graph-posed or not);
//!   - `instances`: every entity's [`RenderTransform`](crate::extract::RenderTransform), in entity order: the
//!     static world, every character's root and every held weapon;
//!   - `held`: what a fight's fighters hold, their grips and their weapons' transforms, in the roster's
//!     order, which its owner adds ([`PresentationHashes::held`]); `0` with none.
//! - **Off unless asked:** a driver records its stages only while [`Scene::tap_presentation`] has turned it
//!   on, which only a capture does, so an app's frames pay nothing. Nothing here reaches the simulation, a
//!   step or a checksum: it reads presentation after the step.
//!
//! [`Scene::tap_presentation`]: crate::scene::Scene::tap_presentation

use sapphire_math::{Affine3A, Mat4, Transform};

/// FNV-1a over 64 bits, fed values' bit patterns.
#[derive(Clone, Copy, Debug)]
pub struct PresentationHasher(u64);

impl Default for PresentationHasher {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl PresentationHasher {
    pub fn u32(&mut self, value: u32) {
        for byte in value.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub fn u64(&mut self, value: u64) {
        self.u32(value as u32);
        self.u32((value >> 32) as u32);
    }

    pub fn f32s(&mut self, values: &[f32]) {
        self.u32(values.len() as u32);
        for value in values {
            self.u32(value.to_bits());
        }
    }

    pub fn mat4(&mut self, matrix: &Mat4) {
        self.f32s(&matrix.to_cols_array());
    }

    pub fn affines(&mut self, matrices: &[Affine3A]) {
        self.u32(matrices.len() as u32);
        for matrix in matrices {
            self.f32s(&matrix.to_cols_array());
        }
    }

    pub fn transforms(&mut self, locals: &[Transform]) {
        self.u32(locals.len() as u32);
        for local in locals {
            self.f32s(&local.translation.to_array());
            self.f32s(&local.rotation.to_array());
            self.f32s(&local.scale.to_array());
        }
    }

    pub fn finish(self) -> u64 {
        self.0
    }
}

/// A pose's hash: its joint locals, then its model transforms.
pub(crate) fn hash_pose(locals: &[Transform], model: &[Affine3A]) -> u64 {
    let mut hasher = PresentationHasher::default();
    hasher.transforms(locals);
    hasher.affines(model);
    hasher.finish()
}

/// A driver's stages for the frame it last posed ([`PresentationHashes`]'s `graph`, `ik` and `springs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoseTaps {
    pub graph: u64,
    pub ik: u64,
    pub springs: u64,
}

/// A frame's presentation, hashed by stage. See the [module docs](self).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentationHashes {
    pub camera: u64,
    pub graph: u64,
    pub ik: u64,
    pub springs: u64,
    pub palettes: u64,
    pub morphs: u64,
    pub held: u64,
    pub instances: u64,
    /// How many drivers the pose stages folded in.
    pub characters: u32,
}

impl std::fmt::Display for PresentationHashes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { camera, graph, ik, springs, palettes, morphs, held, instances, characters } = self;
        write!(
            f,
            "camera={camera:016x} graph={graph:016x} ik={ik:016x} springs={springs:016x} \
             palettes={palettes:016x} morphs={morphs:016x} held={held:016x} instances={instances:016x} \
             characters={characters}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sapphire_math::{Quat, Vec3};

    #[test]
    fn a_hash_follows_every_bit_and_the_order() {
        let a = Transform { translation: Vec3::X, rotation: Quat::IDENTITY, scale: Vec3::ONE };
        let b = Transform { translation: Vec3::Y, ..a };
        let model = [Affine3A::IDENTITY];
        assert_eq!(hash_pose(&[a, b], &model), hash_pose(&[a, b], &model));
        assert_ne!(hash_pose(&[a, b], &model), hash_pose(&[b, a], &model));
        let nudged =
            Transform { translation: Vec3::new(f32::from_bits(1.0f32.to_bits() + 1), 0.0, 0.0), ..a };
        assert_ne!(hash_pose(&[a], &model), hash_pose(&[nudged], &model), "one ulp moves it");
        assert_ne!(hash_pose(&[a], &model), hash_pose(&[a, a], &model), "the count is in it");
        let moved = [Affine3A::from_translation(Vec3::Z)];
        assert_ne!(hash_pose(&[a], &model), hash_pose(&[a], &moved), "the model is in it");
    }
}
