//! 3D insets (M3 U6, spec §5.17's "render-to-texture insets"): a few instances drawn from a camera of
//! their own into a picture the interface shows, such as a character screen's showcase. The packet
//! names each ([`InsetView`]) and the instances it draws; the interface's list names the quads that
//! show it (`UiDrawList::inset_runs`). Neither knows of the other's node or tree (rule R4).
//!
//! # The passes
//!
//! An inset that draws this frame adds three passes after the main view's: its own character
//! prepass (the rim, the bangs' shadow and eyes through hair read it, as the world's characters read
//! theirs), its main pass and its resolve. They draw through the tier's pipelines, so an inset brings
//! no mesh pipeline of its own: 4× MSAA in tile memory on phones, as their main pass has, and no MSAA
//! on desktop, whose temporal AA has no history to give a picture that holds still. Desktop
//! supersamples instead ([`InsetSettings::supersample`]): its scene is twice the picture's width and
//! height, and the resolve's one bilinear tap at each pixel's center is the box of four under it.
//! The resolve exposes by the inset's exposure and grades through the frame's LUTs into the picture,
//! sRGB-encoded 8-bit RGBA, which is all the interface samples. It is post's one pipeline more
//! (`fs_inset`, `PipelineFamily::Inset`), which an inventory declares (`PipelineInventory::insets`).
//!
//! An inset's instances stand in its view (`MeshDraws::stands`): every pass of the world's skips
//! them, shadow maps and debug views included, and its passes draw them alone. Its group 0 is the
//! world's but for its constants (its ambient light and no fog) and its character prepass's targets;
//! its view is a slot of its own after the shadow views. A character in an inset lights by its own
//! entry, resolved for the inset's camera, under the inset's ambient light. It still reads the
//! world's shadow maps, probe grid and local lights where it stands, so a showcase stands far from
//! all of them.
//!
//! # Drawing only on a change
//!
//! An inset draws only in a frame where what it shows changed: its view (camera, size, exposure,
//! ambient, backdrop), its instances' deltas, the poses sent for them, their character entries, or
//! the grade. Otherwise its picture holds, costing the interface's one draw. A frame that never ran
//! leaves every picture it would have drawn to draw again, and so does replacing what drew one
//! (M3 Z5c): a shader reload's pipelines (or the fallback to the compiled-in shaders), a mesh or
//! material reloaded in place or adopted, or a probe grid set. Each is caught where the renderer
//! replaces it, not compared each frame. Time alone doesn't move a held picture, so a breathing
//! emission or an eye highlight's drift stops while nothing else moves.
//!
//! # Memory
//!
//! An inset holds its picture and its character prepass's targets while frames show it, and gives
//! them up in the first frame that doesn't: [`InsetStats::held_bytes`]. A frame that draws it takes
//! its scene color and depth from the transient pool ([`InsetStats::drawn_bytes`]; on phones the MSAA
//! attachments stay in tile memory).

use std::hash::{DefaultHasher, Hasher};

use bevy_reflect::Reflect;
use glam::{Affine3A, Vec3};
use sapphire_rhi::wgpu;
use serde::{Deserialize, Serialize};

use crate::character_prepass::{CharacterPrepassTargets, IDS_FORMAT};
use crate::gpu_types::{FrameUniforms, ViewUniforms};
use crate::pipelines::{DEPTH_FORMAT, SCENE_COLOR_FORMAT};
use crate::post::{InsetPost, PostProcess, PostUniforms};
use crate::renderer::{FrameBindings, create_frame_bind_group};
use crate::scene::InstanceSlots;
use crate::ui::Ui;
use crate::{CameraData, CharacterInstance, FramePacket, InsetView, PacketRange};

/// The most insets a frame draws. Past them, insets are left out and counted
/// ([`InsetStats::over_limit`]).
pub const MAX_INSETS: usize = 2;

/// An inset's picture: what the interface samples, in display encoding, as the target is.
pub(crate) const INSET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The frame graph passes an inset adds, for GPU timings.
pub const INSET_CHARACTER_PASS: &str = "inset characters";
pub const INSET_MAIN_PASS: &str = "inset main";
pub const INSET_RESOLVE_PASS: &str = "inset resolve";
/// All of them, in the order they run.
pub const PASSES: [&str; 3] = [INSET_CHARACTER_PASS, INSET_MAIN_PASS, INSET_RESOLVE_PASS];

