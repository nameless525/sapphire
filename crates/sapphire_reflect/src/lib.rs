//! Reflection and schema identity (spec §5.8).
//!
//! # Owns
//!
//! - The type registry: `bevy_reflect` 0.20, re-exported so engine code reaches it through
//!   this crate. A crate that *derives* `Reflect` must also list `bevy_reflect.workspace =
//!   true`, because the derive finds the crate by that name. Types are registered
//!   explicitly; `bevy_reflect`'s automatic registration (`inventory`) is off, so what a
//!   registry holds never depends on the linker.
//! - Stable type names. A type that is serialized sets its path with `#[type_path =
//!   "sapphire::<area>"]`, so moving or renaming the Rust type doesn't orphan data written
//!   under the old name. [`stable_name`] reads it, and [`stable_hash`] turns it into the
//!   64-bit ID that binary headers store.
//! - Schema versions and migrations: [`Versioned`].
//!
//! # Does not own
//!
//! - Formats and files: `sapphire_serialize` (RON documents, postcard saves, rkyv cooked
//!   data).
//! - ECS reflection (`ReflectComponent`, the inspector): with the debug overlay (M1 F5).
//!
//! # Versions and migrations
//!
//! Serde attributes absorb most changes without a new version: a new field with
//! `#[serde(default)]`, a renamed one with `#[serde(alias = "old")]`, a removed one (unknown
//! fields are ignored). A change they can't absorb bumps [`Versioned::VERSION`] and freezes
//! the old shape as its own type, which converts into the new one:
//!
//! ```
//! use sapphire_reflect::{Decoder, VersionError, Versioned, decode_versioned};
//! use serde::Deserialize;
//!
//! mod v1 {
//!     #[derive(serde::Deserialize)]
//!     pub struct Turntable { pub degrees_per_second: f32 }
//!     impl sapphire_reflect::Versioned for Turntable { const VERSION: u32 = 1; }
//! }
//!
//! #[derive(Deserialize)]
//! struct Turntable { radians_per_second: f32 }
//!
//! impl From<v1::Turntable> for Turntable {
//!     fn from(old: v1::Turntable) -> Self {
//!         Self { radians_per_second: old.degrees_per_second.to_radians() }
//!     }
//! }
//!
//! impl Versioned for Turntable {
//!     const VERSION: u32 = 2;
//!     fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
//!         decode_versioned::<v1::Turntable, D>(version, data).map(Into::into)
//!     }
//! }
//! ```
//!
//! Each version upgrades from the one before it, so data of any supported age walks the
//! chain one step at a time. Data older than the chain's first version, or newer than this
//! build, is rejected with a [`VersionError`].

pub use bevy_reflect;
pub use bevy_reflect::{
    CreateTypeData, FromReflect, GetTypeRegistration, PartialReflect, Reflect, TypePath, TypeRegistry,
};
use serde::de::DeserializeOwned;

/// A serialized type's schema version, and how data from earlier versions loads.
pub trait Versioned: Sized {
    /// The version written with new data. Starts at 1 and goes up by one whenever the
    /// serialized form changes in a way serde attributes can't absorb.
    const VERSION: u32;

    /// Loads `data`, written at `version` (older than [`Self::VERSION`]). The default supports
    /// no older versions; types with a history call [`decode_versioned`] on their previous
    /// version's type and convert.
    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        drop(data);
        Err(VersionError::TooOld { found: version, oldest: Self::VERSION })
    }
}

/// Serialized data that can be decoded as any type: a RON value, a postcard payload.
pub trait Decoder {
    type Error;

    fn decode<T: DeserializeOwned>(self) -> Result<T, Self::Error>;
}

/// Data whose schema version can't be loaded, or that failed to decode.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum VersionError<E> {
    #[error("schema version {found} is newer than this build's {current}")]
    TooNew { found: u32, current: u32 },
    #[error("schema version {found} is older than the oldest supported, {oldest}")]
    TooOld { found: u32, oldest: u32 },
    #[error(transparent)]
    Decode(E),
}

/// Decodes `data`, written at schema `version`, as `T`: directly if it is `T`'s current
/// version, otherwise through [`Versioned::upgrade`].
pub fn decode_versioned<T, D>(version: u32, data: D) -> Result<T, VersionError<D::Error>>
where
    T: Versioned + DeserializeOwned,
    D: Decoder,
{
    match version.cmp(&T::VERSION) {
        std::cmp::Ordering::Equal => data.decode().map_err(VersionError::Decode),
        std::cmp::Ordering::Greater => Err(VersionError::TooNew { found: version, current: T::VERSION }),
        std::cmp::Ordering::Less => T::upgrade(version, data),
    }
}

