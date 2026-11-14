//! Load admission (M5 M2): a cap on bytes read or decoded and not yet on the GPU, that never stalls a load
//! behind what it waits for or starves an urgent one behind the rest.

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// The texture numbered `n`, an ID of its own beside the module's small ones.
fn numbered(n: u16) -> AssetId {
    let mut bytes = [0xad; 16];
    bytes[..2].copy_from_slice(&n.to_le_bytes());
    AssetId::from_bytes(bytes)
}

/// `count` textures, `side` texels square, each its own ID.
fn many(count: u16, side: u32) -> Vec<PackAsset> {
    (0..count).map(|n| packed(numbered(n), AssetKind::Texture, &texture_bytes(side, side, n as u8))).collect()
}

/// One frame, as an app's: every read in flight lands first, so what a frame can upload doesn't depend on
/// how fast the workers decode. Returns the IDs it uploaded.
fn frame(server: &mut AssetServer) -> Vec<AssetId> {
    while server.stats().reads_in_flight > 0 {
        assert!(server.wait(Duration::from_secs(10)), "a read in flight never finished");
    }
    server.update();
    let mut uploaded = Vec::new();
    server.upload(|upload| {
        uploaded.push(upload.id());
        Ok(())
    });
    uploaded
}

/// Frames until every asset in `handles` is ready, and how many that took.
fn frames_until_ready<T: Asset>(server: &mut AssetServer, handles: &[Handle<T>]) -> u32 {
    for frames in 1..=100_000 {
        frame(server);
        if handles.iter().all(|handle| server.state(handle) == LoadState::Ready) {
            return frames;
        }
        let failed = handles.iter().find_map(|handle| match server.state(handle) {
            LoadState::Failed(error) => Some(error),
            _ => None,
        });
        assert_eq!(failed, None);
    }
    panic!("loads stalled: {:?}", server.stats());
}

/// Counts the bytes a server has asked to read and not yet uploaded, from outside it: each read is of one
/// texture of `size` decoded bytes, and the uploader says when one went.
struct Watched {
    inner: MemoryPackSource,
    size: u64,
    issued: AtomicU64,
    uploaded: AtomicU64,
    most: AtomicU64,
}

impl Watched {
    fn held(&self) -> u64 {
        (self.issued.load(Ordering::SeqCst) - self.uploaded.load(Ordering::SeqCst)) * self.size
    }
}

impl PackSource for Watched {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        self.inner.open(name)
    }

    fn read(&self, file: PackFile, offset: u64, len: usize, priority: LoadPriority, done: ReadDone) {
        self.issued.fetch_add(1, Ordering::SeqCst);
        self.most.fetch_max(self.held(), Ordering::SeqCst);
        self.inner.read(file, offset, len, priority, done);
    }
}

