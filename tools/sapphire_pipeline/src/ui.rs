//! Cooking UI documents (M3 U4, spec §5.17, §5.27): `.ui.ron` to `.sui`, and what a screen's keys say to
//! the font cook.
//!
//! A document's own checks run first, each naming its field, then its paths against the
//! [`UiModel`] it names (`hud.model.ron`, which isn't a source: the documents that name it read it, and
//! a change to it cooks them again).
//!
//! # What screens tell fonts
//!
//! A text node names a key and its style a role, which is what a table alone can't say: which fonts
//! show which string. So the cook works out, for every text node and every locale:
//!
//! - **What it can show** ([`sapphire_strings::key_shows`]): the message's literals in every arm of every
//!   select, what it refers to down the locale's chain, and what numbers are written in. Those go in the
//!   subset of every family the locale's role names, in the `fonts` pack, on top of the font's
//!   `CharacterSet` ([`Screens::characters_of`]).
//! - **Whether it can**: [`Screens::check_strings`] refuses a key no table of the chain holds, a role the
//!   locale has no family list for and a variable the node doesn't pass, before any font cooks, and
//!   [`Screens::check_fonts`] refuses a character no family of the list has, a family no font is, and
//!   a family outside its role's `FontChains`, after they have. Each names the screen, the node, the
//!   locale, the key and the character. Those two run on every cook, from the files as they now are,
//!   since a cached table can disagree with a new document and neither's key covers the other's sources.
//!
//! # Where the line is
//!
//! **What a document binds comes from the tables, and `ui.charset.ron` is the rest:** what no document
//! can say. That's what code passes as a variable (a name a player typed, so kana and the commonest Han),
//! the accents of pseudo-localization, and a key chosen at runtime, which is why a sequencer's subtitles
//! will need a bound key or a list of their own (spec §5.17). A variable's characters are not checked,
//! since they aren't known here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use read_fonts_for_skera::FontRef;
use sapphire_assets::AssetId;
use sapphire_assets::font::{FONTS_PACK, Font, FontChains};
use sapphire_assets::ui::{UiDocument, UiError, UiModel};
use sapphire_strings::{KeyShows, StringTable, key_shows};
use serde::{Deserialize, Serialize};

use crate::cache::{Dependency, Stage};
use crate::font::{self, FontCookError};

/// The UI document importer as cache keys know it.
pub const IMPORTER: Stage = Stage { name: "UI document", version: 1 };
/// The UI document cooker. Bump it whenever it changes output for the same document.
pub const COOKER: Stage = Stage { name: "sui", version: 1 };

/// What a UI document's file name ends with.
const SOURCE_SUFFIX: &str = ".ui.ron";

/// Import settings for a UI document, none yet: an empty struct rather than `()`, so settings can join
/// without changing existing sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {}

/// A UI document that won't cook.
#[derive(Debug, thiserror::Error)]
pub enum UiCookError {
    #[error(transparent)]
    Document(#[from] UiError),
    #[error("{field}: {problem}")]
    Path { field: String, problem: String },
    #[error("its model `{path}`")]
    Model {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("not UTF-8")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("cooking")]
    Cooked(#[from] sapphire_serialize::cooked::CookedError),
}

/// A screen that a language can't show, or that doesn't say enough to try.
#[derive(Debug, thiserror::Error)]
pub enum ScreenError {
    #[error("reading {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("{screen} shows text, and the tree cooks no locale to show it in")]
    NoLocale { screen: PathBuf },
    #[error("{screen} node `{node}`: locale `{locale}`, its fallbacks included, has no key `{key}`")]
    NoKey { screen: PathBuf, node: String, locale: String, key: String },
    #[error("{screen} node `{node}`: locale `{locale}` has no font list for the role `{role}`")]
    NoRole { screen: PathBuf, node: String, locale: String, role: String },
    #[error("{screen} node `{node}`: `{key}` in `{locale}` reads `${variable}`, which its `args` don't pass")]
    NoArg { screen: PathBuf, node: String, locale: String, key: String, variable: String },
    #[error(
        "{screen} node `{node}`: `{key}` in `{locale}` shows {character:?} (U+{code:04X}), which no font of \
         its `{role}` list [{families}] has"
    )]
    Uncovered {
        screen: PathBuf,
        node: String,
        locale: String,
        key: String,
        role: String,
        character: char,
        code: u32,
        families: String,
    },
    #[error(
        "locale `{locale}` shows the role `{role}` in `{family}`, which no font of the tree's `fonts` pack is"
    )]
    NoFont { locale: String, role: String, family: String },
    #[error(
        "locale `{locale}` shows the role `{role}` in `{family}`, and the font chain `{role}` doesn't load that"
    )]
    NotInChain { locale: String, role: String, family: String },
    #[error("a screen shows the role `{role}`, and no font chain loads it")]
    NoChain { role: String },
    #[error("a font: {0}")]
    Font(#[from] FontCookError),
}

/// Whether `path` is a UI document, whose extension alone is just `.ron`.
pub fn is_source(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(SOURCE_SUFFIX))
}

