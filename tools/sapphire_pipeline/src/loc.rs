//! Localized strings (M3 V2, spec §5.25): `.ftl` sources and `.locale.ron` documents cooked to
//! `.sloc` tables, one per locale, which `sapphire_loc` runs.
//!
//! A locale is a document, `ja.locale.ron`, whose file name is its tag, stating its fallback chain and
//! its font family names:
//!
//! ```ron
//! (
//!     type: "sapphire::strings::LocaleDocument",
//!     version: 1,
//!     data: (
//!         fallback: ["en"],
//!         fonts: {
//!             "body": ["M PLUS 1"],
//!             "display": ["Dela Gothic One", "Oxanium"],
//!         },
//!     ),
//! )
//! ```
//!
//! Its strings are every `.ftl` source whose sidecar says `importer: Fluent((locale: "ja"))`, wherever
//! it lies: no list in code or in the document. A new sidecar takes its locale from the folder the file
//! is in (`loc/ja/menus.ftl`). A `.ftl` cooks no asset of its own, and an edit of one recooks its
//! locale's table, which is `ja.sloc` beside the document.
//!
//! Parsing is `fluent-syntax`'s, only here. What it reads, [`compile`] lowers to a flat table:
//! adjacent text and literals become one, so a message with nothing to work out is one span and comes
//! back with no allocation. The cook fails by name on a parse error, a duplicate key in a locale, a
//! function other than `NUMBER` or a `NUMBER` option other than the three it runs, a selector Fluent
//! doesn't run, and everything [`check()`] finds across tables.
//!
//! [`check()`] runs on each cook against the tables as written, cached or not: fast, since a table is
//! small, and it means no table's key has to cover other locales' files, so an edit of English recooks
//! only English and still fails the cook if Japanese no longer agrees. What it lists goes in the
//! [`CookReport`](crate::cook::CookReport), so what's untranslated shows on every cook.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use fluent_syntax::ast;
use fluent_syntax::parser::parse;
use icu_locale_core::Locale;
use sapphire_reflect::Versioned;
use sapphire_serialize::{DocumentError, from_ron};
use sapphire_strings::{
    ArgValue, Entry, Expression, FontChain, MAX_FRACTION_DIGITS, NumberFormat, Part, Pattern, Run, Select,
    Span, StringTable, StringsError, TermArg, Variant, VariantKey,
};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

pub mod check;

pub use check::{LocCheckError, LocReport, Stale, Untranslated, check};

/// Reads `.ftl` files and the locale's document. Bump the version whenever that changes output for the
/// same sources.
pub const IMPORTER: Stage = Stage { name: "fluent-syntax 0.12.0", version: 1 };

/// The `.sloc` writer. Bump the version whenever it changes output for the same sources.
pub const COOKER: Stage = Stage { name: "sloc", version: 1 };

/// What a locale document's file name ends with.
pub const LOCALE_SUFFIX: &str = ".locale.ron";

/// What a Fluent source's file name ends with.
const FTL_EXTENSION: &str = "ftl";

/// Import settings for a Fluent source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FluentSettings {
    /// The tag of the locale it's part of, which a `.locale.ron` document of that name must state.
    pub locale: String,
}

impl FluentSettings {
    /// A new sidecar's settings: the locale is the folder the file is in. A wrong guess fails the cook
    /// by name, as no locale document answers to it.
    pub fn for_source(source: &Path) -> Self {
        let parent = source.parent().and_then(Path::file_name).and_then(|name| name.to_str());
        Self { locale: parent.unwrap_or_default().to_owned() }
    }
}

/// Import settings for locale documents: none yet. A struct, so settings can join without changing
/// existing sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocaleDocumentSettings {}

/// A locale: see the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TypePath)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::strings"]
pub struct LocaleDocument {
    /// Where a key this locale lacks is looked for, in order: `ja`'s is `["en"]`, and the locale every
    /// other falls back to has none.
    #[serde(default)]
    pub fallback: Vec<String>,
    /// For each role of text, the font family names it shows in, first choice first.
    #[serde(default)]
    pub fonts: BTreeMap<String, Vec<String>>,
}

impl Versioned for LocaleDocument {
    const VERSION: u32 = 1;
}

