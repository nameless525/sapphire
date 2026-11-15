//! Effects' bends (M5 L6): a bend moves the picture behind it by its strength, a share of the frame's height,
//! at any size and render scale, on either path and through a phone's edge upscaler (M5 L14); a character in
//! front of a bend or behind it comes out exactly as it does without one, and the world beside it unbent only
//! as far as the bend reaches; the bend fades out at the frame's edge; a flat frame stays flat; depth of field's
//! blurred background bends, and the glitch and the CRT keep their places; nothing of it is built, and no pass
//! added, until a bend draws on a tier that has them, and on one without (Mobile-Low) nothing ever is; declared,
//! nothing is built at the first bend on any tier with them, across a tier change too; and the offsets' pass
//! counts exactly in post's counts. Run with
//! `cargo test -p sapphire_render --test gpu vfx_distortion:: -- --ignored --test-threads=1`.
//!
//! The scenes are `vfx`'s: linear values as they are, over a flat grey sky, the camera at the origin looking down
//! -Z over 90° of the frame's height. Most draw an edge: a bright surface on the left half of the view and a dark
//! one on the right, four metres away, so the edge runs down the middle of the frame, and a card that bends the
//! picture outward from a middle well to the edge's left, so on the middle row it moves the edge to the right.

use glam::{Mat4, Quat, Vec2, Vec3};
use sapphire_assets::material::{AnimeCharacterMaterial, CharacterRegion, LightSplit, Outline, Rim};
use sapphire_render::quality::{TierId, TierTable};
use sapphire_render::{
    CharacterBody, CharacterFace, CharacterInstance, CharacterLighting, Crt, DepthOfFieldSettings,
    DirectionalLight, DistortionResolution, Focus, FrameCounts, Glitch, MaterialDrive, MeshInstance, Msaa,
    PassCounts, PipelineInventory, PostParams, PostVariants, RenderSettings, Upscaler, VfxBend,
    VfxDistortion, VfxMesh, VfxMeshShape, VfxPattern,
};

use super::vfx::{Frame, Harness, SIDE, passthrough, wall};

/// What the edge's two sides glow, linear.
const BRIGHT: f32 = 0.6;
const DARK: f32 = 0.1;
/// How far the test's bends move the picture: a twentieth of the frame's height.
const STRENGTH: f32 = 0.05;
/// How far the surfaces are.
const WALLS: f32 = 4.0;
/// How far the bending card is.
const CARD: f32 = 2.0;

