//! A depth-equal main pass (M5 D10) draws every byte the main pass drew with its own depth: two
//! renderers given the same frames, one taking the scene prepass's depth and testing `Equal` against it
//! and one drawing its own (`RenderSettings::main_depth`), on every shipped tier's settings, through
//! [`Side`]'s scene (`render::runs`: a listed party of static and skinned anime characters with hulls, one
//! fading, two-sided ones, eyes and their layers through hair, exact ties between two materials of one
//! object and between two objects, foliage in the wind, transparent surfaces, fog cards, an inset and two
//! debug views' replays), at render scale 1 and below it. Each frame compares every byte of the target,
//! the scene prepass's depth, normals, outline mask and velocity and the character prepass's depth and
//! IDs, and the passes' counts. The target's bytes are asserted where the adapter runs a fragment the depth
//! test killed whole for its quad's derivatives (`render::killed_fragments`' probe draw says, by behaviour):
//! the terminator's width takes the derivative of a value its shadow lookups compute in loops, so where a
//! killed neighbour's loops stop short (lavapipe), a pixel's color already depends on which of its quad's
//! neighbours a depth test killed, with the pass's own depth too; there the test still asserts no holes and
//! the six prepass-derived targets byte for byte, and prints the target's count and what it didn't assert.
//! A desktop tier takes the prepass's depth; a phone, without temporal AA,
//! keeps its own and builds the same pipelines either way. A frame the prepass can't stand in for (an
//! environment alpha-tested draw, which the prepass doesn't cut out, or runs in the main view) keeps its
//! own too, drawing what it drew: the tiers run the main view (M5 D2), so the comparisons draw with the
//! shadow views' runs alone (`DrawRuns::Shadows`). And a hull lying exactly on a two-sided surface, a tie between draws the
//! reverse walk can't keep, is counted (a flat one shows nowhere, so none changes hands). Run alone with
//! `cargo test -p sapphire_render --test gpu render::depth_equal:: -- --ignored --test-threads=1`.

use sapphire_assets::material::{AlignedVec, AlphaMode};
use sapphire_render::quality::{QualityTier, TierTable};
use sapphire_render::{DrawRuns, MainDepth, RenderStats, scaled_size};

use super::killed_fragments::derivatives_read_a_killed_fragment_whole;
use super::runs::{SIZE, Side, scene_meshes};
use super::sequences::environment_sphere;
use super::*;

/// The targets [`Side::frame`] returns, by name.
const NAMES: [&str; 7] =
    ["target", "character depth", "character IDs", "depth", "normals", "outline mask", "velocity"];

/// How many texels of `new` differ from `old` in any byte.
fn differing(new: &[[u8; 4]], old: &[[u8; 4]]) -> usize {
    new.iter().zip(old).filter(|(new, old)| new != old).count()
}

/// The target's texels `new` left black (the main pass's clear, where nothing it drew passed `Equal`
/// and the sky, at the far plane alone, didn't either) where `old` drew something: holes.
fn holes(new: &[[u8; 4]], old: &[[u8; 4]]) -> usize {
    let black = |texel: &[u8; 4]| texel[..3] == [0; 3];
    new.iter().zip(old).filter(|(new, old)| black(new) && !black(old)).count()
}

/// How many differing texels of each target [`report`] lists.
const LISTED: usize = 16;

