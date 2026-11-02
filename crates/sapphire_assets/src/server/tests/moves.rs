//! Assets that move between mounted packs, and the failed assets in other packs that depend on
//! them (review AS-2, AS-3).

use super::*;

const A: &str = "a";
const B: &str = "b";

/// What an uploader saw: each asset's ID and generation.
type Uploaded = Vec<(AssetId, u32)>;

/// An uploader that accepts everything and records what it saw.
fn recording(uploads: &mut Uploaded) -> impl FnMut(Upload<'_>) -> Result<(), UploadError> + '_ {
    |upload| {
        uploads.push((upload.id(), upload.generation()));
        Ok(())
    }
}

/// A pack of `assets`.
fn pack(assets: Vec<PackAsset>) -> Vec<u8> {
    write_pack(assets).unwrap()
}

/// A cooked texture, `id(n)`, every byte `fill`.
fn texture(n: u8, fill: u8) -> PackAsset {
    packed(id(n), AssetKind::Texture, &texture_bytes(4, 4, fill))
}

/// A server with packs `a` and `b` mounted, and the source it reads.
fn serve_two(a: Vec<PackAsset>, b: Vec<PackAsset>) -> (AssetServer, Arc<MemoryPackSource>) {
    let source = Arc::new(MemoryPackSource::new());
    source.insert(A, pack(a));
    source.insert(B, pack(b));
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount(A).unwrap();
    server.mount(B).unwrap();
    (server, source)
}

/// Runs frames until nothing is read, decoded, waiting or queued.
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
    panic!("the server never settled: {:?}", server.stats());
}

/// Texture 1, in pack `a`, ready behind the handle returned; pack `a` also holds texture 5 and
/// pack `b` texture 6, which stay there.
fn loaded_in_a() -> (AssetServer, Arc<MemoryPackSource>, Handle<TextureAsset>) {
    let (mut server, source) = serve_two(vec![texture(1, 7), texture(5, 5)], vec![texture(6, 6)]);
    let handle: Handle<TextureAsset> = server.load(id(1)).unwrap();
    server.finish(&handle, |_| Ok(())).unwrap();
    (server, source, handle)
}

/// [`loaded_in_a`], after a remount of pack `a` without texture 1.
fn removed_from_a() -> (AssetServer, Arc<MemoryPackSource>, Handle<TextureAsset>) {
    let (mut server, source, handle) = loaded_in_a();
    source.insert(A, pack(vec![texture(5, 5)]));
    assert_eq!(server.remount(A).unwrap(), RemountReport { changed: 0, removed: 1, reloading: 0 });
    assert_eq!(server.take_changes(), [AssetChange::Removed { id: id(1), kind: AssetKind::Texture }]);
    assert!(!server.contains(id(1)));
    (server, source, handle)
}

#[test]
fn an_asset_moving_between_packs_follows_when_the_pack_it_left_is_remounted_first() {
    let (mut server, source, handle) = removed_from_a();

    // The same bytes in the other pack: nothing to reload, and nothing is sent.
    source.insert(B, pack(vec![texture(1, 7), texture(6, 6)]));
    assert_eq!(server.remount(B).unwrap(), RemountReport { changed: 1, removed: 0, reloading: 0 });
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert!(uploads.is_empty(), "{uploads:?}");
    assert_eq!((server.state(&handle), server.generation(&handle)), (LoadState::Ready, 0));
    assert!(server.take_changes().is_empty());

    // Edits in its new pack reach it, as the next generation for the same handle.
    source.insert(B, pack(vec![texture(1, 9), texture(6, 6)]));
    assert_eq!(server.remount(B).unwrap(), RemountReport { changed: 1, removed: 0, reloading: 1 });
    settle(&mut server, recording(&mut uploads));
    assert_eq!(uploads, [(id(1), 1)]);
    assert_eq!(server.generation(&handle), 1);

    // A new renderer reads it from its new pack.
    server.reupload_all();
    let mut read = Vec::new();
    server
        .finish(&handle, |upload| {
            read.push((upload.get::<TextureAsset>().unwrap().levels[0][0], upload.generation()));
            Ok(())
        })
        .unwrap();
    assert_eq!(read, [(9, 1)]);
}

#[test]
fn a_remount_that_would_hold_an_asset_its_source_pack_still_holds_is_refused() {
    let (mut server, source, handle) = loaded_in_a();
    source.insert(A, pack(vec![texture(5, 5)]));
    source.insert(B, pack(vec![texture(1, 9), texture(6, 6)]));

    // The pack it moves to first: refused, naming both packs, and nothing changes.
    let error = server.remount(B).unwrap_err();
    assert!(
        matches!(&error, MountError::Duplicate { id: duplicate, first, second }
            if *duplicate == id(1) && first == A && second == B),
        "{error}"
    );
    assert!(server.take_changes().is_empty());
    assert_eq!((server.state(&handle), server.generation(&handle)), (LoadState::Ready, 0));

    // Then the order `remount` documents: the pack it left, then the pack it moved to.
    server.remount(A).unwrap();
    assert_eq!(server.remount(B).unwrap().reloading, 1);
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert_eq!(uploads, [(id(1), 1)]);
}

