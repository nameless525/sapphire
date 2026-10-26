use std::sync::Arc;

use super::*;
use crate::material::{AnimeCharacterMaterial, Material};
use crate::mesh::{
    MaterialSlot, Mesh, PositionFormat, SlotMaterial, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use crate::pack::{Codec, PackAsset, write_pack};
use crate::texture::{Swizzle, Texture, TextureDesc, TextureFormat};

mod reload;

fn id(n: u8) -> AssetId {
    AssetId::from_bytes([n; 16])
}

const TEXTURE: u8 = 1;
const MATERIAL: u8 = 2;
const MESH: u8 = 3;

/// A cooked RGBA8 texture, `width` × `height`, every byte `fill`.
fn texture_bytes(width: u32, height: u32, fill: u8) -> Vec<u8> {
    let texels = vec![fill; (width * height * 4) as usize];
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width,
        height,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    Texture { desc, levels: vec![&texels] }.to_ktx2().unwrap()
}

/// A cooked anime character material, with `texture` as its base color map.
fn material_bytes(texture: Option<AssetId>) -> Vec<u8> {
    let document = AnimeCharacterMaterial { base_color_map: texture, ..AnimeCharacterMaterial::default() };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(&document, &swizzles).unwrap().to_cooked().unwrap().into_vec()
}

/// A cooked triangle whose one slot is bound to material asset `material`.
fn mesh_bytes(material: AssetId) -> Vec<u8> {
    let vertex = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [x, y],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let slot = MaterialSlot { name: "skin".into(), material: SlotMaterial::Asset(material) };
    let submeshes = vec![Submesh { first_index: 0, index_count: 3, material: 0 }];
    let mesh = Mesh::from_vertices(
        &[vertex(0.0, 0.0), vertex(1.0, 0.0), vertex(0.0, 1.0)],
        vec![0, 1, 2],
        submeshes,
        vec![slot],
        PositionFormat::Unorm16,
    );
    mesh.to_cooked().unwrap().into_vec()
}

/// `bytes` encoded for a pack, with the references its kind reads as dependencies.
fn packed(id: AssetId, kind: AssetKind, bytes: &[u8]) -> PackAsset {
    let mut aligned = AlignedVec::<16>::new();
    aligned.extend_from_slice(bytes);
    let dependencies = kind.references(&aligned).unwrap();
    PackAsset::encode(id, kind, Codec::Lz4, bytes, dependencies, 3).unwrap()
}

/// A texture, a material using it and a mesh using the material.
fn character() -> Vec<PackAsset> {
    vec![
        packed(id(TEXTURE), AssetKind::Texture, &texture_bytes(4, 4, 7)),
        packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(TEXTURE)))),
        packed(id(MESH), AssetKind::Mesh, &mesh_bytes(id(MATERIAL))),
    ]
}

/// A server with `assets` in a pack called `shared`, and the source it reads.
fn serve(assets: Vec<PackAsset>, budgets: AssetBudgets) -> (AssetServer, Arc<MemoryPackSource>) {
    let source = Arc::new(MemoryPackSource::new());
    source.insert("shared", write_pack(assets).unwrap());
    let mut server = AssetServer::new(source.clone(), budgets);
    server.mount("shared").unwrap();
    (server, source)
}

fn unlimited() -> AssetBudgets {
    AssetBudgets { upload_per_frame: u64::MAX, resident: [u64::MAX; AssetKind::ALL.len()] }
}

