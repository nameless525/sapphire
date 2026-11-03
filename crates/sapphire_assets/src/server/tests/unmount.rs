//! Packs coming and going: a mount that waits on nothing, and what a handle does after an unmount, on the
//! paths a desktop's disk is too fast to show.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use sapphire_time::Stopwatch;

use super::lifetime::CountedSource;
use super::*;

/// A server with `character()` in a pack called `zone` on a source that counts its files.
fn counted() -> (AssetServer, Arc<CountedSource>) {
    let source = Arc::new(CountedSource::default());
    source.inner.insert("zone", write_pack(character()).unwrap());
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount_now("zone").unwrap();
    (server, source)
}

fn accept(_: Upload<'_>) -> Result<(), UploadError> {
    Ok(())
}

/// Runs the pipeline until every read and decode has come back and nothing is left to upload.
fn drain(server: &mut AssetServer) {
    let start = Stopwatch::start();
    loop {
        server.update();
        let report = server.upload(accept);
        if server.stats().reads_in_flight == 0 && report.pending == 0 && report.assets == 0 {
            return;
        }
        server.wait(Duration::from_millis(5));
        assert!(start.elapsed() < Duration::from_secs(30), "the pipeline stalled: {:?}", server.stats());
    }
}

/// Runs `update` until the pack called `name` isn't mounting any more.
fn wait_for_mount(server: &mut AssetServer, name: &str) {
    let start = Stopwatch::start();
    while matches!(server.mount_state(name), MountState::Mounting) {
        server.wait(Duration::from_millis(5));
        assert!(start.elapsed() < Duration::from_secs(30), "the mount stalled");
    }
}

fn unloaded(server: &mut AssetServer) -> Vec<AssetId> {
    server.take_unloaded().into_iter().map(|unloaded| unloaded.id).collect()
}

// Mounting.

/// The bar `mount` has to meet, in the only terms that fail if it ever waits again: called with the source
/// holding every read back, it returns. Before W2 it blocked on a channel for the header's read, so here
/// the thread running it would never send the server back.
#[test]
fn a_mount_returns_while_its_pack_cant_be_read() {
    let source = Arc::new(CountedSource::default());
    source.inner.insert("zone", write_pack(character()).unwrap());
    source.hold.store(true, Ordering::SeqCst);
    let mut server = AssetServer::new(source.clone(), unlimited());

    let (sender, receiver) = mpsc::channel();
    let frame = std::thread::spawn(move || {
        let result = server.mount("zone");
        sender.send((server, result)).unwrap();
    });
    let returned = receiver.recv_timeout(Duration::from_secs(10));
    // Let go either way, so a mount that waited ends with the test rather than holding a thread.
    source.release();
    frame.join().unwrap();
    let (mut server, result) = returned.expect("mount waited for a read");
    result.unwrap();

    wait_for_mount(&mut server, "zone");
    assert!(matches!(server.mount_state("zone"), MountState::Mounted));
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    server.finish(&mesh, accept).unwrap();
}

/// Opening a file is on the worker too: on Android it goes through an asset stream.
#[test]
fn a_mount_returns_while_its_pack_cant_be_opened() {
    let source = Arc::new(CountedSource::default());
    source.inner.insert("zone", write_pack(character()).unwrap());
    source.hold_open.store(true, Ordering::SeqCst);
    let mut server = AssetServer::new(source.clone(), unlimited());
    let (sender, receiver) = mpsc::channel();
    let frame = std::thread::spawn(move || {
        let result = server.mount("zone");
        sender.send((server, result)).unwrap();
    });
    let returned = receiver.recv_timeout(Duration::from_secs(10));
    source.hold_open.store(false, Ordering::SeqCst);
    frame.join().unwrap();
    let (mut server, result) = returned.expect("mount waited for the file to open");
    result.unwrap();
    wait_for_mount(&mut server, "zone");
    assert!(matches!(server.mount_state("zone"), MountState::Mounted));
}

