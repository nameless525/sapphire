//! Zones (M3 W1, spec §5.21): `.zone.ron` cooked to `.szone`, and a mesh per cell.
//!
//! The document and the cooked format are `sapphire_zone`'s, a plain-data crate the runtime reads too,
//! which is what keeps renderers, worlds and physics out of the cooker (#74's lesson). This module
//! places them in the cook: a source with a sidecar like any asset (`importer: Zone(())`), found by its
//! suffix, with no list anywhere of what zones there are.
//!
//! # Prefabs
//!
//! **A prefab is an assembly**, named by its sidecar's ID: kit pieces and shapes, placed and given
//! slots, with materials bound to those slots by its sidecar. That's the whole of the design: an
//! `.assembly.ron` already is a reusable arrangement of pieces with material slots, and a single kit
//! piece is a one-line assembly. A sidecar saying `cook: false` keeps it from cooking a mesh of its own
//! while a zone can still place it. A placement can override the transform and swap materials by slot
//! name. All of it is resolved here, so nothing about prefabs reaches the runtime: a cell's mesh names
//! final materials, and a zone names only final meshes. Any other importer as a prefab is refused.
//!
//! # Cells
//!
//! Culling is per instance by bounding sphere, and an instance is only as fine as its mesh, so the cook
//! merges each cell's geometry into one mesh, a sub-asset of the zone (`town.cell.2.-1.smesh`), in the
//! zone's space. Whole pieces go to cells and triangles are never cut: a piece belongs to the cell its
//! anchor falls in ([`sapphire_zone::grid`]), and a piece reaching more than a cell past its square
//! fails the cook, naming the placement, as one for the backdrop or for `split`. A placement with `split`
//! makes a piece of each of its assembly's own top-level placements, so one whole town document goes
//! in with one line.
//!
//! Each material used is a submesh, however many prefabs use it: draws are per material, and the
//! material a slot is bound to is what decides. Positions default to `Float32`, so pieces abutting
//! across a cell's edge meet exactly, as quantizing each across its own bounds wouldn't have them (the
//! town's sidecar says the same), and there are no LODs by default, as the town has none (a cell's
//! sized for its whole distance).
//!
//! # Keys
//!
//! A cell's key holds what's in that cell, not the document: [`ZonePlan::recipe`] hashes its
//! pieces' prefabs, transforms and final materials, and its dependencies are its prefabs' documents
//! and what those read. So a save to one placement, or to a piece only some cells place, recooks those
//! cells and the zone's own `.szone`, whose key holds its cells' keys. Each cell then re-encodes for its
//! pack alone.
//!
//! The grain stops at prefabs: a save to a split prefab recooks every cell it reaches. A piece's own
//! file doesn't tell which of a prefab's entries place it, short of asking what the document's groups
//! nest, so this is the simple thing, and a cook that takes seconds at worst.
//!
//! # What a cook checks that a document can't
//!
//! On every cook, cached or not, since what's named can change while the zone stays the same: each
//! material, colliders, probe and encounter reference is that kind of asset of this tree, each instance's
//! a static mesh, and each encounter names this zone's own triggers and spawners.
//!
//! # Navigation
//!
//! A document with `navigation` makes a `.snav` of its own, a sub-asset (`town.snav`), baked from the
//! cooked `.scol` the zone names, with the tile bounds and seeds the document gives. Its key holds that
//! section, its spawners' and checkpoints' places, what its spawners patrol and the colliders' key, and
//! nothing else: a save to a kit piece, a placement or a light leaves it alone, and a moved collider rebakes
//! the tiles it reaches ([`crate::nav`]). Spawners and checkpoints seed it, so the cook refuses one that
//! stands off it. The zone's `.szone` holds its ID and its key.
//!
//! A bake holds each patrol to what it made: every point is on the mesh, with a way to it from the one
//! before, or the cook fails by that point's field. They aren't seeds, so a patrol can't make a roof real.
//!
//! # Exits
//!
//! An exit names another zone and a spawner in it, and a cook holds the two together from the other's
//! document, since each of two zones that lead to each other can't wait for the other's output. All the other
//! gives this one's `.szone` is its pack's name, from its source's stem, so that's in the key. Nothing of the
//! other is a reference, so it goes in no pack of this one's.
//!
//! # Shapes
//!
//! An editor moves one placement while the cells hold it merged, so an editor's cook (`CookOptions::editor`,
//! `cargo xtask content --cook editor`) also makes a mesh of each **shape** a zone places (M3 E5): a prefab,
//! with the materials a placement swaps and the axes its scale mirrors, in the prefab's own space
//! ([`sapphire_zone::Placement::shape_name`]). An editor draws each placement as its shape's mesh at the
//! rest of its transform, and a drag moves an instance with no cook. A shape's mesh cooks as a cell's does,
//! and its key holds its prefab, final materials and mirror, so a moved placement recooks only its cell.
//! Nothing names them and a game loads none, so a cook for an app makes none, and its packs are the bytes
//! they'd be without them; in an editor's, the packer puts them in the zone's pack with it. A zone with a
//! `split` placement has none: its prefab is a whole town.
//!
//! # Colliders
//!
//! A prefab carries no colliders: `colliders` is one set the zone names, as `PhysicsWorld::from_cooked`
//! takes, written apart from what's drawn. A placement or an instance names the colliders that stand for
//! it, and the `.szone`'s cook holds the two together ([`claims`]), so its key holds the colliders' too
//! whenever something names one.
//!
//! # What it doesn't do
//!
//! The coplanar check runs within an assembly and not between placements, so two prefabs fighting in
//! one plane aren't refused yet.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::str::Utf8Error;
use std::sync::{Arc, Mutex, PoisonError};