/// How large an inset's picture and scene are on a tier ([`RenderSettings::inset`](crate::RenderSettings::inset)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(deny_unknown_fields)]
pub struct InsetSettings {
    /// The longest side of a picture in pixels, 64..=2048: a larger node shows its picture stretched.
    pub max_side: u32,
    /// The scene's width and height over the picture's, 1 or 2: 2 on desktop, whose main pass has no
    /// MSAA; 1 on phones, which draw insets with MSAA as their main pass.
    pub supersample: u32,
}

impl Default for InsetSettings {
    /// The desktop tiers': pictures up to 960 pixels, supersampled.
    fn default() -> Self {
        Self { max_side: 960, supersample: 2 }
    }
}

impl InsetSettings {
    /// Checks the ranges, returning the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if !(64..=2048).contains(&self.max_side) {
            return Err(("inset.max_side", "must be within 64..=2048"));
        }
        if !(1..=2).contains(&self.supersample) {
            return Err(("inset.supersample", "must be 1 or 2"));
        }
        Ok(())
    }

    /// The picture's size for a node covering `size` pixels, and the scene's: the node's, scaled down
    /// to [`Self::max_side`] if it is longer, then times [`Self::supersample`]. `None` for an empty one.
    pub fn sizes(&self, (width, height): (u32, u32)) -> Option<((u32, u32), (u32, u32))> {
        if width == 0 || height == 0 {
            return None;
        }
        let longest = width.max(height);
        let picture = if longest > self.max_side {
            let scale = f64::from(self.max_side) / f64::from(longest);
            let side = |pixels: u32| ((f64::from(pixels) * scale).round() as u32).max(1);
            (side(width), side(height))
        } else {
            (width, height)
        };
        let factor = self.supersample.clamp(1, 2);
        Some((picture, (picture.0 * factor, picture.1 * factor)))
    }

    /// The bytes an inset of these sizes holds while shown: its picture, and its character prepass's
    /// depth and IDs at the scene's size.
    pub fn held_bytes(picture: (u32, u32), scene: (u32, u32)) -> u64 {
        let pixels = |(width, height): (u32, u32)| u64::from(width) * u64::from(height);
        let character = bytes_per_pixel(DEPTH_FORMAT) + bytes_per_pixel(IDS_FORMAT);
        pixels(picture) * bytes_per_pixel(INSET_FORMAT) + pixels(scene) * character
    }

    /// The transient bytes a frame drawing an inset of scene size `scene` takes, at `samples` per
    /// pixel: its scene color, and its main pass's depth and color under MSAA.
    pub fn drawn_bytes(scene: (u32, u32), samples: u32) -> u64 {
        let pixels = u64::from(scene.0) * u64::from(scene.1);
        let color = bytes_per_pixel(SCENE_COLOR_FORMAT);
        let multisampled = if samples > 1 { color * u64::from(samples) } else { 0 };
        pixels * (color + multisampled + bytes_per_pixel(DEPTH_FORMAT) * u64::from(samples))
    }
}

fn bytes_per_pixel(format: wgpu::TextureFormat) -> u64 {
    u64::from(format.block_copy_size(None).unwrap_or(4))
}

/// What the insets did in the last prepared frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InsetStats {
    /// Insets the frame shows.
    pub shown: u32,
    /// Of them, those it draws, for a change; the others hold their pictures.
    pub drawn: u32,
    /// Insets left out: past [`MAX_INSETS`], a second view of one number, or of no size.
    pub over_limit: u32,
    /// Pictures and character prepass targets the shown insets hold.
    pub held_bytes: u64,
    /// The transient bytes the drawn ones take this frame.
    pub drawn_bytes: u64,
}

