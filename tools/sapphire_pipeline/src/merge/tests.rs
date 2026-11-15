//! Tests of material merge (M5 L15).

use std::collections::HashMap;
use std::io::Cursor;
use std::num::NonZeroU32;
use std::path::PathBuf;

use sapphire_assets::character_maps::{MaterialId, ilm as ilm_encoding};
use sapphire_assets::material::{
    CharacterRegion, EyeShading, FaceShading, HairShading, Matcap, RampDiffuse, Specular,
};
use sapphire_assets::mesh::{Lod, Submesh, Vertex, VertexFlags};
use sapphire_assets::rig::{
    Expression, ExpressionOverride, MaterialColor, MaterialColorBind, TextureTransformBind,
};

use super::*;
use crate::meta::AssetMeta;
use crate::test_util::{TempDir, material as imported_material};
use crate::texture::{ColorSettings, IlmSettings};

/// A content tree in a scratch directory: sources by ID.
struct Tree {
    dir: TempDir,
    sources: HashMap<AssetId, SourceAsset>,
    next: u8,
}

impl Tree {
    fn new(name: &str) -> Self {
        Self { dir: TempDir::new(name), sources: HashMap::new(), next: 0 }
    }

    fn add(&mut self, name: &str, bytes: &[u8], importer: ImportSettings) -> AssetId {
        self.next += 1;
        let id = AssetId::from_bytes([self.next; 16]);
        let path = self.dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        self.sources.insert(id, SourceAsset { path, meta: AssetMeta { id, importer, cook: true } });
        id
    }

    fn material(&mut self, name: &str, document: &AnimeCharacterMaterial) -> AssetId {
        let bytes = crate::bench::document_bytes(document);
        self.add(&format!("{name}.material.ron"), &bytes, ImportSettings::Material(Default::default()))
    }

    /// An RGB PNG `size` a side whose texels differ from each other's and from other seeds'.
    fn color(&mut self, name: &str, size: u32, seed: u32, settings: TextureSettings) -> AssetId {
        let image = ::image::RgbImage::from_fn(size, size, |x, y| {
            let v = |k: u32| ((x * 7 + y * 13 + seed * 31 + k * 5 + (x ^ y) * 3) % 256) as u8;
            ::image::Rgb([v(0), v(1), v(2)])
        });
        self.add(&format!("{name}.png"), &png(image.into()), ImportSettings::Texture(settings))
    }

    /// An RGBA control map as a texture of the ILM class, its alpha material ID `id`.
    fn control(&mut self, name: &str, size: u32, id: u8) -> AssetId {
        let alpha = (ilm_encoding::encode_material_id(MaterialId::new(id).unwrap()) * 255.0).round() as u8;
        let image = ::image::RgbaImage::from_fn(size, size, |x, y| {
            ::image::Rgba([(x * 9) as u8, (y * 5) as u8, 128, alpha])
        });
        let settings = TextureSettings::from(TextureClass::Ilm(IlmSettings::default()));
        self.add(&format!("{name}.png"), &png(image.into()), ImportSettings::Texture(settings))
    }

    fn lookup<'a>(&'a self) -> impl Fn(AssetId) -> Option<&'a SourceAsset> + 'a {
        |id| self.sources.get(&id)
    }
}

fn png(image: ::image::DynamicImage) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ::image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn color_settings() -> TextureSettings {
    TextureSettings::from(TextureClass::Color(ColorSettings::default()))
}

/// A body bound to `map`, with `regions` regions.
fn body(map: AssetId, regions: usize) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color_map: Some(map),
        regions: vec![CharacterRegion::default(); regions],
        ..AnimeCharacterMaterial::default()
    }
}

/// Mesh settings binding each `(slot, material)` and merging `group`'s slots as "body".
fn settings(bindings: &[(&str, AssetId)], group: &[&str]) -> MeshSettings {
    MeshSettings {
        materials: bindings.iter().map(|&(slot, id)| (slot.to_owned(), id)).collect(),
        merge: vec![MergeGroup {
            name: "body".to_owned(),
            slots: group.iter().map(|slot| (*slot).to_owned()).collect(),
            cooks: Cooks::Every,
        }],
        ..MeshSettings::default()
    }
}

fn refusals(error: MergeError) -> Vec<(String, Vec<Reason>)> {
    match error {
        MergeError::Refused { refusals, .. } => {
            refusals.into_iter().map(|refusal| (refusal.slot, refusal.reasons)).collect()
        }
        other => panic!("expected refusals, got {other}"),
    }
}

