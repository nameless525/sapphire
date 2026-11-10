//! Post's pipelines by variant (the module docs' "Building the variants"): which variants a
//! renderer builds ahead of its frames ([`PostVariants`]), which pipelines a frame draws with, and
//! building them.

use std::ops::{BitOr, BitOrAssign};

use sapphire_look::Dichromacy;
use sapphire_rhi::wgpu;

use super::layers::Variant;
use super::{
    CHARACTERS_CONSTANT, COLOR_EFFECTS_CONSTANT, COMPOSITE_PASS, GpuEffects, LAYERS_CONSTANT,
    LENS_EFFECTS_CONSTANT, Layouts, PostEffectSettings, PostEffects, RADIAL_TAPS_CONSTANT, SHARPEN_CONSTANT,
};
use crate::color_vision::{COLOR_VISION_CONSTANTS, COLOR_VISION_PASS};
use crate::inset::{INSET_FORMAT, INSET_RESOLVE_PASS};
use crate::pipeline_builds::{PipelineBuilds, PipelineFamily};
use crate::shader_builds::Cancel;
use crate::shaders::BuildOn;

/// A set of post's switches, the pipeline-overridable constants of `post.wgsl` that pick each
/// frame's variants (the module docs' "Variants"): the variants of a set are every combination of
/// its switches, the lens effects taking sharpening's place. A renderer builds the variants of
/// [`RenderSettings::post_variants`](crate::RenderSettings::post_variants) with itself and those
/// [`Renderer::prewarm_post`](crate::Renderer::prewarm_post) adds, ahead of the frames that draw
/// them; a frame drawing any other variant builds it as it is prepared, which stalls that frame
/// to compile ([`PostStats::first_use_builds`](super::PostStats::first_use_builds) counts them,
/// and the log warns of each).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PostVariants {
    /// `POST_LAYERS` (M1 P2): the diffusion glow, gradient layers, the lens flare or a tinted
    /// vignette.
    pub layers: bool,
    /// `POST_SHARPEN` (M1 P2): the player's sharpening.
    pub sharpening: bool,
    /// `POST_CHARACTERS` (M1 P2): characters bloom by their own threshold and strength (the bloom
    /// prefilter's switch).
    pub character_bloom: bool,
    /// `POST_LENS_EFFECTS` (M1 P3): the radial blur or chromatic aberration.
    pub lens_effects: bool,
    /// `POST_COLOR_EFFECTS` (M1 P3): the inversion, the duotone or speed lines.
    pub color_effects: bool,
}

impl PostVariants {
    /// No switch: the plain variants alone, R3's post.
    pub const NONE: Self = Self {
        layers: false,
        sharpening: false,
        character_bloom: false,
        lens_effects: false,
        color_effects: false,
    };

    /// The compositing layers, sharpening and characters' bloom (M1 P2): what time of day's looks
    /// and the player's settings turn on, at any hour. Renderers build these by default.
    pub const COMPOSITING: Self =
        Self { layers: true, sharpening: true, character_bloom: true, ..Self::NONE };

    /// The screen effects (M1 P3), lens and color: what gameplay's effect stack turns on.
    pub const SCREEN_EFFECTS: Self = Self { lens_effects: true, color_effects: true, ..Self::NONE };

    /// Every switch: all of post's variants.
    pub const ALL: Self = Self::COMPOSITING.union(Self::SCREEN_EFFECTS);

    /// The switches the screen effects of `effects` turn on: what a frame drawing them draws
    /// through. An effect that draws nothing (no strength, no coverage) turns none on.
    pub fn of_effects(effects: &PostEffects) -> Self {
        let (_, drawn) = GpuEffects::new(effects);
        Self { lens_effects: drawn.lens(), color_effects: drawn.color(), ..Self::NONE }
    }

    /// The switches either set turns on.
    pub const fn union(self, other: Self) -> Self {
        Self {
            layers: self.layers || other.layers,
            sharpening: self.sharpening || other.sharpening,
            character_bloom: self.character_bloom || other.character_bloom,
            lens_effects: self.lens_effects || other.lens_effects,
            color_effects: self.color_effects || other.color_effects,
        }
    }

