//! Bringing streamed textures to the top the stage wants of them, a level at a time (M5 M4, m5-plan §3
//! "Mip streaming").
//!
//! A large texture is cooked as a tail and a level asset for each level above it
//! (`sapphire_assets::texture`'s "Tails"). What loads a texture as a dependency, a material, loads its tail
//! alone, so a material is ready, and draws, once its textures' tails are. [`TextureLevels`] is what the
//! stage then does for each texture on the renderer:
//!
//! - **What it wants.** A top for each texture: the one asked for ([`crate::Content::want_texture_top`],
//!   M5's seam) or, until something asks, level 0, the whole texture, while a material on the renderer
//!   binds it; its tail's own top once none does, so its levels go back and its tail can go too.
//! - **Raising.** It loads the levels between the texture's top and the wanted one through the asset
//!   server, holding their handles, keeps each as it arrives ([`sapphire_assets::Upload::keep`]), and,
//!   once the levels just above the texture's top have come, raises it by them, several at once as one new
//!   texture ([`Renderer::raise_texture`]), as the renderer's budget for the frame allows
//!   ([`Renderer::may_raise`]). The renderer rebinds the materials that hold it.
//! - **Lowering.** A wanted top under the texture's lowers it at once ([`Renderer::lower_texture`]), and
//!   the levels it dropped go back to the server ([`AssetServer::unload`]): a level is uploaded once, so
//!   one the server still held as uploaded would never come again. The rule both ways is that a level is
//!   on the GPU, or kept on its way there, exactly while this holds a handle to it, and a level the server
//!   lets go of takes the texture under it.
//! - **Hot reload.** A level's new version is written over the one on the GPU
//!   ([`Renderer::rewrite_texture_level`]); a tail's of the same shape is written in place by the stage, and
//!   one of another shape makes a new texture, whose levels are asked for again ([`TextureLevels::reshaped`]).
//!
//! It runs at the end of [`crate::Content::update`], after the frame's uploads, within the renderer's
//! budget for raising, which is the renderer's own: the asset server's upload budget admits a level's bytes
//! to the frame, and this one its copy into a new texture. A capture, which must see every texture whole,
//! finishes them before each frame ([`crate::Content::finish_texture_levels`]).

use std::collections::{BTreeMap, HashMap};

use sapphire_assets::texture_level::{ArchivedTextureLevel, TextureLevel};
use sapphire_assets::{AssetId, AssetServer, AssetSlot, CpuResident, Handle, LoadState, Upload, UploadError};
use sapphire_render::Renderer;
use sapphire_render::texture::GpuTexture;

use crate::LOG_TARGET;
use crate::content::Held;

/// What the stage wants of its streamed textures, and holds of their levels, on one renderer.
#[derive(Default)]
pub(crate) struct TextureLevels {
    textures: BTreeMap<AssetId, Wanted>,
    /// Tops asked for ([`crate::Content::want_texture_top`]), which outlast a renderer.
    asked: BTreeMap<AssetId, u32>,
    /// Levels let go of, which go back to the server once nothing here holds them again.
    give_back: Vec<(AssetId, u32)>,
}

/// One texture's levels.
#[derive(Default)]
struct Wanted {
    /// Materials on the renderer that bind the texture.
    users: u32,
    /// Levels asked of the server, held while they're on their way or on the GPU.
    handles: BTreeMap<u32, Handle<TextureLevel>>,
    /// Levels the server handed over, waiting for the levels between them and the texture's top.
    arrived: BTreeMap<u32, Arrived>,
    /// The version of each level above the tail on the GPU.
    written: BTreeMap<u32, Version>,
    /// The deepest level that failed to load or raise: the texture stays under it until its tail changes.
    failed: Option<u32>,
    /// The deepest level the server let go of while it was on the GPU: the texture goes under it.
    lost: Option<u32>,
}

/// Which version of a level the server handed over.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Version {
    slot: AssetSlot,
    generation: u32,
}

struct Arrived {
    version: Version,
    level: CpuResident<TextureLevel>,
}

