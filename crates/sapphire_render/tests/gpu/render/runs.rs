//! Instanced runs (M5 D2) draw what their draws drew one by one, byte for byte: two renderers
//! given the same frames, one drawing runs and one a call a draw (`RenderSettings::draw_runs`), on
//! every shipped tier's settings. The scene has a listed party (static and skinned anime characters
//! with outline hulls, one fading), eyes and their layers through hair, many environment instances
//! out of mesh and material order, some behind the camera, some placed twice at the same place
//! (twins in neighbouring slots, so a run holds an exact tie), rows of one environment mesh and of
//! one foliage card in neighbouring slots (long runs in every view), exact ties in depth between
//! two materials of one object and between two objects (which a reorder of the views that write
//! colour would flip), transparent surfaces, foliage and fog cards, every cascade and the character
//! maps, an inset, and two debug views' replays, which draw a call a draw either way. Each frame
//! compares the target, the scene prepass's depth, normals and outline mask, and the character
//! prepass's depth and IDs; every pass's counts but the calls, which runs cut; and the state the
//! passes changed. It replaces D1's test of the lists against the filtered walk they replaced;
//! `render::culling` draws the same scene through each culling mode (M5 D3), and keeps [`Side`] and
//! [`scene_meshes`]. Run alone with `cargo test -p sapphire_render --test gpu render::runs:: -- --ignored`.

use sapphire_assets::material::AlignedVec;
use sapphire_assets::material::{
    AlphaMode, CharacterSurface, EnvironmentMaterial, EyeLayer, EyeShading, HairShading, HighlightMotion,
    MaterialDocument, ThroughHair,
};
use sapphire_render::quality::TierTable;
use sapphire_render::{
    DebugView, DrawRuns, FrameCounts, InsetView, PassCounts, RenderStats, UiDrawList, UiQuad,
};

use super::sequences::{character, sequence_meshes};
use super::*;

const SIZE: (u32, u32) = (160, 96);
const INSET: (u32, u32) = (48, 48);
/// Instances of the scene's meshes, in an order that mixes them.
const SCATTERED: u64 = 72;
/// The eyes' instance, listed as a character.
const EYES: u64 = 500;
/// Instances placed twice: their twins have ids this much higher.
const TWINS: u64 = 1000;
/// The inset's instances.
const IN_INSET: [u64; 2] = [2000, 2001];

/// A flat anime character material of `surface`.
fn flat(surface: CharacterSurface, rgb: [f32; 3], alpha: AlphaMode, a: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        surface,
        alpha,
        base_color: [rgb[0], rgb[1], rgb[2], a],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions: vec![CharacterRegion { shade_color: [0.25; 3], ..CharacterRegion::default() }],
        ..AnimeCharacterMaterial::default()
    }
}

/// A rectangle facing +z at depth `z`.
fn rect(min: (f32, f32), max: (f32, f32), z: f32) -> [Vertex; 4] {
    [(min.0, min.1), (max.0, min.1), (max.0, max.1), (min.0, max.1)].map(|(x, y)| Vertex {
        position: [x, y, z],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [(x - min.0) / (max.0 - min.0), (max.1 - y) / (max.1 - min.1)],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    })
}

