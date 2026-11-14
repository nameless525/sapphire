//! Materials on the GPU (M1 A3): cooked [`ArchivedMaterial`]s in stable slots of their family's
//! parameter buffer, with their textures bound in group 3.
//!
//! - **One buffer per family**, an array of the family's block (`AnimeCharacterParams`,
//!   `EnvironmentParams`, …, `shaders/materials.wgsl`). The blocks differ in size by more than
//!   ten times, so one shared array would pad every slot to the largest; and a pipeline only
//!   ever reads its own family, so binding one buffer keeps the per-stage storage-buffer count
//!   unchanged (crate docs, "Binding budget").
//! - **Stable slots.** A [`MaterialHandle`] keeps its slot until it's freed, and a
//!   draw record names its material by that slot, so [`Materials::update`] rewrites one slot
//!   and touches nothing else: no draw-list rebuild, no other upload. That is the hot reload
//!   (A8) and live link (A9) path for a material edit.
//! - **A CPU mirror** of every family buffer holds what the GPU should, and slots written since
//!   the last frame are uploaded by the frame's uploads pass, through the staging belt, in runs
//!   of consecutive slots, like instances.
//! - **Group 3** holds the material's texture per slot, a white 1×1 fallback where none is
//!   bound (shaders skip those slots on the block's bound bits), and the three samplers. Each
//!   bound texture must carry the swizzle the material was cooked with, and slots of linear data
//!   (normal, control, mask and SDF maps) take no sRGB texture: decoding would bend the data.
//! - Meshes draw with the anime character, environment (M1 W4), fog card (M1 W3) and foliage
//!   (M1 W6) families, each through a scene group (group 2) of its own holding its buffer beside
//!   the instances (the fog cards' also the depth they fade against, foliage's the wind), rebuilt
//!   when the instance buffer or its family's buffer grows. The sky (W2) draws one material at a
//!   time and reads its block from the mirror ([`Materials::params`]), binding only its group 3.
//!
//! A raise or a lower of a texture (M5 M4) replaces it with a new object, and [`Materials::rebind`] makes
//! again the group 3 of every material binding it, which is why each keeps what its slots bind.
//!
//! A freed material ([`Materials::free`], M3 W3) gives its slot to later uploads and lets go of its
//! group 3, and with it its textures, once no mesh draws with it and no frame can be reading it (the
//! `retire` module). An upload of several materials that fails partway takes back the ones it took
//! ([`Materials::roll_back`]), so rejected content uses up no slots.

use sapphire_assets::AssetId;
use sapphire_assets::material::{
    AnimeCharacterParams, ArchivedMaterial, FogCardParams, FoliageParams, MaterialFamily, MaterialFlags,
    TextureUsage,
};
use sapphire_rhi::wgpu;

use crate::RenderError;
use crate::buffer::{self, GrowableBuffer};
use crate::pipelines::material_binding;
use crate::retire::{Retiring, new_generation};
use crate::texture::GpuTexture;

/// A material uploaded with [`crate::Renderer::upload_material`], until it's freed
/// ([`crate::Renderer::free_material`]). Only meaningful to the renderer that returned it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialHandle {
    index: u32,
    generation: u64,
}

impl MaterialHandle {
    /// A handle from its raw index, for code that builds packets without a renderer, such as
    /// extraction tests. A sky material handle the renderer doesn't know draws the default sky.
    pub const fn from_raw(index: u32) -> Self {
        Self { index, generation: 0 }
    }

    pub(crate) fn index(self) -> usize {
        self.index as usize
    }
}

/// An anime character material's modes (`AnimeCharacterParams::modes`); `None` for another
/// family.
fn character_modes(material: &ArchivedMaterial) -> Option<[u32; 4]> {
    if material.family != MaterialFamily::AnimeCharacter {
        return None;
    }
    let offset = std::mem::offset_of!(AnimeCharacterParams, modes);
    // Validated materials hold a whole block.
    material.params().get(offset..offset + 16).map(bytemuck::pod_read_unaligned)
}

/// The draw order of `material`'s draws within their surface variant: an anime character eye's
/// layer, 0 for anything else ([`MaterialRecord::layer`]).
fn layer(material: &ArchivedMaterial) -> u32 {
    character_modes(material).map_or(0, AnimeCharacterParams::eye_layer)
}

/// Whether `material` reads the reflection cube ([`MaterialRecord::reflects`]).
fn reflects(material: &ArchivedMaterial) -> bool {
    material.reads_reflection()
}

