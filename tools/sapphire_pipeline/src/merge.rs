//! Material merge (M5 L15): a character's same-part materials atlased in the cook, so the character
//! draws fewer ranges (spec §3.4's 6 materials a character on desktop and 4 on mobile).
//!
//! # The sidecar
//!
//! A mesh's sidecar names the slots each group merges (`MeshSettings::merge`), and on which cooks:
//!
//! ```ron
//! merge: [(name: "body", slots: ["skin", "top", "gear"], cooks: Every)],
//! ```
//!
//! `cooks` is `Every` (the default), `Desktop` or `Mobile`. A group the cook leaves out plans and
//! changes nothing there. Without the section a mesh cooks byte for byte as it did before it.
//!
//! # What merges
//!
//! Every member is bound to an anime character material document whose parameters are the others'
//! apart from the textures its UVs address (the base color, control, normal and layer maps). Its
//! regions are a prefix of the member with the most regions (the *standing* member, the first of
//! equals), whose document the merged material is; the cook proves each member's control map (or,
//! without one, its vertices) names only its own regions, so no texel reaches a region its own
//! material didn't have. Its ramp and matcap are the standing member's asset or bake to the same
//! texels. Everything else is refused, slot by slot, with its reason ([`Reason`]): the face, eyes and
//! hair (their shading follows their UVs and their own maps); a blended or see-through surface;
//! stockings, whose weave is laid out in UV units; a parameter that differs, by name; maps of other
//! sizes, settings or channels; a member whose UVs leave 0 to 1, whose vertices another slot shares,
//! or whose slot an expression binds (its color or UV offset). A refused group fails the cook, naming
//! every refused slot: the cook reports a target the look can't reach rather than forcing it.
//!
//! # The atlas
//!
//! One atlas a map: the members' maps, each a square tile of the same size, in a square grid of
//! cells (`ceil(√n)` a side, so the dissolve's noise, laid out in UV units, keeps its cells' size and
//! shape under one scale). Each tile keeps its texel density (nothing is downscaled) and is padded by
//! its own content wrapped around, as the material sampler's repeat would read it. The padding and the
//! tile's place are multiples of each map's block size times `2^(EXACT_LEVELS + d)`, `d` the levels
//! the cook's size cap drops (their least common multiple, where maps' blocks differ, as a phone's 6x6
//! colour beside a 4x4 control map): so at every level from the stored top down to [`EXACT_LEVELS`] below it a
//! tile and its padding are whole blocks, and those levels are assembled from each member's own cooked
//! chain, block by block: the tile's blocks are its own map's and its padding's its own edge blocks
//! wrapped around, on every machine, whatever an encoder makes of a block's surroundings (x86-64's BC7
//! picks its preset from the whole image). Bilinear reads at a tile's edge find what the repeat would
//! have. Below that, and in the empty cells, levels come from the composed image and mix neighbours.
//! The atlas stays within the cook's limit ([`atlas_limit`]) once its cap is met, or the group is
//! refused. Atlases cook whole: none splits levels above a tail for the streamer.
//!
//! # Names
//!
//! Names are resolved at cook time, and the cooked mesh's format doesn't change: the merged slot
//! takes the first member's place and the group's name, the members' triangles join into its range in
//! each LOD, and the slots after it close up. The rig's expression binds, which address slots by
//! index, are remapped by name in the same cook ([`remap_rig`]); a bind on a member refuses the group.
//! The merged material and atlases are sub-assets of the mesh (`merge/<group>` and
//! `merge/<group>/<map>`), cooked beside it (`hero.merge.body.smat`, `hero.merge.body.control.ktx2`).

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sapphire_assets::AssetId;
use sapphire_assets::character_maps::ilm;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, AnyMaterialDocument, CharacterSurface, DiffuseMode,
};
use sapphire_assets::rig::CharacterRig;
use sapphire_assets::texture::MAX_TEXTURE_DIMENSION;
use serde::{Deserialize, Serialize};

use crate::cache::{Dependency, Stage};
use crate::character_maps::{self, MapError, MapSettings};
use crate::cook::Cook;
use crate::import::image::{self, ChannelLayout, SourceFormat, SourceImage};
use crate::material::{self, MaterialCookError, TextureKind};
use crate::mesh::ImportedMesh;
use crate::meta::{self, ImportSettings, MeshSettings, SourceAsset};
use crate::texture::{self, CookedTexture, MaxSize, TextureClass, TextureError, TextureSettings};

#[cfg(test)]
mod tests;

/// Composing an atlas from its members' maps, as cache keys know it. Bump the version whenever its
/// output changes for the same maps. The members' own importers (images, ILM and layer documents) are
/// in its keys as dependencies, by name and version.
pub const ATLAS_IMPORTER: Stage =
    Stage { name: "merge atlas", version: character_maps::with_image_importer(1) };

/// Making the merged material's document from its members', as cache keys know it.
pub const MATERIAL_IMPORTER: Stage = Stage { name: "merge material", version: 1 };

/// How many levels below an atlas's stored top hold each tile's own texels exactly: the padding and a
/// tile's place are aligned to the block size times `2^EXACT_LEVELS` (and the levels the cap drops).
/// 3 keeps a 256 tile exact down to 32 × 32, a few metres off on a phone.
pub const EXACT_LEVELS: u32 = 3;

/// The longest side an atlas may store on `cook`, once its cap is met: what every GPU of the cook's
/// tier samples (desktop GPUs take 16384, but a merge that needs more than 8192 is a sign of maps too
/// large for one part; Vulkan's floor, and older phones', is 4096).
pub fn atlas_limit(cook: Cook) -> u32 {
    match cook {
        Cook::Desktop => 8192,
        Cook::Mobile => 4096,
    }
}

/// A group of a mesh's slots to merge into one (the sidecar's `merge`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeGroup {
    /// The merged slot's name: letters, digits, `_` and `-`, unique among the mesh's slots and groups.
    pub name: String,
    /// Its members, by slot name: two or more, each in one group.
    pub slots: Vec<String>,
    /// The cooks that merge it. Always written: cache keys encode settings with postcard, which doesn't
    /// name fields, so leaving it out would key two lists of groups alike.
    #[serde(default)]
    pub cooks: Cooks,
}

/// The cooks a merge group merges on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cooks {
    #[default]
    Every,
    Desktop,
    Mobile,
}

impl Cooks {
    /// Whether `cook` merges.
    pub fn includes(self, cook: Cook) -> bool {
        match self {
            Self::Every => true,
            Self::Desktop => cook == Cook::Desktop,
            Self::Mobile => cook == Cook::Mobile,
        }
    }
}

/// Whether `settings` merge anything on `cook`.
pub fn merges(settings: &MeshSettings, cook: Cook) -> bool {
    settings.merge.iter().any(|group| group.cooks.includes(cook))
}

/// A map an atlas holds: the anime character material's maps its UVs address on a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AtlasMap {
    BaseColor,
    Control,
    Normal,
    /// A body's layer mask.
    Mask,
}

