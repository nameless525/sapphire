//! The renderer's texture coverage estimate (M5 M5) against what the GPU reads: a scene drawn with every
//! texture whole, then with each lowered to the top the estimate says its views need, must be the same
//! picture, byte for byte. Run with `cargo test -p sapphire_render --test gpu texture_coverage:: --
//! --ignored`.
//!
//! Each level of each texture is its own colour, so a pixel that read a level the lowered texture lacks
//! would read another colour. The scene has the cases the estimate must hold for: a street seen at a
//! grazing angle, alpha-tested foliage cards (whose alpha's mip scale reads the whole texture's size, the
//! shader fix M4 owed), character surfaces near and far, and skinned characters near and far, posed so
//! their surfaces stretch (a deformed draw's bound takes a stretch of up to 2). Insets and the sky's maps
//! aren't here: an inset wants every texture it draws whole, and the sky's slots never stream; unit tests
//! pin both rules (`coverage`'s tests).

use glam::{Affine3A, Mat4, Quat, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, DistanceStylization, EnvironmentMaterial, FoliageGradient,
    FoliageMaterial, LightSplit, Material, MaterialDocument, Outline, Rim, Terminator, Translucency,
    Wind as WindResponse,
};
use sapphire_assets::mesh::{
    FoliageVertex, Influences, MaterialSlot, Mesh, PositionFormat, Skin, SkinBind, Submesh,
    VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags, VertexInfluence,
};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat, TextureTail};
use sapphire_assets::texture_level::TextureLevel;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    AmbientLight, CameraData, DirectionalLight, FramePacketRing, InstanceId, MaterialHandle, MeshHandle,
    MeshInstance, Msaa, RenderSettings, Renderer, SkyGradient, ViewData,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: (u32, u32) = (320, 240);
/// Each texture's side, and the levels above its tail.
const SIDE: u32 = 256;
const ABOVE: u32 = 6;

/// The stage's margin (`sapphire_stage`'s texture levels): a need is the estimate's finest level less half
/// a level, rounded down.
const MARGIN: f32 = 0.5;

/// The top the stage would want of a texture `SIDE` texels square read at `uv_per_pixel`.
fn need(uv_per_pixel: f32) -> u32 {
    if uv_per_pixel <= 0.0 {
        return 0;
    }
    let finest = (uv_per_pixel * SIDE as f32).log2() - MARGIN;
    if finest >= 0.0 { (finest.floor() as u32).min(ABOVE) } else { 0 }
}

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// Level `level` of texture `seed`: a checker of two shades of a colour of its own, alpha a disc over the
/// texture's square.
fn level_texels(seed: u32, level: u32) -> Vec<u8> {
    let side = SIDE >> level;
    let hue = |k: u32| ((seed * 97 + level * 53 + k * 31) % 200 + 40) as u8;
    let (r, g, b) = (hue(0), hue(1), hue(2));
    let mut texels = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let check = ((x * 8 / side.max(1)) + (y * 8 / side.max(1))).is_multiple_of(2);
            let shade = |c: u8| if check { c } else { c / 2 };
            let (u, v) = ((x as f32 + 0.5) / side as f32 - 0.5, (y as f32 + 0.5) / side as f32 - 0.5);
            let alpha = ((0.45 - (u * u + v * v).sqrt()) * 8.0 + 0.5).clamp(0.0, 1.0);
            texels.extend([shade(r), shade(g), shade(b), (alpha * 255.0) as u8]);
        }
    }
    texels
}

/// Texture `seed`, uploaded as its tail and raised whole.
fn whole_texture(gpu: &GpuContext, frames: &mut Frames, id: AssetId, seed: u32) -> GpuTexture {
    let levels: Vec<Vec<u8>> = (0..=SIDE.ilog2()).map(|level| level_texels(seed, level)).collect();
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: SIDE >> ABOVE,
        height: SIDE >> ABOVE,
        mip_level_count: levels.len() as u32 - ABOVE,
        swizzle: Swizzle::IDENTITY,
    };
    let tail = TextureTail {
        texture: Texture { desc, levels: levels[ABOVE as usize..].iter().map(Vec::as_slice).collect() },
        above: ABOVE,
    };
    let mut texture = GpuTexture::upload_tail(gpu, &tail, "streamed").expect("the tail");
    let cooked: Vec<_> = (0..ABOVE)
        .map(|level| {
            TextureLevel {
                texture: id,
                level,
                format: desc.format.vk_format(),
                width: SIDE >> level,
                height: SIDE >> level,
                data: levels[level as usize].clone(),
            }
            .to_cooked()
            .expect("cooked")
        })
        .collect();
    let read: Vec<_> = cooked.iter().map(|bytes| TextureLevel::read(bytes).expect("a level")).collect();
    frames.renderer.raise_texture(id, &mut texture, &read).expect("raised whole");
    texture
}