use glam::{Mat3, Mat4, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::mesh::{Lod, PositionFormat, Submesh, Vertex};
use sapphire_math::det;
use sapphire_nav::{NavGraph, PathSearch, Reach, RouteEnd};
use sapphire_navmesh::NavMesh;
use sapphire_serialize::postcard;
use sapphire_tod::TodTimeline;
use sapphire_zone::{Bounds, Place, Zone, ZoneDocument, ZoneError, ZoneGeometry};
use serde::{Deserialize, Serialize};

use crate::assembly::{self, Assembly, AssemblyError, Detail, matches_pattern};
use crate::bake::OutlineSettings;
use crate::cache::{Cache, CacheError, CacheKey, Stage};
use crate::character_maps;
use crate::mesh::{ImportedMaterial, ImportedMesh};
use crate::meta::{ImportSettings, SourceAsset};
use crate::nav::{self, BakeSettings, NavBakeError, Seed};
use crate::optimize::LodSettings;

pub mod claims;
pub mod visibility;

/// Reads and checks the document, its prefabs' too. Bump the version whenever that changes output for
/// the same document.
pub const IMPORTER: Stage = Stage { name: "zone document", version: 1 };

/// The `.szone` writer. Bump the version whenever it changes output for the same document and cells.
pub const COOKER: Stage = Stage { name: "szone", version: 1 };

/// What merges a cell's placed prefabs into one mesh: carries the assembly importer's version, as that
/// makes the triangles. Bump the top whenever merging changes output for the same pieces.
pub const CELL_IMPORTER: Stage = Stage { name: "zone cell", version: (1 << 28) | assembly::IMPORTER.version };

/// What a zone source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".zone.ron";

/// The document type a zone's `rigs` must be. The library's reader is `sapphire_world`'s, which the
/// cooker can't link, so a cook checks what it can and the runtime reads the rest.
const RIG_LIBRARY: &str = "sapphire::world::LightRigLibrary";

/// The document type a zone's `sky` must be: a sky material, not one of another family's.
const SKY_MATERIAL: &str = "sapphire::render::SkyMaterial";

/// Import settings for zones: how their cells' meshes cook. A source's own `MeshSettings` don't apply,
/// as a prefab's triangles join a cell's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneSettings {
    pub outline: OutlineSettings,
    pub lods: LodSettings,
    pub position_format: PositionFormat,
}

impl Default for ZoneSettings {
    fn default() -> Self {
        Self {
            outline: OutlineSettings::default(),
            lods: LodSettings::new(Vec::new(), 0).expect("an empty chain is valid"),
            position_format: PositionFormat::Float32,
        }
    }
}

/// A zone document that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum ZoneCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] Utf8Error),
    #[error(transparent)]
    Zone(#[from] ZoneError),
    #[error("`{field}`: failed to place prefab {}", path.display())]
    Prefab {
        field: String,
        path: PathBuf,
        /// Boxed, as the cook's are: document and glTF errors are large.
        #[source]
        source: Box<AssemblyError>,
    },
    #[error("failed to read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error("`navigation`: its bake failed")]
    Navigation(#[source] NavBakeError),
}

impl ZoneCookError {
    pub(crate) fn invalid(field: impl Into<String>, problem: impl Into<String>) -> Self {
        Self::Zone(ZoneError::Invalid { field: field.into(), problem: problem.into() })
    }
}

/// Whether `path` names a zone source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// A zone source's file name without its suffix.
pub fn stem(relative: &Path) -> &str {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())]
}

/// The cooked file a zone source at `relative` writes: `town.zone.ron` → `town.szone`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    relative.with_file_name(format!("{}.{extension}", stem(relative)))
}

/// What a place's mesh is called, among a zone's sub-assets. Never changed: its ID is derived from it.
pub fn part_name(place: Place) -> String {
    match place {
        Place::Backdrop => "backdrop".to_owned(),
        Place::Cell(cell) => format!("cell/{}/{}", cell.x, cell.z),
    }
}

/// The file a place's mesh cooks to: `town.zone.ron` → `town.cell.2.-1.smesh`, `town.backdrop.smesh`.
pub fn part_path(relative: &Path, place: Place, extension: &str) -> PathBuf {
    let stem = stem(relative);
    relative.with_file_name(match place {
        Place::Backdrop => format!("{stem}.backdrop.{extension}"),
        Place::Cell(cell) => format!("{stem}.cell.{}.{}.{extension}", cell.x, cell.z),
    })
}

/// The file a shape's mesh cooks to: `proving_ground.zone.ron` → `proving_ground.prefab.0f3a….smesh`.
pub fn shape_path(relative: &Path, shape: &str, extension: &str) -> PathBuf {
    let stem = stem(relative);
    relative.with_file_name(format!("{stem}.{}.{extension}", shape.replace('/', ".")))
}

/// What a navmesh's sub-asset is called among a zone's. Never changed: its ID is derived from it.
pub const NAVIGATION_PART: &str = "navigation";

/// What a zone's navmesh is, in place of the document (see the [module docs](self)). Spawners' and then
/// checkpoints' places are what seed it; their names, what a spawner spawns and a checkpoint's volume
/// don't matter. A zone with no checkpoints keys as it did before there were any.
#[derive(Serialize)]
struct NavigationRecipe<'a> {
    navigation: &'a sapphire_zone::ZoneNavigation,
    seeds: Vec<[f32; 3]>,
    /// What's held to the mesh, and whether its last point goes on to its first.
    patrols: Vec<(&'a [[f32; 3]], bool)>,
    settings: BakeSettings,
}

/// A prefab a zone places.
struct Prefab {
    path: PathBuf,
    /// What the document's bytes hashed to when the plan read them.
    hash: blake3::Hash,
    document_bytes: u64,
    assembly: Assembly,
    /// Its sidecar's materials, by slot.
    bindings: BTreeMap<String, AssetId>,
    /// What an import made of it, once a cell has asked: every cell that places it shares one import.
    geometry: Mutex<[Option<Arc<PrefabGeometry>>; 2]>,
}