/// Without temporal AA, as on the phones, and with it, as on the desktops: each with bends at half size.
fn paths() -> [(&'static str, RenderSettings); 2] {
    let distortion = DistortionResolution::Half;
    [
        (
            "no temporal AA",
            RenderSettings { msaa: Msaa::Off, bloom_levels: 5, distortion, ..RenderSettings::default() },
        ),
        (
            "temporal AA",
            RenderSettings { msaa: Msaa::Off, temporal: true, distortion, ..RenderSettings::default() },
        ),
    ]
}

/// Frames drawn before the one read: temporal AA's history settles on still edges.
fn frames(settings: &RenderSettings) -> u32 {
    if settings.temporal { 12 } else { 1 }
}

/// A card at `CARD` metres, its middle `middle` metres across and up from the view's axis, so wide it covers
/// the whole view, bending outward by `strength`.
fn bend(middle: Vec2, strength: f32) -> VfxDistortion {
    VfxDistortion {
        mesh: VfxMesh {
            shape: VfxMeshShape::Card,
            position: Vec3::new(middle.x, middle.y, -CARD),
            rotation: Quat::IDENTITY,
            scale: Vec3::new(40.0, 40.0, 1.0),
            color: Vec3::ZERO,
            alpha: 1.0,
            additive: 1.0,
            soft: 1e-3,
            pattern: VfxPattern::Solid,
            tiling: Vec2::ONE,
            scroll: Vec2::ZERO,
            erosion: None,
            flipbook: None,
        },
        bend: VfxBend::Outward,
        strength,
    }
}

/// The edge's bend: its middle a metre and a half to the left of the view's axis on the middle row.
fn edge_bend(strength: f32) -> VfxDistortion {
    bend(Vec2::new(-1.5, 0.0), strength)
}

/// A surface glowing `emission` from `left` to `right` metres across at `WALLS`, as high as the view and more.
fn add_band(harness: &mut Harness, left: f32, right: f32, emission: f32) -> MeshInstance {
    let half = (right - left) * 0.5;
    let transform = Mat4::from_scale_rotation_translation(
        Vec3::new(half, 40.0, 1.0),
        Quat::IDENTITY,
        Vec3::new(left + half, 0.0, -WALLS),
    );
    harness.add_surface(transform, Vec3::splat(emission))
}

/// The edge: bright left of `at` metres across, dark right of it.
fn add_edge(harness: &mut Harness, at: f32) {
    add_band(harness, -80.0, at, BRIGHT);
    add_band(harness, at, 80.0, DARK);
}

/// The last of `count` frames of `bends` and `post`.
fn draw(harness: &mut Harness, count: u32, post: PostParams, bends: &[VfxDistortion]) -> Frame {
    draw_with(harness, count, |packet| {
        packet.post = post;
        packet.vfx.distortions.extend_from_slice(bends);
    })
}

fn draw_with(harness: &mut Harness, count: u32, fill: impl Fn(&mut sapphire_render::FramePacket)) -> Frame {
    let mut frame = harness.render(&fill);
    for _ in 1..count {
        frame = harness.render(&fill);
    }
    frame
}

/// Where on `row` the edge between [`BRIGHT`] and [`DARK`] crosses half way, in pixels from the left, searched
/// over `columns`: the last bright pixel's middle and the share of the way to the next.
fn edge_at(frame: &Frame, row: u32, columns: std::ops::Range<u32>, levels: (f32, f32)) -> f32 {
    let half = 0.5 * (levels.0 + levels.1);
    let value = |x: u32| frame.linear((x, row)).x;
    for x in columns.start..columns.end - 1 {
        let (here, next) = (value(x), value(x + 1));
        if here >= half && next < half {
            return x as f32 + 0.5 + (here - half) / (here - next);
        }
    }
    panic!("no edge on row {row} in {columns:?}");
}

/// A bend moves the edge by its strength in output pixels, a share of the frame's height: at two sizes, one
/// wider than high, at render scales 1 and 0.6, without temporal AA and with it, and the same at a quarter's
/// buffer as at a half's.
#[test]
#[ignore = "needs a GPU"]
fn a_bend_moves_the_picture_by_its_strength_at_every_size_and_scale() {
    let mut paths = paths().to_vec();
    paths.push((
        "a quarter's buffer",
        RenderSettings { distortion: DistortionResolution::Quarter, ..paths[0].1 },
    ));
    // Mobile-High's (M5 L14): the distorted composite at the scene's size, then the edge upscaler, which takes a
    // bent frame to the target as it takes a plain one.
    paths.push((
        "the edge upscaler",
        RenderSettings {
            distortion: DistortionResolution::Quarter,
            upscaler: Upscaler::Edge { sharpen: 50 },
            ..paths[0].1
        },
    ));
    for (path, settings) in paths {
        for size in [(SIDE, SIDE), (256, 144)] {
            for scale in [1.0, 0.6] {
                let mut harness = Harness::sized(settings, size);
                harness.renderer.set_render_scale(scale);
                add_edge(&mut harness, 0.0);
                let count = frames(&settings);
                let row = size.1 / 2;
                let columns = size.0 / 4..size.0 * 3 / 4;
                let plain = draw(&mut harness, count, passthrough(), &[]);
                let before = edge_at(&plain, row, columns.clone(), (BRIGHT, DARK));
                let bent = draw(&mut harness, count, passthrough(), &[edge_bend(STRENGTH)]);
                let after = edge_at(&bent, row, columns, (BRIGHT, DARK));
                let expected = STRENGTH * size.1 as f32;
                let what = format!("{path}, {size:?} at {scale}");
                eprintln!("{what}: moved {:.3} px of {expected:.3}", after - before);
                assert!(
                    (after - before - expected).abs() < 0.5,
                    "{what}: the edge moved {} px, not {expected}",
                    after - before
                );
                assert!(harness.renderer.stats().post.distortion, "{what}: the distorted composite drew");
                let upscaled = settings.upscaler.edge() && scale < 1.0;
                assert_eq!(harness.renderer.stats().post.upscale, upscaled, "{what}: the upscaler drew");
                assert_eq!(
                    harness.renderer.stats().passes.upscale.draws,
                    if upscaled { 2 } else { 0 },
                    "{what}"
                );
            }
        }
    }
}

/// A flat character card `half` metres a side at `z`, as `vfx`'s character wall: its anime character material
/// lit flat, no line or rim.
fn add_character(harness: &mut Harness, z: f32, half: f32, across: f32) -> CharacterInstance {
    let material = harness.material(&AnimeCharacterMaterial {
        base_color: [0.3, 0.05, 0.05, 1.0],
        light_split: LightSplit { threshold: -1.0, softness: 0.02 },
        regions: vec![CharacterRegion { shade_color: [1.0; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    });
    let mesh = harness.mesh(&wall(0.0, half), material);
    let id = harness.add(mesh);
    harness.place(MeshInstance { id, mesh, transform: Mat4::from_translation(Vec3::new(across, 0.0, z)) });
    CharacterInstance {
        id,
        lighting: CharacterLighting::unstyled(DirectionalLight {
            direction: Vec3::NEG_Y,
            color: Vec3::ONE,
            intensity: 1.0,
        }),
        face: CharacterFace::default(),
        body: CharacterBody::default(),
        opacity: 1.0,
        drive: MaterialDrive::NONE,
    }
}

/// The pixels a square `half` metres a side `across` metres right of the axis at `z` covers, in a frame of
/// `size`, grown by `grow` pixels each way: (left, top, right, bottom), the right and bottom past it.
fn covered(size: (u32, u32), z: f32, half: f32, across: f32, grow: f32) -> (f32, f32, f32, f32) {
    // 90° of the height: a metre at depth d spans half the height over d.
    let pixels_per_metre = size.1 as f32 * 0.5 / -z;
    let middle = Vec2::new(size.0 as f32 * 0.5 + across * pixels_per_metre, size.1 as f32 * 0.5);
    let reach = half * pixels_per_metre + grow;
    (middle.x - reach, middle.y - reach, middle.x + reach, middle.y + reach)
}

fn inside((x, y): (u32, u32), (left, top, right, bottom): (f32, f32, f32, f32)) -> bool {
    let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
    x >= left && x <= right && y >= top && y <= bottom
}

/// A character comes out byte for byte as it does without a bend, in front of the bend and behind it, on both
/// paths, and so does the pixel round it. Beside it the world bends wherever the bend's reach doesn't come back
/// to the character: past the strength and three pixels from its silhouette every pixel is the bent world's, and
/// nearer, either that or as it is without the bend, never anything moved off the character.
#[test]
#[ignore = "needs a GPU"]
fn a_character_never_bends_and_the_world_beside_it_bends_but_for_the_bends_reach() {
    for (path, settings) in paths() {
        for z in [-1.5, -3.0] {
            let what = format!(
                "{path}, the character {} the bend",
                if z > -CARD { "in front of" } else { "behind" }
            );
            let size = (SIDE, SIDE);
            let count = frames(&settings);
            // Stripes, so the world changes wherever it bends: a metre's bands.
            let stripes = |harness: &mut Harness| {
                for band in -12..12 {
                    let left = band as f32 * 0.5;
                    add_band(harness, left, left + 0.5, if band % 2 == 0 { BRIGHT } else { DARK });
                }
            };
            let (half, across) = (0.6 * -z / WALLS, -z / WALLS);
            // Each from a harness of its own, so temporal AA's history and jitter are the same in both.
            let with = |bends: bool| {
                let mut harness = Harness::sized(settings, size);
                stripes(&mut harness);
                let character = add_character(&mut harness, z, half, across);
                draw_with(&mut harness, count, move |packet: &mut sapphire_render::FramePacket| {
                    packet.post = passthrough();
                    packet.characters.push(character);
                    if bends {
                        packet.vfx.distortions.push(edge_bend(STRENGTH));
                    }
                })
            };
            let (plain, bent) = (with(false), with(true));
            let world = |bends: &[VfxDistortion]| {
                let mut harness = Harness::sized(settings, size);
                stripes(&mut harness);
                draw(&mut harness, count, passthrough(), bends)
            };
            let (world_plain, world_bent) = (world(&[]), world(&[edge_bend(STRENGTH)]));

            let body = covered(size, z, half, across, 1.0);
            let reach = STRENGTH * size.1 as f32 + 2.0;
            let near = covered(size, z, half, across, 1.0 + reach);
            let (mut exact, mut fringe, mut moved) = (0, 0, 0);
            for y in 0..size.1 {
                for x in 0..size.0 {
                    let at = (x, y);
                    let index = (y * size.0 + x) as usize;
                    if inside(at, body) {
                        assert_eq!(
                            bent.pixels[index], plain.pixels[index],
                            "{what}: the character at {at:?}"
                        );
                        exact += 1;
                        continue;
                    }
                    let (shown, bent_world, plain_world) =
                        (bent.linear(at), world_bent.linear(at), world_plain.linear(at));
                    let close = |a: Vec3, b: Vec3| (a - b).abs().max_element() < 0.02;
                    moved += usize::from(!close(bent_world, plain_world));
                    if inside(at, near) {
                        // Bent, or as the frame without the bend, where temporal AA's resolve spreads the
                        // character a little past its silhouette. Bent, its read may land in that spread, which
                        // differs a little from the world's alone; but never on the character, whose red would
                        // show against the grey world.
                        let unbent = plain.linear(at);
                        let near_enough = |a: Vec3, b: Vec3| (a - b).abs().max_element() < 0.05;
                        let grey = (shown.x - shown.y).abs() < 0.01;
                        assert!(
                            close(shown, unbent) || (grey && near_enough(shown, bent_world)),
                            "{what}: {at:?} by the character shows {shown}, neither {bent_world} nor {unbent}"
                        );
                        fringe += 1;
                    } else {
                        assert!(close(shown, bent_world), "{what}: {at:?} shows {shown}, not {bent_world}");
                    }
                }
            }
            eprintln!("{what}: {exact} character pixels exact, {fringe} by it, {moved} the bend moves");
            assert!(exact > 100 && moved > 1000, "{what}: the test sees the character and the bend");
        }
    }
}

/// Near the frame's edge a bend fades out over a twentieth of the height: an edge three pixels from the frame's
/// right moves a pixel and a half where the same edge in the middle moves the bend's whole strength.
#[test]
#[ignore = "needs a GPU"]
fn a_bend_fades_out_at_the_frames_edge() {
    let (_, settings) = paths()[0];
    let size = (SIDE, SIDE);
    // Three pixels from the right at four metres: the half width is four metres over 64 pixels.
    let at = WALLS * (1.0 - 3.0 / 64.0);
    let mut harness = Harness::sized(settings, size);
    add_edge(&mut harness, at);
    let row = size.1 / 2;
    let columns = size.0 - 12..size.0;
    let plain = draw(&mut harness, 1, passthrough(), &[]);
    let bent = draw(&mut harness, 1, passthrough(), &[bend(Vec2::new(-1.5, 0.0), STRENGTH)]);
    let moved =
        edge_at(&bent, row, columns.clone(), (BRIGHT, DARK)) - edge_at(&plain, row, columns, (BRIGHT, DARK));
    // The bend's reach and its fade's, both 6.4 pixels here: the edge settles where its distance from the
    // frame's right, u, solves u (1 + 6.4 / 6.4) = 3.
    eprintln!("three pixels from the edge: moved {moved:.3} px");
    assert!((moved - 1.5).abs() < 0.6, "moved {moved} px");
    for y in [0, size.1 - 1] {
        for x in 0..size.0 {
            let pixel = bent.linear((x, y));
            assert!(pixel.is_finite() && pixel.max_element() <= BRIGHT + 0.02, "({x}, {y}): {pixel}");
        }
    }
}

/// A flat frame bends to itself: under the default post (bloom, vignette, the grade and all), a bend changes no
/// pixel by more than one step, so a bend can neither brighten nor darken it, nor flash.
#[test]
#[ignore = "needs a GPU"]
fn a_flat_frame_stays_flat_under_a_bend() {
    for (path, settings) in paths() {
        let mut harness = Harness::sized(settings, (SIDE, SIDE));
        add_band(&mut harness, -80.0, 80.0, 0.5);
        let count = frames(&settings);
        let post = PostParams::default();
        let plain = draw(&mut harness, count, post, &[]);
        let bends = [edge_bend(0.1), bend(Vec2::new(1.0, 0.7), -0.1)];
        let bent = draw(&mut harness, count, post, &bends);
        assert!(harness.renderer.stats().post.distortion, "{path}: it drew bent");
        let most = plain
            .pixels
            .iter()
            .zip(&bent.pixels)
            .flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c])))
            .max()
            .unwrap_or(0);
        assert!(most <= 1, "{path}: a pixel moved {most} steps");
    }
}

