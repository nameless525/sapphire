//! Authored data: RON documents (editor documents, configs, `.meta` sidecars).
//!
//! A document wraps its data in an envelope naming the type's [stable
//! name](sapphire_reflect::stable_name) and the schema version it was written with:
//!
//! ```ron
//! (
//!     type: "sapphire::render::AnimeCharacterMaterial",
//!     version: 1,
//!     data: (
//!         base_color: (1.0, 0.9, 0.8, 1.0),
//!     ),
//! )
//! ```
//!
//! Data from an older version upgrades on load ([`Versioned`]). `Option` fields may leave
//! out `Some(..)` (RON's `implicit_some`). Errors point at the line and column in the
//! document, including errors inside `data`.
//!
//! [`from_ron_dynamic`] loads a document whose type is known only by name, through a type
//! registry: hot reload, live link and the editor. Types opt in with `#[reflect(Document)]`.

use ron::error::{Position, SpannedError};
use ron::extensions::Extensions;
use ron::ser::PrettyConfig;
use ron::value::RawValue;
use sapphire_reflect::{
    CreateTypeData, Decoder, Reflect, TypePath, TypeRegistry, VersionError, Versioned, decode_versioned,
    stable_name,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// A document that can't be read or written.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    /// Malformed RON, or data that doesn't fit the type. The position is in the document.
    #[error("{0}")]
    Parse(SpannedError),
    #[error("the document holds `{found}`, not `{expected}`")]
    WrongType { expected: &'static str, found: String },
    #[error("schema version {found} is newer than this build's {current}")]
    TooNew { found: u32, current: u32 },
    #[error("schema version {found} is older than the oldest supported, {oldest}")]
    TooOld { found: u32, oldest: u32 },
    #[error("no registered type is named `{0}`")]
    UnknownType(String),
    #[error("`{0}` isn't registered as a document; add #[reflect(Document)]")]
    NotADocument(String),
    #[error("writing RON: {0}")]
    Write(#[from] ron::Error),
}

impl From<VersionError<SpannedError>> for DocumentError {
    fn from(error: VersionError<SpannedError>) -> Self {
        match error {
            VersionError::TooNew { found, current } => Self::TooNew { found, current },
            VersionError::TooOld { found, oldest } => Self::TooOld { found, oldest },
            VersionError::Decode(error) => Self::Parse(error),
        }
    }
}

/// Writes `value` as a document.
pub fn to_ron<T: Versioned + Serialize + TypePath>(value: &T) -> Result<String, DocumentError> {
    #[derive(Serialize)]
    struct Envelope<'a, T> {
        #[serde(rename = "type")]
        type_name: &'static str,
        version: u32,
        data: &'a T,
    }
    let envelope = Envelope { type_name: stable_name::<T>(), version: T::VERSION, data: value };
    // RON defaults to CRLF on Windows; documents are the same bytes on every platform.
    let config = PrettyConfig::new().new_line("\n").indentor("    ");
    let mut text = ron::ser::to_string_pretty(&envelope, config)?;
    text.push('\n');
    Ok(text)
}

/// Reads a document holding a `T`, upgrading data from older schema versions.
pub fn from_ron<T: Versioned + DeserializeOwned + TypePath>(text: &str) -> Result<T, DocumentError> {
    let envelope = Envelope::parse(text)?;
    let expected = stable_name::<T>();
    if envelope.type_name != expected {
        return Err(DocumentError::WrongType { expected, found: envelope.type_name });
    }
    envelope.decode(text)
}

/// Reads a document of any type registered with `#[reflect(Document)]`.
pub fn from_ron_dynamic(registry: &TypeRegistry, text: &str) -> Result<Box<dyn Reflect>, DocumentError> {
    let envelope = Envelope::parse(text)?;
    let document = registered(registry, &envelope.type_name)?;
    (document.load)(text, envelope)
}

/// The stable name a document's envelope gives its type, without decoding its data: for
/// readers that pick the type to decode as from a closed set (material families).
pub fn type_name(text: &str) -> Result<String, DocumentError> {
    Ok(Envelope::parse(text)?.type_name)
}

/// Writes a value of any type registered with `#[reflect(Document)]` as a document.
pub fn to_ron_dynamic(registry: &TypeRegistry, value: &dyn Reflect) -> Result<String, DocumentError> {
    let document = registered(registry, value.reflect_type_path())?;
    (document.save)(value)
}

fn registered<'r>(registry: &'r TypeRegistry, type_name: &str) -> Result<&'r ReflectDocument, DocumentError> {
    let registration = registry
        .get_with_type_path(type_name)
        .ok_or_else(|| DocumentError::UnknownType(type_name.to_owned()))?;
    registration.data::<ReflectDocument>().ok_or_else(|| DocumentError::NotADocument(type_name.to_owned()))
}