/// A UI document's cooked path: `menu.ui.ron` to `menu.sui`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    relative.with_file_name(format!("{}.{extension}", name.strip_suffix(SOURCE_SUFFIX).unwrap_or(name)))
}

/// The model a document names, read from beside the document at `path`.
fn read_model(path: &Path, name: &str) -> Result<(PathBuf, Vec<u8>), UiCookError> {
    let full = path.parent().unwrap_or(Path::new("")).join(name);
    let bytes = std::fs::read(&full)
        .map_err(|source| UiCookError::Model { path: full.clone(), source: Box::new(source) })?;
    Ok((full, bytes))
}

/// What a document reads besides itself: the model it names.
pub fn dependencies(path: &Path, bytes: &[u8]) -> Result<Vec<Dependency>, UiCookError> {
    let document = UiDocument::from_document(std::str::from_utf8(bytes)?)?;
    let Some(name) = document.model else { return Ok(Vec::new()) };
    let (_, model) = read_model(path, &name)?;
    Ok(vec![Dependency::File { path: name, hash: blake3::hash(&model) }])
}

/// The bytes of a document's cooked file, read back through the runtime's check.
pub fn cook(path: &Path, bytes: &[u8]) -> Result<Vec<u8>, UiCookError> {
    let document = UiDocument::from_document(std::str::from_utf8(bytes)?)?;
    if let Some(name) = &document.model {
        let (full, model) = read_model(path, name)?;
        let model = match std::str::from_utf8(&model) {
            Ok(text) => UiModel::from_document(text)
                .map_err(|source| UiCookError::Model { path: full, source: Box::new(source) })?,
            Err(source) => return Err(UiCookError::Model { path: full, source: Box::new(source) }),
        };
        if let Some(issue) = document.path_issues(&model).into_iter().next() {
            return Err(UiCookError::Path { field: issue.field, problem: issue.problem });
        }
    }
    let cooked = document.to_cooked()?;
    UiDocument::read(&cooked)?;
    Ok(cooked.into_vec())
}

/// One text node's strings, in one locale.
struct Shown<'a> {
    screen: &'a Path,
    node: &'a str,
    key: &'a str,
    role: &'a str,
    args: Vec<&'a str>,
    locale: &'a str,
    /// The chain's first answer for the role.
    families: Option<&'a [String]>,
    /// None if no table of the chain holds the key.
    shows: Option<KeyShows>,
}

/// Every screen of a cooked tree and every table, as the font cook and the checks read them.
#[derive(Debug, Default)]
pub struct Screens {
    documents: Vec<(PathBuf, UiDocument)>,
    tables: Vec<StringTable>,
}