/// Three equal bodies of 1, 3 and 2 regions merge, the second standing; their maps are what the plan
/// atlases.
#[test]
fn equal_documents_merge_and_the_member_with_most_regions_stands() {
    let mut tree = Tree::new("merge_equal");
    let maps: Vec<AssetId> =
        (0..3).map(|seed| tree.color(&format!("map{seed}"), 64, seed, color_settings())).collect();
    let ids: Vec<AssetId> = [1, 3, 2]
        .iter()
        .zip(&maps)
        .enumerate()
        .map(|(index, (&regions, &map))| tree.material(&format!("m{index}"), &body(map, regions)))
        .collect();
    let settings = settings(&[("a", ids[0]), ("b", ids[1]), ("c", ids[2])], &["a", "b", "c"]);
    let plans = plan(&settings, Cook::Desktop, &tree.lookup()).unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].standing, 1);
    assert_eq!(plans[0].maps, [(AtlasMap::BaseColor, TextureKind::Color)]);
    let slots: Vec<&str> = plans[0].members.iter().map(|member| member.slot.as_str()).collect();
    assert_eq!(slots, ["a", "b", "c"]);
    // A group the cook leaves out plans nothing there.
    let mobile = MeshSettings {
        merge: vec![MergeGroup { cooks: Cooks::Desktop, ..settings.merge[0].clone() }],
        ..settings.clone()
    };
    assert!(plan(&mobile, Cook::Mobile, &tree.lookup()).unwrap().is_empty());
    assert!(!merges(&mobile, Cook::Mobile) && merges(&mobile, Cook::Desktop));
}

/// Every reason a document keeps its slot out, each named.
#[test]
fn each_reason_refuses_its_slot() {
    let mut tree = Tree::new("merge_reasons");
    let map = tree.color("map", 64, 0, color_settings());
    let sdf = tree.color("sdf", 64, 1, color_settings());
    let standing = body(map, 2);
    let mut documents: Vec<(&str, AnimeCharacterMaterial)> = vec![("standing", standing.clone())];
    let with = |change: &dyn Fn(&mut AnimeCharacterMaterial)| {
        let mut document = standing.clone();
        change(&mut document);
        document
    };
    documents.push(("face", with(&|d| d.surface = CharacterSurface::Face(FaceShading::new(sdf)))));
    documents.push(("eye", with(&|d| d.surface = CharacterSurface::Eye(EyeShading::default()))));
    documents.push(("hair", with(&|d| d.surface = CharacterSurface::Hair(HairShading::default()))));
    documents.push(("blended", with(&|d| d.alpha = AlphaMode::Blend)));
    documents.push(("veil", with(&|d| d.regions[1].opacity = 0.5)));
    documents.push(("stockings", with(&|d| d.regions[0].stocking = 1.0)));
    documents.push(("shinier", with(&|d| d.specular = Specular { softness: 0.3, ..d.specular })));
    documents.push(("tinted", with(&|d| d.regions[0].shade_color = [0.1, 0.2, 0.3])));
    documents.push(("one_sided", with(&|d| d.double_sided = !d.double_sided)));
    documents.push(("dissolving", with(&|d| d.dissolve.scale = 10.0)));
    documents.push(("unmapped", with(&|d| d.base_color_map = None)));
    let mut bindings: Vec<(String, AssetId)> = documents
        .iter()
        .map(|(name, document)| ((*name).to_owned(), tree.material(name, document)))
        .collect();
    // A slot bound to a texture, and one left unbound.
    bindings.push(("texture".to_owned(), map));
    let bound: Vec<(&str, AssetId)> = bindings.iter().map(|(slot, id)| (slot.as_str(), *id)).collect();
    let mut group: Vec<&str> = bound.iter().map(|(slot, _)| *slot).collect();
    group.push("generated");
    let error = plan(&settings(&bound, &group), Cook::Desktop, &tree.lookup()).unwrap_err();
    let differs = |field| Reason::Differs { field };
    let expected: Vec<(String, Vec<Reason>)> = [
        ("face", vec![Reason::Face, differs("surface")]),
        ("eye", vec![Reason::Eye, differs("surface")]),
        ("hair", vec![Reason::Hair, differs("surface")]),
        ("blended", vec![Reason::Blended, differs("alpha")]),
        ("veil", vec![Reason::SeeThrough { region: 1 }, Reason::Regions]),
        ("stockings", vec![Reason::Stockings { region: 0 }, Reason::Regions]),
        ("shinier", vec![differs("specular")]),
        ("tinted", vec![Reason::Regions]),
        ("one_sided", vec![differs("double_sided")]),
        ("dissolving", vec![differs("dissolve")]),
        ("unmapped", vec![differs("base_color_map")]),
        ("texture", vec![Reason::NotAMaterial]),
        ("generated", vec![Reason::Unbound]),
    ]
    .into_iter()
    .map(|(slot, reasons)| (slot.to_owned(), reasons))
    .collect();
    assert_eq!(refusals(error), expected);
}

