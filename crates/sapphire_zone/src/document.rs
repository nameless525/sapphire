//! The zone document (`.zone.ron`): a stage, written by hand or by the editor.
//!
//! ```ron
//! (
//!     type: "sapphire::zone::ZoneDocument",
//!     version: 1,
//!     data: (
//!         grid: (cell_size: 32.0),
//!         placements: [
//!             // A prefab: an assembly, by its sidecar's ID. Its geometry joins its cell's mesh, and the
//!             // zone's collider `crate_0` stands for it.
//!             (prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (4.0, 0.0, -9.0), turn: 30.0, colliders: ["crate_0"]),
//!             // With a transform and a material swapped for this placement.
//!             (
//!                 prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f",
//!                 at: (40.0, 0.0, -9.0),
//!                 scale: (1.2, 1.2, 1.2),
//!                 materials: {"timber": "35094271-5e44-4a97-aab3-817227b49287"},
//!             ),
//!             // Open land stays for as long as the zone does.
//!             (prefab: "9df1c80b-9d2d-4100-a6cc-a16331f57cb5", backdrop: true),
//!         ],
//!         // Meshes drawn on their own: what sways about its own origin, what's far away.
//!         instances: [(mesh: "e94f690f-38e2-4630-aeb3-4d34d06fc233", at: (-15.6, 0.0, -15.4), turn: 20.0)],
//!         lights: [(position: (0.3, 2.4, -35.4), color: (1.0, 0.7, 0.4), intensity: 6.0, range: 9.0, at_night: true)],
//!         probes: Some("44f04da1-31ba-4d3f-9e65-3e38a30fff43"),
//!         day: Some("../lookdev/lookdev_day.ron"),
//!         rigs: Some("../lookdev/lookdev_light_rigs.ron"),
//!         colliders: Some("0aec06e8-ef8a-4f60-8dce-1e159cc277fa"),
//!         navigation: Some((min: (-64.0, -64.0), max: (64.0, 64.0))),
//!         spawners: [
//!             (name: "player_start", position: (0.0, 0.0, 3.0), yaw_degrees: 180.0, spawns: "player"),
//!             (name: "gate_guard", position: (3.2, 0.0, -20.0), spawns: "warrior", patrol: "gate_walk"),
//!         ],
//!         triggers: [(name: "gate", position: (3.2, 1.0, -18.0), volume: Box(half_extents: (3.0, 2.0, 1.0)), event: "gate_entered")],
//!         checkpoints: [(name: "gate_steps", position: (3.2, 0.0, -14.0), yaw_degrees: 180.0, volume: Sphere(radius: 2.0))],
//!         splines: [
//!             (name: "street_rail", points: [(3.2, 2.0, -20.0), (3.2, 2.5, -40.0), (3.2, 3.0, -60.0)]),
//!             (name: "gate_walk", points: [(3.2, 0.0, -20.0), (3.2, 0.0, -30.0), (9.0, 0.0, -30.0)], closed: true),
//!         ],
//!         // Where it leads. A trigger whose `event` is `to_market` is its door.
//!         exits: [(name: "to_market", to: "e8c21831-7d98-4a79-b592-912b806cf3e2", arrive: "player_start", prefetch: true)],
//!     ),
//! )
//! ```
//!
//! [`ZoneDocument::check`] names the field of everything wrong (`spawners[2].yaw_degrees`), and unknown
//! fields are errors. Everything is plain and reflected, so the editor can edit a document by a
//! reflection path (`placements[3].at[1]`) and splice the text.
//!
//! **Stable IDs:** a spawner's, trigger's, checkpoint's, spline's or exit's name is its identity. Its ID
//! is the name's hash ([`Name`]), which no other entry's addition or order can change, so a replay and
//! a save can name it. A placement or an instance has no ID: nothing about them that the simulation
//! can see survives cook.
//!
//! **Patrols:** a spawner's `patrol` names one of the zone's splines, and what spawns there walks its points
//! in order, turning back at the end of an open one. A new patrol is a spline and a line on a spawner. Its
//! points go in the simulation's half of the cooked zone, and a cook holds each to the navmesh, so a point
//! off it or cut off from the one before fails by its field: the pipeline's `zone` module has that.
//!
//! **Prefabs** are resolved at cook, so nothing about them reaches the runtime: the pipeline's `zone`
//! module has what a prefab is. `day` and `rigs` are paths from this document's directory, since those
//! documents are no asset kind: the cook checks them and holds their text in the zone.

use std::collections::{BTreeMap, BTreeSet};

use bevy_reflect::Reflect;
use glam::{Mat4, Vec3};
use sapphire_core::Name;
use sapphire_math::det;
use sapphire_reflect::Versioned;
use sapphire_serialize::{AssetId, CookedError, DocumentError};
use serde::{Deserialize, Serialize};

use crate::cooked::{
    CookedCheckpoint, CookedExit, CookedInstance, CookedLight, CookedLightShape, CookedPatrol, CookedSpawner,
    CookedSpline, CookedTrigger, CookedVolume, PlaceContents, Zone, ZoneCell, ZonePresentation,
    ZoneSimulation,
};
use crate::grid::{Bounds, Grid, MAX_COORDINATE, Place};

/// The most entries any list of a zone may hold.
pub const MAX_ENTRIES: usize = 1 << 20;
/// The longest name or named event, in bytes.
pub const MAX_NAME: usize = 256;
/// The most points a spline may hold.
pub const MAX_SPLINE_POINTS: usize = 1 << 16;
/// The thinnest extent or radius of a trigger's volume, in metres.
pub const MIN_EXTENT: f32 = 0.001;

/// A zone document (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct ZoneDocument {
    pub grid: Grid,
    /// Prefabs, whose geometry the cook merges into a mesh per cell.
    #[serde(default)]
    pub placements: Vec<Placement>,
    /// Meshes drawn on their own, activated with their cell.
    #[serde(default)]
    pub instances: Vec<Instance>,
    #[serde(default)]
    pub lights: Vec<Light>,
    /// The zone's ambient probe grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probes: Option<AssetId>,
    /// A time-of-day timeline's document, from this document's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    /// A character light rig library's document, from this document's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rigs: Option<String>,
    /// The zone's static colliders: a `.colliders.ron` source, by its sidecar's ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colliders: Option<AssetId>,
    #[serde(default)]
    pub spawners: Vec<Spawner>,
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    /// Where a walk through the zone can start again (M3 B4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<Checkpoint>,
    /// Camera rails.
    #[serde(default)]
    pub splines: Vec<Spline>,
    /// What to bake a navmesh over. With this, a cook makes a `.snav` from `colliders`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigation: Option<ZoneNavigation>,
    /// Encounters: `.encounter.ron` sources, by their sidecars' IDs.
    #[serde(default)]
    pub encounters: Vec<AssetId>,
    /// Where this zone leads.
    #[serde(default)]
    pub exits: Vec<Exit>,
}

