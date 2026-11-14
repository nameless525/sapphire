//! Effect meshes (M5 L5): a scrolled pattern lands where its offset says, an erosion at 0, part way and 1 with
//! its edge's color only in its band, each flipbook frame, the soft fade over temporal AA and none in the main
//! pass, that nothing of the family is built until a mesh draws or a load declares one, and that a mesh counts
//! in the pass that draws it (M5 B4). Run with
//! `cargo test -p sapphire_render --test gpu vfx_mesh:: -- --ignored --test-threads=1`.
//!
//! The scenes are `vfx`'s: linear values as they are, over a flat grey sky. Most draw a card that fills the
//! view exactly, so a pixel's place is its UV.

use glam::{Quat, Vec2, Vec3};
use sapphire_render::{
    FrameCounts, PassCounts, PipelineInventory, VfxErosion, VfxFlipbook, VfxMesh, VfxMeshShape, VfxPattern,
};

use super::vfx::{GREY, Harness, SIDE, assert_near, flat_settings, paths, temporal_settings};

/// A card a metre in front of the camera, two metres a side, so it fills the 90° view: a pixel's UV is its
/// middle's place across the frame, `v` down.
fn card(color: Vec3, pattern: VfxPattern) -> VfxMesh {
    VfxMesh {
        shape: VfxMeshShape::Card,
        position: Vec3::new(0.0, 0.0, -1.0),
        rotation: Quat::IDENTITY,
        scale: Vec3::new(2.0, 2.0, 1.0),
        color,
        alpha: 1.0,
        additive: 1.0,
        soft: 1e-3,
        pattern,
        tiling: Vec2::ONE,
        scroll: Vec2::ZERO,
        erosion: None,
        flipbook: None,
    }
}

/// Pixel `(x, y)`'s UV on the card.
fn uv((x, y): (u32, u32)) -> Vec2 {
    Vec2::new((x as f32 + 0.5) / SIDE as f32, (y as f32 + 0.5) / SIDE as f32)
}

fn pixels() -> impl Iterator<Item = (u32, u32)> {
    (0..SIDE).flat_map(|y| (0..SIDE).map(move |x| (x, y)))
}

const COLOR: Vec3 = Vec3::new(0.5, 0.35, 0.2);

/// The pattern's offset is the scroll's speed times the layer's time: 1.5 UVs a second half a second in is
/// three quarters of a repeat toward +u, so every pixel shows what the unscrolled card shows 96 pixels to its
/// left.
#[test]
#[ignore = "needs a GPU"]
fn a_scrolled_pattern_lands_where_speed_times_time_says() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let still = harness.render(|packet| packet.vfx.meshes.push(card(COLOR, VfxPattern::Streaks)));
        let offset = (1.5_f32 * 0.5).rem_euclid(1.0);
        let scrolled = VfxMesh { scroll: Vec2::new(offset, 0.0), ..card(COLOR, VfxPattern::Streaks) };
        let moved = harness.render(|packet| packet.vfx.meshes.push(scrolled));
        let shift = (offset * SIDE as f32).round() as u32;
        let mut varied = 0;
        for y in [10, 40, 64, 90, 120] {
            for x in 0..SIDE {
                let expected = still.linear(((x + SIDE - shift) % SIDE, y));
                assert_near(moved.linear((x, y)), expected, &format!("{path}: ({x}, {y})"));
                varied += usize::from((expected - still.linear((x, y))).abs().max_element() > 0.05);
            }
        }
        assert!(varied > 100, "{path}: the pattern varies along u, so the test can see it move: {varied}");
    }
}

/// What an eroded card shows at a pixel: the sky where the mask is under the threshold's low end, the edge's
/// color across the band above it, the card's color past it; `None` near a boundary, where filtering decides.
fn eroded(mask: f32, threshold: f32, edge: f32) -> Option<Option<bool>> {
    let low = threshold * (1.0 + edge) - edge;
    let margin = 0.08;
    if mask < low - margin {
        Some(None)
    } else if mask > low + margin && mask < low + edge - margin {
        Some(Some(true))
    } else if mask > low + edge + margin {
        Some(Some(false))
    } else {
        None
    }
}

