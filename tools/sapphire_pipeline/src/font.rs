//! Cooking fonts (M3 U3, spec §5.17, §5.27): a font file to a `.sfont` for each pack that shows it,
//! and `.fontchains.ron` documents to `.sfontc`. The whole font stays byte for byte in the content
//! tree, and what ships is a cook's product.
//!
//! # A font's sidecar
//!
//! A `.ttf` or `.otf` is found by its extension, as any source is, and no list in code names fonts.
//! Its sidecar says what ships:
//!
//! ```text
//! importer: Font((
//!     license: "OFL.txt",
//!     baseline_shift: 0.146,
//!     subsets: [(pack: "fonts", characters: "../../fonts/ui.charset.ron")],
//! ))
//! ```
//!
//! `license` is a file beside the font. It's carried in the cooked font and repeated in the notices
//! beside the packs ([`crate::pack`]), since a font's license can require a build to ship it; a font
//! with none doesn't cook. Each of `subsets` is one cooked font for one pack: the first is the
//! source's own asset, the rest sub-assets. A new font's sidecar has no subsets, and its cook asks
//! for them. [`pack::SHARED`], [`pack::AUDIO`], [`pack::LOC`] and [`pack::UI`] take no fonts, so a font never moves
//! their bytes.
//!
//! # What a subset holds
//!
//! What its pack can show, as a [`CharacterSet`] document: ranges for what's shown whole, and a list
//! for the rest. It isn't a source: the fonts that name it read it, and a change to it cooks them
//! again.
//!
//! ```text
//! (
//!     type: "sapphire::pipeline::CharacterSet",
//!     version: 1,
//!     data: (
//!         ranges: [(0x20, 0x7e), (0x3040, 0x30ff)],
//!         characters: "設定音量",
//!     ),
//! )
//! ```
//!
//! A font's subset for [`pack::FONTS`] adds what the tree's screens show in its family
//! ([`crate::ui`], which is also what refuses a string they can't). A character a font doesn't have is
//! simply not in its subset: Oxanium has no kana, and a locale's list goes on to a font that does.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use read_fonts_for_skera::tables::name::Name;
use read_fonts_for_skera::types::NameId;
use read_fonts_for_skera::{FontRef, TableProvider as _};
use sapphire_assets::font::{Font, FontChains, FontError};
use sapphire_reflect::Versioned;
use sapphire_strings::pseudo_characters;
use serde::{Deserialize, Serialize};

use crate::cache::{Dependency, Stage};
use crate::font_subset::{self, FontSubsetError};
use crate::pack;
use crate::ui::ScreenCharacters;

/// The font importer as cache keys know it. Its name carries the subsetter's version, which a test
/// keeps in step with `Cargo.lock`: a new `skera` can cut differently.
pub const IMPORTER: Stage = Stage { name: "font (skera 0.9.0)", version: 1 };
/// The font cooker. Bump it whenever it changes output for the same file.
pub const COOKER: Stage = Stage { name: "sfont", version: 1 };
/// The font chains importer as cache keys know it.
pub const CHAINS_IMPORTER: Stage = Stage { name: "font chains document", version: 1 };
/// The font chains cooker. Bump it whenever it changes output for the same document.
pub const CHAINS_COOKER: Stage = Stage { name: "sfontc", version: 1 };

/// What a font's key calls what the screens need of it: no file, so no file can be called this.
const SCREENS_DEPENDENCY: &str = "#screens";

/// What a font chains document's file name ends with.
const CHAINS_SOURCE_SUFFIX: &str = ".fontchains.ron";

/// Import settings for a font. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontSettings {
    /// The license file, relative to the font.
    pub license: String,
    /// How far down the face's baselines move, in ems.
    pub baseline_shift: f32,
    /// What's cooked: a font for each pack.
    pub subsets: Vec<FontSubset>,
}

/// One cooked font: which pack it goes in, and what it holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontSubset {
    pub pack: String,
    /// A [`CharacterSet`] document, relative to the font.
    pub characters: String,
}

/// Import settings for font chains, none yet: an empty struct rather than `()`, so settings can join
/// without changing existing sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontChainsSettings {}