fn vertex(position: Vec3, normal: Vec3, uv: [f32; 2]) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv,
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    }
}

/// A rectangle from `corner` along `u` and `v`, facing `u × v`, its UVs 0 to `tiles` each way.
fn rectangle(corner: Vec3, u: Vec3, v: Vec3, tiles: [f32; 2]) -> (Vec<Vertex>, Vec<u32>) {
    let normal = u.cross(v).normalize();
    let at = |s: f32, t: f32| vertex(corner + u * s + v * t, normal, [s * tiles[0], t * tiles[1]]);
    (vec![at(0.0, 0.0), at(1.0, 0.0), at(1.0, 1.0), at(0.0, 1.0)], vec![0, 1, 2, 0, 2, 3])
}

/// A UV sphere of `radius` around `center`.
fn sphere(center: Vec3, radius: f32) -> (Vec<Vertex>, Vec<u32>) {
    const RINGS: u32 = 24;
    const SEGMENTS: u32 = 32;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = std::f32::consts::PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            let uv = [segment as f32 / SEGMENTS as f32, ring as f32 / RINGS as f32];
            vertices.push(vertex(center + n * radius, n, uv));
        }
    }
    let stride = SEGMENTS + 1;
    let mut indices = Vec::new();
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            indices.extend([a, c, b, c, d, b]);
        }
    }
    (vertices, indices)
}

/// The second joint's weight at height `y` on a sphere of `radius`: 0 below the band about the equator, 1
/// above it.
fn upper_weight(y: f32, radius: f32) -> f32 {
    ((y / radius + 0.4) / 0.8).clamp(0.0, 1.0)
}

/// `mesh`, a sphere of `radius` about the origin, skinned to two joints at the origin: its lower half to
/// the first, its upper half to the second, blending across a band about the equator.
fn skinned(mut mesh: Mesh, radius: f32) -> Mesh {
    let influences: Vec<VertexInfluence> = (0..mesh.streams.len())
        .map(|v| {
            let upper = upper_weight(mesh.streams.vertex(v).expect("in range").position[1], radius);
            let weights = sapphire_assets::mesh::quantize_weights([1.0 - upper, upper, 0.0, 0.0]);
            VertexInfluence { binds: [0, 1, 0, 0], weights }
        })
        .collect();
    let bind = |joint| SkinBind { joint, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() };
    mesh.skin = Some(Skin {
        skeleton: None,
        joint_count: 2,
        binds: vec![bind(0), bind(1)],
        influences: Influences::pack(&influences, 2),
    });
    mesh
}

/// The pose the skinned spheres hold: the upper half turned about Z and lifted, which stretches the band
/// between the halves ([`pose_stretch`] says by how much).
fn stretching_pose(radius: f32) -> Vec<Affine3A> {
    let bent =
        Affine3A::from_rotation_translation(Quat::from_rotation_z(0.3), Vec3::new(0.0, 0.3 * radius, 0.0));
    vec![Affine3A::IDENTITY, bent]
}

/// How much `palette` stretches the surface of the sphere `(vertices, indices)` of `radius` skinned as
/// [`skinned`] skins it: the largest singular value, over its triangles, of the map from a triangle at rest
/// to the triangle posed (blending the joints' transforms by the unquantized weights).
fn pose_stretch((vertices, indices): &(Vec<Vertex>, Vec<u32>), radius: f32, palette: &[Affine3A]) -> f32 {
    let posed = |p: Vec3| {
        let upper = upper_weight(p.y, radius);
        palette[0].transform_point3(p) * (1.0 - upper) + palette[1].transform_point3(p) * upper
    };
    let mut largest = 0.0f32;
    for triangle in indices.chunks(3) {
        let rest: Vec<Vec3> = triangle.iter().map(|&i| Vec3::from(vertices[i as usize].position)).collect();
        let (e1, e2) = (rest[1] - rest[0], rest[2] - rest[0]);
        let l1 = e1.length();
        let x = e1 / l1;
        let a = e2.dot(x);
        let b = (e2 - x * a).length();
        if l1 < 1e-6 || b < 1e-6 * l1 {
            continue;
        }
        let moved: Vec<Vec3> = rest.iter().map(|&p| posed(p)).collect();
        // The map's columns, from the triangle's plane at rest: d1 = M (l1, 0), d2 = M (a, b).
        let c0 = (moved[1] - moved[0]) / l1;
        let c1 = ((moved[2] - moved[0]) - c0 * a) / b;
        let (p, q, r) = (c0.dot(c0), c0.dot(c1), c1.dot(c1));
        let root = ((p - r) * (p - r) + 4.0 * q * q).sqrt();
        largest = largest.max(((p + r + root) * 0.5).sqrt());
    }
    largest
}