#[test]
fn a_pack_still_mounting_holds_nothing_loadable_and_frames_go_on() {
    let source = Arc::new(CountedSource::default());
    source.inner.insert("zone", write_pack(character()).unwrap());
    source.hold.store(true, Ordering::SeqCst);
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount("zone").unwrap();

    assert!(matches!(server.mount_state("zone"), MountState::Mounting));
    assert_eq!(server.stats().mounting, 1);
    assert!(!server.is_mounted("zone"));
    assert!(!server.contains(id(MESH)));
    assert_eq!(server.load::<Mesh>(id(MESH)).unwrap_err(), LoadError::NotFound(id(MESH)));
    assert!(matches!(server.mount("zone"), Err(MountError::AlreadyMounted(_))));
    assert!(matches!(server.remount("zone"), Err(MountError::NotMounted(_))));
    for _ in 0..3 {
        server.update();
        server.upload(accept);
    }

    source.release();
    wait_for_mount(&mut server, "zone");
    assert!(server.contains(id(MESH)));
    let stats = server.stats();
    assert_eq!((stats.mounts, stats.mounting, stats.reads_in_flight), (1, 0, 0));
}

#[test]
fn a_mount_that_fails_says_why_and_can_be_tried_again() {
    let source = Arc::new(CountedSource::default());
    let mut server = AssetServer::new(source.clone(), unlimited());

    // No such file.
    server.mount("zone").unwrap();
    wait_for_mount(&mut server, "zone");
    assert!(matches!(server.mount_state("zone"), MountState::Failed(MountError::Io { .. })));
    assert_eq!(source.open_files(), 0);

    // Not a pack: the file it opened closes.
    source.inner.insert("zone", vec![7; 256]);
    server.mount("zone").unwrap();
    assert!(matches!(server.mount_state("zone"), MountState::Mounting), "the last failure is forgotten");
    wait_for_mount(&mut server, "zone");
    assert!(matches!(server.mount_state("zone"), MountState::Failed(MountError::Pack { .. })));
    assert_eq!(source.open_files(), 0);

    // An unmount forgets a failure as well.
    server.unmount("zone").unwrap();
    assert!(matches!(server.mount_state("zone"), MountState::Unmounted));
    assert!(matches!(server.unmount("zone"), Err(MountError::NotMounted(_))));

    // Good bytes at last.
    source.inner.insert("zone", write_pack(character()).unwrap());
    server.mount_now("zone").unwrap();
    assert!(matches!(server.mount_state("zone"), MountState::Mounted));
    assert_eq!(source.open_files(), 1);
    assert!(matches!(server.mount_now("zone"), Err(MountError::AlreadyMounted(_))));
}

#[test]
fn a_mount_holding_another_packs_asset_fails_when_it_lands_and_closes_its_file() {
    let (mut server, source) = counted();
    source.inner.insert(
        "other",
        write_pack(vec![packed(id(MESH), AssetKind::Mesh, &mesh_bytes(id(MATERIAL)))]).unwrap(),
    );
    server.mount("other").unwrap();
    wait_for_mount(&mut server, "other");
    assert!(matches!(server.mount_state("other"), MountState::Failed(MountError::Duplicate { .. })));
    assert_eq!(source.open_files(), 1);
    // Waiting says the same.
    assert!(matches!(server.mount_now("other"), Err(MountError::Duplicate { .. })));
    assert!(matches!(server.mount_state("other"), MountState::Unmounted), "the caller was told");
}

#[test]
fn unmounting_a_pack_still_mounting_cancels_it_whatever_the_worker_is_doing() {
    let source = Arc::new(CountedSource::default());
    source.inner.insert("zone", write_pack(character()).unwrap());
    source.hold.store(true, Ordering::SeqCst);
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount("zone").unwrap();
    assert_eq!(server.unmount("zone").unwrap(), UnmountReport::default());
    assert!(matches!(server.mount_state("zone"), MountState::Unmounted));

    // A second try while the first's worker still waits: only it counts.
    server.mount("zone").unwrap();
    source.release();
    wait_for_mount(&mut server, "zone");
    drain(&mut server);
    assert!(matches!(server.mount_state("zone"), MountState::Mounted));
    assert_eq!(source.open_files(), 1, "what the cancelled mount opened is closed");
    assert_eq!(server.stats().mounts, 1);
    drop(server);
    assert_eq!(source.open_files(), 0);
    assert_eq!(*source.bad_closes.lock().unwrap(), 0);
}

#[test]
fn a_server_dropped_with_a_mount_under_way_leaves_nothing_open() {
    let source = Arc::new(CountedSource::default());
    source.inner.insert("zone", write_pack(character()).unwrap());
    source.hold.store(true, Ordering::SeqCst);
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount("zone").unwrap();
    drop(server);
    source.release();
    let start = Stopwatch::start();
    while *source.opened.lock().unwrap() == 0 || source.open_files() > 0 {
        std::thread::sleep(Duration::from_millis(1));
        source.release();
        assert!(start.elapsed() < Duration::from_secs(30), "the worker left a file open");
    }
}

