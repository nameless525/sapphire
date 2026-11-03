//! The reflection walker: following a [`PropertyPath`] through a value, and through the text beside it.
//!
//! A value and its text have two shapes, and [`place`] and [`agreement`] are where they meet. Serde decides
//! the text's: a field can go by another name there ([`SerdeName`]), a wrapper may not be written at all
//! ([`Transparent`]), a struct like `Vec3` is written as a tuple, and an `Option` may or may not say `Some`.
//! A rule this gets wrong fails [`agreement`] when a document opens, naming the path, and that document
//! reads only: nothing here guesses at a file.

use std::borrow::Cow;
use std::ops::Range;

use bevy_reflect::enums::VariantType;
use bevy_reflect::list::List;
use bevy_reflect::{PartialReflect, ReflectFromReflect, ReflectMut, ReflectRef, TypeInfo, TypeRegistry};
use sapphire_reflect::{SerdeName, Transparent};

use crate::path::{PropertyPath, Step};
use crate::text::{Kind, Node};

/// A path that leads nowhere in a value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("nothing at `{path}`: `{stopped_at}` has no such step")]
pub struct NoSuchPath {
    pub path: String,
    pub stopped_at: String,
}

/// Whether `value` is an `Option`, which a path and a text both treat as more than an enum.
#[must_use]
pub fn is_option(value: &dyn PartialReflect) -> bool {
    value
        .get_represented_type_info()
        .is_some_and(|info| info.type_path().starts_with("core::option::Option<"))
}

/// Whether `value` is an `Option` holding nothing.
#[must_use]
pub fn is_none(value: &dyn PartialReflect) -> bool {
    is_option(value) && variant(value) == Some("None")
}

/// What `value` holds, past wrappers the text doesn't write.
#[must_use]
pub fn through(value: &dyn PartialReflect) -> &dyn PartialReflect {
    if is_transparent(value)
        && let ReflectRef::TupleStruct(wrapper) = value.reflect_ref()
        && let Some(inner) = wrapper.field(0)
    {
        return through(inner);
    }
    value
}

fn through_mut(value: &mut dyn PartialReflect) -> &mut dyn PartialReflect {
    if !is_transparent(value) {
        return value;
    }
    let ReflectMut::TupleStruct(wrapper) = value.reflect_mut() else {
        unreachable!("only a tuple struct is transparent");
    };
    through_mut(wrapper.field_mut(0).expect("a transparent wrapper holds one field"))
}

fn is_transparent(value: &dyn PartialReflect) -> bool {
    matches!(
        value.get_represented_type_info(),
        Some(TypeInfo::TupleStruct(info)) if info.has_attribute::<Transparent>()
    )
}

/// The name of the variant `value` is in, if it's an enum.
#[must_use]
pub fn variant(value: &dyn PartialReflect) -> Option<&str> {
    match value.reflect_ref() {
        ReflectRef::Enum(value) => Some(value.variant_name()),
        _ => None,
    }
}

/// What a value is made of.
pub enum Shape<'v> {
    /// A struct's fields, or a struct variant's.
    Fields(Vec<FieldOf<'v>>),
    /// A list's, array's or tuple's items, or a tuple variant's.
    Items(Vec<&'v dyn PartialReflect>),
    /// A number, a string, a unit variant: nothing below it a path can step to.
    Leaf,
}

/// A field, under both its names.
pub struct FieldOf<'v> {
    /// As the type calls it, and as a path does.
    pub name: Cow<'v, str>,
    /// As the text does.
    pub written: Cow<'v, str>,
    pub value: &'v dyn PartialReflect,
}

