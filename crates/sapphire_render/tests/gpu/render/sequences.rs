//! Seeded lifecycle sequences on every shipped tier: uploads, mesh replacements (between frames
//! and between `prepare` and `record`), instances, poses, characters with fades, lights, targets
//! down to one pixel, render scales, debug views, cuts, shader requests, and frames that end every
//! way a frame can (own review RC). `SAPPHIRE_SEQUENCE_SEEDS` runs more seeds than the few the
//! suite runs by default.

use std::collections::BTreeMap;

use sapphire_assets::material::{
    AlignedVec, AlphaMode, EnvironmentMaterial, FogCardMaterial, FoliageMaterial, MaterialDocument,
};
use sapphire_assets::mesh::FoliageVertex;
use sapphire_render::quality::TierTable;
use sapphire_render::{
    CharacterBody, CharacterFace, CharacterInstance, CharacterLighting, ChromaticAberration, DebugView,
    EffectCoverage, Inversion, PostEffects, RadialBlur, ShaderFile, ShaderSources,
};
use sapphire_rhi::TransientDesc;

use super::*;

/// A deterministic generator (xorshift), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn unit(&mut self) -> f32 {
        (self.next() % 1000) as f32 / 1000.0
    }

    fn position(&mut self) -> Mat4 {
        let x = self.unit() * 6.0 - 3.0;
        let (y, z) = (self.unit() * 3.0 - 1.5, self.unit() * 4.0 - 2.0);
        Mat4::from_translation(Vec3::new(x, y, z))
    }
}

fn seeds() -> u64 {
    std::env::var("SAPPHIRE_SEQUENCE_SEEDS").ok().and_then(|seeds| seeds.parse().ok()).unwrap_or(1)
}

