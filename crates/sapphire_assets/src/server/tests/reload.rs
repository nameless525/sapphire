//! Hot reload: remounting changed packs and swapping new versions in behind handles.

use std::collections::HashMap;

use super::*;
use crate::pack::PackReader;
use crate::rig::CharacterRig;

/// What an uploader saw: each asset's kind, ID and generation.
type Uploaded = Vec<(AssetKind, AssetId, u32)>;

/// An uploader that accepts everything and records what it saw.
fn recording(uploads: &mut Uploaded) -> impl FnMut(Upload<'_>) -> Result<(), UploadError> + '_ {
    |upload| {
        uploads.push((upload.kind(), upload.id(), upload.generation()));
        Ok(())
    }
}

/// The character pack with its texture filled with `fill` and `material` as its material.
fn character_pack(fill: u8, material: &[u8]) -> Vec<u8> {
    write_pack(vec![
        packed(id(TEXTURE), AssetKind::Texture, &texture_bytes(4, 4, fill)),
        packed(id(MATERIAL), AssetKind::Material, material),
        packed(id(MESH), AssetKind::Mesh, &mesh_bytes(id(MATERIAL))),
    ])
    .unwrap()
}

/// The character's material, unchanged.
fn same_material() -> Vec<u8> {
    material_bytes(Some(id(TEXTURE)))
}

/// The character's material with another base color.
fn tinted_material() -> Vec<u8> {
    let document = AnimeCharacterMaterial {
        base_color: [0.5, 0.25, 0.125, 1.0],
        base_color_map: Some(id(TEXTURE)),
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(&document, &swizzles).unwrap().to_cooked().unwrap().into_vec()
}

/// The character's mesh, loaded and on the GPU, with the slot each asset was uploaded for.
fn loaded_character(server: &mut AssetServer) -> (Handle<Mesh>, HashMap<AssetId, AssetSlot>) {
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    let mut slots = HashMap::new();
    server
        .finish(&mesh, |upload| {
            slots.insert(upload.id(), upload.slot());
            Ok(())
        })
        .unwrap();
    (mesh, slots)
}

/// Runs frames until nothing is read, decoded, waiting or queued: what follows a remount.
fn settle(server: &mut AssetServer, mut upload: impl FnMut(Upload<'_>) -> Result<(), UploadError>) {
    for _ in 0..1000 {
        server.update();
        let report = server.upload(&mut upload);
        let stats = server.stats();
        let busy = stats.reads_in_flight + stats.loading + stats.waiting + stats.reloading + report.pending;
        if report.assets == 0 && busy == 0 {
            return;
        }
        if report.assets == 0 {
            server.wait(Duration::from_millis(50));
        }
    }
    panic!("hot reload never settled: {:?}", server.stats());
}

#[test]
fn a_changed_asset_reloads_behind_its_handle_and_its_dependents_follow() {
    let (mut server, source) = serve(character(), unlimited());
    let (mesh, slots) = loaded_character(&mut server);
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    let old_material = server.get(&material).unwrap().params().to_vec();

    source.insert("shared", character_pack(9, &same_material()));
    let report = server.remount("shared").unwrap();
    assert_eq!(report, RemountReport { changed: 1, removed: 0, reloading: 1 });
    // The version it has stays ready, and readable, while the new one loads.
    assert_eq!(server.state(&texture), LoadState::Ready);
    assert_eq!(server.stats().reloading, 1);

    let mut uploads = Vec::new();
    settle(&mut server, |upload| {
        assert_eq!(slots[&upload.id()], upload.slot(), "the same slot, so the app replaces its copy");
        if let Some(texture) = upload.get::<TextureAsset>() {
            assert_eq!(texture.levels[0][0], 9, "the new version");
        }
        uploads.push((upload.kind(), upload.id(), upload.generation()));
        Ok(())
    });
    // The texture's new version, then the material that binds it, sent again from its kept bytes.
    // The mesh names its material by the renderer's handle, which an in-place update keeps.
    assert_eq!(uploads, [(AssetKind::Texture, id(TEXTURE), 1), (AssetKind::Material, id(MATERIAL), 1)]);
    assert_eq!(
        [server.generation(&texture), server.generation(&material), server.generation(&mesh)],
        [1, 1, 0]
    );
    assert_eq!(server.get(&material).unwrap().params(), old_material, "a rebinding keeps the bytes");
    assert_eq!(
        server.take_changes(),
        [
            AssetChange::Reloaded {
                id: id(TEXTURE),
                kind: AssetKind::Texture,
                slot: slots[&id(TEXTURE)],
                generation: 1
            },
            AssetChange::Reloaded {
                id: id(MATERIAL),
                kind: AssetKind::Material,
                slot: slots[&id(MATERIAL)],
                generation: 1
            },
        ]
    );
    let stats = server.stats();
    assert_eq!((stats.reloads, stats.reload_failures, stats.reloading, stats.assets), (2, 0, 0, 3));
    assert_eq!(server.state(&mesh), LoadState::Ready);
    assert!(server.take_changes().is_empty());
}

#[test]
fn a_new_version_loads_its_new_dependencies_first_and_releases_the_old_ones() {
    let (mut server, source) = serve(character(), unlimited());
    let (mesh, _) = loaded_character(&mut server);
    // The material samples texture 4 instead.
    source.insert(
        "shared",
        write_pack(vec![
            packed(id(TEXTURE), AssetKind::Texture, &texture_bytes(4, 4, 7)),
            packed(id(4), AssetKind::Texture, &texture_bytes(4, 4, 5)),
            packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(4)))),
            packed(id(MESH), AssetKind::Mesh, &mesh_bytes(id(MATERIAL))),
        ])
        .unwrap(),
    );
    assert_eq!(server.remount("shared").unwrap(), RemountReport { changed: 2, removed: 0, reloading: 1 });
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert_eq!(uploads, [(AssetKind::Texture, id(4), 0), (AssetKind::Material, id(MATERIAL), 1)]);
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    assert_eq!(server.get(&material).unwrap().textures[0].texture(), Some(id(4)));
    // The texture only the old version used lost its last user.
    server.update();
    server.unload_unused();
    let unloaded: Vec<AssetId> = server.take_unloaded().iter().map(|unloaded| unloaded.id).collect();
    assert_eq!(unloaded, [id(TEXTURE)]);
    drop((mesh, material));
}

