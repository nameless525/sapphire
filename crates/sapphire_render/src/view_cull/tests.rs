use glam::{Affine3A, Vec3};
use sapphire_assets::material::{MaterialFamily, MaterialFlags};
use sapphire_assets::mesh::PositionFormat;

use super::*;
use crate::characters::CharacterList;
use crate::gpu_types::Dequantize;
use crate::materials::MaterialHandle;
use crate::mesh_pool::{DeformSource, MeshStreams, Placement, SubmeshDraw, SubmeshMaterial};
use crate::scene::DrawLimits;
use crate::{InstanceDeltas, InstanceId, MeshHandle, MeshInstance};

const SIZE: (u32, u32) = (640, 360);
const NEAR: f32 = 0.05;

/// A camera at `eye` looking down −z, 60° high.
fn camera_at(eye: Vec3) -> CameraData {
    CameraData::look_at(eye, eye + Vec3::NEG_Z, Vec3::Y, 60f32.to_radians(), 640.0 / 360.0, NEAR)
}

fn camera() -> CameraData {
    camera_at(Vec3::ZERO)
}

fn frustum(pad: f32) -> Frustum {
    let camera = camera();
    Frustum::new(camera.proj * camera.view, SIZE, pad)
}

/// The world point `depth` meters ahead that lands at normalized device coordinates `ndc`:
/// reverse-Z with an infinite far plane puts it at depth `NEAR / depth`.
fn at(ndc: [f32; 2], depth: f32) -> Vec3 {
    let camera = camera();
    (camera.proj * camera.view).inverse().project_point3(Vec3::new(ndc[0], ndc[1], NEAR / depth))
}

#[test]
fn spheres_inside_straddling_and_outside() {
    let frustum = frustum(0.0);
    let sphere = |center, radius| Sphere { center, radius };
    assert!(!frustum.excludes(&sphere(Vec3::new(0.0, 0.0, -10.0), 1.0)), "ahead");
    assert!(!frustum.excludes(&sphere(at([1.0, 0.0], 10.0), 0.01)), "on the right edge");
    assert!(!frustum.excludes(&sphere(at([1.2, 0.0], 10.0), 5.0)), "past it, but reaching in");
    assert!(frustum.excludes(&sphere(at([1.2, 0.0], 10.0), 0.5)), "past it");
    assert!(frustum.excludes(&sphere(at([-1.2, 0.3], 10.0), 0.5)), "past the left edge");
    assert!(frustum.excludes(&sphere(at([0.2, 1.3], 10.0), 0.5)), "above");
    assert!(frustum.excludes(&sphere(at([0.2, -1.3], 10.0), 0.5)), "below");
    assert!(frustum.excludes(&sphere(Vec3::new(0.0, 0.0, 10.0), 1.0)), "behind");
    assert!(!frustum.excludes(&sphere(Vec3::new(0.0, 0.0, 0.5), 1.0)), "around the camera");
    // The near plane, 5 cm ahead.
    assert!(frustum.excludes(&sphere(Vec3::new(0.0, 0.0, -0.02), 0.02)), "short of the near plane");
    assert!(!frustum.excludes(&sphere(Vec3::new(0.0, 0.0, -0.02), 0.04)), "reaching past it");
    // Beside the view, without the near plane (a hull's test): the sides still leave out what is
    // behind, and keep what is short of the near plane.
    assert!(!frustum.excludes_beside(&sphere(Vec3::new(0.0, 0.0, -0.02), 0.02)), "short of the near plane");
    assert!(frustum.excludes_beside(&sphere(Vec3::new(0.0, 0.0, 10.0), 1.0)), "behind");
    assert!(frustum.excludes_beside(&sphere(at([1.2, 0.0], 10.0), 0.5)), "past the right edge");
    // Far away, as the mountains are.
    assert!(!frustum.excludes(&sphere(Vec3::new(0.0, 100.0, -3000.0), 200.0)));
    assert!(frustum.excludes(&sphere(Vec3::new(3000.0, 0.0, 0.0), 200.0)), "beside, far");
    // Not finite: kept.
    for center in [Vec3::NAN, Vec3::new(f32::INFINITY, 0.0, -1.0)] {
        assert!(!frustum.excludes(&sphere(center, 1.0)), "{center}");
    }
    assert!(!frustum.excludes(&sphere(Vec3::new(0.0, 0.0, 10.0), f32::NAN)));
}

