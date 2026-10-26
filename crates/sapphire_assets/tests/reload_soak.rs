//! The hot reload soak test (M1 A8): rounds of loading, using and dropping assets under tight
//! budgets while their pack is remounted between two versions must keep the generation protocol
//! (an app is never sent an older version than it holds) and leak neither handles nor memory. Its
//! own process, with a counting global allocator, so live bytes can be checked.

use std::alloc::System;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sapphire_assets::material::{AnimeCharacterMaterial, Material};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, SlotMaterial, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::pack::{Codec, PackAsset, write_pack};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_assets::{
    AssetBudgets, AssetChange, AssetId, AssetKind, AssetServer, AssetSlot, Handle, LoadState,
    MemoryPackSource, Upload, UploadError,
};
use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_serialize::cooked::AlignedVec;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// Characters in the pack: a mesh, a material and a texture each.
const CHARACTERS: u8 = 12;
const ROUNDS: u32 = 300;

fn id(kind: u8, n: u8) -> AssetId {
    let mut bytes = [n; 16];
    bytes[0] = kind;
    AssetId::from_bytes(bytes)
}

/// Version 1 of the pack changes every other texture, a third of the materials (to another
/// character's texture) and a quarter of the meshes.
fn texture(n: u8, version: u8) -> Vec<u8> {
    let side = 16 << (n % 3);
    let fill = if n.is_multiple_of(2) { n + 100 * version } else { n };
    let texels = vec![fill; side * side * 4];
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: side as u32,
        height: side as u32,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    Texture { desc, levels: vec![&texels] }.to_ktx2().unwrap()
}

fn material(n: u8, version: u8) -> Vec<u8> {
    let texture = if version == 1 && n.is_multiple_of(3) { (n + 1) % CHARACTERS } else { n };
    let document =
        AnimeCharacterMaterial { base_color_map: Some(id(1, texture)), ..AnimeCharacterMaterial::default() };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(&document, &swizzles).unwrap().to_cooked().unwrap().into_vec()
}