#[test]
fn a_reload_waits_for_its_dependencies_new_versions() {
    let (mut server, source) = serve(character(), unlimited());
    let (_mesh, _) = loaded_character(&mut server);
    // Both the texture and the material that binds it change.
    source.insert("shared", character_pack(9, &tinted_material()));
    assert_eq!(server.remount("shared").unwrap().reloading, 2);
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    // The material's new version binds the texture's: it goes once, after it.
    assert_eq!(uploads, [(AssetKind::Texture, id(TEXTURE), 1), (AssetKind::Material, id(MATERIAL), 1)]);
}

#[test]
fn a_new_version_that_fails_keeps_the_old_one_and_is_reported() {
    // A corrupt block in the texture's new version.
    let (mut server, source) = serve(character(), unlimited());
    let (_mesh, _) = loaded_character(&mut server);
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    let mut bytes = character_pack(9, &same_material());
    let reader = PackReader::new(&bytes).unwrap();
    let block = reader.toc().blocks(reader.toc().find(id(TEXTURE)).unwrap())[0];
    bytes[block.offset as usize] ^= 1;
    source.insert("shared", bytes);
    assert_eq!(server.remount("shared").unwrap().reloading, 1);
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert!(uploads.is_empty(), "nothing reaches the GPU, the material isn't sent again");
    assert_eq!((server.state(&texture), server.generation(&texture)), (LoadState::Ready, 0));
    let error = LoadError::Pack(PackError::BlockHash { id: id(TEXTURE), block: 0 });
    assert_eq!(
        server.take_changes(),
        [AssetChange::ReloadFailed { id: id(TEXTURE), kind: AssetKind::Texture, error }]
    );

    // A material that isn't one any more.
    let (mut server, source) = serve(character(), unlimited());
    let (_mesh, _) = loaded_character(&mut server);
    let mut assets = character();
    assets[1] =
        PackAsset::encode(id(MATERIAL), AssetKind::Material, Codec::Lz4, &[7; 64], vec![], 3).unwrap();
    source.insert("shared", write_pack(assets).unwrap());
    server.remount("shared").unwrap();
    settle(&mut server, |_| Ok(()));
    let changes = server.take_changes();
    assert!(
        matches!(&changes[..], [AssetChange::ReloadFailed { error: LoadError::Invalid { .. }, .. }]),
        "{changes:?}"
    );
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    assert_eq!(server.get(&material).unwrap().textures[0].texture(), Some(id(TEXTURE)), "the old version");

    // An upload the app refuses: the texture keeps its version, and nothing binds the new one.
    let (mut server, source) = serve(character(), unlimited());
    let (_mesh, _) = loaded_character(&mut server);
    source.insert("shared", character_pack(9, &same_material()));
    server.remount("shared").unwrap();
    let mut uploads = Vec::new();
    settle(&mut server, |upload| {
        uploads.push(upload.kind());
        Err("out of memory".into())
    });
    assert_eq!(uploads, [AssetKind::Texture]);
    let stats = server.stats();
    assert_eq!((stats.reloads, stats.reload_failures, stats.failures, stats.ready), (0, 1, 0, 3));
}

