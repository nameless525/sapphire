//! Texture uploads (M1 A2): a cooked [`Texture`] becomes a sampled wgpu texture holding every
//! mip level, once the device is known to support its format.
//!
//! Cooked textures are sampled with filtering and mips, so a format counts as supported when
//! the device can filter it ([`GpuContext::format_support`]). A device without its cook's
//! formats (BC without `TEXTURE_COMPRESSION_BC`, ASTC without `TEXTURE_COMPRESSION_ASTC`)
//! gets an error naming the cook it needs instead of a texture the driver decompresses or
//! fails on (spec §6.1). [`check_format`] runs the same check without uploading, for a boot
//! check over the formats a cook uses.
//!
//! Levels are uploaded as they are, with one `Queue::write_texture` each: textures are
//! one-off uploads, like meshes. Materials bind them (`materials`, M1 A3), and shaders apply
//! their [`Swizzle`], which wgpu views can't, from the material's block. Handles, residency and
//! the per-frame upload budget are the asset server's (`sapphire_assets`, M1 A4): it hands the
//! app the textures to upload here, and the app drops a [`GpuTexture`] the server unloads.
//!
//! # Tails and levels (M5 M3, M4)
//!
//! A large texture is cooked as a tail and the levels above it, each an asset of its own
//! (`sapphire_assets::texture`'s "Tails"). [`GpuTexture::upload_tail`] uploads a tail as the smaller
//! texture it holds, whose [`top`](GpuTexture::top) is the whole texture's level its level 0 is. wgpu has
//! no sparse textures, so a texture changes its top as m5-plan §3 decides (M5 M0's spike proved it):
//! [`GpuTexture::raised`] makes a new texture with more levels, copies the kept ones into it with
//! `copy_texture_to_texture` and writes the new ones; [`GpuTexture::lowered`] makes one with fewer and
//! copies what it keeps. Either way the texture is a new object, which the materials binding it must
//! rebind and whose old one must outlive the frames reading it: the renderer does both
//! ([`crate::Renderer::raise_texture`]). A texture that can rise (its tail has levels above) is created
//! with `COPY_SRC` too, which costs Metal and D3D12 no flag and Vulkan a usage bit; one that can't is
//! made as ever.
//!
//! A level's size is whole blocks of its format, so every level above a tail can be a texture's top
//! (WebGPU asks it of level 0; the cook's split sees to it). Levels smaller than a block copy as whole
//! blocks, which wgpu pads and clamps per backend.

use sapphire_assets::texture::{
    AstcBlock, Swizzle, Texture, TextureDesc, TextureFormat, TextureFormatError, TextureTail,
};
use sapphire_assets::texture_level::ArchivedTextureLevel;
use sapphire_rhi::{FormatSupport, GpuContext, wgpu};

/// A cooked texture on the GPU: its levels from its [top](Self::top) down, viewed as one 2D texture.
#[derive(Debug)]
pub struct GpuTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// The whole texture's shape, its levels above the tail it was made from included.
    whole: TextureDesc,
    /// The whole texture's level that is this one's level 0.
    top: u32,
    /// How many of the whole texture's levels lie above its tail: the deepest [`Self::top`] goes.
    above: u32,
}

/// Errors from uploading a texture, or changing its levels.
#[derive(Debug, thiserror::Error)]
pub enum TextureError {
    #[error("invalid texture")]
    Invalid(#[from] TextureFormatError),
    #[error("{format:?} textures can't be sampled with filtering on this device ({adapter}): {reason}")]
    Unsupported { format: TextureFormat, adapter: String, reason: &'static str },
    #[error("{width}x{height} is larger than this device's {max}-texel limit")]
    TooLarge { width: u32, height: u32, max: u32 },
    #[error(
        "level {level} ({width}x{height} {format:?}) isn't one this texture can take ({texture:?}, top {top})"
    )]
    NotALevel { level: u32, width: u32, height: u32, format: TextureFormat, texture: TextureDesc, top: u32 },
    #[error("a tail of {tail:?} can't replace this texture's ({texture:?}) in place")]
    OtherShape { tail: TextureDesc, texture: TextureDesc },
    #[error("a texture whose top is level {top} can't be lowered to level {to}: its tail's top is {above}")]
    CantLower { top: u32, to: u32, above: u32 },
}

impl GpuTexture {
    /// Validates `texture`, checks the device supports its format and size, and uploads every
    /// level. `label` names the texture in GPU captures when object labels are on. A tail uploads as
    /// the smaller texture it holds, which can't rise: [`Self::upload_tail`] uploads one that can.
    pub fn upload(gpu: &GpuContext, texture: &Texture<'_>, label: &str) -> Result<Self, TextureError> {
        profiling::scope!("GpuTexture::upload");
        texture.validate()?;
        Self::create_from(gpu, &TextureTail { texture: texture.clone(), above: 0 }, label)
    }

