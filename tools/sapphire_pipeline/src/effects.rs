//! Effects libraries (M6 N1): `.effects.ron` cooked to `.seffects`, what a game's cues and sword trails play,
//! which its cue table names (`effect_library`).
//!
//! The document and its checks are `sapphire_vfx`'s (`EffectLibrary`), the cooked format `sapphire_assets`'
//! ([`sapphire_assets::effects`]). This module places them in the cook: a source with a sidecar like any asset
//! (`importer: Effects(())`) that reads nothing besides itself. A library written at any version the effects
//! crate reads (1 to 5) is checked whole and cooks as that crate writes it back at its current version, so a
//! runtime never upgrades a cooked library, and an old library and its rewrite cook to the same bytes.

use std::path::{Path, PathBuf};

use sapphire_assets::effects::{Effects, EffectsError};
use sapphire_serialize::DocumentError;
use sapphire_vfx::EffectLibrary;
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reads and checks the document. Bump the version whenever that changes output for the same document,
/// as a new schema version of `EffectLibrary` does: the cooked document is written at it.
pub const IMPORTER: Stage = Stage { name: "effects library document", version: 1 };

/// The `.seffects` writer. Bump the version whenever it changes output for the same document.
pub const COOKER: Stage = Stage { name: "seffects", version: 1 };

/// What an effects library's source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".effects.ron";

/// Import settings for effects libraries: none yet. A struct, so settings can join without changing existing
/// sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectsSettings {}