/// Room in each family's buffer before it first grows, in slots.
const INITIAL_SLOTS: u64 = 64;

/// Every uploaded material.
#[derive(Debug)]
pub(crate) struct Materials {
    /// Per [`MaterialFamily::ALL`] entry.
    families: [FamilyBuffer; MaterialFamily::ALL.len()],
    /// Per handle: a freed one's has no group 3.
    records: Vec<MaterialRecord>,
    /// Where a new material's record starts counting from: this pool's own (the `retire` module).
    epoch: u64,
    /// Indices of [`Self::records`] that are free.
    free_records: Vec<u32>,
    /// Materials asked to be freed that a mesh may still draw with.
    dying: Vec<MaterialHandle>,
    /// Indices of [`Self::records`] that no draw names, until no frame can be reading them.
    retired: Retiring<u32>,
    /// Materials uploaded since the renderer was created, each record's place among which is what
    /// [`Self::roll_back`] goes by.
    uploads: u64,
    sampler: wgpu::Sampler,
    ramp_sampler: wgpu::Sampler,
    /// Nearest, level 0: material IDs.
    id_sampler: wgpu::Sampler,
    /// Bound to every slot without a texture.
    fallback: wgpu::TextureView,
    /// Slots the next uploads pass writes (stats).
    pending: u32,
}

/// How many materials had been uploaded: where [`Materials::roll_back`] goes back to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MaterialsMark {
    uploads: u64,
}

/// One uploaded material.
#[derive(Debug)]
pub(crate) struct MaterialRecord {
    pub family: MaterialFamily,
    /// The material's slot in its family's buffer: what draw records name it by.
    pub slot: u32,
    pub flags: MaterialFlags,
    /// Where its draws go among their surface variant's: an eye's layer (M1 C6,
    /// `AnimeCharacterParams::EYE_LAYER_*`), each drawn over the ones before it; 0 for anything
    /// else.
    pub layer: u32,
    /// An anime character material with the reflection add (M1 C7), which reads the reflection
    /// cube: the ambient pass prefilters it while one draws.
    pub reflects: bool,
    /// Group 3: `None` once it's freed, which lets go of its textures.
    group: Option<Group3>,
    /// Its handle's generation: another names nothing.
    generation: u64,
    /// Meshes drawing with it, whose draws name its slot.
    uses: u32,
    /// Which upload it was.
    upload: u64,
}

impl MaterialRecord {
    /// Group 3. Only a material that draws is asked: nothing draws one that's been freed.
    pub(crate) fn bind_group(&self) -> &wgpu::BindGroup {
        &self.group.as_ref().expect("nothing draws a freed material").bind_group
    }
}

/// A material's group 3, and what each of its texture slots binds: the texture's asset ID, if one is bound,
/// and the view bound, so that the group can be made again when a texture is replaced by a raise or a lower
/// ([`Materials::rebind`], M5 M4).
#[derive(Debug)]
struct Group3 {
    bind_group: wgpu::BindGroup,
    slots: Vec<(Option<AssetId>, wgpu::TextureView)>,
}

/// A family's parameter buffer and its CPU mirror.
#[derive(Debug)]
struct FamilyBuffer {
    family: MaterialFamily,
    buffer: GrowableBuffer,
    /// Every slot's block, slot after slot.
    mirror: Vec<u8>,
    /// Slots written since the last flush, each listed once.
    dirty: Vec<u32>,
    /// Slots freed, for the next uploads.
    free: Vec<u32>,
}

impl FamilyBuffer {
    fn stride(&self) -> usize {
        self.family.params_size()
    }

    fn slots(&self) -> u32 {
        (self.mirror.len() / self.stride()) as u32
    }

    /// Appends a slot holding `params`, to upload with the next frame.
    fn push(&mut self, params: &[u8]) -> u32 {
        let slot = self.slots();
        self.mirror.extend_from_slice(params);
        self.dirty.push(slot);
        slot
    }

    /// Rewrites `slot` with `params`, to upload with the next frame.
    fn write(&mut self, slot: u32, params: &[u8]) {
        let stride = self.stride();
        let start = slot as usize * stride;
        self.mirror[start..start + stride].copy_from_slice(params);
        if !self.dirty.contains(&slot) {
            self.dirty.push(slot);
        }
    }

