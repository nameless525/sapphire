use glam::{Vec2, Vec3};
use sapphire_math::sh;

use super::*;
use crate::probes::bake::{BakeKey, BakeMaterial, BakeScene, BakeTriangle};

/// The faces of a cube, by their index.
const POS_X: usize = 0;
const NEG_X: usize = 1;
const POS_Y: usize = 2;
const NEG_Y: usize = 3;

/// Two triangles of the rectangle from `corner` along `u` and `v`, facing `u × v`.
fn quad(corner: Vec3, u: Vec3, v: Vec3, material: u32) -> [BakeTriangle; 2] {
    let normal = u.cross(v).normalize();
    let (a, b, c, d) = (corner, corner + u, corner + v, corner + u + v);
    let triangle =
        |positions| BakeTriangle { positions, normals: [normal; 3], uvs: [Vec2::ZERO; 3], material };
    [triangle([a, b, d]), triangle([a, d, c])]
}

/// Which sides of a room are built.
#[derive(Clone, Copy)]
struct Sides {
    roof: bool,
    pos_x: bool,
}

/// A room from (−2, 0, −2) to (2, 3, 2) facing in, of material 0: its floor and its walls, its roof and
/// its +x wall as `sides` says.
fn room(sides: Sides) -> Vec<BakeTriangle> {
    let (min, max) = (Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 3.0, 2.0));
    let d = max - min;
    let (dx, dy, dz) = (Vec3::X * d.x, Vec3::Y * d.y, Vec3::Z * d.z);
    let mut triangles = Vec::new();
    triangles.extend(quad(min, dz, dx, 0));
    triangles.extend(quad(min, dy, dz, 0));
    triangles.extend(quad(min, dx, dy, 0));
    triangles.extend(quad(Vec3::new(min.x, min.y, max.z), dy, dx, 0));
    if sides.roof {
        triangles.extend(quad(Vec3::new(min.x, max.y, min.z), dx, dz, 0));
    }
    if sides.pos_x {
        triangles.extend(quad(Vec3::new(max.x, min.y, min.z), dz, dy, 0));
    }
    triangles
}

fn scene(triangles: Vec<BakeTriangle>, material: BakeMaterial) -> BakeScene {
    BakeScene { triangles, materials: vec![material], cutouts: Vec::new() }
}

fn grey() -> BakeMaterial {
    BakeMaterial { albedo: Vec3::splat(0.6), ..BakeMaterial::default() }
}

/// A high sun from +x under a pale sky, or with `sun` false the same sky with the sun gone.
fn key(hours: f32, sun: bool) -> BakeKey {
    let sky = [0.5, 0.6, 0.8].map(|value| {
        let mut channel = [0.0; sh::L2];
        channel[0] = sh::project_constant(value)[0];
        channel
    });
    BakeKey {
        hours,
        to_light: Vec3::new(1.0, 2.0, 0.3).normalize(),
        radiance: if sun { Vec3::splat(3.0) } else { Vec3::ZERO },
        shadow_tint: Vec3::splat(0.2),
        sky,
        emission: 1.0,
    }
}

const SIZE: u32 = 16;
const CENTER: Vec3 = Vec3::new(0.0, 1.5, 0.0);
const REACH: (Vec3, Vec3) = (Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 3.0, 2.0));

/// One probe's cubes, per key, per face, per texel.
fn bake(scene: &BakeScene, keys: &[BakeKey], threads: usize) -> Vec<[f32; 4]> {
    bake_cubes(scene, keys, &[CubeProbe { position: CENTER }], REACH, SIZE, threads).unwrap()
}

/// Face `face` of key `key`'s cube.
fn face(cubes: &[[f32; 4]], key: usize, face: usize) -> &[[f32; 4]] {
    let texels = (SIZE * SIZE) as usize;
    let start = (key * 6 + face) * texels;
    &cubes[start..start + texels]
}

/// The texel at the middle of a face, looking straight along its axis.
fn middle(face: &[[f32; 4]]) -> [f32; 4] {
    face[(SIZE / 2 * SIZE + SIZE / 2) as usize]
}

fn luminance(texels: &[[f32; 4]]) -> f32 {
    texels.iter().map(|[r, g, b, _]| 0.2126 * r + 0.7152 * g + 0.0722 * b).sum::<f32>() / texels.len() as f32
}

#[test]
fn texels_look_where_a_cube_lookup_reads_them() {
    let axes = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];
    for (face, axis) in axes.into_iter().enumerate() {
        assert!(texel_direction(face as u32, 0.0, 0.0).abs_diff_eq(axis, 1e-6), "face {face}");
    }
    // The top of +Y leans toward −Z, and the right of +X toward −Z, as cube maps have it.
    assert!(texel_direction(2, 0.0, -1.0).z < -0.5);
    assert!(texel_direction(0, 1.0, 0.0).z < -0.5);
    // The top of +Z leans up, its right toward +X.
    assert!(texel_direction(4, 0.0, -1.0).y > 0.5);
    assert!(texel_direction(4, 1.0, 0.0).x > 0.5);
}