/// Registry type data that loads and saves a type's documents by name. Added by
/// `#[reflect(Document)]` on a type with `Versioned`, serde and a stable `#[type_path]`.
#[derive(Clone)]
pub struct ReflectDocument {
    load: LoadFn,
    save: fn(&dyn Reflect) -> Result<String, DocumentError>,
}

/// Decodes a parsed document's data as the registered type.
type LoadFn = fn(&str, Envelope<'_>) -> Result<Box<dyn Reflect>, DocumentError>;

impl<T> CreateTypeData<T> for ReflectDocument
where
    T: Versioned + Serialize + DeserializeOwned + Reflect + TypePath,
{
    fn create_type_data((): ()) -> Self {
        Self {
            load: |text, envelope| {
                envelope.decode::<T>(text).map(|value| Box::new(value) as Box<dyn Reflect>)
            },
            save: |value| {
                // Registrations are looked up by the value's own type path, so this is a `T`.
                let value = value.downcast_ref::<T>().expect("a value of the registered type");
                to_ron(value)
            },
        }
    }
}

/// A parsed envelope; `data` is still raw text inside the document.
struct Envelope<'a> {
    type_name: String,
    version: u32,
    data: &'a RawValue,
}

impl<'a> Envelope<'a> {
    fn parse(text: &'a str) -> Result<Self, DocumentError> {
        #[derive(Deserialize)]
        struct Raw<'a> {
            #[serde(rename = "type")]
            type_name: String,
            version: u32,
            #[serde(borrow)]
            data: &'a RawValue,
        }
        let Raw { type_name, version, data } = options().from_str(text).map_err(DocumentError::Parse)?;
        Ok(Self { type_name, version, data })
    }

    /// Decodes `data` as a `T` at the envelope's version. `text` is the whole document, which
    /// `data` points into.
    fn decode<T: Versioned + DeserializeOwned>(self, text: &str) -> Result<T, DocumentError> {
        Ok(decode_versioned::<T, _>(self.version, Data { text, raw: self.data })?)
    }
}

/// An envelope's `data`, decoded with errors positioned in the whole document.
struct Data<'a> {
    text: &'a str,
    raw: &'a RawValue,
}

impl Decoder for Data<'_> {
    type Error = SpannedError;

    fn decode<T: DeserializeOwned>(self) -> Result<T, SpannedError> {
        let raw = self.raw.get_ron();
        options().from_str(raw).map_err(|error| {
            // `raw` is a slice of `text`: errors count from its start, so shift them by
            // where it starts in the document.
            let offset = (raw.as_ptr() as usize).saturating_sub(self.text.as_ptr() as usize);
            let origin = position_at(&self.text[..offset.min(self.text.len())]);
            let shift = |at: Position| match at.line {
                1 => Position { line: origin.line, col: origin.col + at.col - 1 },
                line => Position { line: origin.line + line - 1, col: at.col },
            };
            SpannedError {
                code: error.code,
                span: ron::error::Span { start: shift(error.span.start), end: shift(error.span.end) },
            }
        })
    }
}

/// The one-based position just past `prefix`.
fn position_at(prefix: &str) -> Position {
    let line = 1 + prefix.matches('\n').count();
    let col = 1 + prefix.rsplit('\n').next().map_or(0, |last| last.chars().count());
    Position { line, col }
}

fn options() -> ron::Options {
    ron::Options::default().with_default_extension(Extensions::IMPLICIT_SOME)
}

#[cfg(test)]
mod tests {
    use bevy_reflect::Reflect;