/// An eye drawn through a strand of hair in front of it, with its shadow, iris and highlight
/// layers listed highlight first, so only their layers' order puts them right.
fn eyes() -> Mesh {
    let through_hair = Some(ThroughHair { opacity: 0.5, distance: 0.5, ..ThroughHair::default() });
    let eye = |layer| EyeShading { layer, through_hair, ..EyeShading::default() };
    let still = HighlightMotion { drift: 0.0, drift_rate: 0.0, sparkle: 0.0, sparkle_rate: 0.0 };
    let parts = [
        (
            rect((-0.5, -0.3), (0.5, -0.1), 0.2),
            flat(CharacterSurface::Hair(HairShading::default()), [0.1, 0.6, 0.2], AlphaMode::Opaque, 1.0),
        ),
        (
            rect((-0.1, -0.1), (0.1, 0.1), 0.02),
            flat(
                CharacterSurface::Eye(eye(EyeLayer::Highlight(still))),
                [0.9, 0.8, 0.3],
                AlphaMode::Blend,
                0.8,
            ),
        ),
        (
            rect((-0.3, -0.3), (0.3, 0.3), 0.0),
            flat(CharacterSurface::Eye(eye(EyeLayer::Eye)), [0.9; 3], AlphaMode::Opaque, 1.0),
        ),
        (
            rect((-0.2, -0.2), (0.2, 0.2), 0.01),
            flat(CharacterSurface::Eye(eye(EyeLayer::Iris)), [0.1, 0.2, 0.7], AlphaMode::Blend, 0.7),
        ),
        (
            rect((-0.3, 0.0), (0.3, 0.3), 0.005),
            flat(CharacterSurface::Eye(eye(EyeLayer::Shadow)), [0.4, 0.4, 0.8], AlphaMode::Blend, 0.5),
        ),
    ];
    let (mut vertices, mut indices, mut submeshes, mut slots) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (slot, (corners, material)) in parts.into_iter().enumerate() {
        let base = vertices.len() as u32;
        submeshes.push(Submesh { first_index: indices.len() as u32, index_count: 6, material: slot as u32 });
        vertices.extend_from_slice(&corners);
        indices.extend([0, 1, 2, 0, 2, 3].map(|index| base + index));
        slots.push(MaterialSlot::embedded(format!("part {slot}"), cooked(&material)));
    }
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Float32)
}

/// An opaque environment material of `rgb`.
fn environment(rgb: [f32; 3]) -> Material {
    let document =
        EnvironmentMaterial { base_color: [rgb[0], rgb[1], rgb[2], 1.0], ..EnvironmentMaterial::default() };
    let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
    Material::from_document(&document, &swizzles).expect("a valid material")
}

/// A rectangle's corners and its material slot.
type Placed = ((f32, f32), (f32, f32), u32);

/// Exact ties in depth: `rects` drawn in one plane, each a submesh of material slot `material`
/// with its own vertices (in f32, so two meshes' corners match exactly).
fn coplanar(rects: &[Placed], materials: &[[f32; 3]]) -> Mesh {
    let (mut vertices, mut indices, mut submeshes) = (Vec::new(), Vec::new(), Vec::new());
    for &(min, max, material) in rects {
        let base = vertices.len() as u32;
        submeshes.push(Submesh { first_index: indices.len() as u32, index_count: 6, material });
        vertices.extend_from_slice(&rect(min, max, 0.0));
        indices.extend([0, 1, 2, 0, 2, 3].map(|index| base + index));
    }
    let slots = materials
        .iter()
        .enumerate()
        .map(|(slot, &rgb)| MaterialSlot::embedded(format!("tie {slot}"), environment(rgb)))
        .collect();
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Float32)
}

/// One object whose two submeshes are the same square in two materials, the first listed in the
/// later material (a material-first order would draw it second); and another object, a later
/// mesh, covering the square's right half in the same plane, listed before it (a mesh-first
/// order would draw it after). Where they tie, the first drawn wins: the lists must keep it.
fn ties() -> [Mesh; 2] {
    let square = ((-0.6, -0.6), (0.6, 0.6));
    let one =
        coplanar(&[(square.0, square.1, 1), (square.0, square.1, 0)], &[[0.9, 0.1, 0.1], [0.1, 0.2, 0.9]]);
    let other = coplanar(&[((0.0, -0.6), (0.6, 0.6), 0)], &[[0.1, 0.9, 0.2]]);
    [one, other]
}

/// Where the ties stand: in front of everything else.
const TIED_AT: Vec3 = Vec3::new(2.4, 1.8, 1.0);
/// The ties' instances, the covering object's listed first.
const COVERING: u64 = 3000;
const TIED: u64 = 3001;
/// Rows of instances of one mesh in neighbouring slots: an opaque environment sphere's and a
/// foliage card's, by their places in [`sequence_meshes`].
const ROWS: [(u64, usize, u64); 2] = [(4000, 5, 12), (5000, 7, 8)];

/// A renderer and the targets it reads back.
pub(super) struct Side {
    frames: Frames,
    handles: Vec<MeshHandle>,
    /// The character prepass's depth and IDs, the scene prepass's depth, normals, outlines and
    /// velocity (where the tier writes it).
    reads: [Readback; 6],
}