#[test]
fn a_red_room_s_probe_is_red_and_sees_no_sky() {
    let red = BakeMaterial {
        albedo: Vec3::new(0.8, 0.0, 0.0),
        emission: Vec3::new(0.5, 0.0, 0.0),
        ..BakeMaterial::default()
    };
    let cubes = bake(&scene(room(Sides { roof: true, pos_x: true }), red), &[key(12.0, true)], 2);
    assert_eq!(cubes.len(), 6 * (SIZE * SIZE) as usize);
    for &[r, g, b, a] in &cubes {
        assert_eq!((g, b, a), (0.0, 0.0, 0.0), "nothing but red, and no sky");
        // Its glow, and the glow bounced off the other walls.
        assert!(r > 0.5, "{r}");
    }
}

#[test]
fn an_open_courtyard_sees_the_sky_up_and_not_down() {
    let cubes = bake(&scene(room(Sides { roof: false, pos_x: true }), grey()), &[key(12.0, true)], 2);
    assert_eq!(middle(face(&cubes, 0, POS_Y))[3], 1.0, "straight up is sky");
    assert!(face(&cubes, 0, POS_Y).iter().all(|texel| texel[3] > 0.0), "the whole of up sees some sky");
    assert!(face(&cubes, 0, NEG_Y).iter().all(|texel| texel[3] == 0.0), "down sees none");
    // Where it sees the sky, the color is what's left of the texel: up's middle is all sky and no color.
    assert_eq!(middle(face(&cubes, 0, POS_Y))[..3], [0.0; 3]);
    assert!(luminance(face(&cubes, 0, NEG_Y)) > 0.0, "the floor is lit");
}

#[test]
fn a_probe_under_a_roof_sees_the_sky_only_through_its_open_side() {
    let cubes = bake(&scene(room(Sides { roof: true, pos_x: false }), grey()), &[key(12.0, true)], 2);
    assert_eq!(middle(face(&cubes, 0, POS_X))[3], 1.0, "straight out of the open side is sky");
    for other in [NEG_X, POS_Y, NEG_Y, 4, 5] {
        assert!(face(&cubes, 0, other).iter().all(|texel| texel[3] == 0.0), "face {other} sees no sky");
    }
    // Under the roof the floor is in shade: the sun from +x and above reaches only by the open side.
    assert!(luminance(face(&cubes, 0, POS_Y)) > 0.0, "the roof's underside has the bounce and the sky");
}

#[test]
fn each_key_bakes_a_cube_of_its_own_and_the_sun_gone_is_darker() {
    let courtyard = scene(room(Sides { roof: false, pos_x: true }), grey());
    let cubes = bake(&courtyard, &[key(12.0, true), key(20.0, false)], 2);
    let per_key = 6 * (SIZE * SIZE) as usize;
    let (day, dusk) = cubes.split_at(per_key);
    assert_ne!(day, dusk);
    assert!(luminance(dusk) < luminance(day), "{} against {}", luminance(dusk), luminance(day));
    // What sees the sky doesn't hang on the key.
    assert!(day.iter().zip(dusk).all(|(day, dusk)| day[3] == dusk[3]));
}

#[test]
fn cubes_bake_to_the_same_bits_whatever_the_threads() {
    let room = scene(room(Sides { roof: true, pos_x: false }), grey());
    let keys = [key(9.0, true), key(21.0, false)];
    let probes = [CubeProbe { position: CENTER }, CubeProbe { position: Vec3::new(1.0, 0.5, -1.0) }];
    let bits = |threads| {
        let cubes = bake_cubes(&room, &keys, &probes, REACH, 8, threads).unwrap();
        cubes.into_iter().flat_map(|texel| texel.map(f32::to_bits)).collect::<Vec<u32>>()
    };
    let one = bits(1);
    assert_eq!(one.len(), 2 * 2 * 6 * 8 * 8 * 4);
    assert_eq!(one, bits(1), "twice");
    assert_eq!(one, bits(7), "on seven threads");
}

#[test]
fn bad_cubes_are_refused() {
    let room = scene(room(Sides { roof: true, pos_x: true }), grey());
    let probe = [CubeProbe { position: CENTER }];
    let keys = [key(12.0, true)];
    assert!(bake_cubes(&room, &keys, &probe, REACH, 12, 1).is_err(), "not a power of two");
    assert!(bake_cubes(&room, &keys, &probe, REACH, 4, 1).is_err(), "past the chain's end");
    assert!(bake_cubes(&room, &[], &probe, REACH, 8, 1).is_err(), "no keys");
    assert!(bake_cubes(&room, &keys, &[], REACH, 8, 1).is_err(), "no probes");
    let nowhere = [CubeProbe { position: Vec3::NAN }];
    assert!(bake_cubes(&room, &keys, &nowhere, REACH, 8, 1).is_err(), "a probe nowhere");
}