impl AtlasMap {
    pub const ALL: [Self; 4] = [Self::BaseColor, Self::Control, Self::Normal, Self::Mask];

    /// Its name in IDs, file names and messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::BaseColor => "base_color",
            Self::Control => "control",
            Self::Normal => "normal",
            Self::Mask => "mask",
        }
    }

    /// The map `material` binds here.
    pub fn get(self, material: &AnimeCharacterMaterial) -> Option<AssetId> {
        match self {
            Self::BaseColor => material.base_color_map,
            Self::Control => material.control_map,
            Self::Normal => material.normal_map,
            Self::Mask => material.layer_mask,
        }
    }

    fn set(self, material: &mut AnimeCharacterMaterial, id: AssetId) {
        let slot = match self {
            Self::BaseColor => &mut material.base_color_map,
            Self::Control => &mut material.control_map,
            Self::Normal => &mut material.normal_map,
            Self::Mask => &mut material.layer_mask,
        };
        *slot = Some(id);
    }
}

/// The merged material's ID: the mesh's sub-asset `merge/<group>`.
pub fn material_id(mesh: AssetId, group: &str) -> AssetId {
    meta::sub_asset_id(mesh, &format!("merge/{group}"))
}

/// An atlas's ID: the mesh's sub-asset `merge/<group>/<map>`.
pub fn atlas_id(mesh: AssetId, group: &str, map: AtlasMap) -> AssetId {
    meta::sub_asset_id(mesh, &format!("merge/{group}/{}", map.name()))
}

/// Why a slot can't join its group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The sidecar binds it to no material document: its material is generated.
    Unbound,
    /// What it's bound to isn't a material document.
    NotAMaterial,
    /// Its document doesn't read: the message says why.
    Document(String),
    /// Its document isn't of the anime character family.
    NotAnimeCharacter,
    /// It's a face: its shadow follows its face map, mirrored about its UVs.
    Face,
    /// It's an eye: its iris and parallax are laid out in its UVs, and it draws by stencil.
    Eye,
    /// It's hair: its strands run along its UVs.
    Hair,
    /// It blends over what's behind it.
    Blended,
    /// One of its regions draws through the proximity dither (M5 L11).
    SeeThrough { region: usize },
    /// One of its regions wears stockings, whose weave is laid out in UV units.
    Stockings { region: usize },
    /// A parameter differs from the standing member's: no parameter is reasoned dead.
    Differs { field: &'static str },
    /// Its regions aren't a prefix of the standing member's.
    Regions,
    /// A map it binds isn't in the content tree.
    MissingMap { map: &'static str },
    /// A map it binds is generated by a procedural document, which an atlas doesn't read.
    Procedural { map: &'static str },
    /// A map's importer settings differ from the standing member's.
    MapSettings { map: &'static str },
    /// A map isn't square, or isn't the group's tile size.
    MapSize { map: &'static str, size: (u32, u32), tile: u32 },
    /// A map's channels differ from the standing member's.
    Channels { map: &'static str },
    /// Its control map names a region its material doesn't have.
    ControlId { id: u8, regions: usize },
    /// Without a control map, one of its vertices names a region its material doesn't have.
    VertexId { id: u8, regions: usize },
    /// A vertex's UV leaves 0 to 1, which its own map would have wrapped.
    UvRange,
    /// Another slot shares some of its vertices.
    SharedVertices { with: String },
    /// An expression binds its color or UV offset.
    Expression { name: String },
    /// Its ramp is neither the standing member's asset nor bakes to the same texels.
    Ramp,
    /// Its matcap is neither the standing member's asset nor the same texels.
    Matcap,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unbound => write!(f, "it's bound to no material document"),
            Self::NotAMaterial => write!(f, "what it's bound to isn't a material document"),
            Self::Document(why) => write!(f, "its document doesn't read: {why}"),
            Self::NotAnimeCharacter => write!(f, "its document isn't an anime character material"),
            Self::Face => write!(f, "it's a face, whose shadow follows its own face map"),
            Self::Eye => write!(f, "it's an eye, laid out in its own UVs and drawn by stencil"),
            Self::Hair => write!(f, "it's hair, whose strands run along its UVs"),
            Self::Blended => write!(f, "it blends"),
            Self::SeeThrough { region } => write!(f, "its region {region} is see-through"),
            Self::Stockings { region } => {
                write!(f, "its region {region} wears stockings, whose weave is laid out in UV units")
            }
            Self::Differs { field } => write!(f, "its `{field}` differs from the standing member's"),
            Self::Regions => write!(f, "its regions aren't a prefix of the standing member's"),
            Self::MissingMap { map } => write!(f, "its {map} map isn't in the content tree"),
            Self::Procedural { map } => write!(f, "its {map} map is procedural"),
            Self::MapSettings { map } => {
                write!(f, "its {map} map's importer settings differ from the standing member's")
            }
            Self::MapSize { map, size, tile } => {
                write!(f, "its {map} map is {} × {}, not the group's {tile} × {tile} tile", size.0, size.1)
            }
            Self::Channels { map } => write!(f, "its {map} map's channels differ from the standing member's"),
            Self::ControlId { id, regions } => {
                write!(f, "its control map names material ID {id}, but its material has {regions} regions")
            }
            Self::VertexId { id, regions } => {
                write!(f, "a vertex names material ID {id}, but its material has {regions} regions")
            }
            Self::UvRange => write!(f, "its UVs leave 0 to 1"),
            Self::SharedVertices { with } => write!(f, "slot {with:?} shares its vertices"),
            Self::Expression { name } => write!(f, "expression {name:?} binds its color or UV offset"),
            Self::Ramp => write!(f, "its ramp isn't the standing member's and doesn't bake to its texels"),
            Self::Matcap => write!(f, "its matcap isn't the standing member's and isn't its texels"),
        }
    }
}

/// A slot a group refused, with every reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub slot: String,
    pub reasons: Vec<Reason>,
}

/// Why a group as the sidecar writes it can't be planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupError {
    /// Its name is empty or holds something other than letters, digits, `_` and `-`.
    Name,
    /// Another group, or a slot, has its name.
    NameTaken,
    /// It lists fewer than two slots.
    TooFew,
    /// It lists a slot twice, or a slot another group lists.
    Repeated { slot: String },
    /// The mesh has no slot of a name it lists.
    UnknownSlot { slot: String, slots: Vec<String> },
    /// Its tiles aren't a multiple of the alignment its blocks, exact levels and cap need.
    Alignment { tile: u32, alignment: u32 },
    /// Its atlas would store more than the cook takes.
    OverLimit { size: u32, limit: u32 },
}

impl fmt::Display for GroupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name => write!(f, "a group's name is letters, digits, `_` and `-`"),
            Self::NameTaken => write!(f, "another group or a slot has this name"),
            Self::TooFew => write!(f, "a group merges two slots or more"),
            Self::Repeated { slot } => write!(f, "slot {slot:?} is listed twice"),
            Self::UnknownSlot { slot, slots } => {
                write!(f, "the mesh has no slot {slot:?}; its slots are {}", slots.join(", "))
            }
            Self::Alignment { tile, alignment } => write!(
                f,
                "its {tile} tiles aren't a multiple of {alignment}, the alignment its blocks, {EXACT_LEVELS} \
                 exact levels and cap take"
            ),
            Self::OverLimit { size, limit } => {
                write!(f, "its atlas would store {size} on a side, over the cook's {limit}")
            }
        }
    }
}