/// A shimmering column round the camera, `radius` metres out and ten high, the camera a fifth of the way up.
fn column_round_the_camera(radius: f32) -> VfxDistortion {
    let mut column = bend(Vec2::ZERO, STRENGTH);
    column.mesh.shape = VfxMeshShape::Column;
    column.mesh.position = Vec3::new(0.0, -2.0, 0.0);
    column.mesh.scale = Vec3::new(radius, 10.0, radius);
    column.mesh.pattern = VfxPattern::Noise;
    column.bend = VfxBend::Shimmer;
    column
}

/// A bend fades out near the camera, none nearer than half a metre: a column the camera stands in, 0.4 m round
/// it, leaves the frame byte for byte as it is without the bend, where the same column three metres out
/// shimmers over the picture. (Bends past a metre and a half move it as they did: the card the other tests bend
/// with is two metres out.)
#[test]
#[ignore = "needs a GPU"]
fn a_bend_round_the_camera_fades_out() {
    for (path, settings) in paths() {
        let size = (SIDE, SIDE);
        let count = frames(&settings);
        // Each from a harness of its own, so temporal AA's history and jitter are the same in all three.
        let frame = |bends: &[VfxDistortion]| {
            let mut harness = Harness::sized(settings, size);
            for band in -12..12 {
                let left = band as f32 * 0.5;
                add_band(&mut harness, left, left + 0.5, if band % 2 == 0 { BRIGHT } else { DARK });
            }
            let frame = draw(&mut harness, count, passthrough(), bends);
            let stats = harness.renderer.stats();
            assert_eq!(stats.post.distortion, !bends.is_empty(), "{path}: it drew bent where it had a bend");
            frame
        };
        let plain = frame(&[]);
        let close = frame(&[column_round_the_camera(0.4)]);
        let far = frame(&[column_round_the_camera(3.0)]);
        let moved = |frame: &Frame| plain.pixels.iter().zip(&frame.pixels).filter(|(a, b)| a != b).count();
        let (close_moved, far_moved) = (moved(&close), moved(&far));
        eprintln!("{path}: the column 0.4 m round moves {close_moved} pixels, 3 m round {far_moved}");
        assert_eq!(close_moved, 0, "{path}: a column round the camera at 0.4 m moved pixels");
        assert!(far_moved > 1000, "{path}: the column three metres out shimmers");
    }
}