#[test]
fn padding_keeps_spheres_within_its_pixels_of_the_edge() {
    let padded = frustum(PAD_PIXELS);
    let unpadded = frustum(0.0);
    // A pixel is 2 / 640 of x, 2 / 360 of y. Tiny spheres a pixel and three pixels past each
    // side.
    let tiny = |ndc: [f32; 2]| Sphere { center: at(ndc, 20.0), radius: 1e-4 };
    let (px, py) = (2.0 / 640.0, 2.0 / 360.0);
    for (x, y) in [(1.0, 0.0), (-1.0, 0.3), (0.5, 1.0), (-0.5, -1.0)] {
        let step = [x * px, y * py];
        let one = tiny([x + step[0], y + step[1]]);
        let three = tiny([x + 3.0 * step[0], y + 3.0 * step[1]]);
        assert!(!padded.excludes(&one) && unpadded.excludes(&one), "a pixel past ({x}, {y})");
        assert!(padded.excludes(&three), "three pixels past ({x}, {y})");
    }
}

#[test]
fn excludes_matches_a_projection_of_the_center_for_points() {
    // Seeded points: excluded exactly when their projection lands outside the padded image
    // or behind the near plane, up to the slack.
    let camera = camera();
    let clip_from_world = camera.proj * camera.view;
    let frustum = frustum(PAD_PIXELS);
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = |low: f32, high: f32| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        low + (high - low) * ((state >> 40) as f32 / (1u64 << 24) as f32)
    };
    let mut culled = 0;
    for _ in 0..20_000 {
        let center = Vec3::new(next(-40.0, 40.0), next(-25.0, 25.0), next(-60.0, 5.0));
        let excluded = frustum.excludes(&Sphere { center, radius: 0.0 });
        let shown = on_screen(clip_from_world, SIZE, PAD_PIXELS, center) && -center.z >= NEAR;
        // Within a hair of an edge either answer is right; elsewhere they agree.
        let near_edge = on_screen(clip_from_world, SIZE, PAD_PIXELS + 0.01, center)
            != on_screen(clip_from_world, SIZE, PAD_PIXELS - 0.01, center);
        if !near_edge {
            assert_eq!(excluded, !shown, "{center}");
        }
        culled += u32::from(excluded);
    }
    assert!(culled > 5_000 && culled < 19_000, "{culled}");
}

/// A mesh record of one draw, bounded by a unit sphere, with foliage data where `foliage`.
fn record(foliage: bool, deformed: bool) -> MeshRecord {
    let streams = if deformed {
        MeshStreams::Deformable(DeformSource::default())
    } else {
        MeshStreams::Slab { format: PositionFormat::Unorm16, first_vertex: 0 }
    };
    MeshRecord {
        streams,
        dequantize: Dequantize { position_scale: [1.0; 3], ..Dequantize::default() },
        lods: vec![vec![SubmeshDraw {
            first_index: 0,
            index_count: 3,
            material: SubmeshMaterial { handle: MaterialHandle::from_raw(0), slot: 0 },
        }]],
        lod_vertices: vec![3],
        placement: Placement::default(),
        slots: Vec::new(),
        bounds: Sphere { center: Vec3::ZERO, radius: 1.0 },
        palette_reach: Vec::new(),
        morph_reach: Vec::new(),
        foliage: foliage.then_some(0),
    }
}

/// Meshes 0 to 4: environment, foliage, deformed environment, an anime character's and a fog
/// card's, by their materials' handles.
fn meshes() -> [MeshRecord; 5] {
    let mut meshes = [
        record(false, false),
        record(true, false),
        record(false, true),
        record(false, false),
        record(false, false),
    ];
    meshes[1].lods[0][0].material.handle = MaterialHandle::from_raw(1);
    meshes[3].lods[0][0].material.handle = MaterialHandle::from_raw(2);
    meshes[4].lods[0][0].material.handle = MaterialHandle::from_raw(3);
    meshes
}

fn kind(handle: MaterialHandle) -> (MaterialFamily, MaterialFlags) {
    match handle.index() {
        0 => (MaterialFamily::Environment, MaterialFlags::empty()),
        1 => (MaterialFamily::Foliage, MaterialFlags::empty()),
        2 => (MaterialFamily::AnimeCharacter, MaterialFlags::OUTLINE),
        _ => (MaterialFamily::FogCard, MaterialFlags::empty()),
    }
}

fn place(id: u64, mesh: u32, transform: Mat4) -> MeshInstance {
    MeshInstance { id: InstanceId(id), mesh: MeshHandle::from_raw(mesh), transform }
}