impl Side {
    pub(super) fn new(
        gpu: &GpuContext,
        settings: RenderSettings,
        runs: DrawRuns,
        meshes: &[AlignedVec],
    ) -> Self {
        let settings = RenderSettings { draw_runs: runs, ..settings };
        let mut frames = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
        frames.renderer.keep_scene_prepass();
        frames.renderer.set_render_scale(1.0);
        frames.post = PostParams::default();
        frames.sun = DirectionalLight {
            direction: Vec3::new(-0.3, -0.8, -0.5).normalize(),
            color: Vec3::ONE,
            intensity: 1.0,
        };
        let handles = meshes
            .iter()
            .map(|cooked| {
                let mesh = Mesh::read(cooked).expect("a valid mesh");
                frames.renderer.upload_mesh_with_slots(mesh, |_| None).expect("upload")
            })
            .collect();
        let reads = std::array::from_fn(|_| Readback::new(gpu, SIZE));
        Self { frames, handles, reads }
    }

    /// Draws frame `frame` of the scene into `view`, returning its target, its prepass targets
    /// and its stats.
    pub(super) fn frame(
        &mut self,
        gpu: &GpuContext,
        (texture, view): (&wgpu::Texture, &wgpu::TextureView),
        frame: u32,
    ) -> (Vec<Vec<[u8; 4]>>, RenderStats) {
        let frames = &mut self.frames;
        let camera = framing_camera(50.0, SIZE, 0.15);
        let packet = frames.packets.begin();
        packet.views.push(ViewData::new(camera));
        packet.sun = frames.sun;
        packet.sky = frames.sky;
        packet.ambient = frames.ambient;
        packet.post = frames.post;
        packet.time_seconds = 1000.0;
        packet.lights.push(point_light(Vec3::new(1.0, 1.0, 2.0), Vec3::ONE, 2.0, 5.0));
        // The scattered meshes, then the eyes, the tied object and the covering one.
        let kinds = self.handles.len() - 3;
        let place = |id: u64| {
            let (column, row) = ((id * 7) % 12, (id * 5) % 6);
            let behind = id % 9 == 4;
            let z = if behind { 30.0 } else { -2.0 + (id % 4) as f32 * 0.5 };
            Mat4::from_translation(Vec3::new(column as f32 * 0.6 - 3.3, row as f32 * 0.5 - 1.3, z))
        };
        // Which mesh each scattered instance draws: out of their upload order.
        let mesh = |id: u64| ((id * 5 + id / 7) as usize) % kinds;
        let skinned = |id: u64| matches!(mesh(id), 2 | 3);
        if frame == 0 {
            let tied = Mat4::from_translation(TIED_AT);
            packet.instances.changed.push(instance(COVERING, self.handles[kinds + 2], tied));
            packet.instances.changed.push(instance(TIED, self.handles[kinds + 1], tied));
            for id in 0..SCATTERED {
                packet.instances.changed.push(instance(id, self.handles[mesh(id)], place(id)));
                if id % 5 == 0 {
                    packet.instances.changed.push(instance(TWINS + id, self.handles[mesh(id)], place(id)));
                }
            }
        }
        // The rows move a little every frame, so their velocity isn't zero.
        for (first, mesh, count) in ROWS {
            for n in 0..count {
                let x = n as f32 * 0.45 - 2.6 + frame as f32 * 0.013;
                let at = Vec3::new(x, -1.6 + (first / 1000 - 4) as f32 * 0.5, -1.0);
                let row = Mat4::from_scale_rotation_translation(Vec3::splat(0.5), Quat::IDENTITY, at);
                packet.instances.changed.push(instance(first + n, self.handles[mesh], row));
            }
        }
        if frame == 0 {
            let eyes = Mat4::from_translation(Vec3::new(0.0, 0.0, 0.5));
            packet.instances.changed.push(instance(EYES, self.handles[kinds], eyes));
            for (n, &id) in IN_INSET.iter().enumerate() {
                let at = Mat4::from_translation(Vec3::new(n as f32 * 0.8 - 0.4, 0.0, 0.0));
                packet.instances.changed.push(instance(id, self.handles[n], at));
            }
        }
        // Poses on the first two frames, then held.
        for id in (0..SCATTERED).filter(|&id| skinned(id) && frame < 2) {
            let pose = bend(0.2 + frame as f32 * 0.1, 0.1);
            packet.push_skinned(InstanceId(id), pose.iter().copied(), [0.5]);
        }
        // A party: every third instance listed, one in four of them fading, and the eyes.
        for id in (0..SCATTERED).filter(|id| id % 3 == 0) {
            let opacity = if id % 4 == 0 { 0.5 } else { 1.0 };
            packet.characters.push(character(id, place(id).w_axis.truncate(), opacity));
        }
        packet.characters.push(character(EYES, Vec3::new(0.0, -0.5, 0.5), 1.0));
        let inset_camera = framing_camera(38.0, INSET, 0.6);
        packet.push_inset(InsetView::new(1, inset_camera, INSET, frames.ambient), IN_INSET.map(InstanceId));
        let mut ui = UiDrawList { viewport: [SIZE.0 as f32, SIZE.1 as f32], ..UiDrawList::default() };
        ui.push_inset_quad(1, UiQuad::picture([0.0, 0.0], [INSET.0 as f32, 0.0], [0.0, INSET.1 as f32], 255));
        packet.ui = ui;
        // The last frames replay the main pass as debug views.
        let debug =
            [DebugView::Off, DebugView::Off, DebugView::Off, DebugView::Overdraw, DebugView::Wireframe];
        frames.renderer.set_debug_view(debug[frame as usize % debug.len()]);

        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        frames.renderer.prepare(gpu, frames.packets.current());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", view);
        let targets = frames.renderer.record(&mut graph, target).expect("a view and a target");
        let picks = [
            Some(targets.character_depth),
            Some(targets.character_ids),
            Some(targets.depth),
            Some(targets.normals),
            Some(targets.outlines),
            targets.velocity,
        ];
        let picks: Vec<_> = picks.into_iter().flatten().collect();
        let reads = &self.reads;
        let mut pass = graph.add_pass("read prepasses").side_effect();
        for &texture in &picks {
            pass = pass.read(texture);
        }
        let read = picks.len();
        pass.record(move |ctx| {
            for (texture, readback) in picks.into_iter().zip(reads) {
                readback.copy(ctx.encoder, ctx.texture(texture));
            }
        });
        frames.graph.execute(gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none(), "no validation error");
        let mut images = vec![read_back(gpu, texture)];
        images.extend(self.reads[..read].iter().map(|readback| readback.texels(gpu)));
        (images, frames.renderer.stats())
    }
}