/// A prefab's triangles, by which of its own top-level placements made them.
pub struct PrefabGeometry {
    vertices: Vec<Vertex>,
    slots: Vec<String>,
    /// For each top-level placement of the assembly, its triangles, by slot.
    parts: Vec<Vec<SlotTriangles>>,
}

/// A prefab part's triangles in one slot, as three indices each into [`PrefabGeometry::vertices`].
struct SlotTriangles {
    slot: usize,
    indices: Vec<u32>,
}

impl PrefabGeometry {
    fn new(mesh: ImportedMesh, owners: &[u32], parts: usize) -> Self {
        let mut by_part: Vec<BTreeMap<usize, Vec<u32>>> = vec![BTreeMap::new(); parts];
        for submesh in &mesh.lods[0].submeshes {
            let first = submesh.first_index as usize;
            let indices = &mesh.indices[first..first + submesh.index_count as usize];
            for (triangle, corners) in indices.as_chunks::<3>().0.iter().enumerate() {
                let owner = owners[first / 3 + triangle] as usize;
                by_part[owner].entry(submesh.material as usize).or_default().extend_from_slice(corners);
            }
        }
        Self {
            vertices: mesh.vertices,
            slots: mesh.materials.into_iter().map(|material| material.name).collect(),
            parts: by_part
                .into_iter()
                .map(|slots| {
                    slots.into_iter().map(|(slot, indices)| SlotTriangles { slot, indices }).collect()
                })
                .collect(),
        }
    }
}

/// One whole thing placed in a place.
#[derive(Clone, Copy, Debug)]
struct Piece {
    /// Its index in the document's `placements`.
    placement: usize,
    /// The assembly's top-level placement it is, if its placement is split.
    part: Option<usize>,
}

/// What a place holds of a zone's placed prefabs.
pub struct PlacePlan {
    pieces: Vec<Piece>,
    /// What keys its mesh in place of the document: a hash of its pieces (see the [module docs](self)).
    recipe: blake3::Hash,
}

/// What a piece adds to its place's recipe.
#[derive(Serialize)]
struct PieceRecipe<'a> {
    prefab: AssetId,
    part: Option<u32>,
    at: [f32; 3],
    turn: f32,
    scale: [f32; 3],
    materials: &'a BTreeMap<String, AssetId>,
}

/// What a zone's document says about its cells, from before anything cooks: which parts it has, and
/// what keys each.
pub struct ZonePlan {
    /// The zone source's ID, which its parts' are derived from.
    id: AssetId,
    document: ZoneDocument,
    prefabs: BTreeMap<AssetId, Prefab>,
    /// Each placement's final materials, by its prefab's slot names.
    materials: Vec<BTreeMap<String, AssetId>>,
    places: BTreeMap<Place, PlacePlan>,
    /// Each shape's mesh, by [`sapphire_zone::Placement::shape_name`]: none if a placement is split.
    shapes: BTreeMap<String, ShapePlan>,
}

/// What a zone's placements of one shape have in common: a mesh of the prefab alone, for an editor to draw
/// each with (M3 E5).
struct ShapePlan {
    /// The first placement of it, which stands for all of them.
    placement: usize,
    /// What keys its mesh in place of the document: its prefab, its final materials and its mirror.
    recipe: blake3::Hash,
}

/// What a shape adds to its mesh's recipe.
#[derive(Serialize)]
struct ShapeRecipe<'a> {
    prefab: AssetId,
    materials: &'a BTreeMap<String, AssetId>,
    mirror: [f32; 3],
}

/// Every assembly source of the tree by ID, whether it cooks a mesh of its own or not.
pub fn prefab_sources<'a>(sources: impl Iterator<Item = &'a SourceAsset>) -> HashMap<AssetId, SourceAsset> {
    sources
        .filter(|source| matches!(source.meta.importer, ImportSettings::Assembly(_)))
        .map(|source| (source.meta.id, source.clone()))
        .collect()
}

/// A placement's transform: `at · turn · scale`. The turn goes through [`det`], as a zone's instances'
/// do, so a cell's content doesn't hang on a platform's C math library.
pub(crate) fn placement_transform(at: [f32; 3], turn: f32, scale: [f32; 3]) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::from(scale),
        det::quat_from_axis_angle(Vec3::Y, turn.to_radians()),
        Vec3::from(at),
    )
}