impl Screens {
    /// Reads the cooked documents and tables at `outputs`, which can hold anything else too. Each file's
    /// source goes with it, to name a screen by.
    pub fn read<'a>(outputs: impl Iterator<Item = (&'a Path, &'a Path)>) -> Result<Self, ScreenError> {
        let mut screens = Self::default();
        let read = |path: &Path| {
            std::fs::read(path)
                .map_err(|source| ScreenError::Read { path: path.to_owned(), source: Box::new(source) })
        };
        let refused = |path: &Path, source: Box<dyn std::error::Error + Send + Sync>| ScreenError::Read {
            path: path.to_owned(),
            source,
        };
        for (source, output) in outputs {
            match output.extension().and_then(|extension| extension.to_str()) {
                Some(sapphire_assets::ui::UI_EXTENSION) => {
                    let mut bytes = sapphire_serialize::cooked::AlignedVec::<16>::new();
                    bytes.extend_from_slice(&read(output)?);
                    let document =
                        UiDocument::read(&bytes).map_err(|error| refused(output, Box::new(error)))?;
                    let document =
                        document.to_owned_document().map_err(|error| refused(output, Box::new(error)))?;
                    screens.documents.push((source.to_owned(), document));
                }
                Some(sapphire_strings::STRINGS_EXTENSION) => {
                    let table = StringTable::read(&read(output)?)
                        .map_err(|error| refused(output, Box::new(error)))?;
                    screens.tables.push(table);
                }
                _ => {}
            }
        }
        Ok(screens)
    }

    /// Whether any screen shows text.
    pub fn shows_text(&self) -> bool {
        self.documents.iter().any(|(_, document)| document.texts().next().is_some())
    }

    /// `table` and the tables of its fallback chain that the tree holds, in order.
    fn chain_of<'a>(&'a self, table: &'a StringTable) -> Vec<&'a StringTable> {
        let mut chain = vec![table];
        chain.extend(
            table
                .fallback
                .iter()
                .filter_map(|locale| self.tables.iter().find(|other| other.locale == *locale)),
        );
        chain
    }

    /// Every text node of every screen, in every locale.
    fn shown(&self) -> impl Iterator<Item = Shown<'_>> {
        self.documents.iter().flat_map(move |(screen, document)| {
            document.texts().flat_map(move |(node, text, role)| {
                self.tables.iter().map(move |table| {
                    let chain = self.chain_of(table);
                    Shown {
                        screen,
                        node: &node.name,
                        key: &text.key,
                        role,
                        args: text.args.iter().map(|arg| arg.name.as_str()).collect(),
                        locale: &table.locale,
                        families: chain.iter().find_map(|link| link.font_chain(role)),
                        shows: key_shows(&chain, &text.key),
                    }
                })
            })
        })
    }

    /// What the screens need each family to hold, which goes in its font's subset for the `fonts` pack.
    pub fn characters_of(&self) -> ScreenCharacters {
        let mut families: BTreeMap<String, BTreeSet<char>> = BTreeMap::new();
        for shown in self.shown() {
            let (Some(list), Some(shows)) = (shown.families, shown.shows) else { continue };
            for family in list {
                families.entry(family.clone()).or_default().extend(shows.characters.iter().copied());
            }
        }
        ScreenCharacters { families }
    }

    /// Refuses a key, a role or a variable a screen needs and a table doesn't have.
    pub fn check_strings(&self) -> Result<(), ScreenError> {
        if let Some((screen, _)) =
            self.documents.iter().find(|(_, document)| document.texts().next().is_some())
            && self.tables.is_empty()
        {
            return Err(ScreenError::NoLocale { screen: screen.clone() });
        }
        for shown in self.shown() {
            let names = || (shown.screen.to_owned(), shown.node.to_owned(), shown.locale.to_owned());
            let Some(shows) = &shown.shows else {
                let (screen, node, locale) = names();
                return Err(ScreenError::NoKey { screen, node, locale, key: shown.key.to_owned() });
            };
            if shown.families.is_none() {
                let (screen, node, locale) = names();
                return Err(ScreenError::NoRole { screen, node, locale, role: shown.role.to_owned() });
            }
            if let Some(variable) = shows.variables.iter().find(|name| !shown.args.contains(&name.as_str())) {
                let (screen, node, locale) = names();
                return Err(ScreenError::NoArg {
                    screen,
                    node,
                    locale,
                    key: shown.key.to_owned(),
                    variable: variable.clone(),
                });
            }
        }
        Ok(())
    }

    /// Refuses a string a screen shows that its fonts can't, from the `fonts` pack's cooked `fonts`, and
    /// a family its role's chain in `chains` doesn't load. `fonts` holds each cooked font by ID.
    pub fn check_fonts(&self, fonts: &[(AssetId, Font)], chains: &[FontChains]) -> Result<(), ScreenError> {
        if !self.shows_text() {
            return Ok(());
        }
        let fonts: Vec<(AssetId, &Font)> =
            fonts.iter().filter(|(_, font)| font.pack == FONTS_PACK).map(|(id, font)| (*id, font)).collect();
        let opened = fonts
            .iter()
            .map(|(_, font)| font::open(&font.file).map_err(FontCookError::from))
            .collect::<Result<Vec<FontRef<'_>>, _>>()?;
        let of_family = |family: &str| -> Vec<usize> {
            fonts
                .iter()
                .enumerate()
                .filter(|(_, (_, font))| font.family == family)
                .map(|(at, _)| at)
                .collect()
        };
        let mut checked: BTreeSet<(&str, &str)> = BTreeSet::new();
        for shown in self.shown() {
            let (Some(list), Some(shows)) = (shown.families, &shown.shows) else { continue };
            if checked.insert((shown.locale, shown.role)) {
                let names =
                    |family: &String| (shown.locale.to_owned(), shown.role.to_owned(), family.clone());
                let chain =
                    chains.iter().flat_map(|chains| &chains.chains).find(|chain| chain.name == shown.role);
                let Some(chain) = chain else {
                    return Err(ScreenError::NoChain { role: shown.role.to_owned() });
                };
                for family in list {
                    let (locale, role, name) = names(family);
                    let held = of_family(family);
                    if held.is_empty() {
                        return Err(ScreenError::NoFont { locale, role, family: name });
                    }
                    if !held.iter().any(|&at| chain.fonts.contains(&fonts[at].0)) {
                        return Err(ScreenError::NotInChain { locale, role, family: name });
                    }
                }
            }
            let list_fonts: Vec<usize> = list.iter().flat_map(|family| of_family(family)).collect();
            for &character in &shows.characters {
                if character.is_control() || list_fonts.iter().any(|&at| font::covers(&opened[at], character))
                {
                    continue;
                }
                return Err(ScreenError::Uncovered {
                    screen: shown.screen.to_owned(),
                    node: shown.node.to_owned(),
                    locale: shown.locale.to_owned(),
                    key: shown.key.to_owned(),
                    role: shown.role.to_owned(),
                    character,
                    code: character as u32,
                    families: list.join(", "),
                });
            }
        }
        Ok(())
    }
}

/// What the screens of a tree need each font family to hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenCharacters {
    families: BTreeMap<String, BTreeSet<char>>,
}

impl ScreenCharacters {
    /// What `family` must hold, if any screen shows it.
    pub fn of(&self, family: &str) -> Option<&BTreeSet<char>> {
        self.families.get(family)
    }
}