/// An erosion at 0 shows the whole card with no edge, at 1 nothing, and part way the sky where the mask is
/// under the threshold, the edge's color only in its band, and the card's own past it.
#[test]
#[ignore = "needs a GPU"]
fn an_erosion_eats_by_its_mask_and_its_edge_shows_only_in_its_band() {
    let edge_color = Vec3::new(0.1, 0.6, 0.7);
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let eroding = |threshold: f32| VfxMesh {
            erosion: Some(VfxErosion {
                mask: VfxPattern::Noise,
                tiling: Vec2::ONE,
                threshold,
                edge: 0.3,
                edge_color,
            }),
            ..card(COLOR, VfxPattern::Solid)
        };
        let whole = harness.render(|packet| packet.vfx.meshes.push(eroding(0.0)));
        let gone = harness.render(|packet| packet.vfx.meshes.push(eroding(1.0)));
        for at in pixels() {
            assert_near(whole.linear(at), Vec3::splat(GREY) + COLOR, &format!("{path}: whole at {at:?}"));
            assert_near(gone.linear(at), Vec3::splat(GREY), &format!("{path}: gone at {at:?}"));
        }
        let part = harness.render(|packet| packet.vfx.meshes.push(eroding(0.5)));
        let (mut counts, mut wrong) = ([0; 3], Vec::new());
        for at in pixels() {
            let mask = VfxPattern::Noise.coverage(uv(at).x, uv(at).y);
            let Some(expected) = eroded(mask, 0.5, 0.3) else { continue };
            let color = Vec3::splat(GREY)
                + match expected {
                    None => Vec3::ZERO,
                    Some(true) => edge_color,
                    Some(false) => COLOR,
                };
            counts[match expected {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            }] += 1;
            if (part.linear(at) - color).abs().max_element() > 0.04 {
                wrong.push((at, mask, part.linear(at)));
            }
        }
        assert!(
            counts.iter().all(|&count| count > 400),
            "{path}: each kind of pixel is in the frame: {counts:?}"
        );
        assert!(
            wrong.len() < 40,
            "{path}: {} of {counts:?} unlike the mask says, first {:?}",
            wrong.len(),
            wrong.first()
        );
    }
}

