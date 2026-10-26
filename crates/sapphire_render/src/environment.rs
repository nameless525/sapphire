//! The environment family's shading (M1 W4, spec §5.12 "Environment"): the settings a tier
//! draws the world with.
//!
//! The shader is `shaders/environment.wgsl`, a stylized GGX for the world, painted rather than
//! physical and kept softer and less contrasty than the anime characters drawn over it (look
//! pillar 4: soft ambient light and tinted shadows). Its header has the model: the world's key
//! light through a terminator remap whose width grows with distance, wrap lighting, time of
//! day's shadow tint, a terminator band, the ambient hemisphere, GGX specular under a clamp,
//! distance desaturation and emission, and the cast-shadow hook (`cast_shadow`) that shadows
//! (R4) fill. Its materials are cooked
//! [`EnvironmentMaterial`](sapphire_assets::material::EnvironmentMaterial)s, drawn on meshes
//! beside the anime character family's (see the crate docs, "Material families on meshes").
//!
//! # Tiers
//!
//! What phones can't afford is the specular, so each tier picks a specular model
//! ([`EnvironmentSpecular`], `tiers.ron`): the full GGX on desktop, a cheaper GGX that folds
//! visibility and Fresnel into one term on Mobile-High, and none on Mobile-Low, where the world
//! reads by its painted diffuse alone. Everything else (the terminator, wrap, shadow tint, band,
//! desaturation, emission, local lights) runs on every tier: it is a handful of ALU operations
//! that the look depends on.
//!
//! # Later milestones
//!
//! - **Reflections:** the ambient hemisphere stands in for the sky's reflection until W5's
//!   probes and the sky cube's prefiltered mips (W2's cube has one level today), with SSR on
//!   desktop and Mobile-High after them.
//! - **Wetness:** the frame carries the weather's wetness and puddle coverage
//!   ([`Weather`](crate::Weather)) to the GPU; the wet surface model (Lagarde's: darker porous
//!   albedo, lower roughness, puddles, ripples) reads them when weather arrives (M2), with a
//!   porosity input in the material.
//! - **Distance stylization** beyond desaturation: a hue drift toward the fog and sky colors
//!   joins W3's fog, and the painterly lower-mip read of distant surfaces.
//! - **Screen-space edges** for the environment, from the prepass's normals and depth: the
//!   desktop option the tiers carry as `screen_space_outlines`.
//! - **Foliage** (W6) shares the terminator and wrap with its own NPR family.

use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

/// How the environment shader reflects light, per tier (see the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum EnvironmentSpecular {
    /// No specular: metals shade as diffuse in their own color (Mobile-Low).
    Off,
    /// The GGX distribution with visibility and Fresnel folded into one term at the material's
    /// reflectance, after Zioma's mobile GGX (Mobile-High).
    Approximate,
    /// The GGX distribution, height-correlated Smith visibility and Schlick Fresnel (desktop).
    #[default]
    Full,
}

impl EnvironmentSpecular {
    /// The value of the shader's `ENVIRONMENT_SPECULAR` constant.
    pub(crate) fn constant(self) -> f64 {
        match self {
            Self::Off => 0.0,
            Self::Approximate => 1.0,
            Self::Full => 2.0,
        }
    }
}

/// The environment shader's settings for a renderer, from its quality tier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct EnvironmentSettings {
    pub specular: EnvironmentSpecular,
}

/// The name of the shader's specular constant (`environment.wgsl`).
pub(crate) const SPECULAR_CONSTANT: &str = "ENVIRONMENT_SPECULAR";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_desktop_tiers_full_specular() {
        assert_eq!(EnvironmentSettings::default().specular, EnvironmentSpecular::Full);
        let constants =
            [EnvironmentSpecular::Off, EnvironmentSpecular::Approximate, EnvironmentSpecular::Full]
                .map(EnvironmentSpecular::constant);
        assert_eq!(constants, [0.0, 1.0, 2.0]);
    }
}
