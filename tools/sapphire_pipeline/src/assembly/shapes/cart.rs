//! `#cart`: a hand cart, with its goods.
//!
//! A cart ([`Cart`]) is an open bed of boards on two spoked wheels, iron-tyred: a felloe round
//! a turned hub, spokes between. Two shafts run under the bed and on forward to a crossbar
//! handle, and two legs under them keep the bed level at rest. Its front (the shafts) is toward
//! +Z. Its goods ([`Goods`]) stand in its bed and on the ground beside it.

use std::f32::consts::{FRAC_PI_2, TAU};

use glam::{Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use super::goods::{Goods, Place, Surface, validate_goods};
use super::{Detail, Mesh, Part, check, finite};

/// A cart's timbers: the axle's, the shafts', the legs' and the crossbar's square sections, and
/// the bed's floor and side boards' thickness.
const AXLE: f32 = 0.07;
const SHAFT: f32 = 0.07;
const LEG: f32 = 0.06;
const CROSSBAR: f32 = 0.05;
const FLOOR: f32 = 0.04;
const BOARD: f32 = 0.03;
/// Between the bed's side and a wheel.
const WHEEL_GAP: f32 = 0.05;
/// The wheels' iron tyre's thickness, their hub's radius and length, and their spokes' width
/// and thickness.
const TYRE: f32 = 0.015;
const HUB: [f32; 2] = [0.075, 0.18];
const SPOKE: [f32; 2] = [0.035, 0.03];
/// Faces round a wheel and its hub in full detail (light detail takes [`LIGHT_WHEEL`] and 6).
const WHEEL_SIDES: u32 = 20;
const LIGHT_WHEEL: u32 = 12;
const HUB_SIDES: u32 = 10;

/// A hand cart (`#cart`); see the module docs. It stands on its origin, its shafts toward +Z,
/// its wheels' axle a little behind the bed's middle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cart {
    /// The bed: width (X), length (Z), and its sides' height over its floor, meters.
    pub bed: [f32; 3],
    pub wheels: Wheels,
    /// How far the shafts reach past the bed's front.
    pub shafts: f32,
    pub goods: Vec<Goods>,
}

impl Default for Cart {
    fn default() -> Self {
        Self { bed: [1.1, 1.7, 0.3], wheels: Wheels::default(), shafts: 0.9, goods: Vec::new() }
    }
}

/// A cart's wheels: their radius to the tyre, their spokes, and their rim's depth (from the
/// tyre in) and width (along the axle).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Wheels {
    pub radius: f32,
    pub spokes: u32,
    pub rim: [f32; 2],
}

impl Default for Wheels {
    fn default() -> Self {
        Self { radius: 0.45, spokes: 10, rim: [0.07, 0.07] }
    }
}

impl Cart {
    pub fn validate(&self) -> Result<(), String> {
        let [width, length, sides] = self.bed;
        let Wheels { radius, spokes, rim } = self.wheels;
        check(finite(&[width, length, sides, radius, rim[0], rim[1], self.shafts]), || {
            "a cart's numbers must be finite".to_owned()
        })?;
        check(width > 2.0 * SHAFT + 0.1 && length > 0.4 && sides >= 0.0 && self.shafts >= 0.0, || {
            format!("a cart's bed {:?} must be wide and long enough for its shafts", self.bed)
        })?;
        check((0.2..=1.5).contains(&radius) && (3..=24).contains(&spokes), || {
            format!("a cart's wheels need a radius of 0.2 to 1.5 m and 3 to 24 spokes, not {spokes}")
        })?;
        check(
            rim[0] > TYRE + 0.01 && rim[0] < radius - HUB[0] - 0.05 && rim[1] > 0.02 && rim[1] < 0.3,
            || "a wheel's rim must be deeper than its tyre, clear of its hub, and 2 to 30 cm wide".to_owned(),
        )?;
        validate_goods(&self.goods, &|surface| self.place(surface), self.plan().footprint())
    }

    fn plan(&self) -> CartPlan {
        let [width, length, _] = self.bed;
        let radius = self.wheels.radius;
        let shaft_low = radius + 0.5 * AXLE;
        let floor_low = shaft_low + SHAFT;
        CartPlan {
            half: Vec2::new(0.5 * width, 0.5 * length),
            shaft_low,
            floor_low,
            floor_top: floor_low + FLOOR,
            shaft_x: 0.5 * width - 0.02 - 0.5 * SHAFT,
            wheel_x: 0.5 * width + WHEEL_GAP + 0.5 * self.wheels.rim[1],
            axle_z: -0.12 * length,
            front: 0.5 * length + self.shafts,
            radius,
        }
    }

    fn place(&self, surface: Surface) -> Result<Place, String> {
        let plan = self.plan();
        match surface {
            Surface::Bed => Ok(Place {
                middle: Vec3::Y * plan.floor_top,
                half: plan.half - Vec2::splat(BOARD),
                room: f32::INFINITY,
            }),
            Surface::Ground => Ok(Place::ground()),
            Surface::Counter | Surface::Shelf(_) => {
                Err("a cart's goods stand on its bed or the ground".to_owned())
            }
        }
    }