/// `passes` with every pass's calls zeroed: what runs leave as it was.
fn without_calls(passes: FrameCounts) -> FrameCounts {
    let calls = |pass: PassCounts| PassCounts { draws: 0, ..pass };
    FrameCounts {
        character_prepass: calls(passes.character_prepass),
        scene_prepass: calls(passes.scene_prepass),
        main: calls(passes.main),
        cascades: passes.cascades.map(calls),
        character_maps: calls(passes.character_maps),
        insets: passes.insets.map(calls),
        temporal: calls(passes.temporal),
        post: calls(passes.post),
        upscale: calls(passes.upscale),
        ui: calls(passes.ui),
        debug: calls(passes.debug),
    }
}

/// The scene's meshes, cooked: the sequences', the eyes, and the tied and covering objects.
pub(super) fn scene_meshes() -> Vec<AlignedVec> {
    let mut meshes = sequence_meshes();
    meshes.push(eyes().to_cooked().expect("cook"));
    meshes.extend(ties().map(|mesh| mesh.to_cooked().expect("cook")));
    meshes
}

#[test]
#[ignore = "needs a GPU"]
fn runs_draw_what_their_draws_drew_one_by_one_on_every_tier() {
    let meshes = scene_meshes();
    let names =
        ["target", "character depth", "character IDs", "depth", "normals", "outline mask", "velocity"];
    for tier in TierTable::shipped().tiers() {
        let gpu = gpu();
        let texture = target(&gpu, FORMAT, SIZE);
        let view = texture.create_view(&Default::default());
        let settings = tier.render_settings();
        // The default (the shadow views alone), the prepasses too, and every view, each against off.
        // Every view is exact on this scene, whose ties are between equal surfaces, but not on every
        // scene (the goldens' foliage and arena, M5 D2): the default is what's shipped.
        let modes = [DrawRuns::Shadows, DrawRuns::ShadowsAndPrepasses, DrawRuns::EveryView];
        let mut sides = [modes[0], modes[1], modes[2], DrawRuns::Off]
            .map(|runs| Side::new(&gpu, settings, runs, &meshes));
        // The shadow views' calls over the frames, with runs and without: a frame's cascades may hold
        // no two equal casters, the tier's frames together do.
        let mut shadow_calls = [0; 4];
        for frame in 0..5 {
            let [default, prepasses, every, one_by_one] = &mut sides;
            let (old_images, old_stats) = one_by_one.frame(&gpu, (&texture, &view), frame);
            assert_eq!(old_stats.runs, Default::default(), "{} frame {frame}: off forms none", tier.id);
            shadow_calls[3] += old_stats.passes.shadows().draws;
            for (index, (side, runs)) in [default, prepasses, every].into_iter().zip(modes).enumerate() {
                let (images, stats) = side.frame(&gpu, (&texture, &view), frame);
                let id = format!("{} {runs:?} frame {frame}", tier.id);
                for ((name, new), old) in names.iter().zip(&images).zip(&old_images) {
                    let (_, differing) = difference(new, old, 0);
                    assert_eq!(differing, 0, "{id}: {name} differs in {differing} texels");
                }
                assert!(images[0].iter().any(|texel| *texel != images[0][0]), "{id} draws");
                // Every pass draws the same instances and triangles and looks at and culls the same
                // draws; runs make fewer calls of them. The debug views' replays draw a call a draw.
                let (new, old) = (stats.passes, old_stats.passes);
                shadow_calls[index] += new.shadows().draws;
                if runs == DrawRuns::EveryView {
                    assert!(new.main.draws < old.main.draws, "{id}: runs in the main view");
                } else {
                    assert_eq!(new.main.draws, old.main.draws, "{id}: the main view a call a draw");
                    assert_eq!(new.insets, old.insets, "{id}: and the insets");
                }
                if runs == DrawRuns::Shadows {
                    assert_eq!(new.scene_prepass, old.scene_prepass, "{id}: the prepass a call a draw");
                    assert_eq!(new.character_prepass, old.character_prepass, "{id}: both prepasses");
                } else {
                    assert!(new.scene_prepass.draws < old.scene_prepass.draws, "{id}: runs in the prepass");
                }
                assert_eq!(new.debug, old.debug, "{id}: replays draw one by one");
                for ((name, pass), (_, before)) in new.passes().into_iter().zip(old.passes()) {
                    assert!(pass.draws <= before.draws, "{id}: {name}");
                }
                assert_eq!(without_calls(new), without_calls(old), "{id}");
                assert!(new.main.considered > new.main.mesh_instances(), "{id}: something's culled");
                // The rows lie in neighbouring slots, so they run in the prepasses' record order.
                if runs != DrawRuns::Shadows {
                    assert!(stats.runs.longest >= 8, "{id}: the rows run: {:?}", stats.runs);
                }
                // Runs change no more state than their draws did one by one.
                let (changes, before) = (stats.state_changes, old_stats.state_changes);
                assert!(changes.pipelines <= before.pipelines, "{id}: {changes:?} {before:?}");
                assert!(changes.materials <= before.materials, "{id}: {changes:?} {before:?}");
                assert!(changes.vertex_buffers <= before.vertex_buffers, "{id}: {changes:?} {before:?}");
            }
        }
        let [default, prepasses, every, one_by_one] = shadow_calls;
        assert!(
            one_by_one == 0 || default < one_by_one,
            "{}: runs in the shadows, {default} of {one_by_one}",
            tier.id
        );
        assert_eq!([prepasses, every], [default; 2], "{}: the shadows run alike", tier.id);
    }
}