#[test]
fn a_worker_opening_many_packs_at_once_mounts_each() {
    let source = Arc::new(MemoryPackSource::new());
    let names: Vec<String> = (0..8).map(|n| format!("zone_{n}")).collect();
    for (n, name) in names.iter().enumerate() {
        let texture = packed(id(100 + n as u8), AssetKind::Texture, &texture_bytes(4, 4, n as u8));
        source.insert(name, write_pack(vec![texture]).unwrap());
    }
    let mut server = AssetServer::new(source, unlimited());
    for name in &names {
        server.mount(name).unwrap();
    }
    for (n, name) in names.iter().enumerate() {
        wait_for_mount(&mut server, name);
        assert!(server.contains(id(100 + n as u8)));
    }
}

// Unmounting: what's unused goes.

#[test]
fn an_asset_nothing_uses_goes_with_its_pack_and_a_weak_handle_finds_nothing() {
    let (mut server, source) = counted();
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    server.finish(&mesh, accept).unwrap();
    let weak = mesh.downgrade();
    drop(mesh);
    server.update();
    assert_eq!(server.stats().assets, 3, "cached, as anything is while its pack stays");

    let report = server.unmount("zone").unwrap();
    assert_eq!(report, UnmountReport { unloaded: 3, held: 0, cancelled: 0 });
    let gone = unloaded(&mut server);
    assert!(
        gone.contains(&id(MESH)) && gone.contains(&id(MATERIAL)) && gone.contains(&id(TEXTURE)),
        "{gone:?}"
    );
    assert_eq!(server.stats().assets, 0);
    assert!(server.upgrade(weak).is_none());
    assert!(!server.contains(id(MESH)));
    assert_eq!(server.load::<Mesh>(id(MESH)).unwrap_err(), LoadError::NotFound(id(MESH)));
    assert_eq!(source.open_files(), 0, "its file closed");
    assert!(matches!(server.unmount("zone"), Err(MountError::NotMounted(_))));
    assert!(matches!(server.remount("zone"), Err(MountError::NotMounted(_))));
}

/// What a mount was addressed by before W2 was its place in a vector: this is the bug a removal from one
/// would have been.
#[test]
fn the_packs_left_behind_stay_where_their_assets_find_them() {
    let source = Arc::new(MemoryPackSource::new());
    let pack_of =
        |n: u8| write_pack(vec![packed(id(n), AssetKind::Texture, &texture_bytes(4, 4, n))]).unwrap();
    for (name, n) in [("a", 1), ("b", 2), ("c", 3)] {
        source.insert(name, pack_of(n));
    }
    let mut server = AssetServer::new(source.clone(), unlimited());
    for name in ["a", "b", "c"] {
        server.mount_now(name).unwrap();
    }
    server.unmount("a").unwrap();
    // A pack mounted now mustn't be taken for the one that left, by an asset or by the index.
    source.insert("d", pack_of(4));
    server.mount_now("d").unwrap();
    for (n, fill) in [(2, 2), (3, 3), (4, 4)] {
        let texture: Handle<TextureAsset> = server.load(id(n)).unwrap();
        server
            .finish(&texture, |upload| {
                assert_eq!(upload.get::<TextureAsset>().unwrap().levels[0][0], fill);
                Ok(())
            })
            .unwrap();
    }
    server.unmount("c").unwrap();
    assert!(server.contains(id(2)) && server.contains(id(4)));
    assert!(!server.contains(id(1)) && !server.contains(id(3)));
    // And one that left can come back, under a new mount.
    server.mount_now("a").unwrap();
    assert!(server.contains(id(1)));
}

// Unmounting: what's held stays.

