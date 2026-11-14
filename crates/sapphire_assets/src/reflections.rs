//! A zone's reflection probes (`.sreflect`, M5 L2): local cubes the cook bakes once per time-of-day key,
//! which the renderer blends by the day's key, adds the live sky to and prefilters (spec §5.12,
//! "Reflections").
//!
//! A zone document places its probes (`sapphire_zone::reflections`), and the cook bakes them into one asset
//! beside the zone's cells, which the cooked zone names (`ZonePresentation::reflections`) and its pack holds.
//!
//! # What a cube holds
//!
//! Per probe and per key, six square faces of [`ReflectionProbes::size`] texels a side, `Rgba16Float`:
//!
//! - **RGB** is the scene as the probe sees it lit under that key, **without the sky**: linear radiance in
//!   the renderer's normalization, before exposure, 0 where a texel sees only sky. Surfaces are lit by the
//!   key's sun or moon (with shadows), its sky and the bake's bounce, and glow by their emission under the
//!   key's emission scale.
//! - **A** is how much of the texel sees the sky, 0 to 1, in quarters (the bake's four rays a texel).
//!
//! So a frame's cube under its sky is `rgb + a × sky(direction)`, the key's cubes blended by the day's
//! key as the probe grid's bounce is ([`ReflectionProbes::key_hours`]).
//!
//! Faces are in a cube map's layer order, +X, −X, +Y, −Y, +Z, −Z, each row by row from the top and each
//! row from the left, in the orientation a cube-map lookup reads (`probes.wgsl`'s `cube_texel_direction`),
//! so the six faces of a key are six layers of a cube array as they are. Only the top level is stored: the
//! renderer composes the live sky first, so it prefilters what it composed.
//!
//! # Layout
//!
//! [`ReflectionProbes::texels`] is every cube end to end: probe by probe, within a probe key by key, within a
//! key face by face, four half floats (`half`'s bits) a texel. [`ArchivedReflectionProbes::cube`] slices out
//! one probe's one key, six layers' worth, ready to upload.
//!
//! # Sizes
//!
//! The desktop cook bakes 128 a side, which Desktop-High and Desktop-Low use; the mobile cook bakes 64,
//! which Mobile-High uses, and Mobile-Low takes the next level down, 32, from it.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

use crate::probes::MAX_KEYS;

/// Conventional file extension for cooked reflection probes.
pub const REFLECTIONS_EXTENSION: &str = "sreflect";

/// The most probes an asset holds: a zone's most (`sapphire_zone::reflections::MAX_PROBES`), which one
/// texture array of the device floor's 256 layers holds.
pub const MAX_PROBES: usize = sapphire_zone::reflections::MAX_PROBES;

/// The smallest and largest sides a cube may have: the renderer's chain of levels ends at 8 texels.
pub const MIN_SIZE: u32 = 8;
pub const MAX_SIZE: u32 = 256;

/// Half floats a texel holds: red, green, blue and the sky's share.
pub const TEXEL_CHANNELS: usize = 4;

/// A zone's baked reflection probes. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::assets"]
pub struct ReflectionProbes {
    /// Texels along a face's side: a power of two from [`MIN_SIZE`] to [`MAX_SIZE`].
    pub size: u32,
    /// The hours of the time-of-day keys the cubes were baked under, ascending, each in `0..24`: the
    /// zone's day's keys.
    pub key_hours: Vec<f32>,
    /// The probes, in the zone document's order.
    pub probes: Vec<ReflectionProbe>,
    /// Every cube's texels, half floats: per probe, per key, per face, per row, per texel, red, green, blue
    /// and the sky's share.
    pub texels: Vec<u16>,
}

impl Versioned for ReflectionProbes {
    /// 1: M5 L2.
    const VERSION: u32 = 1;
}

/// One probe: where its cubes were baked from and the box its reflections are corrected against.
#[derive(Clone, Debug, Default, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct ReflectionProbe {
    /// Its name in the zone document.
    pub name: String,
    /// Where its cubes were baked from, in the zone's space.
    pub position: [f32; 3],
    /// The box its reflections are corrected against, which holds its position.
    pub parallax_min: [f32; 3],
    pub parallax_max: [f32; 3],
}