/// An uploader that accepts everything and records what it saw.
fn record(order: &mut Vec<(AssetKind, AssetId)>) -> impl FnMut(Upload<'_>) -> Result<(), UploadError> + '_ {
    |upload| {
        order.push((upload.kind(), upload.id()));
        Ok(())
    }
}

#[test]
fn dependencies_reach_the_gpu_before_the_assets_that_use_them() {
    let (mut server, _) = serve(character(), unlimited());
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    assert_eq!(server.state(&mesh), LoadState::Loading);
    let mut order = Vec::new();
    server.finish(&mesh, record(&mut order)).unwrap();
    assert_eq!(
        order,
        [(AssetKind::Texture, id(TEXTURE)), (AssetKind::Material, id(MATERIAL)), (AssetKind::Mesh, id(MESH))]
    );

    // Loading again shares the asset, and its dependencies are ready with it.
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    assert_eq!(server.state(&material), LoadState::Ready);
    assert_eq!(server.stats().assets, 3);
    // Materials keep their bytes for CPU readers; meshes and textures are GPU data.
    let view = server.get(&material).expect("materials keep their bytes");
    assert_eq!(view.textures[0].texture(), Some(id(TEXTURE)));
    assert!(server.get(&mesh).is_none());

    assert_eq!(
        server.load::<Mesh>(id(MATERIAL)).unwrap_err(),
        LoadError::WrongKind { id: id(MATERIAL), expected: AssetKind::Mesh, found: AssetKind::Material }
    );
    assert_eq!(server.load::<Mesh>(id(99)).unwrap_err(), LoadError::NotFound(id(99)));
}

#[test]
fn uploads_are_plain_cooked_data() {
    let (mut server, _) = serve(character(), unlimited());
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    let mut seen = 0;
    server
        .finish(&mesh, |upload| {
            seen += 1;
            assert!((upload.bytes().as_ptr() as usize).is_multiple_of(16));
            match upload.kind() {
                AssetKind::Texture => {
                    let texture = upload.get::<TextureAsset>().expect("a texture");
                    assert_eq!((texture.desc.width, texture.levels[0][0]), (4, 7));
                    assert!(upload.get::<Mesh>().is_none());
                }
                AssetKind::Material => assert!(upload.get::<Material>().is_some()),
                AssetKind::Mesh => {
                    let mesh = upload.get::<Mesh>().expect("a mesh");
                    assert!(matches!(
                        mesh.materials[0].material,
                        crate::mesh::ArchivedSlotMaterial::Asset(_)
                    ));
                }
                AssetKind::Rig => unreachable!("rigs never go to the uploader"),
                AssetKind::ProbeGrid => unreachable!("these assets include no probe grid"),
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(seen, 3);
}

#[test]
fn a_parent_is_ready_only_once_its_children_are() {
    // One asset a frame.
    let budgets = AssetBudgets { upload_per_frame: 1, ..unlimited() };
    let (mut server, _) = serve(character(), budgets);
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    while server.stats().waiting < 3 {
        server.update();
        server.wait(Duration::from_millis(50));
    }
    let states =
        |server: &AssetServer| [server.state(&texture), server.state(&material), server.state(&mesh)];
    let (loading, ready) = (LoadState::Loading, LoadState::Ready);
    let mut frames = Vec::new();
    while server.state(&mesh) != ready {
        server.update();
        let mut order = Vec::new();
        assert_eq!(server.upload(record(&mut order)).assets, 1, "the budget passes one asset a frame");
        frames.push((order, states(&server)));
    }
    assert_eq!(
        frames,
        [
            (vec![(AssetKind::Texture, id(TEXTURE))], [ready.clone(), loading.clone(), loading.clone()]),
            (vec![(AssetKind::Material, id(MATERIAL))], [ready.clone(), ready.clone(), loading]),
            (vec![(AssetKind::Mesh, id(MESH))], [ready.clone(), ready.clone(), ready]),
        ]
    );
}

#[test]
fn the_upload_budget_holds_each_frame() {
    // RGBA8 textures of 1, 1, 3, 6 and 1 MiB, against 4 MiB a frame.
    let sides = [512, 512, 887, 1254, 512];
    let assets: Vec<PackAsset> = (0..)
        .zip(sides)
        .map(|(n, side)| {
            let bytes = texture_bytes(side, side, n);
            PackAsset::encode(id(n + 1), AssetKind::Texture, Codec::None, &bytes, vec![], 3).unwrap()
        })
        .collect();
    let total: u64 = assets.iter().map(|asset| asset.size).sum();
    let largest = assets.iter().map(|asset| asset.size).max().expect("five textures");
    let budget = 4 * MIB;
    let (mut server, _) = serve(assets, AssetBudgets { upload_per_frame: budget, ..unlimited() });
    let handles: Vec<Handle<TextureAsset>> = (1..=5).map(|n| server.load(id(n)).unwrap()).collect();
    let mut frames = Vec::new();
    while handles.iter().any(|handle| server.state(handle) != LoadState::Ready) {
        server.update();
        let report = server.upload(|_| Ok(()));
        if report.assets > 0 {
            assert!(report.bytes <= budget || report.assets == 1, "{report:?} over the budget");
            frames.push(report.bytes);
        } else {
            server.wait(Duration::from_millis(50));
        }
    }
    assert_eq!(frames.iter().sum::<u64>(), total);
    // The 6 MiB texture takes a frame of its own; the rest fit the budget a frame. Decodes
    // finish in any order, so that is the bound.
    assert!(frames.len() > (total - largest).div_ceil(budget) as usize, "{frames:?}");
    assert!(frames.iter().any(|&bytes| bytes > budget), "the 6 MiB texture goes alone: {frames:?}");
}

#[test]
fn strong_and_weak_handles() {
    let (mut server, _) = serve(character(), unlimited());
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    server.finish(&texture, |_| Ok(())).unwrap();
    let weak = texture.downgrade();
    assert_eq!(size_of::<WeakHandle<TextureAsset>>(), 8);
    assert_eq!(size_of::<Handle<TextureAsset>>(), size_of::<usize>());
    let copy = texture.clone();
    assert_eq!(copy, texture);
    assert_eq!(server.upgrade(weak), Some(texture.clone()));
    let untyped = texture.clone().into_untyped();
    assert_eq!((untyped.id(), untyped.kind()), (id(TEXTURE), AssetKind::Texture));
    assert!(untyped.clone().typed::<Mesh>().is_none());
    assert_eq!(untyped.typed::<TextureAsset>(), Some(texture.clone()));

    // Unused and unloaded, the weak handle resolves to nothing.
    drop((texture, copy));
    server.update();
    assert_eq!(server.stats().unused, 1);
    server.unload_unused();
    assert_eq!(server.upgrade(weak), None);
    assert_eq!(server.take_unloaded().iter().map(|unloaded| unloaded.id).collect::<Vec<_>>(), [id(TEXTURE)]);

    // Even after its slot is reused, by the same asset loaded again.
    let again: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    assert_ne!(again.downgrade(), weak, "a new generation");
    assert_eq!(server.upgrade(weak), None);
    assert!(server.upgrade(again.downgrade()).is_some());
}

/// `count` 64-texel RGBA8 textures, IDs 1 to `count`.
fn textures(count: u8) -> Vec<PackAsset> {
    (1..=count).map(|n| packed(id(n), AssetKind::Texture, &texture_bytes(8, 8, n))).collect()
}

#[test]
fn residency_unloads_the_least_recently_used_and_never_what_is_used_or_pinned() {
    let assets = textures(6);
    let size = assets[0].size;
    assert!(assets.iter().all(|asset| asset.size == size));
    let mut budgets = unlimited();
    budgets.resident[AssetKind::Texture.index()] = 3 * size;
    let (mut server, _) = serve(assets, budgets);
    let load = |server: &mut AssetServer, n: u8| -> Handle<TextureAsset> {
        let handle = server.load(id(n)).unwrap();
        server.finish(&handle, |_| Ok(())).unwrap();
        handle
    };
    let unloaded = |server: &mut AssetServer| -> Vec<AssetId> {
        server.take_unloaded().iter().map(|unloaded| unloaded.id).collect()
    };
    let [a, b, c] = [1, 2, 3].map(|n| load(&mut server, n));
    let weak_a = a.downgrade();
    drop(a);
    server.update();
    server.update();
    drop(b);
    server.update();
    // C stays resident without users while pinned.
    server.pin(&c);
    let weak_c = c.downgrade();
    drop(c);
    server.update();
    assert_eq!(server.stats().unused, 3);
    assert!(unloaded(&mut server).is_empty(), "within budget, unused assets stay cached");

    // A fourth goes over budget: the least recently used, A, goes.
    let d = load(&mut server, 4);
    assert_eq!(unloaded(&mut server), [id(1)]);
    assert_eq!(server.upgrade(weak_a), None);
    // Then B. C is pinned, D and E are in use.
    let e = load(&mut server, 5);
    assert_eq!(unloaded(&mut server), [id(2)]);
    // Nothing else may go, so the kind stays over budget.
    let f = load(&mut server, 6);
    assert!(unloaded(&mut server).is_empty());
    let stats = server.stats();
    assert_eq!(stats.resident[AssetKind::Texture.index()], 4 * size);
    assert_eq!(stats.pinned, 1);

    // Unpinned, C goes on the next update.
    let c = server.upgrade(weak_c).expect("still resident");
    server.unpin(&c);
    drop(c);
    server.update();
    assert_eq!(unloaded(&mut server), [id(3)]);
    assert_eq!(server.stats().resident[AssetKind::Texture.index()], 3 * size);
    drop((d, e, f));
}

#[test]
fn unused_dependencies_go_with_their_parent() {
    let (mut server, _) = serve(character(), unlimited());
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    server.finish(&mesh, |_| Ok(())).unwrap();
    drop(mesh);
    server.update();
    server.unload_unused();
    let mut gone: Vec<AssetId> = server.take_unloaded().iter().map(|unloaded| unloaded.id).collect();
    gone.sort();
    assert_eq!(gone, [id(TEXTURE), id(MATERIAL), id(MESH)]);
    assert_eq!(server.stats().assets, 0);
}

#[test]
fn failures_reach_every_asset_waiting_on_them() {
    // A corrupt texture block.
    let mut bytes = write_pack(character()).unwrap();
    let reader = crate::pack::PackReader::new(&bytes).unwrap();
    let block = reader.toc().blocks(reader.toc().find(id(TEXTURE)).unwrap())[0];
    bytes[block.offset as usize] ^= 1;
    let source = Arc::new(MemoryPackSource::new());
    source.insert("shared", bytes);
    let mut server = AssetServer::new(source, unlimited());
    server.mount("shared").unwrap();
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    // Taken before the failure: loading a failed asset tries it again.
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    let mut order = Vec::new();
    let error = server.finish(&mesh, record(&mut order)).unwrap_err();
    assert_eq!(error, LoadError::Dependency { id: id(MESH), dependency: id(MATERIAL) });
    assert!(order.is_empty(), "nothing reaches the GPU");
    assert_eq!(
        server.state(&texture),
        LoadState::Failed(LoadError::Pack(PackError::BlockHash { id: id(TEXTURE), block: 0 }))
    );
    drop((mesh, texture));
    // One update per level: each frees the dependencies of what the last one dropped.
    for _ in 0..3 {
        server.update();
    }
    assert_eq!(server.stats().assets, 0, "failed assets nothing uses are dropped");

    // An upload the app refuses fails the asset and those waiting on it.
    let (mut server, _) = serve(character(), unlimited());
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    let error = server
        .finish(&mesh, |upload| match upload.kind() {
            AssetKind::Texture => Err("no BC support".into()),
            _ => Ok(()),
        })
        .unwrap_err();
    assert_eq!(error, LoadError::Dependency { id: id(MESH), dependency: id(MATERIAL) });
    assert_eq!(
        server.state(&texture),
        LoadState::Failed(LoadError::Upload { id: id(TEXTURE), message: "no BC support".into() })
    );
    assert_eq!(server.stats().failures, 3);
}

/// A source whose reads fail after the pack is mounted.
struct FailingReads {
    inner: Arc<MemoryPackSource>,
    fail: std::sync::atomic::AtomicBool,
}

impl PackSource for FailingReads {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        self.inner.open(name)
    }

    fn read(&self, file: PackFile, offset: u64, len: usize, priority: LoadPriority, done: ReadDone) {
        if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
            done(Err(io::Error::other("the disk is gone")));
        } else {
            self.inner.read(file, offset, len, priority, done);
        }
    }
}

#[test]
fn read_errors_and_bad_references_fail_loads() {
    let inner = Arc::new(MemoryPackSource::new());
    inner.insert("shared", write_pack(character()).unwrap());
    let source = Arc::new(FailingReads { inner, fail: false.into() });
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount("shared").unwrap();
    source.fail.store(true, std::sync::atomic::Ordering::Relaxed);
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    assert_eq!(
        server.finish(&texture, |_| Ok(())).unwrap_err(),
        LoadError::Io { id: id(TEXTURE), message: "the disk is gone".into() }
    );

    // A pack that lists other dependencies than the asset references.
    let mut material = packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(TEXTURE))));
    material.dependencies = vec![id(TEXTURE), id(9)];
    let assets = vec![
        packed(id(TEXTURE), AssetKind::Texture, &texture_bytes(4, 4, 1)),
        packed(id(9), AssetKind::Texture, &texture_bytes(4, 4, 2)),
        material,
    ];
    let (mut server, _) = serve(assets, unlimited());
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    assert_eq!(server.finish(&material, |_| Ok(())).unwrap_err(), LoadError::References { id: id(MATERIAL) });

    // A dependency in no mounted pack.
    let (mut server, _) = serve(
        vec![packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(TEXTURE))))],
        unlimited(),
    );
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    assert_eq!(
        server.state(&material),
        LoadState::Failed(LoadError::Dependency { id: id(MATERIAL), dependency: id(TEXTURE) })
    );
}