#[test]
fn a_handle_that_outlives_the_unmount_keeps_what_it_held_until_it_goes() {
    let (mut server, source) = counted();
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    server.finish(&mesh, accept).unwrap();
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();

    assert_eq!(server.unmount("zone").unwrap(), UnmountReport { unloaded: 0, held: 3, cancelled: 0 });
    assert_eq!(source.open_files(), 0, "the file goes, what's read stays");
    assert!(server.take_unloaded().is_empty());
    assert_eq!(server.state(&mesh), LoadState::Ready);
    assert_eq!(server.state(&material), LoadState::Ready);
    let view = server.get(&material).expect("a material keeps its bytes");
    assert_eq!(view.textures[0].texture(), Some(id(TEXTURE)));
    assert_eq!(server.generation(&mesh), 0);

    // Frames go by with nothing moving, and asking again shares it, with no pack to read.
    for _ in 0..3 {
        server.update();
        server.upload(accept);
    }
    assert!(server.take_unloaded().is_empty());
    let again: Handle<Mesh> = server.load(id(MESH)).unwrap();
    assert_eq!(again, mesh);
    assert!(!server.contains(id(MESH)), "though no mounted pack holds it");
    drop(again);

    // The last user goes: no budget is over, and it unloads anyway, as no pack would bring it back.
    drop(material);
    server.update();
    assert_eq!(server.stats().assets, 3, "the mesh still holds its material");
    let weak = mesh.downgrade();
    drop(mesh);
    let start = Stopwatch::start();
    let mut gone = Vec::new();
    while gone.len() < 3 {
        server.update();
        gone.extend(unloaded(&mut server));
        assert!(start.elapsed() < Duration::from_secs(10), "{gone:?}");
    }
    assert_eq!(server.stats().assets, 0);
    assert!(server.upgrade(weak).is_none());
}

#[test]
fn a_pin_holds_an_asset_past_its_pack_until_it_lets_go() {
    let (mut server, _) = counted();
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    server.finish(&texture, accept).unwrap();
    server.pin(&texture);
    let weak = texture.downgrade();
    drop(texture);
    assert_eq!(server.unmount("zone").unwrap(), UnmountReport { unloaded: 0, held: 1, cancelled: 0 });
    server.update();
    let texture = server.upgrade(weak).expect("pinned");
    server.unpin(&texture);
    drop(texture);
    server.update();
    assert!(server.upgrade(weak).is_none());
    assert_eq!(unloaded(&mut server), [id(TEXTURE)]);
}

#[test]
fn a_ready_parent_in_another_pack_keeps_what_it_binds() {
    let source = Arc::new(MemoryPackSource::new());
    source.insert(
        "textures",
        write_pack(vec![packed(id(TEXTURE), AssetKind::Texture, &texture_bytes(4, 4, 7))]).unwrap(),
    );
    source.insert(
        "materials",
        write_pack(vec![packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(TEXTURE))))])
            .unwrap(),
    );
    let mut server = AssetServer::new(source, unlimited());
    server.mount_now("textures").unwrap();
    server.mount_now("materials").unwrap();
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    server.finish(&material, accept).unwrap();

    assert_eq!(server.unmount("textures").unwrap(), UnmountReport { unloaded: 0, held: 1, cancelled: 0 });
    server.update();
    assert_eq!(server.state(&material), LoadState::Ready);
    assert!(server.take_unloaded().is_empty(), "the material holds its texture");

    // A pack's own unused parent goes, and what it held in another pack stays cached.
    server.mount_now("textures").unwrap();
    drop(material);
    server.update();
    assert_eq!(server.unmount("materials").unwrap().unloaded, 1);
    assert_eq!(unloaded(&mut server), [id(MATERIAL)]);
    server.update();
    server.update();
    assert_eq!(server.stats().assets, 1, "the texture's pack still holds it");
}

// Unmounting: what's on its way stops.

/// The read's queued when the pack goes, and comes back on a closed file. A desktop's disk answers too
/// fast to show this.
#[test]
fn a_load_in_flight_fails_and_its_read_comes_back_to_nothing() {
    let (mut server, source) = counted();
    source.hold.store(true, Ordering::SeqCst);
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    assert_eq!(server.state(&mesh), LoadState::Loading);
    let reads = server.stats().reads_in_flight;
    assert!(reads > 0);

    let report = server.unmount("zone").unwrap();
    assert_eq!(
        report,
        UnmountReport { unloaded: 0, held: 3, cancelled: 3 },
        "held: the handle, and the links"
    );
    assert_eq!(server.state(&mesh), LoadState::Failed(LoadError::Removed(id(MESH))));
    assert_eq!(server.stats().reads_in_flight, reads, "nothing cancels a read: it comes back");

    source.release();
    drain(&mut server);
    assert_eq!(server.stats().reads_in_flight, 0);
    assert_eq!(server.state(&mesh), LoadState::Failed(LoadError::Removed(id(MESH))), "and stays so");

    // A failed handle is safe to hold and to ask about, and trying it again finds nothing to read.
    assert!(server.get(&mesh).is_none());
    let again: Handle<Mesh> = server.load(id(MESH)).unwrap();
    assert_eq!(server.state(&again), LoadState::Failed(LoadError::Removed(id(MESH))));
    drop((mesh, again));
    drain(&mut server);
    server.update();
    server.update();
    assert_eq!(server.stats().assets, 0);
}

