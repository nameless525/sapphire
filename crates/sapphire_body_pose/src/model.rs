//! Local-to-model: from each joint's transform relative to its parent to its transform in the
//! model's space, then to the skinning palette the renderer reads.

use sapphire_assets::mesh::SkinBind;
use sapphire_assets::skeleton::{ArchivedSkeleton, NO_PARENT};
use sapphire_math::{Affine3A, Mat4, Transform};

/// `locals` in `skeleton`'s model space: `model[joint]` is its parent's `model` times its local
/// transform, walked once, parents first. Allocates nothing.
///
/// # Panics
///
/// If `model` is shorter than `locals`, or `locals` is longer than the skeleton.
pub fn local_to_model(skeleton: &ArchivedSkeleton, locals: &[Transform], model: &mut [Affine3A]) {
    walk(skeleton.parents.iter().map(|parent| parent.to_native()), locals, model);
}

pub(crate) fn walk(parents: impl Iterator<Item = u16>, locals: &[Transform], model: &mut [Affine3A]) {
    for (joint, (parent, local)) in parents.zip(locals).enumerate() {
        model[joint] = model_of(parent, local, model);
    }
}

/// One joint's model transform, from its parent's. The product with the identity is kept at a
/// root: it's what the held pose's goldens were drawn with (it can flip the sign of a zero).
#[doc(hidden)]
#[inline]
pub fn model_of(parent: u16, local: &Transform, model: &[Affine3A]) -> Affine3A {
    let parent = if parent == NO_PARENT { Affine3A::IDENTITY } else { model[usize::from(parent)] };
    parent * local.to_affine3a()
}

/// One palette entry's source: a joint and its inverse bind matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bind {
    pub joint: u16,
    pub inverse_bind: Affine3A,
}

impl Bind {
    /// `binds` as the palette reads them.
    #[must_use]
    pub fn from_skin(binds: &[SkinBind]) -> Vec<Self> {
        binds
            .iter()
            .map(|bind| Self {
                joint: bind.joint,
                inverse_bind: Affine3A::from_mat4(Mat4::from_cols_array_2d(&bind.inverse_bind_matrix)),
            })
            .collect()
    }
}

/// The skinning palette of `model` for `binds`, as `sapphire_render::SkinnedInstance` takes it:
/// each entry's `model_from_joint × inverse_bind`. Allocates nothing.
///
/// # Panics
///
/// If a bind's joint is past the end of `model`.
pub fn palette(model: &[Affine3A], binds: &[Bind], out: &mut [Affine3A]) {
    for (entry, bind) in out.iter_mut().zip(binds) {
        *entry = model[usize::from(bind.joint)] * bind.inverse_bind;
    }
}
