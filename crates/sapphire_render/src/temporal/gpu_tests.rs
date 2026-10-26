//! Temporal AA tests that read the history's accumulation weight back: how many frames' worth of
//! samples each pixel carries, which shows directly where the resolve kept or dropped its
//! history. They need a GPU: run them with `cargo test -p sapphire_render --lib -- --ignored`.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};
use sapphire_assets::material::{AnimeCharacterMaterial, CharacterRegion, Material, Outline, Rim};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, MorphDelta, MorphTarget, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex,
    VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use super::tuning;
use crate::graph::{FrameGraph, GraphExecutor};
use crate::{
    CameraData, CharacterBody, CharacterFace, CharacterInstance, CharacterLighting, FramePacketRing,
    InstanceId, MeshHandle, MeshInstance, Msaa, RenderSettings, Renderer, ViewData,
};

const SIZE: (u32, u32) = (320, 180);
/// Pixels per world unit at z = 0 for [`camera`].
const PIXELS_PER_UNIT: f32 = 90.0;

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// A unit sphere in one flat color, with an outline `outline_px` wide at 1080 rows. `character`
/// adds a morph target of zero deltas, as a character's mesh would have (the frame lists the
/// characters, [`Frames::characters`]).
fn outlined_sphere(character: bool, outline_px: f32) -> Mesh {
    let mut mesh = sphere(character);
    let material = AnimeCharacterMaterial {
        base_color: [0.8, 0.5, 0.3, 1.0],
        regions: vec![CharacterRegion {
            shade_color: [1.0; 3],
            outline_color: [0.0; 3],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: outline_px, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    mesh.materials =
        vec![MaterialSlot::embedded("sphere", Material::from_document(&material, &swizzles).expect("valid"))];
    mesh
}

/// A unit sphere in one flat color, without an outline (the outline hull is responsive
/// anywhere, which would hide what these tests look for). `character` adds a morph target of
/// zero deltas, as a character's mesh would have (the frame lists the characters,
/// [`Frames::characters`]).
fn sphere(character: bool) -> Mesh {
    const RINGS: u32 = 32;
    const SEGMENTS: u32 = 48;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(Vertex {
                position: n.to_array(),
                normal: n.to_array(),
                outline_normal: n.to_array(),
                uv: [0.0, 0.0],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            });
        }
    }
    let stride = SEGMENTS + 1;
    let mut indices = Vec::new();
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            indices.extend([a, a + 1, a + stride, a + 1, a + stride + 1, a + stride]);
        }
    }
    let material = AnimeCharacterMaterial {
        base_color: [0.8, 0.5, 0.3, 1.0],
        regions: vec![CharacterRegion { shade_color: [1.0; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let slot =
        MaterialSlot::embedded("sphere", Material::from_document(&material, &swizzles).expect("valid"));
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let mut mesh = Mesh::from_vertices(&vertices, indices, submeshes, vec![slot], PositionFormat::Float32);
    if character {
        mesh.morph_targets =
            vec![MorphTarget { name: "still".into(), default_weight: 0.0, first_delta: 0, delta_count: 1 }];
        mesh.morph_deltas = vec![MorphDelta::new(0, [0.0; 3], [0.0; 3])];
    }
    mesh
}

fn upload(renderer: &mut Renderer, mesh: &Mesh) -> MeshHandle {
    let bytes = mesh.to_cooked().expect("cook");
    let mesh = Mesh::read(&bytes).expect("valid");
    let materials = renderer.upload_embedded_materials(mesh).expect("materials");
    renderer.upload_mesh(mesh, &materials).expect("upload")
}

/// Two world units fill the image's height at z = 0.
fn camera() -> CameraData {
    let fov = 30f32.to_radians();
    let eye = Vec3::new(0.0, 0.0, 1.0 / (fov * 0.5).tan());
    CameraData::look_at(eye, Vec3::ZERO, Vec3::Y, fov, SIZE.0 as f32 / SIZE.1 as f32, 0.05)
}

fn placed(id: u64, mesh: MeshHandle, x: f32) -> MeshInstance {
    let transform = Mat4::from_translation(Vec3::new(x, 0.0, 0.0)) * Mat4::from_scale(Vec3::splat(0.3));
    MeshInstance { id: InstanceId(id), mesh, transform }
}

struct Frames {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    /// The instances each frame lists as characters: what the character prepass draws.
    characters: Vec<InstanceId>,
}

impl Frames {
    fn new(gpu: &GpuContext) -> Self {
        let settings = RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() };
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            renderer: Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, settings).expect("renderer"),
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            characters: Vec::new(),
        }
    }

    fn frame(&mut self, gpu: &GpuContext, view: ViewData, changed: &[MeshInstance]) {
        let packet = self.packets.begin();
        packet.views.push(view);
        packet.instances.changed.extend_from_slice(changed);
        let lighting = CharacterLighting::unstyled(packet.sun);
        packet.characters.extend(self.characters.iter().map(|&id| CharacterInstance {
            id,
            lighting,
            face: CharacterFace::default(),
            body: CharacterBody::default(),
            opacity: 1.0,
        }));
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none());
    }

    /// The accumulation weight of every pixel of the last frame's history: its alpha.
    fn weights(&self, gpu: &GpuContext) -> Vec<f32> {
        let history = self.renderer.temporal().expect("temporal AA").history();
        let padded = (SIZE.0 * 8).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * SIZE.1),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device().create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            history.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(SIZE.1),
                },
            },
            history.size(),
        );
        gpu.queue().submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let mapped = buffer.get_mapped_range(..).expect("mapped");
        mapped
            .chunks_exact(padded as usize)
            .flat_map(|row| row[..SIZE.0 as usize * 8].as_chunks::<8>().0)
            .map(|texel| half::f16::from_le_bytes([texel[6], texel[7]]).to_f32())
            .collect()
    }
}