impl ZonePlan {
    /// Reads a zone document and its prefabs and plans its parts, checking everything that takes no
    /// geometry and nothing that has cooked. `prefabs` is [`prefab_sources`].
    pub fn new(
        id: AssetId,
        bytes: &[u8],
        prefabs: &HashMap<AssetId, SourceAsset>,
        cache: Option<&Cache>,
    ) -> Result<Self, ZoneCookError> {
        let document = parse_document(bytes, cache)?;

        let mut read = BTreeMap::new();
        let mut materials = Vec::with_capacity(document.placements.len());
        for (index, placement) in document.placements.iter().enumerate() {
            let field = format!("placements[{index}]");
            if let Entry::Vacant(entry) = read.entry(placement.prefab) {
                let source = prefabs.get(&placement.prefab).ok_or_else(|| {
                    let problem = format!("{} isn't an assembly of this content tree", placement.prefab);
                    ZoneCookError::invalid(format!("{field}.prefab"), problem)
                })?;
                entry.insert(Prefab::read(source, &field)?);
            }
            let prefab = &read[&placement.prefab];
            let mut resolved = prefab.bindings.clone();
            for (slot, id) in &placement.materials {
                if !prefab.has_slot(slot) {
                    let problem = format!("{} has no slot `{slot}`", prefab.path.display());
                    return Err(ZoneCookError::invalid(format!("{field}.materials"), problem));
                }
                resolved.insert(slot.clone(), *id);
            }
            materials.push(resolved);
        }
        // An instance is held to its colliders by what its assembly draws, as a placement is.
        for (index, instance) in document.instances.iter().enumerate() {
            if instance.colliders.is_empty() {
                continue;
            }
            if let Entry::Vacant(entry) = read.entry(instance.mesh) {
                let field = format!("instances[{index}]");
                let source = prefabs.get(&instance.mesh).ok_or_else(|| {
                    let problem = format!(
                        "{} isn't an assembly of this content tree, so nothing can hold colliders to what it \
                         draws",
                        instance.mesh
                    );
                    ZoneCookError::invalid(format!("{field}.colliders"), problem)
                })?;
                entry.insert(Prefab::read(source, &field)?);
            }
        }

        let mut pieces: BTreeMap<Place, Vec<Piece>> = BTreeMap::new();
        for (index, placement) in document.placements.iter().enumerate() {
            let Some(split) = &placement.split else {
                let place = document.place_of(placement.at, placement.backdrop);
                pieces.entry(place).or_default().push(Piece { placement: index, part: None });
                continue;
            };
            let field = format!("placements[{index}].split.backdrop");
            let assembly = &read[&placement.prefab].assembly;
            let transform = placement_transform(placement.at, placement.turn, placement.scale);
            let mut matched = vec![false; split.backdrop.len()];
            for (part, entry) in assembly.place.iter().enumerate() {
                let mut backdrop = false;
                for (pattern, matched) in split.backdrop.iter().zip(&mut matched) {
                    if matches_pattern(pattern, entry.name()) {
                        backdrop = true;
                        *matched = true;
                    }
                }
                let anchor = transform.transform_point3(Vec3::from(entry.at));
                let place = document.place_of(anchor.to_array(), backdrop);
                pieces.entry(place).or_default().push(Piece { placement: index, part: Some(part) });
            }
            // A stale pattern would drop a mountain into a cell without a word.
            if let Some(stale) = matched.iter().position(|matched| !matched) {
                let problem = format!("`{}` matches none of the prefab's placements", split.backdrop[stale]);
                return Err(ZoneCookError::invalid(field, problem));
            }
        }

        let places = pieces
            .into_iter()
            .map(|(place, pieces)| {
                let recipes: Vec<PieceRecipe<'_>> = pieces
                    .iter()
                    .map(|piece| {
                        let placement = &document.placements[piece.placement];
                        PieceRecipe {
                            prefab: placement.prefab,
                            part: piece.part.map(|part| part as u32),
                            at: placement.at,
                            turn: placement.turn,
                            scale: placement.scale,
                            materials: &materials[piece.placement],
                        }
                    })
                    .collect();
                let encoded = postcard::to_allocvec(&recipes).expect("a piece's recipe always encodes");
                (place, PlacePlan { pieces, recipe: blake3::hash(&encoded) })
            })
            .collect();
        let mut shapes = BTreeMap::new();
        if !document.has_split() {
            for (index, placement) in document.placements.iter().enumerate() {
                shapes.entry(placement.shape_name()).or_insert_with(|| {
                    let recipe = ShapeRecipe {
                        prefab: placement.prefab,
                        materials: &materials[index],
                        mirror: placement.mirror(),
                    };
                    let encoded = postcard::to_allocvec(&recipe).expect("a shape's recipe always encodes");
                    ShapePlan { placement: index, recipe: blake3::hash(&encoded) }
                });
            }
        }
        Ok(Self { id, document, prefabs: read, materials, places, shapes })
    }

    /// The zone source's ID.
    pub fn id(&self) -> AssetId {
        self.id
    }

    /// The ID of the zone's navmesh, which it has if its document says so.
    pub fn navigation_id(&self) -> Option<AssetId> {
        self.document.navigation.as_ref().map(|_| self.id.sub_asset(NAVIGATION_PART))
    }

    /// What the key of the zone's navmesh holds in place of the document's hash.
    pub fn navigation_recipe(&self) -> blake3::Hash {
        let navigation = self.document.navigation.as_ref().expect("only a document with `navigation` bakes");
        let recipe = NavigationRecipe {
            navigation,
            seeds: (self.document.spawners.iter().map(|spawner| spawner.position))
                .chain(self.document.checkpoints.iter().map(|checkpoint| checkpoint.position))
                .collect(),
            patrols: self.document.patrols().map(|(_, spline)| (&spline.points[..], spline.closed)).collect(),
            settings: BakeSettings::CHARACTER,
        };
        blake3::hash(&postcard::to_allocvec(&recipe).expect("a navigation recipe always encodes"))
    }

    /// Whether a placement or an instance names colliders that stand for it, which the `.szone`'s cook then
    /// holds to it ([`claims`]).
    pub fn has_claims(&self) -> bool {
        let placements = self.document.placements.iter().map(|placement| &placement.colliders);
        let instances = self.document.instances.iter().map(|instance| &instance.colliders);
        placements.chain(instances).any(|claims| !claims.is_empty())
    }

    /// The triangles of a prefab the plan read, in its own space, all of it at full detail: what a collider
    /// that names it is held to. `field` is what's blamed if it doesn't import.
    fn triangles(&self, prefab: AssetId, field: &str) -> Result<Vec<[Vec3; 3]>, ZoneCookError> {
        let read = &self.prefabs[&prefab];
        let geometry = read.geometry(Detail::Full).map_err(|source| ZoneCookError::Prefab {
            field: field.to_owned(),
            path: read.path.clone(),
            source: Box::new(source),
        })?;
        let corner = |index: u32| Vec3::from(geometry.vertices[index as usize].position);
        Ok(geometry
            .parts
            .iter()
            .flatten()
            .flat_map(|triangles| triangles.indices.as_chunks::<3>().0)
            .map(|&[a, b, c]| [corner(a), corner(b), corner(c)])
            .collect())
    }

    /// The ID of a place's mesh.
    pub fn part_id(&self, place: Place) -> AssetId {
        self.id.sub_asset(&part_name(place))
    }