/// Errors from merging.
#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    #[error("merge group {group:?}: {why}")]
    Group { group: String, why: GroupError },
    #[error("merge group {group:?} can't merge: {}", refusals_text(refusals))]
    Refused { group: String, refusals: Vec<Refusal> },
    #[error("merge group {group:?}: failed to read {}", path.display())]
    Read {
        group: String,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("merge group {group:?}: failed to read the map {}", path.display())]
    Map {
        group: String,
        path: PathBuf,
        #[source]
        source: Box<MapError>,
    },
    #[error("merge group {group:?}: failed to read or cook {}", path.display())]
    Texture {
        group: String,
        path: PathBuf,
        #[source]
        source: Box<TextureError>,
    },
    #[error("merge group {group:?}: the merged material doesn't cook")]
    Material {
        group: String,
        #[source]
        source: Box<MaterialCookError>,
    },
    #[error("merges read the content tree: cook this mesh with the tree it's in")]
    NeedsTree,
    #[error("an assembly's slots can't merge")]
    Assembly,
}

fn refusals_text(refusals: &[Refusal]) -> String {
    let slots: Vec<String> = refusals
        .iter()
        .map(|refusal| {
            let reasons: Vec<String> = refusal.reasons.iter().map(ToString::to_string).collect();
            format!("slot {:?}: {}", refusal.slot, reasons.join("; "))
        })
        .collect();
    slots.join(", ")
}

/// What a merge reads of the content tree: a source by its sidecar's ID.
pub type Lookup<'a> = dyn Fn(AssetId) -> Option<&'a SourceAsset> + 'a;

/// A group as planned from the sidecar and its members' documents.
#[derive(Clone, Debug)]
pub struct GroupPlan {
    pub name: String,
    /// In the sidecar's order.
    pub members: Vec<Member>,
    /// The member with the most regions, the first of equals: the merged material is its document.
    pub standing: usize,
    /// The maps it atlases, and what they cook as.
    pub maps: Vec<(AtlasMap, TextureKind)>,
    /// The documents it read, with their hashes.
    read: Vec<(AssetId, blake3::Hash)>,
}

/// A member of a planned group.
#[derive(Clone, Debug)]
pub struct Member {
    pub slot: String,
    pub material: AssetId,
    pub document: AnimeCharacterMaterial,
}

/// Plans the groups of `settings` that merge on `cook`, reading their members' documents: what the
/// sidecar asks for, refused slot by slot where the documents can't merge. Every group's shape is
/// checked, whichever cooks it merges on.
pub fn plan(settings: &MeshSettings, cook: Cook, lookup: &Lookup<'_>) -> Result<Vec<GroupPlan>, MergeError> {
    check_groups(settings)?;
    settings
        .merge
        .iter()
        .filter(|group| group.cooks.includes(cook))
        .map(|group| plan_group(group, settings, lookup))
        .collect()
}

/// The sidecar's groups, by their shape alone.
fn check_groups(settings: &MeshSettings) -> Result<(), MergeError> {
    let mut names = HashSet::new();
    let mut listed = HashSet::new();
    for group in &settings.merge {
        let fail = |why| Err(MergeError::Group { group: group.name.clone(), why });
        let valid = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        if group.name.is_empty() || !group.name.chars().all(valid) {
            return fail(GroupError::Name);
        }
        if !names.insert(group.name.as_str()) || settings.materials.contains_key(&group.name) {
            return fail(GroupError::NameTaken);
        }
        if group.slots.len() < 2 {
            return fail(GroupError::TooFew);
        }
        for slot in &group.slots {
            if !listed.insert(slot.as_str()) {
                return fail(GroupError::Repeated { slot: slot.clone() });
            }
        }
    }
    if let Some(group) = settings.merge.iter().find(|group| listed.contains(group.name.as_str())) {
        return Err(MergeError::Group { group: group.name.clone(), why: GroupError::NameTaken });
    }
    Ok(())
}

fn plan_group(
    group: &MergeGroup,
    settings: &MeshSettings,
    lookup: &Lookup<'_>,
) -> Result<GroupPlan, MergeError> {
    let mut read = Vec::new();
    let mut loaded: Vec<(String, Result<Member, Reason>)> = Vec::new();
    for slot in &group.slots {
        let member = load_member(&group.name, slot, settings, lookup, &mut read)?;
        loaded.push((slot.clone(), member));
    }
    let mut reasons: Vec<Vec<Reason>> = loaded
        .iter()
        .map(|(_, member)| match member {
            Ok(member) => own_reasons(&member.document),
            Err(reason) => vec![reason.clone()],
        })
        .collect();
    // The standing member: the most regions, among members clean on their own if any are.
    let candidates: Vec<usize> = (0..loaded.len()).filter(|&index| loaded[index].1.is_ok()).collect();
    let clean: Vec<usize> = candidates.iter().copied().filter(|&index| reasons[index].is_empty()).collect();
    let pool = if clean.is_empty() { &candidates } else { &clean };
    let regions = |index: usize| loaded[index].1.as_ref().map_or(0, |member| member.document.regions.len());
    let standing = pool.iter().copied().fold(None, |best: Option<usize>, index| match best {
        Some(best) if regions(best) >= regions(index) => Some(best),
        _ => Some(index),
    });
    if let Some(standing) = standing {
        let standing_document = loaded[standing].1.as_ref().expect("a candidate loaded").document.clone();
        for (index, (_, member)) in loaded.iter().enumerate() {
            if let (Ok(member), true) = (member, index != standing) {
                reasons[index].extend(compare(&member.document, &standing_document));
            }
        }
    }
    let refusals: Vec<Refusal> = loaded
        .iter()
        .zip(&reasons)
        .filter(|(_, reasons)| !reasons.is_empty())
        .map(|((slot, _), reasons)| Refusal { slot: slot.clone(), reasons: reasons.clone() })
        .collect();
    let standing = match standing {
        Some(standing) if refusals.is_empty() => standing,
        _ => return Err(MergeError::Refused { group: group.name.clone(), refusals }),
    };
    let members: Vec<Member> = loaded.into_iter().map(|(_, member)| member.expect("no refusals")).collect();
    // What each map cooks as, by the standing member's: each member's is checked against it later.
    let mut maps = Vec::new();
    let mut missing = Vec::new();
    for map in AtlasMap::ALL {
        let Some(id) = map.get(&members[standing].document) else { continue };
        match lookup(id).and_then(|source| TextureKind::of(&source.meta.importer)) {
            Some(kind) => maps.push((map, kind)),
            None => missing.push(Refusal {
                slot: members[standing].slot.clone(),
                reasons: vec![Reason::MissingMap { map: map.name() }],
            }),
        }
    }
    if !missing.is_empty() {
        return Err(MergeError::Refused { group: group.name.clone(), refusals: missing });
    }
    Ok(GroupPlan { name: group.name.clone(), members, standing, maps, read })
}