    fn swap(&mut self, a: u32, b: u32) {
        let stride = self.stride();
        let (first, second) = (a.min(b) as usize * stride, a.max(b) as usize * stride);
        let (head, tail) = self.mirror.split_at_mut(second);
        head[first..first + stride].swap_with_slice(&mut tail[..stride]);
        for slot in [a, b] {
            if !self.dirty.contains(&slot) {
                self.dirty.push(slot);
            }
        }
    }
}

impl Materials {
    /// The renderer checked distinct current handles in the same family. Identity, parameter
    /// slot and mesh-use counts stay attached to each handle; only authored payload moves.
    pub(crate) fn swap_payloads(&mut self, existing: MaterialHandle, prepared: MaterialHandle) {
        let (a, b) = (existing.index(), prepared.index());
        let (first, second) = (a.min(b), a.max(b));
        let (head, tail) = self.records.split_at_mut(second);
        let (left, right) = (&mut head[first], &mut tail[0]);
        self.families[left.family.index()].swap(left.slot, right.slot);
        std::mem::swap(&mut left.flags, &mut right.flags);
        std::mem::swap(&mut left.layer, &mut right.layer);
        std::mem::swap(&mut left.reflects, &mut right.reflects);
        std::mem::swap(&mut left.group, &mut right.group);
    }

    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let max = device.limits().max_storage_buffer_binding_size.min(device.limits().max_buffer_size);
        let families = MaterialFamily::ALL.map(|family| FamilyBuffer {
            family,
            buffer: GrowableBuffer::new(
                device,
                family.name(),
                wgpu::BufferUsages::STORAGE,
                INITIAL_SLOTS * family.params_size() as u64,
                max,
            ),
            mirror: Vec::new(),
            dirty: Vec::new(),
            free: Vec::new(),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        // Level 0 only, even for ramps cooked with mips, which would blend rows.
        let ramp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material ramp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            lod_max_clamp: 0.0,
            ..Default::default()
        });
        // Material IDs must never blend, across texels or mips (`character_maps`).
        let id_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material id"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            lod_max_clamp: 0.0,
            ..Default::default()
        });
        let fallback = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("material fallback"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            fallback.as_image_copy(),
            &[255; 4],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: None },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let fallback = fallback.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            families,
            records: Vec::new(),
            epoch: new_generation(),
            free_records: Vec::new(),
            dying: Vec::new(),
            retired: Retiring::default(),
            uploads: 0,
            sampler,
            ramp_sampler,
            id_sampler,
            fallback,
            pending: 0,
        }
    }

    /// Validates `material`, gives it the next slot of its family's buffer and binds its
    /// textures, resolved by `textures`. Returns the handle and whether the family's buffer was
    /// replaced (bind groups that reference it must be rebuilt).
    pub(crate) fn upload<'t>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layouts: &[wgpu::BindGroupLayout],
        material: &ArchivedMaterial,
        textures: impl Fn(AssetId) -> Option<&'t GpuTexture>,
    ) -> Result<(MaterialHandle, bool), RenderError> {
        material.validate()?;
        let family = material.family;
        if self.free_records.is_empty() && u32::try_from(self.records.len()).is_err() {
            return Err(RenderError::MaterialPoolFull(family));
        }
        let group = self.group(device, layouts, material, textures)?;
        let buffer = &mut self.families[family.index()];
        let (slot, replaced) = if let Some(slot) = buffer.free.pop() {
            buffer.write(slot, material.params());
            (slot, false)
        } else {
            let slot = buffer.slots();
            let stride = buffer.stride() as u64;
            let needed = (u64::from(slot) + 1) * stride;
            if needed > buffer.buffer.max_size() {
                return Err(RenderError::MaterialPoolFull(family));
            }
            // The buffer holds every slot uploaded so far, and slots not yet uploaded are in the
            // mirror, so a GPU copy of the old buffer keeps everything.
            let replaced = buffer.buffer.reserve(device, queue, needed, buffer.buffer.buffer().size());
            buffer.push(material.params());
            (slot, replaced)
        };
        let (flags, layer, reflects) = (material.flags(), layer(material), reflects(material));
        self.uploads += 1;
        let (epoch, upload) = (self.epoch, self.uploads);
        let index = self.free_records.pop().map_or(self.records.len(), |index| index as usize);
        let generation = self.records.get(index).map_or(epoch, |record| record.generation);
        let record = MaterialRecord {
            family,
            slot,
            flags,
            layer,
            reflects,
            group: Some(group),
            generation,
            uses: 0,
            upload,
        };
        if index == self.records.len() {
            self.records.push(record);
        } else {
            self.records[index] = record;
        }
        Ok((MaterialHandle { index: index as u32, generation }, replaced))
    }

    /// Asks for material `handle` to be given back, once no mesh draws with it. Returns whether that's
    /// news: not for a handle already freed, or one of another renderer's.
    pub(crate) fn free(&mut self, handle: MaterialHandle) -> bool {
        if self.record(handle).is_none() || self.dying.contains(&handle) {
            return false;
        }
        self.dying.push(handle);
        true
    }

    /// A mesh now draws with `handle`.
    pub(crate) fn add_use(&mut self, handle: MaterialHandle) {
        self.records[handle.index()].uses += 1;
    }

    /// A mesh no longer draws with `handle`.
    pub(crate) fn drop_use(&mut self, handle: MaterialHandle) {
        self.records[handle.index()].uses -= 1;
    }

    /// Retires each dying material no mesh draws with: its handle stops being current, and it waits
    /// for frame `last_read`, the last that may read its slot or group 3.
    pub(crate) fn retire_unused(&mut self, last_read: u64) {
        let Self { records, dying, retired, .. } = self;
        dying.retain(|handle| {
            let record = &mut records[handle.index()];
            if record.uses > 0 {
                return true;
            }
            record.generation = new_generation();
            retired.push(last_read, handle.index);
            false
        });
    }

    /// Gives back the slot and group 3 of what no frame up to `finished` was the last to read. Returns
    /// whether any went.
    pub(crate) fn release_finished(&mut self, finished: u64) -> bool {
        let mut released = false;
        while let Some(index) = self.retired.pop_finished(finished) {
            self.take_back(index);
            released = true;
        }
        released
    }

    /// Materials dying or retired, still to go.
    pub(crate) fn retiring(&self) -> usize {
        self.dying.len() + self.retired.len()
    }

    fn take_back(&mut self, index: u32) {
        let record = &mut self.records[index as usize];
        record.group = None;
        let family = &mut self.families[record.family.index()];
        family.free.push(record.slot);
        family.dirty.retain(|&slot| slot != record.slot);
        self.free_records.push(index);
    }

    /// Replaces material `handle`'s block and textures: its one slot is uploaded with the next
    /// frame, and its group 3 rebuilt. The family can't change, since draw records name the
    /// slot in the family's own buffer.
    pub(crate) fn update<'t>(
        &mut self,
        device: &wgpu::Device,
        layouts: &[wgpu::BindGroupLayout],
        handle: MaterialHandle,
        material: &ArchivedMaterial,
        textures: impl Fn(AssetId) -> Option<&'t GpuTexture>,
    ) -> Result<(), RenderError> {
        material.validate()?;
        let record = self.record(handle).ok_or(RenderError::UnknownMaterial(handle))?;
        if record.family != material.family {
            return Err(RenderError::MaterialFamilyChanged { from: record.family, to: material.family });
        }
        let group = self.group(device, layouts, material, textures)?;
        let record = &mut self.records[handle.index()];
        record.group = Some(group);
        record.flags = material.flags();
        record.layer = layer(material);
        record.reflects = reflects(material);
        self.families[record.family.index()].write(record.slot, material.params());
        Ok(())
    }

    /// Group 3 for `material`, with each slot's texture checked against its binding.
    fn group<'t>(
        &self,
        device: &wgpu::Device,
        layouts: &[wgpu::BindGroupLayout],
        material: &ArchivedMaterial,
        textures: impl Fn(AssetId) -> Option<&'t GpuTexture>,
    ) -> Result<Group3, RenderError> {
        let family = material.family;
        let descs = family.texture_slots();
        let mut slots = Vec::with_capacity(descs.len());
        for (desc, binding) in descs.iter().zip(material.textures.iter()) {
            let Some(id) = binding.texture() else {
                slots.push((None, self.fallback.clone()));
                continue;
            };
            let texture = textures(id).ok_or(RenderError::MissingTexture { slot: desc.name, id })?;
            let (expected, found) = (binding.swizzle(), texture.swizzle());
            if expected != found {
                return Err(RenderError::TextureSwizzle {
                    slot: desc.name,
                    expected: expected.to_string(),
                    found: found.to_string(),
                });
            }
            let format = texture.texture().format();
            if holds_data(desc.usage) && format.is_srgb() {
                return Err(RenderError::SrgbData { slot: desc.name, format });
            }
            slots.push((Some(id), texture.view().clone()));
        }
        let bind_group = self.bind_group(device, &layouts[family.index()], family, &slots);
        Ok(Group3 { bind_group, slots })
    }

    /// A family's group 3 binding `slots`' views.
    fn bind_group(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        family: MaterialFamily,
        slots: &[(Option<AssetId>, wgpu::TextureView)],
    ) -> wgpu::BindGroup {
        let mut entries: Vec<wgpu::BindGroupEntry<'_>> = slots
            .iter()
            .zip(0..)
            .map(|((_, view), binding)| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            })
            .collect();
        entries.push(wgpu::BindGroupEntry {
            binding: material_binding::SAMPLER,
            resource: wgpu::BindingResource::Sampler(&self.sampler),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: material_binding::RAMP_SAMPLER,
            resource: wgpu::BindingResource::Sampler(&self.ramp_sampler),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: material_binding::ID_SAMPLER,
            resource: wgpu::BindingResource::Sampler(&self.id_sampler),
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(family.name()),
            layout,
            entries: &entries,
        })
    }

    /// Makes group 3 again, binding `texture` in every slot that binds texture `id`, for every material that
    /// binds it: what a raise or a lower of the texture asks, which replaces it with another object of the
    /// same format and swizzle (M5 M4).
    /// Returns how many groups it made.
    pub(crate) fn rebind(
        &mut self,
        device: &wgpu::Device,
        layouts: &[wgpu::BindGroupLayout],
        id: AssetId,
        texture: &GpuTexture,
    ) -> u32 {
        let mut rebound = 0;
        for index in 0..self.records.len() {
            let record = &self.records[index];
            let Some(group) =
                record.group.as_ref().filter(|group| group.slots.iter().any(|(bound, _)| *bound == Some(id)))
            else {
                continue;
            };
            let family = record.family;
            let slots: Vec<_> = group
                .slots
                .iter()
                .map(|(bound, view)| (*bound, if *bound == Some(id) { texture.view() } else { view }.clone()))
                .collect();
            let bind_group = self.bind_group(device, &layouts[family.index()], family, &slots);
            self.records[index].group = Some(Group3 { bind_group, slots });
            rebound += 1;
        }
        rebound
    }
    /// Where the materials are now, for [`Self::roll_back`].
    pub(crate) fn mark(&self) -> MaterialsMark {
        MaterialsMark { uploads: self.uploads }
    }

    /// Takes back every material uploaded since `mark`, as if never uploaded: their handles and
    /// slots go to the next uploads, and nothing of theirs reaches the GPU. Buffers that grew
    /// for them keep their size. Only for uploads that failed partway (RA-005), before any
    /// handle uploaded since `mark` was given out; materials updated since keep their update.
    pub(crate) fn roll_back(&mut self, mark: MaterialsMark) {
        // Last first, so that what's next uploaded takes the same slots and handles as if nothing had
        // been tried: the free lists are stacks.
        for index in (0..self.records.len()).rev() {
            let record = &mut self.records[index];
            if record.upload > mark.uploads && record.group.is_some() {
                record.generation = new_generation();
                self.take_back(index as u32);
            }
        }
    }

    /// The material `handle` names, while it's current.
    pub(crate) fn record(&self, handle: MaterialHandle) -> Option<&MaterialRecord> {
        self.records
            .get(handle.index())
            .filter(|record| record.generation == handle.generation && record.group.is_some())
    }

    /// Material `handle`'s block as its family's mirror holds it: what its slot uploads. The sky
    /// reads its material's on the CPU (see the `sky` module).
    pub(crate) fn params(&self, handle: MaterialHandle) -> Option<&[u8]> {
        let record = self.record(handle)?;
        let buffer = &self.families[record.family.index()];
        let start = record.slot as usize * buffer.stride();
        buffer.mirror.get(start..start + buffer.stride())
    }

    /// The wind response of foliage material `handle` (`FoliageParams::wind`: sway, bend, flutter,
    /// flutter frequency), as the GPU will read it; `None` for another family's.
    pub(crate) fn foliage_wind(&self, handle: MaterialHandle) -> Option<[f32; 4]> {
        let record = self.record(handle).filter(|record| record.family == MaterialFamily::Foliage)?;
        let buffer = &self.families[record.family.index()];
        let start = record.slot as usize * buffer.stride();
        let params: FoliageParams =
            bytemuck::pod_read_unaligned(buffer.mirror.get(start..start + buffer.stride())?);
        Some(params.wind)
    }

    /// The fog card block as the GPU reads it. Returns `None` for other families.
    pub(crate) fn fog_card_params(&self, handle: MaterialHandle) -> Option<FogCardParams> {
        let record = self.record(handle).filter(|record| record.family == MaterialFamily::FogCard)?;
        let buffer = &self.families[record.family.index()];
        let start = record.slot as usize * buffer.stride();
        Some(bytemuck::pod_read_unaligned(buffer.mirror.get(start..start + buffer.stride())?))
    }

    /// The anime character block in `slot` of that family's buffer, as the GPU will hold it: what
    /// draw records name materials by.
    pub(crate) fn anime_character(&self, slot: u32) -> Option<AnimeCharacterParams> {
        let buffer = &self.families[MaterialFamily::AnimeCharacter.index()];
        let start = slot as usize * buffer.stride();
        buffer.mirror.get(start..start + buffer.stride()).map(bytemuck::pod_read_unaligned)
    }

    /// The parameter buffer of `family`.
    pub(crate) fn buffer(&self, family: MaterialFamily) -> &wgpu::Buffer {
        self.families[family.index()].buffer.buffer()
    }

    /// Counts the slots the next uploads pass writes, for the stats.
    pub(crate) fn prepare(&mut self) {
        self.pending = self.families.iter().map(|family| family.dirty.len() as u32).sum();
    }

    /// Slots counted by the last [`Self::prepare`].
    pub(crate) fn pending(&self) -> u32 {
        self.pending
    }

    /// Materials with a slot, freed ones still waiting for frames among them.
    pub(crate) fn len(&self) -> u32 {
        (self.records.len() - self.free_records.len()) as u32
    }

    /// Bytes of every family's slots in use.
    pub(crate) fn bytes_used(&self) -> u64 {
        let free = |family: &FamilyBuffer| (family.free.len() * family.stride()) as u64;
        self.families.iter().map(|family| family.mirror.len() as u64 - free(family)).sum()
    }

    pub(crate) fn growths(&self) -> u32 {
        self.families.iter().map(|family| family.buffer.growths()).sum()
    }

    /// Gives `family` a new, empty buffer that holds at most `slots` slots, for tests of a full
    /// one. Bind groups made with the old buffer keep it, so only for tests that draw nothing of
    /// the family's.
    #[cfg(test)]
    pub(crate) fn limit_slots(&mut self, device: &wgpu::Device, family: MaterialFamily, slots: u64) {
        let buffer = &mut self.families[family.index()];
        assert!(buffer.mirror.is_empty(), "{family} already has materials");
        let max = slots * family.params_size() as u64;
        buffer.buffer = GrowableBuffer::new(device, family.name(), wgpu::BufferUsages::STORAGE, max, max);
    }

    /// Splits the materials between a frame's passes: the uploads pass writes every family's
    /// dirty slots, and draws bind each material's group 3, indexed by handle.
    pub(crate) fn split_for_record(&mut self) -> (MaterialUploads<'_>, &[MaterialRecord]) {
        (MaterialUploads { families: &mut self.families }, &self.records)
    }
}