/// A load whose read has landed and whose decode may be running or done.
#[test]
fn a_load_being_decoded_never_reaches_the_uploader() {
    for _ in 0..50 {
        let (mut server, _) = counted();
        let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
        // The memory source reads on this thread, so the decode jobs are on their way now.
        server.unmount("zone").unwrap();
        assert_eq!(server.state(&mesh), LoadState::Failed(LoadError::Removed(id(MESH))));
        let start = Stopwatch::start();
        while server.stats().reads_in_flight > 0 {
            server.update();
            let report = server.upload(|upload| panic!("{} went to the uploader", upload.id()));
            assert_eq!(report.assets, 0);
            assert!(start.elapsed() < Duration::from_secs(30));
        }
    }
}

#[test]
fn a_load_queued_for_the_gpu_stays_off_it() {
    let (mut server, _) = counted();
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    let start = Stopwatch::start();
    while server.stats().reads_in_flight > 0 {
        server.wait(Duration::from_millis(5));
        assert!(start.elapsed() < Duration::from_secs(30));
    }
    assert_eq!(server.state(&texture), LoadState::Loading, "decoded, in the queue");

    assert_eq!(server.unmount("zone").unwrap().cancelled, 1);
    let report = server.upload(|upload| panic!("{} went to the uploader", upload.id()));
    assert_eq!((report.assets, report.pending), (0, 0));
    assert_eq!(server.state(&texture), LoadState::Failed(LoadError::Removed(id(TEXTURE))));
}

#[test]
fn what_waits_in_another_pack_fails_with_what_it_waited_for() {
    let source = Arc::new(CountedSource::default());
    source.inner.insert(
        "textures",
        write_pack(vec![packed(id(TEXTURE), AssetKind::Texture, &texture_bytes(4, 4, 7))]).unwrap(),
    );
    source.inner.insert(
        "materials",
        write_pack(vec![packed(id(MATERIAL), AssetKind::Material, &material_bytes(Some(id(TEXTURE))))])
            .unwrap(),
    );
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount_now("textures").unwrap();
    server.mount_now("materials").unwrap();
    source.hold.store(true, Ordering::SeqCst);
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();

    server.unmount("textures").unwrap();
    assert_eq!(
        server.state(&material),
        LoadState::Failed(LoadError::Dependency { id: id(MATERIAL), dependency: id(TEXTURE) })
    );
    source.release();
    drain(&mut server);
    assert!(server.get(&material).is_none());

    // Its pack back, the same handle loads: mounting tries again what failed for want of it.
    server.mount_now("textures").unwrap();
    server.finish(&material, accept).unwrap();
    assert!(server.get(&material).is_some());
}

#[test]
fn a_new_version_on_its_way_is_dropped_and_the_old_one_stays() {
    let (mut server, source) = counted();
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    server.finish(&texture, accept).unwrap();
    let mut assets = character();
    assets[0] = packed(id(TEXTURE), AssetKind::Texture, &texture_bytes(4, 4, 8));
    source.inner.insert("zone", write_pack(assets).unwrap());
    server.remount("zone").unwrap();
    assert_eq!(server.stats().reloading, 1, "the new version is on its way");

    server.unmount("zone").unwrap();
    assert_eq!(server.stats().reloading, 0);
    let start = Stopwatch::start();
    while server.stats().reads_in_flight > 0 {
        server.update();
        let report = server.upload(|upload| panic!("{} went to the uploader", upload.id()));
        assert_eq!(report.assets, 0);
        assert!(start.elapsed() < Duration::from_secs(30));
    }
    assert_eq!(server.state(&texture), LoadState::Ready);
    assert_eq!(server.generation(&texture), 0);
}

// Unmounting: what comes after.