/// A member's document, or why it has none to merge.
fn load_member(
    group: &str,
    slot: &str,
    settings: &MeshSettings,
    lookup: &Lookup<'_>,
    read: &mut Vec<(AssetId, blake3::Hash)>,
) -> Result<Result<Member, Reason>, MergeError> {
    let Some(&id) = settings.materials.get(slot) else { return Ok(Err(Reason::Unbound)) };
    let Some(source) =
        lookup(id).filter(|source| matches!(source.meta.importer, ImportSettings::Material(_)))
    else {
        return Ok(Err(Reason::NotAMaterial));
    };
    let bytes = read_file(group, &source.path)?;
    read.push((id, blake3::hash(&bytes)));
    Ok(match material::read(&bytes) {
        Ok(AnyMaterialDocument::AnimeCharacter(document)) => {
            Ok(Member { slot: slot.to_owned(), material: id, document })
        }
        Ok(_) => Err(Reason::NotAnimeCharacter),
        Err(error) => Err(Reason::Document(error.to_string())),
    })
}

fn read_file(group: &str, path: &Path) -> Result<Vec<u8>, MergeError> {
    fs::read(path).map_err(|source| MergeError::Read {
        group: group.to_owned(),
        path: path.to_owned(),
        source,
    })
}

/// What keeps a document out of any group, whatever the others.
fn own_reasons(document: &AnimeCharacterMaterial) -> Vec<Reason> {
    let mut reasons = Vec::new();
    match document.surface {
        CharacterSurface::Body => {}
        CharacterSurface::Face(_) => reasons.push(Reason::Face),
        CharacterSurface::Eye(_) => reasons.push(Reason::Eye),
        CharacterSurface::Hair(_) => reasons.push(Reason::Hair),
    }
    if document.alpha == AlphaMode::Blend {
        reasons.push(Reason::Blended);
    }
    for (region, parameters) in document.regions.iter().enumerate() {
        if parameters.sees_through() {
            reasons.push(Reason::SeeThrough { region });
        }
        if parameters.stocking > 0.0 {
            reasons.push(Reason::Stockings { region });
        }
    }
    reasons
}

/// How `member` differs from the standing member's document: every parameter but the maps its UVs
/// address (whose presence still counts), its ramp's and matcap's assets (checked by their texels), and
/// its regions past its own.
fn compare(member: &AnimeCharacterMaterial, standing: &AnimeCharacterMaterial) -> Vec<Reason> {
    // Destructured, so a field added to the family must be placed here before this compiles.
    let AnimeCharacterMaterial {
        surface,
        base_color,
        base_color_map,
        alpha,
        control_map,
        normal_map,
        light_split,
        diffuse,
        regions,
        rim,
        specular,
        matcap,
        outline,
        layer_mask,
        stockings,
        emission,
        double_sided,
        dissolve,
    } = member;
    let same_diffuse = match (diffuse, &standing.diffuse) {
        (DiffuseMode::Bands(a), DiffuseMode::Bands(b)) => a == b,
        (DiffuseMode::Ramp(a), DiffuseMode::Ramp(b)) => a.mix == b.mix,
        _ => false,
    };
    let same_matcap = match (matcap, &standing.matcap) {
        (None, None) => true,
        (Some(a), Some(b)) => a.strength == b.strength && a.shadow_strength == b.shadow_strength,
        _ => false,
    };
    let checks = [
        ("surface", *surface == standing.surface),
        ("base_color", *base_color == standing.base_color),
        ("base_color_map", base_color_map.is_some() == standing.base_color_map.is_some()),
        ("alpha", *alpha == standing.alpha),
        ("control_map", control_map.is_some() == standing.control_map.is_some()),
        ("normal_map", normal_map.is_some() == standing.normal_map.is_some()),
        ("light_split", *light_split == standing.light_split),
        ("diffuse", same_diffuse),
        ("rim", *rim == standing.rim),
        ("specular", *specular == standing.specular),
        ("matcap", same_matcap),
        ("outline", *outline == standing.outline),
        ("layer_mask", layer_mask.is_some() == standing.layer_mask.is_some()),
        ("stockings", *stockings == standing.stockings),
        ("emission", *emission == standing.emission),
        ("double_sided", *double_sided == standing.double_sided),
        ("dissolve", *dissolve == standing.dissolve),
    ];
    let mut reasons: Vec<Reason> =
        checks.into_iter().filter(|(_, same)| !same).map(|(field, _)| Reason::Differs { field }).collect();
    let prefix = regions.len() <= standing.regions.len() && *regions == standing.regions[..regions.len()];
    if !prefix {
        reasons.push(Reason::Regions);
    }
    reasons
}

/// How a group's atlases are laid out: square tiles in a square grid of padded cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// A tile's side, in its maps' source texels.
    pub tile: u32,
    /// The padding on each side of a tile.
    pub padding: u32,
    /// Cells a side.
    pub grid: u32,
    /// The levels each map's cap drops on the cook.
    pub dropped: Vec<(AtlasMap, u32)>,
}

impl Layout {
    /// A tile with its padding.
    pub fn cell(&self) -> u32 {
        self.tile + 2 * self.padding
    }

    /// The atlas's side at level 0, before any cap.
    pub fn size(&self) -> u32 {
        self.grid * self.cell()
    }

    /// Member `member`'s tile's top-left texel.
    pub fn origin(&self, member: usize) -> [u32; 2] {
        let member = member as u32;
        [(member % self.grid) * self.cell() + self.padding, (member / self.grid) * self.cell() + self.padding]
    }

    /// Where member `member`'s `uv` lands in the atlas: the same texel centre at level 0 and at every
    /// exact level.
    pub fn uv(&self, member: usize, uv: [f32; 2]) -> [f32; 2] {
        let origin = self.origin(member);
        let (tile, size) = (f64::from(self.tile), f64::from(self.size()));
        [0, 1].map(|axis| ((f64::from(origin[axis]) + f64::from(uv[axis]) * tile) / size) as f32)
    }

    /// The atlas's UV units to a tile's: what the dissolve's noise scale takes on to keep its cells.
    pub fn scale(&self) -> f32 {
        (f64::from(self.size()) / f64::from(self.tile)) as f32
    }

