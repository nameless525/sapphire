//! Materials on the GPU (M1 A3): cooked [`ArchivedMaterial`]s in stable slots of their family's
//! parameter buffer, with their textures bound in group 3.
//!
//! - **One buffer per family**, an array of the family's block (`AnimeCharacterParams`,
//!   `EnvironmentParams`, …, `shaders/materials.wgsl`). The blocks differ in size by more than
//!   ten times, so one shared array would pad every slot to the largest; and a pipeline only
//!   ever reads its own family, so binding one buffer keeps the per-stage storage-buffer count
//!   unchanged (crate docs, "Binding budget").
//! - **Stable slots.** A [`MaterialHandle`] keeps its slot for the renderer's lifetime, and a
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
//! - Only the anime character family draws meshes so far, so the scene group (group 2) is built
//!   for its buffer alone. Each family whose meshes get shaders (W3–W6) gets a scene group of its
//!   own, rebuilt when the instance buffer or its family's buffer grows. The sky (W2) draws one
//!   material at a time and reads its block from the mirror ([`Materials::params`]), binding
//!   only its group 3.
//!
//! Slots are never freed: residency arrives with the asset runtime (A4), as it does for meshes.

use sapphire_assets::AssetId;
use sapphire_assets::material::{ArchivedMaterial, MaterialFamily, MaterialFlags, TextureUsage};
use sapphire_rhi::wgpu;

use crate::RenderError;
use crate::buffer::{self, GrowableBuffer};
use crate::pipelines::material_binding;
use crate::texture::GpuTexture;

/// A material uploaded with [`crate::Renderer::upload_material`]. Only meaningful to the
/// renderer that returned it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialHandle(pub(crate) u32);

impl MaterialHandle {
    /// A handle from its raw index (the order materials were uploaded in), for code that builds
    /// packets without a renderer, such as extraction tests. A sky material handle the renderer
    /// doesn't know draws the default sky.
    pub const fn from_raw(index: u32) -> Self {
        Self(index)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// Room in each family's buffer before it first grows, in slots.
const INITIAL_SLOTS: u64 = 64;

/// Every uploaded material.
#[derive(Debug)]
pub(crate) struct Materials {
    /// Per [`MaterialFamily::ALL`] entry.
    families: [FamilyBuffer; MaterialFamily::ALL.len()],
    /// Per handle.
    records: Vec<MaterialRecord>,
    sampler: wgpu::Sampler,
    ramp_sampler: wgpu::Sampler,
    /// Nearest, level 0: material IDs.
    id_sampler: wgpu::Sampler,
    /// Bound to every slot without a texture.
    fallback: wgpu::TextureView,
    /// Slots the next uploads pass writes (stats).
    pending: u32,
}

/// One uploaded material.
#[derive(Debug)]
pub(crate) struct MaterialRecord {
    pub family: MaterialFamily,
    /// The material's slot in its family's buffer: what draw records name it by.
    pub slot: u32,
    pub flags: MaterialFlags,
    /// Group 3.
    pub bind_group: wgpu::BindGroup,
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
}

impl Materials {
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
        Self { families, records: Vec::new(), sampler, ramp_sampler, id_sampler, fallback, pending: 0 }
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
        let handle = MaterialHandle(
            u32::try_from(self.records.len()).map_err(|_| RenderError::MaterialPoolFull(family))?,
        );
        let bind_group = self.bind_group(device, layouts, material, textures)?;
        let buffer = &mut self.families[family.index()];
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
        self.records.push(MaterialRecord { family, slot, flags: material.flags(), bind_group });
        Ok((handle, replaced))
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
        let record = self.records.get(handle.index()).ok_or(RenderError::UnknownMaterial(handle))?;
        if record.family != material.family {
            return Err(RenderError::MaterialFamilyChanged { from: record.family, to: material.family });
        }
        let bind_group = self.bind_group(device, layouts, material, textures)?;
        let record = &mut self.records[handle.index()];
        record.bind_group = bind_group;
        record.flags = material.flags();
        self.families[record.family.index()].write(record.slot, material.params());
        Ok(())
    }

    /// Group 3 for `material`, with each slot's texture checked against its binding.
    fn bind_group<'t>(
        &self,
        device: &wgpu::Device,
        layouts: &[wgpu::BindGroupLayout],
        material: &ArchivedMaterial,
        textures: impl Fn(AssetId) -> Option<&'t GpuTexture>,
    ) -> Result<wgpu::BindGroup, RenderError> {
        let family = material.family;
        let slots = family.texture_slots();
        let mut views = Vec::with_capacity(slots.len());
        for (desc, binding) in slots.iter().zip(material.textures.iter()) {
            let Some(id) = binding.texture() else {
                views.push(&self.fallback);
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
            views.push(texture.view());
        }
        let mut entries: Vec<wgpu::BindGroupEntry<'_>> = views
            .iter()
            .zip(0..)
            .map(|(view, binding)| wgpu::BindGroupEntry {
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
        Ok(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(family.name()),
            layout: &layouts[family.index()],
            entries: &entries,
        }))
    }

    pub(crate) fn record(&self, handle: MaterialHandle) -> Option<&MaterialRecord> {
        self.records.get(handle.index())
    }

    /// Material `handle`'s block as its family's mirror holds it: what its slot uploads. The sky
    /// reads its material's on the CPU (see the `sky` module).
    pub(crate) fn params(&self, handle: MaterialHandle) -> Option<&[u8]> {
        let record = self.record(handle)?;
        let buffer = &self.families[record.family.index()];
        let start = record.slot as usize * buffer.stride();
        buffer.mirror.get(start..start + buffer.stride())
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

    pub(crate) fn len(&self) -> u32 {
        self.records.len() as u32
    }

    /// Bytes of every family's slots.
    pub(crate) fn bytes_used(&self) -> u64 {
        self.families.iter().map(|family| family.mirror.len() as u64).sum()
    }

    pub(crate) fn growths(&self) -> u32 {
        self.families.iter().map(|family| family.buffer.growths()).sum()
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
        let sky = Material::from_document(&SkyMaterial::default(), &[Swizzle::IDENTITY; 5]).unwrap();
        let sky = sky.to_cooked().unwrap();
        let error =
            renderer.update_material(handles[0], Material::read(&sky).unwrap(), |_| None).unwrap_err();
        assert!(matches!(error, RenderError::MaterialFamilyChanged { .. }), "{error}");
        let unknown = MaterialHandle(9);
        let error =
            renderer.update_material(unknown, Material::read(&yellow).unwrap(), |_| None).unwrap_err();
        assert!(matches!(error, RenderError::UnknownMaterial(_)), "{error}");
    }
}