#[test]
fn mounting_again_takes_back_the_same_bytes_with_no_read_and_reloads_others() {
    let (mut server, source) = counted();
    let texture: Handle<TextureAsset> = server.load(id(TEXTURE)).unwrap();
    let mut uploads = 0;
    server
        .finish(&texture, |_| {
            uploads += 1;
            Ok(())
        })
        .unwrap();
    server.unmount("zone").unwrap();
    assert!(!server.contains(id(TEXTURE)));

    // The same bytes: nothing to read, nothing to upload, the same handle.
    server.mount_now("zone").unwrap();
    assert_eq!(server.stats().reads_in_flight, 0);
    assert!(server.contains(id(TEXTURE)));
    assert_eq!(server.state(&texture), LoadState::Ready);
    assert_eq!(uploads, 1);

    // Other bytes come in as a new version behind it, as a remount's would.
    server.unmount("zone").unwrap();
    let mut assets = character();
    assets[0] = packed(id(TEXTURE), AssetKind::Texture, &texture_bytes(4, 4, 9));
    source.inner.insert("zone", write_pack(assets).unwrap());
    server.mount_now("zone").unwrap();
    assert_eq!(server.state(&texture), LoadState::Ready, "it keeps what it has meanwhile");
    drain(&mut server);
    assert_eq!(server.generation(&texture), 1);
}

#[test]
fn a_new_renderer_gets_what_kept_its_bytes_and_a_failure_for_what_didnt() {
    let (mut server, _) = counted();
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    server.finish(&mesh, accept).unwrap();
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    server.unmount("zone").unwrap();

    server.reupload_all();
    // Nothing panics reading a mount that's gone, and each handle says where it stands.
    drain(&mut server);
    assert_eq!(server.state(&mesh), LoadState::Failed(LoadError::Removed(id(MESH))));
    assert!(matches!(server.state(&material), LoadState::Failed(_)), "its texture had nowhere to come from");
}

/// Every order of the above, as a walk through zones would reach them, on a source that answers late.
#[test]
fn any_order_of_mounts_unmounts_and_loads_ends_with_nothing_left_behind() {
    // Three packs, and handles held across whatever happens to them.
    let source = Arc::new(CountedSource::default());
    let packs = ["a", "b", "c"];
    for (p, name) in packs.iter().enumerate() {
        let base = 10 * p as u8 + 10;
        let assets = vec![
            packed(id(base), AssetKind::Texture, &texture_bytes(4, 4, base)),
            packed(id(base + 1), AssetKind::Material, &material_bytes(Some(id(base)))),
            packed(id(base + 2), AssetKind::Mesh, &mesh_bytes(id(base + 1))),
        ];
        source.inner.insert(name, write_pack(assets).unwrap());
    }
    let mut server = AssetServer::new(source.clone(), unlimited());
    let mut held: Vec<Handle<Mesh>> = Vec::new();
    let mut state = 0x1234_5678_u32;
    let mut next = move |below: u32| {
        // xorshift: a test's dice, the same every run.
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state % below
    };
    for _ in 0..4_000 {
        let name = packs[next(3) as usize];
        match next(9) {
            0 => {
                let _ = server.mount(name);
            }
            1 => {
                let _ = server.unmount(name);
            }
            2 | 3 => {
                let mesh = id(10 * next(3) as u8 + 12);
                if let Ok(handle) = server.load::<Mesh>(mesh) {
                    held.push(handle);
                }
            }
            4 if !held.is_empty() => {
                let handle = held.swap_remove(next(held.len() as u32) as usize);
                let _ = server.state(&handle);
                drop(handle);
            }
            5 => source.hold.store(next(2) == 0, Ordering::SeqCst),
            6 => source.release(),
            7 => {
                server.reupload_all();
            }
            _ => {
                server.update();
                server.upload(accept);
                let _ = server.take_unloaded();
            }
        }
        for handle in &held {
            let _ = server.get(handle);
        }
    }
    source.release();
    drain(&mut server);
    drop(held);
    for name in packs {
        let _ = server.unmount(name);
    }
    source.release();
    let start = Stopwatch::start();
    while server.stats().assets > 0 || server.stats().reads_in_flight > 0 {
        server.update();
        server.upload(accept);
        server.wait(Duration::from_millis(1));
        source.release();
        assert!(start.elapsed() < Duration::from_secs(30), "left behind: {:?}", server.stats());
    }
    assert_eq!(source.open_files(), 0);
    assert_eq!(*source.bad_closes.lock().unwrap(), 0);
}