#[test]
fn dependencies_must_be_of_a_lower_kind() {
    // Two textures claiming each other, and a material claiming a mesh: what a malformed pack
    // could list to build a cycle or an unbounded chain.
    let mut a = packed(id(7), AssetKind::Texture, &texture_bytes(4, 4, 1));
    let b = packed(id(8), AssetKind::Texture, &texture_bytes(4, 4, 2));
    a.dependencies = vec![id(8)];
    let mut material = packed(id(MATERIAL), AssetKind::Material, &material_bytes(None));
    material.dependencies = vec![id(MESH)];
    let mesh = packed(id(MESH), AssetKind::Mesh, &mesh_bytes(id(MATERIAL)));
    let (mut server, _) = serve(vec![a, b, material, mesh], unlimited());
    let a: Handle<TextureAsset> = server.load(id(7)).unwrap();
    let kind_error = |id, kind, dependency, dependency_kind| {
        LoadState::Failed(LoadError::DependencyKind { id, kind, dependency, dependency_kind })
    };
    assert_eq!(server.state(&a), kind_error(id(7), AssetKind::Texture, id(8), AssetKind::Texture));
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    assert_eq!(
        server.state(&material),
        kind_error(id(MATERIAL), AssetKind::Material, id(MESH), AssetKind::Mesh)
    );
    assert!(server.finish(&mesh, |_| Ok(())).is_err());
    drop((a, mesh, material));
    for _ in 0..3 {
        server.update();
    }
    server.unload_unused();
    assert_eq!(server.stats().assets, 0, "no reference cycle keeps them alive");
}