/// Depth of field runs before the bend: the blurred background is what bends, the blurred edge moving by the
/// bend's strength as the sharp one does.
#[test]
#[ignore = "needs a GPU"]
fn depth_of_fields_blurred_background_bends() {
    let settings = RenderSettings {
        msaa: Msaa::Off,
        temporal: true,
        depth_of_field: DepthOfFieldSettings { view: true, insets: false },
        distortion: DistortionResolution::Half,
        ..RenderSettings::default()
    };
    let post = PostParams {
        focus: Focus { distance: 1.0, sharp: 0.2, near: 0.5, far: 1.0, blur: 0.02 },
        ..passthrough()
    };
    let size = (SIDE, SIDE);
    let mut harness = Harness::sized(settings, size);
    add_edge(&mut harness, 0.0);
    let (row, columns) = (size.1 / 2, size.0 / 4..size.0 * 3 / 4);
    let sharp = draw(&mut harness, 12, passthrough(), &[]);
    let plain = draw(&mut harness, 12, post, &[]);
    assert!(harness.renderer.stats().post.focus, "depth of field drew");
    // The blur spreads the edge: the pixel two to its left is darker than the sharp frame's.
    let at = (edge_at(&sharp, row, columns.clone(), (BRIGHT, DARK)) - 2.0) as u32;
    assert!(plain.linear((at, row)).x < sharp.linear((at, row)).x - 0.02, "the background is blurred");
    let bent = draw(&mut harness, 12, post, &[edge_bend(STRENGTH)]);
    let stats = harness.renderer.stats().post;
    assert!(stats.focus && stats.distortion, "both drew");
    let moved =
        edge_at(&bent, row, columns.clone(), (BRIGHT, DARK)) - edge_at(&plain, row, columns, (BRIGHT, DARK));
    let expected = STRENGTH * size.1 as f32;
    eprintln!("blurred: moved {moved:.3} px of {expected:.3}");
    assert!((moved - expected).abs() < 0.5, "the blurred edge moved {moved} px");
}

