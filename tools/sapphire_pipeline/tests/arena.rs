//! The arena's colliders against the town they stand in (M2 O5).
//!
//! `content/arena/square.colliders.ron` is written by hand, and the town is an assembly anyone may
//! edit, so nothing but this test connects them. It assembles each placement of the town near the
//! square through the assembly's own code, so it reads the triangles the cook does, and holds the
//! colliders to them both ways:
//!
//! - **Nothing drawn at a character's height is outside every collider,** so moving or adding a
//!   building, a stall or a post leaves no way to walk through it.
//! - **Every face of every collider has something drawn at it,** so moving a building leaves no
//!   invisible wall where it was.
//!
//! The stage plants its trees, so they aren't in the assembly: their colliders are the stage's to
//! check (`crates/sapphire_stage/src/arena.rs`).

use std::fs;
use std::path::{Path, PathBuf};

use glam::{Quat, Vec3};
use sapphire_colliders::{ColliderDocument, ColliderSet, CookedShape, Group};
use sapphire_pipeline::assembly::{self, Assembly, Detail, Scale};
use sapphire_pipeline::import::gltf;
use sapphire_pipeline::meta::{self, ImportSettings, SourceTree};

/// How far a collider's face may stand from what's drawn there, or what's drawn from a collider: a
/// little under half a character's radius, past which an invisible wall starts to show.
const TOLERANCE: f32 = 0.12;

/// What a character's capsule reaches from just over the ground: anything drawn here stops a body.
/// Its bottom is over the pavilion's 4 cm stone base, which a body walks onto.
const CHARACTER_BAND: [f32; 2] = [0.1, 1.6];

/// Placements whose origin is outside this, in x and z, aren't near the square.
const NEAR: [[f32; 2]; 2] = [[-26.0, 26.0], [-26.0, 24.0]];

fn content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

fn town_dir() -> PathBuf {
    content().join("lookdev/town")
}

fn town() -> Assembly {
    let bytes = fs::read(town_dir().join("town.assembly.ron")).expect("the town's document");
    assembly::read(&bytes).expect("it reads")
}

fn colliders() -> ColliderSet {
    let text = fs::read_to_string(content().join("arena/square.colliders.ron")).expect("the document");
    ColliderDocument::from_ron(&text).expect("it reads").cook().expect("it cooks")
}

type Triangle = [Vec3; 3];

/// One placement of the town, as the cook would build it alone.
struct Placed {
    /// What messages call it.
    label: String,
    triangles: Vec<Triangle>,
}

/// Every placement near the square, but the paving and lawn, which nothing stands in.
fn placements() -> Vec<Placed> {
    let town = town();
    let dir = town_dir();
    let near = |at: [f32; 3]| {
        (NEAR[0][0]..=NEAR[0][1]).contains(&at[0]) && (NEAR[1][0]..=NEAR[1][1]).contains(&at[2])
    };
    town.place
        .iter()
        .enumerate()
        .filter(|(_, placement)| near(placement.at) && !matches!(placement.name(), "#quad" | "#ground"))
        .map(|(index, placement)| {
            let mut alone = town.clone();
            alone.place = vec![placement.clone()];
            alone.allow_coplanar.clear();
            let mesh = assembly::assemble(&alone, Detail::Full, &mut |file: &str| {
                let path = dir.join(file);
                let bytes = fs::read(&path).expect("the piece");
                Ok(gltf::import_slice(&bytes, path.parent()).expect("it imports"))
            })
            .expect("it assembles");
            let (triangles, _) = mesh.indices.as_chunks::<3>();
            let triangles = triangles
                .iter()
                .map(|corners| corners.map(|corner| Vec3::from(mesh.vertices[corner as usize].position)))
                .collect();
            let label = format!("place[{index}] {} at {:?}", placement.name(), placement.at);
            Placed { label, triangles }
        })
        .collect()
}

/// A collider as a box in its own space: a capsule's box, for this test's purposes.
struct Volume {
    name: String,
    center: Vec3,
    rotation: Quat,
    half: Vec3,
    /// In the world group, rather than one of the blockers nothing draws.
    world: bool,
}

impl Volume {
    fn local(&self, point: Vec3) -> Vec3 {
        self.rotation.inverse() * (point - self.center)
    }

    /// How far `point` is outside the box, along its worst axis: 0 or less is inside.
    fn outside(&self, point: Vec3) -> f32 {
        (self.local(point).abs() - self.half).max_element()
    }
}

fn volumes(set: &ColliderSet) -> Vec<Volume> {
    set.colliders
        .iter()
        .map(|collider| {
            let half = match collider.shape {
                CookedShape::Box { half_extents } => Vec3::from(half_extents),
                CookedShape::Capsule { half_height, radius } => {
                    Vec3::new(radius, half_height + radius, radius)
                }
                CookedShape::Sphere { radius } => Vec3::splat(radius),
                CookedShape::Mesh { .. } => panic!("{}: a mesh has no box here", collider.name),
            };
            Volume {
                name: collider.name.clone(),
                center: Vec3::from(collider.position),
                rotation: Quat::from_array(collider.rotation),
                half,
                world: collider.memberships & Group::World.mask().bits() != 0,
            }
        })
        .collect()
}