/// A group whose regions aren't a prefix, whose ramp is another's of other texels, or whose matcap
/// differs is refused; one whose ramp is another asset of the same texels merges.
#[test]
fn ramps_and_matcaps_are_one_asset_or_the_same_texels() {
    let mut tree = Tree::new("merge_ramps");
    let map = tree.color("map", 64, 0, color_settings());
    let ramp_settings = TextureSettings::from(TextureClass::Ramp(Default::default()));
    let ramp = tree.color("ramp_a", 16, 5, ramp_settings);
    let same = tree.color("ramp_b", 16, 5, ramp_settings);
    let other = tree.color("ramp_c", 16, 6, ramp_settings);
    let matcap = tree.color("matcap", 16, 7, color_settings());
    let matcap_other = tree.color("matcap_b", 16, 8, color_settings());
    let document = |ramp: AssetId, matcap: AssetId| AnimeCharacterMaterial {
        diffuse: DiffuseMode::Ramp(RampDiffuse { map: ramp, mix: 1.0 }),
        matcap: Some(Matcap { map: matcap, strength: 1.0, shadow_strength: 0.5 }),
        ..body(map, 1)
    };
    let a = tree.material("a", &document(ramp, matcap));
    let b = tree.material("b", &document(same, matcap));
    let c = tree.material("c", &document(other, matcap_other));
    let lookup = tree.lookup();
    let merged = plan(&settings(&[("a", a), ("b", b)], &["a", "b"]), Cook::Desktop, &lookup).unwrap();
    merged[0].check_shared_maps(&lookup).unwrap();
    let refused = plan(&settings(&[("a", a), ("c", c)], &["a", "c"]), Cook::Desktop, &lookup).unwrap();
    let error = refused[0].check_shared_maps(&lookup).unwrap_err();
    assert_eq!(refusals(error), [("c".to_owned(), vec![Reason::Ramp, Reason::Matcap])]);
}

/// The sidecar's groups, by their shape.
#[test]
fn a_group_s_shape_is_checked() {
    let id = AssetId::from_bytes([1; 16]);
    let group = |name: &str, slots: &[&str]| MergeGroup {
        name: name.to_owned(),
        slots: slots.iter().map(|slot| (*slot).to_owned()).collect(),
        cooks: Cooks::Every,
    };
    let check = |groups: Vec<MergeGroup>| {
        let settings = MeshSettings {
            materials: [("a".to_owned(), id), ("b".to_owned(), id), ("c".to_owned(), id)].into(),
            merge: groups,
            ..MeshSettings::default()
        };
        match check_groups(&settings) {
            Err(MergeError::Group { why, .. }) => Some(why),
            Err(other) => panic!("{other}"),
            Ok(()) => None,
        }
    };
    assert_eq!(check(vec![group("body", &["a", "b"])]), None);
    assert_eq!(check(vec![group("body.1", &["a", "b"])]), Some(GroupError::Name));
    assert_eq!(check(vec![group("", &["a", "b"])]), Some(GroupError::Name));
    assert_eq!(check(vec![group("a", &["b", "c"])]), Some(GroupError::NameTaken));
    assert_eq!(check(vec![group("x", &["a", "b"]), group("x", &["c", "d"])]), Some(GroupError::NameTaken));
    assert_eq!(check(vec![group("x", &["a", "b"]), group("y", &["x", "c"])]), Some(GroupError::NameTaken));
    assert_eq!(check(vec![group("body", &["a"])]), Some(GroupError::TooFew));
    assert_eq!(check(vec![group("body", &["a", "a"])]), Some(GroupError::Repeated { slot: "a".to_owned() }));
    assert_eq!(
        check(vec![group("x", &["a", "b"]), group("y", &["b", "c"])]),
        Some(GroupError::Repeated { slot: "b".to_owned() })
    );
}

/// A group of three color maps 64 a side, as each cook lays it out.
fn three_colors(tree: &mut Tree, size: u32, settings: TextureSettings) -> GroupPlan {
    let maps: Vec<AssetId> =
        (0..3).map(|seed| tree.color(&format!("map{seed}"), size, seed, settings)).collect();
    let ids: Vec<AssetId> = maps
        .iter()
        .enumerate()
        .map(|(index, &map)| tree.material(&format!("m{index}"), &body(map, 1)))
        .collect();
    let settings = settings_of(&ids);
    plan(&settings, Cook::Desktop, &tree.lookup()).unwrap().remove(0)
}

fn settings_of(ids: &[AssetId]) -> MeshSettings {
    let names = ["a", "b", "c", "d"];
    let bindings: Vec<(&str, AssetId)> = names.iter().copied().zip(ids.iter().copied()).collect();
    settings(&bindings, &names[..ids.len()])
}

