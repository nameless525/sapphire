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
//! # Tails and levels (M5 M3)
//!
//! A large texture is cooked as a tail and the levels above it, each an asset of its own
//! (`sapphire_assets::texture`'s "Tails"). [`GpuTexture::upload`] takes a tail as the smaller texture
//! it holds. For now the app makes the whole texture instead: [`GpuTexture::upload_tail`] creates it at
//! full size and writes the tail's levels, then [`GpuTexture::write_level`] writes each level above
//! in place as it arrives, so no material rebinds, and once every level is written the texture is
//! the one a whole upload makes. Raising and lowering a texture a level at a time by copies
//! (m5-plan §3, M5 M4) starts from a tail uploaded alone.

use sapphire_assets::texture::{
    AstcBlock, Swizzle, Texture, TextureDesc, TextureFormat, TextureFormatError, TextureTail,
};
use sapphire_assets::texture_level::ArchivedTextureLevel;
use sapphire_rhi::{FormatSupport, GpuContext, wgpu};

/// A cooked texture on the GPU: every mip level, viewed as one 2D texture.
#[derive(Debug)]
pub struct GpuTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// The texture's shape on the GPU.
    desc: TextureDesc,
    /// How many of its levels, from 0, lie above the tail it was made from.
    above: u32,
    /// Which of those [`Self::write_level`] hasn't written yet, a bit each.
    missing: u32,
    /// Every written level's bytes.
    bytes: u64,
}

/// Errors from uploading a texture.
#[derive(Debug, thiserror::Error)]
pub enum TextureError {
    #[error("invalid texture")]
    Invalid(#[from] TextureFormatError),
    #[error("{format:?} textures can't be sampled with filtering on this device ({adapter}): {reason}")]
    Unsupported { format: TextureFormat, adapter: String, reason: &'static str },
    #[error("{width}x{height} is larger than this device's {max}-texel limit")]
    TooLarge { width: u32, height: u32, max: u32 },
    #[error(
        "level {level} ({width}x{height} {format:?}) isn't a level above this texture's tail ({texture:?})"
    )]
    NotALevel { level: u32, width: u32, height: u32, format: TextureFormat, texture: TextureDesc },
    #[error("a tail of {tail:?} can't replace this texture's ({texture:?}) in place")]
    OtherShape { tail: TextureDesc, texture: TextureDesc },
}

impl GpuTexture {
    /// Validates `texture`, checks the device supports its format and size, and uploads every
    /// level. `label` names the texture in GPU captures when object labels are on. A tail uploads as
    /// the smaller texture it holds.
    pub fn upload(gpu: &GpuContext, texture: &Texture<'_>, label: &str) -> Result<Self, TextureError> {
        profiling::scope!("GpuTexture::upload");
        texture.validate()?;
        Self::create(gpu, &TextureTail { texture: texture.clone(), above: 0 }, label)
    }

    /// Validates `tail`, checks the device supports its format and its whole texture's size, and
    /// creates the whole texture, writing the tail's levels. The levels above the tail are
    /// [missing](Self::is_whole) until [`Self::write_level`] writes them. A texture that stays whole
    /// uploads as [`Self::upload`] uploads it.
    pub fn upload_tail(gpu: &GpuContext, tail: &TextureTail<'_>, label: &str) -> Result<Self, TextureError> {
        profiling::scope!("GpuTexture::upload_tail");
        tail.validate()?;
        Self::create(gpu, tail, label)
    }