/// A mesh of one part, f32 positions, its one slot drawn by the material it's uploaded with.
fn mesh((vertices, indices): (Vec<Vertex>, Vec<u32>)) -> Mesh {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("part", placeholder)];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Float32)
}

struct Frames {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    camera: CameraData,
    added: Vec<MeshInstance>,
    /// The skinned instances' palettes, sent every frame.
    poses: Vec<(InstanceId, Vec<Affine3A>)>,
    target: wgpu::Texture,
}

impl Frames {
    fn new(gpu: &GpuContext) -> Self {
        Self::with_settings(
            gpu,
            RenderSettings { msaa: Msaa::Off, temporal: false, ..RenderSettings::default() },
        )
    }

    fn with_settings(gpu: &GpuContext, settings: RenderSettings) -> Self {
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            renderer: Renderer::new(gpu, FORMAT, settings).expect("renderer"),
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            camera: CameraData::look_at(
                Vec3::new(0.0, 1.6, 0.0),
                Vec3::new(0.0, 0.9, -20.0),
                Vec3::Y,
                60f32.to_radians(),
                SIZE.0 as f32 / SIZE.1 as f32,
                0.1,
            ),
            added: Vec::new(),
            poses: Vec::new(),
            target,
        }
    }

    fn material<D: MaterialDocument>(
        &mut self,
        document: &D,
        texture: (AssetId, &GpuTexture),
    ) -> MaterialHandle {
        let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
        let bytes = Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook");
        let material = Material::read(&bytes).expect("read");
        self.renderer.upload_material(material, |id| (id == texture.0).then_some(texture.1)).expect("upload")
    }

    fn show(&mut self, mesh: &Mesh, material: MaterialHandle) -> InstanceId {
        self.show_at(mesh, material, Mat4::IDENTITY)
    }

    fn show_at(&mut self, mesh: &Mesh, material: MaterialHandle, transform: Mat4) -> InstanceId {
        let bytes = mesh.to_cooked().expect("cook");
        let handle: MeshHandle = self
            .renderer
            .upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), &[material])
            .expect("upload");
        let id = InstanceId(self.added.len() as u64 + 1);
        self.added.push(MeshInstance { id, mesh: handle, transform });
        id
    }

    /// Renders a frame and reads it back.
    fn render(&mut self, gpu: &GpuContext) -> Vec<u8> {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = DirectionalLight {
            direction: Vec3::new(-0.3, -0.8, -0.5).normalize(),
            color: Vec3::ONE,
            intensity: 1.0,
        };
        packet.sky =
            SkyGradient { zenith: Vec3::splat(0.4), horizon: Vec3::splat(0.6), ground: Vec3::splat(0.3) };
        packet.ambient = AmbientLight { sky: Vec3::splat(0.5), ground: Vec3::splat(0.3), intensity: 0.3 };
        packet.instances.changed.append(&mut self.added);
        for (id, palette) in &self.poses {
            packet.push_skinned(*id, palette.iter().copied(), []);
        }
        self.renderer.prepare(gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(gpu, graph).expect("a valid graph");
        read_back(gpu, &self.target)
    }
}

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.get_mapped_range(..).expect("mapped");
    mapped.chunks(padded as usize).flat_map(|row| &row[..width as usize * 4]).copied().collect()
}

fn differing(a: &[u8], b: &[u8]) -> usize {
    a.chunks(4).zip(b.chunks(4)).filter(|(a, b)| a != b).count()
}