/// A frame of `instances`: their slots and draw list.
fn scene(meshes: &[MeshRecord], instances: &[MeshInstance]) -> (InstanceSlots, DrawList) {
    let mut slots = InstanceSlots::default();
    slots.apply(&InstanceDeltas { changed: instances.to_vec(), removed: Vec::new() });
    let mut list = DrawList::default();
    list.build(meshes, kind, |_| 0, &slots, DrawLimits::NONE);
    (slots, list)
}

/// One frame's culling of `slots`' instances from the main view of `camera`, at `SIZE`, with
/// `reach` per slot and the wind's `margin`.
#[expect(clippy::too_many_arguments, reason = "a frame's inputs")]
fn cull(
    culling: &mut ViewCull,
    (mode, thresholds): (CullingMode, CullingThresholds),
    camera: &CameraData,
    meshes: &[MeshRecord],
    (slots, list): (&InstanceSlots, &DrawList),
    reach: &[CharacterReach],
    margin: f32,
    fresh: &[u32],
) {
    let characters = CharacterList::default();
    culling.prepare(
        mode,
        thresholds,
        Some(camera),
        list,
        slots,
        meshes,
        characters.table(),
        fresh.iter().copied(),
    );
    let widest = reach.iter().map(|reach| reach.pixels).fold(0.0, f32::max);
    let mut views = [None; CULL_VIEWS];
    views[0] = Some(CullView::new(camera, SIZE, SIZE.1, PAD_PIXELS, widest));
    let inputs =
        CullInputs { list, slots, meshes, margin: |_: &MeshRecord| margin, reach, stands: &[], views };
    culling.record(&inputs);
}

fn verdicts(culling: &ViewCull, count: u32) -> Vec<Verdict> {
    let culled = culling.casters();
    (0..count).map(|slot| culled.verdict(slot)).collect()
}

#[test]
fn every_kind_of_instance_is_tested_by_its_bounds() {
    let meshes = meshes();
    let (ahead, behind) =
        (Mat4::from_translation(Vec3::NEG_Z * 10.0), Mat4::from_translation(Vec3::Z * 10.0));
    // 2.6 m right of the right edge, 10 m ahead: a unit sphere 0.8 m past the padded plane.
    let right = Mat4::from_translation(at([1.0, 0.0], 10.0) + Vec3::X * 2.6);
    let projective =
        Mat4::from_cols(Vec4::X, Vec4::Y, Vec4::new(0.0, 0.0, 1.0, 0.5), Vec4::new(0.0, 0.0, 10.0, 1.0));
    let instances = [
        place(0, 0, ahead),
        place(1, 0, behind),
        place(2, 1, behind),
        place(3, 1, right),
        place(4, 2, behind),
        place(5, 3, behind),
        place(6, 0, projective),
        // Three times the size, where the foliage is: it reaches into the view.
        place(7, 0, right * Mat4::from(Affine3A::from_scale(Vec3::splat(3.0)))),
        // The character and a fog card beside the view, past the padding.
        place(8, 3, right),
        place(9, 4, right),
    ];
    let (slots, list) = scene(&meshes, &instances);
    let candidates: Vec<u32> = list.cullable.iter().map(|candidate| candidate.slot).collect();
    assert_eq!(candidates, (0..10).collect::<Vec<_>>(), "every instance with draws");
    let mut culling = ViewCull::default();
    let on = (CullingMode::On, CullingThresholds::NONE);
    use Verdict::{Drawn, Frustum};
    // Calm, the foliage beside the view is culled; with wind that carries it 2 m, it isn't. The
    // deformed instance and the character behind cull by their (cooked) bounds now; the
    // projective transform isn't tested.
    let calm = [Drawn, Frustum, Frustum, Frustum, Frustum, Frustum, Drawn, Drawn, Frustum, Frustum];
    cull(&mut culling, on, &camera(), &meshes, (&slots, &list), &[], 0.0, &[]);
    assert_eq!(verdicts(&culling, 10), calm);
    assert_eq!((culling.stats().tested, culling.stats().culled), (9, 7));
    cull(&mut culling, on, &camera(), &meshes, (&slots, &list), &[], 2.0, &[]);
    assert_eq!(verdicts(&culling, 10)[3], Drawn, "the wind carries the foliage into view");
    // A hull that reaches a meter, or 300 pixels at 1080 rows (100 here), brings the character
    // back.
    let mut reach = vec![CharacterReach::default(); 10];
    reach[8] = CharacterReach { hulled: true, meters: 1.0, ..CharacterReach::default() };
    cull(&mut culling, on, &camera(), &meshes, (&slots, &list), &reach, 0.0, &[]);
    assert_eq!(verdicts(&culling, 10)[8], Drawn, "a hull's meters");
    reach[8] = CharacterReach { hulled: true, pixels: 300.0, ..CharacterReach::default() };
    cull(&mut culling, on, &camera(), &meshes, (&slots, &list), &reach, 0.0, &[]);
    assert_eq!(verdicts(&culling, 10)[8], Drawn, "a hull's pixels");
    reach[8] = CharacterReach { drift: 1.0, ..CharacterReach::default() };
    cull(&mut culling, on, &camera(), &meshes, (&slots, &list), &reach, 0.0, &[]);
    assert_eq!(verdicts(&culling, 10)[8], Drawn, "an eye's drift");
    // Off: nothing culled, nothing counted.
    cull(
        &mut culling,
        (CullingMode::Off, CullingThresholds::NONE),
        &camera(),
        &meshes,
        (&slots, &list),
        &[],
        0.0,
        &[],
    );
    assert!(verdicts(&culling, 10).iter().all(|&verdict| verdict == Drawn));
    assert!(culling.culled().is_none());
    assert_eq!(culling.stats(), ViewCullingStats { drawn: 10, ..ViewCullingStats::default() });
}