    /// The whole texture `tail` is the tail of, with the tail's levels written.
    fn create(gpu: &GpuContext, tail: &TextureTail<'_>, label: &str) -> Result<Self, TextureError> {
        let desc = tail.whole();
        check_format(gpu, desc.format)?;
        let max = gpu.device().limits().max_texture_dimension_2d;
        if desc.width > max || desc.height > max {
            return Err(TextureError::TooLarge { width: desc.width, height: desc.height, max });
        }

        let raw = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: gpu.instance().debug().object_labels.then_some(label),
            size: wgpu::Extent3d { width: desc.width, height: desc.height, depth_or_array_layers: 1 },
            mip_level_count: desc.mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu_format(desc.format),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = raw.create_view(&wgpu::TextureViewDescriptor::default());
        let missing = (1 << tail.above) - 1;
        let mut uploaded = Self { texture: raw, view, desc, above: tail.above, missing, bytes: 0 };
        uploaded.write_tail_levels(gpu, &tail.texture);
        Ok(uploaded)
    }

    /// Writes `tail`'s levels over this texture's below the levels above it, and counts the bytes.
    fn write_tail_levels(&mut self, gpu: &GpuContext, tail: &Texture<'_>) {
        for (level, data) in (self.above..).zip(&tail.levels) {
            self.write(gpu, level, data);
        }
        let above: u64 = (0..self.above)
            .filter(|&level| self.missing & (1 << level) == 0)
            .map(|level| {
                let (width, height) = self.desc.level_extent(level);
                self.desc.format.image_byte_len(width, height)
            })
            .sum();
        self.bytes = above + tail.levels.iter().map(|level| level.len() as u64).sum::<u64>();
    }

    /// Writes `data`, level `level`'s blocks, as it is.
    fn write(&self, gpu: &GpuContext, level: u32, data: &[u8]) {
        let desc = self.desc;
        let (block_width, block_height) = desc.format.block_dimensions();
        let (width, height) = desc.level_extent(level);
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        gpu.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width.div_ceil(block_width) * desc.format.block_bytes()),
                rows_per_image: Some(height.div_ceil(block_height)),
            },
            // Levels smaller than a block still copy whole blocks.
            extent.physical_size(wgpu_format(desc.format)),
        );
    }

    /// Writes `level`, one of the levels above the tail this texture was made from
    /// ([`Self::upload_tail`]), in place. Writing a level again replaces it, as hot reload does.
    pub fn write_level(
        &mut self,
        gpu: &GpuContext,
        level: &ArchivedTextureLevel,
    ) -> Result<(), TextureError> {
        profiling::scope!("GpuTexture::write_level");
        let (index, (width, height), format) = (level.level(), level.extent(), level.format());
        if index >= self.above
            || self.desc.level_extent(index) != (width, height)
            || format != self.desc.format
        {
            return Err(TextureError::NotALevel { level: index, width, height, format, texture: self.desc });
        }
        self.write(gpu, index, level.data());
        if self.missing & (1 << index) != 0 {
            self.missing &= !(1 << index);
            self.bytes += level.data().len() as u64;
        }
        Ok(())
    }

    /// Replaces the tail's levels in place with `tail`'s, a new version of the same shape, as hot
    /// reload does; the levels above stay as written. A tail of another shape needs a new texture.
    pub fn write_tail(&mut self, gpu: &GpuContext, tail: &TextureTail<'_>) -> Result<(), TextureError> {
        profiling::scope!("GpuTexture::write_tail");
        tail.validate()?;
        if tail.whole() != self.desc || tail.above != self.above {
            return Err(TextureError::OtherShape { tail: tail.whole(), texture: self.desc });
        }
        self.write_tail_levels(gpu, &tail.texture);
        Ok(())
    }

    /// The texture's shape on the GPU: for a tail's whole texture, its levels above included.
    pub fn desc(&self) -> TextureDesc {
        self.desc
    }

    /// Whether every level is written: always, but for a texture made from a tail whose levels
    /// above haven't all arrived.
    pub fn is_whole(&self) -> bool {
        self.missing == 0
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Every level's bytes, as uploaded: what it counts for against a texture budget.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// A view of every level, for binding.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// How shaders read the texture's logical channels from what sampling it returns.
    pub fn swizzle(&self) -> Swizzle {
        self.desc.swizzle
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