    /// Validates `tail`, checks the device supports its format and its whole texture's size, and uploads
    /// the tail's levels as a texture whose top is the tail's: the whole texture's level
    /// [`TextureTail::above`]. It [rises](Self::raised) to the levels above. A tail with none above
    /// uploads as [`Self::upload`] uploads it.
    pub fn upload_tail(gpu: &GpuContext, tail: &TextureTail<'_>, label: &str) -> Result<Self, TextureError> {
        profiling::scope!("GpuTexture::upload_tail");
        tail.validate()?;
        Self::create_from(gpu, tail, label)
    }

    /// `tail`'s texture, with its levels written.
    fn create_from(gpu: &GpuContext, tail: &TextureTail<'_>, label: &str) -> Result<Self, TextureError> {
        let whole = tail.whole();
        check_format(gpu, whole.format)?;
        let max = gpu.device().limits().max_texture_dimension_2d;
        if whole.width > max || whole.height > max {
            return Err(TextureError::TooLarge { width: whole.width, height: whole.height, max });
        }
        let label = gpu.instance().debug().object_labels.then_some(label);
        let uploaded = Self::create(gpu.device(), whole, tail.above, tail.above, label);
        for (level, data) in (tail.above..).zip(&tail.texture.levels) {
            uploaded.write(gpu.queue(), level, data);
        }
        Ok(uploaded)
    }

    /// An empty texture of `whole`'s levels from `top` down, for a whole texture with `above` levels above
    /// its tail.
    fn create(device: &wgpu::Device, whole: TextureDesc, top: u32, above: u32, label: Option<&str>) -> Self {
        let desc = from_level(&whole, top);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label,
            size: wgpu::Extent3d { width: desc.width, height: desc.height, depth_or_array_layers: 1 },
            mip_level_count: desc.mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu_format(desc.format),
            usage: usage(above),
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view, whole, top, above }
    }

    /// Writes `data`, the whole texture's level `level`'s blocks, as it is, to the level it is here.
    fn write(&self, queue: &wgpu::Queue, level: u32, data: &[u8]) {
        let (block_width, block_height) = self.whole.format.block_dimensions();
        let (width, height) = self.whole.level_extent(level);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: level - self.top,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width.div_ceil(block_width) * self.whole.format.block_bytes()),
                rows_per_image: Some(height.div_ceil(block_height)),
            },
            copy_extent(&self.whole, level),
        );
    }

    /// Checks that `level` is the whole texture's level `index`, of its size and format.
    fn check_level(&self, level: &ArchivedTextureLevel, index: u32) -> Result<(), TextureError> {
        let (found, (width, height), format) = (level.level(), level.extent(), level.format());
        if found != index || self.whole.level_extent(index) != (width, height) || format != self.whole.format
        {
            let (texture, top) = (self.whole, self.top);
            return Err(TextureError::NotALevel { level: found, width, height, format, texture, top });
        }
        Ok(())
    }

    /// This texture raised by `levels`, the levels just above its top, from the new top down: a new texture
    /// with `levels` written and the kept levels copied from this one by `encoder`. The writes go with the
    /// queue's next submission: submit `encoder` before anything reads the new texture, and keep this one
    /// until then. Several levels at once make one new texture.
    pub fn raised(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        levels: &[&ArchivedTextureLevel],
        label: Option<&str>,
    ) -> Result<Self, TextureError> {
        profiling::scope!("GpuTexture::raised");
        let count = u32::try_from(levels.len()).unwrap_or(u32::MAX);
        let Some(top) = self.top.checked_sub(count).filter(|_| count > 0) else {
            let level = levels.first().map_or(self.top, |level| level.level());
            let (width, height) = self.whole.level_extent(level);
            let (texture, format, top) = (self.whole, self.whole.format, self.top);
            return Err(TextureError::NotALevel { level, width, height, format, texture, top });
        };
        for (index, level) in (top..).zip(levels) {
            self.check_level(level, index)?;
        }
        let raised = Self::create(device, self.whole, top, self.above, label);
        copy_levels(encoder, self, &raised, self.top..self.whole.mip_level_count);
        for (index, level) in (top..).zip(levels) {
            raised.write(queue, index, level.data());
        }
        Ok(raised)
    }

    /// This texture lowered to `top`, under its own and no deeper than its tail's: a new texture that
    /// `encoder` copies the kept levels into. As with [`Self::raised`], submit `encoder` before anything
    /// reads the new one, and keep this one until then.
    pub fn lowered(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        top: u32,
        label: Option<&str>,
    ) -> Result<Self, TextureError> {
        profiling::scope!("GpuTexture::lowered");
        if top <= self.top || top > self.above {
            return Err(TextureError::CantLower { top: self.top, to: top, above: self.above });
        }
        let lowered = Self::create(device, self.whole, top, self.above, label);
        copy_levels(encoder, self, &lowered, top..self.whole.mip_level_count);
        Ok(lowered)
    }

    /// Writes `level`, one of this texture's levels above its tail, over what it holds, as hot reload does.
    /// The renderer writes it ([`crate::Renderer::rewrite_texture_level`]), after the copies into this
    /// texture it has still to submit.
    pub(crate) fn write_level(
        &self,
        queue: &wgpu::Queue,
        level: &ArchivedTextureLevel,
    ) -> Result<(), TextureError> {
        let index = level.level();
        if !(self.top..self.above).contains(&index) {
            let (width, height) = level.extent();
            let (texture, top, format) = (self.whole, self.top, level.format());
            return Err(TextureError::NotALevel { level: index, width, height, format, texture, top });
        }
        self.check_level(level, index)?;
        self.write(queue, index, level.data());
        Ok(())
    }

    /// Replaces the tail's levels in place with `tail`'s, a new version of the same shape, as hot reload
    /// does; the levels above it here stay as written. A tail of another shape needs a new texture. As
    /// with [`Self::write_level`], the renderer writes it ([`crate::Renderer::rewrite_texture_tail`]).
    pub(crate) fn write_tail(&self, queue: &wgpu::Queue, tail: &TextureTail<'_>) -> Result<(), TextureError> {
        tail.validate()?;
        if tail.whole() != self.whole || tail.above != self.above {
            return Err(TextureError::OtherShape { tail: tail.whole(), texture: self.whole });
        }
        for (level, data) in (self.above..).zip(&tail.texture.levels) {
            self.write(queue, level, data);
        }
        Ok(())
    }

    /// The texture's own shape on the GPU: the whole texture's levels from [`Self::top`] down.
    pub fn desc(&self) -> TextureDesc {
        from_level(&self.whole, self.top)
    }

    /// The whole texture's shape, its levels above [`Self::top`] included.
    pub fn whole(&self) -> TextureDesc {
        self.whole
    }

    /// The whole texture's level that is this one's level 0: 0 once it's whole.
    pub fn top(&self) -> u32 {
        self.top
    }

    /// How many of the whole texture's levels lie above the tail it was made from: the deepest
    /// [`Self::top`] can be, and 0 for a texture that can't rise.
    pub fn above(&self) -> u32 {
        self.above
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Its levels' bytes: what it counts for against a texture budget.
    pub fn bytes(&self) -> u64 {
        (self.top..self.whole.mip_level_count).map(|level| self.whole.level_byte_len(level)).sum()
    }

    /// A view of every level, for binding.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// How shaders read the texture's logical channels from what sampling it returns.
    pub fn swizzle(&self) -> Swizzle {
        self.whole.swizzle
    }
}

