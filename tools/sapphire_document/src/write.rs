//! Values as RON: what a splice writes, what a change record keeps, and back.
//!
//! Every type in a document says `#[reflect(Serialize, Deserialize)]`, so a value here goes through its own
//! serde: a field left out, renamed or written as a tuple comes out as the file's parser reads it.

use bevy_reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use bevy_reflect::{PartialReflect, TypeRegistration, TypeRegistry};
use ron::extensions::Extensions;
use ron::ser::PrettyConfig;

use crate::layout;
use crate::text::{Node, scan};

/// How long a new value may be and stay on one line: as long as the lines people wrote in these files.
const ONE_LINE: usize = 96;

/// A value that can't be written or read. A type that isn't registered, most likely: a bug here.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValueTextError(pub String);

fn options(implicit_some: bool) -> ron::Options {
    let options = ron::Options::default();
    if implicit_some { options.with_default_extension(Extensions::IMPLICIT_SOME) } else { options }
}

fn one_line() -> PrettyConfig {
    PrettyConfig::new().depth_limit(0).separator(" ")
}

/// The one way a value is written, whatever a file says: how a change record keeps it, and how two values
/// are told apart. Unlike `==`, it tells 0.0 from -0.0 and calls `NaN` itself.
pub fn canonical(value: &dyn PartialReflect, registry: &TypeRegistry) -> Result<String, ValueTextError> {
    let serializer = TypedReflectSerializer::new(value, registry);
    options(false)
        .to_string_pretty(&serializer, one_line())
        .map_err(|error| ValueTextError(error.to_string()))
}

/// How a value goes into a file.
#[derive(Clone, Copy, Debug)]
pub struct Style<'a> {
    /// Whether this file leaves `Some(..)` out.
    pub implicit_some: bool,
    /// The file's newline.
    pub newline: &'a str,
    /// The indent of the line the value starts on.
    pub indent: &'a str,
}

/// A new value as a person would write it there: on one line if it's short, and if it isn't, a line for
/// each field, under `style.indent`. It leaves out what a person leaves out: every field its type reads back
/// the same without.
pub fn styled(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    style: Style<'_>,
) -> Result<String, ValueTextError> {
    let registration = value.get_represented_type_info().and_then(|info| registry.get(info.type_id()));
    let written = |config: PrettyConfig| -> Result<String, ValueTextError> {
        let serializer = TypedReflectSerializer::new(value, registry);
        let written = options(style.implicit_some)
            .to_string_pretty(&serializer, config)
            .map_err(|error| ValueTextError(error.to_string()))?;
        Ok(match registration {
            Some(registration) => without_defaults(written, registration, registry),
            None => written,
        })
    };
    let one = written(one_line())?;
    if one.len() <= ONE_LINE {
        return Ok(one);
    }
    let lines = written(PrettyConfig::new().new_line(style.newline.to_owned()).indentor("    "))?;
    // RON writes a string's newlines as escapes, so a newline here is always the layout's.
    Ok(lines.replace(style.newline, &format!("{}{}", style.newline, style.indent)))
}

/// `text` without the fields it can go without: tried one at a time, last first, and kept out only if the
/// type's own parser reads the same value. What's a default is the parser's to say, and reflection can't
/// see `#[serde(default)]`.
fn without_defaults(mut text: String, registration: &TypeRegistration, registry: &TypeRegistry) -> String {
    let same_as = |candidate: &str, whole: &str| {
        read(candidate, registration, registry)
            .and_then(|value| canonical(value.as_ref(), registry))
            .is_ok_and(|value| value == whole)
    };
    let Ok(whole) = read(&text, registration, registry).and_then(|value| canonical(value.as_ref(), registry))
    else {
        return text;
    };
    let mut before = usize::MAX;
    loop {
        let Ok(tree) = scan(&text) else { return text };
        let mut fields = Vec::new();
        written_fields(&tree, &mut fields);
        let Some(next) =
            fields.into_iter().filter(|field| field.start < before).max_by_key(|field| field.start)
        else {
            return text;
        };
        before = next.start;
        let removal = layout::removal(&text, next.start, next.end, next.previous_end);
        if let Some(candidate) = layout::apply(&text, &[removal])
            && same_as(&candidate, &whole)
        {
            text = candidate;
        }
    }
}

struct WrittenField {
    start: usize,
    end: usize,
    previous_end: Option<usize>,
}

fn written_fields(node: &Node, out: &mut Vec<WrittenField>) {
    let mut previous_end = None;
    for field in node.fields() {
        out.push(WrittenField { start: field.start, end: field.value.span.end, previous_end });
        previous_end = Some(field.value.span.end);
        written_fields(&field.value, out);
    }
    for item in node.items() {
        written_fields(item, out);
    }
}

/// Reads `text` as a value of the registered type.
pub fn read(
    text: &str,
    registration: &TypeRegistration,
    registry: &TypeRegistry,
) -> Result<Box<dyn PartialReflect>, ValueTextError> {
    let seed = TypedReflectDeserializer::new(registration, registry);
    options(true).from_str_seed(text, seed).map_err(|error| ValueTextError(error.to_string()))
}