/// One inset's GPU state, kept while frames show it.
#[derive(Debug)]
struct InsetTarget {
    /// The number the packet and the interface name it by.
    inset: u16,
    picture_size: (u32, u32),
    scene_size: (u32, u32),
    frame_uniforms: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    character_prepass: CharacterPrepassTargets,
    picture: wgpu::Texture,
    picture_view: wgpu::TextureView,
    /// The interface's group reading the picture, once its pipeline is built.
    ui_group: Option<wgpu::BindGroup>,
    post_buffer: wgpu::Buffer,
    post: InsetPost,
    /// Named by this frame's packet.
    live: bool,
    /// Draws this frame.
    draw: bool,
    constants: FrameUniforms,
    view: ViewUniforms,
    post_uniforms: PostUniforms,
    backdrop: Vec3,
    /// What its picture shows, once drawn: the view without its range, its characters' entries, the
    /// last pose sent and the grade. `None` until drawn, and after a frame that never ran.
    drawn: Option<(InsetView, [u32; 8], u64)>,
    characters: Vec<CharacterInstance>,
    /// This frame's characters, swapped with `characters` when it draws.
    scratch: Vec<CharacterInstance>,
}

/// What the world's frame lends an inset's group 0, but its constants and character targets.
pub(crate) struct InsetBindings<'a> {
    pub device: &'a wgpu::Device,
    pub layout: &'a wgpu::BindGroupLayout,
    pub world: FrameBindings<'a>,
    pub post: &'a PostProcess,
    pub ui: &'a Ui,
    pub labels: bool,
}

/// The renderer's insets. See the module docs.
#[derive(Debug)]
pub(crate) struct Insets {
    settings: InsetSettings,
    targets: Vec<InsetTarget>,
    /// Per instance slot: 0 for the world's view, `n` for target `n - 1` (`MeshDraws::stands`).
    stands: Vec<u16>,
    /// Per target, its camera, for the outline cull.
    cameras: Vec<CameraData>,
    stats: InsetStats,
}

impl Insets {
    pub(crate) fn new(settings: InsetSettings) -> Self {
        Self {
            settings,
            targets: Vec::new(),
            stands: Vec::new(),
            cameras: Vec::new(),
            stats: InsetStats::default(),
        }
    }

    /// Takes the packet's insets: makes, resizes or drops their targets, says which instance slots
    /// stand in them, and decides which draw. `world` is the main view's constants, which an inset's
    /// take but for its ambient light and fog. The world's group 0 made again makes every inset's again
    /// there ([`Self::rebind`]), so here only a resize does. Allocates nothing once its targets and
    /// vectors have grown, unless a target is made or resized.
    pub(crate) fn prepare(
        &mut self,
        packet: &FramePacket,
        slots: &InstanceSlots,
        world: &FrameUniforms,
        bindings: &InsetBindings<'_>,
    ) {
        profiling::scope!("Insets::prepare");
        self.stats = InsetStats::default();
        for target in &mut self.targets {
            target.live = false;
            target.draw = false;
        }
        for view in &packet.insets {
            let live = self.targets.iter().filter(|target| target.live).count();
            let sizes = self.settings.sizes(view.size);
            let taken = self.targets.iter().any(|target| target.live && target.inset == view.inset);
            let (Some((picture, scene)), false, true) = (sizes, taken, live < MAX_INSETS) else {
                self.stats.over_limit += 1;
                continue;
            };
            let index = match self.targets.iter().position(|target| target.inset == view.inset) {
                Some(index) => index,
                None => {
                    self.targets.push(InsetTarget::new(view.inset, bindings));
                    self.targets.len() - 1
                }
            };
            let target = &mut self.targets[index];
            target.live = true;
            target.fit(picture, scene, bindings);
        }
        self.targets.retain(|target| target.live);

        self.stands.clear();
        self.stands.resize(slots.len(), 0);
        self.cameras.clear();
        let grade = bindings.post.inset_grade();
        let views = packet.insets.iter();
        for (index, target) in self.targets.iter_mut().enumerate() {
            let Some(view) = views.clone().find(|view| view.inset == target.inset) else { continue };
            let instances = packet.inset_instances(view).unwrap_or_default();
            let stand = index as u16 + 1;
            for &id in instances {
                if let Some(slot) = slots.slot(id) {
                    self.stands[slot as usize] = stand;
                }
            }
            self.cameras.push(view.camera);
            target.plan(packet, view, instances, world, grade, bindings.post);
            self.stats.shown += 1;
            self.stats.drawn += u32::from(target.draw);
            self.stats.held_bytes += InsetSettings::held_bytes(target.picture_size, target.scene_size);
        }
    }

