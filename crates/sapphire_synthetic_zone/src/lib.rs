//! A zone made of nothing but what streaming reads: a square of cells, each with a merged mesh, some
//! instances and some lights, around a backdrop. Streaming's bugs show on it before art can hide them,
//! and a walk's cost on it says what activation costs with no content to blame (M3 W2).
//!
//! Owns the zone's shape and writing it as a pack in memory. Doesn't own streaming, which is
//! `sapphire_world`'s. Dev-only: tests and benches link it, and no shipped build does (R8).
//!
//! It's written as a cooked pack, as a cook would, because a cooker can't be linked from here (R1). The
//! proof that a real cook makes the same shape is `sapphire_pipeline`'s `large_zone` test and the town.

use sapphire_assets::material::{AnimeCharacterMaterial, Material};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, SlotMaterial, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::pack::{Codec, PackAsset, PackError, write_pack};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_assets::{AssetId, AssetKind};
use sapphire_serialize::cooked::AlignedVec;
use sapphire_zone::{
    CookedExit, CookedInstance, CookedLight, CookedLightShape, PlaceContents, Zone, ZoneCell,
    ZonePresentation, ZoneSimulation,
};

const ROOT: AssetId = AssetId::from_text("7d0f1c52-3a6e-4b1f-9a44-51c7e2b08d36");

/// What a synthetic zone holds.
#[derive(Clone, Copy, Debug)]
pub struct SyntheticZone {
    /// Cells a side: a square of `side * side` cells, one corner at the zone's origin.
    pub side: i32,
    pub cell_size: f32,
    /// In each cell, besides its merged mesh.
    pub instances: u32,
    pub lights: u32,
    /// A side of the grid a merged mesh is, in quads: `2 * quads * quads` triangles.
    pub quads: u32,
    /// A side of the RGBA8 texture each cell's mesh draws with, through a material of its own, so that
    /// a walk has textures and materials to give back as well as meshes. 0 has them all share one
    /// untextured material.
    pub texture: u32,
}

impl SyntheticZone {
    /// A large one: 32 x 32 cells of 64 m, about 2 km a side, of 2,048 triangles a cell, 8 instances and
    /// 2 lights.
    pub const LARGE: Self =
        Self { side: 32, cell_size: 64.0, instances: 8, lights: 2, quads: 32, texture: 0 };

    /// A small one for tests that look at every cell.
    pub const SMALL: Self = Self { side: 8, cell_size: 32.0, instances: 2, lights: 1, quads: 2, texture: 0 };

    /// The zone's ID.
    #[must_use]
    pub fn id() -> AssetId {
        ROOT.sub_asset("zone")
    }

    /// The merged mesh of cell `(x, z)`.
    #[must_use]
    pub fn cell_mesh(x: i32, z: i32) -> AssetId {
        ROOT.sub_asset(&format!("cell/{x}/{z}"))
    }

    /// The texture of cell `(x, z)`, for a zone with [`Self::texture`].
    #[must_use]
    pub fn cell_texture(x: i32, z: i32) -> AssetId {
        ROOT.sub_asset(&format!("cell texture/{x}/{z}"))
    }

    /// The backdrop's mesh.
    #[must_use]
    pub fn backdrop_mesh() -> AssetId {
        ROOT.sub_asset("backdrop")
    }

    /// The mesh the instances share.
    #[must_use]
    pub fn instance_mesh() -> AssetId {
        ROOT.sub_asset("instance")
    }

    /// The pack that holds it all, with `extra` besides.
    ///
    /// # Panics
    ///
    /// If the zone it builds isn't one a cook could make.
    pub fn pack(self, extra: Vec<PackAsset>) -> Result<Vec<u8>, PackError> {
        self.pack_under(ROOT, Vec::new(), extra)
    }

    /// The zone numbered `room`, for a test of going between zones: one of a family, asset IDs of its own.
    #[must_use]
    pub fn room_id(room: u32) -> AssetId {
        room_root(room).sub_asset("zone")
    }

    /// The pack of the zone [`Self::room_id`] names, with its `exits`, and `extra` besides.
    ///
    /// # Panics
    ///
    /// If the zone it builds isn't one a cook could make.
    pub fn room_pack(
        self,
        room: u32,
        exits: Vec<CookedExit>,
        extra: Vec<PackAsset>,
    ) -> Result<Vec<u8>, PackError> {
        self.pack_under(room_root(room), exits, extra)
    }

