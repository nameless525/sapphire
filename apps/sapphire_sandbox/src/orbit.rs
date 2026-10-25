//! An orbit camera for inspecting a model.
//!
//! Input moves goal angles and distance; springs on the real clock ease the camera toward
//! them, so the view stays smooth when the world is slowed or paused (the camera owns its
//! clock, spec §4.5).

use std::f32::consts::FRAC_PI_2;

use sapphire_ecs::prelude::Resource;
use sapphire_math::{SpringDamper, Vec3};
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
        let (yaw, pitch) = (self.yaw.value, self.pitch.value);
        let offset =
            Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos()) * self.distance.value;
        CameraData::look_at(self.target + offset, self.target, Vec3::Y, FOV_Y, aspect, NEAR)
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