/// Draws in the timed frame's main view: spec §3.4's desktop budget.
const TIMED_DRAWS: u64 = 2_500;
/// The timed frame's meshes, each drawn by a block of neighbouring instances.
const TIMED_MESHES: u64 = 10;
/// Frames timed a round, and rounds, each path taking turns going first.
const TIMED_FRAMES: usize = 60;
const TIMED_ROUNDS: usize = 10;

/// One path's renderer of the timed frame and the times it took.
struct Timed {
    frames: Frames,
    runs: DrawRuns,
    /// The squares, sent with the first frame.
    instances: Vec<MeshInstance>,
    unsent: bool,
    times: Vec<std::time::Duration>,
}

impl Timed {
    fn new(gpu: &GpuContext, runs: DrawRuns) -> Self {
        let settings = RenderSettings { draw_runs: runs, ..RenderSettings::default() };
        let mut frames = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
        let meshes: Vec<MeshHandle> = (0..TIMED_MESHES)
            .map(|n| {
                let shade = 0.2 + 0.06 * n as f32;
                upload(&mut frames.renderer, &square(colored([shade, 0.5, 1.0 - shade])))
            })
            .collect();
        // A grid of 50 by 50 small squares facing the camera, each mesh a block of 250 in slot order.
        let instances = (0..TIMED_DRAWS)
            .map(|id| {
                let (column, row) = ((id % 50) as f32, (id / 50) as f32);
                let at = Vec3::new(column * 0.1 - 2.45, row * 0.1 - 2.45, 0.0);
                let transform = Mat4::from_scale_rotation_translation(Vec3::splat(0.1), Quat::IDENTITY, at);
                instance(id, meshes[(id * TIMED_MESHES / TIMED_DRAWS) as usize], transform)
            })
            .collect();
        Self { frames, runs, instances, unsent: true, times: Vec::new() }
    }