#[test]
fn a_facing_fog_card_is_held_by_its_turn_about_its_origin() {
    // A card mesh whose bounds sit 3 m along x from its origin: facing the camera, its quad turns
    // about the origin, so it can reach 4 m from the origin whichever way.
    let mut meshes = meshes();
    meshes[4].bounds = Sphere { center: Vec3::X * 3.0, radius: 1.0 };
    // The origin 2 m past the right edge, 10 m ahead, its cooked bounds 3 m further right.
    let origin = at([1.0, 0.0], 10.0) + Vec3::X * 2.0;
    let (slots, list) = scene(&meshes, &[place(0, 4, Mat4::from_translation(origin))]);
    let mut culling = ViewCull::default();
    cull(
        &mut culling,
        (CullingMode::On, CullingThresholds::NONE),
        &camera(),
        &meshes,
        (&slots, &list),
        &[],
        0.0,
        &[],
    );
    assert_eq!(verdicts(&culling, 1), [Verdict::Drawn], "the turned quad can reach back into view");
}

/// The number of times slot 0's verdict changes as `camera(frame)` moves over 120 frames.
fn changes_over_120_frames(
    thresholds: CullingThresholds,
    camera: impl Fn(u32) -> CameraData,
) -> (u32, Verdict) {
    let meshes = meshes();
    let (slots, list) = scene(&meshes, &[place(0, 0, Mat4::from_translation(Vec3::NEG_Z * 100.0))]);
    let mut culling = ViewCull::default();
    let mut last = None;
    let mut changes = 0;
    for frame in 0..120 {
        let fresh: &[u32] = if frame == 0 { &[0] } else { &[] };
        cull(
            &mut culling,
            (CullingMode::On, thresholds),
            &camera(frame),
            &meshes,
            (&slots, &list),
            &[],
            0.0,
            fresh,
        );
        let verdict = verdicts(&culling, 1)[0];
        changes += u32::from(last.is_some_and(|last| last != verdict));
        last = Some(verdict);
    }
    (changes, last.expect("frames"))
}

#[test]
fn a_camera_held_on_a_threshold_never_flickers() {
    // A unit sphere 100 m ahead: its nearest point 99 m away. The camera sways 5 cm either side of
    // where it crosses each threshold, every frame.
    let sway = |center: f32| {
        move |frame: u32| camera_at(Vec3::Z * (center + if frame.is_multiple_of(2) { 0.05 } else { -0.05 }))
    };
    // Distance: 99 m at the camera's rest.
    let distance = CullingThresholds { distance: 99.0, ..CullingThresholds::NONE };
    let (changes, verdict) = changes_over_120_frames(distance, sway(0.0));
    assert!(changes <= 1, "{changes} changes on the distance");
    assert_eq!(verdict, Verdict::Distance, "first culled, then held");
    // Without the hysteresis it would change every frame: the band is what holds it.
    let far = |gap: f32| distance.distance < gap;
    assert!(far(99.05) && !far(98.95));
    // Size: the sphere shows 2 × 1 × focal × 180 / 99 pixels across at rest.
    let focal = camera().proj.y_axis.y;
    let pixels = 2.0 * focal * SIZE.1 as f32 * 0.5 / 99.0;
    let size = CullingThresholds { least_pixels: pixels, ..CullingThresholds::NONE };
    let (changes, verdict) = changes_over_120_frames(size, sway(0.0));
    assert!(changes <= 1, "{changes} changes on the size");
    assert_eq!(verdict, Verdict::Size);
    // Walking toward it from 10 m further back, it comes back past the return band, and stays.
    let toward = |frame: u32| camera_at(Vec3::Z * (10.0 - frame as f32 * 0.5));
    let (changes, verdict) = changes_over_120_frames(size, toward);
    assert_eq!((changes, verdict), (1, Verdict::Drawn), "back once, as it nears");
}