fn mesh(n: u8, version: u8) -> Vec<u8> {
    let z = f32::from(n) + if n.is_multiple_of(4) { f32::from(version) } else { 0.0 };
    let vertex = |x: f32, y: f32| Vertex {
        position: [x, y, z],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [x, y],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let slot = MaterialSlot { name: "body".into(), material: SlotMaterial::Asset(id(2, n)) };
    let submeshes = vec![Submesh { first_index: 0, index_count: 3, material: 0 }];
    let vertices = [vertex(0.0, 0.0), vertex(1.0, 0.0), vertex(0.0, 1.0)];
    Mesh::from_vertices(&vertices, vec![0, 1, 2], submeshes, vec![slot], PositionFormat::Unorm16)
        .to_cooked()
        .unwrap()
        .into_vec()
}

fn pack(version: u8) -> Vec<u8> {
    let mut assets = Vec::new();
    for n in 0..CHARACTERS {
        for (kind, asset_kind, bytes, codec) in [
            (1, AssetKind::Texture, texture(n, version), Codec::Zstd),
            (2, AssetKind::Material, material(n, version), Codec::Lz4),
            (3, AssetKind::Mesh, mesh(n, version), Codec::Zstd),
        ] {
            let mut aligned = AlignedVec::<16>::new();
            aligned.extend_from_slice(&bytes);
            let dependencies = asset_kind.references(&aligned).unwrap();
            assets.push(PackAsset::encode(id(kind, n), asset_kind, codec, &bytes, dependencies, 3).unwrap());
        }
    }
    write_pack(assets).unwrap()
}

/// xorshift: which characters a round loads and keeps.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// The app's side: the generation it holds of each uploaded asset, by slot, as a renderer would
/// hold its GPU copies. Never sent an older version than it holds.
struct Gpu(HashMap<AssetSlot, u32>);

impl Gpu {
    fn upload(&mut self, upload: Upload<'_>) -> Result<(), UploadError> {
        match upload.kind() {
            AssetKind::Mesh => assert!(upload.get::<Mesh>().is_some()),
            AssetKind::Material => assert!(upload.get::<Material>().is_some()),
            AssetKind::Texture => assert!(upload.get::<sapphire_assets::TextureAsset>().is_some()),
            AssetKind::Rig => unreachable!("rigs never go to the uploader"),
            AssetKind::ProbeGrid => unreachable!("these assets include no probe grid"),
        }
        let held = self.0.insert(upload.slot(), upload.generation());
        assert!(held.is_none_or(|held| held <= upload.generation()), "an older version was sent");
        Ok(())
    }
}

/// Frames until nothing is in flight: loads and reloads alike.
fn settle(server: &mut AssetServer, gpu: &mut Gpu) {
    loop {
        server.update();
        let report = server.upload(|upload| gpu.upload(upload));
        let stats = server.stats();
        let busy = stats.reads_in_flight + stats.loading + stats.waiting + stats.reloading + report.pending;
        if report.assets == 0 && busy == 0 {
            return;
        }
        if report.assets == 0 {
            server.wait(Duration::from_millis(50));
        }
    }
}

/// Checks what the server reloaded against what the app holds, then forgets what it unloaded.
fn follow(server: &mut AssetServer, gpu: &mut Gpu) {
    for change in server.take_changes() {
        match change {
            // An asset reloaded twice before this reports both; the app holds the later.
            AssetChange::Reloaded { slot, generation, .. } => {
                let held = gpu.0.get(&slot).copied();
                assert!(
                    held.is_some_and(|held| held >= generation),
                    "reported {generation}, the app holds {held:?}"
                );
            }
            other => panic!("unexpected change {other:?}"),
        }
    }
    for unloaded in server.take_unloaded() {
        gpu.0.remove(&unloaded.slot);
    }
}

/// One round: load some characters and finish them, maybe remount the pack at its other version
/// and let the reloads land, then drop most handles and let residency trim.
fn round(
    server: &mut AssetServer,
    source: &MemoryPackSource,
    packs: &[Arc<[u8]>; 2],
    version: &mut usize,
    rng: &mut Rng,
    kept: &mut Vec<Handle<Mesh>>,
    gpu: &mut Gpu,
) {
    let mut loaded = Vec::new();
    for _ in 0..1 + rng.next() % 4 {
        let n = (rng.next() % u32::from(CHARACTERS)) as u8;
        let handle: Handle<Mesh> = server.load(id(3, n)).unwrap();
        loaded.push(handle);
    }
    if rng.next().is_multiple_of(3) {
        *version = 1 - *version;
        source.insert("shared", packs[*version].clone());
        server.remount("shared").unwrap();
        // Now and then the edit is undone while its reloads are still on their way.
        if rng.next().is_multiple_of(2) {
            server.update();
            server.upload(|upload| gpu.upload(upload));
            *version = 1 - *version;
            source.insert("shared", packs[*version].clone());
            server.remount("shared").unwrap();
        }
    }
    settle(server, gpu);
    for handle in &loaded {
        assert_eq!(server.state(handle), LoadState::Ready);
    }
    if rng.next().is_multiple_of(4) {
        kept.push(loaded[0].clone());
    }
    if kept.len() > 4 {
        kept.remove(0);
    }
    drop(loaded);
    server.update();
    follow(server, gpu);
}

#[test]
fn hot_reload_leaks_no_handles_or_memory() {
    assert!(alloc::is_installed());
    let packs: [Arc<[u8]>; 2] = [pack(0).into(), pack(1).into()];
    let source = Arc::new(MemoryPackSource::new());
    source.insert("shared", packs[0].clone());
    // Tight budgets, so residency evicts all the time.
    let budgets = AssetBudgets { upload_per_frame: 32 << 10, resident: [400, 30_000, 2_000, 1_000, 1_000] };
    let mut server = AssetServer::new(source.clone(), budgets);
    server.mount("shared").unwrap();
    let mut rng = Rng(0xa8a8);
    let mut version = 0;
    let mut kept = Vec::new();
    let mut gpu = Gpu(HashMap::with_capacity(256));

    // Warm up: the job pool's threads and decoders, the server's maps at full size.
    for _ in 0..40 {
        round(&mut server, &source, &packs, &mut version, &mut rng, &mut kept, &mut gpu);
    }
    kept.clear();
    server.unload_unused();
    follow(&mut server, &mut gpu);
    assert_eq!(server.stats().assets, 0);
    let baseline = alloc::stats().live_bytes;
    let before = server.stats();

    for _ in 0..ROUNDS {
        round(&mut server, &source, &packs, &mut version, &mut rng, &mut kept, &mut gpu);
    }
    let stats = server.stats();
    assert!(
        stats.reloads > before.reloads + u64::from(ROUNDS) / 4,
        "assets in use reloaded throughout: {stats:?}"
    );
    assert!(stats.unloads > before.unloads + u64::from(ROUNDS), "residency evicted throughout: {stats:?}");
    assert_eq!((stats.failures, stats.reload_failures), (0, 0));

    // Everything released: no slot, byte or GPU copy may remain.
    kept.clear();
    server.unload_unused();
    follow(&mut server, &mut gpu);
    let stats = server.stats();
    assert_eq!((stats.assets, stats.resident, stats.reads_in_flight), (0, [0; 5], 0), "{stats:?}");
    assert!(gpu.0.is_empty(), "the app was told to free every copy it uploaded");
    let after = alloc::stats().live_bytes;
    // Allowance for allocator-internal and channel blocks that come and go; a leak per round (a
    // slot, a handle, a decoded version, an open file) would be far larger over the rounds.
    assert!(after <= baseline + 16 * 1024, "live bytes grew from {baseline} to {after} over {ROUNDS} rounds");
}
