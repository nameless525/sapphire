//! A new value of a type, from its reflection alone: what an inspector puts in an `Option` that's turned
//! on, an entry that's added, or a variant that's picked, for a person to fill in.
//!
//! Zeros, empties and first variants. It's a place to start from, and nothing says the document's rules
//! will take it: those show on it like on any other edit.

use bevy_reflect::array::DynamicArray;
use bevy_reflect::enums::{DynamicEnum, DynamicVariant, VariantInfo};
use bevy_reflect::list::DynamicList;
use bevy_reflect::std_traits::ReflectDefault;
use bevy_reflect::structs::DynamicStruct;
use bevy_reflect::tuple::DynamicTuple;
use bevy_reflect::tuple_struct::DynamicTupleStruct;
use bevy_reflect::{PartialReflect, ReflectFromReflect, TypeInfo, TypeRegistry};

/// A type [`blank`] can't make.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("can't make a new `{0}`: it isn't registered, or has no default and no shape to fill in")]
pub struct NoBlank(pub String);

/// A new value of the type of `info`, concrete if the type is registered as it should be.
pub fn blank(info: &'static TypeInfo, registry: &TypeRegistry) -> Result<Box<dyn PartialReflect>, NoBlank> {
    let no_blank = || NoBlank(info.type_path().to_owned());
    let registration = registry.get(info.type_id()).ok_or_else(no_blank)?;
    if let Some(default) = registration.data::<ReflectDefault>() {
        return Ok(default.default().into_partial_reflect());
    }
    let dynamic: Box<dyn PartialReflect> = match info {
        TypeInfo::Struct(fields) => {
            let mut value = DynamicStruct::default();
            for field in fields.iter() {
                value.insert_boxed(field.name(), blank_of(field.ty().id(), registry)?);
            }
            value.set_represented_type(Some(info));
            Box::new(value)
        }
        TypeInfo::TupleStruct(fields) => {
            let mut value = DynamicTupleStruct::default();
            for field in fields.iter() {
                value.insert_boxed(blank_of(field.ty().id(), registry)?);
            }
            value.set_represented_type(Some(info));
            Box::new(value)
        }
        TypeInfo::Tuple(fields) => {
            let mut value = DynamicTuple::default();
            for field in fields.iter() {
                value.insert_boxed(blank_of(field.ty().id(), registry)?);
            }
            value.set_represented_type(Some(info));
            Box::new(value)
        }
        TypeInfo::Array(array) => {
            let items = (0..array.capacity())
                .map(|_| blank_of(array.item_ty().id(), registry))
                .collect::<Result<Vec<_>, _>>()?;
            let mut value = DynamicArray::new(items.into_boxed_slice());
            value.set_represented_type(Some(info));
            Box::new(value)
        }
        TypeInfo::List(_) => {
            let mut value = DynamicList::default();
            value.set_represented_type(Some(info));
            Box::new(value)
        }
        TypeInfo::Enum(variants) => {
            let first = variants.iter().next().ok_or_else(no_blank)?;
            Box::new(enum_value(info, first, registry)?)
        }
        _ => return Err(no_blank()),
    };
    // An array or a tuple isn't registered with `FromReflect`. A dynamic one is as good, since what holds it
    // makes the real one of it.
    let Some(from) = registration.data::<ReflectFromReflect>() else {
        return if matches!(info, TypeInfo::Array(_) | TypeInfo::Tuple(_)) {
            Ok(dynamic)
        } else {
            Err(no_blank())
        };
    };
    from.from_reflect(&*dynamic).map(|value| value.into_partial_reflect()).ok_or_else(no_blank)
}

fn blank_of(id: core::any::TypeId, registry: &TypeRegistry) -> Result<Box<dyn PartialReflect>, NoBlank> {
    let info = registry.get_type_info(id).ok_or_else(|| NoBlank(format!("{id:?}")))?;
    blank(info, registry)
}

/// `info`'s variant `name`, with its fields blank: what picking a variant makes. `None` if it has no such
/// variant.
pub fn blank_variant(
    info: &'static TypeInfo,
    name: &str,
    registry: &TypeRegistry,
) -> Result<Box<dyn PartialReflect>, NoBlank> {
    let no_blank = || NoBlank(format!("{}::{name}", info.type_path()));
    let TypeInfo::Enum(variants) = info else { return Err(no_blank()) };
    let variant = variants.iter().find(|variant| variant.name() == name).ok_or_else(no_blank)?;
    let dynamic = enum_value(info, variant, registry)?;
    registry
        .get(info.type_id())
        .and_then(|registration| registration.data::<ReflectFromReflect>())
        .and_then(|from| from.from_reflect(&dynamic))
        .map(|value| value.into_partial_reflect())
        .ok_or_else(no_blank)
}

fn enum_value(
    info: &'static TypeInfo,
    variant: &VariantInfo,
    registry: &TypeRegistry,
) -> Result<DynamicEnum, NoBlank> {
    let body = match variant {
        VariantInfo::Unit(_) => DynamicVariant::Unit,
        VariantInfo::Tuple(fields) => {
            let mut value = DynamicTuple::default();
            for field in fields.iter() {
                value.insert_boxed(blank_of(field.ty().id(), registry)?);
            }
            DynamicVariant::Tuple(value)
        }
        VariantInfo::Struct(fields) => {
            let mut value = DynamicStruct::default();
            for field in fields.iter() {
                value.insert_boxed(field.name(), blank_of(field.ty().id(), registry)?);
            }
            DynamicVariant::Struct(value)
        }
    };
    let mut value = DynamicEnum::new(variant.name(), body);
    value.set_represented_type(Some(info));
    Ok(value)
}