/// A locale that can't cook. Each names what it's about: the file, and in it the line or the key.
#[derive(Debug, thiserror::Error)]
pub enum LocCookError {
    #[error("{} isn't UTF-8", path.display())]
    Encoding { path: PathBuf },
    #[error("the locale document can't be read")]
    Document(#[from] DocumentError),
    #[error("`{0}` isn't a locale tag")]
    BadTag(String),
    #[error("{}:{line}:{column}: {message}", path.display())]
    Syntax { path: PathBuf, line: usize, column: usize, message: String },
    #[error("key `{key}` is in both {} and {}", first.display(), second.display())]
    DuplicateKey { key: String, first: PathBuf, second: PathBuf },
    #[error("{}: `{key}` calls {function}(), and only NUMBER() runs here", path.display())]
    UnsupportedFunction { path: PathBuf, key: String, function: String },
    #[error("{}: `{key}`: {problem}", path.display())]
    BadNumber { path: PathBuf, key: String, problem: String },
    #[error("{}: `{key}` selects on something only a variable, NUMBER(), a number or a term's attribute can be", path.display())]
    UnsupportedSelector { path: PathBuf, key: String },
    #[error("{}: `{key}` gives a term a positional argument, which Fluent ignores", path.display())]
    PositionalTermArgument { path: PathBuf, key: String },
    #[error("font role `{role}` names no families")]
    EmptyFontChain { role: String },
    #[error("the table it made can't be written or read back")]
    Table(#[from] StringsError),
}

/// A `.ftl` source of a locale.
#[derive(Clone, Debug)]
pub struct FluentFile {
    pub path: PathBuf,
    /// What a cache key calls it: its path under the content root, with forward slashes.
    pub name: String,
}

/// Every `.ftl` source by the locale its sidecar names, in path order.
pub type FluentFiles = BTreeMap<String, Vec<FluentFile>>;

/// Whether `path` names a Fluent source.
pub fn is_ftl_source(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(FTL_EXTENSION))
}

/// Whether `path` names a locale document.
pub fn is_locale_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(LOCALE_SUFFIX))
}

/// The tag of the locale document at `path`: its file name, up to the suffix.
pub fn locale_of_document(path: &Path) -> &str {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    &name[..name.len().saturating_sub(LOCALE_SUFFIX.len())]
}

/// The cooked file a locale document at `relative` writes: `ja.locale.ron` → `ja.sloc`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    relative.with_file_name(format!("{}.{extension}", locale_of_document(relative)))
}

/// Cooks a locale: the document at `path`, holding `document`, and its `.ftl` sources.
pub fn cook(path: &Path, document: &[u8], files: &[FluentFile]) -> Result<Vec<u8>, LocCookError> {
    let encoding = |path: &Path| LocCookError::Encoding { path: path.to_owned() };
    let document: LocaleDocument = from_ron(std::str::from_utf8(document).map_err(|_| encoding(path))?)?;
    let tag = locale_of_document(path);
    let mut sources = Vec::with_capacity(files.len());
    for file in files {
        let bytes = std::fs::read(&file.path).map_err(|_| encoding(&file.path))?;
        let text = String::from_utf8(bytes).map_err(|_| encoding(&file.path))?;
        sources.push((file.path.clone(), text));
    }
    let table = compile(tag, &document, &sources)?;
    let bytes = table.to_cooked()?;
    // Read back through the runtime's checks, so a cook never writes what it would refuse.
    StringTable::read(&bytes)?;
    Ok(bytes)
}

/// Lowers a locale's sources to its table. See the [module docs](self).
pub fn compile(
    tag: &str,
    document: &LocaleDocument,
    sources: &[(PathBuf, String)],
) -> Result<StringTable, LocCookError> {
    if Locale::try_from_str(tag).is_err() {
        return Err(LocCookError::BadTag(tag.to_owned()));
    }
    for fallback in &document.fallback {
        if Locale::try_from_str(fallback).is_err() {
            return Err(LocCookError::BadTag(fallback.clone()));
        }
    }
    let mut fonts = Vec::with_capacity(document.fonts.len());
    for (role, families) in &document.fonts {
        if families.is_empty() {
            return Err(LocCookError::EmptyFontChain { role: role.clone() });
        }
        fonts.push(FontChain { role: role.clone(), families: families.clone() });
    }
    let mut builder = Builder::default();
    let mut seen: HashMap<String, &Path> = HashMap::new();
    for (path, text) in sources {
        let resource = parse(text.as_str()).map_err(|(_, errors)| {
            let first = errors.first().map_or(0, |error| error.pos.start);
            let message = errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ");
            let before = &text[..first.min(text.len())];
            let line = before.matches('\n').count() + 1;
            let column = before.rsplit('\n').next().map_or(0, |line| line.chars().count()) + 1;
            LocCookError::Syntax { path: path.clone(), line, column, message }
        })?;
        for entry in &resource.body {
            let (key, value, attributes) = match entry {
                ast::Entry::Message(message) => {
                    (message.id.name.to_owned(), message.value.as_ref(), &message.attributes)
                }
                ast::Entry::Term(term) => (format!("-{}", term.id.name), Some(&term.value), &term.attributes),
                _ => continue,
            };
            let mut lower = |key: String, pattern: &ast::Pattern<&str>| {
                if let Some(first) = seen.insert(key.clone(), path) {
                    return Err(LocCookError::DuplicateKey {
                        key,
                        first: first.to_owned(),
                        second: path.clone(),
                    });
                }
                builder.entry(path, key, pattern)
            };
            if let Some(value) = value {
                lower(key.clone(), value)?;
            }
            for attribute in attributes {
                lower(format!("{key}.{}", attribute.id.name), &attribute.value)?;
            }
        }
    }
    Ok(builder.finish(tag, document.fallback.clone(), fonts))
}