/// The effects pack composes: the glitch moves where the frame is read and the bend moves it on, so a glitched
/// row's edge moves by the bend's strength from where the glitch put it; the CRT's scanlines stay on the screen,
/// so a bend over a flat part changes nothing and the edge still moves.
#[test]
#[ignore = "needs a GPU"]
fn the_glitch_and_the_crt_compose_with_a_bend() {
    let (_, settings) = paths()[0];
    let size = (SIDE, SIDE);
    let mut effects = passthrough().effects;
    effects.glitch = Glitch { amount: 1.0, bands: 16, share: 1.0, shift: 0.04, split: 0.0, seed: 7 };
    let glitched = PostParams { effects, ..passthrough() };
    let mut effects = passthrough().effects;
    effects.crt = Crt { amount: 1.0, lines: 32.0, depth: 0.5, grille: 0.0, corner: 0.0 };
    let crt = PostParams { effects, ..passthrough() };
    let expected = STRENGTH * size.1 as f32;
    for (name, post) in [("the glitch", glitched), ("the CRT", crt)] {
        let mut harness = Harness::sized(settings, size);
        add_edge(&mut harness, 0.0);
        let plain = draw(&mut harness, 1, post, &[]);
        let bent = draw(&mut harness, 1, post, &[edge_bend(STRENGTH)]);
        assert!(harness.renderer.stats().post.distortion, "{name}: it drew bent");
        for row in [size.1 / 2 - 3, size.1 / 2, size.1 / 2 + 2] {
            // The row's own two sides, which the scanlines darken alike.
            let levels = (plain.linear((2, row)).x, plain.linear((size.0 - 3, row)).x);
            let columns = 8..size.0 - 8;
            let moved = edge_at(&bent, row, columns.clone(), levels) - edge_at(&plain, row, columns, levels);
            assert!((moved - expected).abs() < 0.6, "{name}, row {row}: moved {moved} px of {expected}");
        }
        if name == "the CRT" {
            // Over the dark side's flat part every row keeps its scanline's darkness.
            for y in 0..size.1 {
                let x = size.0 - 20;
                let (a, b) = (plain.linear((x, y)), bent.linear((x, y)));
                assert!((a - b).abs().max_element() < 0.01, "{name}: ({x}, {y}) {b} against {a}");
            }
        }
    }
}