#[test]
fn assets_removed_from_their_pack_keep_their_version_until_they_are_back() {
    let (mut server, source) = serve(character(), unlimited());
    let (mesh, _) = loaded_character(&mut server);
    // The pack loses the mesh.
    source.insert("shared", write_pack(character().into_iter().take(2).collect()).unwrap());
    assert_eq!(server.remount("shared").unwrap(), RemountReport { changed: 0, removed: 1, reloading: 0 });
    assert_eq!(server.take_changes(), [AssetChange::Removed { id: id(MESH), kind: AssetKind::Mesh }]);
    assert_eq!(server.state(&mesh), LoadState::Ready);
    assert!(!server.contains(id(MESH)));
    // A new renderer can't get it: its bytes went with the GPU upload. The material kept its own.
    server.reupload_all();
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    server.finish(&material, |_| Ok(())).unwrap();
    assert_eq!(server.state(&mesh), LoadState::Failed(LoadError::Removed(id(MESH))));

    // Back in its pack, a mesh still in use loads again.
    source.insert("shared", write_pack(character()).unwrap());
    assert_eq!(server.remount("shared").unwrap(), RemountReport { changed: 1, removed: 0, reloading: 1 });
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert_eq!(uploads, [(AssetKind::Mesh, id(MESH), 0)]);
    assert_eq!(server.state(&mesh), LoadState::Ready);
}

#[test]
fn changed_assets_nothing_uses_are_unloaded_and_loading_ones_start_again() {
    let (mut server, source) = serve(textures(2), unlimited());
    let cached: Handle<TextureAsset> = server.load(id(1)).unwrap();
    server.finish(&cached, |_| Ok(())).unwrap();
    drop(cached);
    server.update();
    // Asked for and not yet on the GPU: read, perhaps decoded.
    let loading: Handle<TextureAsset> = server.load(id(2)).unwrap();
    source.insert(
        "shared",
        write_pack(
            (1..=2).map(|n| packed(id(n), AssetKind::Texture, &texture_bytes(8, 8, 40 + n))).collect(),
        )
        .unwrap(),
    );
    assert_eq!(server.remount("shared").unwrap(), RemountReport { changed: 2, removed: 0, reloading: 1 });
    let unloaded: Vec<AssetId> = server.take_unloaded().iter().map(|unloaded| unloaded.id).collect();
    assert_eq!(unloaded, [id(1)], "the cache's copy is stale: the next load reads the new one");
    server
        .finish(&loading, |upload| {
            let texture = upload.get::<TextureAsset>().unwrap();
            assert_eq!(
                (texture.levels[0][0], upload.generation()),
                (42, 0),
                "the new bytes, as a first load"
            );
            Ok(())
        })
        .unwrap();
    assert!(server.take_changes().is_empty(), "nothing loaded was replaced");
}