/// Builds a table's lists. Text is interned, so a key referenced many times is in the pool once. A
/// fixed order of sources makes the same bytes.
#[derive(Default)]
struct Builder {
    pool: String,
    interned: HashMap<String, Span>,
    /// With each its key as text, for sorting.
    entries: Vec<(String, Entry)>,
    parts: Vec<Part>,
    selects: Vec<Select>,
    variants: Vec<Variant>,
    args: Vec<TermArg>,
}

/// A pattern under construction: its finished parts, and text still to be joined by what follows.
#[derive(Default)]
struct Pieces {
    parts: Vec<Part>,
    text: String,
}

/// What's being lowered, for an error to name.
#[derive(Clone, Copy)]
struct Place<'a> {
    path: &'a Path,
    key: &'a str,
}

impl Builder {
    fn intern(&mut self, text: &str) -> Span {
        if let Some(&span) = self.interned.get(text) {
            return span;
        }
        let span = Span { start: self.pool.len() as u32, len: text.len() as u32 };
        self.pool.push_str(text);
        self.interned.insert(text.to_owned(), span);
        span
    }

    fn entry(&mut self, path: &Path, key: String, pattern: &ast::Pattern<&str>) -> Result<(), LocCookError> {
        let place = Place { path, key: &key };
        let lowered = self.pattern(place, pattern)?;
        let span = self.intern(&key);
        self.entries.push((key, Entry { key: span, pattern: lowered }));
        Ok(())
    }

    /// Lowers a pattern. Parts lie together, so a nested select's are pushed before this one's.
    fn pattern(&mut self, place: Place<'_>, pattern: &ast::Pattern<&str>) -> Result<Pattern, LocCookError> {
        let mut pieces = Pieces::default();
        for element in &pattern.elements {
            match element {
                ast::PatternElement::TextElement { value } => pieces.text.push_str(value),
                ast::PatternElement::Placeable { expression } => {
                    self.placeable(place, expression, &mut pieces)?;
                }
            }
        }
        if pieces.parts.is_empty() {
            return Ok(Pattern::Plain(self.intern(&pieces.text)));
        }
        self.flush(&mut pieces);
        let start = self.parts.len() as u32;
        let len = pieces.parts.len() as u32;
        self.parts.append(&mut pieces.parts);
        Ok(Pattern::Parts(Run { start, len }))
    }

    fn flush(&mut self, pieces: &mut Pieces) {
        if !pieces.text.is_empty() {
            let span = self.intern(&pieces.text);
            pieces.text.clear();
            pieces.parts.push(Part::Text(span));
        }
    }