/// Tiles align to the block times `2^EXACT_LEVELS` and the levels a cap drops; the grid is square; an
/// atlas over the cook's limit, or a tile off the alignment, refuses the group.
#[test]
fn the_layout_aligns_tiles_and_respects_the_limit() {
    let mut tree = Tree::new("merge_layout");
    let group = three_colors(&mut tree, 64, color_settings());
    let layout = group.layout(Cook::Desktop, &tree.lookup()).unwrap();
    assert_eq!((layout.tile, layout.padding, layout.grid, layout.size()), (64, 32, 2, 256));
    assert_eq!([layout.origin(0), layout.origin(1), layout.origin(2)], [[32, 32], [160, 32], [32, 160]]);
    assert_eq!(layout.scale(), 4.0);
    // A mobile cap of 32 drops a level: the alignment doubles, and the atlas carries the cap that drops it.
    let mut tree = Tree::new("merge_layout_capped");
    let capped = TextureSettings {
        max_size: MaxSize { desktop: None, mobile: NonZeroU32::new(32) },
        ..color_settings()
    };
    let group = three_colors(&mut tree, 64, capped);
    let layout = group.layout(Cook::Mobile, &tree.lookup()).unwrap();
    assert_eq!((layout.padding, layout.size(), layout.stored(AtlasMap::BaseColor)), (64, 384, 192));
    let atlas = layout.settings(AtlasMap::BaseColor, capped, Cook::Mobile);
    assert_eq!(atlas.max_size, MaxSize { desktop: None, mobile: NonZeroU32::new(192) });
    assert_eq!(group.layout(Cook::Desktop, &tree.lookup()).unwrap().padding, 32);
    // 48 isn't a multiple of 32.
    let mut tree = Tree::new("merge_layout_off");
    let group = three_colors(&mut tree, 48, color_settings());
    assert!(matches!(
        group.layout(Cook::Desktop, &tree.lookup()),
        Err(MergeError::Group { why: GroupError::Alignment { tile: 48, alignment: 32 }, .. })
    ));
    // Three 2048 tiles store 4224 a side: within the desktop's 8192, over the phone's 4096.
    let mut tree = Tree::new("merge_layout_limit");
    let group = three_colors(&mut tree, 2048, color_settings());
    assert_eq!(group.layout(Cook::Desktop, &tree.lookup()).unwrap().size(), 4224);
    assert!(matches!(
        group.layout(Cook::Mobile, &tree.lookup()),
        Err(MergeError::Group { why: GroupError::OverLimit { size: 4224, limit: 4096 }, .. })
    ));
}

/// Maps of other sizes or settings refuse their slots.
#[test]
fn maps_of_other_sizes_or_settings_refuse() {
    let mut tree = Tree::new("merge_map_checks");
    let a = tree.color("a", 64, 0, color_settings());
    let b = tree.color("b", 128, 1, color_settings());
    let mask = TextureSettings::from(TextureClass::Mask(Default::default()));
    let c = tree.color("c", 64, 2, mask);
    let ids = [
        tree.material("ma", &body(a, 1)),
        tree.material("mb", &body(b, 1)),
        tree.material("mc", &body(c, 1)),
    ];
    let group = plan(&settings_of(&ids), Cook::Desktop, &tree.lookup()).unwrap().remove(0);
    let error = group.layout(Cook::Desktop, &tree.lookup()).unwrap_err();
    assert_eq!(
        refusals(error),
        [
            ("b".to_owned(), vec![Reason::MapSize { map: "base_color", size: (128, 128), tile: 64 }]),
            ("c".to_owned(), vec![Reason::MapSettings { map: "base_color" }]),
        ]
    );
}

/// A texel centre of a tile at level 0 and at each exact level is the same texel centre of the atlas.
#[test]
fn uvs_keep_their_texel_centres() {
    let layout = Layout { tile: 256, padding: 32, grid: 2, dropped: Vec::new() };
    for member in 0..3 {
        let origin = layout.origin(member);
        for level in 0..=EXACT_LEVELS {
            let (tile, size) = (layout.tile >> level, layout.size() >> level);
            for texel in [0, 1, tile / 2, tile - 1] {
                let uv = (texel as f32 + 0.5) / tile as f32;
                let moved = layout.uv(member, [uv, uv]);
                for axis in 0..2 {
                    let at = f64::from(moved[axis]) * f64::from(size);
                    let expected = f64::from(origin[axis] >> level) + f64::from(texel) + 0.5;
                    assert!(
                        (at - expected).abs() < 1e-3,
                        "member {member} level {level} texel {texel}: {at}"
                    );
                }
            }
        }
    }
}

/// The block at block column `x`, row `y` of a level `width` texels wide, in `format`.
fn block(level: &[u8], format: sapphire_assets::texture::TextureFormat, width: u32, x: u32, y: u32) -> &[u8] {
    let ((block_width, _), bytes) = (format.block_dimensions(), format.block_bytes());
    let at = ((y * width.div_ceil(block_width) + x) * bytes) as usize;
    &level[at..at + bytes as usize]
}

