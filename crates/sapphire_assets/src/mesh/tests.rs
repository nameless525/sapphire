use super::*;
use crate::character_maps::MaterialId;
use crate::material::{AnimeCharacterMaterial, Material, MaterialError};
use crate::texture::Swizzle;

/// The generated default material, as a slot.
fn default_slot() -> MaterialSlot {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).unwrap();
    MaterialSlot::embedded("default", material)
}

/// A quad (two triangles) with a two-joint skin and one morph target.
fn quad(format: PositionFormat) -> Mesh {
    let vertex = |x: f32, y: f32| Vertex {
        position: [x, y, 0.5],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [x, 1.0 - y],
        color: [128, 200, 255],
        flags: VertexFlags::with_material_id(MaterialId::new(x as u8 + 2).unwrap()),
    };
    let vertices = [vertex(0.0, 0.0), vertex(1.0, 0.0), vertex(1.0, 2.0), vertex(0.0, 2.0)];
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    let mut mesh = Mesh::from_vertices(
        &vertices,
        vec![0, 1, 2, 0, 2, 3, 0, 1, 2],
        submeshes,
        vec![default_slot()],
        format,
    );
    // LOD1: one triangle, reading the first three vertices.
    mesh.lods.push(Lod {
        vertex_count: 3,
        error: 0.25,
        submeshes: vec![Submesh { first_index: 6, index_count: 3, material: 0 }],
    });
    let mut root = Joint { name: "root".into(), parent: None, rest: JointTransform::IDENTITY };
    root.rest.translation = [0.0, 1.0, 0.0];
    let tip = Joint { name: "tip".into(), parent: Some(0), rest: JointTransform::IDENTITY };
    let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    let influences = [
        VertexInfluence { binds: [0, 0, 0, 0], weights: [255, 0, 0, 0] },
        VertexInfluence { binds: [0, 1, 0, 0], weights: [128, 127, 0, 0] },
        VertexInfluence { binds: [1, 0, 0, 0], weights: [255, 0, 0, 0] },
        VertexInfluence { binds: [1, 0, 0, 0], weights: [255, 0, 0, 0] },
    ];
    mesh.skin = Some(Skin {
        joints: vec![root, tip],
        binds: vec![
            SkinBind { joint: 0, inverse_bind_matrix: identity },
            SkinBind { joint: 1, inverse_bind_matrix: identity },
        ],
        influences: Influences::pack(&influences, 2),
    });
    mesh.morph_targets = vec![
        MorphTarget { name: "smile".into(), default_weight: 0.0, first_delta: 0, delta_count: 2 },
        MorphTarget { name: "unused".into(), default_weight: 0.5, first_delta: 2, delta_count: 0 },
    ];
    mesh.morph_deltas = vec![
        MorphDelta::new(2, [0.0, 0.01, 0.0], [0.0; 3]),
        MorphDelta::new(3, [0.0, 0.02, 0.0], [0.0, 0.1, -0.005]),
    ];
    mesh
}

fn default_slot_params() -> Vec<u8> {
    let SlotMaterial::Embedded(material) = default_slot().material else { unreachable!() };
    material.params
}

fn read_error(mesh: &Mesh) -> MeshError {
    Mesh::read(&mesh.to_cooked().expect("cook")).expect_err("invalid mesh")
}

#[test]
fn element_sizes_are_stable() {
    assert_eq!(size_of::<QuantizedPosition>(), 8);
    assert_eq!(size_of::<FloatPosition>(), 12);
    assert_eq!(size_of::<VertexShading>(), 8);
    assert_eq!(size_of::<VertexSurface>(), 8);
    assert_eq!(size_of::<Influence8>(), 8);
    assert_eq!(size_of::<Influence16>(), 12);
    assert_eq!(size_of::<MorphDelta>(), 16);
    for format in PositionFormat::ALL {
        assert_eq!(format.stride() % 4, 0, "vertex buffer strides must be multiples of 4");
    }
}