#[test]
fn a_newly_mounted_pack_takes_over_the_assets_it_holds_that_lost_their_pack() {
    // Ready, with the version it had: a new renderer reads it from the new pack.
    let (mut server, source, handle) = removed_from_a();
    source.insert("c", pack(vec![texture(1, 7)]));
    server.mount("c").unwrap();
    server.reupload_all();
    let mut uploads = Vec::new();
    server.finish(&handle, recording(&mut uploads)).unwrap();
    assert_eq!(uploads, [(id(1), 0)]);

    // Failed, because a new renderer couldn't get it while no pack held it: it loads again.
    let (mut server, source, handle) = removed_from_a();
    server.reupload_all();
    settle(&mut server, |_| Ok(()));
    assert_eq!(server.state(&handle), LoadState::Failed(LoadError::Removed(id(1))));
    source.insert("c", pack(vec![texture(1, 7)]));
    server.mount("c").unwrap();
    assert_eq!(server.state(&handle), LoadState::Loading, "something still wants it");
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert_eq!(uploads, [(id(1), 0)]);
    assert_eq!(server.state(&handle), LoadState::Ready);

    // With other bytes, it reloads behind its handle.
    let (mut server, source, handle) = removed_from_a();
    source.insert("c", pack(vec![texture(1, 9)]));
    server.mount("c").unwrap();
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert_eq!(uploads, [(id(1), 1)]);
    assert_eq!(server.generation(&handle), 1);
}

#[test]
fn an_asset_moved_with_other_bytes_that_nothing_uses_is_unloaded() {
    let (mut server, source) = serve_two(vec![texture(1, 7), texture(5, 5)], vec![texture(6, 6)]);
    let handle: Handle<TextureAsset> = server.load(id(1)).unwrap();
    let mut slot = None;
    server
        .finish(&handle, |upload| {
            slot = Some(upload.slot());
            Ok(())
        })
        .unwrap();
    drop(handle);
    source.insert(A, pack(vec![texture(5, 5)]));
    server.remount(A).unwrap();
    source.insert(B, pack(vec![texture(1, 9), texture(6, 6)]));
    assert_eq!(server.remount(B).unwrap().reloading, 0);
    assert_eq!(
        server.take_unloaded(),
        [Unloaded { id: id(1), kind: AssetKind::Texture, slot: slot.unwrap() }],
        "the cache's copy is stale"
    );
    // The next load reads the new one, as a first load in a new slot.
    let handle: Handle<TextureAsset> = server.load(id(1)).unwrap();
    let mut read = Vec::new();
    server
        .finish(&handle, |upload| {
            read.push((upload.get::<TextureAsset>().unwrap().levels[0][0], upload.generation()));
            Ok(())
        })
        .unwrap();
    assert_eq!(read, [(9, 0)]);
}

#[test]
fn a_moved_asset_stays_under_residency() {
    let (mut server, source, handle) = removed_from_a();
    source.insert(B, pack(vec![texture(1, 7), texture(6, 6)]));
    server.remount(B).unwrap();
    server.update();
    server.unload_unused();
    assert_eq!(server.stats().assets, 1, "it is in use");
    drop(handle);
    server.update();
    server.unload_unused();
    assert_eq!(server.stats().assets, 0);
    let unloaded: Vec<AssetId> = server.take_unloaded().iter().map(|unloaded| unloaded.id).collect();
    assert_eq!(unloaded, [id(1)]);
}

#[test]
fn an_asset_of_another_kind_under_the_same_id_takes_nothing_over() {
    let (mut server, source, handle) = removed_from_a();
    source.insert("c", pack(vec![packed(id(1), AssetKind::Material, &material_bytes(None))]));
    server.mount("c").unwrap();
    assert!(server.take_changes().is_empty(), "already reported as removed");
    assert_eq!((server.state(&handle), server.generation(&handle)), (LoadState::Ready, 0));
    // Still no pack holds the texture, so a new renderer can't get it.
    server.reupload_all();
    assert_eq!(server.state(&handle), LoadState::Failed(LoadError::Removed(id(1))));
}

/// A mesh in pack `a`, on a material in pack `a`, on texture 1 in pack `b`, which fail together:
/// the texture's upload is refused.
fn failed_across_packs() -> (AssetServer, Arc<MemoryPackSource>, Handle<Mesh>, Handle<Material>) {
    let (mut server, source) = serve_two(
        vec![
            packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(1)))),
            packed(id(MESH), AssetKind::Mesh, &mesh_bytes(id(MATERIAL))),
        ],
        vec![texture(1, 7)],
    );
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    assert!(matches!(
        server.finish(&mesh, |_| Err("out of memory".into())),
        Err(LoadError::Dependency { .. })
    ));
    assert!(matches!(server.state(&material), LoadState::Failed(LoadError::Dependency { .. })));
    (server, source, mesh, material)
}

