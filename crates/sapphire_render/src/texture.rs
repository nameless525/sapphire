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
//! one-off uploads, like meshes. Binding textures into materials (and applying their
//! [`Swizzle`], which wgpu views can't) is A3's; a texture pool with handles, residency and
//! the per-frame upload budget come with the asset runtime (A4).

use sapphire_assets::texture::{AstcBlock, Swizzle, Texture, TextureFormat, TextureFormatError};
use sapphire_rhi::{FormatSupport, GpuContext, wgpu};

/// A cooked texture on the GPU: every mip level, viewed as one 2D texture.
#[derive(Debug)]
pub struct GpuTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    swizzle: Swizzle,
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
}

impl GpuTexture {
    /// Validates `texture`, checks the device supports its format and size, and uploads every
    /// level. `label` names the texture in GPU captures when object labels are on.
    pub fn upload(gpu: &GpuContext, texture: &Texture<'_>, label: &str) -> Result<Self, TextureError> {
        profiling::scope!("GpuTexture::upload");
        texture.validate()?;
        let desc = texture.desc;
        check_format(gpu, desc.format)?;
        let max = gpu.device().limits().max_texture_dimension_2d;
        if desc.width > max || desc.height > max {
            return Err(TextureError::TooLarge { width: desc.width, height: desc.height, max });
        }

        let format = wgpu_format(desc.format);
        let raw = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: gpu.instance().debug().object_labels.then_some(label),
            size: wgpu::Extent3d { width: desc.width, height: desc.height, depth_or_array_layers: 1 },
            mip_level_count: desc.mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (block_width, block_height) = desc.format.block_dimensions();
        for (level, data) in (0..).zip(&texture.levels) {
            let (width, height) = desc.level_extent(level);
            let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
            gpu.queue().write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &raw,
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
                extent.physical_size(format),
            );
        }
        let view = raw.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self { texture: raw, view, swizzle: desc.swizzle })
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// A view of every level, for binding.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// How shaders read the texture's logical channels from what sampling it returns.
    pub fn swizzle(&self) -> Swizzle {
        self.swizzle
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