/// Whether a slot of `usage` holds linear data, which an sRGB texture would decode as color.
fn holds_data(usage: TextureUsage) -> bool {
    match usage {
        TextureUsage::Normal | TextureUsage::Mask | TextureUsage::Control | TextureUsage::FaceShadow => true,
        TextureUsage::Color | TextureUsage::Light | TextureUsage::Ramp => false,
    }
}

/// The materials' share of a frame's uploads pass.
pub(crate) struct MaterialUploads<'a> {
    families: &'a mut [FamilyBuffer],
}

impl MaterialUploads<'_> {
    /// Writes each run of consecutive dirty slots through `belt`, then marks them clean.
    pub(crate) fn write(self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        for family in self.families.iter_mut() {
            let stride = family.stride();
            family.dirty.sort_unstable();
            for run in family.dirty.chunk_by(|&a, &b| b == a + 1) {
                let (first, last) = (run[0] as usize, run[run.len() - 1] as usize);
                let bytes = &family.mirror[first * stride..(last + 1) * stride];
                buffer::stage(belt, encoder, family.buffer.buffer(), (first * stride) as u64, bytes);
            }
            family.dirty.clear();
        }
    }
}

/// Tests that read the material buffers back. They need a GPU: run them with
/// `cargo test -p sapphire_render --tests -- --ignored`.
#[cfg(test)]
mod gpu_tests {
    use glam::Vec3;
    use sapphire_assets::material::AlignedVec;
    use sapphire_assets::material::{
        AnimeCharacterMaterial, AnimeCharacterParams, Material, SkyMaterial, SkyParams,
    };
    use sapphire_assets::texture::Swizzle;
    use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc};

    use super::*;
    use crate::graph::{FrameGraph, GraphExecutor};
    use crate::{CameraData, FramePacketRing, RenderSettings, Renderer, ViewData};

    const BLOCK: usize = size_of::<AnimeCharacterParams>();

    fn cooked(base_color: [f32; 4]) -> AlignedVec {
        let document = AnimeCharacterMaterial { base_color, ..AnimeCharacterMaterial::default() };
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        Material::from_document(&document, &swizzles).unwrap().to_cooked().unwrap()
    }

    fn params(bytes: &[u8]) -> AnimeCharacterParams {
        bytemuck::pod_read_unaligned(Material::read(bytes).unwrap().params())
    }

    /// The anime character materials the test uploads.
    const SLOTS: usize = 3;

    /// Prepares and records one frame with no instances, then reads the anime character
    /// family's buffer back: the test's slots as the frame's uploads left them.
    fn frame(
        gpu: &GpuContext,
        renderer: &mut Renderer,
        packets: &mut FramePacketRing,
    ) -> Vec<AnimeCharacterParams> {
        let camera = CameraData::look_at(Vec3::Z, Vec3::ZERO, Vec3::Y, 0.9, 1.0, 0.05);
        packets.begin().views.push(ViewData::new(camera));
        renderer.prepare(gpu, packets.current());
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let buffer = renderer.materials().buffer(MaterialFamily::AnimeCharacter).clone();
        let size = (SLOTS * BLOCK) as u64;
        let readback = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        renderer.record(&mut graph, target);
        let (buffer, readback_buffer) = (&buffer, &readback);
        graph.add_pass("read materials").side_effect().record(move |ctx| {
            ctx.encoder.copy_buffer_to_buffer(buffer, 0, readback_buffer, 0, size);
        });
        GraphExecutor::new().execute(gpu, graph).expect("a valid graph");
        readback.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        bytemuck::pod_collect_to_vec(&readback.get_mapped_range(..).expect("mapped"))
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn an_update_uploads_only_its_own_slot() {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut renderer = Renderer::new(&gpu, format, RenderSettings::default()).expect("renderer");
        // The stats count the sky's default material from the start.
        let stats = renderer.stats();
        assert_eq!((stats.materials, stats.material_bytes), (1, size_of::<SkyParams>() as u64));
        let mut packets = FramePacketRing::new();
        let materials = [[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]].map(cooked);
        let handles = materials
            .each_ref()
            .map(|bytes| renderer.upload_material(Material::read(bytes).unwrap(), |_| None).unwrap());
        // And the sky's default material, which the renderer uploads at creation (M1 W2).
        let stats = renderer.stats();
        let bytes = (SLOTS * BLOCK + size_of::<SkyParams>()) as u64;
        assert_eq!((stats.materials, stats.material_bytes), (SLOTS as u32 + 1, bytes));

        // The first frame uploads all four.
        let slots = frame(&gpu, &mut renderer, &mut packets);
        assert_eq!(renderer.stats().material_uploads, SLOTS as u32 + 1);
        assert_eq!(slots, materials.each_ref().map(|bytes| params(bytes)));
        frame(&gpu, &mut renderer, &mut packets);
        assert_eq!(renderer.stats().material_uploads, 0, "unchanged materials upload nothing");

        // Scribble over every slot on the GPU behind the renderer's back: an update that
        // uploaded more than its own slot would repair the others.
        let buffer = renderer.materials().buffer(MaterialFamily::AnimeCharacter);
        gpu.queue().write_buffer(buffer, 0, &[0xab; SLOTS * BLOCK]);
        let yellow = cooked([1.0, 1.0, 0.0, 1.0]);
        renderer.update_material(handles[1], Material::read(&yellow).unwrap(), |_| None).unwrap();
        let slots = frame(&gpu, &mut renderer, &mut packets);
        assert_eq!(renderer.stats().material_uploads, 1);
        assert_eq!(slots[1], params(&yellow));
        let scribbled: AnimeCharacterParams = bytemuck::pod_read_unaligned(&[0xab; BLOCK]);
        assert_eq!((slots[0], slots[2]), (scribbled, scribbled), "the other slots weren't uploaded again");

        // A handle keeps its family, and belongs to the renderer that made it.
        let sky = Material::from_document(
            &SkyMaterial::default(),
            &[Swizzle::IDENTITY; SkyMaterial::TEXTURE_SLOTS.len()],
        )
        .unwrap();
        let sky = sky.to_cooked().unwrap();
        let error =
            renderer.update_material(handles[0], Material::read(&sky).unwrap(), |_| None).unwrap_err();
        assert!(matches!(error, RenderError::MaterialFamilyChanged { .. }), "{error}");
        let unknown = MaterialHandle::from_raw(9);
        let error =
            renderer.update_material(unknown, Material::read(&yellow).unwrap(), |_| None).unwrap_err();
        assert!(matches!(error, RenderError::UnknownMaterial(_)), "{error}");
    }

    /// A cooked triangle drawn once per material of `colors`, slot `i` by the `i`th, embedded.
    fn mesh_embedding(colors: &[[f32; 4]]) -> AlignedVec {
        use sapphire_assets::mesh::{
            MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
        };

        let vertex = |position: [f32; 3]| Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [0.0; 2],
            color: VERTEX_COLOR_NEUTRAL,
            flags: VertexFlags::NONE,
        };
        let vertices = [vertex([0.0; 3]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])];
        let count = colors.len() as u32;
        let indices = [0, 1, 2].repeat(colors.len());
        let submeshes =
            (0..count).map(|i| Submesh { first_index: 3 * i, index_count: 3, material: i }).collect();
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        let slots = colors
            .iter()
            .map(|&base_color| {
                let document = AnimeCharacterMaterial { base_color, ..AnimeCharacterMaterial::default() };
                MaterialSlot::embedded("slot", Material::from_document(&document, &swizzles).unwrap())
            })
            .collect();
        Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
            .to_cooked()
            .unwrap()
    }

    /// RA-005: a mesh whose embedded materials don't all fit their family's buffer uploads none
    /// of them, however often it is retried; the slots it took go to the next uploads, and frames
    /// upload only those.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_full_family_takes_back_what_the_upload_took() {
        use sapphire_assets::mesh::Mesh;

        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut renderer = Renderer::new(&gpu, format, RenderSettings::default()).expect("renderer");
        renderer.limit_material_slots(MaterialFamily::AnimeCharacter, SLOTS as u64);
        let mut packets = FramePacketRing::new();
        let colors = [[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 0.0, 1.0]];
        // The sky's default material, uploaded at creation.
        let before = (renderer.materials().len(), renderer.materials().bytes_used());

        let too_many = mesh_embedding(&colors);
        for attempt in 1..=3 {
            let error = renderer.upload_embedded_materials(Mesh::read(&too_many).unwrap()).unwrap_err();
            assert!(
                matches!(error, RenderError::MaterialPoolFull(MaterialFamily::AnimeCharacter)),
                "{error}"
            );
            let materials = renderer.materials();
            assert_eq!((materials.len(), materials.bytes_used()), before, "attempt {attempt}");
            assert_eq!(renderer.stats().materials, before.0, "the stats follow");
        }

        // What fits takes the first slots and handles, as if nothing had been tried before it.
        let fits = mesh_embedding(&colors[1..]);
        let handles = renderer.upload_embedded_materials(Mesh::read(&fits).unwrap()).unwrap();
        let indices: Vec<usize> = handles.iter().map(|handle| handle.index()).collect();
        assert_eq!(indices, (0..SLOTS).map(|i| before.0 as usize + i).collect::<Vec<_>>());
        let slots: Vec<u32> =
            handles.iter().map(|&handle| renderer.materials().record(handle).unwrap().slot).collect();
        assert_eq!(slots, [0, 1, 2]);
        // A frame uploads those and the sky's: nothing of the slots taken back.
        let uploaded = frame(&gpu, &mut renderer, &mut packets);
        assert_eq!(renderer.stats().material_uploads, SLOTS as u32 + 1);
        let expected: Vec<AnimeCharacterParams> =
            colors[1..].iter().map(|&color| params(&cooked(color))).collect();
        assert_eq!(uploaded, expected);
    }
}
