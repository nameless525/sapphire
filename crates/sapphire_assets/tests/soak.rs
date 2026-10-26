//! The load and unload soak test (spec §5.10): rounds of loading assets with their
//! dependencies, using and dropping them under a tight budget, must leak neither handles nor
//! memory. Its own process, with a counting global allocator, so live bytes can be checked.

use std::alloc::System;
use std::sync::Arc;

use sapphire_assets::material::{AnimeCharacterMaterial, Material};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, SlotMaterial, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::pack::{Codec, PackAsset, write_pack};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_assets::{
    AssetBudgets, AssetId, AssetKind, AssetServer, Handle, LoadState, MemoryPackSource, UntypedHandle,
    WeakHandle,
};
use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_serialize::cooked::AlignedVec;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// Characters in the pack: a mesh, a material and a texture each.
const CHARACTERS: u8 = 24;
const ROUNDS: u32 = 300;

fn id(kind: u8, n: u8) -> AssetId {
    let mut bytes = [n; 16];
    bytes[0] = kind;
    AssetId::from_bytes(bytes)
}

fn texture(n: u8) -> Vec<u8> {
    // Sizes vary, so residency trims by bytes, not counts.
    let side = 16 << (n % 4);
    let texels = vec![n; side * side * 4];
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: side as u32,
        height: side as u32,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    Texture { desc, levels: vec![&texels] }.to_ktx2().unwrap()
}

fn material(n: u8) -> Vec<u8> {
    let document =
        AnimeCharacterMaterial { base_color_map: Some(id(1, n)), ..AnimeCharacterMaterial::default() };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(&document, &swizzles).unwrap().to_cooked().unwrap().into_vec()
}

fn mesh(n: u8) -> Vec<u8> {
    let vertex = |x: f32, y: f32| Vertex {
        position: [x, y, f32::from(n)],
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

fn pack() -> Vec<u8> {
    let mut assets = Vec::new();
    for n in 0..CHARACTERS {
        for (kind, asset_kind, bytes, codec) in [
            (1, AssetKind::Texture, texture(n), Codec::Zstd),
            (2, AssetKind::Material, material(n), Codec::Lz4),
            (3, AssetKind::Mesh, mesh(n), Codec::Zstd),
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

/// One round: load some characters, finish them, drop most handles, and let residency trim.
fn round(
    server: &mut AssetServer,
    rng: &mut Rng,
    kept: &mut Vec<Handle<Mesh>>,
    weak: &mut Vec<WeakHandle<Mesh>>,
) {
    let mut loaded = Vec::new();
    for _ in 0..1 + rng.next() % 6 {
        let n = (rng.next() % u32::from(CHARACTERS)) as u8;
        let handle: Handle<Mesh> = server.load(id(3, n)).unwrap();
        weak.push(handle.downgrade());
        loaded.push(handle);
    }
    // Pin one now and then, and unpin it later.
    if rng.next().is_multiple_of(5) {
        server.pin(&loaded[0]);
        kept.push(loaded[0].clone());
    }
    for handle in &loaded {
        server
            .finish(handle, |upload| {
                // Uploads read the asset in place, as the renderer would.
                match upload.kind() {
                    AssetKind::Mesh => assert!(upload.get::<Mesh>().is_some()),
                    AssetKind::Material => assert!(upload.get::<Material>().is_some()),
                    AssetKind::Texture => assert!(upload.get::<sapphire_assets::TextureAsset>().is_some()),
                    AssetKind::Rig => unreachable!("rigs never go to the uploader"),
                    AssetKind::ProbeGrid => unreachable!("these assets include no probe grid"),
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(server.state(handle), LoadState::Ready);
    }
    if kept.len() > 3 {
        let handle = kept.remove(0);
        server.unpin(&handle);
    }
    drop(loaded);
    server.update();
    let _ = server.take_unloaded();
}

#[test]
fn loading_and_unloading_leaks_no_handles_or_memory() {
    assert!(alloc::is_installed());
    let source = Arc::new(MemoryPackSource::new());
    source.insert("shared", pack());
    // Tight budgets, so residency evicts all the time.
    let budgets = AssetBudgets { upload_per_frame: 64 << 10, resident: [600, 40_000, 3_000, 1_000, 1_000] };
    let mut server = AssetServer::new(source, budgets);
    server.mount("shared").unwrap();
    let mut rng = Rng(0x5eed);
    let (mut kept, mut weak) = (Vec::new(), Vec::new());

    // Warm up: the job pool's threads and decoders, the server's maps at full size.
    let everything: Vec<UntypedHandle> = (0..CHARACTERS)
        .map(|n| server.load_untyped(id(3, n), sapphire_assets::LoadPriority::Normal).unwrap())
        .collect();
    for handle in &everything {
        server.finish(&handle.clone().typed::<Mesh>().unwrap(), |_| Ok(())).unwrap();
    }
    drop(everything);
    server.unload_unused();
    let _ = server.take_unloaded();
    for _ in 0..20 {
        round(&mut server, &mut rng, &mut kept, &mut weak);
    }
    for handle in kept.drain(..) {
        server.unpin(&handle);
    }
    server.unload_unused();
    let _ = server.take_unloaded();
    weak.clear();
    weak.shrink_to_fit();
    assert_eq!(server.stats().assets, 0);
    let baseline = alloc::stats().live_bytes;

    for _ in 0..ROUNDS {
        round(&mut server, &mut rng, &mut kept, &mut weak);
        assert_eq!(server.stats().reads_in_flight, 0);
    }
    let stats = server.stats();
    assert!(stats.loads > u64::from(ROUNDS), "{stats:?}");
    assert!(stats.unloads > u64::from(ROUNDS), "residency evicted throughout: {stats:?}");
    assert_eq!(stats.failures, 0);

    // Everything released: no slot, byte or handle may remain.
    for handle in kept.drain(..) {
        server.unpin(&handle);
    }
    server.unload_unused();
    let _ = server.take_unloaded();
    let stats = server.stats();
    assert_eq!((stats.assets, stats.resident, stats.reads_in_flight), (0, [0; 5], 0), "{stats:?}");
    assert!(weak.iter().all(|&weak| server.upgrade(weak).is_none()), "a weak handle outlived its asset");
    weak.clear();
    weak.shrink_to_fit();
    let after = alloc::stats().live_bytes;
    // Allowance for allocator-internal and channel blocks that come and go; a leak per round
    // (a slot, a handle, a decoded asset) would be far larger over the rounds.
    assert!(after <= baseline + 16 * 1024, "live bytes grew from {baseline} to {after} over {ROUNDS} rounds");
}