    /// Per instance slot, the view it stands in (`MeshDraws::stands`), and each inset's camera.
    pub(crate) fn stands(&self) -> crate::outlines::Insets<'_> {
        crate::outlines::Insets { stands: &self.stands, cameras: &self.cameras }
    }

    /// Whether the frame shows any inset.
    pub(crate) fn any(&self) -> bool {
        !self.targets.is_empty()
    }

    /// The last prepared frame's, with the transient bytes of `samples` per pixel.
    pub(crate) fn stats(&self, samples: u32) -> InsetStats {
        let drawn = self.targets.iter().filter(|target| target.draw);
        let drawn_bytes = drawn.map(|target| InsetSettings::drawn_bytes(target.scene_size, samples)).sum();
        InsetStats { drawn_bytes, ..self.stats }
    }

    /// Every held picture draws again in the next frame that shows it: what drew it was replaced, a
    /// pipeline by a shader reload, or a mesh, material or probe grid by a content reload. The
    /// renderer calls it where each is replaced, so a frame with no reload pays nothing for it.
    pub(crate) fn redraw(&mut self) {
        for target in &mut self.targets {
            target.drawn = None;
        }
    }

    /// Every picture the last frame would have drawn draws again: it never ran.
    pub(crate) fn frame_lost(&mut self) {
        for target in self.targets.iter_mut().filter(|target| target.draw) {
            target.drawn = None;
        }
    }

    /// Makes each inset's group 0 again, around the world's resources just replaced.
    pub(crate) fn rebind(&mut self, bindings: &InsetBindings<'_>) {
        for target in &mut self.targets {
            target.frame_group = target.frame_group(bindings);
        }
    }

    /// This frame's insets for its passes: the uploads, those that draw, and those it shows, by
    /// target, each view at `first_view` onward in the view uniforms, `view_stride` apart.
    pub(crate) fn split_for_record(&mut self, first_view: u32, view_stride: u32) -> InsetFrame<'_> {
        let mut frame = InsetFrame { stands: &self.stands, ..InsetFrame::default() };
        for (index, target) in self.targets.iter_mut().enumerate() {
            let InsetTarget {
                inset,
                scene_size,
                ref frame_uniforms,
                ref frame_group,
                ref character_prepass,
                ref picture_view,
                ref ui_group,
                ref post_buffer,
                ref mut post,
                draw,
                constants,
                view,
                post_uniforms,
                backdrop,
                ..
            } = *target;
            let view_offset = view_stride * (first_view + index as u32);
            frame.shown[index] = Some(InsetShown { inset, group: ui_group.as_ref(), picture: picture_view });
            if !draw {
                continue;
            }
            frame.uploads[index] = Some(InsetUpload {
                frame: frame_uniforms,
                constants,
                view,
                view_offset: u64::from(view_offset),
                post: post_buffer,
                post_uniforms,
            });
            frame.draws[index] = Some(InsetDraw {
                stand: index as u16 + 1,
                view_offset,
                frame_group,
                character_prepass,
                post,
                scene_size,
                backdrop: wgpu::Color {
                    r: f64::from(backdrop.x),
                    g: f64::from(backdrop.y),
                    b: f64::from(backdrop.z),
                    a: 1.0,
                },
            });
        }
        frame
    }
}

impl InsetTarget {
    /// A new target for inset `inset`, 1 × 1 until fitted.
    fn new(inset: u16, bindings: &InsetBindings<'_>) -> Self {
        let device = bindings.device;
        let frame_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: bindings.labels.then_some("inset frame uniforms"),
            size: size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let character_prepass = CharacterPrepassTargets::new(device);
        let frame_group = create_frame_bind_group(
            device,
            bindings.layout,
            FrameBindings {
                uniforms: &frame_uniforms,
                character_prepass: &character_prepass,
                ..bindings.world
            },
        );
        let picture = picture_texture(device, (1, 1), bindings.labels);
        let (post_buffer, post) = bindings.post.inset_post();
        Self {
            inset,
            picture_size: (1, 1),
            scene_size: (1, 1),
            frame_uniforms,
            frame_group,
            character_prepass,
            picture_view: picture.create_view(&Default::default()),
            picture,
            ui_group: None,
            post_buffer,
            post,
            live: true,
            draw: false,
            constants: bytemuck::Zeroable::zeroed(),
            view: bytemuck::Zeroable::zeroed(),
            post_uniforms: PostUniforms::default(),
            backdrop: Vec3::ZERO,
            drawn: None,
            characters: Vec::new(),
            scratch: Vec::new(),
        }
    }