impl Wanted {
    /// Whether a level is held here: asked for, arrived or on the GPU.
    fn holds(&self, index: u32) -> bool {
        self.handles.contains_key(&index)
            || self.arrived.contains_key(&index)
            || self.written.contains_key(&index)
    }

    /// Lets go of every level it holds or has asked for that `keep` refuses, for the server to have back.
    fn let_go(&mut self, texture: AssetId, keep: impl Fn(u32) -> bool, give_back: &mut Vec<(AssetId, u32)>) {
        let dropped = |index: &u32| !keep(*index);
        give_back.extend(self.handles.keys().copied().filter(dropped).map(|index| (texture, index)));
        give_back.extend(self.arrived.keys().copied().filter(dropped).map(|index| (texture, index)));
        self.handles.retain(|index, _| keep(*index));
        self.arrived.retain(|index, _| keep(*index));
    }
}

impl TextureLevels {
    /// For a new renderer, which holds nothing yet: the tops asked for stay asked for.
    pub(crate) fn for_new_renderer(&self) -> Self {
        Self { asked: self.asked.clone(), ..Self::default() }
    }

    /// A material on the renderer binds `texture`.
    pub(crate) fn bind(&mut self, texture: AssetId) {
        self.textures.entry(texture).or_default().users += 1;
    }

    /// A material that bound `texture` left the renderer, or binds others now.
    pub(crate) fn unbind(&mut self, texture: AssetId) {
        if let Some(wanted) = self.textures.get_mut(&texture) {
            wanted.users = wanted.users.saturating_sub(1);
        }
    }

    /// Asks for `texture` at `top`, or with `None` gives it back to the stage's choice.
    pub(crate) fn ask(&mut self, texture: AssetId, top: Option<u32>) {
        match top {
            Some(top) => self.asked.insert(texture, top),
            None => self.asked.remove(&texture),
        };
    }

    /// Takes `level`, which the server hands over in `upload`, for `texture`, the GPU texture it's a level
    /// of if that's on the renderer: a level the texture holds is written over if this is a new version,
    /// and any other is kept until the texture can rise to it.
    pub(crate) fn arrive(
        &mut self,
        upload: Upload<'_>,
        level: &ArchivedTextureLevel,
        texture: Option<&GpuTexture>,
        renderer: &mut Renderer,
    ) -> Result<(), UploadError> {
        let version = Version { slot: upload.slot(), generation: upload.generation() };
        let index = level.level();
        let wanted = self.textures.entry(level.texture()).or_default();
        if let Some(texture) = texture.filter(|texture| (texture.top()..texture.above()).contains(&index)) {
            // Sent again, for a renderer kept, it's the version written.
            if wanted.written.get(&index).is_some_and(|written| {
                written.slot == version.slot && written.generation >= version.generation
            }) {
                return Ok(());
            }
            renderer.rewrite_texture_level(texture, level)?;
            wanted.written.insert(index, version);
            return Ok(());
        }
        let level = upload.keep::<TextureLevel>().ok_or("a texture level that isn't one")?;
        wanted.arrived.insert(index, Arrived { version, level });
        Ok(())
    }

    /// The tail of `texture` was replaced by one of another shape (hot reload): the levels held for the
    /// old one go back, and are asked for again, as the new tail's levels.
    pub(crate) fn reshaped(&mut self, texture: AssetId) {
        if let Some(wanted) = self.textures.get_mut(&texture) {
            wanted.let_go(texture, |_| false, &mut self.give_back);
            wanted.written.clear();
            (wanted.failed, wanted.lost) = (None, None);
        }
    }