/// Each tile of an atlas cooks to its member's own blocks at the stored top and every exact level below,
/// on each cook and at a block above 4 × 4, and every block of its padding to its own edge blocks
/// wrapped around: on every machine, since those levels are assembled from the members' own cooks.
#[test]
fn atlas_tiles_cook_to_their_members_own_blocks() {
    let block8 =
        ColorSettings { astc_block: sapphire_assets::texture::AstcBlock::B8x8, ..ColorSettings::default() };
    for (cook, settings) in [
        (Cook::Desktop, color_settings()),
        (Cook::Mobile, color_settings()),
        (Cook::Mobile, TextureSettings::from(TextureClass::Color(block8))),
        (Cook::Mobile, TextureSettings::from(TextureClass::Ilm(IlmSettings::default()))),
    ] {
        let mut tree = Tree::new("merge_blocks");
        let group = three_colors(&mut tree, 64, settings);
        let lookup = tree.lookup();
        let layout = group.layout(cook, &lookup).unwrap();
        let atlas = group.cook_atlas(AtlasMap::BaseColor, &layout, cook, &lookup).unwrap();
        let format = atlas.desc.format;
        let (block_width, block_height) = format.block_dimensions();
        for (index, member) in group.members.iter().enumerate() {
            let source = lookup(member.document.base_color_map.unwrap()).unwrap();
            let own =
                texture::cook_chain(&source.path, &fs::read(&source.path).unwrap(), &settings, cook).unwrap();
            assert_eq!(own.desc.format, format);
            let origin = layout.origin(index);
            for level in 0..=EXACT_LEVELS {
                let (tile, size, padding) =
                    (layout.tile >> level, layout.size() >> level, layout.padding >> level);
                let own_level = &own.levels[level as usize];
                let atlas_level = &atlas.levels[level as usize];
                let (x0, y0) = ((origin[0] >> level) / block_width, (origin[1] >> level) / block_height);
                let (columns, rows) = ((tile / block_width) as i64, (tile / block_height) as i64);
                let (pad_columns, pad_rows) =
                    ((padding / block_width) as i64, (padding / block_height) as i64);
                assert!(pad_columns > 0 && pad_rows > 0, "{cook} {settings:?} level {level}: padded");
                // The tile's blocks are its own, and every padding block is its own edge's, wrapped around.
                for by in -pad_rows..rows + pad_rows {
                    for bx in -pad_columns..columns + pad_columns {
                        let at = ((i64::from(x0) + bx) as u32, (i64::from(y0) + by) as u32);
                        let from = (bx.rem_euclid(columns) as u32, by.rem_euclid(rows) as u32);
                        assert_eq!(
                            block(atlas_level, format, size, at.0, at.1),
                            block(own_level, format, tile, from.0, from.1),
                            "{cook} {settings:?} member {index} level {level} block {bx}, {by}"
                        );
                    }
                }
            }
        }
    }
}

/// A control map naming a region past its member's material's refuses it; within them, it merges.
#[test]
fn control_ids_stay_within_each_member_s_regions() {
    let mut tree = Tree::new("merge_control");
    let colors: Vec<AssetId> =
        (0..2).map(|seed| tree.color(&format!("c{seed}"), 64, seed, color_settings())).collect();
    let within = tree.control("within", 64, 0);
    let past = tree.control("past", 64, 2);
    let standing = tree.control("standing", 64, 2);
    let document = |color, control, regions| AnimeCharacterMaterial {
        control_map: Some(control),
        ..body(color, regions)
    };
    let a = tree.material("a", &document(colors[0], standing, 3));
    let b = tree.material("b", &document(colors[1], within, 1));
    let c = tree.material("c", &document(colors[1], past, 2));
    let lookup = tree.lookup();
    for (other, expected) in [(b, None), (c, Some(Reason::ControlId { id: 2, regions: 2 }))] {
        let group = plan(&settings_of(&[a, other]), Cook::Desktop, &lookup).unwrap().remove(0);
        let layout = group.layout(Cook::Desktop, &lookup).unwrap();
        let cooked = group.cook_atlas(AtlasMap::Control, &layout, Cook::Desktop, &lookup);
        match expected {
            None => assert!(cooked.is_ok()),
            Some(reason) => assert_eq!(refusals(cooked.unwrap_err()), [("b".to_owned(), vec![reason])]),
        }
    }
}

/// On a phone a colour map of 6x6 blocks and a control map of 4x4 ones align to both at every exact level, the
/// least common multiple of the two: a tile on it lays out and cooks to whole blocks of each, and a tile on one
/// block's alignment alone refuses the group with its reason, never a panic in the cook.
#[test]
fn mixed_astc_blocks_align_to_both_or_refuse_the_group() {
    let block6 =
        ColorSettings { astc_block: sapphire_assets::texture::AstcBlock::B6x6, ..ColorSettings::default() };
    let color = TextureSettings::from(TextureClass::Color(block6));
    let group = |tree: &mut Tree, size: u32| {
        let ids: Vec<AssetId> = (0..2)
            .map(|seed| {
                let document = AnimeCharacterMaterial {
                    control_map: Some(tree.control(&format!("control{seed}"), size, 0)),
                    ..body(tree.color(&format!("color{seed}"), size, seed, color), 1)
                };
                tree.material(&format!("m{seed}"), &document)
            })
            .collect();
        plan(&settings_of(&ids), Cook::Mobile, &tree.lookup()).unwrap().remove(0)
    };
    // 6 x 8 and 4 x 8 a side at the top: 96 aligns both, and 960 is ten of it.
    let mut tree = Tree::new("merge_mixed_blocks");
    let mixed = group(&mut tree, 960);
    let lookup = tree.lookup();
    let layout = mixed.layout(Cook::Mobile, &lookup).unwrap();
    assert_eq!((layout.tile, layout.padding), (960, 96));
    for map in [AtlasMap::BaseColor, AtlasMap::Control] {
        mixed.cook_atlas(map, &layout, Cook::Mobile, &lookup).unwrap();
    }
    // 1008 is on the 6x6 blocks' 48 alone.
    let mut tree = Tree::new("merge_mixed_blocks_off");
    let off = group(&mut tree, 1008);
    assert!(matches!(
        off.layout(Cook::Mobile, &tree.lookup()),
        Err(MergeError::Group { why: GroupError::Alignment { tile: 1008, alignment: 96 }, .. })
    ));
}