/// What `value` is made of, past wrappers the text doesn't write.
#[must_use]
pub fn shape(value: &dyn PartialReflect) -> Shape<'_> {
    let value = through(value);
    match value.reflect_ref() {
        ReflectRef::Struct(fields) => Shape::Fields(
            (0..fields.field_len())
                .filter_map(|at| {
                    let name = fields.name_at(at)?;
                    let written = written_name(value, name);
                    Some(FieldOf {
                        name: Cow::Borrowed(name),
                        written: Cow::Borrowed(written),
                        value: through(fields.field_at(at)?),
                    })
                })
                .collect(),
        ),
        ReflectRef::TupleStruct(items) => Shape::Items(items.iter_fields().map(through).collect()),
        ReflectRef::Tuple(items) => Shape::Items(items.iter_fields().map(through).collect()),
        ReflectRef::List(items) => Shape::Items(items.iter().map(through).collect()),
        ReflectRef::Array(items) => Shape::Items(items.iter().map(through).collect()),
        ReflectRef::Enum(variant) => match variant.variant_type() {
            VariantType::Struct => Shape::Fields(
                variant
                    .iter_fields()
                    .filter_map(|field| {
                        let name = field.name()?;
                        Some(FieldOf {
                            name: Cow::Borrowed(name),
                            written: Cow::Borrowed(name),
                            value: through(field.value()),
                        })
                    })
                    .collect(),
            ),
            VariantType::Tuple => {
                Shape::Items(variant.iter_fields().map(|field| through(field.value())).collect())
            }
            VariantType::Unit => Shape::Leaf,
        },
        _ => Shape::Leaf,
    }
}

/// What the text calls a struct's field.
fn written_name<'n>(value: &dyn PartialReflect, name: &'n str) -> &'n str {
    if let Some(TypeInfo::Struct(info)) = value.get_represented_type_info()
        && let Some(renamed) = info.field(name).and_then(|field| field.get_attribute::<SerdeName>())
    {
        return renamed.0;
    }
    name
}

/// An entry's name, if it has one: a struct with a `name` that's a string.
#[must_use]
pub fn name_of(entry: &dyn PartialReflect) -> Option<&str> {
    let ReflectRef::Struct(fields) = through(entry).reflect_ref() else { return None };
    fields.field("name")?.try_downcast_ref::<String>().map(String::as_str)
}

/// The place of the one entry called `key`. `None` if none is, or more than one.
fn place_of_key(list: &dyn List, key: &str) -> Option<usize> {
    let mut found = None;
    for (at, entry) in list.iter().enumerate() {
        if name_of(entry) == Some(key) {
            if found.is_some() {
                return None;
            }
            found = Some(at);
        }
    }
    found
}

/// How a path calls the entry at `at`: by its name if no other has it, or else by its place.
#[must_use]
pub fn step_to_entry(list: &dyn List, at: usize) -> Step {
    match list.get(at).and_then(name_of) {
        Some(name) if place_of_key(list, name) == Some(at) => Step::Key(name.to_owned()),
        _ => Step::Index(at),
    }
}

/// One step down from `value`.
#[must_use]
pub fn child<'v>(value: &'v dyn PartialReflect, step: &Step) -> Option<&'v dyn PartialReflect> {
    let value = through(value);
    let found = match (value.reflect_ref(), step) {
        (ReflectRef::Struct(fields), Step::Field(name)) => fields.field(name),
        (ReflectRef::TupleStruct(items), Step::Index(at)) => items.field(*at),
        (ReflectRef::Tuple(items), Step::Index(at)) => items.field(*at),
        (ReflectRef::List(items), Step::Index(at)) => items.get(*at),
        (ReflectRef::List(items), Step::Key(key)) => items.get(place_of_key(items, key)?),
        (ReflectRef::Array(items), Step::Index(at)) => items.get(*at),
        (ReflectRef::Enum(variant), Step::Some) if is_option(value) => variant.field_at(0),
        (ReflectRef::Enum(variant), Step::Field(name)) => variant.field(name),
        (ReflectRef::Enum(variant), Step::Index(at))
            if !is_option(value) && variant.variant_type() == VariantType::Tuple =>
        {
            variant.field_at(*at)
        }
        _ => None,
    };
    found.map(through)
}