/// Sutherland–Hodgman against one plane: what of `polygon` has `distance(point) >= 0`.
fn clip_plane(polygon: &[Vec3], distance: impl Fn(Vec3) -> f32) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(polygon.len() + 1);
    for k in 0..polygon.len() {
        let (a, b) = (polygon[k], polygon[(k + 1) % polygon.len()]);
        let (da, db) = (distance(a), distance(b));
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

/// What of a triangle lies in the band of heights.
fn in_band(triangle: Triangle, band: [f32; 2]) -> Vec<Vec3> {
    let above = clip_plane(&triangle, |p| p.y - band[0]);
    clip_plane(&above, |p| band[1] - p.y)
}

/// **Nothing to walk through:** every point drawn at a character's height, of any placement near the
/// square, lies in some collider, within the tolerance. A moon gate's foot lies in its blocker.
#[test]
fn what_the_town_draws_at_a_characters_height_is_in_a_collider() {
    let set = colliders();
    let volumes: Vec<Volume> = volumes(&set).into_iter().filter(|volume| volume.name != "ground").collect();
    let placements = placements();
    assert!(placements.len() >= 30, "the square's placements: {}", placements.len());
    let mut failures = Vec::new();
    for placed in &placements {
        let mut worst: Option<(f32, Vec3)> = None;
        let mut points = 0_u32;
        for triangle in &placed.triangles {
            for point in in_band(*triangle, CHARACTER_BAND) {
                points += 1;
                let outside = volumes.iter().map(|v| v.outside(point)).fold(f32::INFINITY, f32::min);
                if outside > TOLERANCE && worst.is_none_or(|(most, _)| outside > most) {
                    worst = Some((outside, point));
                }
            }
        }
        assert!(points > 0, "{} draws nothing at a character's height", placed.label);
        if let Some((outside, point)) = worst {
            failures.push(format!("{}: {point} is drawn {outside:.3} m from every collider", placed.label));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// **No invisible walls:** every face of every collider in the world group has something drawn
/// within the tolerance of it, inside the box or just past it. The ground is the paving's, which
/// the next test holds, and the trees are the stage's.
#[test]
fn every_face_of_every_collider_has_something_drawn_at_it() {
    let set = colliders();
    let placements = placements();
    let mut failures = Vec::new();
    let held =
        |volume: &&Volume| volume.world && volume.name != "ground" && !volume.name.starts_with("tree_");
    let volumes = volumes(&set);
    assert!(volumes.iter().filter(held).count() >= 30);
    for volume in volumes.iter().filter(held) {
        let reach = volume.half + Vec3::splat(TOLERANCE);
        let (mut least, mut most) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for triangle in placements.iter().flat_map(|placed| &placed.triangles) {
            let mut polygon: Vec<Vec3> = triangle.iter().map(|&point| volume.local(point)).collect();
            for axis in 0..3 {
                polygon = clip_plane(&polygon, |p| p[axis] + reach[axis]);
                polygon = clip_plane(&polygon, |p| reach[axis] - p[axis]);
            }
            for point in polygon {
                least = least.min(point);
                most = most.max(point);
            }
        }
        for (axis, letter) in ["x", "y", "z"].iter().enumerate() {
            // The ground under a box isn't in these placements: the bottom of what stands on it is.
            let gap_low = least[axis] + volume.half[axis];
            let gap_high = volume.half[axis] - most[axis];
            if gap_low > TOLERANCE {
                failures.push(format!(
                    "{}: its -{letter} face stands {gap_low:.3} m from anything drawn",
                    volume.name
                ));
            }
            if gap_high > TOLERANCE {
                failures.push(format!(
                    "{}: its +{letter} face stands {gap_high:.3} m from anything drawn",
                    volume.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The ground collider's top is the paving's level, and it reaches past the paving.
#[test]
fn the_ground_is_the_pavings_level() {
    let town = town();
    let paving = town.place.first().expect("the paving comes first");
    assert_eq!(paving.name(), "#quad");
    assert_eq!(paving.at[1], 0.0, "the paving's level");
    let Scale::Axes(scale) = paving.scale else { panic!("the paving is scaled by axis") };
    let set = colliders();
    let volumes = volumes(&set);
    let ground = volumes.iter().find(|volume| volume.name == "ground").expect("a collider");
    assert_eq!(ground.center.y + ground.half.y, paving.at[1]);
    for axis in [0, 2] {
        let edge = paving.at[axis].abs() + scale[axis] * 0.5;
        assert!(ground.half[axis] > edge, "axis {axis}: {} against {edge}", ground.half[axis]);
    }
}

/// A locked cook, as CI runs, needs the document's sidecar: this is what `--locked` reads of it. The
/// spawn points beside it are compiled in, so no importer reads them and they need none.
#[test]
fn a_locked_cook_finds_the_documents_sidecar() {
    let tree = SourceTree::scan(&content().join("arena")).expect("the directory");
    let resolved = meta::resolve(&tree, true).expect("every source has its sidecar");
    let names: Vec<&str> = resolved
        .assets
        .iter()
        .filter_map(|asset| asset.path.file_name().and_then(|name| name.to_str()))
        .collect();
    assert_eq!(names, ["square.colliders.ron"], "the spawn points aren't a source");
    assert!(matches!(resolved.assets[0].meta.importer, ImportSettings::Colliders(_)));
    assert!(resolved.created.is_empty());
    let bytes = fs::read(&resolved.assets[0].path).expect("the document");
    let cooked = sapphire_pipeline::colliders::cook(&bytes).expect("it cooks");
    assert!(!cooked.is_empty());
}