/// A plan of one group over `members` of `regions` regions each, without a control map, laid out for
/// tiles 64 a side: for the mesh and rig, which read only its members' slots and documents.
fn mesh_plan(members: &[(&str, usize)]) -> (GroupPlan, Layout) {
    let map = AssetId::from_bytes([9; 16]);
    let members: Vec<Member> = members
        .iter()
        .map(|&(slot, regions)| Member {
            slot: slot.to_owned(),
            material: AssetId::from_bytes([regions as u8; 16]),
            document: body(map, regions),
        })
        .collect();
    let plan = GroupPlan {
        name: "body".to_owned(),
        members,
        standing: 0,
        maps: vec![(AtlasMap::BaseColor, TextureKind::Color)],
        read: Vec::new(),
    };
    (plan, Layout { tile: 64, padding: 32, grid: 2, dropped: Vec::new() })
}

/// A mesh of four slots `a` to `d`, a triangle each over vertices of its own, in two LODs.
fn four_slots() -> ImportedMesh {
    let vertex = |u: f32| Vertex { uv: [u, 1.0 - u], ..Vertex::default() };
    let vertices: Vec<Vertex> = (0..12).map(|index| vertex(index as f32 / 11.0)).collect();
    let lod0: Vec<u32> = (0..12).collect();
    let lod1: Vec<u32> = vec![0, 1, 2, 6, 7, 8];
    let mut indices = lod0.clone();
    indices.extend(&lod1);
    let submeshes = |first: u32, slots: &[u32]| -> Vec<Submesh> {
        let at = |(i, &material): (usize, &u32)| Submesh {
            first_index: first + 3 * i as u32,
            index_count: 3,
            material,
        };
        slots.iter().enumerate().map(at).collect()
    };
    ImportedMesh {
        authored_outline: vec![false; vertices.len()],
        vertices,
        indices,
        lods: vec![
            Lod { vertex_count: 12, error: 0.0, submeshes: submeshes(0, &[0, 1, 2, 3]) },
            Lod { vertex_count: 9, error: 0.01, submeshes: submeshes(12, &[0, 2]) },
        ],
        materials: ["a", "b", "c", "d"].map(imported_material).to_vec(),
        ..ImportedMesh::default()
    }
}

/// Merging `b` and `d`: the group takes `b`'s place and name, `d`'s range joins it in each LOD, the slots
/// after close up, the members' UVs move into their tiles and the bindings follow the names.
#[test]
fn a_merged_mesh_joins_ranges_and_moves_uvs() {
    let mut mesh = four_slots();
    let before = mesh.clone();
    let (plan, layout) = mesh_plan(&[("b", 1), ("d", 1)]);
    let mesh_id = AssetId::from_bytes([7; 16]);
    let ids = |n: u8| AssetId::from_bytes([n; 16]);
    let bindings: BTreeMap<String, AssetId> =
        [("a", ids(1)), ("b", ids(2)), ("d", ids(4))].map(|(slot, id)| (slot.to_owned(), id)).into();
    let merged =
        merge_mesh(&mut mesh, std::slice::from_ref(&plan), std::slice::from_ref(&layout), &bindings, mesh_id)
            .unwrap();
    let names: Vec<&str> = mesh.materials.iter().map(|material| material.name.as_str()).collect();
    assert_eq!(names, ["a", "body", "c"]);
    assert_eq!(merged, [("a".to_owned(), ids(1)), ("body".to_owned(), material_id(mesh_id, "body"))].into());
    let ranges = |lod: &Lod| -> Vec<(u32, u32, u32)> {
        lod.submeshes.iter().map(|s| (s.first_index, s.index_count, s.material)).collect()
    };
    assert_eq!(ranges(&mesh.lods[0]), [(0, 3, 0), (3, 6, 1), (9, 3, 2)]);
    assert_eq!(ranges(&mesh.lods[1]), [(12, 3, 0), (15, 3, 2)]);
    assert_eq!(&mesh.indices[..12], &[0, 1, 2, 3, 4, 5, 9, 10, 11, 6, 7, 8]);
    assert_eq!(&mesh.indices[12..], &before.indices[12..]);
    for (index, (after, before)) in mesh.vertices.iter().zip(&before.vertices).enumerate() {
        let member = match index / 3 {
            1 => Some(0),
            3 => Some(1),
            _ => None,
        };
        let expected = member.map_or(before.uv, |member| layout.uv(member, before.uv));
        assert_eq!(after.uv, expected, "vertex {index}");
        assert_eq!(after.position, before.position);
    }
}

