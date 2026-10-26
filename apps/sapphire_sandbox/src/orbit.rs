//! An orbit camera for inspecting a model.
//!
//! Input moves goal angles and distance; springs on the real clock ease the camera toward
//! them, so the view stays smooth when the world is slowed or paused (the camera owns its
//! clock, spec §4.5).

use std::f32::consts::FRAC_PI_2;

use sapphire_ecs::prelude::Resource;
use sapphire_math::{Quat, SpringDamper, Vec3};
use sapphire_render::CameraData;

const FOV_Y: f32 = 0.75;
const NEAR: f32 = 0.05;
const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.05;
/// Seconds for the camera to close half the gap to its goal.
const HALF_LIFE: f32 = 0.06;

#[derive(Resource)]
pub(crate) struct OrbitCamera {
    target: Vec3,
    min_distance: f32,
    max_distance: f32,
    goal_yaw: f32,
    goal_pitch: f32,
    goal_distance: f32,
    yaw: SpringDamper<f32>,
    pitch: SpringDamper<f32>,
    distance: SpringDamper<f32>,
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
            min_distance: radius * 0.6,
            max_distance: radius * 8.0,
            goal_yaw: yaw,
            goal_pitch: pitch,
            goal_distance: distance,
            yaw: SpringDamper::new(yaw, HALF_LIFE),
            pitch: SpringDamper::new(pitch, HALF_LIFE),
            distance: SpringDamper::new(distance, HALF_LIFE),
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

    /// Orbits by the given angles in radians. Positive yaw orbits to the right.
    pub fn rotate(&mut self, yaw: f32, pitch: f32) {
        self.goal_yaw += yaw;
        self.goal_pitch = (self.goal_pitch + pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Multiplies the distance by `factor`: below 1 moves closer.
    pub fn zoom(&mut self, factor: f32) {
        self.goal_distance = (self.goal_distance * factor).clamp(self.min_distance, self.max_distance);
    }

    /// Eases toward the goals over `dt` real seconds.
    pub fn update(&mut self, dt: f32) {
        self.yaw.update(self.goal_yaw, dt);
        self.pitch.update(self.goal_pitch, dt);
        self.distance.update(self.goal_distance, dt);
    }

    pub fn camera_data(&self, aspect: f32) -> CameraData {
        CameraData::look_at(self.eye(), self.target, Vec3::Y, FOV_Y, aspect, NEAR)
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
}