#[test]
fn failed_assets_load_again_when_asked_again() {
    let inner = Arc::new(MemoryPackSource::new());
    inner.insert("shared", write_pack(character()).unwrap());
    let source = Arc::new(FailingReads { inner, fail: false.into() });
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount("shared").unwrap();
    // The texture's read fails, which fails the material and the mesh.
    source.fail.store(true, std::sync::atomic::Ordering::Relaxed);
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    assert!(server.finish(&mesh, |_| Ok(())).is_err());
    // A refused upload fails too.
    source.fail.store(false, std::sync::atomic::Ordering::Relaxed);
    let again: Handle<Mesh> = server.load(id(MESH)).unwrap();
    assert_eq!(again, mesh, "the same asset, tried again with its dependencies");
    let refuse = |upload: Upload<'_>| match upload.kind() {
        AssetKind::Mesh => Err("the device is busy".into()),
        _ => Ok(()),
    };
    assert!(matches!(server.finish(&mesh, refuse), Err(LoadError::Upload { .. })));
    // Asked again, it loads: the material and texture are ready already, only the mesh is read.
    server.load::<Mesh>(id(MESH)).unwrap();
    let mut order = Vec::new();
    server.finish(&mesh, record(&mut order)).unwrap();
    assert_eq!(order, [(AssetKind::Mesh, id(MESH))]);
    assert_eq!(server.stats().loads, 3, "three assets, each loaded once");
}