/// A font or font chains document that won't cook.
#[derive(Debug, thiserror::Error)]
pub enum FontCookError {
    #[error("its sidecar names no subsets, so nothing would ship: add `subsets` to its Font settings")]
    NoSubsets,
    #[error("its sidecar names no license: add `license`, a file beside the font")]
    NoLicense,
    #[error("the pack `{pack}`: {problem}")]
    Pack { pack: String, problem: &'static str },
    #[error("can't read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the character set {}: {problem}", path.display())]
    Characters { path: PathBuf, problem: String },
    #[error(transparent)]
    Subset(#[from] FontSubsetError),
    #[error("the font names no family")]
    NoFamily,
    #[error("the font has none of the characters of its subset for the pack `{pack}`")]
    EmptySubset { pack: String },
    #[error(transparent)]
    Font(#[from] FontError),
    #[error(transparent)]
    Cooked(#[from] sapphire_serialize::cooked::CookedError),
    #[error("the document isn't UTF-8")]
    Utf8(#[from] std::str::Utf8Error),
}

/// Whether `path` names a font chains document.
pub fn is_chains_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(CHAINS_SOURCE_SUFFIX))
}

/// The cooked file a chains document at `relative` writes: `ui.fontchains.ron` → `ui.sfontc`.
pub fn chains_output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(CHAINS_SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// A character set document, as written. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TypePath)]
#[serde(default, deny_unknown_fields)]
#[type_path = "sapphire::pipeline"]
pub struct CharacterSet {
    /// Inclusive ranges of code points, for what's shown whole.
    pub ranges: Vec<(u32, u32)>,
    /// Single characters, for the rest. Line breaks in it are for the eye.
    pub characters: String,
}

impl Versioned for CharacterSet {
    const VERSION: u32 = 1;
}

/// The characters a pack can show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Characters(BTreeSet<char>);

impl Characters {
    /// Reads and checks a [`CharacterSet`] document, or says what's wrong with it.
    pub fn from_document(text: &str) -> Result<Self, String> {
        let document =
            sapphire_serialize::from_ron::<CharacterSet>(text).map_err(|error| error.to_string())?;
        let mut set = Self::default();
        for &(first, last) in &document.ranges {
            if first > last || last > u32::from(char::MAX) {
                return Err(format!("the range 0x{first:x} to 0x{last:x} isn't one of code points"));
            }
            set.0.extend((first..=last).filter_map(char::from_u32).filter(|c| !c.is_control()));
        }
        set.add_text(&document.characters);
        if set.0.is_empty() {
            return Err("it holds no characters".to_owned());
        }
        Ok(set)
    }

    fn add_text(&mut self, text: &str) {
        self.0.extend(text.chars().filter(|c| !c.is_control()));
    }

    /// Adds what pseudo-localization adds to English.
    pub fn add_pseudo(&mut self) {
        self.0.extend(pseudo_characters());
    }

    pub fn contains(&self, character: char) -> bool {
        self.0.contains(&character)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every character, in code point order.
    pub fn to_text(&self) -> String {
        self.0.iter().collect()
    }
}

/// The family a font says it's in, in English: the typographic family if it names one, as a variable
/// font's does, and its plain family if not.
pub fn family_name(font: &[u8]) -> Result<String, FontCookError> {
    let font = open(font)?;
    let name = font.name().map_err(|_| FontCookError::NoFamily)?;
    [NameId::TYPOGRAPHIC_FAMILY_NAME, NameId::FAMILY_NAME]
        .into_iter()
        .find_map(|id| english_name(&name, id))
        .ok_or(FontCookError::NoFamily)
}

/// The name `id` as Windows' US English records it, or the Mac's English, which is language 0.
fn english_name(name: &Name<'_>, id: NameId) -> Option<String> {
    let found = name.name_record().iter().find(|record| {
        record.name_id() == id && matches!((record.platform_id(), record.language_id()), (3, 0x0409) | (1, 0))
    })?;
    let text = found.string(name.string_data()).ok()?.to_string();
    (!text.is_empty()).then_some(text)
}

pub(crate) fn open(font: &[u8]) -> Result<FontRef<'_>, FontSubsetError> {
    FontRef::new(font).map_err(|error| FontSubsetError::NotAFont(error.to_string()))
}

/// Whether `font` has a glyph for `character`.
pub(crate) fn covers(font: &FontRef<'_>, character: char) -> bool {
    let glyph = font.cmap().ok().and_then(|cmap| cmap.map_codepoint(character));
    glyph.is_some_and(|glyph| glyph.to_u32() != 0)
}

/// Checks a font's sidecar: a license, and something to cook, each for a pack that may hold it.
pub fn check_settings(settings: &FontSettings) -> Result<(), FontCookError> {
    if settings.subsets.is_empty() {
        return Err(FontCookError::NoSubsets);
    }
    if settings.license.is_empty() {
        return Err(FontCookError::NoLicense);
    }
    for (index, subset) in settings.subsets.iter().enumerate() {
        let problem = |problem| Err(FontCookError::Pack { pack: subset.pack.clone(), problem });
        let plain =
            |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_' || *byte == b'-';
        if subset.pack.is_empty() || !subset.pack.as_bytes().iter().all(plain) {
            return problem("a pack's name is lower-case letters, digits, - and _");
        }
        if [pack::SHARED, pack::AUDIO, pack::LOC, pack::UI].contains(&subset.pack.as_str()) {
            return problem("fonts stay out of it, so a font's change never moves its bytes");
        }
        if settings.subsets[..index].iter().any(|earlier| earlier.pack == subset.pack) {
            return problem("two subsets go in it");
        }
    }
    Ok(())
}

/// The files the cook of `subset` reads besides the font: its license and its character set.
pub fn dependencies(
    path: &Path,
    bytes: &[u8],
    settings: &FontSettings,
    subset: usize,
    screens: &ScreenCharacters,
) -> Result<Vec<Dependency>, FontCookError> {
    let dir = path.parent().unwrap_or(Path::new(""));
    let entry = settings.subsets.get(subset).ok_or(FontCookError::NoSubsets)?;
    let mut dependencies = [settings.license.as_str(), entry.characters.as_str()]
        .into_iter()
        .map(|file| {
            let full = dir.join(file);
            let bytes = std::fs::read(&full).map_err(|source| FontCookError::Read { path: full, source })?;
            Ok(Dependency::File { path: file.to_owned(), hash: blake3::hash(&bytes) })
        })
        .collect::<Result<Vec<_>, FontCookError>>()?;
    if let Some(shown) = shown_by_screens(entry, bytes, screens)? {
        let text: String = shown.iter().collect();
        dependencies.push(Dependency::File { path: SCREENS_DEPENDENCY.to_owned(), hash: blake3::hash(text.as_bytes()) });
    }
    Ok(dependencies)
}

/// What the tree's screens show in this font, which only a subset for [`pack::FONTS`] holds: a zone's
/// own subset adds to what a character set says it needs.
fn shown_by_screens<'a>(
    entry: &FontSubset,
    bytes: &[u8],
    screens: &'a ScreenCharacters,
) -> Result<Option<&'a BTreeSet<char>>, FontCookError> {
    if entry.pack != pack::FONTS {
        return Ok(None);
    }
    Ok(screens.of(&family_name(bytes)?))
}

/// The bytes of a font's cooked file for `subset`, read back through the runtime's check.
pub fn cook(
    path: &Path,
    bytes: &[u8],
    settings: &FontSettings,
    subset: usize,
    screens: &ScreenCharacters,
) -> Result<Vec<u8>, FontCookError> {
    check_settings(settings)?;
    let dir = path.parent().unwrap_or(Path::new(""));
    let entry = settings.subsets.get(subset).ok_or(FontCookError::NoSubsets)?;
    let read = |file: &str| {
        let full = dir.join(file);
        std::fs::read_to_string(&full).map_err(|source| FontCookError::Read { path: full, source })
    };
    let license = read(&settings.license)?;
    let mut characters = Characters::from_document(&read(&entry.characters)?)
        .map_err(|problem| FontCookError::Characters { path: dir.join(&entry.characters), problem })?;
    if let Some(shown) = shown_by_screens(entry, bytes, screens)? {
        characters.0.extend(shown.iter().filter(|character| !character.is_control()));
    }
    let font = open(bytes)?;
    if !characters.0.iter().any(|&character| covers(&font, character)) {
        return Err(FontCookError::EmptySubset { pack: entry.pack.clone() });
    }
    let cooked = Font {
        family: family_name(bytes)?,
        pack: entry.pack.clone(),
        baseline_shift: settings.baseline_shift,
        license,
        file: font_subset::subset(bytes, &characters.to_text())?,
    }
    .to_cooked()?;
    Font::read(&cooked)?;
    Ok(cooked.into_vec())
}

/// Reads and checks a font chains document.
pub fn read_chains(bytes: &[u8]) -> Result<FontChains, FontCookError> {
    Ok(FontChains::from_document(std::str::from_utf8(bytes)?)?)
}

/// The bytes of a font chains cooked file, read back through the runtime's check.
pub fn cook_chains(bytes: &[u8]) -> Result<Vec<u8>, FontCookError> {
    let cooked = read_chains(bytes)?.to_cooked()?;
    FontChains::read(&cooked)?;
    Ok(cooked.into_vec())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::sync::Arc;

    use sapphire_assets::font::{FONT_CHAINS_EXTENSION, FONT_EXTENSION, FONTS_PACK};
    use sapphire_assets::{AssetBudgets, AssetId, AssetServer, Handle, MemoryPackSource};

    use super::*;
    use crate::cook::{self, Cook, CookError, CookOptions, Recipe, cook_dir};
    use crate::meta::{self, AssetMeta, ImportSettings};
    use crate::pack::{NOTICES, pack_cooked};
    use crate::test_util::{TempDir, read_aligned};

    fn third_party(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/third_party").join(relative)
    }

    const OXANIUM: &str = "oxanium/Oxanium[wght].ttf";
    const MPLUS1: &str = "mplus1/MPLUS1[wght].ttf";

    const CHARACTER_SET: &str = "(\n    type: \"sapphire::pipeline::CharacterSet\",\n    version: 1,\n    data: (\n        ranges: [(0x41, 0x5a)],\n        characters: \"abc\",\n    ),\n)\n";

    /// A content tree with Oxanium, which is small, under two names, and a license and a character set
    /// they share.
    struct Tree {
        dir: TempDir,
    }

    impl Tree {
        fn new(name: &str) -> Self {
            let dir = TempDir::new(name);
            let content = dir.path().join("content");
            fs::create_dir_all(content.join("type")).unwrap();
            let oxanium = fs::read(third_party(OXANIUM)).unwrap();
            fs::write(content.join("type/one.ttf"), &oxanium).unwrap();
            fs::write(content.join("type/two.ttf"), &oxanium).unwrap();
            fs::write(content.join("type/OFL.txt"), "the license\n").unwrap();
            fs::write(content.join("ui.charset.ron"), CHARACTER_SET).unwrap();
            let tree = Self { dir };
            tree.sidecar("type/one.ttf", &[("fonts", "../ui.charset.ron")], 0.25);
            tree.sidecar("type/two.ttf", &[("fonts", "../ui.charset.ron")], 0.0);
            tree
        }

        fn content(&self) -> PathBuf {
            self.dir.path().join("content")
        }

        fn sidecar(&self, source: &str, subsets: &[(&str, &str)], baseline_shift: f32) {
            let subsets = subsets
                .iter()
                .map(|&(pack, characters)| FontSubset {
                    pack: pack.to_owned(),
                    characters: characters.to_owned(),
                })
                .collect();
            let settings = FontSettings { license: "OFL.txt".to_owned(), baseline_shift, subsets };
            let path = meta::sidecar_path(&self.content().join(source));
            let id = meta::read(&path).map_or_else(|_| meta::new_id().unwrap(), |meta| meta.id);
            meta::rewrite(&path, &AssetMeta::new(id, ImportSettings::Font(settings))).unwrap();
        }

        fn cook_as(&self, cook: Cook) -> Result<cook::CookReport, CookError> {
            let options = CookOptions { cook, cache: None, locked: false };
            cook_dir(&self.content(), &self.dir.path().join("loose"), &options)
        }

        fn cook(&self) -> Result<cook::CookReport, CookError> {
            self.cook_as(Cook::Desktop)
        }

        fn id_of(&self, source: &str) -> AssetId {
            meta::read(&meta::sidecar_path(&self.content().join(source))).expect("a sidecar").id
        }
    }

    /// An error and what caused it, as a cook prints them.
    fn chain_of(error: &dyn std::error::Error) -> String {
        let mut text = error.to_string();
        let mut source = error.source();
        while let Some(next) = source {
            text.push_str(": ");
            text.push_str(&next.to_string());
            source = next.source();
        }
        text
    }

    #[test]
    fn extensions_pick_the_importers() {
        for source in ["a/b.ttf", "a/B.TTF", "a/b.otf"] {
            assert_eq!(
                ImportSettings::for_source(Path::new(source)),
                Some(ImportSettings::Font(FontSettings::default())),
                "{source}",
            );
        }
        assert_eq!(
            ImportSettings::for_source(Path::new("a/ui.fontchains.ron")),
            Some(ImportSettings::FontChains(FontChainsSettings::default()))
        );
        assert_eq!(ImportSettings::for_source(Path::new("a/ui.charset.ron")), None, "read through a font");
        let recipe = Recipe::for_settings(&ImportSettings::Font(FontSettings::default()));
        assert_eq!(recipe.output_path(Path::new("a/b[wght].ttf")), Path::new("a/b[wght].sfont"));
        let recipe = Recipe::for_settings(&ImportSettings::FontChains(FontChainsSettings::default()));
        assert_eq!(recipe.output_path(Path::new("a/ui.fontchains.ron")), Path::new("a/ui.sfontc"));
    }

    #[test]
    fn a_font_and_its_chains_cook_pack_and_load_as_assets() {
        let tree = Tree::new("font_round_trip");
        let (one, two) = (tree.id_of("type/one.ttf"), tree.id_of("type/two.ttf"));
        fs::write(
            tree.content().join("ui.fontchains.ron"),
            format!(
                "(type: \"sapphire::assets::FontChains\", version: 2, data: (chains: [(name: \"display\", fonts: [\"{one}\", \"{two}\"])]))"
            ),
        )
        .unwrap();
        let report = tree.cook().unwrap();
        assert_eq!(report.assets.len(), 3);
        let chains_id = tree.id_of("ui.fontchains.ron");
        let output = |id| report.assets.iter().find(|asset| asset.id == id).unwrap().output.clone();
        assert_eq!(output(one).extension().unwrap(), FONT_EXTENSION);
        assert_eq!(output(chains_id).extension().unwrap(), FONT_CHAINS_EXTENSION);

        let bytes = read_aligned(&output(one));
        let font = Font::read(&bytes).unwrap();
        assert_eq!((font.family.as_str(), font.pack.as_str()), ("Oxanium", FONTS_PACK));
        assert_eq!(font.baseline_shift.to_native(), 0.25);
        assert_eq!(font.license, "the license\n");
        let whole = fs::metadata(third_party(OXANIUM)).unwrap().len();
        assert!((font.file.len() as u64) < whole, "a subset: {} of {whole}", font.file.len());

        // In a pack of their own, and it's all there is, with the license beside it.
        let packs = tree.dir.path().join("packs");
        let packed = pack_cooked(&report, &packs, None).unwrap();
        let names: BTreeSet<&str> = packed.packs.iter().map(|pack| pack.name.as_str()).collect();
        assert_eq!(names, BTreeSet::from([FONTS_PACK]));
        assert!(!packs.join("shared.spack").exists());
        assert_eq!(packed.notices.as_deref(), Some(packs.join(NOTICES).as_path()));
        let notices = fs::read_to_string(packs.join(NOTICES)).unwrap();
        assert!(notices.contains("Oxanium") && notices.contains("the license"), "{notices}");
        assert_eq!(notices.matches("the license\n").count(), 1, "a family once: {notices}");

        // Asking for the chains loads the fonts, and nothing reaches the uploader.
        let source = Arc::new(MemoryPackSource::new());
        source.insert(FONTS_PACK, fs::read(packs.join("fonts.spack")).unwrap());
        let mut server = AssetServer::new(source, AssetBudgets::DESKTOP);
        server.mount_now(FONTS_PACK).unwrap();
        let chains: Handle<FontChains> = server.load(chains_id).unwrap();
        server.finish(&chains, |upload| panic!("{} was sent to the uploader", upload.kind())).unwrap();
        let read = server.get(&chains).expect("the chains loaded");
        assert_eq!(read.fonts("display"), Some([one, two].as_slice()));
        let font: Handle<Font> = server.load(two).unwrap();
        assert_eq!(server.get(&font).expect("loaded with its chains").family, "Oxanium");
    }

    #[test]
    fn notices_go_with_the_last_font_and_never_change_for_nothing() {
        let tree = Tree::new("font_notices");
        let packs = tree.dir.path().join("packs");
        let report = tree.cook().unwrap();
        pack_cooked(&report, &packs, None).unwrap();
        let notices = packs.join(NOTICES);
        let first = fs::read(&notices).unwrap();
        pack_cooked(&report, &packs, None).unwrap();
        assert_eq!(fs::read(&notices).unwrap(), first, "the same fonts, the same bytes");

        for source in ["type/one.ttf", "type/two.ttf"] {
            fs::remove_file(tree.content().join(source)).unwrap();
            fs::remove_file(meta::sidecar_path(&tree.content().join(source))).unwrap();
        }
        let report = tree.cook().unwrap();
        assert!(report.assets.is_empty());
        let packed = pack_cooked(&report, &packs, None).unwrap();
        assert_eq!(packed.notices, None);
        assert!(!notices.exists(), "no fonts, so nothing to give notice of");
        assert!(packed.removed.contains(&notices));
    }

    #[test]
    fn both_cooks_make_the_same_font() {
        let tree = Tree::new("font_both_cooks");
        let one = tree.cook_as(Cook::Desktop).unwrap();
        let other = tree.cook_as(Cook::Mobile).unwrap();
        let hashes = |report: &cook::CookReport| {
            report.assets.iter().map(|asset| (asset.id, asset.output_hash)).collect::<Vec<_>>()
        };
        assert_eq!(hashes(&one), hashes(&other));
    }

    #[test]
    fn a_second_subset_goes_in_its_own_pack_with_what_it_alone_can_show() {
        let tree = Tree::new("font_two_packs");
        fs::write(tree.content().join("zone.charset.ron"), CHARACTER_SET.replace("abc", "xyz")).unwrap();
        let subsets = [("fonts", "../ui.charset.ron"), ("zone-a", "../zone.charset.ron")];
        tree.sidecar("type/two.ttf", &subsets, 0.0);
        let report = tree.cook().unwrap();
        assert_eq!(report.assets.len(), 3);
        let id = tree.id_of("type/two.ttf");
        let derived = meta::sub_asset_id(id, "pack/zone-a");
        let asset = report.assets.iter().find(|asset| asset.id == derived).unwrap();
        assert!(asset.output.ends_with("type/two.zone-a.sfont"), "{}", asset.output.display());
        let packs = tree.dir.path().join("packs");
        let packed = pack_cooked(&report, &packs, None).unwrap();
        let names: BTreeSet<&str> = packed.packs.iter().map(|pack| pack.name.as_str()).collect();
        assert_eq!(names, BTreeSet::from([FONTS_PACK, "zone-a"]));
        let output = |id| report.assets.iter().find(|asset| asset.id == id).unwrap().output.clone();
        let (own, zone) = (read_aligned(&output(id)), read_aligned(&output(derived)));
        let glyph = |file: &[u8], c| {
            let font = FontRef::new(file).unwrap();
            covers(&font, c)
        };
        let (own, zone) = (Font::read(&own).unwrap(), Font::read(&zone).unwrap());
        assert!(glyph(&own.file, 'a') && !glyph(&own.file, 'x'));
        assert!(glyph(&zone.file, 'x') && !glyph(&zone.file, 'a'));
    }

    #[test]
    fn a_change_to_what_a_font_reads_cooks_it_again() {
        let tree = Tree::new("font_inputs");
        let cache = tree.dir.path().join("cache");
        let id = tree.id_of("type/one.ttf");
        let cook = || {
            let options = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked: false };
            let out = tree.dir.path().join("loose");
            let report = cook_dir(&tree.content(), &out, &options).unwrap();
            report.assets.iter().find(|asset| asset.id == id).unwrap().output_hash
        };
        let first = cook();
        assert_eq!(cook(), first);
        fs::write(tree.content().join("type/OFL.txt"), "another license\n").unwrap();
        let licensed = cook();
        assert_ne!(licensed, first);
        fs::write(tree.content().join("ui.charset.ron"), CHARACTER_SET.replace("abc", "xyz")).unwrap();
        assert_ne!(cook(), licensed);
    }

    #[test]
    fn a_new_font_with_nothing_said_asks_for_what_it_needs() {
        let tree = Tree::new("font_new");
        fs::remove_file(meta::sidecar_path(&tree.content().join("type/one.ttf"))).unwrap();
        let error = chain_of(&tree.cook().unwrap_err());
        assert!(error.contains("names no subsets") && error.contains("one.ttf"), "{error}");
    }

    #[test]
    fn a_font_failing_to_cook_says_which_and_why() {
        let tree = Tree::new("font_failures");
        fs::remove_file(tree.content().join("type/OFL.txt")).unwrap();
        let error = chain_of(&tree.cook().unwrap_err());
        assert!(error.contains("one.ttf") && error.contains("OFL.txt"), "{error}");

        let tree = Tree::new("font_broken");
        fs::write(tree.content().join("type/one.ttf"), b"not a font").unwrap();
        let error = chain_of(&tree.cook().unwrap_err());
        assert!(error.contains("one.ttf") && error.contains("not a font"), "{error}");

        // None of the characters it was asked for.
        let tree = Tree::new("font_none_of_them");
        let charset =
            "(type: \"sapphire::pipeline::CharacterSet\", version: 1, data: (characters: \"設定\"))";
        fs::write(tree.content().join("ui.charset.ron"), charset).unwrap();
        let error = chain_of(&tree.cook().unwrap_err());
        assert!(error.contains("none of the characters") && error.contains("fonts"), "{error}");
    }

    #[test]
    fn chains_naming_what_is_not_a_font_fail_the_cook_by_name() {
        let tree = Tree::new("font_not_a_font");
        let missing: AssetId = "9f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c".parse().unwrap();
        let document = |id: AssetId| {
            format!(
                "(type: \"sapphire::assets::FontChains\", version: 2, data: (chains: [(name: \"body\", fonts: [\"{id}\"])]))"
            )
        };
        fs::write(tree.content().join("ui.fontchains.ron"), document(missing)).unwrap();
        let error = tree.cook().unwrap_err();
        assert!(matches!(&error, CookError::NotAFont { id, .. } if *id == missing), "{error:?}");
        assert!(error.to_string().contains("ui.fontchains.ron"), "{error}");
        // Chains naming chains.
        let own = tree.id_of("ui.fontchains.ron");
        fs::write(tree.content().join("ui.fontchains.ron"), document(own)).unwrap();
        assert!(matches!(tree.cook(), Err(CookError::NotAFont { .. })));
    }

    #[test]
    fn a_bad_sidecar_says_what_is_wrong_with_it() {
        let subset = |pack: &str| FontSubset { pack: pack.to_owned(), characters: "c.ron".to_owned() };
        let settings = |subsets| FontSettings { license: "OFL.txt".to_owned(), baseline_shift: 0.0, subsets };
        let refused = |settings: FontSettings| check_settings(&settings).unwrap_err().to_string();
        assert!(refused(settings(Vec::new())).contains("no subsets"));
        let no_license = FontSettings { license: String::new(), ..settings(vec![subset("fonts")]) };
        assert!(refused(no_license).contains("no license"));
        for pack in [pack::SHARED, pack::AUDIO, pack::LOC] {
            assert!(refused(settings(vec![subset(pack)])).contains("fonts stay out"));
        }
        assert!(refused(settings(vec![subset("audio")])).contains("fonts stay out"));
        assert!(refused(settings(vec![subset("Fonts")])).contains("lower-case"));
        assert!(refused(settings(vec![subset("")])).contains("lower-case"));
        assert!(refused(settings(vec![subset("fonts"), subset("fonts")])).contains("two subsets"));
        assert!(check_settings(&settings(vec![subset("fonts"), subset("zone-1")])).is_ok());
    }

    #[test]
    fn a_character_set_says_what_is_wrong_with_it() {
        let document =
            |data: &str| format!("(type: \"sapphire::pipeline::CharacterSet\", version: 1, data: ({data}))");
        let set =
            Characters::from_document(&document("ranges: [(0x41, 0x43)], characters: \"かB\"")).unwrap();
        assert_eq!(set.to_text(), "ABCか");
        assert!(set.contains('か') && !set.contains('D'));
        let error = |data: &str| Characters::from_document(&document(data)).unwrap_err();
        assert!(error("").contains("no characters"));
        assert!(error("ranges: [(0x43, 0x41)]").contains("isn't one of code points"));
        assert!(error("ranges: [(0, 0x110000)]").contains("isn't one of code points"));
        assert!(error("rangs: []").contains("rangs"));
        // Surrogates aren't characters, so a range over them holds what's around them.
        let around = Characters::from_document(&document("ranges: [(0xd7ff, 0xe000)]")).unwrap();
        assert_eq!(around.len(), 2);
    }

    #[test]
    fn m_plus_1_and_the_shipped_set_show_pseudo_localization() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/fonts/ui.charset.ron");
        let shipped = Characters::from_document(&fs::read_to_string(path).unwrap()).unwrap();
        let mut pseudo = Characters::default();
        pseudo.add_pseudo();
        assert!(pseudo.len() > 50);
        let font = fs::read(third_party(MPLUS1)).unwrap();
        let font = FontRef::new(&font).unwrap();
        let text = pseudo.to_text();
        let unshown: String = text.chars().filter(|c| !covers(&font, *c)).collect();
        assert_eq!(unshown, "", "accents M PLUS 1 doesn't have");
        let unshipped: String = text.chars().filter(|c| !shipped.contains(*c)).collect();
        assert_eq!(unshipped, "", "accents the interface's subsets leave out");
    }

