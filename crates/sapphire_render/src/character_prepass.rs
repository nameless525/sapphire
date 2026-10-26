//! The character prepass (M1 C5; spec §5.12 "Prepasses"): the characters' own depth and IDs,
//! drawn before the main pass on every tier, which the anime character shader reads for the
//! depth-offset rim, the bangs' shadow on faces and eyes through the bangs.
//!
//! - **What it draws:** the opaque and alpha-tested anime character surfaces of the instances the
//!   frame lists as characters ([`FramePacket::characters`](crate::FramePacket::characters)), the
//!   same set that reads a light rig (rule R4), static or skinned. An unlisted instance, a skinned
//!   prop's or an anime-shaded pedestal's, stays out, so nothing takes it for a character.
//!   Position-only, through the instance's own clip position (invariant, as every pass's), so its
//!   depth is the main pass's bit for bit.
//! - **Targets:** persistent textures the renderer owns at the scene's size, made again when the
//!   size changes, single-sampled, and imported into each frame's graph. The depth is
//!   `Depth32Float` (reverse-Z, cleared to 0); the IDs are `Rg16Uint`, two IDs ([`encode_id`]):
//!   the character's entry in the frame's character buffer in the low 14 bits (0 for none) and
//!   its part in the top 2 (body, face, hair, eye: the material's `CharacterSurface`). R is the
//!   nearest surface's; G the nearest that isn't hair (M1 C6), since hair draws last and writes
//!   R alone ([`HAIR_ID_WRITES`]). The part bits do the work of the stencil bits a hardware
//!   stencil would hold for eyes over hair: R says which pixels are hair the eyes may show
//!   through, and G whether the character's eye would be the nearest surface there but for hair,
//!   the **eye depth mask**: a layered eye's iris, shadow and highlights draw only within it,
//!   and eyes draw through hair only where it holds them, so a lid closed over an eye hides it
//!   under the bangs too. A frame has at most 16,383 characters, so no entry's bits alias
//!   another's or none: the character list ignores and counts the rest
//!   ([`RenderStats::ignored_characters`](crate::RenderStats::ignored_characters)).
//! - **Who reads them:** the main pass, through group 0 (bindings 8 and 9, with a nearest clamped
//!   sampler at 10), so the frame's bind group is made again with the targets; and temporal AA's
//!   responsive mask, which marks where a character is nearest. The prepass therefore runs every
//!   frame with a view, clearing both when no character is listed.
//! - **Mobile:** the main pass keeps MSAA color and depth in tile memory, so the prepass can't
//!   share it: it is a render pass of its own before the main pass, whose two targets are stored
//!   (8 bytes a pixel, about 6 MB a frame at Mobile-Low's 1280 × 590; 6 and 4.5 MB before M1
//!   C6's second ID) and sampled by the main pass. Its draws are position-only, about one per
//!   character part.
//!
//! # Later milestones
//!
//! - Alpha-tested materials cut out of the prepass (a variant binding group 3, with UVs): today
//!   it holds their whole geometry, so a cutout hair card's holes still rim and shadow.
//! - On tilers, packing the depth into the ID target as linear depth (one `Rg16Float`-sized
//!   texel) and keeping the depth attachment transient, if the store shows up in captures.

use std::mem::offset_of;

use sapphire_assets::material::{AnimeCharacterParams, MaterialFlags};
use sapphire_rhi::wgpu;

use crate::graph::{FrameGraph, TextureId};
use crate::materials::{MaterialHandle, Materials};
use crate::pipelines::{DEPTH_FORMAT, Part, Shading, SurfaceVariant};
use crate::scene::DrawList;

/// The character IDs' format: R the nearest surface's ID, G the nearest non-hair surface's.
pub(crate) const IDS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Uint;

/// What hair writes of the IDs: R alone, drawn after every other part, so G keeps the nearest
/// surface that isn't hair (M1 C6).
pub(crate) const HAIR_ID_WRITES: wgpu::ColorWrites = wgpu::ColorWrites::RED;

/// Bits of a character ID that hold the character's entry (WGSL `CHARACTER_ID_ENTRY_BITS`).
pub(crate) const ID_ENTRY_BITS: u32 = 14;

/// The entry's bits of a character ID (WGSL `CHARACTER_ID_ENTRY_MASK`).
pub(crate) const ID_ENTRY_MASK: u32 = (1 << ID_ENTRY_BITS) - 1;

/// The character ID the prepass writes for part `part` (`AnimeCharacterParams::SURFACE_*`) of the
/// character in entry `entry` (WGSL `character_id`).
#[cfg_attr(not(test), expect(dead_code, reason = "the shader encodes; tests decode with it"))]
pub(crate) const fn encode_id(entry: u32, part: u32) -> u32 {
    (entry & ID_ENTRY_MASK) | part << ID_ENTRY_BITS
}

/// The character prepass's persistent targets and the sampler the main pass reads them with.
#[derive(Debug)]
pub(crate) struct CharacterPrepassTargets {
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    ids: wgpu::Texture,
    ids_view: wgpu::TextureView,
    /// Nearest, clamped to the edge, level 0: the shader's gathers and loads.
    sampler: wgpu::Sampler,
}