#[test]
fn cooked_meshes_round_trip_and_are_read_in_place() {
    for format in PositionFormat::ALL {
        let mesh = quad(format);
        let bytes = mesh.to_cooked().unwrap();
        assert_eq!(&bytes[..4], b"SPCK");
        let archived = Mesh::read(&bytes).unwrap();
        assert_eq!(Mesh::from_cooked(&bytes).unwrap(), mesh);

        let streams = &archived.streams;
        assert_eq!((streams.len(), streams.position_format()), (4, format));
        assert_eq!(streams.position_bytes().len(), 4 * format.stride());
        assert_eq!((streams.shading_bytes().len(), streams.surface_bytes().len()), (32, 32));
        assert_eq!(&archived.index_bytes()[4..8], &1u32.to_le_bytes());
        assert_eq!(archived.skin.as_ref().unwrap().influences.as_bytes().len(), 32);
        assert_eq!(archived.materials[0].name, "default");
        let ArchivedSlotMaterial::Embedded(material) = &archived.materials[0].material else {
            panic!("an embedded material");
        };
        assert_eq!(material.params(), &default_slot_params()[..]);

        // The shaders' affine decode reproduces the vertices.
        let (offset, scale) = streams.position_transform();
        let (uv_offset, uv_scale) = streams.uv_transform();
        assert_eq!((uv_offset, uv_scale), ([0.0, -1.0], [1.0, 2.0]));
        let corner = mesh.streams.vertex(2).unwrap();
        let decoded = match format {
            PositionFormat::Unorm16 => {
                let q = QuantizedPosition::encode(corner.position, &mesh.streams.bounds).get();
                std::array::from_fn(|i| offset[i] + scale[i] * f32::from(q[i]) / 65535.0)
            }
            PositionFormat::Float32 => corner.position,
        };
        assert_eq!(decoded, [1.0, 2.0, 0.5]);
    }
}

#[test]
fn packing_bounds_fit_the_vertices() {
    let mesh = quad(PositionFormat::Unorm16);
    assert_eq!(mesh.streams.bounds, Bounds { min: [0.0, 0.0, 0.5], max: [1.0, 2.0, 0.5] });
    for index in 0..4 {
        let vertex = mesh.streams.vertex(index).unwrap();
        assert_eq!(vertex.position[2], 0.5, "a flat axis decodes exactly");
        assert_eq!(vertex.normal, [0.0, 0.0, 1.0]);
        assert_eq!(vertex.color, [128, 200, 255]);
        let id = mesh.streams.vertex(index).unwrap().flags.material_id().get();
        assert_eq!(id, if index == 1 || index == 2 { 3 } else { 2 });
    }
    assert_eq!(mesh.streams.vertex(4), None);
    assert_eq!(Bounds::enclosing([]), Bounds::default());
    assert!(!Bounds { min: [1.0; 3], max: [0.0; 3] }.is_valid());
}

#[test]
fn cooked_bytes_are_deterministic() {
    let a = quad(PositionFormat::Unorm16).to_cooked().unwrap();
    let b = quad(PositionFormat::Unorm16).to_cooked().unwrap();
    assert_eq!(a.as_slice(), b.as_slice());
}

#[test]
fn stale_and_corrupt_bytes_are_errors() {
    let bytes = quad(PositionFormat::Unorm16).to_cooked().unwrap();
    // Schema version (header offset 16).
    let mut stale = bytes.clone();
    stale[16] = 1;
    assert!(matches!(
        Mesh::read(&stale),
        Err(MeshError::Cooked(CookedError::Stale { found: 1, current: 4 }))
    ));
    // No byte flip may panic or read out of bounds; most are caught.
    let mut rejected = 0;
    for index in 32..bytes.len() {
        let mut corrupt = bytes.clone();
        corrupt[index] ^= 0xff;
        rejected += usize::from(Mesh::read(&corrupt).is_err());
    }
    assert!(rejected > 0);
}