/// Nothing of the bends is built and no pass added until a frame has a bend: no pipeline, buffer, texture or
/// pass for frames without one, and an effect mesh's first frame builds its own colour pipeline alone; the first
/// bend builds the offsets' pipeline and the distorted composite and adds the offsets' pass, and a frame without
/// one again drops the pass. Declared, they are built ahead, so the first bend builds nothing.
#[test]
#[ignore = "needs a GPU"]
fn nothing_is_built_and_no_pass_added_until_a_bend_draws() {
    let colored = VfxMesh { color: Vec3::splat(0.1), ..bend(Vec2::ZERO, 0.0).mesh };
    for (path, settings) in paths() {
        let mut meshes = Harness::sized(settings, (SIDE, SIDE));
        add_edge(&mut meshes, 0.0);
        meshes.render(|_| {});
        let before = meshes.render_pipelines();
        for _ in 0..3 {
            meshes.render(|packet| packet.vfx.meshes.push(colored));
        }
        assert_eq!(meshes.render_pipelines(), before + 1, "{path}: the meshes' colour pipeline alone");
        assert!(!meshes.renderer.stats().post.distortion, "{path}");
        assert_eq!(meshes.renderer.stats().vfx.distortions, 0, "{path}");

        let mut harness = Harness::sized(settings, (SIDE, SIDE));
        add_edge(&mut harness, 0.0);
        for _ in 0..2 {
            harness.render(|_| {});
        }
        let pipelines = harness.render_pipelines();
        let resources = harness.resources();
        let passes = harness.graph.stats().passes;
        for _ in 0..3 {
            harness.render(|_| {});
        }
        assert_eq!(harness.render_pipelines(), pipelines, "{path}: no pipeline");
        assert_eq!(harness.resources(), resources, "{path}: no buffer or texture");
        assert_eq!(harness.graph.stats().passes, passes, "{path}: no pass");
        assert!(!harness.renderer.stats().post.distortion, "{path}");

        harness.render(|packet| packet.vfx.distortions.push(edge_bend(STRENGTH)));
        assert_eq!(
            harness.render_pipelines(),
            pipelines + 2,
            "{path}: the offsets' and the distorted composite"
        );
        assert_eq!(harness.graph.stats().passes, passes + 1, "{path}: the offsets' pass");
        assert_eq!(harness.renderer.stats().vfx.distortions, 1, "{path}");
        harness.render(|packet| packet.vfx.distortions.push(edge_bend(STRENGTH)));
        assert_eq!(harness.render_pipelines(), pipelines + 2, "{path}: none again");
        harness.render(|_| {});
        assert_eq!(harness.graph.stats().passes, passes, "{path}: the pass goes with the bends");
        assert!(!harness.renderer.stats().post.distortion, "{path}: the plain composite again");

        let mut declared = Harness::sized(settings, (SIDE, SIDE));
        declared.render(|_| {});
        let before = declared.render_pipelines();
        let built = declared
            .renderer
            .prewarm(&PipelineInventory { distortion: true, ..PipelineInventory::default() });
        assert!(built >= 2, "{path}: the offsets' and the variants' distorted composites: {built}");
        let first_uses = declared.renderer.stats().pipelines.first_use_builds;
        declared.render(|packet| packet.vfx.distortions.push(edge_bend(STRENGTH)));
        assert_eq!(
            declared.renderer.stats().pipelines.first_use_builds,
            first_uses,
            "{path}: none at the first"
        );
        assert!(declared.render_pipelines() >= before + 2, "{path}");
    }
}

