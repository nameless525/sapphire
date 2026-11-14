//! A zone's reflection probes (M5 L2): a few hand-placed points the cook bakes a cube of the zone from, once
//! per time-of-day key of its day, which the renderer reflects (spec §5.12, "Reflections").
//!
//! A probe is where it stands and the box its reflections are corrected against (box parallax): the room it
//! stands in, the street between two rows of houses, or the open place around it out to where its ground
//! ends. The box is authored, never derived: only the author knows which walls a probe's surroundings are.
//!
//! ```ron
//! reflections: [
//!     // The open yard: out to the paving round it, and up past its roofs.
//!     (name: "yard", position: (8.0, 1.6, 6.0), parallax: (min: (-8.0, 0.0, -8.0), max: (24.0, 12.0, 24.0))),
//!     // Under a shelter open at its front: its walls, its roof and its open side are the box.
//!     (name: "shelter", position: (-24.0, 1.6, 30.0), parallax: (min: (-27.0, 0.0, 27.7), max: (-21.0, 3.5, 32.0))),
//! ],
//! ```
//!
//! The cook bakes them into one asset beside the zone's cells (`town.sreflect`, the pipeline's `reflections`
//! module), which the cooked zone names in its presentation. Nothing about them reaches the simulation.

use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::document::{check_name, check_position};

/// The most reflection probes a zone may hold: one texture array of the device floor's 256 layers holds 42
/// cubes of six faces (M5 L0), and the renderer keeps a zone's in one.
pub const MAX_PROBES: usize = 42;

/// The thinnest a probe's parallax box may be along an axis, in metres.
pub const MIN_PARALLAX_EXTENT: f32 = 0.1;

/// One reflection probe, as a zone document places it (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct ReflectionProbe {
    /// What a cook's errors and the renderer's statistics call it. Unique within the zone.
    pub name: String,
    /// Where its cube is baked from, in the zone's space.
    pub position: [f32; 3],
    /// The box its reflections are corrected against, which holds its position.
    pub parallax: ParallaxBox,
}

/// An axis-aligned box in the zone's space, in metres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct ParallaxBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl ReflectionProbe {
    /// Whether it's a probe a cook can bake, or what's wrong with it, by the field at fault.
    pub(crate) fn check(&self) -> Result<(), (&'static str, String)> {
        check_name(&self.name).map_err(|problem| ("name", problem))?;
        check_position(self.position).map_err(|problem| ("position", problem))?;
        let ParallaxBox { min, max } = self.parallax;
        check_position(min).map_err(|problem| ("parallax.min", problem))?;
        check_position(max).map_err(|problem| ("parallax.max", problem))?;
        // Both corners are finite by now.
        if (0..3).any(|axis| max[axis] - min[axis] < MIN_PARALLAX_EXTENT) {
            let problem =
                format!("{min:?} to {max:?} isn't a box at least {MIN_PARALLAX_EXTENT} m on each side");
            return Err(("parallax", problem));
        }
        if (0..3).any(|axis| !(min[axis] < self.position[axis] && self.position[axis] < max[axis])) {
            let problem = format!("{min:?} to {max:?} doesn't hold its position {:?}", self.position);
            return Err(("parallax", problem));
        }
        Ok(())
    }
}