impl Versioned for ZoneDocument {
    /// 1: M3 W1.
    const VERSION: u32 = 1;
}

/// One placed prefab, transformed by `at · turn · scale`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Placement {
    /// An assembly, by its sidecar's ID.
    pub prefab: AssetId,
    #[serde(default)]
    pub at: [f32; 3],
    /// Degrees about +Y, counter-clockwise seen from above.
    #[serde(default)]
    pub turn: f32,
    #[serde(default = "unit_scale")]
    pub scale: [f32; 3],
    /// Material assets in place of those the prefab's sidecar binds, by the prefab's slot names.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub materials: BTreeMap<String, AssetId>,
    /// Stays for as long as the zone does, in no cell.
    #[serde(default, skip_serializing_if = "is_false")]
    pub backdrop: bool,
    /// Each of the assembly's own placements is a piece, in the cell its position falls in, rather than
    /// the whole being one piece at `at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<Split>,
    /// The zone's colliders that stand for what this draws, by name: what a body meets of it, since a zone's
    /// colliders are a document of their own. A cook holds each to what this draws, so moving one without
    /// the other is refused (M4 X1). With none, nothing holds it to the colliders.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colliders: Vec<String>,
}

/// How a [`Placement`] of a whole town is cut along the grid.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Split {
    /// Which of the assembly's placements go in the backdrop, by what they place (`"#ground"`,
    /// `"kenney/rock-*"`, `"@hall"`), `*` matching any run of characters.
    #[serde(default)]
    pub backdrop: Vec<String>,
}

/// One mesh asset drawn on its own, transformed by `at · turn · scale`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub mesh: AssetId,
    #[serde(default)]
    pub at: [f32; 3],
    /// Degrees about +Y, counter-clockwise seen from above.
    #[serde(default)]
    pub turn: f32,
    #[serde(default = "unit_scale")]
    pub scale: [f32; 3],
    /// Stays for as long as the zone does, in no cell.
    #[serde(default, skip_serializing_if = "is_false")]
    pub backdrop: bool,
    /// The zone's colliders that stand for what this draws, by name: what a body meets of it, since a zone's
    /// colliders are a document of their own. A cook holds each to what this draws, so moving one without
    /// the other is refused (M4 X1). With none, nothing holds it to the colliders.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colliders: Vec<String>,
}

/// A local light, as the renderer's `LocalLight` takes one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Light {
    pub position: [f32; 3],
    /// Linear RGB.
    pub color: [f32; 3],
    pub intensity: f32,
    /// Distance in metres at which it has faded to nothing.
    pub range: f32,
    #[serde(default)]
    pub shape: LightShape,
    /// Its intensity follows time of day's night, as a lamp's does.
    #[serde(default, skip_serializing_if = "is_false")]
    pub at_night: bool,
    /// Stays for as long as the zone does, in no cell.
    #[serde(default, skip_serializing_if = "is_false")]
    pub backdrop: bool,
}

/// The shape of a [`Light`]'s light.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub enum LightShape {
    #[default]
    Point,
    /// A cone along `direction`, which needn't be unit length; angles are half angles.
    Spot { direction: [f32; 3], inner_degrees: f32, outer_degrees: f32 },
}

/// A place something spawns at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Spawner {
    /// Unique among the zone's spawners: its identity (see the [module docs](self)).
    pub name: String,
    /// Where its feet stand, in metres.
    pub position: [f32; 3],
    /// About +Y, in degrees; 0 faces +Z.
    #[serde(default)]
    pub yaw_degrees: f32,
    /// What spawns here, by a name gameplay knows.
    pub spawns: String,
    /// The spline in this zone that what spawns here walks, if it walks one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patrol: Option<String>,
}

/// A way out of a zone: all there is to say of a transition but how it looks (M3 W4). A trigger whose `event`
/// is an exit's name is that exit's door, for whoever hears triggers.
///
/// Naming a zone loads nothing of it and puts nothing of it in this zone's pack: the two stay apart, however
/// they lead to each other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Exit {
    /// Unique among the zone's exits: its identity (see the [module docs](self)).
    pub name: String,
    /// The zone it leads to: a `.zone.ron` source, by its sidecar's ID.
    pub to: AssetId,
    /// Which of that zone's spawners whoever takes it arrives at. A cook holds the two together.
    pub arrive: String,
    /// Whether that zone's pack comes in ahead of time, as long as anyone's in this zone: what makes this
    /// zone a hub. It costs memory for a door that may not be taken, so an exit only asks if it's likely.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prefetch: bool,
}

/// Where a zone's navmesh bakes, and which of what it finds is real.
///
/// A bake finds every flat top a character could stand on, roofs and crates too, and most of them lead
/// nowhere. **The ground that's real is what a character can walk to from a spawner or from a seed:** a
/// cook floods out from each and drops the rest, and a spawner off the mesh fails the cook, by its field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct ZoneNavigation {
    /// One corner of where a character can be, as metres on X and Z. Ground drawn past this bakes no
    /// tiles, however far it goes on.
    pub min: [f32; 2],
    /// The other corner.
    pub max: [f32; 2],
    /// Ground that's real although no spawner stands on it: somewhere a player reaches by a jump or a
    /// cutscene, say. In metres, at a character's feet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seeds: Vec<[f32; 3]>,
}

/// A volume that raises an event when something enters it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    /// Unique among the zone's triggers: its identity (see the [module docs](self)).
    pub name: String,
    /// The volume's center, in metres.
    pub position: [f32; 3],
    pub volume: Volume,
    /// What it raises, by a name gameplay knows.
    pub event: String,
}

/// Somewhere a walk through the zone can start again (M3 B4): what a save names to start the zone there,
/// and what a teleport puts a player back at. A player standing in its volume with no encounter running
/// has reached it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    /// Unique among the zone's checkpoints: its identity (see the [module docs](self)), which a save holds.
    pub name: String,
    /// Where a player's feet stand when the zone starts here, in metres.
    pub position: [f32; 3],
    /// About +Y, in degrees; 0 faces +Z.
    #[serde(default)]
    pub yaw_degrees: f32,
    /// What reaches it, about its position.
    pub volume: Volume,
}

/// A trigger's or a checkpoint's volume, about its position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub enum Volume {
    /// Along the zone's axes.
    Box {
        half_extents: [f32; 3],
    },
    Sphere {
        radius: f32,
    },
}

/// Points in order: a Catmull-Rom's control points for a camera to ride, or, for a spawner naming it as its
/// `patrol`, where to walk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Spline {
    /// Unique among the zone's splines: its identity (see the [module docs](self)).
    pub name: String,
    pub points: Vec<[f32; 3]>,
    /// Whether it runs on from its last point to its first.
    #[serde(default, skip_serializing_if = "is_false")]
    pub closed: bool,
}

