//! Handles to loaded assets: [`Handle`] (strong, ref-counted), [`WeakHandle`] (generational, `Copy`)
//! and [`UntypedHandle`] (strong, of any kind).

use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use sapphire_serialize::cooked::access_unchecked;

use crate::material::{ArchivedMaterial, Material};
use crate::mesh::{ArchivedMesh, Mesh};
use crate::rig::{ArchivedCharacterRig, CharacterRig};
use crate::texture::Texture;
use crate::{AssetId, AssetKind};

slotmap::new_key_type! {
    /// An asset's slot in its server: an index and a generation. A slot's generation changes
    /// when its asset is unloaded, so a key to an unloaded asset never finds another.
    pub(crate) struct SlotKey;
}

/// A kind of asset handles can be typed with: [`Mesh`], [`TextureAsset`], [`Material`],
/// [`CharacterRig`].
pub trait Asset: sealed::Sealed + 'static {
    const KIND: AssetKind;
    /// How a loaded asset of this kind is read: in place, borrowing its bytes.
    type View<'a>;

    /// Reads `bytes` as this kind.
    ///
    /// # Safety
    ///
    /// `bytes` must have passed [`AssetKind::references`] for [`Self::KIND`] (which runs this
    /// kind's full check) and be unchanged since. The asset server keeps decoded bytes immutable
    /// behind an `Arc` from that check on.
    #[doc(hidden)]
    unsafe fn view(bytes: &[u8]) -> Self::View<'_>;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::mesh::Mesh {}
    impl Sealed for crate::material::Material {}
    impl Sealed for crate::rig::CharacterRig {}
    impl Sealed for super::TextureAsset {}
}

impl Asset for CharacterRig {
    const KIND: AssetKind = AssetKind::Rig;
    type View<'a> = &'a ArchivedCharacterRig;

    unsafe fn view(bytes: &[u8]) -> &ArchivedCharacterRig {
        // SAFETY: as for meshes: `CharacterRig::read` accepted these bytes, unchanged since.
        unsafe { access_unchecked::<CharacterRig>(bytes) }
            .expect("the envelope was checked when the rig was decoded")
    }
}

impl Asset for Mesh {
    const KIND: AssetKind = AssetKind::Mesh;
    type View<'a> = &'a ArchivedMesh;

    unsafe fn view(bytes: &[u8]) -> &ArchivedMesh {
        // SAFETY: the caller guarantees `Mesh::read` accepted these bytes, unchanged since, and
        // that check includes the envelope and the full bytecheck walk.
        unsafe { access_unchecked::<Mesh>(bytes) }
            .expect("the envelope was checked when the mesh was decoded")
    }
}

impl Asset for Material {
    const KIND: AssetKind = AssetKind::Material;
    type View<'a> = &'a ArchivedMaterial;

    unsafe fn view(bytes: &[u8]) -> &ArchivedMaterial {
        // SAFETY: as for meshes: `Material::read` accepted these bytes, unchanged since.
        unsafe { access_unchecked::<Material>(bytes) }
            .expect("the envelope was checked when the material was decoded")
    }
}

/// The texture kind, for typed handles: a loaded texture reads as a [`Texture`], which borrows
/// its levels from the asset's bytes.
#[derive(Debug)]
pub enum TextureAsset {}

impl Asset for TextureAsset {
    const KIND: AssetKind = AssetKind::Texture;
    type View<'a> = Texture<'a>;

    unsafe fn view(bytes: &[u8]) -> Texture<'_> {
        // Parsing a KTX2 header and level index costs little, so it is simply repeated.
        Texture::from_ktx2(bytes).expect("the texture was checked when it was decoded")
    }
}

/// What every strong handle to one asset shares. The server keeps one reference itself, so the
/// asset has users while the count is above one.
#[derive(Debug)]
pub(crate) struct HandleInner {
    pub(crate) key: SlotKey,
    pub(crate) id: AssetId,
    pub(crate) kind: AssetKind,
}

/// A strong reference to an asset of any kind. While one exists, its asset stays loaded.
///
/// Cloning bumps an atomic count and dropping lowers it, so handles can be held and dropped on
/// any thread, inside the fixed step too: nothing allocates or sends a message (rule R6). The
/// asset server notices unused assets when it next updates.
#[derive(Clone)]
pub struct UntypedHandle {
    pub(crate) inner: Arc<HandleInner>,
}

impl UntypedHandle {
    pub fn id(&self) -> AssetId {
        self.inner.id
    }

    pub fn kind(&self) -> AssetKind {
        self.inner.kind
    }

    /// The handle typed as `T`, if its asset is of that kind.
    pub fn typed<T: Asset>(self) -> Option<Handle<T>> {
        (self.inner.kind == T::KIND).then_some(Handle { untyped: self, _kind: PhantomData })
    }

    pub(crate) fn key(&self) -> SlotKey {
        self.inner.key
    }
}

impl PartialEq for UntypedHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for UntypedHandle {}

impl fmt::Debug for UntypedHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UntypedHandle({} {})", self.inner.kind, self.inner.id)
    }
}

/// A strong, typed reference to an asset (spec §5.10): it keeps the asset loaded, and the asset
/// server reads it through it ([`AssetServer::get`](super::AssetServer::get)). One pointer, and
/// cheap to clone (see [`UntypedHandle`]). [`Self::downgrade`] gives the `Copy` [`WeakHandle`].
pub struct Handle<T: Asset> {
    untyped: UntypedHandle,
    _kind: PhantomData<fn() -> T>,
}

impl<T: Asset> Handle<T> {
    pub(crate) fn new(untyped: UntypedHandle) -> Self {
        debug_assert_eq!(untyped.kind(), T::KIND);
        Self { untyped, _kind: PhantomData }
    }

    pub fn id(&self) -> AssetId {
        self.untyped.id()
    }

    /// A weak handle to the same asset, which doesn't keep it loaded.
    pub fn downgrade(&self) -> WeakHandle<T> {
        WeakHandle { key: self.untyped.key(), _kind: PhantomData }
    }

    pub fn untyped(&self) -> &UntypedHandle {
        &self.untyped
    }

    pub fn into_untyped(self) -> UntypedHandle {
        self.untyped
    }

    pub(crate) fn key(&self) -> SlotKey {
        self.untyped.key()
    }
}

impl<T: Asset> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self { untyped: self.untyped.clone(), _kind: PhantomData }
    }
}

impl<T: Asset> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.untyped == other.untyped
    }
}

impl<T: Asset> Eq for Handle<T> {}

impl<T: Asset> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle<{}>({})", T::KIND, self.id())
    }
}

/// A weak, typed reference to an asset: an index and a generation, `Copy`, 8 bytes. It doesn't
/// keep its asset loaded; once the asset is unloaded it resolves to nothing, even after its slot
/// is reused ([`AssetServer::upgrade`](super::AssetServer::upgrade)).
pub struct WeakHandle<T: Asset> {
    pub(crate) key: SlotKey,
    _kind: PhantomData<fn() -> T>,
}

impl<T: Asset> Clone for WeakHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Asset> Copy for WeakHandle<T> {}

impl<T: Asset> PartialEq for WeakHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl<T: Asset> Eq for WeakHandle<T> {}

impl<T: Asset> fmt::Debug for WeakHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WeakHandle<{}>({:?})", T::KIND, self.key)
    }
}