/// m5-plan M5's readback test: every texture lowered to the top its estimate needs draws the frame its
/// whole texture draws, byte for byte, and lowered a level further it doesn't.
#[test]
#[ignore = "needs a GPU"]
fn textures_at_the_top_their_estimate_needs_draw_the_whole_textures_picture() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu);
    let ids: [AssetId; 7] = std::array::from_fn(|index| AssetId::from_bytes([0x50 + index as u8; 16]));
    let [street_id, card_id, near_id, far_id, farther_id, skinned_near_id, skinned_far_id] = ids;
    let mut textures: Vec<GpuTexture> =
        (0..ids.len()).map(|index| whole_texture(&gpu, &mut frames, ids[index], index as u32)).collect();

    // A street from under the camera to 60 m ahead, eight tiles a metre, seen at a grazing angle.
    let street = EnvironmentMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(street_id),
        roughness: 0.5,
        specular_clamp: 0.0,
        wrap: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..EnvironmentMaterial::default()
    };
    let street = frames.material(&street, (street_id, &textures[0]));
    let ground = rectangle(
        Vec3::new(-10.0, 0.0, 0.0),
        Vec3::new(20.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, -60.0),
        [160.0, 480.0],
    );
    frames.show(&mesh(ground), street);

    // Two alpha-tested foliage cards, a metre square, 6 m and 15 m away.
    let card = FoliageMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(card_id),
        alpha_cutoff: 0.5,
        wrap: 0.0,
        terminator_width: 0.0,
        terminator_growth: 0.0,
        shadow_tint: [1.0; 3],
        canopy_occlusion: 0.0,
        translucency: Translucency { strength: 0.0, ..Translucency::default() },
        gradient: FoliageGradient { root: [1.0; 3], tip: [1.0; 3] },
        wind: WindResponse { sway: 0.0, bend: 0.0, flutter: 0.0, flutter_frequency: 1.0 },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..FoliageMaterial::default()
    };
    let card = frames.material(&card, (card_id, &textures[1]));
    for (x, z) in [(1.2, -6.0), (-2.0, -15.0)] {
        let quad = rectangle(Vec3::new(x, 0.6, z), Vec3::X, Vec3::Y, [1.0, 1.0]);
        let mut card_mesh = mesh(quad);
        let still = vec![FoliageVertex { gradient: 1.0, ..FoliageVertex::default() }; 4];
        card_mesh.streams = card_mesh.streams.clone().with_foliage(&still);
        frames.show(&card_mesh, card);
    }

    // Character surfaces without outlines (an outlined one wants its base colour whole) near and far.
    let character = |id| AnimeCharacterMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(id),
        light_split: LightSplit { threshold: 0.0, softness: 0.02 },
        regions: vec![CharacterRegion { shade_color: [0.8; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    };
    for (index, (id, center, radius)) in [
        (near_id, Vec3::new(-1.0, 1.1, -3.0), 0.5),
        (far_id, Vec3::new(2.5, 1.0, -14.0), 0.8),
        (farther_id, Vec3::new(-4.0, 1.5, -45.0), 1.0),
    ]
    .into_iter()
    .enumerate()
    {
        let material = frames.material(&character(id), (id, &textures[2 + index]));
        frames.show(&mesh(sphere(center, radius)), material);
    }

    // Skinned characters near and far, posed so their surfaces stretch: a deformed draw's bound is from its
    // posed bounds, and takes a stretch of up to `DEFORMED_STRETCH`. Up and aside, so neither hides anything
    // the test lowers.
    for (index, (id, center, radius)) in [
        (skinned_near_id, Vec3::new(-2.6, 2.6, -6.0), 0.6),
        (skinned_far_id, Vec3::new(3.5, 3.0, -24.0), 0.7),
    ]
    .into_iter()
    .enumerate()
    {
        let material = frames.material(&character(id), (id, &textures[5 + index]));
        let surface = sphere(Vec3::ZERO, radius);
        let stretch = pose_stretch(&surface, radius, &stretching_pose(radius));
        // The estimate takes a stretch of up to 2 (`coverage`'s `DEFORMED_STRETCH`); this pose comes near it.
        assert!((1.5..2.0).contains(&stretch), "the pose stretches the surface by {stretch}");
        eprintln!("skinned {index}: the pose stretches its surface by up to {stretch:.2}");
        let body = skinned(mesh(surface), radius);
        let instance = frames.show_at(&body, material, Mat4::from_translation(center));
        frames.poses.push((instance, stretching_pose(radius)));
    }

    // Whole, settled: two frames alike, so the cascades and everything cached are too.
    for _ in 0..4 {
        frames.render(&gpu);
    }
    let whole = frames.render(&gpu);
    assert_eq!(differing(&whole, &frames.render(&gpu)), 0, "a settled frame repeats");

    frames.renderer.estimate_texture_coverage();
    let coverage = frames.renderer.texture_coverage();
    let needs: Vec<u32> = ids
        .iter()
        .map(|id| {
            let entry = coverage.textures.iter().find(|entry| entry.texture == *id);
            need(entry.expect("every texture is read").uv_per_pixel)
        })
        .collect();
    // The scene lowers something of each kind, or it proves nothing.
    assert!(needs[0] >= 1 && needs[1] >= 1, "the street and the cards stream: {needs:?}");
    assert!(needs[2] < needs[3] && needs[3] < needs[4], "farther reads coarser: {needs:?}");
    assert!(needs[5] < needs[6], "the far skinned character reads coarser: {needs:?}");

    for (index, id) in ids.iter().enumerate() {
        if needs[index] > 0 {
            frames.renderer.lower_texture(*id, &mut textures[index], needs[index]).expect("lowered");
        }
    }
    let lowered = frames.render(&gpu);
    assert_eq!(differing(&whole, &lowered), 0, "lowered to {needs:?}, the picture changed");

    // Lowered further, a level at a time, the picture changes before the tail: the scene reads the levels
    // apart, so the equality above is the estimate's doing. How many levels further it took is the
    // estimate's slack (it bounds the read from below), printed for the record.
    let mut previous = lowered;
    for index in [0, 1, 4, 5, 6] {
        let first_change = (needs[index] + 1..=ABOVE).find(|&top| {
            frames.renderer.lower_texture(ids[index], &mut textures[index], top).expect("lowered");
            let blurred = frames.render(&gpu);
            let changed = differing(&previous, &blurred) > 0;
            previous = blurred;
            changed
        });
        let first_change =
            first_change.unwrap_or_else(|| panic!("texture {index} lowered to its tail drew the same"));
        eprintln!("texture {index}: need {}, first change at {first_change}", needs[index]);
    }
}

/// An alpha-tested foliage card out of the main view's frustum, aside and ahead, whose shadow a
/// cascade casts: a still view holds every cascade a cache that keeps drew (M5 D8), and a held map
/// still reads the card's base colour, so the estimate wants it on every frame, not only on those
/// that drew a cascade (M5 Z3a's F1: the streamer lowered it to its tail once the cascades were
/// held, and the held map redrew with the coarse cut-out).
#[test]
#[ignore = "needs a GPU"]
fn a_held_cascade_keeps_its_casters_textures_wanted() {
    use sapphire_render::{CascadeCache, ShadowSettings};

    for cache in [CascadeCache::Kept, CascadeCache::Held] {
        let gpu = gpu();
        let shadows = ShadowSettings { spots: 0, cache, ..ShadowSettings::default() };
        let settings = RenderSettings {
            msaa: Msaa::Off,
            temporal: false,
            shadows: Some(shadows),
            ..RenderSettings::default()
        };
        let mut frames = Frames::with_settings(&gpu, settings);
        let card_id = AssetId::from_bytes([0x70; 16]);
        let texture = whole_texture(&gpu, &mut frames, card_id, 1);

        // Ground for the shadow to fall on, untextured.
        let ground = frames.material(&EnvironmentMaterial::default(), (card_id, &texture));
        let plane = rectangle(
            Vec3::new(-60.0, 0.0, 20.0),
            Vec3::new(120.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -140.0),
            [1.0; 2],
        );
        frames.show(&mesh(plane), ground);

        // The card, 2 m square, 25 m aside at 15 m ahead: 59° off the view's axis, past the frustum's
        // 38° half width, and in the last cascade's reach.
        let card = FoliageMaterial {
            base_color: [1.0; 4],
            base_color_map: Some(card_id),
            alpha_cutoff: 0.5,
            wind: WindResponse { sway: 0.0, bend: 0.0, flutter: 0.0, flutter_frequency: 1.0 },
            ..FoliageMaterial::default()
        };
        let card = frames.material(&card, (card_id, &texture));
        let quad = rectangle(Vec3::new(25.0, 0.5, -15.0), Vec3::X * 2.0, Vec3::Y * 2.0, [1.0, 1.0]);
        let mut card_mesh = mesh(quad);
        let still = vec![FoliageVertex { gradient: 1.0, ..FoliageVertex::default() }; 4];
        card_mesh.streams = card_mesh.streams.clone().with_foliage(&still);
        frames.show(&card_mesh, card);

        let mut drawn = Vec::new();
        let mut read = Vec::new();
        for _ in 0..8 {
            frames.render(&gpu);
            drawn.push(frames.renderer.stats().shadows.cascades_drawn);
            frames.renderer.estimate_texture_coverage();
            let wanted =
                frames.renderer.texture_coverage().textures.iter().any(|entry| entry.texture == card_id);
            read.push(wanted);
        }
        // The first frame draws every cascade and a still view none after it: the case, or the test
        // proves nothing.
        assert!(
            drawn[0] > 0 && drawn[2..].iter().all(|&count| count == 0),
            "{cache:?}: cascades drawn {drawn:?}"
        );
        assert!(read[0], "{cache:?}: the frame that drew the cascades reads the card's texture");
        assert!(read.iter().all(|&wanted| wanted), "{cache:?}: the card's texture was wanted on {read:?}");
    }
}
