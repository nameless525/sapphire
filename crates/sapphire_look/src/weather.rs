//! The weather as the frame draws it (M5 L4): how wet the world's surfaces are and how far its
//! puddles have filled, resolved outside the renderer from a zone's weather, its day's keyed
//! weather and a tool's override, in that order (spec §5.21), and drawn by the environment's wet
//! surface response (spec §5.12).

/// The weather's wetness inputs: what the environment's wet surfaces read (M5 L4). Both are
/// shares, 0 to 1, and the default is a dry world, which draws exactly as the world drew before
/// weather.
///
/// - **Wetness** darkens a porous surface (paving, plaster, wood) and lays a film of water over
///   every surface, lowering its roughness toward a wet film's, so it picks up its reflection and
///   the lights' highlights.
/// - **Puddles** fill the flat, upward-facing places a material lets water stand in, which go
///   flat and mirror-like.
///
/// Where these come from is the world's to decide (the stage resolves them); the renderer reads
/// only the values (rule R4).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Weather {
    /// How wet surfaces are, 0 (dry) to 1 (soaked).
    pub wetness: f32,
    /// How far puddles have filled, 0 (none) to 1 (every place a material lets water stand).
    pub puddles: f32,
}

impl Weather {
    /// A dry world: what draws as the world drew before weather.
    pub const DRY: Self = Self { wetness: 0.0, puddles: 0.0 };

    /// Whether it draws anything: either share above 0.
    #[must_use]
    pub fn is_wet(&self) -> bool {
        self.wetness > 0.0 || self.puddles > 0.0
    }

    /// The same weather with each share held to 0 to 1, a NaN counting as dry: what the frame
    /// carries to the GPU.
    #[must_use]
    pub fn clamped(self) -> Self {
        let share = |value: f32| if value > 0.0 { value.min(1.0) } else { 0.0 };
        Self { wetness: share(self.wetness), puddles: share(self.puddles) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_is_the_default_and_draws_nothing() {
        assert_eq!(Weather::default(), Weather::DRY);
        assert!(!Weather::DRY.is_wet());
        assert!(Weather { wetness: 0.0, puddles: 0.1 }.is_wet());
        assert!(Weather { wetness: 0.1, puddles: 0.0 }.is_wet());
    }

    #[test]
    fn shares_are_held_to_their_range() {
        let wild = Weather { wetness: 1.5, puddles: -0.5 };
        assert_eq!(wild.clamped(), Weather { wetness: 1.0, puddles: 0.0 });
        assert_eq!(
            Weather { wetness: f32::NAN, puddles: 0.25 }.clamped(),
            Weather { wetness: 0.0, puddles: 0.25 }
        );
    }
}