#[test]
fn rigs_reload_without_the_gpu() {
    let rig = |joints| CharacterRig { joint_count: joints, ..CharacterRig::default() }.to_cooked().unwrap();
    let (mut server, source) = serve(vec![packed(id(20), AssetKind::Rig, &rig(2))], unlimited());
    let handle: Handle<CharacterRig> = server.load(id(20)).unwrap();
    server.finish(&handle, |_| Ok(())).unwrap();
    source.insert("shared", write_pack(vec![packed(id(20), AssetKind::Rig, &rig(3))]).unwrap());
    server.remount("shared").unwrap();
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert!(uploads.is_empty());
    assert_eq!((server.get(&handle).unwrap().joint_count.to_native(), server.generation(&handle)), (3, 1));
    assert!(matches!(&server.take_changes()[..], [AssetChange::Reloaded { generation: 1, .. }]));
}

#[test]
fn a_new_renderer_gets_the_new_version_of_an_asset_reloading() {
    let (mut server, source) = serve(character(), unlimited());
    let (mesh, _) = loaded_character(&mut server);
    source.insert("shared", character_pack(9, &same_material()));
    server.remount("shared").unwrap();
    // Device loss before the new texture reached the GPU: the new renderer gets it, and the
    // material that binds it comes as a new generation, for an app keeping its old renderer.
    server.reupload_all();
    let mut uploads = Vec::new();
    server
        .finish(&mesh, |upload| {
            if let Some(texture) = upload.get::<TextureAsset>() {
                assert_eq!(texture.levels[0][0], 9);
            }
            uploads.push((upload.kind(), upload.id(), upload.generation()));
            Ok(())
        })
        .unwrap();
    assert_eq!(
        uploads,
        [
            (AssetKind::Texture, id(TEXTURE), 1),
            (AssetKind::Material, id(MATERIAL), 1),
            (AssetKind::Mesh, id(MESH), 0)
        ]
    );
    assert_eq!(server.take_changes().len(), 2);
    settle(&mut server, |_| panic!("nothing more to upload"));
}

#[test]
fn remounting_checks_the_pack_and_keeps_the_mounted_one_on_failure() {
    let (mut server, source) = serve(character(), unlimited());
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    server.finish(&texture, |_| Ok(())).unwrap();
    assert!(matches!(server.remount("zone"), Err(MountError::NotMounted(_))));
    // The same pack again changes nothing.
    source.insert("shared", write_pack(character()).unwrap());
    assert_eq!(server.remount("shared").unwrap(), RemountReport::default());

    // A corrupt pack isn't mounted; the mounted one still loads.
    let mut corrupt = character_pack(9, &same_material());
    corrupt[0] = b'X';
    source.insert("shared", corrupt);
    assert!(matches!(server.remount("shared"), Err(MountError::Pack { source: PackError::NotAPack, .. })));
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    server.finish(&mesh, |_| Ok(())).unwrap();

    // Nor one that holds another mounted pack's asset.
    source.insert(
        "zone",
        write_pack(vec![packed(id(9), AssetKind::Texture, &texture_bytes(4, 4, 1))]).unwrap(),
    );
    server.mount("zone").unwrap();
    let mut assets = character();
    assets.push(packed(id(9), AssetKind::Texture, &texture_bytes(4, 4, 2)));
    source.insert("shared", write_pack(assets).unwrap());
    assert!(matches!(server.remount("shared"), Err(MountError::Duplicate { .. })));
    assert!(server.take_changes().is_empty());
    assert_eq!(server.generation(&texture), 0);
}

/// `bytes`, a pack, with the first block of asset `id` corrupted.
fn corrupt(mut bytes: Vec<u8>, id: AssetId) -> Vec<u8> {
    let reader = PackReader::new(&bytes).unwrap();
    let block = reader.toc().blocks(reader.toc().find(id).unwrap())[0];
    bytes[block.offset as usize] ^= 1;
    bytes
}

/// Decodes everything in flight without uploading anything.
fn decode_all(server: &mut AssetServer) {
    while server.stats().reads_in_flight > 0 {
        server.update();
        server.wait(Duration::from_millis(50));
    }
}