/// What a texture is made for: sampled and written, and copied from when a raise or a lower replaces it,
/// if it has `above` levels above its tail to rise to.
pub fn usage(above: u32) -> wgpu::TextureUsages {
    let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
    if above > 0 { usage | wgpu::TextureUsages::COPY_SRC } else { usage }
}

/// `whole` from `level` down: the shape of a texture whose top is that level.
fn from_level(whole: &TextureDesc, level: u32) -> TextureDesc {
    let (width, height) = whole.level_extent(level);
    TextureDesc { width, height, mip_level_count: whole.mip_level_count - level, ..*whole }
}

/// The whole texture's level `level`'s extent in whole blocks, as copies and writes of a compressed
/// format take it: levels smaller than a block still copy whole blocks.
fn copy_extent(whole: &TextureDesc, level: u32) -> wgpu::Extent3d {
    let (width, height) = whole.level_extent(level);
    wgpu::Extent3d { width, height, depth_or_array_layers: 1 }.physical_size(wgpu_format(whole.format))
}

/// Records copies of the whole texture's levels `levels` from `from` to `to`, which both hold them.
fn copy_levels(
    encoder: &mut wgpu::CommandEncoder,
    from: &GpuTexture,
    to: &GpuTexture,
    levels: std::ops::Range<u32>,
) {
    fn at(texture: &GpuTexture, level: u32) -> wgpu::TexelCopyTextureInfo<'_> {
        wgpu::TexelCopyTextureInfo {
            texture: &texture.texture,
            mip_level: level - texture.top,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        }
    }
    for level in levels {
        encoder.copy_texture_to_texture(at(from, level), at(to, level), copy_extent(&from.whole, level));
    }
}

/// Checks that `gpu` can sample `format` as cooked textures are sampled: filtered, with mips.
pub fn check_format(gpu: &GpuContext, format: TextureFormat) -> Result<(), TextureError> {
    check_support(format, gpu.format_support(wgpu_format(format)), &gpu.caps().adapter.name)
}