    fn pack_under(
        self,
        root: AssetId,
        exits: Vec<CookedExit>,
        extra: Vec<PackAsset>,
    ) -> Result<Vec<u8>, PackError> {
        let material_id = root.sub_asset("material");
        let mut assets = extra;
        assets.push(asset(material_id, AssetKind::Material, &material_bytes(None)));
        assets.push(asset(
            root.sub_asset("instance"),
            AssetKind::Mesh,
            &patch([0.0; 3], 1.0, 1, material_id),
        ));
        let backdrop = self.cell_size * self.side as f32;
        assets.push(asset(
            root.sub_asset("backdrop"),
            AssetKind::Mesh,
            &patch([0.0; 3], backdrop, 1, material_id),
        ));

        let mut cells = Vec::with_capacity((self.side * self.side) as usize);
        let mut instances = Vec::new();
        let mut lights = Vec::new();
        for x in 0..self.side {
            for z in 0..self.side {
                let corner = [x as f32 * self.cell_size, 0.0, z as f32 * self.cell_size];
                let mesh = root.sub_asset(&format!("cell/{x}/{z}"));
                let material = if self.texture == 0 {
                    material_id
                } else {
                    let texture = root.sub_asset(&format!("cell texture/{x}/{z}"));
                    let material = root.sub_asset(&format!("cell material/{x}/{z}"));
                    assets.push(asset(texture, AssetKind::Texture, &texture_bytes(self.texture, x, z)));
                    assets.push(asset(material, AssetKind::Material, &material_bytes(Some(texture))));
                    material
                };
                assets.push(asset(
                    mesh,
                    AssetKind::Mesh,
                    &patch(corner, self.cell_size, self.quads, material),
                ));
                let first_instance = instances.len() as u32;
                let first_light = lights.len() as u32;
                let spot = |n: u32, of: u32| {
                    let along = (n as f32 + 0.5) / of as f32 * self.cell_size;
                    [corner[0] + along, 0.0, corner[2] + along]
                };
                instances.extend((0..self.instances).map(|n| CookedInstance {
                    mesh: root.sub_asset("instance"),
                    position: spot(n, self.instances),
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [2.0, 2.0, 2.0],
                }));
                lights.extend((0..self.lights).map(|n| CookedLight {
                    position: spot(n, self.lights),
                    color: [1.0, 0.8, 0.6],
                    intensity: 10.0,
                    range: 8.0,
                    shape: CookedLightShape::Point,
                    at_night: false,
                }));
                cells.push(ZoneCell {
                    coord: [x, z],
                    contents: PlaceContents {
                        mesh: Some(mesh),
                        bounds_min: [corner[0], -1.0, corner[2]],
                        bounds_max: [corner[0] + self.cell_size, 4.0, corner[2] + self.cell_size],
                        first_instance,
                        instance_count: self.instances,
                        first_light,
                        light_count: self.lights,
                    },
                });
            }
        }
        let zone = Zone {
            simulation: ZoneSimulation {
                colliders: None,
                spawners: Vec::new(),
                triggers: Vec::new(),
                navigation: None,
                encounters: Vec::new(),
                exits,
            },
            presentation: ZonePresentation {
                cell_size: self.cell_size,
                backdrop: Some(PlaceContents {
                    mesh: Some(root.sub_asset("backdrop")),
                    bounds_min: [0.0, -1.0, 0.0],
                    bounds_max: [backdrop, 1.0, backdrop],
                    first_instance: 0,
                    instance_count: 0,
                    first_light: 0,
                    light_count: 0,
                }),
                cells,
                instances,
                lights,
                probes: None,
                day: None,
                rigs: None,
                splines: Vec::new(),
            },
        };
        let bytes = zone.to_cooked().expect("a synthetic zone is a valid one");
        assets.push(asset(root.sub_asset("zone"), AssetKind::Zone, &bytes));
        write_pack(assets)
    }
}

fn room_root(room: u32) -> AssetId {
    ROOT.sub_asset(&format!("room/{room}"))
}

fn asset(id: AssetId, kind: AssetKind, bytes: &[u8]) -> PackAsset {
    let mut aligned = AlignedVec::<16>::new();
    aligned.extend_from_slice(bytes);
    let dependencies = kind.references(&aligned).expect("a synthetic asset is a valid one");
    PackAsset::encode(id, kind, Codec::Lz4, bytes, dependencies, 3).expect("a synthetic asset encodes")
}

/// A square of one color, a cell's own, so that it costs a real texture's GPU memory and a pack little.
fn texture_bytes(side: u32, x: i32, z: i32) -> Vec<u8> {
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: side,
        height: side,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    let texel = [x as u8, z as u8, 128, 255];
    let texels = texel.repeat((side * side) as usize);
    Texture { desc, levels: vec![&texels] }.to_ktx2().expect("a synthetic texture is a valid one")
}

fn material_bytes(base_color_map: Option<AssetId>) -> Vec<u8> {
    let document = AnimeCharacterMaterial { base_color_map, ..AnimeCharacterMaterial::default() };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&document, &swizzles).expect("the default material is valid");
    material.to_cooked().expect("the default material cooks").into_vec()
}

/// A flat grid of `quads * quads` squares from `corner`, `size` across, with one material.
fn patch(corner: [f32; 3], size: f32, quads: u32, material: AssetId) -> Vec<u8> {
    let row = quads + 1;
    let vertices: Vec<Vertex> = (0..row * row)
        .map(|n| {
            let (u, v) = ((n % row) as f32 / quads as f32, (n / row) as f32 / quads as f32);
            Vertex {
                position: [corner[0] + u * size, corner[1], corner[2] + v * size],
                normal: [0.0, 1.0, 0.0],
                outline_normal: [0.0, 1.0, 0.0],
                uv: [u, v],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            }
        })
        .collect();
    let mut indices = Vec::with_capacity((quads * quads * 6) as usize);
    for z in 0..quads {
        for x in 0..quads {
            let at = z * row + x;
            indices.extend_from_slice(&[at, at + row, at + 1, at + 1, at + row, at + row + 1]);
        }
    }
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slot = MaterialSlot { name: "ground".into(), material: SlotMaterial::Asset(material) };
    Mesh::from_vertices(&vertices, indices, submeshes, vec![slot], PositionFormat::Float32)
        .to_cooked()
        .expect("a synthetic mesh cooks")
        .into_vec()
}