/// Mobile-Low has no bends: with every pipeline declared and a frame full of bends it builds nothing of them,
/// adds no pass, and draws the frame byte for byte as without them.
#[test]
#[ignore = "needs a GPU"]
fn mobile_low_builds_nothing_of_the_bends() {
    let table = TierTable::shipped();
    let tier = table.tiers().iter().find(|tier| tier.id == TierId::MobileLow).expect("Mobile-Low ships");
    let settings = tier.render_settings();
    assert_eq!(settings.distortion, DistortionResolution::Off);
    let mut harness = Harness::sized(settings, (SIDE, SIDE));
    add_edge(&mut harness, 0.0);
    let plain = draw(&mut harness, 2, passthrough(), &[]);
    let pipelines = harness.render_pipelines();
    let passes = harness.graph.stats().passes;
    let declared = PipelineInventory {
        effects: true,
        effect_meshes: true,
        decals: true,
        distortion: true,
        ..PipelineInventory::default()
    };
    let built = harness.renderer.prewarm(&declared);
    let after_prewarm = harness.render_pipelines();
    assert_eq!(after_prewarm - pipelines, built as usize, "only what the other effects declared");
    let bends = [edge_bend(STRENGTH), bend(Vec2::new(1.0, 0.5), 0.1)];
    let bent = draw(&mut harness, 2, passthrough(), &bends);
    assert_eq!(harness.render_pipelines(), after_prewarm, "nothing at the bends");
    assert_eq!(harness.graph.stats().passes, passes, "no pass");
    assert_eq!(harness.renderer.stats().vfx.distortions, 0);
    assert!(!harness.renderer.stats().post.distortion);
    assert!(bent.pixels == plain.pixels, "the frame as without them");

    // And a renderer with bends off builds none when they're declared alone.
    let mut off = Harness::sized(settings, (SIDE, SIDE));
    off.render(|_| {});
    assert_eq!(
        off.renderer.prewarm(&PipelineInventory { distortion: true, ..PipelineInventory::default() }),
        0
    );
}

/// The shipped tier `id`'s settings.
fn tier_settings(id: TierId) -> RenderSettings {
    let table = TierTable::shipped();
    table.tiers().iter().find(|tier| tier.id == id).expect("the tier ships").render_settings()
}

/// What a game's load declares of its effects when they bend (M5 L5, L7, L6), with the compositing variants.
fn effects_declared() -> PipelineInventory {
    PipelineInventory {
        effects: true,
        effect_meshes: true,
        decals: true,
        distortion: true,
        post: PostVariants::COMPOSITING,
        ..PipelineInventory::default()
    }
}