/// Everything a frame whose targets differ saw, printed before it fails: each target's count of
/// differing texels (all of them, not the first that differs), the holes, and up to [`LISTED`]
/// differing texels of each, with their places and both sides' bytes; for the target's, the scene
/// prepass's depth at the same places (the scene's size is `scale` of the target's) on both sides,
/// as bytes and as the depth they hold. `new` is the prepass's side, `old` its own.
fn report(id: &str, new: &[Vec<[u8; 4]>], old: &[Vec<[u8; 4]>], scale: f32) -> String {
    let width = SIZE.0 as usize;
    let at = |index: usize| (index % width, index / width);
    let places = |new: &[[u8; 4]], old: &[[u8; 4]]| -> Vec<usize> {
        new.iter().zip(old).enumerate().filter(|(_, (new, old))| new != old).map(|(index, _)| index).collect()
    };
    let mut lines = vec![format!("{id}: holes {}", holes(&new[0], &old[0]))];
    for ((name, new), old) in NAMES.iter().zip(new).zip(old) {
        let differ = places(new, old);
        lines.push(format!("{id}: {name}: {} texels differ", differ.len()));
        for &index in differ.iter().take(LISTED) {
            let (x, y) = at(index);
            lines.push(format!("  {name} ({x}, {y}): prepass {:?}, own {:?}", new[index], old[index]));
        }
    }
    // The scene prepass's depth under the target's differing texels.
    let (scene_width, scene_height) = scaled_size(SIZE, scale);
    let depth = NAMES.iter().position(|name| *name == "depth").expect("the depth target");
    for &index in places(&new[0], &old[0]).iter().take(LISTED) {
        let (x, y) = at(index);
        let scene = |coordinate: usize, size: u32, of: u32| {
            ((coordinate as f32 + 0.5) * size as f32 / of as f32).floor() as usize
        };
        let (sx, sy) = (scene(x, scene_width, SIZE.0), scene(y, scene_height, SIZE.1));
        let texel = sy * width + sx;
        let (prepass, own) = (new[depth][texel], old[depth][texel]);
        lines.push(format!(
            "  depth under the target's ({x}, {y}), at the scene's ({sx}, {sy}): prepass {prepass:?} ({}), \
             own {own:?} ({})",
            f32::from_le_bytes(prepass),
            f32::from_le_bytes(own)
        ));
    }
    lines.join("\n")
}

/// Whether this adapter keeps the target's premise (a pixel's color depends only on its own fragments,
/// whatever a depth test killed beside it in its quad), by [`derivatives_read_a_killed_fragment_whole`];
/// where it doesn't, says once what the test doesn't assert there and why.
fn colors_asserted(gpu: &GpuContext, test: &str) -> bool {
    let whole = derivatives_read_a_killed_fragment_whole(gpu);
    if !whole {
        let caps = gpu.caps();
        eprintln!(
            "depth_equal: {test}: the target's bytes not asserted on {} ({:?}): its derivatives read a \
             fragment the depth test killed as its loops left it, not run whole (the probe draw), so a \
             pixel's color depends on which of its quad's neighbours a depth test killed, with the pass's \
             own depth too; no holes and the six prepass-derived targets are asserted byte for byte, and \
             the target's differing texels printed",
            caps.adapter.name,
            caps.backend()
        );
    }
    whole
}