    #[test]
    fn the_shipped_character_set_is_all_in_the_body_font() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/fonts/ui.charset.ron");
        let text = fs::read_to_string(path).unwrap();
        assert!(Characters::from_document(&text).unwrap().len() > 500);
        let font = fs::read(third_party(MPLUS1)).unwrap();
        let font = FontRef::new(&font).unwrap();
        // Ranges take in whatever a font lacks in them. The list is what someone chose.
        let document = sapphire_serialize::from_ron::<CharacterSet>(&text).unwrap();
        let missing: String =
            document.characters.chars().filter(|c| !c.is_control() && !covers(&font, *c)).collect();
        assert_eq!(missing, "", "chosen characters M PLUS 1 doesn't have");
    }

    #[test]
    fn the_family_is_what_the_font_says() {
        for (file, family) in [
            (MPLUS1, "M PLUS 1"),
            (OXANIUM, "Oxanium"),
            ("dela_gothic_one/DelaGothicOne-Regular.ttf", "Dela Gothic One"),
        ] {
            assert_eq!(family_name(&fs::read(third_party(file)).unwrap()).unwrap(), family);
        }
        assert!(family_name(b"not a font").is_err());
    }

    /// `IMPORTER` says which `skera` a cache key's cut came from.
    #[test]
    fn the_importers_name_holds_the_subsetter_in_cargo_lock() {
        let lock = include_str!("../../../Cargo.lock");
        let version = lock
            .split("[[package]]")
            .find_map(|package| {
                let mut lines = package.trim().lines();
                (lines.next()? == "name = \"skera\"").then(|| {
                    lines.next().unwrap().trim_start_matches("version = \"").trim_end_matches('"').to_owned()
                })
            })
            .expect("skera in Cargo.lock");
        assert_eq!(IMPORTER.name, format!("font (skera {version})"));
    }
}