fn unit_scale() -> [f32; 3] {
    [1.0; 3]
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_false(value: &bool) -> bool {
    !*value
}

/// A zone document that can't cook, or a cooked zone the runtime refuses.
#[derive(Debug, thiserror::Error)]
pub enum ZoneError {
    #[error(transparent)]
    Document(#[from] Box<DocumentError>),
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("`{field}`: {problem}")]
    Invalid { field: String, problem: String },
    /// A digest needs an asset the zone names, which whoever asked for it didn't hand over.
    #[error("the zone's {what} {id} isn't loaded")]
    NotLoaded { what: &'static str, id: AssetId },
}

impl ZoneError {
    pub(crate) fn invalid(field: impl Into<String>, problem: impl Into<String>) -> Self {
        Self::Invalid { field: field.into(), problem: problem.into() }
    }
}

/// What a cook made of a zone's geometry and read from its meshes, for [`ZoneDocument::cook`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneGeometry {
    /// Each place's merged mesh, if it has one, and its bounds in the zone's space.
    pub meshes: BTreeMap<Place, (AssetId, Bounds)>,
    /// Each of the document's instances' bounds as placed, in its order.
    pub instances: Vec<Bounds>,
    /// The text of the document's `day`.
    pub day: Option<String>,
    /// The text of the document's `rigs`.
    pub rigs: Option<String>,
    /// The navmesh baked for the document's `navigation`.
    pub navigation: Option<AssetId>,
    /// The pack each of the document's exits leads into, in its order.
    pub exit_packs: Vec<String>,
}

impl ZoneDocument {
    /// Reads a document from RON text and checks it.
    pub fn from_ron(text: &str) -> Result<Self, ZoneError> {
        let document: Self = sapphire_serialize::from_ron(text).map_err(Box::new)?;
        document.check()?;
        Ok(document)
    }

    /// Whether a placement cuts its prefab along the grid. Such a zone's placements cook into its cells
    /// alone, with no mesh of their shapes ([`Placement::shape_name`]), since a whole town's prefab would
    /// be the town again.
    #[must_use]
    pub fn has_split(&self) -> bool {
        self.placements.iter().any(|placement| placement.split.is_some())
    }

    /// Where a thing placed at `at` belongs.
    #[must_use]
    pub fn place_of(&self, at: [f32; 3], backdrop: bool) -> Place {
        if backdrop { Place::Backdrop } else { Place::Cell(self.grid.cell_of(at)) }
    }

    /// Checks everything that needs nothing outside the document, naming the field.
    pub fn check(&self) -> Result<(), ZoneError> {
        self.grid.check().map_err(|problem| ZoneError::invalid("grid.cell_size", problem))?;
        for (list, count) in [
            ("placements", self.placements.len()),
            ("instances", self.instances.len()),
            ("lights", self.lights.len()),
            ("spawners", self.spawners.len()),
            ("triggers", self.triggers.len()),
            ("checkpoints", self.checkpoints.len()),
            ("splines", self.splines.len()),
            ("navigation.seeds", self.navigation.as_ref().map_or(0, |navigation| navigation.seeds.len())),
            ("encounters", self.encounters.len()),
            ("exits", self.exits.len()),
        ] {
            if count > MAX_ENTRIES {
                return Err(ZoneError::invalid(
                    list,
                    format!("{count} is more than the {MAX_ENTRIES} it may hold"),
                ));
            }
        }
        let invalid = |field: String| move |problem: String| ZoneError::invalid(field, problem);
        for (index, placement) in self.placements.iter().enumerate() {
            let field = |name: &str| format!("placements[{index}].{name}");
            check_position(placement.at).map_err(invalid(field("at")))?;
            check_finite(placement.turn).map_err(invalid(field("turn")))?;
            check_scale(placement.scale).map_err(invalid(field("scale")))?;
            if placement.backdrop && placement.split.is_some() {
                return Err(ZoneError::invalid(
                    field("split"),
                    "a split placement puts pieces in cells: name those for the backdrop in `split.backdrop`",
                ));
            }
            self.check_claims(&placement.colliders, &field("colliders"))?;
        }
        for (index, instance) in self.instances.iter().enumerate() {
            let field = |name: &str| format!("instances[{index}].{name}");
            check_position(instance.at).map_err(invalid(field("at")))?;
            check_finite(instance.turn).map_err(invalid(field("turn")))?;
            check_scale(instance.scale).map_err(invalid(field("scale")))?;
            self.check_claims(&instance.colliders, &field("colliders"))?;
        }
        for (index, light) in self.lights.iter().enumerate() {
            let field = |name: &str| format!("lights[{index}].{name}");
            check_position(light.position).map_err(invalid(field("position")))?;
            check_color(light.color).map_err(invalid(field("color")))?;
            check_positive(light.intensity).map_err(invalid(field("intensity")))?;
            check_positive(light.range).map_err(invalid(field("range")))?;
            if let LightShape::Spot { direction, inner_degrees, outer_degrees } = light.shape {
                check_spot(direction, inner_degrees.to_radians(), outer_degrees.to_radians())
                    .map_err(invalid(field("shape")))?;
            }
        }
        if let Some(day) = &self.day {
            check_path(day).map_err(invalid("day".to_owned()))?;
        }
        if let Some(rigs) = &self.rigs {
            check_path(rigs).map_err(invalid("rigs".to_owned()))?;
        }
        check_names("spawners", self.spawners.iter().map(|spawner| spawner.name.as_str()))?;
        for (index, spawner) in self.spawners.iter().enumerate() {
            let field = |name: &str| format!("spawners[{index}].{name}");
            check_position(spawner.position).map_err(invalid(field("position")))?;
            check_finite(spawner.yaw_degrees).map_err(invalid(field("yaw_degrees")))?;
            check_name(&spawner.spawns).map_err(invalid(field("spawns")))?;
            if let Some(patrol) = &spawner.patrol {
                self.check_patrol(patrol).map_err(invalid(field("patrol")))?;
            }
        }
        check_names("triggers", self.triggers.iter().map(|trigger| trigger.name.as_str()))?;
        for (index, trigger) in self.triggers.iter().enumerate() {
            let field = |name: &str| format!("triggers[{index}].{name}");
            check_position(trigger.position).map_err(invalid(field("position")))?;
            trigger.volume.cooked().check().map_err(invalid(field("volume")))?;
            check_name(&trigger.event).map_err(invalid(field("event")))?;
        }
        check_names("checkpoints", self.checkpoints.iter().map(|checkpoint| checkpoint.name.as_str()))?;
        for (index, checkpoint) in self.checkpoints.iter().enumerate() {
            let field = |name: &str| format!("checkpoints[{index}].{name}");
            check_position(checkpoint.position).map_err(invalid(field("position")))?;
            check_finite(checkpoint.yaw_degrees).map_err(invalid(field("yaw_degrees")))?;
            checkpoint.volume.cooked().check().map_err(invalid(field("volume")))?;
        }
        check_names("splines", self.splines.iter().map(|spline| spline.name.as_str()))?;
        for (index, spline) in self.splines.iter().enumerate() {
            let field = format!("splines[{index}].points");
            check_points(spline.points.len(), |point| spline.points[point]).map_err(invalid(field))?;
        }
        if let Some(navigation) = &self.navigation {
            self.check_navigation(navigation)?;
        }
        check_names("exits", self.exits.iter().map(|exit| exit.name.as_str()))?;
        for (index, exit) in self.exits.iter().enumerate() {
            check_name(&exit.arrive).map_err(invalid(format!("exits[{index}].arrive")))?;
        }
        Ok(())
    }

    /// What a placement's or an instance's `colliders` (`field`) may name: the zone's colliders, by names that
    /// name something, each once. Whether the zone's set has them is the cook's to say.
    fn check_claims(&self, claims: &[String], field: &str) -> Result<(), ZoneError> {
        if !claims.is_empty() && self.colliders.is_none() {
            return Err(ZoneError::invalid(field, "it names colliders, and the zone names no `colliders`"));
        }
        let mut seen = BTreeSet::new();
        for (index, claim) in claims.iter().enumerate() {
            let field = format!("{field}[{index}]");
            check_name(claim).map_err(|problem| ZoneError::invalid(field.clone(), problem))?;
            if !seen.insert(claim.as_str()) {
                return Err(ZoneError::invalid(field, format!("`{claim}` is named twice")));
            }
        }
        Ok(())
    }

    /// Whether a spawner may name `patrol` as what it walks.
    fn check_patrol(&self, patrol: &str) -> Result<(), String> {
        if self.navigation.is_none() {
            return Err("a patrol walks the zone's navmesh, and it asks for none".to_owned());
        }
        if !self.splines.iter().any(|spline| spline.name == patrol) {
            return Err(format!("`{patrol}` isn't one of the zone's splines"));
        }
        Ok(())
    }

    /// The splines some spawner walks, with where each is in `splines`, in that order.
    pub fn patrols(&self) -> impl Iterator<Item = (usize, &Spline)> {
        self.splines.iter().enumerate().filter(|(_, spline)| {
            self.spawners.iter().any(|spawner| spawner.patrol.as_deref() == Some(spline.name.as_str()))
        })
    }

    fn check_navigation(&self, navigation: &ZoneNavigation) -> Result<(), ZoneError> {
        let field = |name: &str| format!("navigation.{name}");
        let invalid = |field: String| move |problem: String| ZoneError::invalid(field, problem);
        let ZoneNavigation { min, max, seeds } = navigation;
        check_position([min[0], 0.0, min[1]]).map_err(invalid(field("min")))?;
        check_position([max[0], 0.0, max[1]]).map_err(invalid(field("max")))?;
        if min[0] >= max[0] || min[1] >= max[1] {
            let problem = format!("{min:?} isn't below {max:?} on both axes");
            return Err(ZoneError::invalid(field("min"), problem));
        }
        for (index, seed) in seeds.iter().enumerate() {
            check_position(*seed).map_err(invalid(format!("navigation.seeds[{index}]")))?;
        }
        if self.colliders.is_none() {
            return Err(ZoneError::invalid(
                "navigation",
                "a navmesh bakes from the zone's `colliders`, and it names none",
            ));
        }
        if self.spawners.is_empty() && seeds.is_empty() {
            return Err(ZoneError::invalid(
                "navigation",
                "nothing says which ground is real: it needs a spawner, or an entry in `seeds`",
            ));
        }
        Ok(())
    }

    /// What the simulation reads of this document, with stable IDs, in ID order.
    pub fn simulation(&self) -> Result<ZoneSimulation, ZoneError> {
        self.check()?;
        let mut spawners: Vec<CookedSpawner> = self
            .spawners
            .iter()
            .map(|spawner| CookedSpawner {
                id: Name::new(&spawner.name).as_u64(),
                name: spawner.name.clone(),
                position: spawner.position,
                yaw_degrees: spawner.yaw_degrees,
                spawns: spawner.spawns.clone(),
                patrol: spawner.patrol.as_deref().map(|patrol| Name::new(patrol).as_u64()),
            })
            .collect();
        spawners.sort_unstable_by_key(|spawner| spawner.id);
        let mut patrols: Vec<CookedPatrol> = self
            .patrols()
            .map(|(_, spline)| CookedPatrol {
                id: Name::new(&spline.name).as_u64(),
                name: spline.name.clone(),
                points: spline.points.clone(),
                closed: spline.closed,
            })
            .collect();
        patrols.sort_unstable_by_key(|patrol| patrol.id);
        let mut triggers: Vec<CookedTrigger> = self
            .triggers
            .iter()
            .map(|trigger| CookedTrigger {
                id: Name::new(&trigger.name).as_u64(),
                name: trigger.name.clone(),
                position: trigger.position,
                volume: trigger.volume.cooked(),
                event: trigger.event.clone(),
            })
            .collect();
        triggers.sort_unstable_by_key(|trigger| trigger.id);
        let mut checkpoints: Vec<CookedCheckpoint> = self
            .checkpoints
            .iter()
            .map(|checkpoint| CookedCheckpoint {
                id: Name::new(&checkpoint.name).as_u64(),
                name: checkpoint.name.clone(),
                position: checkpoint.position,
                yaw_degrees: checkpoint.yaw_degrees,
                volume: checkpoint.volume.cooked(),
            })
            .collect();
        checkpoints.sort_unstable_by_key(|checkpoint| checkpoint.id);
        check_distinct("spawners", &spawners, |spawner| (spawner.id, &spawner.name))?;
        check_distinct("triggers", &triggers, |trigger| (trigger.id, &trigger.name))?;
        check_distinct("checkpoints", &checkpoints, |checkpoint| (checkpoint.id, &checkpoint.name))?;
        let mut exits: Vec<CookedExit> = self
            .exits
            .iter()
            .map(|exit| CookedExit {
                id: Name::new(&exit.name).as_u64(),
                name: exit.name.clone(),
                to: exit.to,
                pack: String::new(),
                arrive: Name::new(&exit.arrive).as_u64(),
                prefetch: exit.prefetch,
            })
            .collect();
        exits.sort_unstable_by_key(|exit| exit.id);
        check_distinct("exits", &exits, |exit| (exit.id, &exit.name))?;
        check_distinct("patrols", &patrols, |patrol| (patrol.id, &patrol.name))?;
        let sorted_set = |ids: &[AssetId]| ids.iter().copied().collect::<BTreeSet<_>>().into_iter().collect();
        Ok(ZoneSimulation {
            colliders: self.colliders,
            spawners,
            patrols,
            triggers,
            checkpoints,
            navigation: None,
            encounters: sorted_set(&self.encounters),
            exits,
        })
    }

    /// The cooked zone, from what a cook made of its geometry.
    pub fn cook(&self, geometry: ZoneGeometry) -> Result<Zone, ZoneError> {
        let mut simulation = self.simulation()?;
        if geometry.navigation.is_some() != self.navigation.is_some() {
            let problem = format!(
                "the document asks for {} and the cook made {}",
                if self.navigation.is_some() { "a navmesh" } else { "none" },
                if geometry.navigation.is_some() { "one" } else { "none" },
            );
            return Err(ZoneError::invalid("navigation", problem));
        }
        simulation.navigation = geometry.navigation;
        if geometry.exit_packs.len() != self.exits.len() {
            let problem = format!("{} packs for {} exits", geometry.exit_packs.len(), self.exits.len());
            return Err(ZoneError::invalid("exits", problem));
        }
        for (exit, pack) in self.exits.iter().zip(geometry.exit_packs) {
            let id = Name::new(&exit.name).as_u64();
            let cooked = simulation.exits.iter_mut().find(|cooked| cooked.id == id);
            cooked.expect("cooked from these exits").pack = pack;
        }
        if geometry.instances.len() != self.instances.len() {
            return Err(ZoneError::invalid(
                "instances",
                format!("{} bounds for {} instances", geometry.instances.len(), self.instances.len()),
            ));
        }

        // Each place's instances and lights, in the document's order.
        let mut instances: BTreeMap<Place, Vec<usize>> = BTreeMap::new();
        for (index, instance) in self.instances.iter().enumerate() {
            let place = self.place_of(instance.at, instance.backdrop);
            instances.entry(place).or_default().push(index);
        }
        let mut lights: BTreeMap<Place, Vec<usize>> = BTreeMap::new();
        for (index, light) in self.lights.iter().enumerate() {
            let place = self.place_of(light.position, light.backdrop);
            lights.entry(place).or_default().push(index);
        }
        let places: BTreeSet<Place> =
            geometry.meshes.keys().chain(instances.keys()).chain(lights.keys()).copied().collect();

        let mut presentation = ZonePresentation {
            cell_size: self.grid.cell_size,
            backdrop: None,
            cells: Vec::with_capacity(places.len()),
            instances: Vec::with_capacity(self.instances.len()),
            lights: Vec::with_capacity(self.lights.len()),
            probes: self.probes,
            day: geometry.day,
            rigs: geometry.rigs,
            splines: self.cooked_splines()?,
        };
        // In `Place`'s order: the backdrop's entries, then each cell's.
        for place in places {
            let mesh = geometry.meshes.get(&place).copied();
            let mut bounds: Option<Bounds> = None;
            let mut grow = |more: Bounds, field: String| {
                if !more.is_valid() {
                    return Err(ZoneError::invalid(
                        field,
                        format!("reaches {more:?}, past {MAX_COORDINATE} m"),
                    ));
                }
                if let Place::Cell(cell) = place
                    && let Some(reach) = self.grid.overreach(cell, more)
                {
                    return Err(ZoneError::invalid(
                        field,
                        format!(
                            "reaches {reach} m past its cell ({}, {}), more than a cell's {} m: put it in the backdrop",
                            cell.x, cell.z, self.grid.cell_size
                        ),
                    ));
                }
                bounds = Some(bounds.map_or(more, |bounds| bounds.union(more)));
                Ok(())
            };
            if let Some((_, around)) = mesh {
                grow(around, "placements".to_owned())?;
            }
            let first_instance = presentation.instances.len() as u32;
            for &index in instances.get(&place).map_or(&[][..], Vec::as_slice) {
                grow(geometry.instances[index], format!("instances[{index}]"))?;
                presentation.instances.push(self.instances[index].cooked());
            }
            let first_light = presentation.lights.len() as u32;
            for &index in lights.get(&place).map_or(&[][..], Vec::as_slice) {
                let light = &self.lights[index];
                let reach =
                    |axis: usize| [light.position[axis] - light.range, light.position[axis] + light.range];
                let sphere = Bounds {
                    min: [reach(0)[0], reach(1)[0], reach(2)[0]],
                    max: [reach(0)[1], reach(1)[1], reach(2)[1]],
                };
                grow(sphere, format!("lights[{index}]"))?;
                presentation.lights.push(light.cooked());
            }
            let bounds = bounds.expect("a place has a mesh, an instance or a light");
            let contents = PlaceContents {
                mesh: mesh.map(|(id, _)| id),
                bounds_min: bounds.min,
                bounds_max: bounds.max,
                first_instance,
                instance_count: presentation.instances.len() as u32 - first_instance,
                first_light,
                light_count: presentation.lights.len() as u32 - first_light,
            };
            match place {
                Place::Backdrop => presentation.backdrop = Some(contents),
                Place::Cell(cell) => presentation.cells.push(ZoneCell { coord: [cell.x, cell.z], contents }),
            }
        }
        Ok(Zone { simulation, presentation })
    }

    fn cooked_splines(&self) -> Result<Vec<CookedSpline>, ZoneError> {
        let mut splines: Vec<CookedSpline> = self
            .splines
            .iter()
            .map(|spline| CookedSpline {
                id: Name::new(&spline.name).as_u64(),
                name: spline.name.clone(),
                points: spline.points.clone(),
                closed: spline.closed,
            })
            .collect();
        splines.sort_unstable_by_key(|spline| spline.id);
        check_distinct("splines", &splines, |spline| (spline.id, &spline.name))?;
        Ok(splines)
    }
}

impl Volume {
    fn cooked(self) -> CookedVolume {
        match self {
            Self::Box { half_extents } => CookedVolume::Box { half_extents },
            Self::Sphere { radius } => CookedVolume::Sphere { radius },
        }
    }
}

impl Placement {
    /// What a cook calls the mesh of this placement's prefab on its own, which an editor draws it with
    /// (M3 E5): `prefab/<hash>`, by its prefab, the materials it swaps and the axes its scale mirrors, so
    /// placements alike in those share one, wherever they stand, however turned and however large. The
    /// mesh is the prefab mirrored as [`Self::mirror`] says, and drawn at [`Self::shape_transform`].
    #[must_use]
    pub fn shape_name(&self) -> String {
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        hasher.update(&self.prefab.to_bytes());
        for (slot, material) in &self.materials {
            hasher.update(&(slot.len() as u64).to_le_bytes());
            hasher.update(slot.as_bytes());
            hasher.update(&material.to_bytes());
        }
        hasher.update(&self.mirror().map(|sign| u8::from(sign < 0.0)));
        format!("prefab/{:016x}", hasher.digest())
    }

    /// The sign of its scale on each axis: what of its transform a mesh of its shape holds, since a mirror turns
    /// triangles inside out and a renderer draws only one side.
    #[must_use]
    pub fn mirror(&self) -> [f32; 3] {
        self.scale.map(|scale| if scale < 0.0 { -1.0 } else { 1.0 })
    }

    /// The rest of its transform, `at · turn · |scale|`, which a mesh of its shape is drawn at.
    #[must_use]
    pub fn shape_transform(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::from(self.scale.map(f32::abs)),
            det::quat_from_axis_angle(Vec3::Y, self.turn.to_radians()),
            Vec3::from(self.at),
        )
    }
}