    fn frame_group(&self, bindings: &InsetBindings<'_>) -> wgpu::BindGroup {
        let target = FrameBindings {
            uniforms: &self.frame_uniforms,
            character_prepass: &self.character_prepass,
            ..bindings.world
        };
        create_frame_bind_group(bindings.device, bindings.layout, target)
    }

    /// Sizes its picture and targets, and makes again the groups that bind what changed.
    fn fit(&mut self, picture: (u32, u32), scene: (u32, u32), bindings: &InsetBindings<'_>) {
        let resized = self.character_prepass.resize(bindings.device, scene);
        if resized {
            self.frame_group = self.frame_group(bindings);
        }
        if (self.picture.width(), self.picture.height()) != picture {
            self.picture = picture_texture(bindings.device, picture, bindings.labels);
            self.picture_view = self.picture.create_view(&Default::default());
            self.ui_group = None;
            self.drawn = None;
        }
        if resized {
            self.drawn = None;
        }
        if self.ui_group.is_none() {
            self.ui_group = bindings.ui.picture_group(&self.picture_view);
        }
        (self.picture_size, self.scene_size) = (picture, scene);
    }

    /// Decides whether it draws this frame, and if so what it draws with.
    fn plan(
        &mut self,
        packet: &FramePacket,
        view: &InsetView,
        instances: &[crate::InstanceId],
        world: &FrameUniforms,
        grade: [u32; 8],
        post: &PostProcess,
    ) {
        let named = |id: &crate::InstanceId| instances.contains(id);
        self.scratch.clear();
        self.scratch.extend(packet.characters.iter().filter(|character| named(&character.id)).copied());
        // The poses sent this frame, by their matrices' and weights' bits.
        let mut hasher = DefaultHasher::new();
        let mut posed = false;
        for skinned in packet.skinned.iter().filter(|skinned| named(&skinned.id)) {
            posed = true;
            hasher.write_u64(skinned.id.0);
            let palette = packet.palettes.get(skinned.palette.range()).unwrap_or_default();
            for matrix in palette {
                write_floats(&mut hasher, &Affine3A::to_cols_array(matrix));
            }
            write_floats(
                &mut hasher,
                packet.morph_weights.get(skinned.morph_weights.range()).unwrap_or_default(),
            );
        }
        let deltas = &packet.instances;
        let moved =
            deltas.changed.iter().any(|instance| named(&instance.id)) || deltas.removed.iter().any(named);
        let key = InsetView { instances: PacketRange::default(), ..*view };
        let draw = match self.drawn {
            None => true,
            Some((drawn, drawn_grade, pose)) => {
                drawn != key
                    || drawn_grade != grade
                    || moved
                    || (posed && hasher.finish() != pose)
                    || self.characters != self.scratch
            }
        };
        self.draw = draw;
        if !draw {
            return;
        }
        let pose = if posed { hasher.finish() } else { self.drawn.map_or(0, |(_, _, pose)| pose) };
        self.drawn = Some((key, grade, pose));
        std::mem::swap(&mut self.characters, &mut self.scratch);
        let ambient = view.ambient;
        self.constants = FrameUniforms {
            ambient_sky: (ambient.sky * ambient.intensity).extend(0.0).to_array(),
            ambient_ground: (ambient.ground * ambient.intensity).extend(0.0).to_array(),
            fog: crate::fog::GpuFog::new(&sapphire_look::Fog::default()),
            ..*world
        };
        let camera = view.camera;
        self.view = ViewUniforms::new(&camera, self.scene_size, camera.proj * camera.view, [0.0; 2]);
        self.post_uniforms = post.inset_uniforms(view.exposure);
        self.backdrop = if view.backdrop.is_finite() { view.backdrop.max(Vec3::ZERO) } else { Vec3::ZERO };
    }
}

fn write_floats(hasher: &mut DefaultHasher, floats: &[f32]) {
    for value in floats {
        hasher.write_u32(value.to_bits());
    }
}