    /// Whether every switch `other` turns on is in this set.
    pub const fn contains(self, other: Self) -> bool {
        (self.layers || !other.layers)
            && (self.sharpening || !other.sharpening)
            && (self.character_bloom || !other.character_bloom)
            && (self.lens_effects || !other.lens_effects)
            && (self.color_effects || !other.color_effects)
    }
}

impl BitOr for PostVariants {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl BitOrAssign for PostVariants {
    fn bitor_assign(&mut self, other: Self) {
        *self = self.union(other);
    }
}

/// One of post's pipelines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Slot {
    /// The bloom prefilter.
    Prefilter {
        /// It tells characters apart (`POST_CHARACTERS`).
        characters: bool,
    },
    Down,
    Up,
    /// The fused pass, by [`Variant::composite_index`].
    Composite(usize),
    /// An inset's resolve (M3 U6, `inset`): its scene exposed and graded into its picture. No frame
    /// variant draws it, so it is outside [`Self::all`]: inventories declare it (`PipelineInventory::insets`).
    Inset,
    /// The colour-vision debug view (M4 A5, `color_vision`): the finished frame as a dichromat sees it.
    /// Outside [`Self::all`] too, built only while the view shows.
    ColorVision(Dichromacy),
}

impl Slot {
    /// How many the frame's variants have.
    const COUNT: usize = 4 + Variant::COMPOSITES;
    /// How many there are, the inset's resolve and the colour-vision views with them.
    const SLOTS: usize = Self::COUNT + 1 + Dichromacy::ALL.len();

    /// The pipelines outside the frame's variants, which something else asks for.
    fn others() -> impl Iterator<Item = Self> {
        [Self::Inset].into_iter().chain(Dichromacy::ALL.map(Self::ColorVision))
    }

    /// Every pipeline post has, in [`Self::index`]'s order.
    fn all() -> impl Iterator<Item = Self> {
        [Self::Prefilter { characters: false }, Self::Prefilter { characters: true }, Self::Down, Self::Up]
            .into_iter()
            .chain((0..Variant::COMPOSITES).map(Self::Composite))
    }

    fn index(self) -> usize {
        match self {
            Self::Prefilter { characters } => usize::from(characters),
            Self::Down => 2,
            Self::Up => 3,
            Self::Composite(index) => 4 + index,
            Self::Inset => Self::COUNT,
            Self::ColorVision(dichromacy) => Self::COUNT + 1 + dichromacy as usize,
        }
    }

    /// The switches a frame turns on to draw with this pipeline, and the bloom levels its chain
    /// must have: the prefilter needs one, the down and up passes two.
    fn needs(self) -> (PostVariants, u32) {
        match self {
            Self::Prefilter { characters } => {
                (PostVariants { character_bloom: characters, ..PostVariants::NONE }, 1)
            }
            Self::Down | Self::Up => (PostVariants::NONE, 2),
            Self::Composite(index) => {
                let (layers, sharpening, lens_effects, color_effects) = Variant::composite(index);
                let switches =
                    PostVariants { layers, sharpening, lens_effects, color_effects, ..PostVariants::NONE };
                (switches, 0)
            }
            Self::Inset | Self::ColorVision(_) => (PostVariants::NONE, 0),
        }
    }

    /// The pipelines of the variants of `variants`, with a bloom chain of `bloom_levels`.
    pub(super) fn within(variants: PostVariants, bloom_levels: u32) -> impl Iterator<Item = Self> {
        Self::all().filter(move |slot| {
            let (switches, levels) = slot.needs();
            variants.contains(switches) && bloom_levels >= levels
        })
    }

    /// The pipelines a frame drawing `variant` draws with: the bloom chain's, of `bloom_levels`,
    /// if it runs (`bloom`), and the fused pass's.
    pub(super) fn drawn(variant: Variant, bloom: bool, bloom_levels: u32) -> impl Iterator<Item = Self> {
        let composite = variant.composite_index();
        Self::all().filter(move |&slot| match slot {
            Self::Prefilter { characters } => bloom && characters == variant.characters,
            Self::Down | Self::Up => bloom && bloom_levels >= 2,
            Self::Composite(index) => index == composite,
            Self::Inset | Self::ColorVision(_) => false,
        })
    }
}