impl Instance {
    fn cooked(&self) -> CookedInstance {
        let turn = det::quat_from_axis_angle(Vec3::Y, self.turn.to_radians());
        CookedInstance { mesh: self.mesh, position: self.at, rotation: turn.to_array(), scale: self.scale }
    }
}

impl Light {
    fn cooked(&self) -> CookedLight {
        CookedLight {
            position: self.position,
            color: self.color,
            intensity: self.intensity,
            range: self.range,
            shape: match self.shape {
                LightShape::Point => CookedLightShape::Point,
                LightShape::Spot { direction, inner_degrees, outer_degrees } => CookedLightShape::Spot {
                    direction,
                    inner_angle: inner_degrees.to_radians(),
                    outer_angle: outer_degrees.to_radians(),
                },
            },
            at_night: self.at_night,
        }
    }
}

// These checks read plain values, so the document's and the archive's run the same ones.

/// Whether a value is finite, or what's wrong with it.
pub(crate) fn check_finite(value: f32) -> Result<(), String> {
    if value.is_finite() { Ok(()) } else { Err(format!("{value} isn't finite")) }
}

/// Whether a value is finite and above zero, or what's wrong with it.
pub(crate) fn check_positive(value: f32) -> Result<(), String> {
    if value.is_finite() && value > 0.0 { Ok(()) } else { Err(format!("{value} isn't positive")) }
}