    /// The places that have a merged mesh.
    pub fn places(&self) -> impl Iterator<Item = Place> + '_ {
        self.places.keys().copied()
    }

    /// The document as the plan read it.
    pub fn document(&self) -> &ZoneDocument {
        &self.document
    }

    /// What a place's key holds in place of the document's hash.
    pub fn recipe(&self, place: Place) -> blake3::Hash {
        self.places[&place].recipe
    }

    /// The names of the shapes that have a mesh of their own ([`sapphire_zone::Placement::shape_name`]).
    pub fn shapes(&self) -> impl Iterator<Item = &str> + '_ {
        self.shapes.keys().map(String::as_str)
    }

    /// The ID of a shape's mesh.
    pub fn shape_id(&self, shape: &str) -> AssetId {
        self.id.sub_asset(shape)
    }

    /// What a shape's key holds in place of the document's hash.
    pub fn shape_recipe(&self, shape: &str) -> blake3::Hash {
        self.shapes[shape].recipe
    }

    /// The materials a shape's mesh names, with the field each comes from.
    pub fn shape_materials(&self, shape: &str) -> impl Iterator<Item = (String, AssetId)> + '_ {
        let index = self.shapes[shape].placement;
        self.materials[index].iter().map(move |(slot, &id)| (format!("placements[{index}].{slot}"), id))
    }

    /// A shape's prefab's ID, document's path and that document's hash when the plan read it, for a key.
    pub fn shape_prefab_file(&self, shape: &str) -> (AssetId, &Path, blake3::Hash) {
        let id = self.document.placements[self.shapes[shape].placement].prefab;
        let prefab = &self.prefabs[&id];
        (id, prefab.path.as_path(), prefab.hash)
    }

    /// About how many source bytes a shape's cook starts from.
    pub fn shape_source_bytes(&self, shape: &str) -> u64 {
        self.prefabs[&self.document.placements[self.shapes[shape].placement].prefab].document_bytes
    }

    /// A shape's geometry: its prefab mirrored as its placements are, in the prefab's own space, with its
    /// slots named and bound as [`Self::merge`]'s are.
    pub fn merge_shape(
        &self,
        shape: &str,
        detail: Detail,
    ) -> Result<(ImportedMesh, BTreeMap<String, AssetId>), ZoneCookError> {
        let index = self.shapes[shape].placement;
        let field = format!("placements[{index}]");
        let mirror = Mat4::from_scale(Vec3::from(self.document.placements[index].mirror()));
        let mut by_material = BTreeMap::new();
        let piece = Piece { placement: index, part: None };
        self.place_piece(piece, mirror, &field, detail, &mut by_material)?;
        if by_material.values().all(|merged: &Merged| merged.indices.is_empty()) {
            return Err(ZoneCookError::invalid(field, "it has no triangles to draw"));
        }
        Ok(mesh_of(by_material))
    }

    /// The prefabs a place's pieces place, each once.
    fn prefabs_in(&self, place: Place) -> impl Iterator<Item = (AssetId, &Prefab)> + '_ {
        let ids: BTreeSet<AssetId> = self.places[&place]
            .pieces
            .iter()
            .map(|piece| self.document.placements[piece.placement].prefab)
            .collect();
        ids.into_iter().map(|id| (id, &self.prefabs[&id]))
    }

    /// The materials a place's pieces name, with the field each comes from.
    pub fn materials_in(&self, place: Place) -> impl Iterator<Item = (String, AssetId)> + '_ {
        self.places[&place].pieces.iter().flat_map(|piece| {
            let index = piece.placement;
            self.materials[index].iter().map(move |(slot, &id)| (format!("placements[{index}].{slot}"), id))
        })
    }

    /// A place's prefabs' IDs, documents' paths, and those documents' hashes when the plan read them,
    /// for a key: one that changed since is the caller's to refuse.
    pub fn prefab_files_in(&self, place: Place) -> Vec<(AssetId, &Path, blake3::Hash)> {
        self.prefabs_in(place).map(|(id, prefab)| (id, prefab.path.as_path(), prefab.hash)).collect()
    }

    /// About how many source bytes a place's cook starts from, for what it's taken to hold.
    pub fn source_bytes_in(&self, place: Place) -> u64 {
        self.prefabs_in(place).map(|(_, prefab)| prefab.document_bytes).sum()
    }

    /// A place's merged geometry, importing whichever of its prefabs no cell has yet in `detail`.
    /// Fails on a piece that reaches too far from its cell, on a slot with no material and on nothing
    /// to draw, each by the placement's field. The mesh's slots are named by their materials' IDs: the
    /// map this returns as well binds them.
    pub fn merge(
        &self,
        place: Place,
        detail: Detail,
    ) -> Result<(ImportedMesh, BTreeMap<String, AssetId>), ZoneCookError> {
        let grid = self.document.grid;
        let mut by_material: BTreeMap<AssetId, Merged> = BTreeMap::new();
        for &piece in &self.places[&place].pieces {
            let placement = &self.document.placements[piece.placement];
            let field = match piece.part {
                Some(part) => format!("placements[{}].place[{part}]", piece.placement),
                None => format!("placements[{}]", piece.placement),
            };
            let transform = placement_transform(placement.at, placement.turn, placement.scale);
            let bounds = self.place_piece(piece, transform, &field, detail, &mut by_material)?;
            if let (Place::Cell(cell), Some(bounds)) = (place, bounds)
                && let Some(reach) = grid.overreach(cell, bounds)
            {
                let problem = format!(
                    "reaches {reach} m past cell ({}, {}), more than a cell's {} m: set `backdrop`, or `split` it",
                    cell.x, cell.z, grid.cell_size
                );
                return Err(ZoneCookError::invalid(field, problem));
            }
        }
        if by_material.values().all(|merged| merged.indices.is_empty()) {
            return Err(ZoneCookError::invalid(
                format!("placements in {}", part_name(place)),
                "they have no triangles to draw",
            ));
        }
        Ok(mesh_of(by_material))
    }

    /// Adds `piece`'s triangles, placed by `transform`, to `by_material`, and says what bounds they have there.
    fn place_piece(
        &self,
        piece: Piece,
        transform: Mat4,
        field: &str,
        detail: Detail,
        by_material: &mut BTreeMap<AssetId, Merged>,
    ) -> Result<Option<Bounds>, ZoneCookError> {
        let placement = &self.document.placements[piece.placement];
        let prefab = &self.prefabs[&placement.prefab];
        let geometry = prefab.geometry(detail).map_err(|source| ZoneCookError::Prefab {
            field: field.to_owned(),
            path: prefab.path.clone(),
            source: Box::new(source),
        })?;
        let placing = Placing::new(transform);
        let parts = match piece.part {
            Some(part) => std::slice::from_ref(&geometry.parts[part]),
            None => geometry.parts.as_slice(),
        };
        let mut bounds: Option<Bounds> = None;
        let mut placed = HashMap::new();
        for triangles in parts.iter().flatten() {
            let slot = &geometry.slots[triangles.slot];
            let material = self.materials[piece.placement].get(slot).copied().ok_or_else(|| {
                let problem = format!(
                    "draws in slot `{slot}`, which {}'s sidecar doesn't bind, and a cell's mesh names only \
                     material assets",
                    prefab.path.display()
                );
                ZoneCookError::invalid(field, problem)
            })?;
            let merged = by_material.entry(material).or_default();
            for corners in triangles.indices.as_chunks::<3>().0 {
                let mut corners = *corners;
                if placing.mirrors {
                    corners.swap(1, 2);
                }
                for corner in corners {
                    let index = *placed.entry((material, corner)).or_insert_with(|| {
                        let vertex = placing.vertex(&geometry.vertices[corner as usize]);
                        let more = Bounds { min: vertex.position, max: vertex.position };
                        bounds = Some(bounds.map_or(more, |bounds| bounds.union(more)));
                        merged.vertices.push(vertex);
                        (merged.vertices.len() - 1) as u32
                    });
                    merged.indices.push(index);
                }
            }
        }
        Ok(bounds)
    }
}

