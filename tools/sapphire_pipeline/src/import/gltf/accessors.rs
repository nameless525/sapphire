//! What stands between a file and the `gltf` crate's accessor readers.
//!
//! `gltf::accessor::Iter::new` and the mesh and skin readers built on it trust an accessor's every
//! word: they hit `unreachable!` on a component type the specification rules out, assert that its
//! element size and stride fit the type they read, and compute its byte range in unchecked
//! arithmetic from 64-bit numbers in the file. The crate's validation checks none of this. So
//! every accessor the importer reads goes through [`check`] first, and an accessor that passes
//! reads exactly `count` elements without a panic. Accessors nothing reads (`TANGENT`, a second UV
//! set) aren't checked: a fault in them costs nothing.

use ::gltf::Accessor;
use ::gltf::accessor::{DataType, Dimensions};
use ::gltf::buffer::View;
use DataType::{F32, I8, I16, U8, U16, U32};

/// What an importer reads an accessor as.
pub(super) struct Kind {
    dimensions: &'static [Dimensions],
    /// Each allowed (component type, normalized).
    types: &'static [(DataType, bool)],
}

/// Float, or normalized unsigned integers: what glTF allows UVs, colors and weights.
const FLOAT_OR_UNORM: &[(DataType, bool)] = &[(F32, false), (U8, true), (U16, true)];

/// `POSITION`, `NORMAL` and morph targets' displacements. `KHR_mesh_quantization`, which allows
/// integers here, isn't supported.
pub(super) const FLOAT_VEC3: Kind = Kind { dimensions: &[Dimensions::Vec3], types: &[(F32, false)] };
pub(super) const TEX_COORD: Kind = Kind { dimensions: &[Dimensions::Vec2], types: FLOAT_OR_UNORM };
pub(super) const COLOR: Kind =
    Kind { dimensions: &[Dimensions::Vec3, Dimensions::Vec4], types: FLOAT_OR_UNORM };
pub(super) const INDICES: Kind =
    Kind { dimensions: &[Dimensions::Scalar], types: &[(U8, false), (U16, false), (U32, false)] };
pub(super) const JOINTS: Kind = Kind { dimensions: &[Dimensions::Vec4], types: &[(U8, false), (U16, false)] };
pub(super) const WEIGHTS: Kind = Kind { dimensions: &[Dimensions::Vec4], types: FLOAT_OR_UNORM };
pub(super) const INVERSE_BIND_MATRICES: Kind =
    Kind { dimensions: &[Dimensions::Mat4], types: &[(F32, false)] };
pub(super) const OUTLINE_NORMAL: Kind =
    Kind { dimensions: &[Dimensions::Vec3], types: &[(F32, false), (I16, true), (I8, true)] };
pub(super) const MATERIAL_ID: Kind =
    Kind { dimensions: &[Dimensions::Scalar], types: &[(F32, false), (U8, false), (U16, false)] };
/// Float or normalized integers: what glTF allows a rotation's keys and a morph weight's.
const FLOAT_OR_NORMALIZED: &[(DataType, bool)] =
    &[(F32, false), (I8, true), (U8, true), (I16, true), (U16, true)];
/// An animation sampler's `input`: key times.
pub(super) const ANIMATION_TIME: Kind = Kind { dimensions: &[Dimensions::Scalar], types: &[(F32, false)] };
/// An animation sampler's `output`, for translations and scales.
pub(super) const ANIMATION_VEC3: Kind = FLOAT_VEC3;
/// An animation sampler's `output`, for rotations.
pub(super) const ANIMATION_ROTATION: Kind =
    Kind { dimensions: &[Dimensions::Vec4], types: FLOAT_OR_NORMALIZED };
/// An animation sampler's `output`, for morph weights.
pub(super) const ANIMATION_WEIGHT: Kind =
    Kind { dimensions: &[Dimensions::Scalar], types: FLOAT_OR_NORMALIZED };