/// [`check_format`] for a device named `adapter` that reports `support` for `format`.
pub fn check_support(
    format: TextureFormat,
    support: FormatSupport,
    adapter: &str,
) -> Result<(), TextureError> {
    if support.filterable {
        return Ok(());
    }
    let reason = match format {
        TextureFormat::Bc4RUnorm
        | TextureFormat::Bc5RgUnorm
        | TextureFormat::Bc6hRgbUfloat
        | TextureFormat::Bc7RgbaUnorm
        | TextureFormat::Bc7RgbaUnormSrgb => {
            "BC formats come from the desktop cook and this device has no BC support; it needs the mobile cook (ASTC)"
        }
        TextureFormat::Astc { .. } => {
            "ASTC formats come from the mobile cook and this device has no ASTC support; it needs the desktop cook (BC)"
        }
        TextureFormat::R16Unorm | TextureFormat::Rg16Unorm => {
            "16-bit normalized formats need TEXTURE_FORMAT_16BIT_NORM; cook SDF maps at 8 bits for this device"
        }
        _ => "the device doesn't support filtering this format",
    };
    Err(TextureError::Unsupported { format, adapter: adapter.to_owned(), reason })
}

/// The wgpu format a cooked format is uploaded as.
pub fn wgpu_format(format: TextureFormat) -> wgpu::TextureFormat {
    use wgpu::TextureFormat as W;
    match format {
        TextureFormat::Rgba8UnormSrgb => W::Rgba8UnormSrgb,
        TextureFormat::R8Unorm => W::R8Unorm,
        TextureFormat::Rg8Unorm => W::Rg8Unorm,
        TextureFormat::R16Unorm => W::R16Unorm,
        TextureFormat::Rg16Unorm => W::Rg16Unorm,
        TextureFormat::Rgba16Float => W::Rgba16Float,
        TextureFormat::Rgb9e5Ufloat => W::Rgb9e5Ufloat,
        TextureFormat::Bc4RUnorm => W::Bc4RUnorm,
        TextureFormat::Bc5RgUnorm => W::Bc5RgUnorm,
        TextureFormat::Bc6hRgbUfloat => W::Bc6hRgbUfloat,
        TextureFormat::Bc7RgbaUnorm => W::Bc7RgbaUnorm,
        TextureFormat::Bc7RgbaUnormSrgb => W::Bc7RgbaUnormSrgb,
        TextureFormat::Astc { block, srgb } => W::Astc {
            block: match block {
                AstcBlock::B4x4 => wgpu::AstcBlock::B4x4,
                AstcBlock::B5x5 => wgpu::AstcBlock::B5x5,
                AstcBlock::B6x6 => wgpu::AstcBlock::B6x6,
                AstcBlock::B8x8 => wgpu::AstcBlock::B8x8,
            },
            channel: if srgb { wgpu::AstcChannel::UnormSrgb } else { wgpu::AstcChannel::Unorm },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wgpu_formats_have_the_same_layout() {
        for format in TextureFormat::ALL {
            let raw = wgpu_format(format);
            assert_eq!(raw.block_dimensions(), format.block_dimensions(), "{format:?}");
            assert_eq!(raw.block_copy_size(None), Some(format.block_bytes()), "{format:?}");
            assert_eq!(raw.is_srgb(), format.is_srgb(), "{format:?}");
            assert_eq!(raw.is_compressed(), format.is_compressed(), "{format:?}");
        }
    }

    /// A texture that can't rise is made as it was before textures streamed; one that can is also a copy's
    /// source, for the raise or lower that replaces it (M5 M0).
    #[test]
    fn only_a_texture_that_can_rise_is_a_copy_source() {
        use wgpu::TextureUsages as U;
        assert_eq!(usage(0), U::TEXTURE_BINDING | U::COPY_DST);
        assert_eq!(usage(3), U::TEXTURE_BINDING | U::COPY_DST | U::COPY_SRC);
    }

    #[test]
    fn unsupported_formats_name_the_cook_they_need() {
        let unsupported = FormatSupport::default();
        let error = check_support(TextureFormat::Bc7RgbaUnormSrgb, unsupported, "Phone GPU").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Bc7RgbaUnormSrgb") && message.contains("Phone GPU"), "{message}");
        assert!(message.contains("needs the mobile cook"), "{message}");
        let astc = TextureFormat::Astc { block: AstcBlock::B4x4, srgb: true };
        assert!(
            check_support(astc, unsupported, "Desktop GPU").unwrap_err().to_string().contains("desktop cook")
        );
        assert!(
            check_support(TextureFormat::R16Unorm, unsupported, "GPU")
                .unwrap_err()
                .to_string()
                .contains("16BIT_NORM")
        );
    }
}