#[test]
fn failed_parents_in_other_packs_load_again_when_the_pack_of_what_they_need_is_fixed() {
    let (mut server, source, mesh, material) = failed_across_packs();
    source.insert(B, pack(vec![texture(1, 9)]));
    // The texture, its material and the mesh on that.
    assert_eq!(server.remount(B).unwrap(), RemountReport { changed: 1, removed: 0, reloading: 3 });
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert_eq!(uploads, [(id(1), 0), (id(MATERIAL), 0), (id(MESH), 0)]);
    assert_eq!((server.state(&mesh), server.state(&material)), (LoadState::Ready, LoadState::Ready));
}

#[test]
fn a_failed_parent_nothing_uses_waits_to_be_loaded_again() {
    let (mut server, source, mesh, material) = failed_across_packs();
    drop((mesh, material));
    // The mesh goes with its last user. The material, which it held, stays until the next update.
    server.update();
    assert_eq!(server.stats().assets, 2);
    source.insert(B, pack(vec![texture(1, 9)]));
    assert_eq!(server.remount(B).unwrap().reloading, 1, "the texture the material holds, not the material");
    settle(&mut server, |_| Ok(()));
    assert_eq!(server.stats().assets, 1, "the failed material went");
    // A load asks for all of it again.
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    server.finish(&mesh, |_| Ok(())).unwrap();
}

#[test]
fn a_failed_parent_reached_through_one_a_remount_retried_loads_again() {
    // Two meshes on one material, on a texture: mesh 3 in pack `b`, with the texture, so the
    // remount itself retries it, which retries the material; mesh 4 in pack `a`, with the material.
    let (mut server, source) = serve_two(
        vec![
            packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(1)))),
            packed(id(4), AssetKind::Mesh, &mesh_bytes(id(MATERIAL))),
        ],
        vec![texture(1, 7), packed(id(3), AssetKind::Mesh, &mesh_bytes(id(MATERIAL)))],
    );
    let meshes: [Handle<Mesh>; 2] = [id(3), id(4)].map(|n| server.load(n).unwrap());
    settle(&mut server, |_| Err("out of memory".into()));
    assert_eq!(server.stats().failed, 4);
    source.insert(B, pack(vec![texture(1, 9), packed(id(3), AssetKind::Mesh, &mesh_bytes(id(MATERIAL)))]));
    server.remount(B).unwrap();
    settle(&mut server, |_| Ok(()));
    for mesh in &meshes {
        assert_eq!(server.state(mesh), LoadState::Ready);
    }
}

#[test]
fn a_parent_that_failed_for_a_dependency_no_pack_held_loads_when_one_does() {
    let material_pack = || vec![packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(1))))];
    let (mut server, source) = serve_two(material_pack(), vec![texture(6, 6)]);
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    let error = LoadError::Dependency { id: id(MATERIAL), dependency: id(1) };
    assert_eq!(server.state(&material), LoadState::Failed(error));

    // By a remount of another pack...
    source.insert(B, pack(vec![texture(1, 7), texture(6, 6)]));
    assert_eq!(server.remount(B).unwrap().reloading, 1);
    let mut uploads = Vec::new();
    settle(&mut server, recording(&mut uploads));
    assert_eq!(uploads, [(id(1), 0), (id(MATERIAL), 0)]);
    assert_eq!(server.state(&material), LoadState::Ready);

    // ...or a new pack.
    let source = Arc::new(MemoryPackSource::new());
    source.insert(A, pack(material_pack()));
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount(A).unwrap();
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    assert!(matches!(server.state(&material), LoadState::Failed(_)));
    source.insert(B, pack(vec![texture(1, 7)]));
    server.mount(B).unwrap();
    settle(&mut server, |_| Ok(()));
    assert_eq!(server.state(&material), LoadState::Ready);
}

#[test]
fn a_parent_that_failed_for_itself_or_for_another_pack_is_left_alone() {
    // Its own upload was refused: nothing in the other pack could have caused that.
    let material_pack = || vec![packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(1))))];
    let (mut server, source) = serve_two(material_pack(), vec![texture(1, 7)]);
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    let refuse = |upload: Upload<'_>| match upload.kind() {
        AssetKind::Material => Err("out of memory".into()),
        _ => Ok(()),
    };
    assert!(matches!(server.finish(&material, refuse), Err(LoadError::Upload { .. })));
    source.insert(B, pack(vec![texture(1, 9)]));
    assert_eq!(server.remount(B).unwrap().reloading, 1, "the texture, in use and changed");
    settle(&mut server, |_| Ok(()));
    assert!(matches!(server.state(&material), LoadState::Failed(LoadError::Upload { .. })));
    // A load still tries it again.
    server.load::<Material>(id(MATERIAL)).unwrap();
    server.finish(&material, |_| Ok(())).unwrap();

    // Its dependency in a third pack is what failed, and stays failed.
    let (mut server, source) = serve_two(material_pack(), vec![texture(6, 6)]);
    source.insert("c", pack(vec![texture(1, 7)]));
    server.mount("c").unwrap();
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    assert!(server.finish(&material, |_| Err("out of memory".into())).is_err());
    source.insert(B, pack(vec![texture(6, 9)]));
    assert_eq!(server.remount(B).unwrap().reloading, 0);
    assert!(matches!(server.state(&material), LoadState::Failed(LoadError::Dependency { .. })));
}