/// Why an accessor can't be read. The importer's errors say whose accessor it is.
#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum AccessorFault {
    #[error("is {dimensions:?} of {data_type:?} (normalized: {normalized}), which isn't supported")]
    Type { data_type: DataType, dimensions: Dimensions, normalized: bool },
    #[error("has no elements")]
    Empty,
    #[error("has {sparse} sparse elements for its {count}; it may have from one up to all of them")]
    SparseCount { sparse: usize, count: usize },
    #[error("sparse index {element} is {index}; indices must increase strictly and stay below {count}")]
    SparseIndex { element: usize, index: u32, count: usize },
    #[error("has a stride of {stride} bytes for elements of {size} bytes")]
    Stride { stride: usize, size: usize },
    #[error("reaches past the end of buffer view {view}")]
    PastView { view: usize },
    #[error("reads buffer view {view}, which reaches past the end of buffer {buffer}")]
    PastBuffer { view: usize, buffer: usize },
}

/// Fails unless the readers can read `accessor` as `kind` from `buffers`. See the module docs.
pub(super) fn check(accessor: &Accessor<'_>, kind: &Kind, buffers: &[Vec<u8>]) -> Result<(), AccessorFault> {
    let (data_type, dimensions, normalized) =
        (accessor.data_type(), accessor.dimensions(), accessor.normalized());
    if !kind.dimensions.contains(&dimensions) || !kind.types.contains(&(data_type, normalized)) {
        return Err(AccessorFault::Type { data_type, dimensions, normalized });
    }
    let (count, size) = (accessor.count(), accessor.size());
    if count == 0 {
        return Err(AccessorFault::Empty);
    }
    if let Some(view) = accessor.view() {
        check_span(&view, accessor.offset(), count, size, buffers)?;
    }
    if let Some(sparse) = accessor.sparse() {
        let sparse_count = sparse.count();
        if sparse_count == 0 || sparse_count > count {
            return Err(AccessorFault::SparseCount { sparse: sparse_count, count });
        }
        let indices = sparse.indices();
        check_span(&indices.view(), indices.offset(), sparse_count, indices.index_type().size(), buffers)?;
        let values = sparse.values();
        check_span(&values.view(), values.offset(), sparse_count, size, buffers)?;
        let view = indices.view();
        let index_size = indices.index_type().size();
        let stride = view.stride().unwrap_or(index_size);
        // Both spans are proven inside their actual buffers above, including these offsets
        // and strides. Match the pinned reader's byte stepping when checking the payload.
        let buffer = &buffers[view.buffer().index()];
        let start = view.offset() + indices.offset();
        let mut previous = None;
        for element in 0..sparse_count {
            let at = start + element * stride;
            let bytes = &buffer[at..at + index_size];
            let index = match index_size {
                1 => u32::from(bytes[0]),
                2 => u32::from(u16::from_le_bytes(bytes.try_into().expect("two index bytes"))),
                4 => u32::from_le_bytes(bytes.try_into().expect("four index bytes")),
                _ => unreachable!("glTF sparse indices are u8, u16 or u32"),
            };
            if index as usize >= count || previous.is_some_and(|previous| index <= previous) {
                return Err(AccessorFault::SparseIndex { element, index, count });
            }
            previous = Some(index);
        }
    }
    Ok(())
}

/// Fails unless `count` (at least one) elements of `size` bytes from `offset` lie inside `view`,
/// and `view` inside its buffer: the byte range the reader computes, without overflowing.
fn check_span(
    view: &View<'_>,
    offset: usize,
    count: usize,
    size: usize,
    buffers: &[Vec<u8>],
) -> Result<(), AccessorFault> {
    let stride = view.stride().unwrap_or(size);
    if stride < size {
        return Err(AccessorFault::Stride { stride, size });
    }
    let end = stride
        .checked_mul(count - 1)
        .and_then(|last| last.checked_add(size))
        .and_then(|span| span.checked_add(offset));
    if end.is_none_or(|end| end > view.length()) {
        return Err(AccessorFault::PastView { view: view.index() });
    }
    let buffer = view.buffer().index();
    let view_end = view.offset().checked_add(view.length());
    if view_end.is_none_or(|end| end > buffers.get(buffer).map_or(0, Vec::len)) {
        return Err(AccessorFault::PastBuffer { view: view.index(), buffer });
    }
    Ok(())
}