    fn placeable(
        &mut self,
        place: Place<'_>,
        expression: &ast::Expression<&str>,
        pieces: &mut Pieces,
    ) -> Result<(), LocCookError> {
        match expression {
            ast::Expression::Inline(inline) => match inline {
                // A literal is text once its escapes are read: `{ "{" }`.
                ast::InlineExpression::StringLiteral { value } => {
                    pieces.text.push_str(&fluent_syntax::unicode::unescape_unicode_to_string(value));
                }
                ast::InlineExpression::Placeable { expression } => {
                    self.placeable(place, expression, pieces)?;
                }
                other => {
                    let expression = self.inline(place, other)?;
                    self.flush(pieces);
                    pieces.parts.push(Part::Place(expression));
                }
            },
            ast::Expression::Select { selector, variants } => {
                let selector = self.selector(place, selector)?;
                let mut lowered = Vec::with_capacity(variants.len());
                let mut default = 0;
                for (index, variant) in variants.iter().enumerate() {
                    if variant.default {
                        default = index as u32;
                    }
                    let key = match variant.key {
                        ast::VariantKey::Identifier { name } => VariantKey::Name(self.intern(name)),
                        ast::VariantKey::NumberLiteral { value } => {
                            VariantKey::Number(self.number(place, value)?.0)
                        }
                    };
                    lowered.push(Variant { key, pattern: self.pattern(place, &variant.value)? });
                }
                let start = self.variants.len() as u32;
                let run = Run { start, len: lowered.len() as u32 };
                self.variants.append(&mut lowered);
                let index = self.selects.len() as u32;
                self.selects.push(Select { selector, variants: run, default });
                self.flush(pieces);
                pieces.parts.push(Part::Select(index));
            }
        }
        Ok(())
    }

    fn selector(
        &mut self,
        place: Place<'_>,
        selector: &ast::InlineExpression<&str>,
    ) -> Result<Expression, LocCookError> {
        match selector {
            ast::InlineExpression::StringLiteral { .. }
            | ast::InlineExpression::MessageReference { .. }
            | ast::InlineExpression::Placeable { .. } => Err(LocCookError::UnsupportedSelector {
                path: place.path.to_owned(),
                key: place.key.to_owned(),
            }),
            // Only a term's attribute is text to choose by; Fluent's own parser refuses a term.
            other => self.inline(place, other),
        }
    }

    fn inline(
        &mut self,
        place: Place<'_>,
        inline: &ast::InlineExpression<&str>,
    ) -> Result<Expression, LocCookError> {
        let unsupported =
            || LocCookError::UnsupportedSelector { path: place.path.to_owned(), key: place.key.to_owned() };
        Ok(match inline {
            ast::InlineExpression::VariableReference { id } => Expression::Variable(self.intern(id.name)),
            ast::InlineExpression::NumberLiteral { value } => {
                let (value, min_fraction) = self.number(place, value)?;
                let format = NumberFormat {
                    min_fraction,
                    max_fraction: min_fraction.max(NumberFormat::default().max_fraction),
                    grouping: true,
                };
                Expression::Number { value, format }
            }
            ast::InlineExpression::MessageReference { id, attribute } => {
                let key = attribute.as_ref().map_or_else(
                    || id.name.to_owned(),
                    |attribute| format!("{}.{}", id.name, attribute.name),
                );
                Expression::Message(self.intern(&key))
            }
            ast::InlineExpression::TermReference { id, attribute, arguments } => {
                let key = attribute.as_ref().map_or_else(
                    || format!("-{}", id.name),
                    |attribute| format!("-{}.{}", id.name, attribute.name),
                );
                let args = self.term_args(place, arguments.as_ref())?;
                Expression::Term { key: self.intern(&key), args }
            }
            ast::InlineExpression::FunctionReference { id, arguments } => {
                if id.name != "NUMBER" {
                    return Err(LocCookError::UnsupportedFunction {
                        path: place.path.to_owned(),
                        key: place.key.to_owned(),
                        function: id.name.to_owned(),
                    });
                }
                self.number_function(place, arguments)?
            }
            ast::InlineExpression::StringLiteral { .. } | ast::InlineExpression::Placeable { .. } => {
                return Err(unsupported());
            }
        })
    }

    fn term_args(
        &mut self,
        place: Place<'_>,
        arguments: Option<&ast::CallArguments<&str>>,
    ) -> Result<Run, LocCookError> {
        let Some(arguments) = arguments else { return Ok(Run::default()) };
        if !arguments.positional.is_empty() {
            return Err(LocCookError::PositionalTermArgument {
                path: place.path.to_owned(),
                key: place.key.to_owned(),
            });
        }
        let mut args = Vec::with_capacity(arguments.named.len());
        for named in &arguments.named {
            let value = match &named.value {
                ast::InlineExpression::StringLiteral { value } => {
                    ArgValue::Text(self.intern(&fluent_syntax::unicode::unescape_unicode_to_string(value)))
                }
                ast::InlineExpression::NumberLiteral { value } => {
                    let (value, min_fraction) = self.number(place, value)?;
                    let max_fraction = min_fraction.max(NumberFormat::default().max_fraction);
                    let format = NumberFormat { min_fraction, max_fraction, grouping: true };
                    ArgValue::Number { value, format }
                }
                // Fluent's parser accepts only literals here.
                _ => {
                    return Err(LocCookError::PositionalTermArgument {
                        path: place.path.to_owned(),
                        key: place.key.to_owned(),
                    });
                }
            };
            args.push(TermArg { name: self.intern(named.name.name), value });
        }
        let run = Run { start: self.args.len() as u32, len: args.len() as u32 };
        self.args.append(&mut args);
        Ok(run)
    }

