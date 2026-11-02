//! An orbit camera for inspecting a model.
//!
//! Input moves goal angles and distance; springs on the real clock ease the camera toward
//! them, so the view stays smooth when the world is slowed or paused (the camera owns its
//! clock, spec §4.5).

use std::f32::consts::{FRAC_PI_2, PI};

use sapphire_ecs::prelude::Resource;
use sapphire_math::{Mat4, Quat, SpringDamper, Vec3};
use sapphire_render::CameraData;

const FOV_Y: f32 = 0.75;
const NEAR: f32 = 0.05;
const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.05;
/// Seconds for the camera to close half the gap to its goal.
const HALF_LIFE: f32 = 0.06;

#[derive(Resource)]
pub(crate) struct OrbitCamera {
    target: Vec3,
    /// The vertical field of view, radians.
    fov_y: f32,
    min_distance: f32,
    max_distance: f32,
    goal_yaw: f32,
    goal_pitch: f32,
    goal_distance: f32,
    yaw: SpringDamper<f32>,
    pitch: SpringDamper<f32>,
    distance: SpringDamper<f32>,
    /// What cues' shakes do to the view this frame ([`Self::set_shake`]).
    shake: (Vec3, Vec3),
}

impl OrbitCamera {
    /// A camera framing the object-space bounds `[min, max]` from slightly above and in front.
    pub fn framing(bounds: [[f32; 3]; 2]) -> Self {
        let (min, max) = (Vec3::from(bounds[0]), Vec3::from(bounds[1]));
        let radius = ((max - min).length() * 0.5).max(0.1);
        let distance = radius * 2.6;
        let (yaw, pitch) = (0.35, 0.18);
        Self {
            target: (min + max) * 0.5,
            fov_y: FOV_Y,
            min_distance: radius * 0.6,
            max_distance: radius * 8.0,
            goal_yaw: yaw,
            goal_pitch: pitch,
            goal_distance: distance,
            yaw: SpringDamper::new(yaw, HALF_LIFE),
            pitch: SpringDamper::new(pitch, HALF_LIFE),
            distance: SpringDamper::new(distance, HALF_LIFE),
            shake: (Vec3::ZERO, Vec3::ZERO),
        }
    }

    /// A camera framing a flat arrangement `half_extents` (width, height) across around
    /// `target`, facing it from the default angles, so it fills a 16:9 view: rows of heads
    /// rather than one figure, which [`Self::framing`]'s bounding sphere would leave small.
    pub fn fitting(target: Vec3, half_extents: (f32, f32)) -> Self {
        let tan = (FOV_Y * 0.5).tan();
        let distance = (half_extents.1 / tan).max(half_extents.0 / (tan * 16.0 / 9.0)) * 1.08;
        let mut camera = Self::framing([target.to_array(); 2]);
        camera.min_distance = distance * 0.3;
        camera.max_distance = distance * 4.0;
        camera.goal_distance = distance;
        camera.distance = SpringDamper::new(distance, HALF_LIFE);
        camera
    }

    /// An authored shot: looking at `target` from `yaw` and `pitch` radians at `distance` meters,
    /// through a lens `fov_y` radians high. Zoom ranges from a third to four times the distance.
    pub fn shot(target: Vec3, yaw: f32, pitch: f32, distance: f32, fov_y: f32) -> Self {
        let pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
        Self {
            target,
            fov_y,
            min_distance: distance / 3.0,
            max_distance: distance * 4.0,
            goal_yaw: yaw,
            goal_pitch: pitch,
            goal_distance: distance,
            yaw: SpringDamper::new(yaw, HALF_LIFE),
            pitch: SpringDamper::new(pitch, HALF_LIFE),
            distance: SpringDamper::new(distance, HALF_LIFE),
            shake: (Vec3::ZERO, Vec3::ZERO),
        }
    }

    /// The same camera `factor` times farther from its target, at once and as its zoom's limit
    /// too: a wider shot of the same subject.
    pub fn pulled_back(mut self, factor: f32) -> Self {
        self.goal_distance *= factor;
        self.max_distance *= factor;
        self.distance = SpringDamper::new(self.goal_distance, HALF_LIFE);
        self
    }

    /// The camera's default angles, as a rotation turning +Z toward it: what a model facing +Z
    /// turns by to face the camera, upright.
    pub fn facing_rotation(&self) -> Quat {
        Quat::from_rotation_y(self.goal_yaw) * Quat::from_rotation_x(-self.goal_pitch)
    }