fn child_mut<'v>(value: &'v mut dyn PartialReflect, step: &Step) -> Option<&'v mut dyn PartialReflect> {
    let value = through_mut(value);
    let option = is_option(value);
    let found = match (value.reflect_mut(), step) {
        (ReflectMut::Struct(fields), Step::Field(name)) => fields.field_mut(name),
        (ReflectMut::TupleStruct(items), Step::Index(at)) => items.field_mut(*at),
        (ReflectMut::Tuple(items), Step::Index(at)) => items.field_mut(*at),
        (ReflectMut::List(items), Step::Index(at)) => items.get_mut(*at),
        (ReflectMut::List(items), Step::Key(key)) => {
            let at = place_of_key(items, key)?;
            items.get_mut(at)
        }
        (ReflectMut::Array(items), Step::Index(at)) => items.get_mut(*at),
        (ReflectMut::Enum(variant), Step::Some) if option => variant.field_at_mut(0),
        (ReflectMut::Enum(variant), Step::Field(name)) => variant.field_mut(name),
        (ReflectMut::Enum(variant), Step::Index(at))
            if !option && variant.variant_type() == VariantType::Tuple =>
        {
            variant.field_at_mut(*at)
        }
        _ => None,
    };
    found.map(through_mut)
}

/// What's at `path`.
pub fn get<'v>(
    root: &'v dyn PartialReflect,
    path: &PropertyPath,
) -> Result<&'v dyn PartialReflect, NoSuchPath> {
    let mut at = root;
    for (taken, step) in path.steps().iter().enumerate() {
        at = child(at, step).ok_or_else(|| no_such_path(path, taken))?;
    }
    Ok(at)
}

/// `path` with each entry's name turned into its place, as it is in `root` now. A value's walk uses this
/// form, because an edit can change the very name its path calls the entry by, and the same path then finds
/// nothing in the value it made.
pub fn positional(root: &dyn PartialReflect, path: &PropertyPath) -> Result<PropertyPath, NoSuchPath> {
    let mut at = through(root);
    let mut steps = path.prefix(0);
    for (taken, step) in path.steps().iter().enumerate() {
        let next = child(at, step).ok_or_else(|| no_such_path(path, taken))?;
        let plain = match (step, at.reflect_ref()) {
            (Step::Key(key), ReflectRef::List(items)) => {
                Step::Index(place_of_key(items, key).ok_or_else(|| no_such_path(path, taken))?)
            }
            _ => step.clone(),
        };
        steps = steps.join(plain);
        at = next;
    }
    Ok(steps)
}

/// Puts `value` in `target`'s place, whole. Not `try_apply`: applying a shorter list leaves the old list's
/// tail where it was, at any depth, and the value then isn't the one asked for.
pub fn assign(
    target: &mut dyn PartialReflect,
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
) -> Result<(), String> {
    let info = target.get_represented_type_info().ok_or("it has no type")?;
    let built = registry
        .get_type_data::<ReflectFromReflect>(info.type_id())
        .ok_or_else(|| format!("`{}` can't be built from a value", info.type_path()))?
        .from_reflect(value)
        .ok_or_else(|| format!("that isn't a `{}`", info.type_path()))?;
    let target = target.try_as_reflect_mut().ok_or("it isn't a concrete value")?;
    target.set(built).map_err(|_| "it holds something else".to_owned())
}

/// What's at `path`, to change.
pub fn get_mut<'v>(
    root: &'v mut dyn PartialReflect,
    path: &PropertyPath,
) -> Result<&'v mut dyn PartialReflect, NoSuchPath> {
    let mut at = root;
    for (taken, step) in path.steps().iter().enumerate() {
        at = child_mut(at, step).ok_or_else(|| no_such_path(path, taken))?;
    }
    Ok(at)
}

fn no_such_path(path: &PropertyPath, taken: usize) -> NoSuchPath {
    NoSuchPath { path: path.to_string(), stopped_at: path.prefix(taken).to_string() }
}

/// The text of what an `Option` holds: inside its `Some(..)`, or the same text if the file left that out.
#[must_use]
pub fn inside_some<'n>(node: &'n Node, source: &str) -> &'n Node {
    match node.items() {
        [only] if is_named(node.text(source), "Some") => only,
        _ => node,
    }
}

fn is_named(text: &str, name: &str) -> bool {
    leading_name(text) == Some(name)
}

/// The name a value's text starts with, if it starts with one.
fn leading_name(text: &str) -> Option<&str> {
    let end =
        text.bytes().position(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_')).unwrap_or(text.len());
    (end > 0 && !text.as_bytes()[0].is_ascii_digit()).then(|| &text[..end])
}

/// The text of the field a struct's value calls `written`, positionally if the text is a tuple.
fn field_text<'n>(node: &'n Node, value: &dyn PartialReflect, written: &str) -> Option<&'n Node> {
    match &node.kind {
        Kind::Struct(_) => node.field(written),
        Kind::Seq(items) => {
            let Shape::Fields(fields) = shape(value) else { return None };
            items.get(fields.iter().position(|field| field.written == written)?)
        }
        _ => None,
    }
}