/// Cooked reflection probes that aren't valid.
#[derive(Debug, thiserror::Error)]
pub enum ReflectionProbesError {
    /// The envelope, the schema version or the archive itself.
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("{what}: {reason}")]
    Invalid { what: &'static str, reason: String },
}

impl ReflectionProbes {
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and everything the renderer relies
    /// on ([`ArchivedReflectionProbes::validate`]).
    pub fn read(bytes: &[u8]) -> Result<&ArchivedReflectionProbes, ReflectionProbesError> {
        let probes = sapphire_serialize::access::<Self>(bytes)?;
        probes.validate()?;
        Ok(probes)
    }

    /// Half floats one cube holds: six faces of `size`² texels.
    pub fn cube_len(size: u32) -> usize {
        6 * (size as usize) * (size as usize) * TEXEL_CHANNELS
    }
}

impl ArchivedReflectionProbes {
    pub fn size(&self) -> u32 {
        self.size.to_native()
    }

    /// The time-of-day keys' hours.
    pub fn key_hours(&self) -> impl ExactSizeIterator<Item = f32> + '_ {
        self.key_hours.iter().map(|hours| hours.to_native())
    }

    /// Probe `probe`'s cube under key `key`: its six faces' texels in order, as half floats' bits.
    ///
    /// # Panics
    ///
    /// If there's no such probe or key.
    pub fn cube(&self, probe: usize, key: usize) -> impl ExactSizeIterator<Item = u16> + '_ {
        assert!(probe < self.probes.len() && key < self.key_hours.len(), "probe {probe}, key {key}");
        let len = ReflectionProbes::cube_len(self.size());
        let first = (probe * self.key_hours.len() + key) * len;
        self.texels[first..first + len].iter().map(|bits| bits.to_native())
    }

    /// Checks what the renderer relies on: the size, the keys, between 1 and [`MAX_PROBES`] probes each in
    /// its box, the texels' count, and every texel finite and not negative with its sky's share at most 1.
    pub fn validate(&self) -> Result<(), ReflectionProbesError> {
        let invalid =
            |what: &'static str, reason: String| Err(ReflectionProbesError::Invalid { what, reason });
        let size = self.size();
        if !size.is_power_of_two() || !(MIN_SIZE..=MAX_SIZE).contains(&size) {
            return invalid("size", format!("{size}, a power of two from {MIN_SIZE} to {MAX_SIZE}"));
        }
        let keys = self.key_hours.len();
        if keys == 0 || keys > MAX_KEYS as usize {
            return invalid("keys", format!("{keys}, 1..={MAX_KEYS}"));
        }
        let hours: Vec<f32> = self.key_hours().collect();
        if !hours.iter().all(|h| (0.0..24.0).contains(h)) || !hours.is_sorted_by(|a, b| a < b) {
            return invalid("key hours", format!("{hours:?}"));
        }
        let count = self.probes.len();
        if count == 0 || count > MAX_PROBES {
            return invalid("probes", format!("{count}, 1..={MAX_PROBES}"));
        }
        for probe in self.probes.iter() {
            let position = probe.position.map(|v| v.to_native());
            let (min, max) =
                (probe.parallax_min.map(|v| v.to_native()), probe.parallax_max.map(|v| v.to_native()));
            let finite = position.iter().chain(&min).chain(&max).all(|v| v.is_finite());
            if !finite || (0..3).any(|axis| !(min[axis] < position[axis] && position[axis] < max[axis])) {
                return invalid(
                    "probe",
                    format!("`{}` at {position:?} in {min:?} to {max:?}", probe.name.as_str()),
                );
            }
        }
        let expected = count * keys * ReflectionProbes::cube_len(size);
        if self.texels.len() != expected {
            return invalid("texels", format!("{} half floats, expected {expected}", self.texels.len()));
        }
        let texel = |channels: &[rend::u16_le; TEXEL_CHANNELS]| {
            let [r, g, b, a] = channels.map(|channel| half::f16::from_bits(channel.to_native()));
            let color = [r, g, b].iter().all(|v| v.is_finite() && v.to_f32() >= 0.0);
            color && (0.0..=1.0).contains(&a.to_f32())
        };
        if let Some(index) =
            self.texels.as_chunks::<TEXEL_CHANNELS>().0.iter().position(|channels| !texel(channels))
        {
            return invalid(
                "texels",
                format!("texel {index} isn't finite and positive, or sees more than the sky"),
            );
        }
        Ok(())
    }
}