    /// Draws [`TIMED_FRAMES`] frames, timing each one's record and the graph's execution, which
    /// records every pass: the CPU's share alone, as the GPU catches up between frames.
    #[allow(clippy::disallowed_methods, reason = "timing needs the OS clock, and nothing here ships")]
    fn round(&mut self, gpu: &GpuContext, view: &wgpu::TextureView, measured: bool) {
        let camera = CameraData::look_at(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO, Vec3::Y, 0.9, 1.0, 0.05);
        let frames = &mut self.frames;
        for _ in 0..TIMED_FRAMES {
            let packet = frames.packets.begin();
            packet.views.push(ViewData::new(camera));
            packet.sun = frames.sun;
            packet.sky = frames.sky;
            packet.ambient = frames.ambient;
            packet.post = frames.post;
            if std::mem::take(&mut self.unsent) {
                packet.instances.changed.extend_from_slice(&self.instances);
            }
            frames.renderer.prepare(gpu, frames.packets.current());
            let start = std::time::Instant::now();
            let mut graph = FrameGraph::new();
            let target = graph.import_texture("target", view);
            frames.renderer.record(&mut graph, target).expect("a view and a target");
            frames.graph.execute(gpu, graph).expect("a valid graph");
            let took = start.elapsed();
            if measured {
                self.times.push(took);
            }
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        }
        let main = frames.renderer.stats().passes.main;
        assert_eq!(main.instances, TIMED_DRAWS as u32 + 1, "every square in view, and the sky: {main:?}");
    }

    fn report(&mut self) {
        let main = self.frames.renderer.stats().passes.main;
        self.times.sort_unstable();
        let us = |at: usize| {
            self.times[(self.times.len() * at / 100).min(self.times.len() - 1)].as_secs_f64() * 1e6
        };
        eprintln!(
            "run_record_times: runs {:?}: the main pass {} calls of {} instances; record and execute p50 {:.1} µs, \
             p90 {:.1}, p99 {:.1}, over {} frames",
            self.runs,
            main.draws,
            main.instances,
            us(50),
            us(90),
            us(99),
            self.times.len(),
        );
    }
}

/// What 2,500 draws in the main view cost the frame's CPU, drawn as runs and a call a draw: one
/// renderer each, warmed by one round, then taking turns round by round so the machine's load falls
/// on both alike. Prints each one's median, 90th and 99th percentile; asserts nothing about time.
#[test]
#[ignore = "needs a GPU; a measurement, run alone (M5 D2)"]
fn run_record_times_at_the_desktop_budget() {
    let gpu = gpu();
    let size = (256, 256);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    // Every view, so the main pass's 2,500 draws run: the record time a run of them costs.
    let mut paths = [Timed::new(&gpu, DrawRuns::EveryView), Timed::new(&gpu, DrawRuns::Off)];
    for path in &mut paths {
        path.round(&gpu, &view, false);
    }
    for round in 0..TIMED_ROUNDS {
        let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
        for at in order {
            paths[at].round(&gpu, &view, true);
        }
    }
    for path in &mut paths {
        path.report();
    }
}