    /// The levels `map`'s cap drops.
    pub fn dropped(&self, map: AtlasMap) -> u32 {
        self.dropped.iter().find(|(candidate, _)| *candidate == map).map_or(0, |&(_, dropped)| dropped)
    }

    /// The atlas's side as `map`'s atlas stores it.
    pub fn stored(&self, map: AtlasMap) -> u32 {
        (self.size() >> self.dropped(map)).max(1)
    }

    /// The settings `map`'s atlas cooks with on `cook`: its members' class, capped to drop their levels.
    pub fn settings(&self, map: AtlasMap, members: TextureSettings, cook: Cook) -> TextureSettings {
        let cap = (self.dropped(map) > 0).then(|| std::num::NonZeroU32::new(self.stored(map))).flatten();
        let max_size = match cook {
            Cook::Desktop => MaxSize { desktop: cap, mobile: None },
            Cook::Mobile => MaxSize { desktop: None, mobile: cap },
        };
        TextureSettings { class: members.class, max_size }
    }
}

/// Whether a source is an image or a map document, whose level 0 a merge reads.
fn map_image(importer: &ImportSettings) -> Option<()> {
    matches!(importer, ImportSettings::Texture(_) | ImportSettings::CharacterMap(..)).then_some(())
}

/// The texture settings a map source cooks with, if an atlas can read it.
fn map_settings(importer: &ImportSettings) -> Option<TextureSettings> {
    match importer {
        ImportSettings::Texture(settings) => Some(*settings),
        ImportSettings::CharacterMap(settings, max_size) => {
            let class = match settings {
                MapSettings::Ilm(ilm) => TextureClass::Ilm(*ilm),
                MapSettings::LayerMask(mask) => TextureClass::Mask(*mask),
                MapSettings::Ramp(_) | MapSettings::FaceSdf(_) => return None,
            };
            Some(TextureSettings { class, max_size: *max_size })
        }
        _ => None,
    }
}

impl GroupPlan {
    fn standing(&self) -> &Member {
        &self.members[self.standing]
    }

    fn refused(&self, refusals: BTreeMap<usize, Vec<Reason>>) -> MergeError {
        let refusals = refusals
            .into_iter()
            .map(|(member, reasons)| Refusal { slot: self.members[member].slot.clone(), reasons })
            .collect();
        MergeError::Refused { group: self.name.clone(), refusals }
    }