/// `T`'s stable name: its type path, which a serialized type sets with `#[type_path]`.
///
/// # Panics
///
/// In debug builds, if `T` didn't set its path: the derived one follows the module the type
/// lives in, so refactoring would orphan data written under it.
pub fn stable_name<T: TypePath>() -> &'static str {
    let name = T::type_path();
    debug_assert_ne!(
        name,
        std::any::type_name::<T>(),
        "`{name}` needs a #[type_path = \"sapphire::...\"] attribute to have a stable name"
    );
    name
}

/// A stable 64-bit ID for a stable name (FNV-1a), for binary headers. It never changes
/// between builds or platforms.
pub const fn stable_hash(name: &str) -> u64 {
    let bytes = name.as_bytes();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    /// Decodes RON text, as `sapphire_serialize` does for documents.
    struct Ron<'a>(&'a str);

    impl Decoder for Ron<'_> {
        type Error = ron::error::SpannedError;

        fn decode<T: DeserializeOwned>(self) -> Result<T, Self::Error> {
            ron::from_str(self.0)
        }
    }

    mod v1 {
        #[derive(Debug, serde::Deserialize)]
        pub struct Light {
            pub lux: f32,
        }

        impl super::Versioned for Light {
            const VERSION: u32 = 1;
        }
    }

    mod v2 {
        use super::*;

        #[derive(Debug, Deserialize)]
        pub struct Light {
            pub lux: f32,
            pub color: [f32; 3],
        }

        impl From<v1::Light> for Light {
            fn from(old: v1::Light) -> Self {
                Self { lux: old.lux, color: [1.0; 3] }
            }
        }

        impl Versioned for Light {
            const VERSION: u32 = 2;

            fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
                decode_versioned::<v1::Light, D>(version, data).map(Into::into)
            }
        }
    }

    /// The current version: intensity moved from lux to a 0–1 scale.
    #[derive(Debug, PartialEq, Deserialize)]
    struct Light {
        intensity: f32,
        color: [f32; 3],
    }

    impl From<v2::Light> for Light {
        fn from(old: v2::Light) -> Self {
            Self { intensity: old.lux / 100_000.0, color: old.color }
        }
    }

    impl Versioned for Light {
        const VERSION: u32 = 3;

        fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
            decode_versioned::<v2::Light, D>(version, data).map(Into::into)
        }
    }

    #[test]
    fn every_supported_version_upgrades_to_the_current_one() {
        let current = Light { intensity: 0.5, color: [1.0, 0.5, 0.25] };
        let load = |version, text| decode_versioned::<Light, _>(version, Ron(text));
        assert_eq!(load(3, "(intensity: 0.5, color: (1.0, 0.5, 0.25))"), Ok(current));
        assert_eq!(
            load(2, "(lux: 50000.0, color: (1.0, 0.5, 0.25))").map(|light| light.color),
            Ok([1.0, 0.5, 0.25])
        );
        assert_eq!(load(1, "(lux: 50000.0)"), Ok(Light { intensity: 0.5, color: [1.0; 3] }));
    }

    #[test]
    fn versions_outside_the_chain_are_rejected() {
        let load = |version| decode_versioned::<Light, _>(version, Ron("()"));
        assert_eq!(load(4), Err(VersionError::TooNew { found: 4, current: 3 }));
        assert_eq!(load(0), Err(VersionError::TooOld { found: 0, oldest: 1 }));
        assert!(matches!(load(2), Err(VersionError::Decode(_))), "version 2 data must have `lux`");
    }

    #[derive(TypePath)]
    #[type_path = "sapphire::test"]
    struct Stable;

    #[derive(TypePath)]
    struct Unstable;

    #[test]
    fn stable_names_come_from_the_type_path_attribute() {
        assert_eq!(stable_name::<Stable>(), "sapphire::test::Stable");
        assert_eq!(stable_hash("sapphire::test::Stable"), stable_hash(stable_name::<Stable>()));
        // FNV-1a's published test vectors.
        assert_eq!(stable_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(stable_hash("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "needs a #[type_path")]
    fn a_derived_type_path_is_not_stable() {
        stable_name::<Unstable>();
    }
}
