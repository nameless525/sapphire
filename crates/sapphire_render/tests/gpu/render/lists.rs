//! Each view's own draw list (M5 D1) draws what the filtered walk drew before it, byte for byte:
//! two renderers given the same frames, one walking the lists and one walking the whole draw list
//! with its tests (`Renderer::walk_filtered`), on every shipped tier's settings. The scene has a
//! listed party (static and skinned anime characters with outline hulls, one fading), eyes and
//! their layers through hair, many environment instances out of mesh and material order, some
//! behind the camera, some placed twice at the same place, exact ties in depth between two
//! materials of one object and between two objects (which a material-first or a mesh-first order
//! of the views that write colour would flip: m5-plan D2 meets them here), transparent surfaces,
//! foliage and fog cards, every cascade and the character maps, an inset, and two debug views'
//! replays. Each frame
//! compares the target, the scene prepass's depth, normals and outline mask, and the character
//! prepass's depth and IDs; and every pass's counts but the debug views'. m5-plan D2 deletes the
//! filtered walk, and this test with it; `render::culling` draws the same scene through each
//! culling mode (M5 D3), and keeps [`Side`] and [`scene_meshes`]. Run alone with
//! `cargo test -p sapphire_render --test gpu render::lists:: -- --ignored`.

use sapphire_assets::material::AlignedVec;
use sapphire_assets::material::{
    AlphaMode, CharacterSurface, EnvironmentMaterial, EyeLayer, EyeShading, HairShading, HighlightMotion,
    MaterialDocument, ThroughHair,
};
use sapphire_render::quality::TierTable;
use sapphire_render::{DebugView, InsetView, RenderStats, UiDrawList, UiQuad};

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

/// A renderer and the targets it reads back.
pub(super) struct Side {
    frames: Frames,
    handles: Vec<MeshHandle>,
    /// The character prepass's depth and IDs, the scene prepass's depth, normals and outlines.
    reads: [Readback; 5],
}

impl Side {
    pub(super) fn new(
        gpu: &GpuContext,
        settings: RenderSettings,
        filtered: bool,
        meshes: &[AlignedVec],
    ) -> Self {
        let mut frames = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
        frames.renderer.walk_filtered(filtered);
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
            targets.character_depth,
            targets.character_ids,
            targets.depth,
            targets.normals,
            targets.outlines,
        ];
        let reads = &self.reads;
        let mut pass = graph.add_pass("read prepasses").side_effect();
        for texture in picks {
            pass = pass.read(texture);
        }
        pass.record(move |ctx| {
            for (texture, readback) in picks.into_iter().zip(reads) {
                readback.copy(ctx.encoder, ctx.texture(texture));
            }
        });
        frames.graph.execute(gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none(), "no validation error");
        let mut images = vec![read_back(gpu, texture)];
        images.extend(self.reads.iter().map(|readback| readback.texels(gpu)));
        (images, frames.renderer.stats())
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
fn view_lists_draw_what_the_filtered_walk_drew_on_every_tier() {
    let meshes = scene_meshes();
    let names = ["target", "character depth", "character IDs", "depth", "normals", "outline mask"];
    for tier in TierTable::shipped().tiers() {
        let gpu = gpu();
        let texture = target(&gpu, FORMAT, SIZE);
        let view = texture.create_view(&Default::default());
        let settings = tier.render_settings();
        let mut sides = [false, true].map(|filtered| Side::new(&gpu, settings, filtered, &meshes));
        for frame in 0..5 {
            let [listed, filtered] = &mut sides;
            let (images, stats) = listed.frame(&gpu, (&texture, &view), frame);
            let (old_images, old_stats) = filtered.frame(&gpu, (&texture, &view), frame);
            for ((name, new), old) in names.iter().zip(&images).zip(&old_images) {
                let (_, differing) = difference(new, old, 0);
                assert_eq!(differing, 0, "{} frame {frame}: {name} differs in {differing} texels", tier.id);
            }
            assert!(images[0].iter().any(|texel| *texel != images[0][0]), "{} frame {frame} draws", tier.id);
            // Every pass counts what it did before, but the debug views', whose replays count
            // only what they walk now.
            let (mut new, mut old) = (stats.passes, old_stats.passes);
            (new.debug, old.debug) = Default::default();
            assert_eq!(new, old, "{} frame {frame}", tier.id);
            assert!(
                new.main.considered > new.main.mesh_draws(),
                "{} frame {frame}: something's culled",
                tier.id
            );
            // On their lists, the passes change no more state than they did.
            let (changes, before) = (stats.state_changes, old_stats.state_changes);
            assert!(
                changes.pipelines <= before.pipelines,
                "{} frame {frame}: {changes:?} {before:?}",
                tier.id
            );
            assert!(
                changes.materials <= before.materials,
                "{} frame {frame}: {changes:?} {before:?}",
                tier.id
            );
            assert!(
                changes.vertex_buffers <= before.vertex_buffers,
                "{} frame {frame}: {changes:?} {before:?}",
                tier.id
            );
        }
    }
}