    /// Member `member`'s `map` source.
    fn map_source<'a>(&self, member: usize, map: AtlasMap, lookup: &Lookup<'a>) -> Option<&'a SourceAsset> {
        map.get(&self.members[member].document).and_then(lookup)
    }

    /// The atlases' layout on `cook`, by the maps' sizes and settings: each member's map refused where
    /// it isn't the standing member's kind, settings and size.
    pub fn layout(&self, cook: Cook, lookup: &Lookup<'_>) -> Result<Layout, MergeError> {
        let mut refusals: BTreeMap<usize, Vec<Reason>> = BTreeMap::new();
        let mut tile = None;
        let mut sizes = Vec::new();
        for &(map, _) in &self.maps {
            let standing = self.map_source(self.standing, map, lookup).expect("planned from it");
            for member in 0..self.members.len() {
                let mut refuse = |reason| refusals.entry(member).or_default().push(reason);
                let Some(source) = self.map_source(member, map, lookup) else {
                    refuse(Reason::MissingMap { map: map.name() });
                    continue;
                };
                if map_settings(&source.meta.importer).is_none() {
                    refuse(Reason::Procedural { map: map.name() });
                    continue;
                }
                if source.meta.importer != standing.meta.importer {
                    refuse(Reason::MapSettings { map: map.name() });
                    continue;
                }
                let size = source_size(&self.name, source)?;
                let side = *tile.get_or_insert(size.0);
                if size != (side, side) {
                    refuse(Reason::MapSize { map: map.name(), size, tile: side });
                }
            }
            sizes.push((map, map_settings(&standing.meta.importer)));
        }
        if !refusals.is_empty() {
            return Err(self.refused(refusals));
        }
        let tile = tile.expect("a group atlases a map");
        let mut alignment = 1;
        let mut dropped = Vec::new();
        for (map, settings) in sizes {
            let settings = settings.expect("checked above");
            let texture_error = |source| MergeError::Texture {
                group: self.name.clone(),
                path: self.map_source(self.standing, map, lookup).expect("planned").path.clone(),
                source: Box::new(source),
            };
            // A class's block doesn't depend on the source's channels (`texture::cooked_size`'s reason).
            let plan = texture::plan(&settings.class, cook, ChannelLayout::Rgba).map_err(texture_error)?;
            let (stored, _) = texture::cooked_size(&settings, cook, tile, tile).map_err(texture_error)?;
            let levels = (0..32).find(|&level| (tile >> level).max(1) == stored).unwrap_or(0);
            let (width, height) = plan.format.block_dimensions();
            // Every map's blocks: their least common multiple, which is the largest where they're one size.
            alignment = lcm(alignment, width.max(height) << (EXACT_LEVELS + levels));
            dropped.push((map, levels));
        }
        let group_error = |why| MergeError::Group { group: self.name.clone(), why };
        if tile % alignment != 0 {
            return Err(group_error(GroupError::Alignment { tile, alignment }));
        }
        let grid =
            (1..).find(|grid: &u32| (grid * grid) as usize >= self.members.len()).expect("a grid fits");
        let layout = Layout { tile, padding: alignment, grid, dropped };
        let size = layout.size();
        if size > MAX_TEXTURE_DIMENSION {
            return Err(group_error(GroupError::OverLimit { size, limit: MAX_TEXTURE_DIMENSION }));
        }
        for &(map, _) in &self.maps {
            let stored = layout.stored(map);
            if stored > atlas_limit(cook) {
                return Err(group_error(GroupError::OverLimit { size: stored, limit: atlas_limit(cook) }));
            }
        }
        Ok(layout)
    }

    /// The merged material's document: the standing member's, its maps the atlases, its dissolve's noise
    /// scaled to keep its cells.
    pub fn document(&self, mesh: AssetId, layout: &Layout) -> AnimeCharacterMaterial {
        let mut document = self.standing().document.clone();
        for &(map, _) in &self.maps {
            map.set(&mut document, atlas_id(mesh, &self.name, map));
        }
        document.dissolve.scale *= layout.scale();
        document
    }

    /// Checks that every member's ramp and matcap are the standing member's asset, or the same texels.
    pub fn check_shared_maps(&self, lookup: &Lookup<'_>) -> Result<(), MergeError> {
        let ramp = |member: &Member| match &member.document.diffuse {
            DiffuseMode::Ramp(ramp) => Some(ramp.map),
            DiffuseMode::Bands(_) => None,
        };
        let matcap = |member: &Member| member.document.matcap.map(|matcap| matcap.map);
        let mut refusals: BTreeMap<usize, Vec<Reason>> = BTreeMap::new();
        for (index, member) in self.members.iter().enumerate() {
            let pairs = [
                (ramp(member), ramp(self.standing()), Reason::Ramp),
                (matcap(member), matcap(self.standing()), Reason::Matcap),
            ];
            for (own, standing, reason) in pairs {
                if let (Some(own), Some(standing)) = (own, standing)
                    && !self.same_texels(own, standing, lookup)?
                {
                    refusals.entry(index).or_default().push(reason);
                }
            }
        }
        if refusals.is_empty() { Ok(()) } else { Err(self.refused(refusals)) }
    }

    /// Whether textures `a` and `b` are one asset, or the same importer settings and source texels.
    fn same_texels(&self, a: AssetId, b: AssetId, lookup: &Lookup<'_>) -> Result<bool, MergeError> {
        if a == b {
            return Ok(true);
        }
        let (Some(a), Some(b)) = (lookup(a), lookup(b)) else { return Ok(false) };
        if a.meta.importer != b.meta.importer || map_image(&a.meta.importer).is_none() {
            return Ok(false);
        }
        Ok(self.source_image(a)? == self.source_image(b)?)
    }

    /// A map source's level 0, as the texture cook takes it.
    fn source_image(&self, source: &SourceAsset) -> Result<SourceImage, MergeError> {
        let bytes = read_file(&self.name, &source.path)?;
        match &source.meta.importer {
            ImportSettings::Texture(settings) => {
                texture::import(&source.path, &bytes, settings).map_err(|error| MergeError::Texture {
                    group: self.name.clone(),
                    path: source.path.clone(),
                    source: Box::new(error),
                })
            }
            ImportSettings::CharacterMap(settings, _) => {
                character_maps::generate(&source.path, &bytes, settings).map(|map| map.image).map_err(
                    |error| MergeError::Map {
                        group: self.name.clone(),
                        path: source.path.clone(),
                        source: Box::new(error),
                    },
                )
            }
            _ => unreachable!("only images and map documents are read: `map_image`"),
        }
    }

    /// Composes `map`'s atlas and cooks it for `cook`: each member's tile at its place, padded by its own
    /// texels wrapped around, the empty cells zero. The stored top and the [`EXACT_LEVELS`] below it are
    /// then each member's own cooked blocks (`copy_tile`), so a tile is its member's whatever an encoder
    /// makes of the blocks around it; the empty cells and the levels below come from the composed image.
    /// A control map naming a region its member's material doesn't have, or a map whose channels differ
    /// from the standing member's, refuses the group.
    pub fn cook_atlas(
        &self,
        map: AtlasMap,
        layout: &Layout,
        cook: Cook,
        lookup: &Lookup<'_>,
    ) -> Result<CookedTexture, MergeError> {
        let mut refusals: BTreeMap<usize, Vec<Reason>> = BTreeMap::new();
        let standing = self.map_source(self.standing, map, lookup).expect("planned from it");
        let members = map_settings(&standing.meta.importer).expect("laid out from it");
        let size = layout.size();
        let mut pixels = vec![[0.0f32; 4]; size as usize * size as usize];
        let mut channels = None;
        let mut images = Vec::with_capacity(self.members.len());
        for (index, member) in self.members.iter().enumerate() {
            let source = self.map_source(index, map, lookup).expect("laid out from it");
            let image = self.source_image(source)?;
            let layout_of = *channels.get_or_insert(image.layout);
            if image.layout != layout_of {
                refusals.entry(index).or_default().push(Reason::Channels { map: map.name() });
                continue;
            }
            if (image.width, image.height) != (layout.tile, layout.tile) {
                let size = (image.width, image.height);
                refusals.entry(index).or_default().push(Reason::MapSize {
                    map: map.name(),
                    size,
                    tile: layout.tile,
                });
                continue;
            }
            if map == AtlasMap::Control {
                let regions = member.document.regions.len();
                let most = image.pixels.iter().map(|pixel| ilm::decode_material_id(pixel[3]).get()).max();
                if let Some(id) = most.filter(|&id| usize::from(id) >= regions) {
                    refusals.entry(index).or_default().push(Reason::ControlId { id, regions });
                    continue;
                }
            }
            place(&mut pixels, size, layout, index, &image);
            images.push((index, image));
        }
        if !refusals.is_empty() {
            return Err(self.refused(refusals));
        }
        let atlas = SourceImage::new(size, size, channels.expect("a group has members"), pixels)
            .expect("one pixel a texel");
        let texture_error = |path: &Path| {
            let path = path.to_owned();
            move |error| MergeError::Texture { group: self.name.clone(), path, source: Box::new(error) }
        };
        let mut cooked = texture::cook(&atlas, &layout.settings(map, members, cook), cook)
            .map_err(texture_error(&standing.path))?;
        for (index, image) in images {
            // The member's own chain, as its map cooks on its own (`texture::cook_chain`, and a map
            // document's `character_maps::cook_source`): the same settings and cap.
            let path = &self.map_source(index, map, lookup).expect("laid out from it").path;
            let own = texture::cook(&image, &members, cook).map_err(texture_error(path))?;
            copy_tile(&mut cooked, &own, layout, index);
        }
        Ok(cooked)
    }

    /// What its parts' keys hold besides the mesh's source and sidecar: its documents' hashes, and every
    /// map, ramp and matcap a member reads, by its source, sidecar and inputs, and its importer.
    pub fn dependencies(&self, lookup: &Lookup<'_>) -> Result<Vec<Dependency>, MergeError> {
        let marker = |path: String| Dependency::File { path, hash: blake3::hash(b"") };
        let mut dependencies = vec![marker(format!("#merge {} {}", ATLAS_IMPORTER.version, self.name))];
        for (id, hash) in &self.read {
            dependencies.push(Dependency::File { path: format!("#merge document {id}"), hash: *hash });
        }
        let mut seen = HashSet::new();
        for member in &self.members {
            let document = &member.document;
            let ramp = match &document.diffuse {
                DiffuseMode::Ramp(ramp) => Some(ramp.map),
                DiffuseMode::Bands(_) => None,
            };
            let maps = AtlasMap::ALL.map(|map| map.get(document));
            let ids = maps.into_iter().chain([ramp, document.matcap.map(|matcap| matcap.map)]).flatten();
            for id in ids {
                if !seen.insert(id) {
                    continue;
                }
                let Some(source) = lookup(id) else { continue };
                let file = |path: &Path| -> Result<Dependency, MergeError> {
                    let bytes = read_file(&self.name, path)?;
                    Ok(Dependency::File {
                        path: format!("#merge {id} {}", path.display()),
                        hash: blake3::hash(&bytes),
                    })
                };
                dependencies.push(file(&source.path)?);
                let sidecar = meta::sidecar_path(&source.path);
                if sidecar.is_file() {
                    dependencies.push(file(&sidecar)?);
                }
                if let ImportSettings::CharacterMap(settings, _) = &source.meta.importer {
                    let kind = settings.kind();
                    let importer = kind.importer();
                    dependencies
                        .push(marker(format!("#merge importer {} {}", importer.name, importer.version)));
                    let bytes = read_file(&self.name, &source.path)?;
                    let map_error = |error| MergeError::Map {
                        group: self.name.clone(),
                        path: source.path.clone(),
                        source: Box::new(error),
                    };
                    for input in character_maps::inputs(kind, &bytes).map_err(map_error)? {
                        let path = character_maps::resolve_input(&source.path, &input).map_err(map_error)?;
                        dependencies.push(file(&path)?);
                    }
                }
            }
        }
        Ok(dependencies)
    }
}