/// A load that declares bends builds them ahead on every tier with them, Mobile-High among them: the first bend
/// builds nothing. They stay declared across a tier change (M4 T3), to the class's other tier and back as exit
/// test 1's pause does, Mobile-Low's turn among them, which built none of them: back on the tier with bends, the
/// first bend builds nothing. And a screen effect's variants a later load prewarms come with their distorted
/// composites.
#[test]
#[ignore = "needs a GPU"]
fn declared_bends_build_nothing_on_every_tier_with_them_and_across_a_tier_change() {
    let mut effects = passthrough().effects;
    effects.glitch = Glitch { amount: 1.0, bands: 16, share: 1.0, shift: 0.04, split: 0.0, seed: 7 };
    let glitched = PostParams { effects, ..passthrough() };
    // Mobile-High first: the walk CI failed (its pause changes to Mobile-Low and back).
    let changes = [
        (TierId::MobileHigh, TierId::MobileLow),
        (TierId::DesktopHigh, TierId::DesktopLow),
        (TierId::DesktopLow, TierId::DesktopHigh),
    ];
    for (held, other) in changes {
        let settings = tier_settings(held);
        assert!(settings.distortion.any(), "{held} has bends");
        let mut harness = Harness::sized(settings, (SIDE, SIDE));
        add_edge(&mut harness, 0.0);
        harness.render(|_| {});
        let declared = PipelineInventory { distortion: false, ..effects_declared() };
        harness.renderer.prewarm(&declared);
        let built = harness.renderer.prewarm(&effects_declared());
        assert!(built >= 2, "{held}: the offsets' and the variants' distorted composites: {built}");
        let first_uses = harness.renderer.stats().pipelines.first_use_builds;
        draw(&mut harness, 2, passthrough(), &[edge_bend(STRENGTH)]);
        assert!(harness.renderer.stats().post.distortion, "{held}: it drew bent");
        let at_first = harness.renderer.stats().pipelines.first_use_builds;
        assert_eq!(at_first, first_uses, "{held}: none at the first bend");

        harness.change_tier(tier_settings(other));
        draw(&mut harness, 2, passthrough(), &[edge_bend(STRENGTH)]);
        harness.change_tier(settings);
        let first_uses = harness.renderer.stats().pipelines.first_use_builds;
        draw(&mut harness, 2, passthrough(), &[edge_bend(STRENGTH)]);
        assert!(harness.renderer.stats().post.distortion, "{held}, back from {other}: it drew bent");
        let back = harness.renderer.stats().pipelines.first_use_builds;
        assert_eq!(back, first_uses, "{held}, back from {other}: none at the first bend");

        let screen_effects =
            PipelineInventory { post: PostVariants::SCREEN_EFFECTS, ..PipelineInventory::default() };
        harness.renderer.prewarm(&screen_effects);
        draw(&mut harness, 2, glitched, &[edge_bend(STRENGTH)]);
        assert!(harness.renderer.stats().post.distortion, "{held}: the glitch drew bent");
        let later = harness.renderer.stats().pipelines.first_use_builds;
        assert_eq!(later, first_uses, "{held}: none at the glitch's first bend");
    }

    // Mobile-Low builds none of them, and hands their declaration to Mobile-High.
    let mut harness = Harness::sized(tier_settings(TierId::MobileLow), (SIDE, SIDE));
    add_edge(&mut harness, 0.0);
    harness.render(|_| {});
    harness.renderer.prewarm(&PipelineInventory { distortion: false, ..effects_declared() });
    assert_eq!(harness.renderer.prewarm(&effects_declared()), 0, "Mobile-Low: none of the bends");
    harness.change_tier(tier_settings(TierId::MobileHigh));
    let first_uses = harness.renderer.stats().pipelines.first_use_builds;
    draw(&mut harness, 2, passthrough(), &[edge_bend(STRENGTH)]);
    assert!(harness.renderer.stats().post.distortion, "Mobile-High, from Mobile-Low: it drew bent");
    let first = harness.renderer.stats().pipelines.first_use_builds;
    assert_eq!(first, first_uses, "Mobile-High, from Mobile-Low: none at the first bend");
}

/// The offsets' pass counts in post's counts (M5 B4), exactly: a call of one instance a bend, its card's two
/// triangles, and nothing else of any pass moves; the distorted composite stands in the plain one's place.
#[test]
#[ignore = "needs a GPU"]
fn the_offsets_pass_counts_exactly_in_posts_counts() {
    let triangles = VfxMeshShape::Card.geometry().indices.len() as u64 / 3;
    for (path, settings) in paths() {
        let mut harness = Harness::sized(settings, (SIDE, SIDE));
        add_edge(&mut harness, 0.0);
        let count = frames(&settings);
        draw(&mut harness, count, passthrough(), &[]);
        let without = harness.renderer.stats().passes;
        for bends in [1_u32, 3] {
            let list: Vec<_> = (0..bends).map(|k| bend(Vec2::new(k as f32 - 1.0, 0.0), 0.01)).collect();
            draw(&mut harness, count, passthrough(), &list);
            let with = harness.renderer.stats().passes;
            let added = PassCounts {
                draws: with.post.draws - without.post.draws,
                instances: with.post.instances - without.post.instances,
                triangles: with.post.triangles - without.post.triangles,
                ..PassCounts::default()
            };
            assert_eq!(
                added,
                PassCounts {
                    draws: bends,
                    instances: bends,
                    triangles: triangles * u64::from(bends),
                    ..PassCounts::default()
                },
                "{path}: {bends} bends"
            );
            let others = |counts: &FrameCounts| FrameCounts { post: PassCounts::default(), ..*counts };
            assert_eq!(others(&with), others(&without), "{path}: no other pass moves");
            let post_rest =
                |counts: &PassCounts| PassCounts { draws: 0, instances: 0, triangles: 0, ..*counts };
            assert_eq!(post_rest(&with.post), post_rest(&without.post), "{path}: nothing else of post's");
        }
    }
}
