//! Canonical graph-history fields. This byte order is independent of Rust struct layout.

use sapphire_math::{Affine3A, Transform, Vec2, Vec3};

use super::{Frame, GraphInput, GraphPlayer, Layers};

/// A little-endian sink, with the replay convention for floating-point state.
pub(crate) struct Canonical<'a, F>(pub(crate) &'a mut F);

impl<F: FnMut(&[u8])> Canonical<'_, F> {
    pub(crate) fn u16(&mut self, value: u16) {
        self.0(&value.to_le_bytes());
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.0(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.0(&value.to_le_bytes());
    }

    pub(crate) fn len(&mut self, value: usize) {
        // Only bounded joint, morph, or foot-side indices and counts are written here.
        self.u32(u32::try_from(value).expect("bounded graph state"));
    }

    pub(crate) fn flag(&mut self, value: bool) {
        self.0(&[u8::from(value)]);
    }

    pub(crate) fn f32(&mut self, value: f32) {
        self.u32(if value.is_nan() { f32::NAN.to_bits() } else { value.to_bits() });
    }

    pub(crate) fn f64(&mut self, value: f64) {
        self.u64(if value.is_nan() { f64::NAN.to_bits() } else { value.to_bits() });
    }

    pub(crate) fn floats(&mut self, values: &[f32]) {
        for &value in values {
            self.f32(value);
        }
    }

    pub(crate) fn optional<T>(&mut self, value: Option<T>, write: impl FnOnce(&mut Self, T)) {
        self.flag(value.is_some());
        if let Some(value) = value {
            write(self, value);
        }
    }

    pub(crate) fn transforms(&mut self, joints: &[Transform]) {
        self.len(joints.len());
        for joint in joints {
            self.floats(&joint.translation.to_array());
            self.floats(&joint.rotation.to_array());
            self.floats(&joint.scale.to_array());
        }
    }

    pub(crate) fn vec2(&mut self, value: Vec2) {
        self.floats(&value.to_array());
    }

    pub(crate) fn vec3(&mut self, value: Vec3) {
        self.floats(&value.to_array());
    }

    pub(crate) fn affine(&mut self, value: Affine3A) {
        self.floats(&value.to_cols_array());
    }

    fn frame(&mut self, frame: &Frame) {
        self.transforms(&frame.joints);
        self.len(frame.weights.len());
        self.floats(&frame.weights);
        self.floats(&frame.landing_shift.to_array());
    }

    fn input(&mut self, input: GraphInput) {
        self.f64(input.ticks);
        let locomotion = input.locomotion;
        self.floats(&[locomotion.speed, locomotion.direction]);
        self.flag(locomotion.grounded);
        self.flag(locomotion.preparing_jump);
        self.optional(locomotion.moving_jump_preparation, Self::f32);
        self.flag(locomotion.landed);
        self.optional(locomotion.departure_speed, Self::f32);
        self.optional(locomotion.touchdown_forward_speed, Self::f32);
        self.optional(input.combat, |out, combat| {
            out.u64(combat.name.as_u64());
            out.u32(combat.serial);
            out.f64(combat.position);
        });
        self.u64(input.reaction.name.as_u64());
        self.u32(input.reaction.count);
        self.flag(input.cut);
        self.flag(input.defeated);
    }

    fn layers(&mut self, layers: &Layers) {
        self.u16(layers.state);
        self.f64(layers.started);
        self.optional(layers.landing, |out, landing| {
            out.f64(landing.ready);
            out.u32(landing.serial);
            out.optional(landing.support, Self::len);
        });
        self.optional(layers.departure, Self::f32);
        self.f32(layers.touchdown);
        self.optional(layers.pose_entry, Self::f32);
        self.optional(layers.pose_mix, Self::f32);
        self.floats(&[layers.descent_mix, layers.descent_rate, layers.phase]);
        self.floats(&layers.velocity.to_array());
        self.floats(&layers.velocity_rate.to_array());
        self.floats(&layers.blend);
        self.optional(layers.combat, |out, combat| {
            out.u16(combat.entry);
            out.u32(combat.serial);
            out.f64(combat.position);
        });
        self.optional(layers.reaction, |out, reaction| {
            out.u16(reaction.entry);
            out.f64(reaction.started);
        });
        self.u32(layers.reaction_count);
        self.flag(layers.defeated);
    }

    pub(crate) fn player(&mut self, player: &GraphPlayer) {
        self.layers(&player.layers);
        self.frame(&player.output);
        self.frame(&player.earlier);
        self.f32(player.earlier_step);
        self.optional(player.drawn, Self::f64);
        self.optional(player.key, |out, key| out.input(key.input));
        self.flag(player.gather_requested);
        self.optional(player.gather_started, Self::f64);
        self.optional(player.footwork, |out, footwork| footwork.write_canonical(out));
        self.optional(player.landing_step, |out, step| {
            out.len(step.side);
            out.f64(step.ends);
        });
        self.u32(player.seed);
        self.u32(player.stats.transitions);
        self.floats(&player.target.landing_shift.to_array());
        player.inertia.write_canonical(self);
    }
}