/// How far down `path` the text goes: the deepest node written, and how many steps it took. A short count
/// means the next field is left out of the file, its value being its default.
pub fn place<'n>(
    root: &dyn PartialReflect,
    tree: &'n Node,
    source: &str,
    path: &PropertyPath,
) -> Result<(&'n Node, usize), NoSuchPath> {
    let mut value = through(root);
    let mut node = tree;
    for (taken, step) in path.steps().iter().enumerate() {
        let next = child(value, step).ok_or_else(|| no_such_path(path, taken))?;
        let found = match step {
            Step::Some => Some(inside_some(node, source)),
            Step::Field(name) => {
                let written = match value.reflect_ref() {
                    ReflectRef::Struct(_) => written_name(value, name),
                    _ => name.as_str(),
                };
                field_text(node, value, written)
            }
            Step::Index(at) => node.items().get(*at),
            Step::Key(key) => {
                let ReflectRef::List(list) = value.reflect_ref() else {
                    return Err(no_such_path(path, taken));
                };
                let at = place_of_key(list, key).ok_or_else(|| no_such_path(path, taken))?;
                node.items().get(at)
            }
        };
        match found {
            Some(found) => {
                node = found;
                value = next;
            }
            None => return Ok((node, taken)),
        }
    }
    Ok((node, path.steps().len()))
}

/// What a property is, for an inspector to draw and for a fuzz to change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyKind {
    /// A number, a string, a flag or an opaque value such as an asset ID.
    Leaf,
    /// An enum: its variant is the property, and its fields come after it.
    Enum,
    /// An `Option`: whether it holds anything is the property, and what it holds comes after it.
    Option,
    /// A list: its entries are the property, by insertion and removal.
    List,
}

/// A property of a document, and where the file writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub path: PropertyPath,
    pub kind: PropertyKind,
    /// Its bytes, or `None` if the file leaves it out and it holds its default.
    pub written: Option<Range<usize>>,
}

/// Every property of a document, in the order it declares them: those the file leaves out too.
#[must_use]
pub fn properties(root: &dyn PartialReflect, tree: &Node, source: &str) -> Vec<Property> {
    let mut found = Vec::new();
    visit(root, Some(tree), source, &PropertyPath::root(), &mut found);
    found
}

/// The properties at `path` and under it.
pub fn properties_under(
    root: &dyn PartialReflect,
    tree: &Node,
    source: &str,
    path: &PropertyPath,
) -> Result<Vec<Property>, NoSuchPath> {
    let value = get(root, path)?;
    let (node, taken) = place(root, tree, source, &positional(root, path)?)?;
    let mut found = Vec::new();
    visit(value, (taken == path.steps().len()).then_some(node), source, path, &mut found);
    Ok(found)
}

fn visit(
    value: &dyn PartialReflect,
    node: Option<&Node>,
    source: &str,
    path: &PropertyPath,
    out: &mut Vec<Property>,
) {
    let value = through(value);
    let mut property = |kind| {
        out.push(Property { path: path.clone(), kind, written: node.map(|node| node.span.clone()) });
    };
    if is_option(value) {
        property(PropertyKind::Option);
        if let Some(inner) = child(value, &Step::Some) {
            visit(inner, node.map(|node| inside_some(node, source)), source, &path.join(Step::Some), out);
        }
        return;
    }
    let is_enum = variant(value).is_some();
    match value.reflect_ref() {
        ReflectRef::List(list) => {
            property(PropertyKind::List);
            for (at, entry) in list.iter().enumerate() {
                let item = node.and_then(|node| node.items().get(at));
                visit(entry, item, source, &path.join(step_to_entry(list, at)), out);
            }
            return;
        }
        ReflectRef::Enum(_) => property(PropertyKind::Enum),
        _ => {}
    }
    match shape(value) {
        Shape::Fields(fields) => {
            for field in &fields {
                let text = node.and_then(|node| field_text(node, value, &field.written));
                visit(field.value, text, source, &path.join(Step::Field(field.name.to_string())), out);
            }
        }
        Shape::Items(items) => {
            for (at, item) in items.into_iter().enumerate() {
                let text = node.and_then(|node| node.items().get(at));
                visit(item, text, source, &path.join(Step::Index(at)), out);
            }
        }
        Shape::Leaf if !is_enum => out.push(Property {
            path: path.clone(),
            kind: PropertyKind::Leaf,
            written: node.map(|node| node.span.clone()),
        }),
        Shape::Leaf => {}
    }
}