/// Whether a position lies within [`MAX_COORDINATE`], or what's wrong with it.
pub(crate) fn check_position(position: [f32; 3]) -> Result<(), String> {
    if !position.iter().all(|v| v.is_finite()) {
        return Err(format!("{position:?} isn't finite"));
    }
    if position.iter().any(|v| v.abs() > MAX_COORDINATE) {
        return Err(format!("{position:?} lies past {MAX_COORDINATE} m"));
    }
    Ok(())
}

/// Whether a scale is finite with no zero axis, or what's wrong with it. A negative axis mirrors.
pub(crate) fn check_scale(scale: [f32; 3]) -> Result<(), String> {
    if scale.iter().all(|v| v.is_finite() && *v != 0.0) {
        Ok(())
    } else {
        Err(format!("{scale:?} must be finite and nonzero on every axis"))
    }
}

/// Whether a light's color can light anything, or what's wrong with it.
pub(crate) fn check_color(color: [f32; 3]) -> Result<(), String> {
    if color.iter().all(|v| v.is_finite() && *v >= 0.0) && color.iter().any(|v| *v > 0.0) {
        Ok(())
    } else {
        Err(format!("{color:?} must be finite, not negative and not black"))
    }
}

/// Whether a spot's cone is one the renderer takes, or what's wrong with it. Angles are in radians.
pub(crate) fn check_spot(direction: [f32; 3], inner: f32, outer: f32) -> Result<(), String> {
    let length = direction.iter().map(|v| v * v).sum::<f32>();
    if !length.is_finite() || length == 0.0 {
        return Err(format!("direction {direction:?} isn't a direction"));
    }
    let half_turn = std::f32::consts::PI;
    if !(inner.is_finite() && outer.is_finite() && 0.0 <= inner && inner <= outer && outer <= half_turn) {
        return Err(format!("half angles {inner} and {outer} rad must rise from 0 to at most a half turn"));
    }
    Ok(())
}