#[test]
fn reuploads_follow_dependencies_and_read_again_what_was_dropped() {
    let (mut server, _) = serve(character(), unlimited());
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    server.finish(&mesh, |_| Ok(())).unwrap();
    let loads = server.stats().loads;
    server.reupload_all();
    assert_eq!(server.state(&mesh), LoadState::Loading);
    // The texture and mesh dropped their bytes, so they are read again; the material kept its.
    assert_eq!(server.stats().reads_in_flight, 2);
    let mut order = Vec::new();
    server.finish(&mesh, record(&mut order)).unwrap();
    assert_eq!(
        order,
        [(AssetKind::Texture, id(TEXTURE)), (AssetKind::Material, id(MATERIAL)), (AssetKind::Mesh, id(MESH))]
    );
    assert_eq!(server.stats().loads, loads, "the same assets, not new loads");
}

#[test]
fn rigs_are_ready_without_the_gpu_and_keep_their_bytes() {
    use crate::rig::CharacterRig;
    let rig = CharacterRig { joint_count: 2, ..CharacterRig::default() }.to_cooked().unwrap();
    let mut assets = character();
    assets.push(packed(id(20), AssetKind::Rig, &rig));
    let (mut server, _) = serve(assets, unlimited());
    let handle: Handle<CharacterRig> = server.load(id(20)).unwrap();
    let mut order = Vec::new();
    server.finish(&handle, record(&mut order)).unwrap();
    assert!(order.is_empty(), "nothing went to the uploader");
    assert_eq!(server.get(&handle).expect("rigs keep their bytes").joint_count, 2);
    // A new renderer gets nothing of it, and it stays ready.
    server.reupload_all();
    assert_eq!(server.state(&handle), LoadState::Ready);
}