/// Probes of `size` with `keys` keys, every texel a value of its own: for tests here and in the server's.
#[cfg(test)]
pub(crate) fn two_probes(size: u32, keys: usize) -> ReflectionProbes {
    let probe = |name: &str, x: f32| ReflectionProbe {
        name: name.to_owned(),
        position: [x, 1.5, 0.0],
        parallax_min: [x - 4.0, 0.0, -4.0],
        parallax_max: [x + 4.0, 4.0, 4.0],
    };
    let len = 2 * keys * ReflectionProbes::cube_len(size);
    ReflectionProbes {
        size,
        key_hours: (0..keys).map(|key| key as f32 * 3.0).collect(),
        probes: vec![probe("hall", -10.0), probe("yard", 10.0)],
        texels: (0..len).map(|i| half::f16::from_f32((i % 997) as f32 / 1000.0).to_bits()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooked_probes_round_trip_and_slice_by_probe_and_key() {
        let probes = two_probes(8, 3);
        let bytes = probes.to_cooked().expect("cook");
        let read = ReflectionProbes::read(&bytes).expect("valid");
        assert_eq!(read.key_hours().collect::<Vec<_>>(), [0.0, 3.0, 6.0]);
        let len = ReflectionProbes::cube_len(8);
        assert_eq!(len, 6 * 8 * 8 * 4);
        let cube: Vec<u16> = read.cube(1, 2).collect();
        assert_eq!(cube, probes.texels[5 * len..6 * len], "the second probe's third key is the sixth cube");
        let back: ReflectionProbes =
            rkyv::deserialize::<ReflectionProbes, rkyv::rancor::Error>(read).expect("deserialize");
        assert_eq!(back, probes);
    }

    #[test]
    fn invalid_probes_are_refused() {
        let check = |edit: &dyn Fn(&mut ReflectionProbes)| {
            let mut probes = two_probes(8, 2);
            edit(&mut probes);
            ReflectionProbes::read(&probes.to_cooked().expect("cook")).map(|_| ())
        };
        assert!(check(&|_| {}).is_ok());
        assert!(check(&|p| p.size = 12).is_err(), "not a power of two");
        assert!(check(&|p| p.size = 4).is_err(), "smaller than the chain's end");
        assert!(check(&|p| p.key_hours = vec![6.0, 3.0]).is_err(), "keys out of order");
        assert!(check(&|p| p.key_hours = vec![24.0, 25.0]).is_err(), "past the day");
        assert!(
            check(&|p| {
                p.probes.clear();
                p.texels.clear();
            })
            .is_err(),
            "no probes"
        );
        assert!(check(&|p| p.probes[0].position[1] = 9.0).is_err(), "a probe outside its box");
        assert!(check(&|p| p.probes[1].parallax_max[0] = f32::NAN).is_err(), "a box that isn't finite");
        assert!(check(&|p| p.texels.truncate(p.texels.len() - 1)).is_err(), "a texel short");
        assert!(check(&|p| p.texels[1] = half::f16::INFINITY.to_bits()).is_err(), "an infinite color");
        assert!(check(&|p| p.texels[2] = half::f16::from_f32(-0.5).to_bits()).is_err(), "a negative color");
        assert!(check(&|p| p.texels[3] = half::f16::from_f32(1.25).to_bits()).is_err(), "more than the sky");
        let many = |count: usize| {
            let mut probes = two_probes(8, 1);
            probes.probes = vec![probes.probes[0].clone(); count];
            probes.texels = vec![0; count * ReflectionProbes::cube_len(8)];
            ReflectionProbes::read(&probes.to_cooked().unwrap()).map(|_| ())
        };
        assert!(many(MAX_PROBES).is_ok());
        assert!(many(MAX_PROBES + 1).is_err(), "more than an array holds");
    }
}