/// A material's share of a place's mesh.
#[derive(Default)]
struct Merged {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

/// The mesh of a place's materials: a submesh for each, by ascending ID.
fn mesh_of(by_material: BTreeMap<AssetId, Merged>) -> (ImportedMesh, BTreeMap<String, AssetId>) {
    let mut mesh = ImportedMesh::default();
    let mut submeshes = Vec::with_capacity(by_material.len());
    let mut bindings = BTreeMap::new();
    for (material, merged) in by_material {
        let name = material.to_string();
        let base = mesh.vertices.len() as u32;
        submeshes.push(Submesh {
            first_index: mesh.indices.len() as u32,
            index_count: merged.indices.len() as u32,
            material: mesh.materials.len() as u32,
        });
        mesh.indices.extend(merged.indices.into_iter().map(|index| base + index));
        mesh.vertices.extend(merged.vertices);
        mesh.materials.push(ImportedMaterial { name: name.clone(), generated: Default::default() });
        bindings.insert(name, material);
    }
    mesh.authored_outline = vec![false; mesh.vertices.len()];
    mesh.lods = vec![Lod { vertex_count: mesh.vertices.len() as u32, error: 0.0, submeshes }];
    (mesh, bindings)
}

/// How a piece's vertices land in the zone.
struct Placing {
    transform: Mat4,
    normals: Mat3,
    /// A mirroring transform turns the winding inside out.
    mirrors: bool,
}

impl Placing {
    fn new(transform: Mat4) -> Self {
        let linear = Mat3::from_mat4(transform);
        Self { transform, normals: linear.inverse().transpose(), mirrors: linear.determinant() < 0.0 }
    }

    /// `vertex`, placed. Its texture coordinates stay the prefab's, so a placed copy looks like its
    /// prefab, and the outline normal is the bake's to make.
    fn vertex(&self, vertex: &Vertex) -> Vertex {
        let normal = (self.normals * Vec3::from(vertex.normal)).normalize_or_zero();
        Vertex {
            position: self.transform.transform_point3(Vec3::from(vertex.position)).to_array(),
            normal: normal.to_array(),
            outline_normal: normal.to_array(),
            ..*vertex
        }
    }
}

impl Prefab {
    fn read(source: &SourceAsset, field: &str) -> Result<Self, ZoneCookError> {
        let path = source.path.clone();
        let bytes =
            std::fs::read(&path).map_err(|source| ZoneCookError::Read { path: path.clone(), source })?;
        let assembly = assembly::read(&bytes).map_err(|source| ZoneCookError::Prefab {
            field: field.to_owned(),
            path: path.clone(),
            source: Box::new(source),
        })?;
        let bindings =
            source.meta.importer.mesh().map(|settings| settings.materials.clone()).unwrap_or_default();
        for slot in bindings.keys() {
            if !assembly.slots.iter().any(|spec| spec.name == *slot) {
                let problem =
                    format!("{}'s sidecar binds `{slot}`, which it has no slot for", path.display());
                return Err(ZoneCookError::invalid(field, problem));
            }
        }
        Ok(Self {
            hash: blake3::hash(&bytes),
            document_bytes: bytes.len() as u64,
            path,
            assembly,
            bindings,
            geometry: Mutex::new([None, None]),
        })
    }

    fn has_slot(&self, slot: &str) -> bool {
        self.assembly.slots.iter().any(|spec| spec.name == slot)
    }