    /// Moves straight to `yaw` and `pitch` in radians, without easing: a starting view. Pitch is
    /// held short of straight up and down, as orbiting holds it.
    pub fn look_from(&mut self, yaw: f32, pitch: f32) {
        let pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
        (self.goal_yaw, self.goal_pitch) = (yaw, pitch);
        self.yaw = SpringDamper::new(yaw, HALF_LIFE);
        self.pitch = SpringDamper::new(pitch, HALF_LIFE);
    }

    /// Moves straight to `pitch` radians, keeping the yaw: a starting view.
    pub fn set_pitch(&mut self, pitch: f32) {
        self.look_from(self.goal_yaw, pitch);
    }

    /// Sets the vertical field of view to `fov_y` radians and moves the camera so what it frames
    /// keeps its size in the image: a narrow lens from farther away, a wide one from closer.
    pub fn set_fov(&mut self, fov_y: f32) {
        let factor = (self.fov_y * 0.5).tan() / (fov_y * 0.5).tan();
        self.fov_y = fov_y;
        self.min_distance *= factor;
        self.max_distance *= factor;
        self.goal_distance *= factor;
        self.distance = SpringDamper::new(self.goal_distance, HALF_LIFE);
    }

    /// Moves straight to `factor` times the distance, without easing (within the zoom's limits):
    /// a starting view.
    pub fn set_zoom(&mut self, factor: f32) {
        self.goal_distance = (self.goal_distance * factor).clamp(self.min_distance, self.max_distance);
        self.distance = SpringDamper::new(self.goal_distance, HALF_LIFE);
    }

