//! What the tests share: the real documents, opened, and how a fuzz changes a property.

#![allow(dead_code, reason = "each test binary uses its own part of this")]

use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy_reflect::{
    FromReflect, GetTypeRegistration, PartialReflect, Reflect, TypeInfo, TypeRegistry, Typed,
};
use sapphire_assets::AssetId;
use sapphire_document::blank::{blank, blank_variant};
use sapphire_document::walk::{self, PropertyKind};
use sapphire_document::{Document, Property, Schema, registry_of};

pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).expect("tools/<crate>").to_owned()
}

/// A file of the repository, as text.
pub fn read(relative: &str) -> String {
    std::fs::read_to_string(repository_root().join(relative)).expect(relative)
}

/// `relative`, open.
pub fn open<T>(relative: &str, schema: Schema<T>) -> Document<T>
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    Document::open(read(relative), schema, Arc::new(registry_of::<T>()))
}

/// splitmix64: enough of a generator for a fuzz, and the same run every time.
pub struct Random(pub u64);

impl Random {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }

    pub fn below(&mut self, count: usize) -> usize {
        (self.next() % count as u64) as usize
    }
}

/// A new value for a property. The first choices are the hard spellings: a whole float, one that takes all
/// its digits, one small enough to want an exponent, a negative zero, a string that needs escapes.
pub fn another_value(
    value: &dyn PartialReflect,
    kind: PropertyKind,
    registry: &TypeRegistry,
    choice: usize,
) -> Option<Box<dyn PartialReflect>> {
    let info = value.get_represented_type_info()?;
    match kind {
        PropertyKind::List => None,
        PropertyKind::Option if walk::is_none(value) => some_of_blank(info, registry),
        PropertyKind::Option => blank_variant(info, "None", registry).ok(),
        PropertyKind::Enum => {
            let TypeInfo::Enum(variants) = info else { return None };
            let current = walk::variant(value)?;
            let others: Vec<_> = variants.iter().filter(|variant| variant.name() != current).collect();
            let picked = others.get(choice % others.len().max(1))?;
            blank_variant(info, picked.name(), registry).ok()
        }
        PropertyKind::Leaf => another_leaf(value, choice),
    }
}

fn some_of_blank(info: &'static TypeInfo, registry: &TypeRegistry) -> Option<Box<dyn PartialReflect>> {
    let TypeInfo::Enum(option) = info else { return None };
    let some = option.variant("Some")?.as_tuple_variant().ok()?;
    let inner = registry.get_type_info(some.field_at(0)?.ty().id())?;
    let inner = if inner.type_id() == TypeId::of::<AssetId>() {
        Box::new(AssetId::from_bytes([7; 16]))
    } else {
        blank(inner, registry).ok()?
    };
    let mut fields = bevy_reflect::tuple::DynamicTuple::default();
    fields.insert_boxed(inner);
    let mut value =
        bevy_reflect::enums::DynamicEnum::new("Some", bevy_reflect::enums::DynamicVariant::Tuple(fields));
    value.set_represented_type(Some(info));
    Some(Box::new(value))
}

fn another_leaf(value: &dyn PartialReflect, choice: usize) -> Option<Box<dyn PartialReflect>> {
    macro_rules! whole {
        ($($ty:ty),*) => {
            $(if let Some(value) = value.try_downcast_ref::<$ty>() {
                let choices = [value.wrapping_add(1), 0, 7, value.wrapping_add(25)];
                return Some(Box::new(choices[choice % choices.len()]));
            })*
        };
    }
    whole!(u8, u16, u32, u64, usize, i8, i16, i32, i64);
    if let Some(value) = value.try_downcast_ref::<f32>() {
        let choices = [1.0, 0.1, 1e-5, -0.0, 123_456.7, 0.333_333_34, value + 0.25, 1e20];
        return Some(Box::new(choices[choice % choices.len()]));
    }
    if let Some(value) = value.try_downcast_ref::<f64>() {
        let choices = [1.0, 0.1, 1e-300, value + 0.25];
        return Some(Box::new(choices[choice % choices.len()]));
    }
    if let Some(value) = value.try_downcast_ref::<bool>() {
        return Some(Box::new(!value));
    }
    if let Some(value) = value.try_downcast_ref::<AssetId>() {
        let other = if *value == AssetId::from_bytes([7; 16]) { [8; 16] } else { [7; 16] };
        return Some(Box::new(AssetId::from_bytes(other)));
    }
    if value.try_downcast_ref::<String>().is_some() {
        let choices = ["renamed", "with \"quotes\", a \\ and\na newline", "héllo", "// not a comment"];
        return Some(Box::new(choices[choice % choices.len()].to_owned()));
    }
    None
}

/// What the fuzz came to, for its report.
#[derive(Debug, Default)]
pub struct Tally {
    pub edits: usize,
    pub unchanged: usize,
    pub refused_by_document: usize,
    pub wrote_absent: usize,
    pub rewrote_whole: usize,
    pub skipped: usize,
}

/// The value at `property`, in `document`.
pub fn value_of<'d, T>(document: &'d Document<T>, property: &Property) -> &'d dyn PartialReflect
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let root = document.value().expect("a document that reads").as_partial_reflect();
    walk::get(root, &property.path).expect("a path the walker gave")
}