    /// What it's made of, imported by whichever cell asks first while the others wait.
    fn geometry(&self, detail: Detail) -> Result<Arc<PrefabGeometry>, AssemblyError> {
        let mut geometry = self.geometry.lock().unwrap_or_else(PoisonError::into_inner);
        let geometry = &mut geometry[match detail {
            Detail::Full => 0,
            Detail::Light => 1,
        }];
        if let Some(geometry) = geometry {
            return Ok(Arc::clone(geometry));
        }
        let parts = assembly::import_parts(&self.path, &self.assembly, detail)?;
        let imported = Arc::new(PrefabGeometry::new(parts.mesh, &parts.owners, self.assembly.place.len()));
        *geometry = Some(Arc::clone(&imported));
        Ok(imported)
    }
}

/// A zone's document, from its last parse if these bytes have been parsed before. RON's number parser
/// looks for `..` through the whole rest of the document at every number (`Parser::float` and
/// `next_bytes_is_float`, ron 0.12.3), so a parse is quadratic: a second at 3,584 placements, every
/// cook, where everything else in a no-op takes a tenth of that. The copy kept is JSON because a
/// document's optional fields are left out when they're empty, which only a self-describing form reads
/// back; for the same reason a field added or taken away since reads as what a parse would give, or is
/// refused and parsed again.
fn parse_document(bytes: &[u8], cache: Option<&Cache>) -> Result<ZoneDocument, ZoneCookError> {
    let text = std::str::from_utf8(bytes)?;
    let Some(cache) = cache else {
        return Ok(ZoneDocument::from_ron(text)?);
    };
    let key = CacheKey::parsed(&blake3::hash(bytes), IMPORTER);
    // Checked as a parse is, since a check added after this was kept bumps no stage: one it fails is parsed
    // again, which says what's wrong from the document itself.
    if let Some(parsed) = cache.get(&key)?
        && let Ok(document) = serde_json::from_slice::<ZoneDocument>(&parsed)
        && document.check().is_ok()
    {
        return Ok(document);
    }
    let document = ZoneDocument::from_ron(text)?;
    cache.put(&key, &serde_json::to_vec(&document).expect("a checked document's numbers are finite"))?;
    Ok(document)
}

/// Reads the file a zone's `day` or `rigs` names, from the document's directory.
pub fn read_document(zone: &Path, field: &str, relative: &str) -> Result<(PathBuf, Vec<u8>), ZoneCookError> {
    let path = character_maps::resolve_input(zone, relative)
        .map_err(|_| ZoneCookError::invalid(field, format!("{relative:?} isn't a path from here")))?;
    let bytes = std::fs::read(&path).map_err(|source| ZoneCookError::Read { path: path.clone(), source })?;
    Ok((path, bytes))
}

/// `bytes` as a `day` document's text, checked as the runtime will read it.
fn day_text(bytes: Vec<u8>) -> Result<String, ZoneCookError> {
    let text = String::from_utf8(bytes)
        .map_err(|error| ZoneCookError::invalid("day", format!("it isn't UTF-8: {error}")))?;
    TodTimeline::from_ron(&text)
        .map_err(|error| ZoneCookError::invalid("day", format!("it doesn't read: {error}")))?;
    Ok(text)
}

/// `bytes` as a `rigs` document's text, as far as a cook can check.
fn rigs_text(bytes: Vec<u8>) -> Result<String, ZoneCookError> {
    let text = String::from_utf8(bytes)
        .map_err(|error| ZoneCookError::invalid("rigs", format!("it isn't UTF-8: {error}")))?;
    let found = sapphire_serialize::document::type_name(&text)
        .map_err(|error| ZoneCookError::invalid("rigs", format!("it doesn't read: {error}")))?;
    if found != RIG_LIBRARY {
        let problem = format!("it's a `{found}` document, not a `{RIG_LIBRARY}`");
        return Err(ZoneCookError::invalid("rigs", problem));
    }
    Ok(text)
}

/// Whether `bytes`, the source of the material a zone's `sky` names, is a sky material's (M5 M6): another
/// family's would cook, and then draw nothing in the sky.
pub fn check_sky_material(bytes: &[u8]) -> Result<(), ZoneCookError> {
    let found = sapphire_serialize::document::type_name(&String::from_utf8_lossy(bytes))
        .map_err(|error| ZoneCookError::invalid("sky", format!("it doesn't read: {error}")))?;
    if found != SKY_MATERIAL {
        return Err(ZoneCookError::invalid("sky", format!("it's a `{found}`, not a `{SKY_MATERIAL}`")));
    }
    Ok(())
}

/// `bounds` under `at · turn · scale`: the box around its corners, as an instance stands.
pub fn placed_bounds(bounds: Bounds, at: [f32; 3], turn: f32, scale: [f32; 3]) -> Bounds {
    let transform = placement_transform(at, turn, scale);
    let corner = |index: usize| {
        let pick = |axis: usize| if index & (1 << axis) == 0 { bounds.min[axis] } else { bounds.max[axis] };
        transform.transform_point3(Vec3::new(pick(0), pick(1), pick(2)))
    };
    let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for index in 0..8 {
        let corner = corner(index).to_array();
        min = std::array::from_fn(|axis| det::min(min[axis], corner[axis]));
        max = std::array::from_fn(|axis| det::max(max[axis], corner[axis]));
    }
    Bounds { min, max }
}

/// A zone's navmesh, as cooked bytes, from the `.scol` its document names.
pub fn cook_navigation(
    plan: &ZonePlan,
    colliders: &[u8],
    cache: Option<&Cache>,
) -> Result<Vec<u8>, ZoneCookError> {
    let document = plan.document();
    let navigation = document.navigation.as_ref().expect("only a document with `navigation` bakes");
    let spawners = document
        .spawners
        .iter()
        .enumerate()
        .map(|(index, spawner)| Seed { field: format!("spawners[{index}]"), position: spawner.position });
    // Where the player comes back to stands on ground enemies can walk to, or the cook refuses it.
    let checkpoints = document.checkpoints.iter().enumerate().map(|(index, checkpoint)| Seed {
        field: format!("checkpoints[{index}] ({})", checkpoint.name),
        position: checkpoint.position,
    });
    let seeds = navigation
        .seeds
        .iter()
        .enumerate()
        .map(|(index, position)| Seed { field: format!("navigation.seeds[{index}]"), position: *position });
    let seeds: Vec<Seed> = spawners.chain(checkpoints).chain(seeds).collect();
    let (baked, _) = nav::bake_colliders(
        colliders,
        (navigation.min, navigation.max),
        &seeds,
        &BakeSettings::CHARACTER,
        cache,
    )
    .map_err(ZoneCookError::Navigation)?;
    let cooked = baked.mesh.to_cooked().map_err(|error| ZoneCookError::Navigation(error.into()))?;
    let mesh = NavMesh::read(&cooked).map_err(|error| ZoneCookError::Navigation(error.into()))?;
    check_patrols(document, &NavGraph::new(mesh))?;
    Ok(cooked.into_vec())
}

/// Whether each patrol's points are on `graph`, with a way to each from the one before: what a document
/// can't say for itself, so a walker can't be sent somewhere it would stand and stare.
fn check_patrols(document: &ZoneDocument, graph: &NavGraph) -> Result<(), ZoneCookError> {
    let mut search = PathSearch::new(graph, graph.polygon_count() as u32);
    let mut corridor = [0; 1];
    for (index, spline) in document.patrols() {
        let field = |point: usize| format!("splines[{index}].points[{point}]");
        let mut last = None;
        for (point, place) in spline.points.iter().enumerate() {
            let Some(here) = graph.locate(Vec3::from_array(*place), Reach::CHARACTER) else {
                return Err(ZoneCookError::invalid(field(point), format!("{place:?} isn't on the navmesh")));
            };
            if let Some(last) = last
                && search.find(graph, last, here, &mut corridor).end != RouteEnd::Reached
            {
                let problem = format!("{place:?} has no way to it from the point before");
                return Err(ZoneCookError::invalid(field(point), problem));
            }
            last = Some(here);
        }
    }
    Ok(())
}

/// A place's mesh, as a zone's cook reads it.
pub struct CookedPlace {
    pub place: Place,
    pub id: AssetId,
    pub bounds: Bounds,
}

/// Cooks a zone document, from what its places' and its instances' meshes turned out to be: the bytes
/// of its `.szone`, read back through the runtime's checks.
pub fn cook(
    plan: &ZonePlan,
    zone: &Path,
    places: Vec<CookedPlace>,
    instance_bounds: Vec<Bounds>,
    exit_packs: Vec<String>,
) -> Result<Vec<u8>, ZoneCookError> {
    let document = plan.document();
    let instances = document
        .instances
        .iter()
        .zip(instance_bounds)
        .map(|(instance, bounds)| placed_bounds(bounds, instance.at, instance.turn, instance.scale))
        .collect();
    let day = document
        .day
        .as_deref()
        .map(|day| read_document(zone, "day", day).and_then(|(_, bytes)| day_text(bytes)))
        .transpose()?;
    let rigs = document
        .rigs
        .as_deref()
        .map(|rigs| read_document(zone, "rigs", rigs).and_then(|(_, bytes)| rigs_text(bytes)))
        .transpose()?;
    let meshes = places.into_iter().map(|place| (place.place, (place.id, place.bounds))).collect();
    let navigation = plan.navigation_id();
    let geometry = ZoneGeometry { meshes, instances, day, rigs, navigation, exit_packs };
    let cooked = document.cook(geometry)?.to_cooked()?;
    Zone::read(&cooked)?;
    Ok(cooked.into_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A parse kept in the cache is checked as one just made is, since a check added after it was kept bumps
    /// no stage: one that fails isn't taken for the document, which is parsed again.
    #[test]
    fn a_kept_parse_that_fails_a_check_is_parsed_again() {
        let scratch = crate::test_util::TempDir::new("zone_parse_kept");
        let cache = Cache::new(scratch.path());
        let bytes = br#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 32.0)))"#;
        let document = parse_document(bytes, Some(&cache)).expect("a good document");
        assert_eq!(parse_document(bytes, Some(&cache)).expect("from the cache"), document);

        let mut refused = document.clone();
        refused.grid.cell_size = 0.0;
        assert!(refused.check().is_err(), "a cell of 0 m isn't a grid");
        let key = CacheKey::parsed(&blake3::hash(bytes), IMPORTER);
        cache.put(&key, &serde_json::to_vec(&refused).unwrap()).unwrap();
        assert_eq!(parse_document(bytes, Some(&cache)).expect("parsed again"), document);
        assert_eq!(parse_document(bytes, Some(&cache)).expect("kept again"), document);
    }

    #[test]
    fn a_prefabs_geometry_is_cached_separately_for_each_detail() {
        let document = br##"(
            type: "sapphire::pipeline::Assembly", version: 1,
            data: (kits: {}, slots: [(name: "stone")], surfaces: [],
                   place: [(shape: Some(Column(())), slot: Some("stone"))]),
        )"##;
        for order in [[Detail::Full, Detail::Light], [Detail::Light, Detail::Full]] {
            let prefab = Prefab {
                path: PathBuf::from("prefab.assembly.ron"),
                hash: blake3::hash(document),
                document_bytes: document.len() as u64,
                assembly: assembly::read(document).unwrap(),
                bindings: BTreeMap::new(),
                geometry: Mutex::new([None, None]),
            };
            let first = prefab.geometry(order[0]).unwrap();
            let second = prefab.geometry(order[1]).unwrap();
            assert!(!Arc::ptr_eq(&first, &second));
            assert_ne!(first.vertices.len(), second.vertices.len());
            assert!(Arc::ptr_eq(&first, &prefab.geometry(order[0]).unwrap()));
            assert!(Arc::ptr_eq(&second, &prefab.geometry(order[1]).unwrap()));
        }
    }
}