/// What post's pipelines are built with, but `post.wgsl`'s module: their layouts (which the
/// passes' bind groups use too), the formats they write and the tier's effect settings.
#[derive(Clone, Debug)]
pub(super) struct Builder {
    pub device: wgpu::Device,
    pub shared_layout: wgpu::BindGroupLayout,
    pub layouts: Layouts,
    pub bloom_format: wgpu::TextureFormat,
    pub output_format: wgpu::TextureFormat,
    pub effects: PostEffectSettings,
    pub builds: PipelineBuilds,
}

impl Builder {
    /// Pipeline `slot` from `module` (`post.wgsl`): bloom passes write the bloom format, the fused
    /// pass the output's, its effect variants with the tier's effect settings.
    fn build(&self, module: &wgpu::ShaderModule, slot: Slot) -> wgpu::RenderPipeline {
        let switch = |on: bool| if on { 1.0 } else { 0.0 };
        let (fullscreen, two) = ("vs_fullscreen", &self.layouts.two);
        match slot {
            Slot::Prefilter { characters } => {
                let constants = [(CHARACTERS_CONSTANT, switch(characters))];
                let (layout, format) = (two, self.bloom_format);
                let names = ("bloom prefilter", fullscreen, "fs_bloom_prefilter");
                self.pipeline(module, PipelineFamily::Post, names, layout, format, &constants)
            }
            Slot::Down => {
                let (layout, format) = (&self.layouts.one, self.bloom_format);
                let names = ("bloom down", fullscreen, "fs_bloom_down");
                self.pipeline(module, PipelineFamily::Post, names, layout, format, &[])
            }
            Slot::Up => {
                let names = ("bloom up", fullscreen, "fs_bloom_up");
                self.pipeline(module, PipelineFamily::Post, names, two, self.bloom_format, &[])
            }
            // The plain variant is R3's fused pass; the others composite (and measure the flare's
            // visibility once a frame, in their vertex shader).
            Slot::Composite(index) => {
                let (layers, sharpen, lens, color) = Variant::composite(index);
                let constants = [
                    (LAYERS_CONSTANT, switch(layers)),
                    (SHARPEN_CONSTANT, switch(sharpen)),
                    (LENS_EFFECTS_CONSTANT, switch(lens)),
                    (COLOR_EFFECTS_CONSTANT, switch(color)),
                    (RADIAL_TAPS_CONSTANT, f64::from(self.effects.radial_blur_taps)),
                ];
                let (vertex, fragment, constants) = match index {
                    0 => (fullscreen, "fs_post", &[][..]),
                    _ => ("vs_composite", "fs_composite", &constants[..]),
                };
                let (layout, format) = (&self.layouts.composite, self.output_format);
                let names = (COMPOSITE_PASS, vertex, fragment);
                self.pipeline(module, PipelineFamily::Post, names, layout, format, constants)
            }
            // The composite's layout, as an inset's group binds the frame's grading LUTs.
            Slot::Inset => {
                let names = (INSET_RESOLVE_PASS, fullscreen, "fs_inset");
                let layout = &self.layouts.composite;
                self.pipeline(module, PipelineFamily::Inset, names, layout, INSET_FORMAT, &[])
            }
            // The finished frame in, by `texture0`, and the dichromacy's matrix as constants.
            Slot::ColorVision(dichromacy) => {
                let matrix = dichromacy.matrix();
                let constants: [(&str, f64); 9] = std::array::from_fn(|index| {
                    (COLOR_VISION_CONSTANTS[index], f64::from(matrix[index / 3][index % 3]))
                });
                let names = (COLOR_VISION_PASS, fullscreen, "fs_color_vision");
                let (layout, format) = (&self.layouts.one, self.output_format);
                self.pipeline(module, PipelineFamily::Debug, names, layout, format, &constants)
            }
        }
    }