/// The weight at world position (`x`, `y`) in the plane z = 0.
fn at(weights: &[f32], x: f32, y: f32) -> f32 {
    let column = (SIZE.0 as f32 / 2.0 + x * PIXELS_PER_UNIT) as usize;
    let row = (SIZE.1 as f32 / 2.0 - y * PIXELS_PER_UNIT) as usize;
    weights[row * SIZE.0 as usize + column]
}

#[test]
#[ignore = "needs a GPU"]
fn disoccluded_pixels_restart_their_accumulation() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu);
    let ball = upload(&mut frames.renderer, &sphere(false));
    let view = ViewData::new(camera());
    // Still for a while, so everything carries a full history, then 6 pixels to the right.
    for _ in 0..24 {
        frames.frame(&gpu, view, &[placed(0, ball, -0.5)]);
    }
    let step = 6.0 / PIXELS_PER_UNIT;
    frames.frame(&gpu, view, &[placed(0, ball, -0.5 + step)]);
    let weights = frames.weights(&gpu);
    // The sky the sphere just uncovered (its left edge moved from about -0.8 to -0.733) starts
    // over with this frame's samples alone: at most one frame's weight. (Nearer the new edge,
    // velocity dilation lends pixels the sphere's motion.) Sky it never covered keeps its
    // history.
    for x in [-0.785, -0.775, -0.765] {
        let weight = at(&weights, x, 0.0);
        assert!(weight <= 1.0, "uncovered at x = {x}: {weight}");
    }
    let untouched = at(&weights, -1.5, 0.0);
    assert!(untouched > 10.0, "never covered: {untouched}");
    // The sphere itself moved with its velocity and kept its history, down to the moving cap.
    let moved = at(&weights, -0.5 + step, 0.0);
    assert!(moved > tuning::HISTORY_MOVING * 0.5, "the sphere: {moved}");
}

#[test]
#[ignore = "needs a GPU"]
fn pixels_an_outline_leaves_restart_their_accumulation() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu);
    // 18 px at 1080 rows: 3 px here. The scene prepass draws the hull, so last frame's depth
    // shows where it was.
    let ball = upload(&mut frames.renderer, &outlined_sphere(false, 18.0));
    let view = ViewData::new(camera());
    for _ in 0..24 {
        frames.frame(&gpu, view, &[placed(0, ball, 0.0)]);
    }
    frames.frame(&gpu, view, &[placed(0, ball, 6.0 / PIXELS_PER_UNIT)]);
    let weights = frames.weights(&gpu);
    // The outline lay 0 to 3 px outside the left silhouette (about x = -0.302); now the
    // silhouette is 6 px further right, so the old outline is bare sky, out of reach of the
    // sphere's dilated velocity. Its inner pixels, whose 3 × 3 was all outline and sphere last
    // frame, start over; the outermost, beside the sky a jittered edge reaches, keeps its
    // history for the clip box to sort out.
    for x in [-0.32, -0.315, -0.31] {
        let weight = at(&weights, x, 0.0);
        assert!(weight <= 1.0, "the outline just left x = {x}: {weight}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn moving_characters_keep_less_history_than_other_geometry() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu);
    let character = upload(&mut frames.renderer, &sphere(true));
    frames.characters.push(InstanceId(0));
    let ball = upload(&mut frames.renderer, &sphere(false));
    let view = ViewData::new(camera());
    // Both move 3 pixels a frame, then hold still.
    let step = 3.0 / PIXELS_PER_UNIT;
    for frame in 0..12 {
        let x = step * frame as f32;
        frames.frame(&gpu, view, &[placed(0, character, -0.9 + x), placed(1, ball, 0.3 + x)]);
    }
    let moving = frames.weights(&gpu);
    let (character_weight, other_weight) =
        (at(&moving, -0.9 + step * 11.0, 0.0), at(&moving, 0.3 + step * 11.0, 0.0));
    eprintln!("moving: character {character_weight}, other {other_weight}");
    // The moving cap holds both; a character keeps the responsive share of it.
    assert!(other_weight <= tuning::HISTORY_MOVING + 1.0 && other_weight > tuning::HISTORY_MOVING * 0.75);
    assert!(
        character_weight <= tuning::HISTORY_MOVING * tuning::RESPONSIVE_HISTORY + 1.0,
        "character {character_weight} against {other_weight}"
    );
    for _ in 0..40 {
        frames.frame(&gpu, view, &[]);
    }
    let still = frames.weights(&gpu);
    let (character_weight, other_weight) =
        (at(&still, -0.9 + step * 11.0, 0.0), at(&still, 0.3 + step * 11.0, 0.0));
    eprintln!("still: character {character_weight}, other {other_weight}");
    // Holding still, a character accumulates like everything else.
    for weight in [character_weight, other_weight] {
        assert!(weight > tuning::HISTORY_STILL - 1.0, "{weight}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_cut_restarts_every_pixel() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu);
    let ball = upload(&mut frames.renderer, &sphere(false));
    let view = ViewData::new(camera());
    frames.frame(&gpu, view, &[placed(0, ball, 0.0)]);
    let first = frames.weights(&gpu);
    assert!(first.iter().all(|&weight| weight <= 1.0), "the first frame has no history");
    for _ in 0..30 {
        frames.frame(&gpu, view, &[]);
    }
    let converged = frames.weights(&gpu);
    let mean = converged.iter().sum::<f32>() / converged.len() as f32;
    assert!(mean > 16.0, "{mean}");
    frames.frame(&gpu, ViewData { cut: true, ..view }, &[]);
    assert!(frames.weights(&gpu).iter().all(|&weight| weight <= 1.0), "a cut drops every history");
}