/// Draws `frames` frames of [`Side`]'s scene through two renderers of `settings`, one with each main
/// depth, at `scale`, asserting every byte and count alike (and on a mismatch printing [`report`]
/// first), the target's bytes only where `colors` (from [`colors_asserted`]; elsewhere printing their
/// count); returns the last frames' stats, the prepass's side's first.
fn compare(
    gpu: &GpuContext,
    settings: RenderSettings,
    meshes: &[AlignedVec],
    (scale, colors): (f32, bool),
    label: &str,
) -> (RenderStats, RenderStats) {
    let texture = target(gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let side = |main_depth| {
        let mut side = Side::new(gpu, RenderSettings { main_depth, ..settings }, settings.draw_runs, meshes);
        side.set_render_scale(scale);
        side
    };
    let (mut prepass, mut own) = (side(MainDepth::FromPrepass), side(MainDepth::Own));
    let mut last = None;
    for frame in 0..5 {
        let (old, old_stats) = own.frame(gpu, (&texture, &view), frame);
        let (new, stats) = prepass.frame(gpu, (&texture, &view), frame);
        let id = format!("{label} at scale {scale}, frame {frame}");
        assert_eq!(old_stats.main_depth, MainDepth::Own, "{id}: own is own");
        assert_eq!(new.len(), old.len(), "{id}: the same targets");
        let differ = NAMES.iter().zip(&new).zip(&old).any(|((_, new), old)| differing(new, old) > 0);
        if differ || holes(&new[0], &old[0]) > 0 {
            eprintln!("{}", report(&id, &new, &old, scale));
        }
        assert_eq!(holes(&new[0], &old[0]), 0, "{id}: no pixel the main pass left at its clear");
        if !colors {
            let count = differing(&new[0], &old[0]);
            eprintln!("depth_equal: {id}: {count} texels of the target differ (not asserted)");
        }
        let asserted = if colors { 0 } else { 1 };
        for ((name, new), old) in NAMES.iter().zip(&new).zip(&old).skip(asserted) {
            assert_eq!(differing(new, old), 0, "{id}: {name} differs");
        }
        assert!(new[0].iter().any(|texel| *texel != new[0][0]), "{id} draws");
        // The same draws, instances and triangles, looked at and culled alike, in every pass.
        assert_eq!(stats.passes, old_stats.passes, "{id}");
        last = Some((stats, old_stats));
    }
    last.expect("frames")
}

#[test]
#[ignore = "needs a GPU"]
fn the_main_pass_draws_every_byte_it_drew_testing_equal_on_every_tier() {
    let meshes = scene_meshes();
    let mut desktop = 0;
    let colors = colors_asserted(&gpu(), "every tier");
    for tier in TierTable::shipped().tiers() {
        let gpu = gpu();
        let settings = asked(tier);
        let takes = settings.main_depth_from_prepass();
        desktop += usize::from(takes);
        for scale in [1.0, 0.7] {
            let label = tier.id.to_string();
            let (stats, own) = compare(&gpu, settings, &meshes, (scale, colors), &label);
            let expected = if takes { MainDepth::FromPrepass } else { MainDepth::Own };
            assert_eq!(stats.main_depth, expected, "{label}: the settings say");
            let (built, before) = (stats.pipelines.built(), own.pipelines.built());
            eprintln!(
                "depth_equal: {label} at scale {scale}: {:?}, {built} pipelines built, {before} with its own depth",
                stats.main_depth
            );
            if takes {
                assert!(built > before, "{label}: the twins are built beside");
            } else {
                assert_eq!(built, before, "{label}: a tier without temporal AA builds nothing new");
            }
        }
    }
    assert!(desktop >= 2, "both desktop tiers take the prepass's depth");
}

/// `tier`'s settings asking for the prepass's depth, with the shadow views' runs alone: runs in the main
/// view, the tiers', keep its own.
fn asked(tier: &QualityTier) -> RenderSettings {
    RenderSettings {
        main_depth: MainDepth::FromPrepass,
        draw_runs: DrawRuns::Shadows,
        ..tier.render_settings()
    }
}

/// The first shipped tier's settings that take the prepass's depth when asked: a desktop tier's.
fn desktop_settings() -> RenderSettings {
    let table = TierTable::shipped();
    let tier = table.tiers().iter().find(|tier| asked(tier).main_depth_from_prepass());
    asked(tier.expect("a desktop tier"))
}

#[test]
#[ignore = "needs a GPU"]
fn a_frame_the_prepass_cant_stand_in_for_keeps_its_own_depth() {
    let settings = desktop_settings();
    let gpu = gpu();
    let colors = colors_asserted(&gpu, "a frame that keeps its own depth");
    // Runs in the main view: a run's instances can't be walked in reverse.
    let meshes = scene_meshes();
    let every = RenderSettings { draw_runs: DrawRuns::EveryView, ..settings };
    let (stats, _) = compare(&gpu, every, &meshes, (1.0, colors), "every view's runs");
    assert_eq!(stats.main_depth, MainDepth::Own, "runs in the main view keep its own");
    // An environment alpha-tested surface among the scattered meshes: the prepass draws it uncut.
    let mut meshes = scene_meshes();
    let cut = environment_sphere(AlphaMode::Mask { cutoff: 0.5 }).to_cooked().expect("cook");
    meshes.insert(meshes.len() - 5, cut);
    let (stats, _) = compare(&gpu, settings, &meshes, (1.0, colors), "an environment alpha-tested draw");
    assert_eq!(stats.main_depth, MainDepth::Own, "an uncut surface keeps its own");
}

/// A square facing away from the camera, two-sided, whose hull's vertices lie exactly on its own: no
/// width (vertex color green 0) and no depth offset (blue 0, the material's offset and flattening 0).
fn hull_on_its_surface() -> Mesh {
    let corners = [(-0.6, -0.6), (0.6, -0.6), (0.6, 0.6), (-0.6, 0.6)].map(|(x, y)| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, -1.0],
        outline_normal: [0.0, 0.0, -1.0],
        uv: [0.5; 2],
        color: [255, 0, 0],
        flags: VertexFlags::NONE,
    });
    let material = AnimeCharacterMaterial {
        double_sided: true,
        outline: Outline { z_offset: 0.0, flatten: 0.0, ..flat([0.9, 0.5, 0.2], 5.0).outline },
        ..flat([0.9, 0.5, 0.2], 5.0)
    };
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    let slots = vec![MaterialSlot::embedded("hull on its surface", cooked(&material))];
    Mesh::from_vertices(&corners, vec![0, 1, 2, 0, 2, 3], submeshes, slots, PositionFormat::Float32)
}