/// An effects library that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum EffectsCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] std::str::Utf8Error),
    /// The library doesn't read, or a field of it is wrong: the error names it, with its line and column.
    #[error(transparent)]
    Library(#[from] DocumentError),
    #[error(transparent)]
    Cooked(#[from] EffectsError),
}

/// Whether `path` names an effects library's source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a source at `relative` writes: `combat.effects.ron` → `combat.seffects`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Reads and checks a document's bytes, at any version the effects crate reads.
pub fn read(bytes: &[u8]) -> Result<EffectLibrary, EffectsCookError> {
    Ok(EffectLibrary::from_ron(std::str::from_utf8(bytes)?)?)
}

/// Cooks a document's bytes: the library as the effects crate writes it back, read again through the
/// runtime's check.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, EffectsCookError> {
    let cooked = Effects { library: read(bytes)?.to_ron()?, sheets: Vec::new() }.to_cooked()?;
    Effects::read(&cooked)?;
    Ok(cooked.into_vec())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sapphire_assets::effects::{EFFECTS_EXTENSION, LIBRARY_TYPE};
    use sapphire_assets::{AssetBudgets, AssetServer, Handle, MemoryPackSource};
    use sapphire_reflect::{Versioned, stable_name};
    use sapphire_serialize::cooked::AlignedVec;

    use super::*;
    use crate::cook::{Cook, CookOptions, cook_dir};
    use crate::meta::{self, ImportSettings};
    use crate::pack::{self, pack_cooked};
    use crate::test_util::TempDir;

    const SHIPPED: &[u8] = include_bytes!("../../../crates/sapphire_vfx/default_effects.ron");

    /// The libraries each older version wrote (`sapphire_vfx`'s fixtures), by version.
    const OLDER: [(u32, &[u8]); 4] = [
        (1, include_bytes!("../../../crates/sapphire_vfx/testdata/v1.effects.ron")),
        (2, include_bytes!("../../../crates/sapphire_vfx/testdata/v2.effects.ron")),
        (3, include_bytes!("../../../crates/sapphire_vfx/testdata/v3.effects.ron")),
        (4, include_bytes!("../../../crates/sapphire_vfx/testdata/v4.effects.ron")),
    ];

    /// The cooked shipped library's blake3. A library is text and an rkyv envelope, with no float arithmetic
    /// on the way, so this is one value on every architecture (CI's `Test` job holds it on Linux and Windows
    /// as well as macOS). If it fails everywhere, the shipped library, the effects schema's writer or the
    /// envelope changed on purpose: update it, and bump [`COOKER`] or [`IMPORTER`] if the same document now
    /// cooks otherwise.
    const SHIPPED_HASH: &str = "c41c668f60f9f986fac86353dac9c5e3326143e35815c3e9efa0c2c570950272";

    /// A cooked library's bytes, aligned as a pack's reader hands them over.
    fn aligned(bytes: &[u8]) -> AlignedVec {
        let mut aligned = AlignedVec::new();
        aligned.extend_from_slice(bytes);
        aligned
    }

    #[test]
    fn a_source_is_known_by_its_whole_suffix() {
        assert!(is_source(Path::new("combat/combat.effects.ron")));
        assert!(is_source(Path::new("COMBAT.EFFECTS.RON")));
        assert!(!is_source(Path::new("default_effects.ron")));
        assert!(!is_source(Path::new("default_screen_effects.ron")));
        assert_eq!(
            output_path(Path::new("combat/combat.effects.ron"), EFFECTS_EXTENSION),
            Path::new("combat/combat.seffects")
        );
    }

    /// The asset server holds the library's type by name, since it can't link the effects crate.
    #[test]
    fn the_cooked_format_names_the_librarys_type() {
        assert_eq!(LIBRARY_TYPE, stable_name::<EffectLibrary>());
    }

    /// The shipped library cooks to the same bytes twice, pinned, and reads back as the document said, every
    /// value to the bit (its write matches), at the effects crate's current version.
    #[test]
    fn the_shipped_library_cooks_the_same_bytes_every_time_and_reads_back() {
        let first = cook(SHIPPED).expect("cooks");
        assert_eq!(first, cook(SHIPPED).expect("cooks again"), "two cooks agree byte for byte");
        assert_eq!(blake3::hash(&first).to_hex().as_str(), SHIPPED_HASH, "the cooked shipped library moved");
        let bytes = aligned(&first);
        let cooked = Effects::read(&bytes).expect("reads").library();
        let header = format!("version: {},", EffectLibrary::VERSION);
        assert!(cooked.contains(&header), "written at the current version:\n{cooked}");
        let shipped = read(SHIPPED).expect("reads");
        assert_eq!(
            EffectLibrary::from_ron(cooked).expect("reads back").to_ron().unwrap(),
            shipped.to_ron().unwrap()
        );
    }

    /// A library at every older version cooks to the bytes its rewrite at the current version does: what a
    /// runtime reads doesn't depend on when the library was written.
    #[test]
    fn an_older_library_cooks_as_its_rewrite_does() {
        for (version, text) in OLDER {
            let rewritten = read(text).expect("reads").to_ron().expect("writes");
            assert!(!rewritten.contains(&format!("version: {version},")), "version {version}");
            assert_eq!(cook(text).unwrap(), cook(rewritten.as_bytes()).unwrap(), "version {version}");
        }
    }

    /// A library in a content tree gets an `Effects` sidecar, cooks into the shared pack the same bytes twice,
    /// and loads through the asset server, which never sends it to the uploader, as the library it was.
    #[test]
    fn a_library_cooks_packs_and_loads_as_an_asset() {
        let dir = TempDir::new("effects_round_trip");
        let content = dir.path().join("content");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::write(content.join("combat.effects.ron"), OLDER[3].1).unwrap();
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false, editor: false };
        let cook_into = |loose: &str| cook_dir(&content, &dir.path().join(loose), &options).unwrap();
        let report = cook_into("loose");
        let sidecar =
            meta::read(&meta::sidecar_path(&content.join("combat.effects.ron"))).expect("a sidecar");
        assert_eq!(sidecar.importer, ImportSettings::Effects(EffectsSettings::default()));
        let [cooked] = report.assets.as_slice() else { panic!("one asset: {:?}", report.assets) };
        assert_eq!(cooked.output.extension().unwrap(), EFFECTS_EXTENSION);
        let again = cook_into("loose_again");
        let bytes = |report: &crate::cook::CookReport| std::fs::read(&report.assets[0].output).unwrap();
        assert_eq!(bytes(&report), bytes(&again), "a second cook writes the same bytes");

        let packs = dir.path().join("packs");
        pack_cooked(&report, &packs, None).unwrap();
        let source = Arc::new(MemoryPackSource::new());
        source.insert(pack::SHARED, std::fs::read(packs.join("shared.spack")).unwrap());
        let mut server = AssetServer::new(source, AssetBudgets::DESKTOP);
        server.mount_now(pack::SHARED).unwrap();
        let handle: Handle<Effects> = server.load(sidecar.id).unwrap();
        server.finish(&handle, |upload| panic!("{} was sent to the uploader", upload.kind())).unwrap();
        let library = EffectLibrary::from_ron(server.get(&handle).expect("loaded").library()).expect("reads");
        assert_eq!(library.to_ron().unwrap(), read(OLDER[3].1).unwrap().to_ron().unwrap());
    }

    /// A bad field fails the cook, named, rather than the fight that would play it.
    #[test]
    fn a_bad_field_fails_the_cook_naming_it() {
        let text =
            String::from_utf8_lossy(SHIPPED).replacen("lifetime: (0.08, 0.1),", "lifetime: (0.1, 0.08),", 1);
        let error = cook(text.as_bytes()).expect_err("a backward lifetime").to_string();
        assert!(error.contains("`lifetime`") && error.contains("hit_spark"), "{error}");
    }
}