/// The burst's sheet at `(u, v)` as its top level is sampled: its texels' coverages in eight bits, filtered
/// linearly between their middles. Its rays are sharp enough that the pattern itself would differ from them.
fn sheet(u: f32, v: f32) -> f32 {
    let side = sapphire_look::vfx::mesh::PATTERN_SIZE as f32;
    let texel = |i: f32, j: f32| {
        let (i, j) = (i.rem_euclid(side), j.rem_euclid(side));
        (VfxPattern::Burst.coverage((i + 0.5) / side, (j + 0.5) / side) * 255.0).round() / 255.0
    };
    let (x, y) = (u * side - 0.5, v * side - 0.5);
    let (i, j) = (x.floor(), y.floor());
    let (fx, fy) = (x - i, y - j);
    let top = texel(i, j) * (1.0 - fx) + texel(i + 1.0, j) * fx;
    let bottom = texel(i, j + 1.0) * (1.0 - fx) + texel(i + 1.0, j + 1.0) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Each of the burst's sixteen frames shows its own cell of the sheet, filling the card; blended half way, a
/// frame is the mean of it and the next.
#[test]
#[ignore = "needs a GPU"]
fn each_flipbook_frame_shows_its_own_cell() {
    let mut harness = Harness::new(temporal_settings());
    let showing = |harness: &mut Harness, frame: u32, next: u32, blend: f32| {
        let book = VfxFlipbook { columns: 4, rows: 4, frame, next, blend };
        harness.render(|packet| {
            packet.vfx.meshes.push(VfxMesh { flipbook: Some(book), ..card(COLOR, VfxPattern::Burst) })
        })
    };
    let mut frames = Vec::new();
    for frame in 0..16 {
        let shown = showing(&mut harness, frame, frame, 0.0);
        let (column, row) = ((frame % 4) as f32, (frame / 4) as f32);
        let mut far = 0;
        for at in pixels() {
            let coverage = sheet((column + uv(at).x) / 4.0, (row + uv(at).y) / 4.0);
            far += usize::from(
                (shown.linear(at) - (Vec3::splat(GREY) + COLOR * coverage)).abs().max_element() > 0.03,
            );
        }
        assert!(far < 50, "frame {frame}: {far} pixels unlike its cell");
        frames.push(shown);
    }
    for pair in frames.windows(2) {
        assert_ne!(pair[0].pixels, pair[1].pixels, "frames differ");
    }
    let between = showing(&mut harness, 5, 6, 0.5);
    for at in pixels().step_by(7) {
        let mean = (frames[5].linear(at) + frames[6].linear(at)) * 0.5;
        assert_near(between.linear(at), mean, &format!("half way at {at:?}"));
    }
}

/// Over temporal AA a mesh fades into the surface behind it over its soft distance, as a sprite does; in the
/// main pass (phones) it has no depth to read, so it shows whole right up to the surface, and not behind it.
#[test]
#[ignore = "needs a GPU"]
fn a_mesh_fades_into_surfaces_over_temporal_aa_and_cuts_on_phones() {
    let wall_color = Vec3::new(0.05, 0.1, 0.05);
    let bright = Vec3::new(0.8, 0.4, 0.2);
    let at_gap = |gap: f32| VfxMesh {
        position: Vec3::new(0.0, 0.0, -5.0 + gap),
        scale: Vec3::splat(4.0),
        soft: 1.0,
        ..card(bright, VfxPattern::Solid)
    };
    let mut soft = Harness::new(temporal_settings());
    soft.add_wall(-5.0, wall_color);
    soft.render(|_| {});
    let mut shown =
        |gap: f32| soft.render(|packet| packet.vfx.meshes.push(at_gap(gap))).middle() - wall_color;
    assert_near(shown(0.01), bright * 0.01, "at the wall");
    assert_near(shown(0.5), bright * 0.5, "half way");
    assert_near(shown(2.0), bright, "clear of it");

    let mut cut = Harness::new(flat_settings());
    cut.add_wall(-5.0, wall_color);
    cut.render(|_| {});
    let mut shown = |gap: f32| cut.render(|packet| packet.vfx.meshes.push(at_gap(gap))).middle() - wall_color;
    assert_near(shown(0.01), bright, "on a phone, whole at the wall");
    assert_near(shown(-0.5), Vec3::ZERO, "and hidden behind it");
}

/// A frame with sprites alone builds nothing of the meshes, and one with meshes alone nothing of the sprites:
/// each family's first draw builds its own color pipeline, once. Declared, the meshes' pipelines are built
/// ahead, so their first draw builds none.
#[test]
#[ignore = "needs a GPU"]
fn nothing_of_the_family_is_built_until_a_mesh_draws_or_is_declared() {
    let sprite = super::vfx::dot(Vec3::new(0.0, 0.0, -4.0), 0.5, Vec3::ONE);
    let mesh = card(COLOR, VfxPattern::Streaks);
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        for _ in 0..2 {
            harness.render(|_| {});
        }
        let pipelines = harness.render_pipelines();
        for _ in 0..3 {
            harness.render(|packet| packet.vfx.sprites.push(sprite));
        }
        assert_eq!(harness.render_pipelines(), pipelines + 1, "{path}: the sprites' color pipeline alone");
        let (_, sprite_textures) = harness.resources();
        assert_eq!(harness.renderer.stats().vfx.meshes, 0, "{path}");
        harness.render(|packet| {
            packet.vfx.sprites.push(sprite);
            packet.vfx.meshes.push(mesh);
        });
        assert_eq!(harness.render_pipelines(), pipelines + 2, "{path}: and the meshes' at the first mesh");
        // What a backend keeps of a texture held only by its view varies, so count that some came, not how many.
        assert!(
            harness.resources().1 > sprite_textures,
            "{path}: the meshes' textures come with the first mesh"
        );
        assert_eq!(harness.renderer.stats().vfx.pipelines, 2, "{path}");
        assert_eq!(harness.renderer.stats().vfx.meshes, 1, "{path}");

        let mut meshes_only = Harness::new(settings);
        for _ in 0..2 {
            meshes_only.render(|_| {});
        }
        meshes_only.render(|packet| packet.vfx.meshes.push(mesh));
        assert_eq!(meshes_only.render_pipelines(), pipelines + 1, "{path}: the meshes' color pipeline alone");
        assert_eq!(meshes_only.renderer.stats().vfx.pipelines, 1, "{path}");

        let mut declared = Harness::new(settings);
        declared.render(|_| {});
        let before = declared.render_pipelines();
        declared.renderer.prewarm(&PipelineInventory { effect_meshes: true, ..PipelineInventory::default() });
        assert_eq!(declared.render_pipelines(), before + 2, "{path}: the meshes' color and mask");
        let first_uses = declared.renderer.stats().pipelines.first_use_builds;
        declared.render(|packet| packet.vfx.meshes.push(mesh));
        assert_eq!(declared.render_pipelines(), before + 2, "{path}: none at the first mesh");
        assert_eq!(declared.renderer.stats().pipelines.first_use_builds, first_uses, "{path}");
    }
}