/// A square in the plane z = 0 whose corners' normals lean along x, its left and right apart, in one
/// opaque environment material: placed twice at one place, the second turned a quarter about z, the two
/// tie exactly in depth everywhere and are lit differently.
fn leaning_square() -> Mesh {
    let corners = rect((-0.6, -0.6), (0.6, 0.6), 0.0).map(|mut vertex| {
        vertex.normal = [0.6 * vertex.position[0].signum(), 0.0, 0.8];
        vertex
    });
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    let slots = vec![MaterialSlot::embedded("leaning", environment([0.8; 3]))];
    Mesh::from_vertices(&corners, vec![0, 1, 2, 0, 2, 3], submeshes, slots, PositionFormat::Float32)
}

/// Which of two instances of one mesh wins where they tie exactly in depth (M5 D2), drawn as two calls
/// and as one call of two instances (a run, in a main view that runs), in either order. Two calls keep
/// the first drawn, as every view's order assumes; the one call is asserted only to show one instance or
/// the other at every pixel, and what it does is printed, for the record of each backend (Metal on this
/// Mac; lavapipe and WARP in CI).
#[test]
#[ignore = "needs a GPU"]
fn a_tie_between_a_runs_instances_shows_one_of_them() {
    let gpu = gpu();
    let size = (64, 64);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(40.0, size, 0.5);
    // A quarter turn about z, exactly: the square's corners land on its corners.
    let quarter = Mat4::from_cols(glam::Vec4::Y, -glam::Vec4::X, glam::Vec4::Z, glam::Vec4::W);
    let draw = |runs: DrawRuns, placed: &[Mat4]| {
        let settings = RenderSettings { draw_runs: runs, msaa: Msaa::Off, ..RenderSettings::default() };
        let mut frames = Frames::with_settings(&gpu, FORMAT, settings).expect("renderer");
        frames.sun = DirectionalLight {
            direction: Vec3::new(-0.6, -0.3, -0.75).normalize(),
            color: Vec3::ONE,
            intensity: 1.0,
        };
        let mesh = upload(&mut frames.renderer, &leaning_square());
        let instances: Vec<MeshInstance> =
            placed.iter().enumerate().map(|(id, &transform)| instance(id as u64, mesh, transform)).collect();
        assert!(frames.render(&gpu, &view, camera, &instances, &[]).is_none());
        (read_back(&gpu, &texture), frames.renderer.stats().passes.main.draws)
    };
    let (still, _) = draw(DrawRuns::Off, &[Mat4::IDENTITY]);
    let (turned, _) = draw(DrawRuns::Off, &[quarter]);
    let apart: Vec<usize> = (0..still.len()).filter(|&at| still[at] != turned[at]).collect();
    assert!(apart.len() > 100, "the two are lit apart: {} pixels", apart.len());
    for (name, order) in
        [("still first", [Mat4::IDENTITY, quarter]), ("turned first", [quarter, Mat4::IDENTITY])]
    {
        let (first, second) = if order[0] == Mat4::IDENTITY { (&still, &turned) } else { (&turned, &still) };
        let (calls, call_draws) = draw(DrawRuns::Off, &order);
        let (one, one_draws) = draw(DrawRuns::EveryView, &order);
        assert_eq!((call_draws, one_draws), (2 + 1, 1 + 1), "{name}: two calls, then one, and the sky's");
        let shown = |image: &[[u8; 4]]| {
            let mut wins = [0; 2];
            for (at, texel) in image.iter().enumerate() {
                if texel == &first[at] {
                    wins[0] += usize::from(first[at] != second[at]);
                } else {
                    assert_eq!(texel, &second[at], "{name}: pixel {at} is one instance's or the other's");
                    wins[1] += 1;
                }
            }
            wins
        };
        let [calls_first, calls_second] = shown(&calls);
        let [one_first, one_second] = shown(&one);
        eprintln!(
            "a_tie_between_a_runs_instances: {name}, of {} pixels lit apart: two calls show the first drawn at \
             {calls_first} and the second at {calls_second}; one instanced call the first at {one_first} and \
             the second at {one_second}",
            apart.len()
        );
        assert_eq!((calls_first, calls_second), (apart.len(), 0), "{name}: two calls keep the first drawn");
    }
}