/// A member sharing a vertex with another slot, leaving 0 to 1, or naming a region its material lacks
/// refuses the group and leaves the mesh as it was.
#[test]
fn a_mesh_merge_refuses_shared_vertices_wrapped_uvs_and_foreign_regions() {
    let (plan, layout) = mesh_plan(&[("b", 1), ("d", 1)]);
    let attempt = |change: &dyn Fn(&mut ImportedMesh)| {
        let mut mesh = four_slots();
        change(&mut mesh);
        let before = mesh.clone();
        let error = merge_mesh(
            &mut mesh,
            std::slice::from_ref(&plan),
            std::slice::from_ref(&layout),
            &BTreeMap::new(),
            AssetId::from_bytes([7; 16]),
        )
        .unwrap_err();
        assert_eq!(mesh, before);
        refusals(error)
    };
    let shared = attempt(&|mesh| mesh.indices[3] = 0);
    assert_eq!(shared, [("b".to_owned(), vec![Reason::SharedVertices { with: "a".to_owned() }])]);
    let wrapped = attempt(&|mesh| mesh.vertices[10].uv = [1.5, 0.5]);
    assert_eq!(wrapped, [("d".to_owned(), vec![Reason::UvRange])]);
    let foreign =
        attempt(&|mesh| mesh.vertices[4].flags = VertexFlags::with_material_id(MaterialId::new(1).unwrap()));
    assert_eq!(foreign, [("b".to_owned(), vec![Reason::VertexId { id: 1, regions: 1 }])]);
    let unknown = mesh_plan(&[("b", 1), ("e", 1)]).0;
    let mut mesh = four_slots();
    let error = merge_mesh(
        &mut mesh,
        &[unknown],
        std::slice::from_ref(&layout),
        &BTreeMap::new(),
        AssetId::from_bytes([7; 16]),
    )
    .unwrap_err();
    assert!(matches!(error, MergeError::Group { why: GroupError::UnknownSlot { .. }, .. }), "{error}");
}

fn expression(name: &str, color: Option<u32>, transform: Option<u32>) -> Expression {
    Expression {
        name: name.to_owned(),
        preset: None,
        binary: false,
        override_blink: ExpressionOverride::None,
        override_look_at: ExpressionOverride::None,
        override_mouth: ExpressionOverride::None,
        morph_targets: Vec::new(),
        material_colors: color
            .map(|slot| MaterialColorBind { slot, color: MaterialColor::Base, value: [1.0; 4] })
            .into_iter()
            .collect(),
        texture_transforms: transform
            .map(|slot| TextureTransformBind { slot, scale: [1.0; 2], offset: [0.0; 2] })
            .into_iter()
            .collect(),
    }
}

/// The rig's binds follow their slots' names through the merge; a bind on a member refuses its group.
#[test]
fn rig_binds_follow_names_and_a_bind_on_a_member_refuses() {
    let slots: Vec<String> = ["a", "b", "c", "d"].map(str::to_owned).to_vec();
    // `a` and `c` merge into `body`, where `a` was: `b` keeps its place and `d` moves up one.
    let (plan, _) = mesh_plan(&[("a", 1), ("c", 1)]);
    let mut rig = CharacterRig { material_slot_count: 4, ..CharacterRig::default() };
    rig.expressions = vec![expression("blush", Some(3), None), expression("scroll", None, Some(1))];
    remap_rig(&mut rig, &slots, std::slice::from_ref(&plan)).unwrap();
    assert_eq!(rig.material_slot_count, 3);
    assert_eq!(rig.expressions[0].material_colors[0].slot, 2);
    assert_eq!(rig.expressions[1].texture_transforms[0].slot, 1);
    let mut rig = CharacterRig { material_slot_count: 4, ..CharacterRig::default() };
    rig.expressions = vec![expression("glow", Some(2), None)];
    let error = remap_rig(&mut rig, &slots, &[plan]).unwrap_err();
    assert_eq!(refusals(error), [("c".to_owned(), vec![Reason::Expression { name: "glow".to_owned() }])]);
}

fn content() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