/// Copies `image` into `pixels` (an atlas `size` a side) at member `member`'s place, with its padding
/// its own texels wrapped around.
fn place(pixels: &mut [[f32; 4]], size: u32, layout: &Layout, member: usize, image: &SourceImage) {
    let [x0, y0] = layout.origin(member);
    let (tile, padding, size) = (layout.tile as i64, layout.padding as i64, size as usize);
    for y in -padding..tile + padding {
        let row = (i64::from(y0) + y) as usize * size;
        let from = y.rem_euclid(tile) as usize * tile as usize;
        for x in -padding..tile + padding {
            let at = row + (i64::from(x0) + x) as usize;
            pixels[at] = image.pixels[from + x.rem_euclid(tile) as usize];
        }
    }
}

/// The least common multiple of `a` and `b`, both above 0.
fn lcm(a: u32, b: u32) -> u32 {
    let (mut x, mut y) = (a, b);
    while y != 0 {
        (x, y) = (y, x % y);
    }
    a / x * b
}

/// Copies member `member`'s own cooked blocks (`own`, its map cooked alone) into `atlas` at the stored
/// top and each of the [`EXACT_LEVELS`] below it: the tile's blocks at its place, and its padding's from
/// its own edge blocks wrapped around, as the padding's texels are. The layout aligns a tile's place, its
/// side and its padding to whole blocks at every one of those levels, which this asserts.
fn copy_tile(atlas: &mut CookedTexture, own: &CookedTexture, layout: &Layout, member: usize) {
    let format = atlas.desc.format;
    assert_eq!(own.desc.format, format, "a member cooks to its atlas's format");
    let (block_width, block_height) = format.block_dimensions();
    let bytes = format.block_bytes() as usize;
    let origin = layout.origin(member);
    let levels = (EXACT_LEVELS as usize + 1).min(atlas.levels.len()).min(own.levels.len());
    for level in 0..levels {
        let (width, _) = atlas.desc.level_extent(level as u32);
        let (tile, _) = own.desc.level_extent(level as u32);
        // The atlas's level holds the layout's texels scaled down by `scale`, a power of two.
        let scale = layout.size() / width;
        let (x0, y0, padding) = (origin[0] / scale, origin[1] / scale, layout.padding / scale);
        assert_eq!(layout.tile / scale, tile, "member {member} level {level}: the tile's side");
        for (what, value, block) in [
            ("place", x0, block_width),
            ("place", y0, block_height),
            ("padding", padding, block_width),
            ("padding", padding, block_height),
            ("side", tile, block_width),
            ("side", tile, block_height),
        ] {
            assert!(
                value.is_multiple_of(block),
                "member {member} level {level}: its {what} {value} off the block"
            );
        }
        let (tile_columns, tile_rows) = ((tile / block_width) as i64, (tile / block_height) as i64);
        let (pad_columns, pad_rows) = ((padding / block_width) as i64, (padding / block_height) as i64);
        let (column0, row0) = (i64::from(x0 / block_width), i64::from(y0 / block_height));
        let atlas_columns = width.div_ceil(block_width) as i64;
        for row in -pad_rows..tile_rows + pad_rows {
            for column in -pad_columns..tile_columns + pad_columns {
                let from =
                    (row.rem_euclid(tile_rows) * tile_columns + column.rem_euclid(tile_columns)) as usize;
                let to = ((row0 + row) * atlas_columns + column0 + column) as usize;
                atlas.levels[level][to * bytes..(to + 1) * bytes]
                    .copy_from_slice(&own.levels[level][from * bytes..(from + 1) * bytes]);
            }
        }
    }
}

/// A map source's size at level 0, from its header (or, for a map document, its inputs'), without
/// decoding it.
fn source_size(group: &str, source: &SourceAsset) -> Result<(u32, u32), MergeError> {
    let unreadable = || MergeError::Read {
        group: group.to_owned(),
        path: source.path.clone(),
        source: io::Error::new(io::ErrorKind::InvalidData, "its size doesn't read"),
    };
    let bytes = read_file(group, &source.path)?;
    let header = |path: &Path, bytes: &[u8]| {
        SourceFormat::from_path(path).and_then(|format| image::header_size(bytes, format))
    };
    match &source.meta.importer {
        ImportSettings::CharacterMap(settings, _) => {
            let map_error = |error| MergeError::Map {
                group: group.to_owned(),
                path: source.path.clone(),
                source: Box::new(error),
            };
            let inputs = character_maps::inputs(settings.kind(), &bytes).map_err(map_error)?;
            match inputs.first() {
                Some(input) => {
                    let path = character_maps::resolve_input(&source.path, input).map_err(map_error)?;
                    let input = read_file(group, &path)?;
                    header(&path, &input).ok_or_else(unreadable)
                }
                None => {
                    let map = character_maps::generate(&source.path, &bytes, settings).map_err(map_error)?;
                    Ok((map.image.width, map.image.height))
                }
            }
        }
        _ => header(&source.path, &bytes).ok_or_else(unreadable),
    }
}

/// How a merge renumbers a mesh's slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotMap {
    /// The slots after the merge, by name.
    pub names: Vec<String>,
    /// Each slot's new index, by its old one.
    pub new: Vec<u32>,
    /// Each slot's group and its place in it, if it's a member, by its old index.
    pub member: Vec<Option<(usize, usize)>>,
}

impl SlotMap {
    /// The renumbering of `slots` (by name, in order) that merging `plans` makes: each group's slot at its
    /// first member's place, named after it, and the others closing up.
    pub fn new(slots: &[String], plans: &[GroupPlan]) -> Result<Self, MergeError> {
        let mut member = vec![None; slots.len()];
        for (group, plan) in plans.iter().enumerate() {
            let fail = |why| Err(MergeError::Group { group: plan.name.clone(), why });
            if slots.contains(&plan.name) {
                return fail(GroupError::NameTaken);
            }
            for (index, candidate) in plan.members.iter().enumerate() {
                let Some(old) = slots.iter().position(|slot| *slot == candidate.slot) else {
                    let slots = slots.iter().map(|slot| format!("{slot:?}")).collect();
                    return fail(GroupError::UnknownSlot { slot: candidate.slot.clone(), slots });
                };
                member[old] = Some((group, index));
            }
        }
        let mut names = Vec::new();
        let mut new = vec![0; slots.len()];
        let mut placed: Vec<Option<u32>> = vec![None; plans.len()];
        for (old, slot) in slots.iter().enumerate() {
            new[old] = match member[old] {
                Some((group, _)) => *placed[group].get_or_insert_with(|| {
                    names.push(plans[group].name.clone());
                    names.len() as u32 - 1
                }),
                None => {
                    names.push(slot.clone());
                    names.len() as u32 - 1
                }
            };
        }
        Ok(Self { names, new, member })
    }
}