fn picture_texture(device: &wgpu::Device, (width, height): (u32, u32), labels: bool) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: labels.then_some("inset picture"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: INSET_FORMAT,
        // Copyable, so tools and tests can read it back.
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// This frame's insets, by target ([`Insets::split_for_record`]).
#[derive(Default)]
pub(crate) struct InsetFrame<'f> {
    /// Per instance slot, the view it stands in (`MeshDraws::stands`).
    pub stands: &'f [u16],
    pub uploads: [Option<InsetUpload<'f>>; MAX_INSETS],
    pub draws: [Option<InsetDraw<'f>>; MAX_INSETS],
    pub shown: [Option<InsetShown<'f>>; MAX_INSETS],
}

/// What the uploads pass writes for an inset that draws.
pub(crate) struct InsetUpload<'f> {
    frame: &'f wgpu::Buffer,
    constants: FrameUniforms,
    view: ViewUniforms,
    view_offset: u64,
    post: &'f wgpu::Buffer,
    post_uniforms: PostUniforms,
}

impl InsetUpload<'_> {
    /// Stages its constants, its view into `views` and its post constants.
    pub(crate) fn write(
        &self,
        belt: &mut wgpu::util::StagingBelt,
        encoder: &mut wgpu::CommandEncoder,
        views: &wgpu::Buffer,
    ) {
        crate::buffer::stage(belt, encoder, self.frame, 0, bytemuck::bytes_of(&self.constants));
        crate::buffer::stage(belt, encoder, views, self.view_offset, bytemuck::bytes_of(&self.view));
        crate::buffer::stage(belt, encoder, self.post, 0, bytemuck::bytes_of(&self.post_uniforms));
    }
}

/// An inset that draws this frame, for its passes.
pub(crate) struct InsetDraw<'f> {
    /// What its instances stand in (`MeshDraws::stand`).
    pub stand: u16,
    /// Its view's offset in the view uniforms.
    pub view_offset: u32,
    pub frame_group: &'f wgpu::BindGroup,
    pub character_prepass: &'f CharacterPrepassTargets,
    pub post: &'f mut InsetPost,
    pub scene_size: (u32, u32),
    /// Its main pass's clear: the backdrop.
    pub backdrop: wgpu::Color,
}

/// An inset the frame shows, for the interface.
#[derive(Clone, Copy)]
pub(crate) struct InsetShown<'f> {
    pub inset: u16,
    /// The interface's group reading its picture, once its pipeline is built.
    pub group: Option<&'f wgpu::BindGroup>,
    pub picture: &'f wgpu::TextureView,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_fits_the_tier_and_the_scene_supersamples_it() {
        let desktop = InsetSettings::default();
        assert_eq!(desktop.sizes((400, 600)), Some(((400, 600), (800, 1200))));
        // Longer than the tier's side: scaled down, keeping its shape.
        assert_eq!(desktop.sizes((640, 1920)), Some(((320, 960), (640, 1920))));
        let phone = InsetSettings { max_side: 512, supersample: 1 };
        assert_eq!(phone.sizes((1024, 512)), Some(((512, 256), (512, 256))));
        assert_eq!(phone.sizes((0, 512)), None, "an empty node shows nothing");
        assert_eq!(phone.sizes((5000, 1)), Some(((512, 1), (512, 1))), "never below a pixel");
    }

    #[test]
    fn settings_out_of_range_are_refused() {
        assert_eq!(InsetSettings::default().validate(), Ok(()));
        let small = InsetSettings { max_side: 32, ..InsetSettings::default() };
        assert_eq!(small.validate(), Err(("inset.max_side", "must be within 64..=2048")));
        let thrice = InsetSettings { supersample: 3, ..InsetSettings::default() };
        assert_eq!(thrice.validate(), Err(("inset.supersample", "must be 1 or 2")));
    }

    #[test]
    fn memory_counts_the_picture_and_the_character_targets() {
        // A 4-byte picture texel, and 4 bytes of depth and 4 of IDs per scene pixel.
        assert_eq!(InsetSettings::held_bytes((10, 10), (20, 20)), 400 + 400 * 8);
        // 8 bytes of scene color and 4 of depth, then 4× MSAA's color and depth.
        assert_eq!(InsetSettings::drawn_bytes((10, 10), 1), 100 * 12);
        assert_eq!(InsetSettings::drawn_bytes((10, 10), 4), 100 * (8 + 32 + 16));
    }
}