/// An environment sphere about the origin.
pub(super) fn environment_sphere(alpha: AlphaMode) -> Mesh {
    let document =
        EnvironmentMaterial { base_color: [0.4, 0.6, 0.2, 0.5], alpha, ..EnvironmentMaterial::default() };
    let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
    let material = Material::from_document(&document, &swizzles).expect("a valid material");
    let (vertices, indices) = sphere_geometry(Vec3::ZERO, 0.7);
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("environment", material)];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// A card standing on the origin, of `document`'s material; `swaying`, with the wind's weights.
pub(super) fn card<D: MaterialDocument>(document: &D, swaying: bool) -> Mesh {
    let corner = |s: f32, t: f32| Vertex {
        position: [s, t + 1.0, 0.0],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [(s + 1.0) * 0.5, (1.0 - t) * 0.5],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let vertices = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
    let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
    let material = Material::from_document(document, &swizzles).expect("a valid material");
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    let slots = vec![MaterialSlot::embedded("card", material)];
    let mut mesh =
        Mesh::from_vertices(&vertices, vec![0, 1, 2, 0, 2, 3], submeshes, slots, PositionFormat::Unorm16);
    if swaying {
        let sway = FoliageVertex { sway: 1.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 };
        mesh.streams = mesh.streams.clone().with_foliage(&[FoliageVertex::decode(sway.encode()); 4]);
    }
    mesh
}

/// How many of [`sequence_meshes`] come first that draw the same whatever order instances take
/// their slots in. Transparent draws aren't sorted among themselves, foliage's cards and fog cards
/// are in slot order too, and a run's slots aren't a fresh renderer's.
const ORDER_FREE_MESHES: usize = 6;

/// Every kind of mesh the sequences draw, cooked: the static and skinned ones first.
fn sequence_meshes() -> Vec<AlignedVec> {
    let two_sided = AnimeCharacterMaterial { double_sided: true, ..colored([0.7, 0.3, 0.2]) };
    [
        sphere(white(1.0)),
        sphere_at(Vec3::ZERO, 0.8, colored([0.2, 0.3, 0.7]), PositionFormat::Float32),
        skinned_sphere(colored([0.9, 0.1, 0.1])),
        skinned_sphere(white(2.0)),
        sphere(two_sided),
        environment_sphere(AlphaMode::Opaque),
        environment_sphere(AlphaMode::Blend),
        card(&FoliageMaterial { alpha_cutoff: 0.0, ..FoliageMaterial::default() }, true),
        card(&FogCardMaterial::default(), false),
    ]
    .iter()
    .map(|mesh| mesh.to_cooked().expect("cook"))
    .collect()
}

fn character(id: u64, base: Vec3, opacity: f32) -> CharacterInstance {
    let key = DirectionalLight { direction: Vec3::NEG_Y, color: Vec3::ONE, intensity: 1.0 };
    CharacterInstance {
        id: InstanceId(id),
        lighting: CharacterLighting::unstyled(key),
        face: CharacterFace::default(),
        body: CharacterBody::capsule(base, base + Vec3::Y, 0.3),
        opacity,
    }
}

/// `frames` frames of lifecycle operations in the order `seed` says. A panic or a validation
/// error fails it.
fn run_sequence(gpu: &GpuContext, settings: RenderSettings, label: &str, seed: u64, frames: u32) {
    let mut rng = Rng::new(seed);
    let sizes = [(64, 64), (32, 48), (1, 1), (128, 72), (9, 7), (15, 15), (16, 2), (2, 16), (200, 100)];
    let textures: Vec<_> = sizes.iter().map(|&size| target(gpu, FORMAT, size)).collect();
    let views: Vec<_> = textures.iter().map(|texture| texture.create_view(&Default::default())).collect();
    let debug_views = [
        DebugView::Off,
        DebugView::Off,
        DebugView::Off,
        DebugView::Wireframe,
        DebugView::Overdraw,
        DebugView::Lights,
        DebugView::MaterialIds,
        DebugView::RampRows,
        DebugView::OutlineWidths,
    ];
    let cooked = sequence_meshes();
    let read = |rng: &mut Rng| Mesh::read(&cooked[rng.below(cooked.len())]).expect("a valid mesh");
    let mut scene = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
    scene.post = PostParams::default();
    let mut handles: Vec<MeshHandle> = cooked[..3]
        .iter()
        .map(|bytes| {
            let mesh = Mesh::read(bytes).expect("a valid mesh");
            scene.renderer.upload_mesh_with_slots(mesh, |_| None).expect("upload")
        })
        .collect();
    let mut live: Vec<u64> = Vec::new();
    let mut next_id = 0;
    let mut requested = 0;
    for frame in 0..frames {
        if rng.chance(8) {
            let handle = handles[rng.below(handles.len())];
            scene.renderer.update_mesh(handle, read(&mut rng), |_| None).expect("update");
        }
        if rng.chance(3) && handles.len() < 8 {
            let mesh = read(&mut rng);
            handles.push(scene.renderer.upload_mesh_with_slots(mesh, |_| None).expect("upload"));
        }
        if rng.chance(1) && requested < 4 {
            requested += 1;
            let file = if rng.chance(50) { ShaderFile::Post } else { ShaderFile::Frame };
            let mut sources = ShaderSources::compiled_in();
            let edited = format!("{}\n// edited {frame}\n", sources.get(file));
            sources.set(file, edited);
            scene.renderer.request_shader_sources(sources);
        }
        let which = rng.below(sizes.len());
        let camera = framing_camera(38.0, sizes[which], 0.2 + rng.unit() * 0.5);
        let no_view = rng.chance(5);
        let cut = rng.chance(5);
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        if rng.chance(30) || live.is_empty() {
            live.push(next_id);
            let handle = handles[rng.below(handles.len())];
            changed.push(instance(next_id, handle, rng.position()));
            next_id += 1;
        }
        if rng.chance(15) && live.len() > 1 {
            removed.push(InstanceId(live.swap_remove(rng.below(live.len()))));
        }
        if rng.chance(20) {
            let id = live[rng.below(live.len())];
            changed.push(instance(id, handles[rng.below(handles.len())], rng.position()));
        }
        let mut effects = PostEffects::NONE;
        if rng.chance(15) {
            effects.radial_blur = RadialBlur { center: glam::Vec2::splat(0.5), strength: 0.05 };
        }
        if rng.chance(15) {
            effects.chromatic_aberration =
                ChromaticAberration { center: glam::Vec2::splat(0.5), strength: 0.05 };
        }
        if rng.chance(15) {
            effects.inversion = Inversion { amount: 1.0, coverage: EffectCoverage::WORLD };
        }
        scene.post.effects = effects;
        let packet = scene.packets.begin();
        if !no_view {
            let mut view = ViewData::new(camera);
            view.cut = cut;
            packet.views.push(view);
        }
        packet.sun = scene.sun;
        packet.sky = scene.sky;
        packet.ambient = scene.ambient;
        packet.post = scene.post;
        packet.time_seconds = f64::from(frame) / 60.0;
        packet.instances.changed.extend_from_slice(&changed);
        packet.instances.removed.extend_from_slice(&removed);
        for _ in 0..rng.below(3) {
            let at = Vec3::new(rng.unit() * 6.0 - 3.0, rng.unit() * 4.0 - 2.0, rng.unit() * 4.0);
            packet.lights.push(point_light(at, Vec3::ONE, 2.0, 3.0));
        }
        for &id in &live {
            if rng.chance(40) {
                packet.push_skinned(InstanceId(id), bend(rng.unit(), rng.unit()), [rng.unit()]);
            }
            if rng.chance(30) {
                let opacity = [1.0, 1.0, 0.5, 0.0][rng.below(4)];
                packet.characters.push(character(id, Vec3::new(0.0, rng.unit() * 2.0, 0.0), opacity));
            }
        }
        scene.renderer.set_debug_view(debug_views[rng.below(debug_views.len())]);
        scene.renderer.set_outline_distance([1.0, 1.0, 0.5, 0.0][rng.below(4)]);
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        scene.renderer.prepare(gpu, scene.packets.current());
        scene.renderer.set_render_scale([1.0, 1.0, 0.75, 0.5, 0.25][rng.below(5)]);
        // Against its docs, and tolerated (RC-6).
        if rng.chance(4) {
            let handle = handles[rng.below(handles.len())];
            scene.renderer.update_mesh(handle, read(&mut rng), |_| None).expect("update");
        }
        // How the frame ends.
        let ending = rng.below(100);
        if !no_view {
            if ending < 72 {
                let mut graph = FrameGraph::new();
                let target = graph.import_texture("target", &views[which]);
                scene.renderer.record(&mut graph, target).expect("a view and a target");
                scene.graph.execute(gpu, graph).expect("a valid graph");
            } else if ending < 80 {
                // Prepared, never recorded.
            } else if ending < 85 {
                let usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
                let desc = TransientDesc { width: 0, height: 0, format: FORMAT, sample_count: 1, usage };
                let mut graph = FrameGraph::new();
                let empty = graph.create_texture("empty", desc);
                assert!(scene.renderer.record(&mut graph, empty).is_none());
            } else {
                let mut graph = FrameGraph::new();
                let target = graph.import_texture("target", &views[which]);
                scene.renderer.record(&mut graph, target).expect("a view and a target");
            }
        }
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{label} seed {seed} frame {frame}: {error:?}");
        if frame % 8 == 0 {
            let _ = gpu.device().poll(wgpu::PollType::Poll);
        }
    }
    let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
}

#[test]
#[ignore = "needs a GPU"]
fn lifecycle_sequences_hold_on_every_tier() {
    for tier in TierTable::shipped().tiers() {
        for seed in 1..=seeds() {
            let gpu = gpu();
            run_sequence(&gpu, tier.render_settings(), tier.id.name(), seed, 120);
        }
    }
}

type Pose = (Vec<Affine3A>, Vec<f32>);

/// An instance as the sequence left it: which handle it draws, where, and its last pose since
/// its mesh last changed.
#[derive(Clone)]
struct Placed {
    handle: usize,
    transform: Mat4,
    pose: Option<Pose>,
}

/// One settled frame's packet: what both renderers are given at the end.
fn settled_packet(scene: &mut Frames, camera: CameraData, placed: &BTreeMap<u64, Placed>) {
    let packet = scene.packets.begin();
    packet.views.push(ViewData::new(camera));
    packet.sun = scene.sun;
    packet.sky = scene.sky;
    packet.ambient = scene.ambient;
    packet.post = scene.post;
    packet.time_seconds = 1000.0;
    packet.lights.push(point_light(Vec3::new(1.0, 1.0, 2.0), Vec3::ONE, 2.0, 5.0));
    for (&id, place) in placed {
        if id % 2 == 0 {
            packet.characters.push(character(id, place.transform.w_axis.truncate(), 1.0));
        }
    }
}

fn execute_frame(scene: &mut Frames, gpu: &GpuContext, view: &wgpu::TextureView) {
    scene.renderer.prepare(gpu, scene.packets.current());
    let mut graph = FrameGraph::new();
    let target = graph.import_texture("target", view);
    scene.renderer.record(&mut graph, target).expect("a view and a target");
    scene.graph.execute(gpu, graph).expect("a valid graph");
}

/// After 200 frames of lifecycle operations whose ends include every way of losing a frame, 12
/// settled frames, and the same 12 from a fresh renderer given the same final state: how many
/// pixels differ, and how many pixels of the first frame aren't its corner's.
fn settled_against_fresh(gpu: &GpuContext, settings: RenderSettings, seed: u64) -> (usize, usize) {
    let mut rng = Rng::new(seed);
    let cooked = sequence_meshes();
    let read = |index: usize| Mesh::read(&cooked[index]).expect("a valid mesh");
    let final_size = (128, 72);
    let final_texture = target(gpu, FORMAT, final_size);
    let final_view = final_texture.create_view(&Default::default());
    let sizes = [(64, 64), (32, 48), (128, 72), (15, 15), (200, 100)];
    let textures: Vec<_> = sizes.iter().map(|&size| target(gpu, FORMAT, size)).collect();
    let views: Vec<_> = textures.iter().map(|texture| texture.create_view(&Default::default())).collect();
    let debug_views = [
        DebugView::Off,
        DebugView::Off,
        DebugView::Wireframe,
        DebugView::Overdraw,
        DebugView::MaterialIds,
        DebugView::OutlineWidths,
    ];
    let mut old = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
    old.post = PostParams::default();
    let mut handles: Vec<MeshHandle> = Vec::new();
    // Which cooked mesh each handle holds now.
    let mut holds: Vec<usize> = Vec::new();
    for index in 0..3 {
        handles.push(old.renderer.upload_mesh_with_slots(read(index), |_| None).expect("upload"));
        holds.push(index);
    }
    let mut placed: BTreeMap<u64, Placed> = BTreeMap::new();
    let mut next_id = 0;
    for frame in 0..200u32 {
        if rng.chance(8) {
            let which = rng.below(handles.len());
            let replacement = rng.below(ORDER_FREE_MESHES);
            old.renderer.update_mesh(handles[which], read(replacement), |_| None).expect("update");
            holds[which] = replacement;
            for place in placed.values_mut().filter(|place| place.handle == which) {
                place.pose = None;
            }
        }
        if rng.chance(3) && handles.len() < 8 {
            let index = rng.below(ORDER_FREE_MESHES);
            handles.push(old.renderer.upload_mesh_with_slots(read(index), |_| None).expect("upload"));
            holds.push(index);
        }
        let which = rng.below(sizes.len());
        let camera = framing_camera(38.0, sizes[which], 0.2 + rng.unit() * 0.5);
        let no_view = rng.chance(5);
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        if rng.chance(30) || placed.is_empty() {
            let handle = rng.below(handles.len());
            let transform = rng.position();
            changed.push(instance(next_id, handles[handle], transform));
            placed.insert(next_id, Placed { handle, transform, pose: None });
            next_id += 1;
        }
        if rng.chance(15) && placed.len() > 1 {
            let keys: Vec<u64> = placed.keys().copied().collect();
            let id = keys[rng.below(keys.len())];
            if !changed.iter().any(|instance: &MeshInstance| instance.id == InstanceId(id)) {
                placed.remove(&id);
                removed.push(InstanceId(id));
            }
        }
        if rng.chance(20) {
            let keys: Vec<u64> = placed.keys().copied().collect();
            let id = keys[rng.below(keys.len())];
            let handle = rng.below(handles.len());
            let transform = rng.position();
            changed.push(instance(id, handles[handle], transform));
            let place = placed.get_mut(&id).expect("placed");
            if place.handle != handle {
                place.pose = None;
            }
            (place.handle, place.transform) = (handle, transform);
        }
        let packet = old.packets.begin();
        if !no_view {
            packet.views.push(ViewData::new(camera));
        }
        packet.sun = old.sun;
        packet.sky = old.sky;
        packet.ambient = old.ambient;
        packet.post = old.post;
        packet.time_seconds = f64::from(frame) / 60.0;
        packet.instances.changed.extend_from_slice(&changed);
        packet.instances.removed.extend_from_slice(&removed);
        for _ in 0..rng.below(3) {
            let at = Vec3::new(rng.unit() * 6.0 - 3.0, rng.unit() * 4.0 - 2.0, rng.unit() * 4.0);
            packet.lights.push(point_light(at, Vec3::ONE, 2.0, 3.0));
        }
        for (&id, place) in placed.iter_mut() {
            if rng.chance(40) {
                let pose = (bend(rng.unit(), rng.unit()), vec![rng.unit()]);
                packet.push_skinned(InstanceId(id), pose.0.iter().copied(), pose.1.iter().copied());
                place.pose = Some(pose);
            }
            if rng.chance(30) {
                let opacity = [1.0, 0.5, 0.0][rng.below(3)];
                packet.characters.push(character(id, place.transform.w_axis.truncate(), opacity));
            }
        }
        old.renderer.set_debug_view(debug_views[rng.below(debug_views.len())]);
        old.renderer.set_outline_distance([1.0, 1.0, 0.5, 0.0][rng.below(4)]);
        old.renderer.prepare(gpu, old.packets.current());
        old.renderer.set_render_scale([1.0, 1.0, 0.75, 0.5][rng.below(4)]);
        let ending = rng.below(100);
        if !no_view {
            if ending < 70 {
                let mut graph = FrameGraph::new();
                let target = graph.import_texture("target", &views[which]);
                old.renderer.record(&mut graph, target).expect("a view and a target");
                old.graph.execute(gpu, graph).expect("a valid graph");
            } else if ending < 80 {
                // Prepared, never recorded.
            } else if ending < 90 {
                let usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
                let desc = TransientDesc { width: 0, height: 0, format: FORMAT, sample_count: 1, usage };
                let mut graph = FrameGraph::new();
                let empty = graph.create_texture("empty", desc);
                assert!(old.renderer.record(&mut graph, empty).is_none());
            } else {
                let mut graph = FrameGraph::new();
                let target = graph.import_texture("target", &views[which]);
                old.renderer.record(&mut graph, target).expect("a view and a target");
            }
        }
        if frame % 8 == 0 {
            let _ = gpu.device().poll(wgpu::PollType::Poll);
        }
    }

    old.renderer.set_debug_view(DebugView::Off);
    old.renderer.set_outline_distance(1.0);
    old.renderer.set_render_scale(1.0);
    let camera = framing_camera(38.0, final_size, 0.25);
    for _ in 0..12 {
        settled_packet(&mut old, camera, &placed);
        execute_frame(&mut old, gpu, &final_view);
    }
    let first = read_back(gpu, &final_texture);
    let drawn = first.iter().filter(|texel| **texel != first[0]).count();

    let mut fresh = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
    fresh.post = PostParams::default();
    let fresh_handles: Vec<MeshHandle> = holds
        .iter()
        .map(|&index| fresh.renderer.upload_mesh_with_slots(read(index), |_| None).expect("upload"))
        .collect();
    settled_packet(&mut fresh, camera, &placed);
    let packet = fresh.packets.current_mut();
    for (&id, place) in &placed {
        packet.instances.changed.push(instance(id, fresh_handles[place.handle], place.transform));
        if let Some((palette, weights)) = &place.pose {
            packet.push_skinned(InstanceId(id), palette.iter().copied(), weights.iter().copied());
        }
    }
    execute_frame(&mut fresh, gpu, &final_view);
    for _ in 0..11 {
        settled_packet(&mut fresh, camera, &placed);
        execute_frame(&mut fresh, gpu, &final_view);
    }
    let second = read_back(gpu, &final_texture);
    (difference(&first, &second, 0).1, drawn)
}

/// However a run went, what it settles to is what a fresh renderer draws: nothing is left of the
/// frames that were lost. Without temporal AA, whose history converges rather than matches.
#[test]
#[ignore = "needs a GPU"]
fn settled_frames_match_a_fresh_renderer_on_every_tier() {
    for tier in TierTable::shipped().tiers() {
        for seed in 1..=seeds() {
            let gpu = gpu();
            let settings = RenderSettings { temporal: false, ..tier.render_settings() };
            let (off, drawn) = settled_against_fresh(&gpu, settings, seed);
            assert!(drawn > 0, "{} seed {seed}: the frame draws something", tier.id);
            assert_eq!(off, 0, "{} seed {seed}: {drawn} pixels drawn", tier.id);
        }
    }
}