/// Merges `plans` into `mesh`, an optimized mesh whose slots `bindings` binds: each member's UVs moved
/// into its tile, each LOD's member ranges joined into its group's, and the slots renumbered. Returns the
/// bindings the merged mesh packs with. A member whose vertices another slot shares, whose UVs leave 0 to
/// 1, or (without a control map) whose vertices name a region its material doesn't have refuses its
/// group, and the mesh is left as it was.
pub fn merge_mesh(
    mesh: &mut ImportedMesh,
    plans: &[GroupPlan],
    layouts: &[Layout],
    bindings: &BTreeMap<String, AssetId>,
    mesh_id: AssetId,
) -> Result<BTreeMap<String, AssetId>, MergeError> {
    let slots: Vec<String> = mesh.materials.iter().map(|material| material.name.clone()).collect();
    let map = SlotMap::new(&slots, plans)?;
    // Each vertex's slot, over every LOD; `u32::MAX` for none yet.
    let mut owner = vec![u32::MAX; mesh.vertices.len()];
    let mut refusals: Vec<BTreeMap<usize, Vec<Reason>>> = vec![BTreeMap::new(); plans.len()];
    let refuse = |slot: usize, reason: Reason, refusals: &mut Vec<BTreeMap<usize, Vec<Reason>>>| {
        if let Some((group, member)) = map.member[slot] {
            let reasons = refusals[group].entry(member).or_default();
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
    };
    for lod in &mesh.lods {
        for submesh in &lod.submeshes {
            let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
            for &vertex in &mesh.indices[range] {
                let current = &mut owner[vertex as usize];
                if *current == u32::MAX {
                    *current = submesh.material;
                } else if *current != submesh.material {
                    let (a, b) = (*current as usize, submesh.material as usize);
                    refuse(a, Reason::SharedVertices { with: slots[b].clone() }, &mut refusals);
                    refuse(b, Reason::SharedVertices { with: slots[a].clone() }, &mut refusals);
                }
            }
        }
    }
    for (vertex, &slot) in mesh.vertices.iter().zip(&owner) {
        let Some((group, member)) = map.member.get(slot as usize).copied().flatten() else { continue };
        if !vertex.uv.iter().all(|c| (0.0..=1.0).contains(c)) {
            refuse(slot as usize, Reason::UvRange, &mut refusals);
        }
        let document = &plans[group].members[member].document;
        let id = vertex.flags.material_id().get();
        if document.control_map.is_none() && usize::from(id) >= document.regions.len() {
            refuse(slot as usize, Reason::VertexId { id, regions: document.regions.len() }, &mut refusals);
        }
    }
    if let Some((group, refused)) = refusals.into_iter().enumerate().find(|(_, refused)| !refused.is_empty())
    {
        return Err(plans[group].refused(refused));
    }

    for (vertex, &slot) in mesh.vertices.iter_mut().zip(&owner) {
        if let Some((group, member)) = map.member.get(slot as usize).copied().flatten() {
            vertex.uv = layouts[group].uv(member, vertex.uv);
        }
    }
    let mut indices = Vec::with_capacity(mesh.indices.len());
    for lod in &mut mesh.lods {
        let old = std::mem::take(&mut lod.submeshes);
        let mut emitted = HashSet::new();
        for submesh in &old {
            let new = map.new[submesh.material as usize];
            if !emitted.insert(new) {
                continue;
            }
            let first_index = indices.len() as u32;
            for part in old.iter().filter(|part| map.new[part.material as usize] == new) {
                let range = part.first_index as usize..(part.first_index + part.index_count) as usize;
                indices.extend_from_slice(&mesh.indices[range]);
            }
            let index_count = indices.len() as u32 - first_index;
            lod.submeshes.push(sapphire_assets::mesh::Submesh { first_index, index_count, material: new });
        }
    }
    debug_assert_eq!(indices.len(), mesh.indices.len(), "a merge moves ranges, never triangles");
    mesh.indices = indices;
    let old = std::mem::take(&mut mesh.materials);
    let mut materials: Vec<Option<crate::mesh::ImportedMaterial>> = vec![None; map.names.len()];
    for (slot, material) in old.into_iter().enumerate() {
        let new = map.new[slot] as usize;
        if materials[new].is_none() {
            materials[new] = Some(crate::mesh::ImportedMaterial { name: map.names[new].clone(), ..material });
        }
    }
    mesh.materials =
        materials.into_iter().map(|material| material.expect("every slot has a first")).collect();
    let mut merged = bindings.clone();
    for plan in plans {
        for member in &plan.members {
            merged.remove(&member.slot);
        }
        merged.insert(plan.name.clone(), material_id(mesh_id, &plan.name));
    }
    Ok(merged)
}

/// Renumbers the material slots `rig`'s expressions bind as merging `plans` renumbers `slots` (the
/// mesh's, by name, in order). A bind on a member refuses its group: its color or UV offset would move
/// the whole merged slot.
pub fn remap_rig(rig: &mut CharacterRig, slots: &[String], plans: &[GroupPlan]) -> Result<(), MergeError> {
    let map = SlotMap::new(slots, plans)?;
    let mut refusals: Vec<BTreeMap<usize, Vec<Reason>>> = vec![BTreeMap::new(); plans.len()];
    for expression in &rig.expressions {
        let bound = expression
            .material_colors
            .iter()
            .map(|bind| bind.slot)
            .chain(expression.texture_transforms.iter().map(|bind| bind.slot));
        for slot in bound {
            if let Some((group, member)) = map.member.get(slot as usize).copied().flatten() {
                let reason = Reason::Expression { name: expression.name.clone() };
                let reasons = refusals[group].entry(member).or_default();
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
            }
        }
    }
    if let Some((group, refused)) = refusals.into_iter().enumerate().find(|(_, refused)| !refused.is_empty())
    {
        return Err(plans[group].refused(refused));
    }
    // A slot past the mesh's is left for the rig's own checks to refuse.
    let renumber = |slot: &mut u32| *slot = map.new.get(*slot as usize).copied().unwrap_or(*slot);
    for expression in &mut rig.expressions {
        expression.material_colors.iter_mut().for_each(|bind| renumber(&mut bind.slot));
        expression.texture_transforms.iter_mut().for_each(|bind| renumber(&mut bind.slot));
    }
    rig.material_slot_count = map.names.len() as u32;
    Ok(())
}
