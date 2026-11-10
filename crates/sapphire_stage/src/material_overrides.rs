//! What the sandbox's `--hair-shadow-softness` and `--hair-transmission` change in the anime character
//! materials a stage uploads (M1 C8). A stage applies them as each material loads, sample scenes or not, so
//! they live outside the face rig, whose module re-exports them where the sandbox reads them
//! (`faces::MaterialOverrides`, with the `samples` feature).

use sapphire_assets::material::{AnimeCharacterParams, HairTransmission, Material};

/// What `--hair-shadow-softness` and `--hair-transmission` change in the anime character
/// materials the sandbox uploads (M1 C8): a face's bangs' shadow's soft edge and hair's light
/// through it, each edited into a copy of the cooked material as it loads.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MaterialOverrides {
    /// Faces' bangs' shadow's soft edge, in meters, for faces that cast one.
    pub hair_shadow_softness: Option<f32>,
    /// Hair's transmission strength, at its sharpness (the default's, where it had none).
    pub hair_transmission: Option<f32>,
}

impl MaterialOverrides {
    /// Whether they change nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Applies them to `material`, where it is a face or hair they change. Returns whether it
    /// changed.
    pub(crate) fn apply(&self, material: &mut Material) -> bool {
        let mut changed = false;
        material.edit_anime_character(|params| match params.modes[2] {
            AnimeCharacterParams::SURFACE_FACE => {
                if let Some(softness) = self.hair_shadow_softness
                    && params.hair_shadow[0] > 0.0
                {
                    params.hair_shadow[2] = softness;
                    changed = true;
                }
            }
            AnimeCharacterParams::SURFACE_HAIR => {
                if let Some(strength) = self.hair_transmission {
                    let sharpness = params.surface_color[3];
                    params.surface[3] = strength;
                    params.surface_color[3] =
                        if sharpness > 0.0 { sharpness } else { HairTransmission::default().sharpness };
                    changed = true;
                }
            }
            _ => {}
        });
        changed
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::AssetId;
    use sapphire_assets::material::{AnimeCharacterMaterial, CharacterSurface, FaceShading, HairShading};
    use sapphire_assets::texture::Swizzle;

    use super::*;

    #[test]
    fn overrides_soften_faces_and_light_hair_through_and_leave_the_rest() {
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        let cook = |surface| {
            let document = AnimeCharacterMaterial { surface, ..AnimeCharacterMaterial::default() };
            Material::from_document(&document, &swizzles).expect("valid")
        };
        let read = |material: &mut Material| {
            let mut params = None;
            material.edit_anime_character(|block| params = Some(*block));
            params.expect("an anime character material")
        };
        let overrides = MaterialOverrides { hair_shadow_softness: Some(0.02), hair_transmission: Some(0.8) };
        assert!(MaterialOverrides::default().is_empty() && !overrides.is_empty());
        let face = FaceShading::new(AssetId::from_text("79309b71-b35f-4ce6-9d53-b629493934e7"));
        let mut material = cook(CharacterSurface::Face(face.clone()));
        assert!(overrides.apply(&mut material));
        assert_eq!(read(&mut material).hair_shadow, [0.03, 1.0, 0.02, 0.0], "the default face, softened");
        // Hair transmits at the default sharpness, or its own.
        let mut hair = cook(CharacterSurface::Hair(HairShading::default()));
        assert!(overrides.apply(&mut hair));
        let params = read(&mut hair);
        assert_eq!((params.surface[3], params.surface_color[3]), (0.8, 4.0));
        let own = HairShading {
            transmission: HairTransmission { strength: 0.3, sharpness: 2.0 },
            ..HairShading::default()
        };
        let mut own = cook(CharacterSurface::Hair(own));
        assert!(overrides.apply(&mut own));
        let params = read(&mut own);
        assert_eq!((params.surface[3], params.surface_color[3]), (0.8, 2.0));
        // A face that casts no bangs' shadow, a body, and hair with no transmission asked stay as
        // they are.
        let mut shadowless = face;
        shadowless.hair_shadow.distance = 0.0;
        let mut shadowless = cook(CharacterSurface::Face(shadowless));
        let before = shadowless.clone();
        assert!(!overrides.apply(&mut shadowless));
        assert_eq!(shadowless, before);
        assert!(!overrides.apply(&mut cook(CharacterSurface::Body)));
        let softening = MaterialOverrides { hair_transmission: None, ..overrides };
        assert!(!softening.apply(&mut cook(CharacterSurface::Hair(HairShading::default()))));
    }
}
