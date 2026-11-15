use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3};
use sapphire_assets::material::{FogCardMaterial, MaterialFamily, SkyMaterial, TextureUsage};

use super::{
    CoverageViews, Footprint, TexelDensity, View, aligned_levels, box_distance, reads, texture_streams,
    triangle_stretch,
};
use crate::MaterialHandle;
use crate::bounds::Sphere;
use crate::draw::Walk;
use crate::inset::MAX_INSETS;
use crate::mesh_pool::{SubmeshDraw, SubmeshMaterial};
use crate::pipelines::SurfaceVariant;
use crate::shadows::MAX_CHARACTER_MAPS;

#[test]
fn a_triangle_stretches_its_uvs_by_its_larger_singular_value() {
    let square = [Vec3::ZERO, Vec3::X, Vec3::Y];
    // One UV unit a meter, both ways.
    let unit = triangle_stretch(square, [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]).expect("area");
    assert!((unit - 1.0).abs() < 1e-6, "{unit}");
    // Four units a meter along x, one along y: the larger.
    let wide = triangle_stretch(square, [[0.0, 0.0], [4.0, 0.0], [0.0, 1.0]]).expect("area");
    assert!((wide - 4.0).abs() < 1e-5, "{wide}");
    // Turned and scaled in space: half a unit a meter.
    let big = [Vec3::ZERO, Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 2.0, 0.0)];
    let half = triangle_stretch(big, [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]).expect("area");
    assert!((half - 0.5).abs() < 1e-6, "{half}");
    // UVs collapsed to a line still stretch; to a point, not at all.
    let line = triangle_stretch(square, [[0.0, 0.0], [1.0, 0.0], [0.0, 0.0]]).expect("area");
    assert!((line - 1.0).abs() < 1e-6, "{line}");
    assert_eq!(triangle_stretch(square, [[0.5, 0.5]; 3]), Some(0.0));
    // No area, nothing drawn.
    assert_eq!(
        triangle_stretch([Vec3::ZERO, Vec3::X, Vec3::X * 2.0], [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
        None
    );
}

#[test]
fn only_slots_sampled_through_the_meshs_uvs_stream() {
    // Slots a shader takes an edge, a threshold or an ID from, or samples at a level of its own, never
    // stream; every family's slot is answered for.
    let mut streamed = Vec::new();
    for family in MaterialFamily::ALL {
        for (slot, desc) in family.texture_slots().iter().enumerate() {
            if texture_streams(family, slot) {
                streamed.push(format!("{} {}", family.name(), desc.name));
            }
            if matches!(desc.usage, TextureUsage::Ramp | TextureUsage::FaceShadow | TextureUsage::Control) {
                assert!(!texture_streams(family, slot), "{} {}", family.name(), desc.name);
            }
        }
    }
    assert_eq!(
        streamed,
        [
            "anime character base_color",
            "anime character normal",
            "anime character expression",
            "environment base_color",
            "environment normal",
            "environment mask",
            "environment emission",
            "foliage base_color",
            "foliage mask",
        ]
    );
}

/// The sky samples its maps at levels of its own, and a fog card its map: neither streams, so the stage keeps
/// them whole while a material binds them (`sapphire_stage`'s `TextureLevels::bind`).
#[test]
fn the_skys_and_the_fog_cards_slots_never_stream() {
    assert!((0..SkyMaterial::TEXTURE_SLOTS.len()).all(|slot| !texture_streams(MaterialFamily::Sky, slot)));
    assert!(
        (0..MaterialFamily::Sky.texture_slots().len())
            .all(|slot| !texture_streams(MaterialFamily::Sky, slot))
    );
    assert!(!texture_streams(MaterialFamily::FogCard, FogCardMaterial::MAP));
}

/// An inset reads every texture its draws bind whole, so a close-up opens sharp: its list is read as
/// [`View::Whole`], whose bound is 0, and an inset that didn't draw isn't read.
#[test]
fn an_insets_draws_want_their_textures_whole() {
    let views = CoverageViews {
        footprint: Footprint::new(Mat4::IDENTITY, Mat4::IDENTITY, Vec3::ZERO, (64, 64)),
        insets: std::array::from_fn(|index| index == 1),
        cascades: std::array::from_fn(|index| (index == 0).then_some(0.05)),
        held: 0,
        map_texels: [0.01; MAX_CHARACTER_MAPS],
        maps: 1,
    };
    let read: Vec<(Walk, View)> = reads(&views).collect();
    assert_eq!(
        read,
        [
            (Walk::Main, View::Main),
            (Walk::InsetMain(1), View::Whole),
            (Walk::Cascade(0), View::Shadow(0.05)),
            (Walk::CharacterMap(0), View::Shadow(0.01)),
        ]
    );
    // Whole reads every surface it draws, at level 0.
    assert!(SurfaceVariant::ALL.into_iter().all(|variant| View::Whole.reads(variant)));
    assert_eq!(View::Whole.bound_of(|| unreachable!("a whole view needs no footprint")), 0.0);
}

/// A held cascade's list is of the frame that drew it (M5 D8), which a rebuilt draw list no longer
/// matches: it isn't walked, and what its map holds is taken from the shadow plan's kept casters
/// instead ([`CoverageEstimate::walk_held`], the GPU test `a_held_cascade_keeps_its_casters_textures_wanted`).
#[test]
fn a_held_cascades_list_isnt_walked() {
    let views = CoverageViews {
        footprint: Footprint::new(Mat4::IDENTITY, Mat4::IDENTITY, Vec3::ZERO, (64, 64)),
        insets: [false; MAX_INSETS],
        cascades: std::array::from_fn(|index| (index < 2).then_some(0.05 * (index + 1) as f32)),
        held: 0b10,
        map_texels: [0.01; MAX_CHARACTER_MAPS],
        maps: 0,
    };
    let read: Vec<(Walk, View)> = reads(&views).collect();
    assert_eq!(read, [(Walk::Main, View::Main), (Walk::Cascade(0), View::Shadow(0.05))]);
}

/// A coarser level of a mesh asks for the same texture levels as LOD0 (m5-plan D4): each slot takes its least
/// density over every level, in LOD0's order, whichever level a view's list entry draws and whether a level
/// simplified a slot away.
#[test]
fn a_coarser_level_asks_for_the_same_texture_levels() {
    // Two slots; LOD1 is denser in slot 1 (less stretch) and drops slot 0, cooked in its own order.
    let densities = vec![vec![(0, 4.0), (1, 2.0)], vec![(1, 1.5)], vec![(1, 3.0), (0, 8.0)]];
    let least = [4.0, 1.5];
    let levels = aligned_levels(&densities, &least);
    assert_eq!(levels, vec![vec![4.0, 1.5]; 3]);
    // The pool's aligned submeshes: every level in LOD0's order, a dropped slot empty at LOD0's start.
    let material = |slot| SubmeshMaterial { handle: MaterialHandle::from_raw(slot), slot };
    let draw =
        |first_index, index_count, slot| SubmeshDraw { first_index, index_count, material: material(slot) };
    let lods = vec![
        vec![draw(0, 30, 0), draw(30, 60, 1)],
        vec![draw(0, 0, 0), draw(90, 12, 1)],
        vec![draw(110, 9, 0), draw(102, 8, 1)],
    ];
    let density = TexelDensity { levels, local_box: None };
    for (lod, submeshes) in lods.iter().enumerate() {
        for (submesh, at) in submeshes.iter().enumerate() {
            assert_eq!(density.of_draw(&lods, at.first_index), least[submesh], "LOD{lod} submesh {submesh}");
        }
    }
    // A draw that isn't the mesh's wants whole.
    assert_eq!(density.of_draw(&lods, 7), 0.0);
    // No slot without area or triangles drives a level: whole.
    assert_eq!(aligned_levels(&[vec![(0, 1.0), (2, 1.0)]], &[f32::INFINITY, 1.0]), vec![vec![0.0, 0.0]]);
}

#[test]
fn a_pixel_spans_no_less_than_the_footprint_says() {
    let size = (1920, 1080);
    let proj = directx::perspective_infinite_reverse(60f32.to_radians(), 16.0 / 9.0, 0.1);
    let view = view::look_at_mat4(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
    let footprint = Footprint::new(view, proj, Vec3::ZERO, size);
    let focal = 540.0 / (30f32.to_radians()).tan();
    // Straight ahead, 10 m off, a point: 10 m over the focal length.
    let ahead = footprint.meters_per_pixel(Sphere { center: Vec3::new(0.0, 0.0, -10.0), radius: 0.0 }, 0.0);
    assert!((ahead - 10.0 / focal).abs() < 1e-6 * ahead.max(1.0), "{ahead}");
    // A sphere's nearest point, a little less.
    let near = footprint.meters_per_pixel(Sphere { center: Vec3::new(0.0, 0.0, -10.0), radius: 2.0 }, 0.0);
    assert!(near <= 8.0 / focal + 1e-6, "{near}");
    // Off the axis, less again; around the eye, the near plane's, unless a box says the surface is farther.
    let aside = footprint.meters_per_pixel(Sphere { center: Vec3::new(5.0, 0.0, -10.0), radius: 0.0 }, 0.0);
    assert!(aside < (125f32).sqrt() / focal, "{aside}");
    let around = footprint.meters_per_pixel(Sphere { center: Vec3::ZERO, radius: 1.0 }, 0.0);
    assert!(around <= 0.1 / focal, "{around}");
    let above = footprint.meters_per_pixel(Sphere { center: Vec3::ZERO, radius: 1.0 }, 0.5);
    assert!(above > around && above <= 0.5 / focal, "{above}");
    // Orthographic: the same everywhere.
    let ortho = directx::orthographic(-10.0, 10.0, -10.0, 10.0, 0.1, 100.0);
    let flat = Footprint::new(view, ortho, Vec3::ZERO, (200, 200));
    let texel = flat.meters_per_pixel(Sphere { center: Vec3::new(3.0, 0.0, -50.0), radius: 1.0 }, 0.0);
    assert!((texel - 0.1).abs() < 1e-6, "{texel}");
}

/// A street's box under a camera 1.6 m above it is 1.6 m away, however it's placed, at the least: scaled,
/// turned or moved.
#[test]
fn a_box_is_no_nearer_than_its_placement_says() {
    let street = [Vec3::new(-10.0, 0.0, -60.0), Vec3::new(10.0, 0.0, 0.0)];
    let eye = Vec3::new(0.0, 1.6, -5.0);
    assert!((box_distance(street, Mat4::IDENTITY, eye) - 1.6).abs() < 1e-5);
    // Scaled twice as tall, the eye is 0.8 object units above: 1.6 m, at the least, still.
    let tall = Mat4::from_scale(Vec3::new(1.0, 2.0, 1.0));
    let distance = box_distance(street, tall, eye);
    assert!((0.8 - 1e-5..=1.6 + 1e-5).contains(&distance), "{distance}");
    let turned = Mat4::from_rotation_y(0.7) * Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0));
    let world_eye = turned.transform_point3(eye);
    assert!((box_distance(street, turned, world_eye) - 1.6).abs() < 1e-4);
    // Inside the box, nothing.
    assert_eq!(box_distance(street, Mat4::IDENTITY, Vec3::new(0.0, 0.0, -5.0)), 0.0);
    // Tilted and stretched, no point of the placed box is nearer than it says.
    let tilted = Mat4::from_rotation_x(0.6) * Mat4::from_scale(Vec3::new(1.5, 1.0, 0.5));
    for eye in [Vec3::new(0.0, 4.0, -5.0), Vec3::new(12.0, -3.0, 2.0), Vec3::new(-3.0, 30.0, -40.0)] {
        let bound = box_distance(street, tilted, eye);
        let nearest = (0..=40)
            .flat_map(|i| (0..=40).map(move |k| (i, k)))
            .map(|(i, k)| {
                let local = Vec3::new(-10.0 + i as f32 * 0.5, 0.0, -60.0 + k as f32 * 1.5);
                tilted.transform_point3(local).distance(eye)
            })
            .fold(f32::INFINITY, f32::min);
        assert!(bound <= nearest + 1e-4, "{bound} past {nearest} from {eye}");
    }
}