impl CharacterPrepassTargets {
    /// 1 × 1 targets, so the frame's bind group is complete before the first frame sizes them.
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let (depth, ids) = (texture(device, "character depth", DEPTH_FORMAT, (1, 1)), {
            texture(device, "character ids", IDS_FORMAT, (1, 1))
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("character prepass"),
            ..Default::default()
        });
        Self {
            depth_view: depth.create_view(&Default::default()),
            ids_view: ids.create_view(&Default::default()),
            depth,
            ids,
            sampler,
        }
    }

    /// Makes the targets `size` unless they are. Returns whether they were made again, so the
    /// frame's bind group, which binds them, must be too.
    pub(crate) fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) -> bool {
        if (self.depth.width(), self.depth.height()) == size {
            return false;
        }
        self.depth = texture(device, "character depth", DEPTH_FORMAT, size);
        self.ids = texture(device, "character ids", IDS_FORMAT, size);
        self.depth_view = self.depth.create_view(&Default::default());
        self.ids_view = self.ids.create_view(&Default::default());
        true
    }

    /// Group 0 bindings 8 to 10.
    pub(crate) fn frame_entries(&self) -> [wgpu::BindGroupEntry<'_>; 3] {
        [
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&self.depth_view),
            },
            wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&self.ids_view) },
            wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::Sampler(&self.sampler) },
        ]
    }

    /// The targets in `graph`: depth, then IDs.
    pub(crate) fn import<'f>(&'f self, graph: &mut FrameGraph<'f>) -> (TextureId, TextureId) {
        (graph.import_texture("character depth", &self.depth_view), {
            graph.import_texture("character ids", &self.ids_view)
        })
    }
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
    (width, height): (u32, u32),
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// What the character prepass and its readers drew in a frame (rule R9), in
/// [`RenderStats::character_prepass`](crate::RenderStats::character_prepass).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CharacterPrepassStats {
    /// Draws in the character prepass: the opaque and alpha-tested anime character surfaces of
    /// the characters the frame lists.
    pub draws: u32,
    /// Characters with a face whose material casts the bangs' shadow on it.
    pub hair_shadow_characters: u32,
    /// Face and eye draws drawn again through the character's hair (eyes and brows over the
    /// bangs), after the outlines in the main pass; eye layers among them (M1 C6).
    pub through_hair_draws: u32,
    /// Eye layers over the characters' eyes (M1 C6): their shadows, irises and highlights, drawn
    /// within the eye depth mask after the sky.
    pub eye_layer_draws: u32,
}

impl CharacterPrepassStats {
    /// Counts the frame's: `list`'s opaque and alpha-tested anime character draws of the instances
    /// `entry` gives a character entry (by slot), the characters among them with a face whose
    /// material casts the bangs' shadow, their eye layers, and the face and eye draws that draw
    /// through hair. `characters` is scratch, kept so a frame allocates nothing.
    pub(crate) fn count(
        list: &DrawList,
        entry: impl Fn(u32) -> u32,
        materials: &Materials,
        characters: &mut Vec<u32>,
    ) -> Self {
        let mut stats = Self::default();
        characters.clear();
        let variants =
            SurfaceVariant::CHARACTER_DEPTH_WRITING.into_iter().chain([SurfaceVariant::EYE_LAYERS]);
        for variant in variants {
            let index = variant.index();
            let statics = list.commands.iter().flat_map(|commands| &commands[index]);
            let draws = statics
                .map(|command| (command.slot, command.material))
                .chain(list.deformed[index].iter().map(|draw| (draw.slot, draw.material)));
            for (slot, material) in draws {
                let character = entry(slot);
                if character == 0 {
                    continue;
                }
                // Eye layers blend: they aren't in the prepass.
                if variant == SurfaceVariant::EYE_LAYERS {
                    stats.eye_layer_draws += 1;
                } else {
                    stats.draws += 1;
                }
                let through = materials
                    .record(material)
                    .is_some_and(|record| record.flags.contains(MaterialFlags::THROUGH_HAIR));
                stats.through_hair_draws += u32::from(through);
                if variant.shading == Shading::Character(Part::Face) && casts_hair_shadow(materials, material)
                {
                    characters.push(character);
                }
            }
        }
        characters.sort_unstable();
        characters.dedup();
        stats.hair_shadow_characters = characters.len() as u32;
        stats
    }
}

/// Whether face material `material` casts the bangs' shadow: a distance and a strength above 0.
fn casts_hair_shadow(materials: &Materials, material: MaterialHandle) -> bool {
    let offset = offset_of!(AnimeCharacterParams, hair_shadow);
    let field = materials.params(material).and_then(|block| block.get(offset..offset + 8));
    field.is_some_and(|bytes| {
        let [distance, strength]: [f32; 2] = bytemuck::pod_read_unaligned(bytes);
        distance > 0.0 && strength > 0.0
    })
}
