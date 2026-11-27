//! A zone's scatter sets (M6 G1a): the plants its foliage document places in each cell, cooked with the zone.
//!
//! A zone that names `foliage` (a [`sapphire_zone::FoliageDocument`]'s sidecar ID) has a `.sscatter` for each
//! cell the document's layers reach ([`sapphire_zone::FoliageDocument::cells`]), a sub-asset of the zone
//! (`grove.scatter.2.-1.sscatter`), some perhaps empty. The `.szone` names the document and each set, outside
//! its digest. A set is [`sapphire_zone::foliage::scatter_set`] on the ground this module gives it.
//!
//! # The ground
//!
//! A plant stands on the highest face of the zone's static colliders (the `World` group, a navmesh's solids
//! less its invisible walls) under its point that faces up: [`ColliderGround`], from the cooked `.scol` the zone
//! names, which it must. Triangles are [`crate::nav::colliders_to_triangles`]'s, placed with scalar maths and
//! `det`'s sines, and a point's height and the test that it's over a triangle are `f64` products and sums of
//! them, with no root, so the ground reads the same on every architecture. A zone's drawn ground with no
//! collider under it grows nothing; a terrain's height field (W12a) will be a ground of its own.
//!
//! # Keys
//!
//! A set's key holds its cell and the zone's grid ([`super::ZonePlan::scatter_recipe`]), the foliage document's
//! bytes and the colliders' key, and nothing else of the zone: a save to a placement, a light or a spawner
//! leaves every set alone, and a save to the foliage document or a moved collider recooks every set. The
//! zone's `.szone` holds the document's bytes too, since which cells have a set is the document's to say.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use sapphire_colliders::{ColliderSet, Group};
use sapphire_zone::FoliageDocument;
use sapphire_zone::foliage::{Ground, GroundPoint, scatter_set};
use sapphire_zone::grid::{CellCoord, Grid};

use super::ZoneCookError;
use crate::cache::Stage;
use crate::nav::{self, Triangle};

/// The `.sscatter` writer. Bump the version whenever it changes output for the same document and ground, as a
/// change to placement in `sapphire_zone::foliage` does.
pub const SCATTER_COOKER: Stage = Stage { name: "sscatter", version: 1 };

/// A bucket's side, in metres: the ground finds a point's triangles among its bucket's.
const BUCKET: f64 = 8.0;

/// The most buckets one triangle may span, past which the colliders are refused rather than filling memory:
/// a 4 km square at 8 m.
const MAX_BUCKETS: i64 = 1 << 18;

/// The zone's static colliders as ground: their faces that look up, by the buckets their footprints meet.
pub struct ColliderGround {
    triangles: Vec<[[f64; 3]; 3]>,
    /// Each bucket's triangles, by index, ascending.
    buckets: BTreeMap<(i64, i64), Vec<u32>>,
}

impl ColliderGround {
    /// The ground of a cooked collider set's `World` solids.
    pub fn from_cooked(colliders: &[u8]) -> Result<Self, ZoneCookError> {
        let set = ColliderSet::from_cooked(colliders)
            .map_err(|error| ZoneCookError::invalid("colliders", format!("they don't read: {error}")))?;
        Self::new(&nav::colliders_to_triangles(&set, Group::World.mask()))
    }

    /// The ground of `triangles`: those that face up, as `colliders_to_triangles` writes a top.
    pub fn new(triangles: &[Triangle]) -> Result<Self, ZoneCookError> {
        let mut ground = Self { triangles: Vec::new(), buckets: BTreeMap::new() };
        for triangle in triangles {
            let corners = triangle.map(|corner| corner.map(f64::from));
            if up(&corners) <= 0.0 {
                continue;
            }
            let index = u32::try_from(ground.triangles.len()).map_err(|_| {
                ZoneCookError::invalid("colliders", "they have too many faces to stand plants on")
            })?;
            let bucket = |value: f64| (value / BUCKET).floor() as i64;
            let (low, high) = corners.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(low, high), corner| {
                (
                    [low[0].min(corner[0]), low[1].min(corner[2])],
                    [high[0].max(corner[0]), high[1].max(corner[2])],
                )
            });
            let (x0, z0, x1, z1) = (bucket(low[0]), bucket(low[1]), bucket(high[0]), bucket(high[1]));
            if (x1 - x0 + 1).saturating_mul(z1 - z0 + 1) > MAX_BUCKETS {
                let problem = "a face of them spans more ground than plants may be placed on";
                return Err(ZoneCookError::invalid("colliders", problem));
            }
            for x in x0..=x1 {
                for z in z0..=z1 {
                    ground.buckets.entry((x, z)).or_default().push(index);
                }
            }
            ground.triangles.push(corners);
        }
        Ok(ground)
    }
}

/// Twice the area of a triangle's footprint, positive when its corners run anticlockwise from above: the y of
/// `(b − a) × (c − a)`.
fn up([a, b, c]: &[[f64; 3]; 3]) -> f64 {
    (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2])
}