#[test]
fn a_new_dependency_that_fails_fails_every_new_version_waiting_on_it() {
    // Two materials, edited in one cook to sample a new texture whose block is corrupt.
    let other = id(5);
    let (mut server, source) = serve(
        vec![
            packed(id(TEXTURE), AssetKind::Texture, &texture_bytes(4, 4, 7)),
            packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(TEXTURE)))),
            packed(other, AssetKind::Material, &material_bytes(Some(id(TEXTURE)))),
        ],
        unlimited(),
    );
    let materials: [Handle<Material>; 2] = [id(MATERIAL), other].map(|n| server.load(n).unwrap());
    for material in &materials {
        server.finish(material, |_| Ok(())).unwrap();
    }
    let pack = write_pack(vec![
        packed(id(TEXTURE), AssetKind::Texture, &texture_bytes(4, 4, 7)),
        packed(id(4), AssetKind::Texture, &texture_bytes(4, 4, 9)),
        packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(4)))),
        packed(other, AssetKind::Material, &material_bytes(Some(id(4)))),
    ])
    .unwrap();
    source.insert("shared", corrupt(pack, id(4)));
    assert_eq!(server.remount("shared").unwrap().reloading, 2);
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert!(uploads.is_empty());
    for material in &materials {
        assert_eq!(server.get(material).unwrap().textures[0].texture(), Some(id(TEXTURE)), "the old version");
    }
    let failed = server.take_changes();
    assert_eq!(failed.len(), 2, "{failed:?}");
    assert!(
        failed
            .iter()
            .all(|change| matches!(change, AssetChange::ReloadFailed { kind: AssetKind::Material, .. }))
    );
}

#[test]
fn a_failed_asset_retried_by_a_remount_finds_its_dependencies_in_the_new_pack() {
    // The character fails to load (a corrupt texture) in a pack that holds more assets, sorted
    // first; the next pack fixes the texture and holds fewer, so every entry moves.
    let extra = |n: u8| AssetId::from_bytes([0, n, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let mut assets = character();
    assets.extend((1..=5).map(|n| packed(extra(n), AssetKind::Texture, &texture_bytes(4, 4, n))));
    let source = Arc::new(MemoryPackSource::new());
    source.insert("shared", corrupt(write_pack(assets).unwrap(), id(TEXTURE)));
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount("shared").unwrap();
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    assert!(server.finish(&mesh, |_| Ok(())).is_err());
    source.insert("shared", write_pack(character()).unwrap());
    assert_eq!(server.remount("shared").unwrap().removed, 5);
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert_eq!(server.state(&mesh), LoadState::Ready);
    assert_eq!(uploads.len(), 3, "{uploads:?}");
}

#[test]
fn a_new_version_waiting_on_one_that_is_dropped_goes_on() {
    let (mut server, source) = serve(character(), unlimited());
    let (_mesh, _) = loaded_character(&mut server);
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    // The texture and the material change; the material's new version waits for the
    // texture's, which is decoded but not yet on the GPU...
    source.insert("shared", character_pack(9, &tinted_material()));
    server.remount("shared").unwrap();
    decode_all(&mut server);
    // ...when the texture's edit is undone. The material's goes on with the texture it has.
    source.insert("shared", character_pack(7, &tinted_material()));
    assert_eq!(server.remount("shared").unwrap().reloading, 1);
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert_eq!(uploads, [(AssetKind::Material, id(MATERIAL), 1)]);
    let tinted = tinted_material();
    assert_eq!(server.get(&material).unwrap().params(), Material::read(&aligned(&tinted)).unwrap().params());
}

#[test]
fn an_asset_sent_again_to_a_new_renderer_that_changes_is_a_new_generation() {
    let (mut server, source) = serve(character(), unlimited());
    let (mesh, _) = loaded_character(&mut server);
    // A new renderer, then the texture changes before it has everything again.
    server.reupload_all();
    source.insert("shared", character_pack(9, &same_material()));
    server.remount("shared").unwrap();
    let mut uploads = Vec::new();
    server.finish(&mesh, recording(&mut uploads)).unwrap();
    assert_eq!(
        uploads,
        [
            (AssetKind::Texture, id(TEXTURE), 1),
            (AssetKind::Material, id(MATERIAL), 1),
            (AssetKind::Mesh, id(MESH), 0)
        ],
        "an app keeping its old renderer replaces the texture and rebinds the material"
    );
}

/// `bytes` copied to 16-byte alignment, as cooked materials are read.
fn aligned(bytes: &[u8]) -> AlignedVec {
    let mut aligned = AlignedVec::new();
    aligned.extend_from_slice(bytes);
    aligned
}