/// Whether a name or a named event is one a list may hold, or what's wrong with it.
pub(crate) fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("it's empty".to_owned());
    }
    if name.len() > MAX_NAME {
        return Err(format!("{} bytes is longer than the {MAX_NAME} a name may take", name.len()));
    }
    Ok(())
}

/// Whether a path to a document is relative with a file at its end, or what's wrong with it.
fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("it's empty".to_owned());
    }
    if path.starts_with(['/', '\\']) || path.contains(':') {
        return Err(format!("{path:?} isn't a path from the document's directory"));
    }
    Ok(())
}

/// Whether a spline's points make one, or what's wrong with them.
pub(crate) fn check_points(count: usize, point: impl Fn(usize) -> [f32; 3]) -> Result<(), String> {
    if !(2..=MAX_SPLINE_POINTS).contains(&count) {
        return Err(format!("{count} points isn't from 2 to {MAX_SPLINE_POINTS}"));
    }
    (0..count).try_for_each(|index| check_position(point(index)))
}

/// Whether sorted entries' IDs differ: two names can hash to one, however unlikely, and an ID is an
/// identity.
fn check_distinct<'a, T>(
    list: &str,
    sorted: &'a [T],
    id_and_name: impl Fn(&'a T) -> (u64, &'a String),
) -> Result<(), ZoneError> {
    for pair in sorted.windows(2) {
        let [(first, first_name), (second, second_name)] = [id_and_name(&pair[0]), id_and_name(&pair[1])];
        if first == second {
            let problem = format!("`{first_name}` and `{second_name}` hash to one ID: rename one");
            return Err(ZoneError::invalid(list, problem));
        }
    }
    Ok(())
}