/// Aster (M5 C8) merges skin, jacket and gear on both cooks: six slots to four, every LOD's triangles
/// kept, and nothing but the members' ranges and UVs moved.
#[test]
fn aster_merges_six_slots_to_four() {
    let root = content().join("combat/hero");
    let source = root.join("hero.vrm");
    let sidecar = meta::read(&meta::sidecar_path(&source)).unwrap();
    let settings = sidecar.importer.mesh().unwrap();
    for cook in [Cook::Desktop, Cook::Mobile] {
        let merged = crate::cook::cook_mesh_in(&root, &source, cook).unwrap();
        let plain =
            crate::cook::cook_mesh(&source, &MeshSettings { merge: Vec::new(), ..settings.clone() }, cook)
                .unwrap();
        let names: Vec<&str> = merged.materials.iter().map(|slot| slot.name.as_str()).collect();
        let before: Vec<&str> = plain.materials.iter().map(|slot| slot.name.as_str()).collect();
        assert_eq!(before.len(), 6, "{cook}: {before:?}");
        assert_eq!(names.len(), 4, "{cook}: {names:?}");
        assert!(names.contains(&"body") && !names.contains(&"skin"), "{cook}: {names:?}");
        assert_eq!(merged.indices.len(), plain.indices.len(), "{cook}");
        assert_eq!(merged.lods.len(), plain.lods.len(), "{cook}");
        for (a, b) in merged.lods.iter().zip(&plain.lods) {
            let triangles = |lod: &Lod| lod.submeshes.iter().map(|s| s.index_count).sum::<u32>();
            assert_eq!(triangles(a), triangles(b), "{cook}");
            assert_eq!(a.vertex_count, b.vertex_count, "{cook}");
        }
        assert_eq!(merged.streams.positions, plain.streams.positions, "{cook}");
        // The slots left alone keep their UVs to the bit: the range quantized UVs span hasn't moved.
        assert_eq!(merged.streams.uv_bounds, plain.streams.uv_bounds, "{cook}");
        let kept: Vec<u32> = plain
            .materials
            .iter()
            .enumerate()
            .filter(|(_, slot)| names.contains(&slot.name.as_str()))
            .map(|(index, _)| index as u32)
            .collect();
        for submesh in plain.lods[0].submeshes.iter().filter(|submesh| kept.contains(&submesh.material)) {
            let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
            for &index in &plain.indices[range] {
                let at = index as usize;
                assert_eq!(merged.streams.surface[at], plain.streams.surface[at], "{cook}: vertex {index}");
            }
        }
        assert_eq!(merged.skin, plain.skin, "{cook}");
        assert_eq!(merged.morph_deltas, plain.morph_deltas, "{cook}");
    }
}

/// The look-dev character (the shipped heroine's mesh) stays at its 13 slots: every pair of its slots is
/// refused, the face, eyes and hair whatever the other, and its five bodies by the parameters they don't
/// share. What each pair of bodies differs in is what an artist would have to make agree.
#[test]
fn the_look_dev_character_stays_at_thirteen() {
    let root = content().join("lookdev/character");
    let source = root.join("character.vrm");
    let sidecar = meta::read(&meta::sidecar_path(&source)).unwrap();
    let base = sidecar.importer.mesh().unwrap().clone();
    let resolved = crate::cook::resolve_tree(&root).unwrap();
    let sources: HashMap<AssetId, &SourceAsset> =
        resolved.assets.iter().map(|asset| (asset.meta.id, asset)).collect();
    let lookup = |id| sources.get(&id).copied();
    let slots: Vec<String> = base.materials.keys().cloned().collect();
    assert_eq!(slots.len(), 13);
    let mut table = Vec::new();
    for (i, a) in slots.iter().enumerate() {
        for b in &slots[i + 1..] {
            let settings = MeshSettings {
                merge: vec![MergeGroup {
                    name: "merged".to_owned(),
                    slots: vec![a.clone(), b.clone()],
                    cooks: Cooks::Every,
                }],
                ..base.clone()
            };
            let reasons = match plan(&settings, Cook::Desktop, &lookup) {
                Ok(_) => panic!("{a} and {b} merge"),
                Err(error) => refusals(error),
            };
            table.push((a.clone(), b.clone(), reasons));
        }
    }
    let never = [
        "EyeHighlight_00_EYE",
        "EyeIris_00_EYE",
        "EyeWhite_00_EYE",
        "FaceBrow_00_FACE",
        "FaceEyeline_00_FACE",
    ];
    for (a, b, reasons) in &table {
        for (slot, reasons) in reasons {
            let own = |reason: &Reason| {
                matches!(reason, Reason::Face | Reason::Eye | Reason::Hair | Reason::Blended)
            };
            if never.contains(&slot.as_str()) || slot.starts_with("Face_") || slot.starts_with("Hair") {
                assert!(reasons.iter().any(own), "{a} + {b}: {slot}: {reasons:?}");
            }
        }
    }
    let bodies = ["Body_00_SKIN", "Bottoms_01_CLOTH", "FaceMouth_00_FACE", "Shoes_01_CLOTH", "Tops_01_CLOTH"];
    for (a, b, reasons) in
        table.iter().filter(|(a, b, _)| bodies.contains(&a.as_str()) && bodies.contains(&b.as_str()))
    {
        let fields: Vec<String> =
            reasons.iter().flat_map(|(_, reasons)| reasons.iter().map(ToString::to_string)).collect();
        let fields = fields.join("; ");
        // The three garments are their one region's values and their specular apart; the skin is also its
        // tint and its own control and normal maps away, and the mouth its light, ramp, rim and line.
        if [a, b].iter().any(|slot| slot.starts_with("Body") || slot.starts_with("FaceMouth")) {
            assert!(
                fields.contains("`base_color`") || fields.contains("`light_split`"),
                "{a} + {b}: {fields}"
            );
        } else {
            let garments = "its `specular` differs from the standing member's; \
                            its regions aren't a prefix of the standing member's";
            assert_eq!(fields, garments, "{a} + {b}");
        }
    }
}