#[test]
fn named_characters_and_what_they_hold_are_never_culled_by_distance_or_size() {
    // Without a character table every instance may cull; with the instance overlapping a named
    // character's bounds it's kept: the overlap test.
    let named = [Sphere { center: Vec3::NEG_Z * 100.0, radius: 1.0 }];
    assert!(overlaps(&named, &Sphere { center: Vec3::new(0.5, 0.0, -100.0), radius: 0.2 }));
    assert!(!overlaps(&named, &Sphere { center: Vec3::new(3.0, 0.0, -100.0), radius: 0.2 }));
}

#[test]
fn who_drew_each_instance_is_published_for_the_next_frame() {
    // Slots 0 to 3: drawn in the main view; culled from it but in a cascade; culled and in no
    // shadow; standing in inset 0, which draws, and in a shadow.
    let mut culling = ViewCull {
        slots: SlotCull::of(&[Verdict::Drawn, Verdict::Frustum, Verdict::Size, Verdict::Drawn]),
        ..ViewCull::default()
    };
    let stands = [0, 0, 0, 1];
    let mut views = [false; CULL_VIEWS];
    views[0] = true;
    views[1] = true;
    culling.publish(&stands, &views, |slot| slot == 1 || slot == 3);
    let drawn: Vec<DrawnBy> = (0..5).map(|slot| culling.drawn_by(slot)).collect();
    assert_eq!(drawn[0], DrawnBy { view: true, ..DrawnBy::default() });
    assert!(drawn[1].only_shadow(), "{:?}", drawn[1]);
    assert!(!drawn[2].any());
    assert_eq!(drawn[3], DrawnBy { shadow: true, inset: true, ..DrawnBy::default() });
    assert!(!drawn[4].any(), "past the slots");
    assert_eq!((0..4).map(|slot| culling.drawn(slot)).collect::<Vec<_>>(), [true, true, false, true]);
    // An inset whose picture is held draws nothing that stands in it.
    views[1] = false;
    culling.publish(&stands, &views, |_| false);
    assert!(!culling.drawn(3));
}

#[test]
fn the_tiers_thresholds_check_their_ranges() {
    assert_eq!(CullingThresholds::NONE.validate(), Ok(()));
    let bad = CullingThresholds { distance: -1.0, ..CullingThresholds::NONE };
    assert_eq!(bad.validate().map_err(|(field, _)| field), Err("culling.distance"));
    let bad = CullingThresholds { least_texels: f32::NAN, ..CullingThresholds::NONE };
    assert_eq!(bad.validate().map_err(|(field, _)| field), Err("culling.least_texels"));
}

#[test]
fn a_warm_frame_culls_without_allocating() {
    let meshes = meshes();
    let instances: Vec<MeshInstance> = (0..200)
        .map(|id| place(id, (id % 5) as u32, Mat4::from_translation(Vec3::new(id as f32, 0.0, -20.0))))
        .collect();
    let (slots, list) = scene(&meshes, &instances);
    let thresholds = CullingThresholds { distance: 150.0, least_pixels: 2.0, least_texels: 0.0 };
    let reach = vec![CharacterReach::default(); 200];
    let mut culling = ViewCull::default();
    let characters = CharacterList::default();
    let camera = camera();
    culling.prepare(
        CullingMode::On,
        thresholds,
        Some(&camera),
        &list,
        &slots,
        &meshes,
        characters.table(),
        0..200,
    );
    let mut views = [None; CULL_VIEWS];
    views[0] = Some(CullView::new(&camera, SIZE, SIZE.1, PAD_PIXELS, 0.0));
    let inputs = CullInputs {
        list: &list,
        slots: &slots,
        meshes: &meshes,
        margin: |_: &MeshRecord| 0.0,
        reach: &reach,
        stands: &[],
        views,
    };
    culling.record(&inputs);
    let ((), allocated) = sapphire_core::alloc::count_in_scope(|| {
        culling.prepare(
            CullingMode::On,
            thresholds,
            Some(&camera),
            &list,
            &slots,
            &meshes,
            characters.table(),
            0..0,
        );
        culling.record(&inputs);
        culling.publish(&[], &[true; CULL_VIEWS], |_| false);
    });
    assert_eq!(allocated.allocations, 0, "a warm frame's culling allocated: {allocated:?}");
}