    /// A fullscreen pipeline for `family` drawing one triangle into `format`, reading the shared
    /// group and `layout`, named and with entry points by `(name, vertex, fragment)`.
    fn pipeline(
        &self,
        module: &wgpu::ShaderModule,
        family: PipelineFamily,
        (name, vertex, fragment): (&str, &str, &str),
        layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        constants: &[(&str, f64)],
    ) -> wgpu::RenderPipeline {
        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(name),
            bind_group_layouts: &[Some(&self.shared_layout), Some(layout)],
            immediate_size: 0,
        });
        let compilation_options = wgpu::PipelineCompilationOptions { constants, ..Default::default() };
        self.builds.render(
            &self.device,
            family,
            &wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vertex),
                    compilation_options: compilation_options.clone(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fragment),
                    compilation_options,
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: self.builds.cache(),
            },
        )
    }
}

/// Post's pipelines built so far, by [`Slot`].
#[derive(Clone, Debug, Default)]
pub(crate) struct Pipelines {
    slots: [Option<wgpu::RenderPipeline>; Slot::SLOTS],
}

impl Pipelines {
    /// Pipeline `slot`, if it is built.
    pub(super) fn get(&self, slot: Slot) -> Option<&wgpu::RenderPipeline> {
        self.slots[slot.index()].as_ref()
    }

    /// How many are built.
    pub(crate) fn count(&self) -> u32 {
        self.slots.iter().flatten().count() as u32
    }

    /// Builds those of `slots` that aren't built yet from `module`, and says how many it built.
    pub(super) fn build(
        &mut self,
        builder: &Builder,
        module: &wgpu::ShaderModule,
        slots: impl IntoIterator<Item = Slot>,
    ) -> u32 {
        let mut built = 0;
        for slot in slots {
            let pipeline = &mut self.slots[slot.index()];
            if pipeline.is_none() {
                *pipeline = Some(builder.build(module, slot));
                built += 1;
            }
        }
        built
    }

    /// [`Self::build`] of a batch, where `on` says.
    pub(super) fn build_all(
        &mut self,
        builder: &Builder,
        module: &wgpu::ShaderModule,
        slots: impl IntoIterator<Item = Slot>,
        on: BuildOn,
    ) -> u32 {
        let missing: Vec<Slot> = slots.into_iter().filter(|&slot| self.get(slot).is_none()).collect();
        let builds = missing.iter().map(|&slot| move || builder.build(module, slot)).collect();
        for (slot, pipeline) in missing.iter().zip(on.run(builds)) {
            self.slots[slot.index()] = Some(pipeline);
        }
        missing.len() as u32
    }

    /// Takes in what `warmed` (a warm-up's clone of these) built that these lack.
    pub(crate) fn adopt(&mut self, warmed: Self) {
        for (slot, pipeline) in self.slots.iter_mut().zip(warmed.slots) {
            if slot.is_none() {
                *slot = pipeline;
            }
        }
    }