/// Holds the text to the value: every value written says the same in both, every field written is one the
/// type has under that name, and every list is as long. So a rule of [`place`]'s that's wrong for a type
/// fails here with the path it failed at, before any edit could land in the wrong bytes.
pub fn agreement(root: &dyn PartialReflect, tree: &Node, source: &str) -> Result<(), String> {
    agree(root, tree, source, &PropertyPath::root())
}

fn agree(value: &dyn PartialReflect, node: &Node, source: &str, path: &PropertyPath) -> Result<(), String> {
    let value = through(value);
    let differ = |what: String| Err(format!("at `{path}`: {what}"));
    if is_option(value) {
        return match child(value, &Step::Some) {
            Some(inner) => agree(inner, inside_some(node, source), source, &path.join(Step::Some)),
            None if node.text(source) == "None" => Ok(()),
            None => differ(format!("the value is `None`, and the text `{}`", node.text(source))),
        };
    }
    if let Some(variant) = variant(value)
        && !is_named(node.text(source), variant)
    {
        return differ(format!("the value is `{variant}`, and the text `{}`", brief(node.text(source))));
    }
    match shape(value) {
        Shape::Fields(fields) => match &node.kind {
            Kind::Struct(written) => {
                for field in written {
                    if !fields.iter().any(|of| of.written == field.name) {
                        return differ(format!("the text has `{}`, and the type no such field", field.name));
                    }
                }
                for field in &fields {
                    if let Some(text) = node.field(&field.written) {
                        let next = path.join(Step::Field(field.name.to_string()));
                        agree(field.value, text, source, &next)?;
                    }
                }
                Ok(())
            }
            // `()`: every field left out.
            Kind::Seq(items) if items.is_empty() => Ok(()),
            // A struct that serde writes as a tuple, such as `Vec3`.
            Kind::Seq(items) if items.len() == fields.len() => {
                for (field, text) in fields.iter().zip(items) {
                    let next = path.join(Step::Field(field.name.to_string()));
                    agree(field.value, text, source, &next)?;
                }
                Ok(())
            }
            _ => differ(format!("the value has fields, and the text is `{}`", brief(node.text(source)))),
        },
        Shape::Items(items) => {
            if node.items().len() != items.len() {
                return differ(format!(
                    "the value has {} items, and the text {}",
                    items.len(),
                    node.items().len()
                ));
            }
            for (at, (item, text)) in items.into_iter().zip(node.items()).enumerate() {
                agree(item, text, source, &path.join(Step::Index(at)))?;
            }
            Ok(())
        }
        Shape::Leaf if variant(value).is_some() => Ok(()),
        Shape::Leaf => match leaf_agrees(value, node.text(source)) {
            true => Ok(()),
            false => differ(format!("the text `{}` doesn't read as the value", brief(node.text(source)))),
        },
    }
}

fn brief(text: &str) -> &str {
    let mut end = text.len().min(60);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Whether a number, string or flag written as `text` reads as `value`. Anything else goes by its own type,
/// which [`crate::document`]'s parse has read already.
fn leaf_agrees(value: &dyn PartialReflect, text: &str) -> bool {
    macro_rules! read_as {
        ($($ty:ty),*) => {
            $(if let Some(value) = value.try_downcast_ref::<$ty>() {
                return ron::from_str::<$ty>(text).is_ok_and(|read| format!("{read:?}") == format!("{value:?}"));
            })*
        };
    }
    read_as!(f32, f64, u8, u16, u32, u64, usize, i8, i16, i32, i64, bool, String);
    true
}