    /// The server unloaded asset `id` in `slot`: a texture's tail, which takes what's held of its levels
    /// with it, or a level, which takes the texture under it.
    pub(crate) fn unloaded(&mut self, id: AssetId, slot: AssetSlot) {
        if let Some(mut wanted) = self.textures.remove(&id) {
            wanted.let_go(id, |_| false, &mut self.give_back);
            // A material that binds it still counts, for when it comes again.
            if wanted.users > 0 {
                self.textures.insert(id, Wanted { users: wanted.users, ..Wanted::default() });
            }
            return;
        }
        for wanted in self.textures.values_mut() {
            wanted.handles.retain(|_, handle| handle.id() != id);
            wanted.arrived.retain(|_, arrived| arrived.version.slot != slot);
            let lost =
                wanted.written.iter().filter(|(_, written)| written.slot == slot).map(|(&index, _)| index);
            if let Some(index) = lost.max() {
                wanted.written.remove(&index);
                wanted.lost = wanted.lost.max(Some(index));
            }
        }
    }

    /// Keeps only what the server still holds, by `resident`: a renderer set aside while a new one was
    /// tried missed what the server unloaded meanwhile.
    pub(crate) fn take_stale(&mut self, resident: impl Fn(AssetId) -> bool) {
        for (&texture, wanted) in &mut self.textures {
            let gone: Vec<u32> = wanted
                .written
                .keys()
                .copied()
                .filter(|&index| !resident(TextureLevel::id(texture, index)))
                .collect();
            if let Some(&deepest) = gone.last() {
                wanted.lost = wanted.lost.max(Some(deepest));
            }
            for index in gone {
                wanted.written.remove(&index);
            }
            wanted.arrived.retain(|&index, _| resident(TextureLevel::id(texture, index)));
        }
    }

    /// Brings each texture in `textures` toward its wanted top: lowers it now, asks the server for the
    /// levels it lacks, and raises it by those that have come, within the renderer's budget for the frame
    /// unless `unlimited`. Gives back to the server what it let go of. Returns the bytes the textures hold
    /// now, less what they held.
    pub(crate) fn follow(
        &mut self,
        textures: &mut HashMap<AssetId, Held<GpuTexture>>,
        server: &mut AssetServer,
        renderer: &mut Renderer,
        unlimited: bool,
    ) -> i64 {
        let Self { textures: wants, asked, give_back } = self;
        let mut change = 0;
        wants.retain(|&id, wanted| {
            let Some(texture) = textures.get_mut(&id).map(|held| &mut held.object) else {
                // Its tail isn't on the renderer: what's held of its levels waits for it while a material
                // binds it.
                if wanted.users == 0 {
                    wanted.let_go(id, |_| false, give_back);
                }
                return wanted.users > 0;
            };
            let before = texture.bytes();
            let above = texture.above();
            let top = wanted_top(asked, id, wanted, above);
            let floor = top.max(wanted.lost.take().map_or(0, |lost| lost + 1)).min(above);
            if texture.top() < floor
                && let Err(error) = renderer.lower_texture(id, texture, floor)
            {
                tracing::warn!(target: LOG_TARGET, %id, %error, "a texture couldn't be lowered");
            }
            let on_gpu = texture.top();
            wanted.written.retain(|&index, _| index >= on_gpu);
            wanted.let_go(id, |index| (top..above).contains(&index), give_back);
            ask_levels(id, wanted, top, on_gpu, server, give_back);
            raise(id, wanted, texture, top, renderer, unlimited, give_back);
            change += texture.bytes() as i64 - before as i64;
            // Nothing binds it and nothing's held of it: it's at its tail, with nothing left to do.
            wanted.users > 0 || !wanted.handles.is_empty() || !wanted.arrived.is_empty()
        });
        for (texture, index) in std::mem::take(give_back) {
            // Let go of and asked for again since, it stays. One the server can't unload yet (loading, or
            // held elsewhere) comes back as an arrival nothing wants, or is found ready and never handed over
            // when it's asked for again, and goes back then.
            if !wants.get(&texture).is_some_and(|wanted| wanted.holds(index)) {
                server.unload(TextureLevel::id(texture, index));
            }
        }
        change
    }

    /// The levels a capture waits for: asked for and not yet come.
    pub(crate) fn waiting(&self) -> Vec<Handle<TextureLevel>> {
        let mut waiting = Vec::new();
        for wanted in self.textures.values() {
            for (index, handle) in &wanted.handles {
                if !wanted.arrived.contains_key(index) && !wanted.written.contains_key(index) {
                    waiting.push(handle.clone());
                }
            }
        }
        waiting
    }