    /// Every pipeline built here, built again from `module`, unless `cancel` is set first: then
    /// what it returns is to be thrown away.
    pub(super) fn rebuilt(&self, builder: &Builder, module: &wgpu::ShaderModule, cancel: &Cancel) -> Self {
        let mut rebuilt = Self::default();
        let built = Slot::all().chain(Slot::others()).filter(|&slot| self.get(slot).is_some());
        rebuilt.build(builder, module, built.take_while(|_| !cancel.is_set()));
        rebuilt
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;
    use sapphire_look::post::{
        ChromaticAberration, Duotone, EffectCoverage, Inversion, RadialBlur, SpeedLines,
    };

    fn count(variants: PostVariants, bloom_levels: u32) -> usize {
        Slot::within(variants, bloom_levels).count()
    }

    #[test]
    fn a_set_builds_every_combination_of_its_switches() {
        // The bloom chain's three and the plain fused pass.
        assert_eq!(count(PostVariants::NONE, 5), 4);
        // P2's: the prefilter with the character mask, and the fused pass with the layers,
        // sharpened, or both.
        assert_eq!(count(PostVariants::COMPOSITING, 5), 8);
        // With the screen effects, every pipeline.
        assert_eq!(count(PostVariants::ALL, 5), Slot::COUNT);
        let all: Vec<Slot> = Slot::all().collect();
        assert!(all.iter().enumerate().all(|(index, slot)| slot.index() == index));
        let others: Vec<Slot> = Slot::others().collect();
        assert!(others.iter().zip(Slot::COUNT..).all(|(slot, index)| slot.index() == index));
        assert_eq!(Slot::COUNT + others.len(), Slot::SLOTS);
        // The effects alone: the color effects over the plain or the lens effects' scene.
        assert_eq!(count(PostVariants::SCREEN_EFFECTS, 5), 3 + 4);
        let lens = PostVariants { lens_effects: true, ..PostVariants::NONE };
        assert_eq!(count(lens, 5), 3 + 2);
        // The lens effects never combine with sharpening, so P2 and the lens effects add the
        // lens variant with and without the layers.
        assert_eq!(count(PostVariants::COMPOSITING | lens, 5), 8 + 2);
        // A shorter chain: one level has no down or up pass, none no chain at all.
        assert_eq!(count(PostVariants::NONE, 1), 2);
        assert_eq!(count(PostVariants::COMPOSITING, 0), 4);
    }

    #[test]
    fn a_frame_draws_with_pipelines_of_its_own_variant() {
        let variant = Variant { layers: true, characters: true, color_effects: true, ..Variant::default() };
        let drawn: Vec<Slot> = Slot::drawn(variant, true, 5).collect();
        let composite = Slot::Composite(variant.composite_index());
        assert_eq!(drawn, [Slot::Prefilter { characters: true }, Slot::Down, Slot::Up, composite]);
        assert_eq!(Slot::drawn(variant, false, 5).collect::<Vec<_>>(), [composite]);
        // Whatever a frame draws is within the set of its switches.
        let switches =
            PostVariants { layers: true, character_bloom: true, color_effects: true, ..PostVariants::NONE };
        let within: Vec<Slot> = Slot::within(switches, 5).collect();
        assert!(drawn.iter().all(|slot| within.contains(slot)));
    }

    #[test]
    fn effects_turn_on_the_switches_of_what_they_draw() {
        assert_eq!(PostVariants::of_effects(&PostEffects::NONE), PostVariants::NONE);
        let blur = PostEffects {
            radial_blur: RadialBlur { center: Vec2::splat(0.5), strength: 0.1 },
            ..PostEffects::NONE
        };
        let lens = PostVariants { lens_effects: true, ..PostVariants::NONE };
        assert_eq!(PostVariants::of_effects(&blur), lens);
        let aberration = ChromaticAberration { center: Vec2::splat(0.5), strength: 0.05 };
        let split = PostEffects { chromatic_aberration: aberration, ..PostEffects::NONE };
        assert_eq!(PostVariants::of_effects(&split), lens);
        let color = PostVariants { color_effects: true, ..PostVariants::NONE };
        for effects in [
            PostEffects {
                inversion: Inversion { amount: 1.0, coverage: EffectCoverage::EVERYWHERE },
                ..PostEffects::NONE
            },
            PostEffects { duotone: Duotone { amount: 1.0, ..Duotone::NONE }, ..PostEffects::NONE },
            PostEffects {
                speed_lines: SpeedLines { opacity: 1.0, width: 0.2, ..SpeedLines::NONE },
                ..PostEffects::NONE
            },
        ] {
            assert_eq!(PostVariants::of_effects(&effects), color, "{effects:?}");
        }
        // Covering nothing draws nothing.
        let hidden = Inversion { amount: 1.0, coverage: EffectCoverage { world: 0.0, characters: 0.0 } };
        assert_eq!(
            PostVariants::of_effects(&PostEffects { inversion: hidden, ..PostEffects::NONE }),
            PostVariants::NONE
        );
        let mut both = lens;
        both |= color;
        assert_eq!(both, PostVariants::SCREEN_EFFECTS);
        assert!(PostVariants::ALL.contains(both) && !both.contains(PostVariants::COMPOSITING));
        assert_eq!(PostVariants::default(), PostVariants::NONE);
    }
}
