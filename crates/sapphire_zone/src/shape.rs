//! [`ZoneShape`] (M6 W8a): the one shape a zone places a volume of air by (m6-plan §3, "One volume
//! shape"), about a position its owner gives. Weather regions place by it now; atmosphere volumes, fog
//! volumes and wind volumes will. The world reads it as its `VolumeShape`, which measures how far a
//! point is from it.
//!
//! ```ron
//! Sphere(radius: 40.0)
//! Box(half_extents: (30.0, 20.0, 12.0), yaw: 35.0)
//! Ellipsoid(radii: (60.0, 25.0, 30.0), yaw: -10.0)
//! ```
//!
//! A yaw turns a box or an ellipsoid about +Y, in degrees, counterclockwise seen from above (+X toward
//! −Z), as a placement's `turn` does; it's 0 when left out. It is the same plain value in the document
//! and in the cooked zone.

use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::document::MIN_EXTENT;
use crate::grid::MAX_COORDINATE;

/// Where a volume is about its position (see the [module docs](self)).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
#[rkyv(derive(Debug))]
pub enum ZoneShape {
    Sphere {
        radius: f32,
    },
    Box {
        half_extents: [f32; 3],
        /// Degrees about +Y.
        #[serde(default, skip_serializing_if = "is_zero")]
        yaw: f32,
    },
    Ellipsoid {
        /// Its half-axes along its own X, Y and Z.
        radii: [f32; 3],
        /// Degrees about +Y.
        #[serde(default, skip_serializing_if = "is_zero")]
        yaw: f32,
    },
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

impl ZoneShape {
    /// Whether its extents and its yaw are ones the world can measure from, or what's wrong with them:
    /// every extent from [`MIN_EXTENT`] to [`MAX_COORDINATE`] metres, the yaw finite.
    pub fn check(&self) -> Result<(), String> {
        let good = |extent: f32| extent.is_finite() && (MIN_EXTENT..=MAX_COORDINATE).contains(&extent);
        let (extents, yaw): (&[f32], f32) = match self {
            Self::Sphere { radius } => (std::slice::from_ref(radius), 0.0),
            Self::Box { half_extents, yaw } => (half_extents, *yaw),
            Self::Ellipsoid { radii, yaw } => (radii, *yaw),
        };
        if !extents.iter().all(|&extent| good(extent)) {
            return Err(format!("extents {extents:?} aren't from {MIN_EXTENT} to {MAX_COORDINATE} m"));
        }
        if !yaw.is_finite() {
            return Err(format!("yaw {yaw} isn't finite"));
        }
        Ok(())
    }
}

impl ArchivedZoneShape {
    #[must_use]
    pub fn to_native(&self) -> ZoneShape {
        let three = |values: &[rkyv::rend::f32_le; 3]| values.each_ref().map(|value| value.to_native());
        match self {
            Self::Sphere { radius } => ZoneShape::Sphere { radius: radius.to_native() },
            Self::Box { half_extents, yaw } => {
                ZoneShape::Box { half_extents: three(half_extents), yaw: yaw.to_native() }
            }
            Self::Ellipsoid { radii, yaw } => {
                ZoneShape::Ellipsoid { radii: three(radii), yaw: yaw.to_native() }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shape_reads_its_defaults_and_refuses_what_the_world_cannot_measure() {
        let shape: ZoneShape = ron::from_str("Box(half_extents: (1.0, 2.0, 3.0))").expect("reads");
        assert_eq!(shape, ZoneShape::Box { half_extents: [1.0, 2.0, 3.0], yaw: 0.0 });
        assert_eq!(ron::to_string(&shape).expect("writes"), "Box(half_extents:(1.0,2.0,3.0))");
        let turned: ZoneShape = ron::from_str("Ellipsoid(radii: (4.0, 2.0, 3.0), yaw: 30.0)").expect("reads");
        assert_eq!(turned.check(), Ok(()));
        assert_eq!(ZoneShape::Sphere { radius: 5.0 }.check(), Ok(()));
        for bad in [
            ZoneShape::Sphere { radius: 0.0 },
            ZoneShape::Sphere { radius: f32::INFINITY },
            ZoneShape::Box { half_extents: [1.0, -1.0, 1.0], yaw: 0.0 },
            ZoneShape::Box { half_extents: [1.0; 3], yaw: f32::NAN },
            ZoneShape::Ellipsoid { radii: [1.0, 1.0, 2.0 * MAX_COORDINATE], yaw: 0.0 },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
        assert!(ron::from_str::<ZoneShape>("Sphere(radius: 1.0, yaw: 3.0)").is_err(), "a sphere has no yaw");
    }

    #[test]
    fn the_cooked_shape_is_the_documents() {
        for shape in [
            ZoneShape::Sphere { radius: 5.0 },
            ZoneShape::Box { half_extents: [1.0, 2.0, 3.0], yaw: 15.0 },
            ZoneShape::Ellipsoid { radii: [4.0, 2.0, 3.0], yaw: -40.0 },
        ] {
            let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&shape).expect("writes");
            let archived = rkyv::access::<ArchivedZoneShape, rkyv::rancor::Error>(&bytes).expect("reads");
            assert_eq!(archived.to_native(), shape);
        }
    }
}