    /// `NUMBER($name, …)` or `NUMBER(3, …)`. Fluent's own options, of those a game's text wants: the
    /// rest (currency, percent, ordinals) are refused by name rather than ignored.
    fn number_function(
        &mut self,
        place: Place<'_>,
        arguments: &ast::CallArguments<&str>,
    ) -> Result<Expression, LocCookError> {
        let problem = |problem: String| LocCookError::BadNumber {
            path: place.path.to_owned(),
            key: place.key.to_owned(),
            problem,
        };
        let [argument] = arguments.positional.as_slice() else {
            return Err(problem("NUMBER() takes one value".to_owned()));
        };
        let (mut min, mut max) = (None, None);
        let mut grouping = true;
        for named in &arguments.named {
            let name = named.name.name;
            match (name, &named.value) {
                ("minimumFractionDigits", ast::InlineExpression::NumberLiteral { value }) => {
                    min = Some(self.fraction_digits(place, value)?);
                }
                ("maximumFractionDigits", ast::InlineExpression::NumberLiteral { value }) => {
                    max = Some(self.fraction_digits(place, value)?);
                }
                ("useGrouping", ast::InlineExpression::StringLiteral { value: "true" }) => grouping = true,
                ("useGrouping", ast::InlineExpression::StringLiteral { value: "false" }) => grouping = false,
                _ => return Err(problem(format!("NUMBER() can't take `{name}` as it's written here"))),
            }
        }
        let default = NumberFormat::default();
        let min_fraction = min.unwrap_or(default.min_fraction.min(max.unwrap_or(default.max_fraction)));
        let max_fraction = max.unwrap_or(default.max_fraction.max(min_fraction));
        if min_fraction > max_fraction {
            return Err(problem(format!("{min_fraction}..{max_fraction} fraction digits")));
        }
        let format = NumberFormat { min_fraction, max_fraction, grouping };
        match argument {
            ast::InlineExpression::VariableReference { id } => {
                Ok(Expression::NumberVariable { name: self.intern(id.name), format })
            }
            ast::InlineExpression::NumberLiteral { value } => {
                Ok(Expression::Number { value: self.number(place, value)?.0, format })
            }
            _ => Err(problem("NUMBER() takes a variable or a number".to_owned())),
        }
    }

    fn fraction_digits(&self, place: Place<'_>, value: &str) -> Result<u8, LocCookError> {
        value.parse::<u8>().ok().filter(|digits| *digits <= MAX_FRACTION_DIGITS).ok_or_else(|| {
            LocCookError::BadNumber {
                path: place.path.to_owned(),
                key: place.key.to_owned(),
                problem: format!("`{value}` fraction digits, where {MAX_FRACTION_DIGITS} is the most"),
            }
        })
    }

    /// A number as the source wrote it, and how many fraction digits it wrote.
    fn number(&self, place: Place<'_>, value: &str) -> Result<(f64, u8), LocCookError> {
        let bad = || LocCookError::BadNumber {
            path: place.path.to_owned(),
            key: place.key.to_owned(),
            problem: format!("`{value}` isn't a number that can be written"),
        };
        let number: f64 = value.parse().map_err(|_| bad())?;
        let digits = value.split_once('.').map_or(0, |(_, fraction)| fraction.len());
        let digits =
            u8::try_from(digits).ok().filter(|digits| *digits <= MAX_FRACTION_DIGITS).ok_or_else(bad)?;
        if !number.is_finite() {
            return Err(bad());
        }
        Ok((number, digits))
    }

    fn finish(mut self, tag: &str, fallback: Vec<String>, fonts: Vec<FontChain>) -> StringTable {
        self.entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        StringTable {
            locale: tag.to_owned(),
            fallback,
            fonts,
            pool: self.pool,
            entries: self.entries.into_iter().map(|(_, entry)| entry).collect(),
            parts: self.parts,
            selects: self.selects,
            variants: self.variants,
            args: self.args,
        }
    }
}

#[cfg(test)]
mod tests;