/// Where a hull ties its own two-sided surface exactly, today the surface keeps the pixel (drawn first,
/// it passed `Greater`; the hull, after it, didn't); testing `Equal` in reverse, the hull, drawn last,
/// does. Such a hull fights its surface pixel by pixel with its own depth too (a hair's difference either
/// way decides), so the scene shows noise either way: counted and printed, for the record of each backend,
/// and asserted only to be a share of the square's pixels, never the whole.
#[test]
#[ignore = "needs a GPU"]
fn a_hull_on_its_own_two_sided_surface_is_counted() {
    let settings = desktop_settings();
    let gpu = gpu();
    let size = (96, 96);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(40.0, size, 0.5);
    let draw = |main_depth, hulls: bool| {
        let mut frames =
            Frames::with_settings(&gpu, FORMAT, RenderSettings { main_depth, ..settings }).expect("renderer");
        frames.renderer.set_render_scale(1.0);
        frames.renderer.set_outline_distance(if hulls { 1.0 } else { 0.0 });
        let mesh = upload(&mut frames.renderer, &hull_on_its_surface());
        let turned = Mat4::from_rotation_y(0.3);
        let square = [instance(1, mesh, turned)];
        let mut images = Vec::new();
        for frame in 0..4 {
            let changed: &[MeshInstance] = if frame == 0 { &square } else { &[] };
            assert!(frames.render(&gpu, &view, camera, changed, &[]).is_none(), "no validation error");
            images.push(read_back(&gpu, &texture));
        }
        (images, frames.renderer.stats())
    };
    let (own, own_stats) = draw(MainDepth::Own, true);
    let (equal, equal_stats) = draw(MainDepth::FromPrepass, true);
    // The square alone: where the hull shows, it won its fight with its surface.
    let (bare, _) = draw(MainDepth::Own, false);
    assert_eq!((own_stats.main_depth, equal_stats.main_depth), (MainDepth::Own, MainDepth::FromPrepass));
    assert_eq!(own_stats.passes.main.outline_draws, 1, "the hull draws");
    let sky = own[0][0];
    for (frame, ((own, equal), bare)) in own.iter().zip(&equal).zip(&bare).enumerate() {
        let square = own.iter().filter(|texel| **texel != sky).count();
        let changed = differing(equal, own);
        let hull = differing(own, bare);
        eprintln!(
            "depth_equal: a hull on its own two-sided surface, frame {frame}: {changed} of the square's {square} \
             pixels change hands; the hull shows at {hull} with the main pass's own depth"
        );
        assert!(square > 100, "frame {frame}: the square shows");
        assert!(changed < square, "frame {frame}: a share, not the whole");
    }
}

/// The probe draw (`render::killed_fragments`) prints each shape's verdict with the killed pixel at each
/// corner of the quad, side by side, and both draws' values where a shape says it isn't run whole. Straight
/// code, with nothing for a killed fragment to leave unfinished, must leave the three pixels as they are
/// on any adapter: the probe itself works.
#[test]
#[ignore = "needs a GPU"]
fn the_probe_says_whether_a_killed_fragment_runs_whole() {
    use super::killed_fragments::{LOOPED, STRAIGHT, verdicts};
    let gpu = gpu();
    let caps = gpu.caps();
    let verdicts = verdicts(&gpu);
    for shape in LOOPED.into_iter().chain([STRAIGHT]) {
        let line: Vec<String> = verdicts
            .iter()
            .filter(|verdict| verdict.shape == shape)
            .map(|verdict| {
                let word = if verdict.whole { "whole" } else { "NOT whole" };
                format!("killed {:?} {word}", verdict.killed)
            })
            .collect();
        eprintln!(
            "killed_fragments: {} ({:?}): {shape}: {}",
            caps.adapter.name,
            caps.backend(),
            line.join(", ")
        );
    }
    for verdict in verdicts.iter().filter(|verdict| !verdict.whole) {
        eprintln!(
            "killed_fragments: {} killed at {:?}: occluded {:?}, open {:?}",
            verdict.shape, verdict.killed, verdict.occluded, verdict.open
        );
    }
    let whole =
        verdicts.iter().filter(|verdict| LOOPED.contains(&verdict.shape)).all(|verdict| verdict.whole);
    eprintln!("killed_fragments: the adapter runs a killed fragment whole for its quad: {whole}");
    for verdict in verdicts.iter().filter(|verdict| verdict.shape == STRAIGHT) {
        assert!(verdict.whole, "straight code killed at {:?} leaves its quad as it was", verdict.killed);
    }
}