#[test]
fn rejects_meshes_the_gpu_cannot_use() {
    let check = |edit: fn(&mut Mesh), expected: fn(&MeshError) -> bool| {
        let mut mesh = quad(PositionFormat::Unorm16);
        edit(&mut mesh);
        let error = read_error(&mesh);
        assert!(expected(&error), "{error:?}");
    };
    check(|m| m.indices[2] = 9, |e| matches!(e, MeshError::IndexOutOfRange { index: 9, .. }));
    check(|m| m.indices.push(0), |e| matches!(e, MeshError::NotTriangles(10)));
    check(|m| m.streams.shading.pop().map_or((), drop), |e| matches!(e, MeshError::StreamLengths { .. }));
    check(|m| m.streams.bounds.max[0] = f32::NAN, |e| matches!(e, MeshError::InvalidBounds));
    check(
        |m| m.streams.bounds = Bounds { min: [-3e38; 3], max: [3e38; 3] },
        |e| matches!(e, MeshError::InvalidBounds),
    );
    check(|m| m.streams.uv_bounds.min = [2.0; 2], |e| matches!(e, MeshError::InvalidUvBounds));
    check(
        |m| {
            let Positions::Quantized(p) = &mut m.streams.positions else { unreachable!() };
            p[1] = QuantizedPosition::from_bytes([0, 0, 0, 0, 0, 0, 1, 0]);
        },
        |e| matches!(e, MeshError::ReservedNotZero { vertex: 1 }),
    );
    check(
        |m| {
            let bytes = m.streams.surface[2].to_bytes();
            m.streams.surface[2] =
                VertexSurface::from_bytes([bytes[0], bytes[1], bytes[2], bytes[3], 0, 0, 0, 0x0b]);
        },
        |e| matches!(e, MeshError::ReservedFlags { vertex: 2, bits: 0x0b }),
    );
    check(
        |m| {
            let SlotMaterial::Embedded(material) = &mut m.materials[0].material else { unreachable!() };
            material.params.pop();
        },
        |e| matches!(e, MeshError::InvalidMaterial { slot: 0, source: MaterialError::ParamsSize { .. } }),
    );
    check(|m| m.lods.clear(), |e| matches!(e, MeshError::NoLods));
    check(|m| m.lods[0].vertex_count = 3, |e| matches!(e, MeshError::LodVertexCount { lod: 0, .. }));
    check(
        |m| m.lods[1].vertex_count = 5,
        |e| matches!(e, MeshError::LodVertexCount { lod: 1, found: 5, .. }),
    );
    check(
        |m| m.lods[1].vertex_count = 2,
        |e| matches!(e, MeshError::OutsideLodPrefix { lod: 1, index: 2, .. }),
    );
    check(|m| m.lods[1].error = -1.0, |e| matches!(e, MeshError::LodError { lod: 1 }));
    check(|m| m.lods[0].error = 0.5, |e| matches!(e, MeshError::LodError { lod: 1 }));
    check(|m| m.lods[0].submeshes[0].first_index = 1, |e| matches!(e, MeshError::SubmeshNotTriangles { .. }));
    check(
        |m| m.lods[1].submeshes[0].first_index = 9,
        |e| matches!(e, MeshError::SubmeshOutOfRange { lod: 1, .. }),
    );
    check(
        |m| m.lods[0].submeshes[0].material = 1,
        |e| matches!(e, MeshError::MissingMaterial { material: 1, .. }),
    );
    check(
        |m| m.skin.as_mut().unwrap().joints[0].parent = Some(1),
        |e| matches!(e, MeshError::JointOrder { .. }),
    );
    check(
        |m| m.skin.as_mut().unwrap().joints[1].rest.rotation[3] = f32::NAN,
        |e| matches!(e, MeshError::NonFiniteJoint { joint: 1 }),
    );
    check(
        |m| m.skin.as_mut().unwrap().binds[1].joint = 2,
        |e| matches!(e, MeshError::MissingJoint { bind: 1, .. }),
    );
    check(
        |m| m.skin.as_mut().unwrap().binds.truncate(1),
        |e| matches!(e, MeshError::MissingBind { bind: 1, .. }),
    );
    check(
        |m| {
            let influences = [VertexInfluence { binds: [0; 4], weights: [254, 0, 0, 0] }; 4];
            m.skin.as_mut().unwrap().influences = Influences::pack(&influences, 2);
        },
        |e| matches!(e, MeshError::WeightSum { vertex: 0, sum: 254 }),
    );
    check(
        |m| m.skin.as_mut().unwrap().influences = Influences::pack(&[VertexInfluence::default(); 3], 2),
        |e| matches!(e, MeshError::InfluenceCount { found: 3, vertex_count: 4 }),
    );
    check(|m| m.morph_targets[1].first_delta = 1, |e| matches!(e, MeshError::MorphRange { target: 1 }));
    check(|m| m.morph_targets[1].delta_count = 1, |e| matches!(e, MeshError::MorphRange { target: 1 }));
    check(
        |m| m.morph_deltas.push(MorphDelta::new(0, [0.0; 3], [0.0; 3])),
        |e| matches!(e, MeshError::UnownedDeltas(1)),
    );
    check(
        |m| m.morph_targets[0].default_weight = f32::NAN,
        |e| matches!(e, MeshError::NonFiniteWeight { target: 0 }),
    );
    check(
        |m| m.morph_deltas.swap(0, 1),
        |e| matches!(e, MeshError::MorphVertex { target: 0, delta: 1, vertex: 2 }),
    );
    check(
        |m| m.morph_deltas[0] = MorphDelta::new(4, [0.0; 3], [0.0; 3]),
        |e| matches!(e, MeshError::MorphVertex { vertex: 4, .. }),
    );
    check(
        |m| m.morph_deltas[1] = MorphDelta::new(3, [f32::INFINITY, 0.0, 0.0], [0.0; 3]),
        |e| matches!(e, MeshError::NonFiniteDelta { target: 0, delta: 1 }),
    );
    let mut float = quad(PositionFormat::Float32);
    let Positions::Float(positions) = &mut float.streams.positions else { unreachable!() };
    positions[3] = FloatPosition::new([0.0, f32::NAN, 0.0]);
    assert!(matches!(read_error(&float), MeshError::NonFinitePosition { vertex: 3 }));
    let mut outside = quad(PositionFormat::Float32);
    let Positions::Float(positions) = &mut outside.streams.positions else { unreachable!() };
    positions[2] = FloatPosition::new([1.0, 2.5, 0.5]);
    assert!(matches!(read_error(&outside), MeshError::OutsideBounds { vertex: 2 }));
}