    /// Whether every texture in `textures` is at its wanted top, or as near as the levels that failed let it.
    pub(crate) fn settled(&self, textures: &HashMap<AssetId, Held<GpuTexture>>) -> bool {
        self.textures.iter().all(|(&id, wanted)| {
            textures.get(&id).is_none_or(|held| {
                held.object.top() <= wanted_top(&self.asked, id, wanted, held.object.above())
            })
        })
    }
}

/// The top wanted of `texture`, with `above` levels above its tail: the one asked for, or the whole
/// texture, while a material binds it; its tail's once none does. A level that failed keeps it under.
fn wanted_top(asked: &BTreeMap<AssetId, u32>, texture: AssetId, wanted: &Wanted, above: u32) -> u32 {
    if wanted.users == 0 {
        return above;
    }
    let floor = wanted.failed.map_or(0, |failed| failed + 1);
    asked.get(&texture).copied().unwrap_or(0).max(floor).min(above)
}

/// Asks the server for `texture`'s levels from `top` to `on_gpu`, the texture's top, that aren't on their
/// way, the nearest first. A level that failed stops the texture under it; one the server says is ready but
/// never handed over here (it was uploaded for another renderer) goes back, to be asked for again.
fn ask_levels(
    texture: AssetId,
    wanted: &mut Wanted,
    top: u32,
    on_gpu: u32,
    server: &mut AssetServer,
    give_back: &mut Vec<(AssetId, u32)>,
) {
    for index in (top..on_gpu).rev() {
        if wanted.failed.is_some_and(|failed| index <= failed) {
            break;
        }
        if let Some(handle) = wanted.handles.get(&index) {
            match server.state(handle) {
                LoadState::Loading => {}
                LoadState::Ready if wanted.arrived.contains_key(&index) => {}
                LoadState::Ready => {
                    wanted.handles.remove(&index);
                    give_back.push((texture, index));
                }
                LoadState::Failed(error) => {
                    tracing::warn!(target: LOG_TARGET, %texture, level = index, %error, "a texture level failed");
                    wanted.handles.remove(&index);
                    wanted.failed = Some(index);
                    give_back.push((texture, index));
                }
            }
            continue;
        }
        match server.load::<TextureLevel>(TextureLevel::id(texture, index)) {
            Ok(handle) => {
                wanted.handles.insert(index, handle);
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %texture, level = index, %error, "a texture level can't load");
                wanted.failed = Some(index);
            }
        }
    }
}

/// Raises `texture`, asset `id`, by the levels that have come just above its top, no higher than `top`, if
/// the frame's budget allows.
fn raise(
    id: AssetId,
    wanted: &mut Wanted,
    texture: &mut GpuTexture,
    top: u32,
    renderer: &mut Renderer,
    unlimited: bool,
    give_back: &mut Vec<(AssetId, u32)>,
) {
    let on_gpu = texture.top();
    let Some(new_top) = (top..on_gpu).rev().take_while(|index| wanted.arrived.contains_key(index)).last()
    else {
        return;
    };
    let levels: Vec<&ArchivedTextureLevel> =
        (new_top..on_gpu).map(|index| wanted.arrived[&index].level.view()).collect();
    let bytes = levels.iter().map(|level| level.data().len() as u64).sum();
    if !unlimited && !renderer.may_raise(bytes) {
        return;
    }
    let raised = renderer.raise_texture(id, texture, &levels);
    drop(levels);
    match raised {
        Ok(()) => {
            for index in new_top..on_gpu {
                let arrived = wanted.arrived.remove(&index).expect("in the run");
                wanted.written.insert(index, arrived.version);
            }
        }
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %id, %error, "a texture couldn't be raised");
            wanted.failed = Some(on_gpu - 1);
            wanted.let_go(id, |index| index >= on_gpu, give_back);
        }
    }
}