#[test]
fn handles_from_another_server_find_nothing() {
    let (mut a, _) = serve(character(), unlimited());
    let (mut b, _) = serve(textures(1), unlimited());
    // Each server's first slot: the same key, different assets of different kinds.
    let mesh: Handle<Mesh> = a.load(id(MESH)).unwrap();
    let texture: Handle<TextureAsset> = b.load(id(1)).unwrap();
    a.finish(&mesh, |_| Ok(())).unwrap();
    b.finish(&texture, |_| Ok(())).unwrap();
    assert_eq!(mesh.downgrade().key, texture.downgrade().key);
    assert_eq!(b.state(&mesh), LoadState::Failed(LoadError::NotFound(id(MESH))));
    assert!(b.get(&mesh).is_none());
    assert_eq!(b.upgrade(mesh.downgrade()), None, "a slot of another kind");
    b.pin(&mesh);
    assert_eq!(b.stats().pinned, 0);
}

#[test]
fn mounting_checks_packs() {
    let source = Arc::new(MemoryPackSource::new());
    source.insert("shared", write_pack(character()).unwrap());
    source.insert("zone", write_pack(textures(1)).unwrap());
    let mut corrupt = write_pack(textures(2)).unwrap();
    corrupt[0] = b'X';
    source.insert("corrupt", corrupt);
    let mut server = AssetServer::new(source, unlimited());
    server.mount("shared").unwrap();
    assert!(server.contains(id(MESH)));
    assert!(matches!(server.mount("shared"), Err(MountError::AlreadyMounted(_))));
    assert!(matches!(server.mount("missing"), Err(MountError::Io { .. })));
    assert!(matches!(server.mount("corrupt"), Err(MountError::Pack { source: PackError::NotAPack, .. })));
    // Texture 1 is in both.
    assert!(matches!(server.mount("zone"), Err(MountError::Duplicate { .. })));
    assert!(!server.contains(id(4)));
}