/// Whether a list's names are each good and unique, naming the first that isn't.
fn check_names<'a>(list: &str, names: impl Iterator<Item = &'a str>) -> Result<(), ZoneError> {
    let mut seen = BTreeSet::new();
    for (index, name) in names.enumerate() {
        check_name(name).map_err(|problem| ZoneError::invalid(format!("{list}[{index}].name"), problem))?;
        if !seen.insert(name) {
            let problem = format!("`{name}` is the name of an earlier entry, and names are identities");
            return Err(ZoneError::invalid(format!("{list}[{index}].name"), problem));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bevy_reflect::{GetPath, TypeRegistry};
    use sapphire_serialize::ReflectDocument;

    use super::*;

    const PREFAB: &str = "1ff3290e-5fa9-4f61-b891-2ca6d76b285f";

    fn parse(data: &str) -> Result<ZoneDocument, ZoneError> {
        ZoneDocument::from_ron(&format!(
            r#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 32.0), {data}))"#
        ))
    }

    /// Every field of the crate's example, which the module docs repeat.
    fn full() -> ZoneDocument {
        parse(&format!(
            r#"
            placements: [
                (prefab: "{PREFAB}", at: (4.0, 0.0, -9.0), turn: 30.0, colliders: ["crate_0"]),
                (prefab: "{PREFAB}", at: (40.0, 0.0, -9.0), scale: (1.2, 1.2, 1.2), materials: {{"timber": "{PREFAB}"}}),
                (prefab: "{PREFAB}", backdrop: true),
                (prefab: "{PREFAB}", split: Some((backdrop: ["kenney/rock-*"]))),
            ],
            instances: [(mesh: "{PREFAB}", at: (-15.6, 0.0, -15.4), turn: 20.0, colliders: ["tree_0"])],
            lights: [
                (position: (0.3, 2.4, -35.4), color: (1.0, 0.7, 0.4), intensity: 6.0, range: 9.0, at_night: true),
                (
                    position: (0.0, 3.0, 0.0), color: (1.0, 1.0, 1.0), intensity: 1.0, range: 4.0,
                    shape: Spot(direction: (0.0, -1.0, 0.0), inner_degrees: 20.0, outer_degrees: 30.0),
                ),
            ],
            probes: Some("{PREFAB}"),
            day: Some("../lookdev/lookdev_day.ron"),
            rigs: Some("../lookdev/lookdev_light_rigs.ron"),
            colliders: Some("{PREFAB}"),
            spawners: [(name: "player_start", position: (0.0, 0.0, 3.0), yaw_degrees: 180.0, spawns: "player")],
            triggers: [
                (name: "gate", position: (3.2, 1.0, -18.0), volume: Box(half_extents: (3.0, 2.0, 1.0)), event: "gate_entered"),
                (name: "well", position: (0.0, 0.0, 0.0), volume: Sphere(radius: 2.0), event: "well_entered"),
            ],
            checkpoints: [(name: "gate_steps", position: (3.2, 0.0, -14.0), yaw_degrees: 180.0, volume: Sphere(radius: 2.0))],
            splines: [(name: "street_rail", points: [(3.2, 2.0, -20.0), (3.2, 2.5, -40.0)], closed: true)],
            "#
        ))
        .expect("a good document")
    }

    /// A shape is its prefab, its materials and its mirror: where it stands, its turn and its size are the
    /// transform it's drawn at, and the two together are the placement's whole transform.
    #[test]
    fn a_shape_leaves_out_what_its_transform_holds() {
        let document = full();
        let [plain, swapped, ..] = &document.placements[..] else { unreachable!() };
        let moved = Placement { at: [1.0, 2.0, 3.0], turn: 75.0, scale: [2.0, 0.5, 3.0], ..plain.clone() };
        assert_eq!(moved.shape_name(), plain.shape_name());
        assert_ne!(swapped.shape_name(), plain.shape_name(), "it swaps a material");
        let mirrored = Placement { scale: [-2.0, 0.5, 3.0], ..moved.clone() };
        assert_ne!(mirrored.shape_name(), plain.shape_name());
        assert!(plain.shape_name().starts_with("prefab/"));

        let whole = Mat4::from_scale_rotation_translation(
            Vec3::from(mirrored.scale),
            det::quat_from_axis_angle(Vec3::Y, mirrored.turn.to_radians()),
            Vec3::from(mirrored.at),
        );
        let split = mirrored.shape_transform() * Mat4::from_scale(Vec3::from(mirrored.mirror()));
        assert!(split.abs_diff_eq(whole, 1e-5), "{split:?} against {whole:?}");
        assert!(document.has_split());
    }

    #[test]
    fn a_document_round_trips_through_its_text() {
        let document = full();
        let text = sapphire_serialize::to_ron(&document).expect("write");
        assert_eq!(ZoneDocument::from_ron(&text).expect("read"), document);
    }

    /// What the editor's `RonPatch` leans on: a path into a typed document, and the registry's road.
    #[test]
    fn a_document_is_reflected_down_to_its_values() {
        let mut document = full();
        assert_eq!(*document.path::<f32>("placements[1].at[0]").expect("a path"), 40.0);
        assert_eq!(*document.path::<f32>("spawners[0].yaw_degrees").expect("a path"), 180.0);
        *document.path_mut::<f32>("placements[0].turn").expect("a path") = 45.0;
        assert_eq!(document.placements[0].turn, 45.0);

        let mut registry = TypeRegistry::default();
        registry.register::<ZoneDocument>();
        registry.register_type_data::<ZoneDocument, ReflectDocument>();
        let text = sapphire_serialize::to_ron(&document).expect("write");
        let dynamic = sapphire_serialize::from_ron_dynamic(&registry, &text).expect("by its type's name");
        assert_eq!(dynamic.downcast_ref::<ZoneDocument>(), Some(&document));
    }

    #[test]
    fn an_unknown_field_is_an_error_that_names_it() {
        let message = parse("placements: [], spawnerz: []").unwrap_err().to_string();
        assert!(message.contains("spawnerz"), "{message}");
        let prefab = format!(r#"placements: [(prefab: "{PREFAB}", att: (0.0, 0.0, 0.0))]"#);
        let message = parse(&prefab).unwrap_err().to_string();
        assert!(message.contains("att"), "{message}");
    }

    #[test]
    fn every_check_names_the_field() {
        type Tamper = fn(&mut ZoneDocument);
        let cases: [(&str, Tamper); 31] = [
            ("grid.cell_size", |d| d.grid.cell_size = 1.0),
            ("grid.cell_size", |d| d.grid.cell_size = f32::NAN),
            ("placements[1].at", |d| d.placements[1].at[2] = f32::INFINITY),
            ("placements[0].at", |d| d.placements[0].at[0] = 20_000.0),
            ("placements[0].turn", |d| d.placements[0].turn = f32::NAN),
            ("placements[1].scale", |d| d.placements[1].scale[1] = 0.0),
            ("placements[3].split", |d| d.placements[3].backdrop = true),
            ("placements[0].colliders[1]", |d| d.placements[0].colliders = vec!["post".to_owned(); 2]),
            ("placements[0].colliders", |d| (d.colliders, d.navigation) = (None, None)),
            ("instances[0].colliders[0]", |d| d.instances[0].colliders = vec![String::new()]),
            ("instances[0].at", |d| d.instances[0].at[1] = f32::NAN),
            ("instances[0].scale", |d| d.instances[0].scale = [0.0; 3]),
            ("lights[0].color", |d| d.lights[0].color = [0.0; 3]),
            ("lights[0].range", |d| d.lights[0].range = 0.0),
            ("lights[1].shape", |d| {
                d.lights[1].shape =
                    LightShape::Spot { direction: [0.0; 3], inner_degrees: 1.0, outer_degrees: 2.0 };
            }),
            ("lights[1].shape", |d| {
                d.lights[1].shape =
                    LightShape::Spot { direction: [1.0; 3], inner_degrees: 40.0, outer_degrees: 20.0 };
            }),
            ("day", |d| d.day = Some("/etc/day.ron".to_owned())),
            ("spawners[0].name", |d| d.spawners[0].name.clear()),
            ("spawners[0].spawns", |d| d.spawners[0].spawns = "x".repeat(MAX_NAME + 1)),
            ("spawners[0].yaw_degrees", |d| d.spawners[0].yaw_degrees = f32::NAN),
            ("spawners[0].patrol", |d| d.spawners[0].patrol = Some("no_such_spline".to_owned())),
            ("spawners[0].patrol", |d| {
                d.navigation = None;
                d.spawners[0].patrol = Some("street_rail".to_owned());
            }),
            ("triggers[1].name", |d| d.triggers[1].name = "gate".to_owned()),
            ("triggers[1].volume", |d| d.triggers[1].volume = Volume::Sphere { radius: 0.0 }),
            ("triggers[0].event", |d| d.triggers[0].event.clear()),
            ("checkpoints[0].name", |d| d.checkpoints[0].name.clear()),
            ("checkpoints[0].position", |d| d.checkpoints[0].position[2] = f32::INFINITY),
            ("checkpoints[0].yaw_degrees", |d| d.checkpoints[0].yaw_degrees = f32::NAN),
            ("checkpoints[0].volume", |d| {
                d.checkpoints[0].volume = Volume::Box { half_extents: [1.0, 0.0, 1.0] }
            }),
            ("checkpoints[1].name", |d| d.checkpoints.push(d.checkpoints[0].clone())),
            ("splines[0].points", |d| d.splines[0].points.truncate(1)),
        ];
        for (field, tamper) in cases {
            let mut document = full();
            tamper(&mut document);
            let message = document.check().expect_err(field).to_string();
            assert!(message.contains(&format!("`{field}`")), "`{message}` must name `{field}`");
        }
    }

    #[test]
    fn a_stable_id_is_its_names_hash_whatever_the_order_or_company() {
        let simulation = full().simulation().expect("cooks");
        let id = Name::new("gate").as_u64();
        assert!(simulation.triggers.iter().any(|trigger| trigger.id == id));

        let mut reordered = full();
        reordered.triggers.reverse();
        reordered.triggers.push(Trigger { name: "new".to_owned(), ..reordered.triggers[0].clone() });
        let more = reordered.simulation().expect("cooks");
        let of = |simulation: &ZoneSimulation, name: &str| {
            simulation.triggers.iter().find(|trigger| trigger.name == name).expect("there").id
        };
        assert_eq!(of(&more, "gate"), of(&simulation, "gate"));
        assert_eq!(of(&more, "well"), of(&simulation, "well"));
    }

    #[test]
    fn an_exit_names_itself_and_where_it_arrives() {
        let exit = |name: &str, arrive: &str| Exit {
            name: name.to_owned(),
            to: PREFAB.parse().expect("an ID"),
            arrive: arrive.to_owned(),
            prefetch: false,
        };
        let mut document = full();
        document.exits = vec![exit("to_market", "player_start"), exit("to_docks", "player_start")];
        document.check().expect("good");

        let text = sapphire_serialize::to_ron(&document).expect("write");
        assert!(!text.contains("prefetch"), "left out unless asked: {text}");
        assert_eq!(ZoneDocument::from_ron(&text).expect("read"), document);

        document.exits[1].name = "to_market".to_owned();
        let message = document.check().expect_err("twice").to_string();
        assert!(message.contains("exits"), "{message}");

        document.exits[1] = exit("to_docks", "");
        let message = document.check().expect_err("nowhere").to_string();
        assert!(message.contains("exits[1].arrive"), "{message}");

        document.exits[1] = exit("to_docks", "player_start");
        let geometry = ZoneGeometry { exit_packs: vec!["zone_a".to_owned()], ..ZoneGeometry::default() };
        document.placements.clear();
        document.lights.clear();
        document.instances.clear();
        document.colliders = None;
        let message = document.cook(geometry).expect_err("short a pack").to_string();
        assert!(message.contains("1 packs for 2 exits"), "{message}");
    }

    #[test]
    fn what_stands_in_a_cell_stays_near_it_or_goes_in_the_backdrop() {
        let mut document = full();
        document.lights.clear();
        let cell = document.place_of(document.instances[0].at, false);
        let wide = Bounds { min: [-100.0, 0.0, -30.0], max: [0.0, 10.0, 0.0] };
        let geometry = ZoneGeometry { instances: vec![wide], ..ZoneGeometry::default() };
        let message = document.cook(geometry.clone()).expect_err("too wide").to_string();
        assert!(message.contains("`instances[0]`") && message.contains("backdrop"), "{message}");
        assert!(matches!(cell, Place::Cell(_)));

        document.instances[0].backdrop = true;
        let zone = document.cook(geometry).expect("the backdrop may be any size");
        assert!(zone.presentation.cells.is_empty());
        let backdrop = zone.presentation.backdrop.expect("holds it");
        assert_eq!((backdrop.first_instance, backdrop.instance_count), (0, 1));
    }

    #[test]
    fn what_a_document_gets_wrong_about_navigation_is_named() {
        let spawner = r#"spawners: [(name: "start", position: (0.0, 0.0, 0.0), spawns: "player")],"#;
        let colliders = format!(r#"colliders: Some("{PREFAB}"),"#);
        let area = "min: (-8.0, -8.0), max: (8.0, 8.0)";
        parse(&format!("{colliders} {spawner} navigation: Some(({area})),")).expect("a good one");
        parse(&format!("{colliders} navigation: Some(({area}, seeds: [(1.0, 0.0, 1.0)])),"))
            .expect("a seed will do");
        let cases = [
            (format!("{spawner} navigation: Some(({area})),"), "`navigation`"),
            (format!("{colliders} navigation: Some(({area})),"), "`navigation`"),
            (
                format!("{colliders} {spawner} navigation: Some((min: (8.0, -8.0), max: (8.0, 8.0))),"),
                "`navigation.min`",
            ),
            (
                format!("{colliders} {spawner} navigation: Some((min: (-8.0, 9.0), max: (8.0, 8.0))),"),
                "`navigation.min`",
            ),
            (
                format!("{colliders} {spawner} navigation: Some((min: (-1e9, -8.0), max: (8.0, 8.0))),"),
                "`navigation.min`",
            ),
            (
                format!("{colliders} {spawner} navigation: Some((min: (-8.0, -8.0), max: (8.0, inf))),"),
                "`navigation.max`",
            ),
            (
                format!("{colliders} {spawner} navigation: Some(({area}, seeds: [(0.0, NaN, 0.0)])),"),
                "`navigation.seeds[0]`",
            ),
        ];
        for (data, expected) in cases {
            let message = parse(&data).expect_err(&data).to_string();
            assert!(message.contains(expected), "{data}: `{message}` must say `{expected}`");
        }
    }

    #[test]
    fn a_navmesh_is_there_if_and_only_if_the_document_asks() {
        let mut document = full();
        document.placements.clear();
        document.lights.clear();
        document.instances.clear();
        document.navigation = None;
        let id: AssetId = PREFAB.parse().expect("an ID");
        let with = ZoneGeometry { navigation: Some(id), ..ZoneGeometry::default() };
        assert!(document.cook(with.clone()).is_err(), "made one nobody asked for");
        assert_eq!(document.cook(ZoneGeometry::default()).expect("cooks").simulation.navigation, None);

        document.colliders = Some(id);
        document.navigation = Some(ZoneNavigation { min: [-8.0; 2], max: [8.0; 2], seeds: vec![[0.0; 3]] });
        assert!(document.cook(ZoneGeometry::default()).is_err(), "asked for one and none came");
        assert_eq!(document.cook(with).expect("cooks").simulation.navigation, Some(id));
    }
}