    pub(super) fn build(&self, mesh: &mut Mesh, detail: Detail) -> Result<(), String> {
        let plan = self.plan();
        let sides = self.bed[2];
        let (hx, hz) = (plan.half.x, plan.half.y);
        // The bed: its floor, and its sides on it.
        mesh.cuboid(
            Part::Boards,
            Vec3::new(-hx, plan.floor_low, -hz),
            Vec3::new(hx, plan.floor_top, hz),
            &[],
        );
        if sides > 0.0 {
            let (low, high) = (plan.floor_top, plan.floor_top + sides);
            for x in [1.0, -1.0] {
                let (a, b) = if x > 0.0 { (hx - BOARD, hx) } else { (-hx, -hx + BOARD) };
                mesh.cuboid(Part::Boards, Vec3::new(a, low, -hz), Vec3::new(b, high, hz), &[-Vec3::Y]);
            }
            for z in [1.0, -1.0] {
                let (a, b) = if z > 0.0 { (hz - BOARD, hz) } else { (-hz, -hz + BOARD) };
                mesh.cuboid(
                    Part::Boards,
                    Vec3::new(-hx + BOARD, low, a),
                    Vec3::new(hx - BOARD, high, b),
                    &[-Vec3::Y, Vec3::X, -Vec3::X],
                );
            }
        }
        // Shafts under the bed and on forward, a crossbar between their ends, legs under their
        // front, and the axle under them.
        let shaft = 0.5 * SHAFT;
        for x in [plan.shaft_x, -plan.shaft_x] {
            mesh.cuboid(
                Part::Timber,
                Vec3::new(x - shaft, plan.shaft_low, -hz),
                Vec3::new(x + shaft, plan.floor_low, plan.front),
                &[],
            );
            let leg = Vec3::new(0.5 * LEG, 0.0, 0.5 * LEG);
            let at = Vec3::new(x, 0.0, hz - 0.15);
            mesh.cuboid(Part::Timber, at - leg, at + leg + Vec3::Y * plan.shaft_low, &[-Vec3::Y, Vec3::Y]);
        }
        let bar = plan.front - 0.06;
        let middle = 0.5 * (plan.shaft_low + plan.floor_low);
        mesh.cuboid(
            Part::Timber,
            Vec3::new(-plan.shaft_x + shaft, middle - 0.5 * CROSSBAR, bar - 0.5 * CROSSBAR),
            Vec3::new(plan.shaft_x - shaft, middle + 0.5 * CROSSBAR, bar + 0.5 * CROSSBAR),
            &[Vec3::X, -Vec3::X],
        );
        let axle = 0.5 * AXLE;
        mesh.cuboid(
            Part::Timber,
            Vec3::new(-plan.wheel_x, plan.radius - axle, plan.axle_z - axle),
            Vec3::new(plan.wheel_x, plan.radius + axle, plan.axle_z + axle),
            &[Vec3::X, -Vec3::X],
        );
        for x in [plan.wheel_x, -plan.wheel_x] {
            let mut wheel = Mesh::default();
            self.wheel(&mut wheel, detail);
            // Turned about +Y, then +Y turned to +X.
            mesh.append(wheel, Quat::from_rotation_z(-FRAC_PI_2), Vec3::new(x, plan.radius, plan.axle_z));
        }
        for goods in &self.goods {
            let place = self.place(goods.on)?;
            goods.build(mesh, &place, detail);
        }
        Ok(())
    }

    /// A wheel about +Y through its origin: its iron tyre, the timber felloe inside it, its hub,
    /// and spokes between.
    fn wheel(&self, mesh: &mut Mesh, detail: Detail) {
        let Wheels { radius, spokes, rim } = self.wheels;
        let sides = if detail == Detail::Light { LIGHT_WHEEL } else { WHEEL_SIDES };
        let (inner, tyre, half) = (radius - rim[0], radius - TYRE, 0.5 * rim[1]);
        mesh.lathe(
            Part::Iron,
            Vec3::ZERO,
            &[(tyre, -half), (radius, -half), (radius, half), (tyre, half)],
            sides,
            [false, false],
            false,
        );
        mesh.lathe(
            Part::Timber,
            Vec3::ZERO,
            &[(tyre, half), (inner, half), (inner, -half), (tyre, -half)],
            sides,
            [false, false],
            false,
        );
        let [hub, hub_length] = HUB;
        let hub_half = 0.5 * hub_length;
        mesh.lathe(
            Part::Timber,
            Vec3::ZERO,
            &[(0.6 * hub, -hub_half), (hub, -0.5 * hub_half), (hub, 0.5 * hub_half), (0.6 * hub, hub_half)],
            if detail == Detail::Light { 6 } else { HUB_SIDES },
            [true, true],
            false,
        );
        for k in 0..spokes {
            let angle = TAU * (k as f32 + 0.5) / spokes as f32;
            let out = Vec3::new(angle.cos(), 0.0, -angle.sin());
            mesh.bar(Part::Timber, out * 0.8 * hub, out * (inner + 0.01), SPOKE, Vec3::Y, [false, false]);
        }
    }
}

/// A cart's numbers.
struct CartPlan {
    /// Half the bed's width and length.
    half: Vec2,
    /// The shafts' undersides (on the axle) and the floor's underside (on the shafts), and its top.
    shaft_low: f32,
    floor_low: f32,
    floor_top: f32,
    /// The shafts' middles and the wheels' along X, the axle's along Z, the shafts' front ends.
    shaft_x: f32,
    wheel_x: f32,
    axle_z: f32,
    front: f32,
    radius: f32,
}

impl CartPlan {
    /// What goods on the ground keep out of: the bed, its wheels and its shafts, least and
    /// greatest corners in X and Z.
    fn footprint(&self) -> (Vec2, Vec2) {
        let x = self.wheel_x + 0.5 * HUB[1];
        let back = (-self.half.y).min(self.axle_z - self.radius);
        (Vec2::new(-x, back), Vec2::new(x, self.front))
    }
}