    /// Orbits by the given angles in radians. Positive yaw orbits to the right.
    pub fn rotate(&mut self, yaw: f32, pitch: f32) {
        self.goal_yaw += yaw;
        self.goal_pitch = (self.goal_pitch + pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Multiplies the distance by `factor`: below 1 moves closer.
    pub fn zoom(&mut self, factor: f32) {
        self.goal_distance = (self.goal_distance * factor).clamp(self.min_distance, self.max_distance);
    }

    /// Aims at `target` from the same angles and distance: a camera that follows something.
    pub fn set_target(&mut self, target: Vec3) {
        self.target = target;
    }

    /// Takes a view something else worked out: `eye`, looking along `forward` through a lens `fov_y`
    /// radians high, upright, at once. So everything that reads this camera (the frame, sound, what
    /// the stick turns by) reads that view. `sapphire_camera`'s director comes through here in the
    /// arena, and a shake's roll would need more than this.
    pub fn set_view(&mut self, eye: Vec3, forward: Vec3, fov_y: f32) {
        // Any distance gives the same view.
        const DISTANCE: f32 = 4.0;
        let forward = forward.normalize_or(Vec3::NEG_Z);
        let yaw = (-forward.x).atan2(-forward.z);
        let pitch = (-forward.y).asin();
        self.target = eye + forward * DISTANCE;
        self.fov_y = fov_y;
        (self.goal_yaw, self.goal_pitch, self.goal_distance) = (yaw, pitch, DISTANCE);
        self.yaw = SpringDamper::new(yaw, HALF_LIFE);
        self.pitch = SpringDamper::new(pitch, HALF_LIFE);
        self.distance = SpringDamper::new(DISTANCE, HALF_LIFE);
    }

    /// Eases toward the goals over `dt` real seconds.
    pub fn update(&mut self, dt: f32) {
        self.yaw.update(self.goal_yaw, dt);
        self.pitch.update(self.goal_pitch, dt);
        self.distance.update(self.goal_distance, dt);
    }

    /// Where it looks across the ground, as an actor's yaw: +Z at 0, turning toward +X. It stands
    /// on the far side of its target from there, so this is half a turn from the yaw it orbits by.
    pub fn heading(&self) -> f32 {
        self.yaw.value + PI
    }

    /// What cues' shakes do to the view, as `sapphire_camera::ShakeStack::sample` gives it: radians about
    /// its right, up and backward, then metres along its right, up and forward. Input, the orbit's
    /// springs and the listener don't see it, so it can't move what the player aims by.
    pub fn set_shake(&mut self, turn: Vec3, offset: Vec3) {
        self.shake = (turn, offset);
    }

    pub fn camera_data(&self, aspect: f32) -> CameraData {
        let mut data = CameraData::look_at(self.eye(), self.target, Vec3::Y, self.fov_y, aspect, NEAR);
        let (turn, offset) = self.shake;
        if turn != Vec3::ZERO || offset != Vec3::ZERO {
            let moved = Vec3::new(offset.x, offset.y, -offset.z);
            data.position = data.view.inverse().transform_point3(moved);
            // Small angles: half of each, plus a scalar of 1, normalized.
            let small = Quat::from_xyzw(turn.x * 0.5, turn.y * 0.5, turn.z * 0.5, 1.0).normalize();
            data.view = Mat4::from_quat(small.inverse()) * Mat4::from_translation(-moved) * data.view;
        }
        data
    }

    /// Where sound is heard from: the camera, facing what it looks at.
    pub fn listener(&self) -> sapphire_audio::Listener {
        let eye = self.eye();
        sapphire_audio::Listener { position: eye, forward: self.target - eye, up: Vec3::Y }
    }

    /// Where the camera is, in world space.
    pub fn eye(&self) -> Vec3 {
        let (yaw, pitch) = (self.yaw.value, self.pitch.value);
        let offset =
            Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos()) * self.distance.value;
        self.target + offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_the_bounds_and_clamps_input() {
        let mut camera = OrbitCamera::framing([[-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]]);
        assert_eq!(camera.target, Vec3::new(0.0, 1.0, 0.0));

        camera.rotate(0.0, 10.0);
        assert!(camera.goal_pitch <= PITCH_LIMIT);
        camera.zoom(1e-6);
        assert_eq!(camera.goal_distance, camera.min_distance);

        for _ in 0..240 {
            camera.update(1.0 / 60.0);
        }
        let eye = camera.camera_data(16.0 / 9.0).position;
        assert!((eye.distance(camera.target) - camera.min_distance).abs() < 1e-3);
    }

    /// A shake moves the view by what it says and touches nothing else.
    #[test]
    fn a_shake_moves_the_view_and_nothing_else() {
        let mut camera = OrbitCamera::framing([[-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]]);
        let still = camera.camera_data(1.0);
        let (eye, listener) = (camera.eye(), camera.listener());

        camera.set_shake(Vec3::ZERO, Vec3::new(0.01, 0.02, 0.03));
        let moved = camera.camera_data(1.0);
        let inverse = still.view.inverse();
        let (right, up, back) =
            (inverse.x_axis.truncate(), inverse.y_axis.truncate(), inverse.z_axis.truncate());
        let expected = eye + right * 0.01 + up * 0.02 - back * 0.03;
        assert!(moved.position.abs_diff_eq(expected, 1e-5), "{} {expected}", moved.position);
        assert!(moved.view.inverse().w_axis.truncate().abs_diff_eq(expected, 1e-5), "the view agrees");
        assert_eq!(moved.proj, still.proj);

        camera.set_shake(Vec3::new(0.0, 0.0, 0.02), Vec3::ZERO);
        let turned = camera.camera_data(1.0);
        assert!(turned.position.abs_diff_eq(eye, 1e-5), "a turn leaves the eye");
        let looked = turned.view.inverse().z_axis.truncate();
        assert!(looked.abs_diff_eq(back, 1e-5), "a roll leaves where it looks");
        assert!(!turned.view.inverse().x_axis.truncate().abs_diff_eq(right, 1e-3), "and turns its right");

        assert_eq!(camera.eye(), eye);
        assert_eq!(camera.listener(), listener);
        camera.set_shake(Vec3::ZERO, Vec3::ZERO);
        assert_eq!(camera.camera_data(1.0).view, still.view, "gone with no trace");
    }

    #[test]
    fn a_new_field_of_view_keeps_the_framing() {
        let mut camera = OrbitCamera::framing([[-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]]);
        let before = camera.goal_distance;
        let size = |camera: &OrbitCamera| 1.0 / (camera.goal_distance * (camera.fov_y * 0.5).tan());
        let framed = size(&camera);
        camera.set_fov(15f32.to_radians());
        assert!((size(&camera) - framed).abs() < 1e-5 * framed, "the same size in the image");
        assert!(camera.goal_distance > 2.5 * before, "a narrow lens from farther away");
        let proj = camera.camera_data(1.0).proj;
        assert!((proj.y_axis.y - 1.0 / 7.5f32.to_radians().tan()).abs() < 1e-3, "the new lens");
        camera.set_zoom(0.5);
        assert!((camera.eye().distance(camera.target) - camera.goal_distance).abs() < 1e-4, "no easing");
    }
}
