//! Controller observations shared by the presentation adapter and the fixed body owner.

use sapphire_math::Vec3;

use crate::{LocomotionInput, pose_math::PoseMath};

/// A simulation step's raw controller motion and ground events.
#[derive(Clone, Copy, Debug)]
pub struct MotionObservation {
    pub velocity: Vec3,
    pub yaw: f32,
    pub grounded: bool,
    pub preparing_jump: bool,
    pub moving_jump_preparation: Option<f32>,
    pub jumped: bool,
    pub left_ground: bool,
    pub landed: bool,
}

impl MotionObservation {
    /// A legacy view without accumulated departure or touchdown events.
    #[must_use]
    pub fn legacy_input(self) -> LocomotionInput {
        self.input(PoseMath::Legacy)
    }

    fn input(self, math: PoseMath) -> LocomotionInput {
        let across = Vec3::new(self.velocity.x, 0.0, self.velocity.z);
        let speed = across.length();
        let direction = if speed > 1e-3 { math.atan2(across.x, across.z) - self.yaw } else { 0.0 };
        LocomotionInput {
            speed,
            direction,
            grounded: self.grounded,
            preparing_jump: self.preparing_jump,
            moving_jump_preparation: self.moving_jump_preparation,
            ..LocomotionInput::default()
        }
    }
}

/// A departure persists during flight. A touchdown is consumed by the next owner evaluation.
#[derive(Clone, Copy, Debug, Default)]
pub struct MotionHistory {
    math: PoseMath,
    departure: Option<f32>,
    touchdown: Option<f32>,
}

impl MotionHistory {
    /// The simulation-owned cursor uses the pinned direction and forward-speed arithmetic.
    #[must_use]
    pub fn fixed() -> Self {
        Self { math: PoseMath::Pinned, ..Self::default() }
    }

    pub fn clear(&mut self) {
        self.departure = None;
        self.touchdown = None;
    }

    pub fn observe(&mut self, sample: MotionObservation) {
        let motion = sample.input(self.math);
        if !motion.grounded && (sample.jumped || sample.left_ground) {
            self.departure = Some(motion.speed);
        }
        if sample.landed {
            self.touchdown = Some(motion.speed * self.math.cos(motion.direction));
        }
    }

    pub fn input(&mut self, sample: MotionObservation) -> LocomotionInput {
        let mut motion = sample.input(self.math);
        motion.departure_speed = self.departure;
        motion.touchdown_forward_speed = self.touchdown.take();
        motion.landed = motion.touchdown_forward_speed.is_some();
        if motion.grounded {
            self.departure = None;
        }
        motion
    }

    pub fn write_canonical(&self, sink: &mut impl FnMut(&[u8])) {
        sink(&[u8::from(self.math == PoseMath::Pinned)]);
        for event in [self.departure, self.touchdown] {
            sink(&[u8::from(event.is_some())]);
            if let Some(value) = event {
                let bits = if value.is_nan() { f32::NAN.to_bits() } else { value.to_bits() };
                sink(&bits.to_le_bytes());
            }
        }
    }
}