/// The row's proof: a gibibyte of textures asked for at once holds no more than the cap in memory before
/// the GPU takes it, as counted by the source and the uploader, not only by the server's own counter.
#[test]
fn a_gibibyte_of_loads_never_holds_more_than_the_cap() {
    const SIDE: u32 = 1024;
    const COUNT: u16 = 256;
    let assets = many(COUNT, SIDE);
    let size = assets[0].size;
    assert!(assets.iter().all(|asset| asset.size == size));
    assert!(size * u64::from(COUNT) >= 1 << 30, "{size} bytes each");
    let source = Arc::new(Watched {
        inner: MemoryPackSource::new(),
        size,
        issued: AtomicU64::new(0),
        uploaded: AtomicU64::new(0),
        most: AtomicU64::new(0),
    });
    source.inner.insert("shared", write_pack(assets).unwrap());
    let budgets = AssetBudgets::DESKTOP;
    let cap = budgets.admission;
    let mut server = AssetServer::new(source.clone(), budgets);
    server.mount_now("shared").unwrap();
    // The mount's reads of the header and table aren't assets'.
    source.issued.store(0, Ordering::SeqCst);

    let handles: Vec<Handle<TextureAsset>> = (0..COUNT).map(|n| server.load(numbered(n)).unwrap()).collect();
    let stats = server.stats();
    assert!(stats.held_back > 0 && stats.admitted <= cap, "{stats:?}");
    let mut frames = 0u32;
    while !handles.iter().all(|handle| server.state(handle) == LoadState::Ready) {
        frames += 1;
        assert!(frames < 10_000, "loads stalled: {:?}", server.stats());
        while server.stats().reads_in_flight > 0 {
            assert!(server.wait(Duration::from_secs(10)));
            assert!(server.stats().admitted <= cap);
        }
        server.update();
        server.upload(|upload| {
            assert_eq!(upload.bytes().len() as u64, size);
            source.uploaded.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert!(server.stats().admitted <= cap);
    }
    let stats = server.stats();
    let most = source.most.load(Ordering::SeqCst);
    assert!(most <= cap, "{most} bytes held, against a cap of {cap}");
    assert_eq!(stats.admitted_peak, most, "the server counts what the source and uploader saw");
    // It held back what didn't fit, rather than reading a gibibyte and waiting.
    assert!(most > cap - size, "{most} of {cap}");
    assert_eq!((stats.admitted, stats.held_back, stats.ready), (0, 0, u32::from(COUNT)), "{stats:?}");
    // The uploads were the bottleneck, as without a cap: a frame's budget, every frame.
    let per_frame = budgets.upload_per_frame / size;
    assert!(u64::from(frames) <= u64::from(COUNT).div_ceil(per_frame) + 2, "{frames} frames");
}

/// A cap of one byte admits one read at a time, so each asset waits for every other: what's admitted must
/// never be something that waits on a read that can't be. Parents asked for first, shared dependencies,
/// and a new renderer's reupload, which reads again in no particular order, all finish.
#[test]
fn a_capped_queue_never_stalls_an_asset_behind_what_it_waits_for() {
    let mut assets = Vec::new();
    for n in [10, 20, 30] {
        assets.push(packed(id(n), AssetKind::Texture, &texture_bytes(8, 8, n)));
    }
    // Three trees over two textures: 30's mesh shares texture 10 with 10's tree.
    assets.extend(tree(10, 10));
    assets.extend(tree(20, 20));
    assets.extend(tree(30, 10));
    let largest = assets.iter().map(|asset| asset.size).max().unwrap();
    let budgets = AssetBudgets { admission: 1, ..unlimited() };
    let (mut server, _) = serve(assets, budgets);
    let meshes: Vec<Handle<Mesh>> = [12, 22, 32].iter().map(|&n| server.load(id(n)).unwrap()).collect();
    assert!(server.stats().held_back > 0);
    frames_until_ready(&mut server, &meshes);
    let stats = server.stats();
    assert_eq!((stats.admitted, stats.held_back), (0, 0), "{stats:?}");

    server.reupload_all();
    assert!(server.stats().held_back > 0);
    frames_until_ready(&mut server, &meshes);
    let stats = server.stats();
    assert_eq!((stats.admitted, stats.held_back, stats.failed), (0, 0, 0), "{stats:?}");
    // One read at a time, all along: nothing was let in over the cap to unstick a parent.
    assert!(stats.admitted_peak <= largest, "{stats:?}");

    // `finish`, a load screen's loop, gets through the queue on its own as well.
    server.reupload_all();
    for mesh in &meshes {
        server.finish(mesh, |_| Ok(())).unwrap();
    }
}

/// An admitted asset read again from a new pack can come to depend on a texture already held back for
/// another load. It keeps its admission, so the texture is let in at once: held behind the parent's bytes,
/// it would never go, and the parent would never give them back.
#[test]
fn an_admitted_asset_read_again_takes_its_new_dependencies_in() {
    let assets = |texture| {
        vec![
            packed(id(10), AssetKind::Texture, &texture_bytes(8, 8, 10)),
            packed(id(MATERIAL), AssetKind::Material, &material_bytes(texture)),
        ]
    };
    let budgets = AssetBudgets { admission: 1, ..unlimited() };
    let (mut server, source) = serve(assets(None), budgets);
    let material: Handle<Material> = server.load(id(MATERIAL)).unwrap();
    let texture: Handle<TextureAsset> = server.load_with(id(10), LoadPriority::Prefetch).unwrap();
    assert_eq!(server.stats().held_back, 1, "the material is admitted and the texture waits");

    // The edit gives the material the held texture while its first read is still on its way.
    source.insert("shared", write_pack(assets(Some(id(10)))).unwrap());
    server.remount("shared").unwrap();
    assert_eq!(server.stats().held_back, 0, "{:?}", server.stats());
    frames_until_ready(&mut server, &[material]);
    assert_eq!(server.state(&texture), LoadState::Ready);
    let stats = server.stats();
    assert_eq!((stats.admitted, stats.held_back), (0, 0), "{stats:?}");
}

/// A split texture under a one-byte cap loads whole (M5 M3's levels), its level asked for beside the mesh
/// as the stage asks (M5 M4): the tail is admitted first, then the material that holds it, the mesh, and the
/// level, which waits its turn like any load, one read at a time and none let over the cap. It loads whole
/// again after a remount changes the tail and the level, and after a new renderer asks for everything again.
#[test]
fn a_split_texture_loads_whole_one_read_at_a_time() {
    let budgets = AssetBudgets { admission: 1, ..unlimited() };
    let (mut server, source) = serve(split_texture(10), budgets);
    let level = TextureLevel::id(id(TEXTURE), 0);
    let largest = split_texture(10).iter().map(|asset| asset.size).max().unwrap();
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    let held: Handle<TextureLevel> = server.load(level).unwrap();
    assert!(server.stats().held_back > 0);
    // Frames until the mesh and the level are both ready, and what they uploaded.
    let whole = |server: &mut AssetServer| {
        let mut order = Vec::new();
        for _ in 0..100 {
            order.extend(frame(server));
            if server.state(&mesh) == LoadState::Ready && server.state(&held) == LoadState::Ready {
                return order;
            }
        }
        panic!("loads stalled: {:?}", server.stats());
    };
    assert_eq!(whole(&mut server), [id(TEXTURE), id(MATERIAL), id(MESH), level]);
    assert!(server.stats().admitted_peak <= largest, "{:?}", server.stats());

    source.insert("shared", write_pack(split_texture(40)).unwrap());
    server.remount("shared").unwrap();
    // The mesh and the level are ready throughout, as their new versions load behind them.
    let mut order = whole(&mut server);
    for _ in 0..100 {
        order.extend(frame(&mut server));
    }
    assert!(
        order.contains(&id(TEXTURE)) && order.contains(&level),
        "the tail and the level again: {order:?}"
    );
    server.reupload_all();
    let mut order = whole(&mut server);
    for _ in 0..100 {
        order.extend(frame(&mut server));
    }
    assert!(
        order.contains(&id(TEXTURE)) && order.contains(&level),
        "the tail and the level again: {order:?}"
    );
    let stats = server.stats();
    // A reload may let a tail in over the cap for a level already admitted that waits on it.
    assert_eq!(
        (stats.admitted, stats.held_back, stats.failed, stats.reload_failures),
        (0, 0, 0, 0),
        "{stats:?}"
    );
}

/// An urgent load asked for behind a long queue of speculative ones goes as soon as the cap has room: it
/// waits for what was already admitted, not for the queue.
#[test]
fn an_urgent_load_isnt_starved_behind_speculative_ones() {
    let assets = many(51, 64);
    let size = assets[0].size;
    // Room for two reads, and one upload a frame.
    let budgets = AssetBudgets { upload_per_frame: size, admission: 2 * size, ..unlimited() };
    let (mut server, _) = serve(assets, budgets);
    let prefetched: Vec<Handle<TextureAsset>> =
        (0..50).map(|n| server.load_with(numbered(n), LoadPriority::Prefetch).unwrap()).collect();
    frame(&mut server);
    let urgent: Handle<TextureAsset> = server.load_with(numbered(50), LoadPriority::Critical).unwrap();
    let frames = frames_until_ready(&mut server, std::slice::from_ref(&urgent));
    // The two admitted before it, then it.
    assert!(frames <= 3, "{frames} frames");
    let ready = prefetched.iter().filter(|handle| server.state(handle) == LoadState::Ready).count();
    assert!(ready <= 4, "{ready} speculative loads went first");
    frames_until_ready(&mut server, &prefetched);
}

/// Raising a queued load's priority moves it, and what it depends on, up the queue: the streamer asking for
/// a level it now needs sooner.
#[test]
fn a_queued_load_raised_goes_next_with_its_dependencies() {
    let mut assets = many(40, 64);
    let size = assets[0].size;
    assets.push(packed(id(10), AssetKind::Texture, &texture_bytes(64, 64, 10)));
    assets.extend(tree(10, 10));
    let budgets = AssetBudgets { upload_per_frame: size, admission: size, ..unlimited() };
    let (mut server, _) = serve(assets, budgets);
    let filler: Vec<Handle<TextureAsset>> =
        (0..40).map(|n| server.load_with(numbered(n), LoadPriority::Prefetch).unwrap()).collect();
    let mesh: Handle<Mesh> = server.load_with(id(12), LoadPriority::Prefetch).unwrap();
    let last: Handle<TextureAsset> = server.load_with(numbered(39), LoadPriority::Prefetch).unwrap();
    frame(&mut server);

    // Through `raise_priority`, the mesh, its material and its texture.
    server.raise_priority(&mesh, LoadPriority::High);
    let frames = frames_until_ready(&mut server, std::slice::from_ref(&mesh));
    assert!(frames <= 5, "{frames} frames");
    // Through asking again at a higher priority.
    let _again: Handle<TextureAsset> = server.load_with(numbered(39), LoadPriority::Critical).unwrap();
    let frames = frames_until_ready(&mut server, std::slice::from_ref(&last));
    assert!(frames <= 3, "{frames} frames");
    let ready = filler.iter().filter(|handle| server.state(handle) == LoadState::Ready).count();
    assert!(ready < 20, "{ready} of the filler went first");

    // A priority never falls: asking at a lower one changes nothing.
    let _ = server.load_with::<TextureAsset>(numbered(38), LoadPriority::High).unwrap();
    let _ = server.load_with::<TextureAsset>(numbered(38), LoadPriority::Prefetch).unwrap();
    let frames = frames_until_ready(&mut server, std::slice::from_ref(&filler[38]));
    assert!(frames <= 3, "{frames} frames");
    frames_until_ready(&mut server, &filler);
}

/// Kinds that never go to the GPU aren't held back: nothing waits on a frame's uploads to take them.
#[test]
fn kinds_that_dont_upload_are_never_held_back() {
    use crate::rig::CharacterRig;
    let rig = CharacterRig { joint_count: 2, ..CharacterRig::default() }.to_cooked().unwrap();
    let mut assets = many(4, 64);
    let size = assets[0].size;
    assets.push(packed(id(40), AssetKind::Rig, &rig));
    let budgets = AssetBudgets { upload_per_frame: size, admission: size, ..unlimited() };
    let (mut server, _) = serve(assets, budgets);
    let textures: Vec<Handle<TextureAsset>> = (0..4).map(|n| server.load(numbered(n)).unwrap()).collect();
    assert_eq!(server.stats().held_back, 3);
    let rig: Handle<CharacterRig> = server.load(id(40)).unwrap();
    assert_eq!(server.stats().held_back, 3);
    frame(&mut server);
    assert_eq!(server.state(&rig), LoadState::Ready);
    frames_until_ready(&mut server, &textures);
}

/// What a load held back for gives way when it goes: an unmount, a failure, a drop all leave the cap empty.
#[test]
fn held_and_admitted_bytes_go_with_what_unmounts() {
    let assets = many(8, 64);
    let size = assets[0].size;
    let budgets = AssetBudgets { admission: size, ..unlimited() };
    let (mut server, _) = serve(assets, budgets);
    let handles: Vec<Handle<TextureAsset>> = (0..8).map(|n| server.load(numbered(n)).unwrap()).collect();
    let stats = server.stats();
    assert_eq!((stats.held_back, stats.admitted), (7, budgets.admission), "{stats:?}");
    server.unmount("shared").unwrap();
    let stats = server.stats();
    assert_eq!((stats.held_back, stats.admitted), (0, 0), "{stats:?}");
    assert!(handles.iter().all(|handle| matches!(server.state(handle), LoadState::Failed(_))));
}