    use super::*;

    #[derive(Debug, PartialEq, Reflect, Serialize, Deserialize)]
    #[type_path = "sapphire::test"]
    #[reflect(Document)]
    struct Fog {
        density: f32,
        #[serde(default)]
        color: Option<[f32; 3]>,
    }

    impl Versioned for Fog {
        const VERSION: u32 = 2;

        fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
            decode_versioned::<v1::Fog, D>(version, data)
                .map(|old| Fog { density: old.thickness, color: None })
        }
    }

    mod v1 {
        #[derive(serde::Deserialize)]
        pub struct Fog {
            pub thickness: f32,
        }

        impl sapphire_reflect::Versioned for Fog {
            const VERSION: u32 = 1;
        }
    }

    #[derive(Debug, Reflect, Serialize, Deserialize)]
    #[type_path = "sapphire::test"]
    struct Undocumented;

    #[test]
    fn documents_round_trip_with_an_envelope() {
        let fog = Fog { density: 0.25, color: Some([0.5, 0.6, 0.7]) };
        let text = to_ron(&fog).expect("write");
        assert!(text.starts_with("(\n    type: \"sapphire::test::Fog\",\n    version: 2,\n"), "{text}");
        assert!(!text.contains('\r'), "documents use LF everywhere");
        assert_eq!(from_ron::<Fog>(&text).expect("read"), fog);
    }

    #[test]
    fn authored_documents_may_skip_some_and_defaulted_fields() {
        let text =
            r#"(type: "sapphire::test::Fog", version: 2, data: (density: 1.0, color: (1.0, 0.0, 0.0)))"#;
        assert_eq!(from_ron::<Fog>(text).expect("read").color, Some([1.0, 0.0, 0.0]));
        let text = r#"(type: "sapphire::test::Fog", version: 2, data: (density: 1.0))"#;
        assert_eq!(from_ron::<Fog>(text).expect("read").color, None);
    }

    #[test]
    fn old_versions_upgrade_and_new_ones_are_rejected() {
        let text = r#"(type: "sapphire::test::Fog", version: 1, data: (thickness: 0.5))"#;
        assert_eq!(from_ron::<Fog>(text).expect("upgrade"), Fog { density: 0.5, color: None });
        let text = r#"(type: "sapphire::test::Fog", version: 3, data: ())"#;
        assert!(matches!(from_ron::<Fog>(text), Err(DocumentError::TooNew { found: 3, current: 2 })));
        let text = r#"(type: "sapphire::test::Light", version: 2, data: ())"#;
        assert!(matches!(from_ron::<Fog>(text), Err(DocumentError::WrongType { .. })));
    }

    #[test]
    fn errors_inside_data_point_into_the_document() {
        let text = "(\n    type: \"sapphire::test::Fog\",\n    version: 2,\n    data: (\n        density: \"thick\",\n    ),\n)\n";
        let Err(DocumentError::Parse(error)) = from_ron::<Fog>(text) else {
            panic!("expected a parse error")
        };
        assert_eq!(error.span.start.line, 5, "{error}");
        assert!(error.span.start.col > 9, "{error}");
    }

    #[test]
    fn registered_documents_load_and_save_by_name() {
        let mut registry = TypeRegistry::new();
        registry.register::<Fog>();
        registry.register::<Undocumented>();
        let text = r#"(type: "sapphire::test::Fog", version: 1, data: (thickness: 0.5))"#;
        let value = from_ron_dynamic(&registry, text).expect("load");
        assert_eq!(value.downcast_ref::<Fog>(), Some(&Fog { density: 0.5, color: None }));
        assert_eq!(
            to_ron_dynamic(&registry, value.as_ref()).expect("save"),
            to_ron(&Fog { density: 0.5, color: None }).expect("write")
        );

        let unknown = r#"(type: "sapphire::test::Missing", version: 1, data: ())"#;
        assert!(matches!(from_ron_dynamic(&registry, unknown), Err(DocumentError::UnknownType(_))));
        assert!(matches!(to_ron_dynamic(&registry, &Undocumented), Err(DocumentError::NotADocument(_))));
    }
}