impl Ground for ColliderGround {
    fn at(&self, x: f32, z: f32) -> Option<GroundPoint> {
        let (px, pz) = (f64::from(x), f64::from(z));
        let bucket = ((px / BUCKET).floor() as i64, (pz / BUCKET).floor() as i64);
        let mut best: Option<(f64, [f64; 3])> = None;
        for &index in self.buckets.get(&bucket)? {
            let [a, b, c] = self.triangles[index as usize];
            // Over it, edges included: each edge's side of the point as `up` measures a corner's.
            let side = |from: [f64; 3], to: [f64; 3]| {
                (to[2] - from[2]) * (px - from[0]) - (to[0] - from[0]) * (pz - from[2])
            };
            if side(a, b) < 0.0 || side(b, c) < 0.0 || side(c, a) < 0.0 {
                continue;
            }
            let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            let normal = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let height = a[1] - (normal[0] * (px - a[0]) + normal[2] * (pz - a[2])) / normal[1];
            // The highest, and of two as high the first, so the answer is the triangles' order's.
            if best.is_none_or(|(highest, _)| height > highest) {
                best = Some((height, normal));
            }
        }
        best.map(|(height, normal)| GroundPoint { height: height as f32, normal: normal.map(|n| n as f32) })
    }
}

/// The ground a zone's sets share, kept from one set's cook to the next while the colliders are the same.
#[derive(Default)]
pub struct SharedGround(Mutex<Option<(blake3::Hash, Arc<ColliderGround>)>>);

impl SharedGround {
    /// The ground of `colliders`, made by the first set that asks.
    pub fn of(&self, colliders: &[u8]) -> Result<Arc<ColliderGround>, ZoneCookError> {
        let hash = blake3::hash(colliders);
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((kept_hash, ground)) = &*kept
            && *kept_hash == hash
        {
            return Ok(Arc::clone(ground));
        }
        let ground = Arc::new(ColliderGround::from_cooked(colliders)?);
        *kept = Some((hash, Arc::clone(&ground)));
        Ok(ground)
    }
}

/// Cooks the cell `cell`'s set of `foliage`, on `grid`, standing on `ground`: the `.sscatter`'s bytes, read back
/// through the runtime's check.
pub fn cook(
    foliage: &FoliageDocument,
    ground: &impl Ground,
    grid: Grid,
    cell: CellCoord,
) -> Result<Vec<u8>, ZoneCookError> {
    let set = scatter_set(foliage, ground, grid, cell)?;
    let cooked = set.to_cooked()?;
    sapphire_zone::ScatterSet::read(&cooked)?;
    Ok(cooked.into_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ramp rising 1 m in 10 along X over a 20 m square, and a box's top 2 m up over part of it.
    fn ground() -> ColliderGround {
        let ramp: [Triangle; 2] = [
            [[0.0, 0.0, 0.0], [0.0, 0.0, 20.0], [20.0, 2.0, 20.0]],
            [[0.0, 0.0, 0.0], [20.0, 2.0, 20.0], [20.0, 2.0, 0.0]],
        ];
        let roof: [Triangle; 2] = [
            [[4.0, 3.0, 4.0], [4.0, 3.0, 8.0], [8.0, 3.0, 8.0]],
            [[4.0, 3.0, 4.0], [8.0, 3.0, 8.0], [8.0, 3.0, 4.0]],
        ];
        // The roof's underside, which faces down, is never ground.
        let under: Triangle = [[4.0, 2.5, 4.0], [8.0, 2.5, 8.0], [4.0, 2.5, 8.0]];
        let triangles: Vec<Triangle> = ramp.into_iter().chain(roof).chain([under]).collect();
        ColliderGround::new(&triangles).expect("small")
    }

    #[test]
    fn the_ground_is_the_highest_face_up_under_a_point() {
        let ground = ground();
        let on_ramp = ground.at(15.0, 10.0).expect("over the ramp");
        assert!((on_ramp.height - 1.5).abs() < 1e-6, "{on_ramp:?}");
        let [nx, ny, nz] = on_ramp.normal;
        assert!(ny > 0.0 && nx < 0.0 && nz.abs() < 1e-6, "it leans back up the ramp: {on_ramp:?}");
        assert_eq!(ground.at(6.0, 6.0).expect("over the roof").height, 3.0);
        assert_eq!(ground.at(4.0, 6.0).expect("on the roof's edge").height, 3.0);
        assert_eq!(ground.at(-1.0, 5.0), None, "off every face");
        assert_eq!(ground.at(5.0, 300.0), None, "in no bucket");
    }

    #[test]
    fn a_face_too_wide_to_bucket_is_refused() {
        let huge: Triangle = [[-1e5, 0.0, -1e5], [-1e5, 0.0, 1e5], [1e5, 0.0, 1e5]];
        let error = ColliderGround::new(&[huge]).err().expect("refused").to_string();
        assert!(error.contains("colliders"), "{error}");
    }

    #[test]
    fn the_shared_ground_is_made_once_for_the_same_colliders() {
        use sapphire_colliders::{ColliderDocument, ColliderEntry, ColliderShape, Rotation};
        let entry = ColliderEntry {
            name: "floor".to_owned(),
            shape: ColliderShape::Box { half_extents: [10.0, 0.5, 10.0] },
            position: [0.0, -0.5, 0.0],
            rotation: Rotation::default(),
            groups: vec![Group::World],
        };
        let set = ColliderDocument { colliders: vec![entry] }.cook().expect("cooks");
        let bytes = set.to_cooked().expect("writes");
        let shared = SharedGround::default();
        let first = shared.of(&bytes).expect("reads");
        assert!(Arc::ptr_eq(&first, &shared.of(&bytes).expect("reads")));
        assert_eq!(first.at(1.0, 2.0).expect("on the floor").height, 0.0);
    }
}