/// A mesh and a covering sprite sort together by depth: the nearer covers the farther whichever comes first in
/// the frame.
#[test]
#[ignore = "needs a GPU"]
fn a_mesh_sorts_among_sprites_by_depth() {
    let red = Vec3::new(0.6, 0.0, 0.0);
    let blue = Vec3::new(0.0, 0.0, 0.6);
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let covering = |position: Vec3, color: Vec3| VfxMesh {
            position,
            scale: Vec3::splat(6.0),
            additive: 0.0,
            ..card(color, VfxPattern::Solid)
        };
        let mut sprite = super::vfx::dot(Vec3::new(0.0, 0.0, -3.0), 2.0, blue);
        sprite.additive = 0.0;
        let near_sprite = harness.render(|packet| {
            packet.vfx.sprites.push(sprite);
            packet.vfx.meshes.push(covering(Vec3::new(0.0, 0.0, -4.0), red));
        });
        assert_near(near_sprite.middle(), blue, &format!("{path}: the sprite in front"));
        let near_mesh = harness.render(|packet| {
            packet.vfx.sprites.push(sprite);
            packet.vfx.meshes.push(covering(Vec3::new(0.0, 0.0, -2.0), red));
        });
        assert_near(near_mesh.middle(), red, &format!("{path}: the mesh in front"));
    }
}

/// A shape's own fade: a ring seen from above shows its band and nothing at its middle.
#[test]
#[ignore = "needs a GPU"]
fn a_ring_from_above_shows_its_band_and_not_its_middle() {
    let mut harness = Harness::new(flat_settings());
    let ring = VfxMesh {
        shape: VfxMeshShape::Ring,
        position: Vec3::new(0.0, 0.0, -2.0),
        rotation: Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        scale: Vec3::ONE,
        ..card(COLOR, VfxPattern::Solid)
    };
    let frame = harness.render(|packet| packet.vfx.meshes.push(ring));
    assert_near(frame.middle(), Vec3::splat(GREY), "its middle");
    // At radius 0.9 (two metres away, 90°: a pixel is 1/32 m), its band at full.
    let band = (SIDE / 2 + (0.9 * 32.0) as u32, SIDE / 2);
    assert!((frame.linear(band) - Vec3::splat(GREY)).max_element() > COLOR.x * 0.6, "{}", frame.linear(band));
}

/// An effect mesh counts where it draws (M5 B4): in the main pass where no temporal AA follows, as on phones,
/// and in temporal AA's counts where it draws after the resolve; each draw a call of one instance and the
/// card's triangles, with no level (it isn't a scene mesh). No other pass moves.
#[test]
#[ignore = "needs a GPU"]
fn an_effect_mesh_counts_in_the_pass_that_draws_it() {
    let mesh = card(COLOR, VfxPattern::Solid);
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        for _ in 0..2 {
            harness.render(|_| {});
        }
        let without = harness.renderer.stats().passes;
        harness.render(|packet| packet.vfx.meshes.push(mesh));
        let with = harness.renderer.stats().passes;
        let (pass, drew): (&dyn Fn(&FrameCounts) -> PassCounts, _) = if path == "the main pass" {
            (&|counts| counts.main, "main")
        } else {
            (&|counts| counts.temporal, "temporal")
        };
        let (before, after) = (pass(&without), pass(&with));
        let calls = after.draws - before.draws;
        assert!(calls >= 1, "{path}: the mesh's draws count in {drew}");
        assert_eq!(after.instances - before.instances, calls, "{path}: one instance a call");
        let triangles = after.triangles - before.triangles;
        assert!(triangles >= 2 * u64::from(calls), "{path}: at least the card's two triangles a call");
        assert_eq!(triangles % u64::from(calls), 0, "{path}: the same card each call");
        assert_eq!(
            (after.considered, after.levels),
            (before.considered, before.levels),
            "{path}: no mesh draw"
        );
        let others = |counts: &FrameCounts| FrameCounts {
            main: PassCounts::default(),
            temporal: PassCounts::default(),
            ..*counts
        };
        assert_eq!(others(&with), others(&without), "{path}: no other pass moves");
    }
}
