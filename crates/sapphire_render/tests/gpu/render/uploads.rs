//! What a mesh upload costs the thread that asks for it, through the mesh pool's staging ring and by
//! `Queue::write_buffer` alone (M5 D9), and what the frames that carry the uploads cost. Prints the
//! median and 99th percentile of each; asserts nothing about time.
//!
//! Its sets: the proving ground's and the proving hub's meshes, read from their cooked desktop packs
//! (`cargo xtask content` makes them; a set whose pack isn't there or is stale is skipped, said so), and a
//! synthetic burst of many small meshes and a few large ones. Each path has a renderer of its own,
//! warmed by one round, then the two take turns, round by round, so the machine's load falls on both
//! alike: a round uploads the set [`PER_FRAME`] meshes a frame, then frees it and lets the fence give
//! it back, so every round after the first uploads into a pool already grown.

use std::path::Path;
use std::time::{Duration, Instant};

use sapphire_assets::AssetKind;
use sapphire_assets::mesh::{AlignedVec, ArchivedMesh, MeshError};
use sapphire_assets::pack::{PACK_VERSION, PackError, PackReader, file_name};
use sapphire_serialize::CookedError;

use super::*;

/// Rounds each path is measured over, after its warm-up.
const ROUNDS: usize = 10;
/// Meshes uploaded a frame, about what a zone's streaming asks at its busiest.
const PER_FRAME: usize = 8;
const SIZE: (u32, u32) = (64, 64);

/// Cooked meshes, each 16-byte aligned as `Mesh::read` reads them.
type Cooked = Vec<AlignedVec>;

/// The meshes of zone `stem`'s cooked desktop pack, each read here once so a round reads bytes known
/// good. A pack that isn't there, or was cooked at an older pack or mesh format, is skipped, said so,
/// as every cooked version bump leaves a checkout's cook behind until it's re-made; a current pack that
/// still doesn't read fails the test.
fn zone_meshes(stem: &str) -> Option<Cooked> {
    let name = file_name(&format!("zone_{stem}"));
    let skip = |why: String| -> Option<Cooked> {
        eprintln!(
            "mesh_uploads: skipping {stem}, target/content/desktop/{name}: {why}; `cargo xtask content` re-makes it"
        );
        None
    };
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/content/desktop");
    let Ok(bytes) = std::fs::read(content.join(&name)) else {
        return skip("it isn't cooked".to_owned());
    };
    let reader = match PackReader::new(&bytes) {
        Ok(reader) => reader,
        Err(PackError::UnsupportedVersion(found)) => {
            return skip(format!("its pack format is {found}, this build reads {PACK_VERSION}"));
        }
        Err(error) => panic!("{name}: {error}"),
    };
    let mut meshes = Vec::new();
    for entry in reader.toc().entries().iter().filter(|entry| entry.kind == AssetKind::Mesh) {
        let bytes = reader.read(entry.id).unwrap_or_else(|error| panic!("{name}: {error}"));
        match Mesh::read(&bytes) {
            Ok(_) => meshes.push(bytes),
            Err(MeshError::Cooked(CookedError::Stale { found, current })) => {
                return skip(format!("its meshes are cooked at version {found}, this build reads {current}"));
            }
            Err(error) => panic!("{name}: mesh {}: {error}", entry.id),
        }
    }
    Some(meshes)
}

/// A flat square sheet of `cells` by `cells` quads.
fn sheet(cells: u32) -> Mesh {
    let side = cells + 1;
    let vertices: Vec<Vertex> = (0..side * side)
        .map(|n| {
            let (x, y) = ((n % side) as f32 / cells as f32, (n / side) as f32 / cells as f32);
            Vertex {
                position: [x - 0.5, y - 0.5, 0.0],
                normal: [0.0, 0.0, 1.0],
                outline_normal: [0.0, 0.0, 1.0],
                uv: [x, y],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            }
        })
        .collect();
    let indices: Vec<u32> = (0..cells * cells)
        .flat_map(|n| {
            let a = (n / cells) * side + n % cells;
            let (b, c, d) = (a + 1, a + side, a + side + 1);
            [a, b, c, b, d, c]
        })
        .collect();
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("sheet", cooked(&colored([0.5, 0.5, 0.5])))];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// 192 small meshes (49 vertices each) and 4 large ones (40,401 each), interleaved.
fn burst() -> Cooked {
    (0..196)
        .map(|n| if n % 49 == 48 { sheet(200) } else { sheet(6) }.to_cooked().expect("it cooks"))
        .collect()
}

/// One path's renderer and what it measured.
struct Uploader {
    frames: Frames,
    ring: bool,
    /// One material for every slot: the pack's meshes name material assets this test doesn't load.
    material: MaterialHandle,
    uploads: Vec<Duration>,
    frame_times: Vec<Duration>,
    /// Each round's uploads and frames together: what the thread that asks spent on them.
    rounds: Vec<Duration>,
}

impl Uploader {
    fn new(gpu: &GpuContext, ring: bool) -> Self {
        let mut frames = Frames::new(gpu, FORMAT, Msaa::default()).expect("renderer");
        frames.renderer.stage_mesh_uploads(ring);
        let grey = cooked(&colored([0.5, 0.5, 0.5])).to_cooked().expect("it cooks");
        let material =
            frames.renderer.upload_material(Material::read(&grey).expect("valid"), |_| None).expect("upload");
        Self { frames, ring, material, uploads: Vec::new(), frame_times: Vec::new(), rounds: Vec::new() }
    }

    /// Uploads `set`, [`PER_FRAME`] a frame, then frees it and waits for the pool to have it back.
    #[allow(clippy::disallowed_methods, reason = "timing needs the OS clock, and nothing here ships")]
    fn round(&mut self, gpu: &GpuContext, view: &wgpu::TextureView, set: &Cooked, measured: bool) {
        let camera = framing_camera(38.0, SIZE, 0.6);
        let mut handles = Vec::with_capacity(set.len());
        let mut total = Duration::ZERO;
        for batch in set.chunks(PER_FRAME) {
            for bytes in batch {
                let mesh: &ArchivedMesh = Mesh::read(bytes).expect("a valid mesh");
                // Out of the time: the burst's embedded materials, uploaded per mesh and freed with it, and
                // the shared one for the packs' material assets.
                let material = self.material;
                let materials =
                    self.frames.renderer.upload_slot_materials(mesh, |_| Some(material)).expect("materials");
                let start = Instant::now();
                let handle = self.frames.renderer.upload_mesh(mesh, &materials).expect("it uploads");
                let took = start.elapsed();
                handles.push(handle);
                total += took;
                if measured {
                    self.uploads.push(took);
                }
            }
            let start = Instant::now();
            assert!(self.frames.render(gpu, view, camera, &[], &[]).is_none(), "no validation error");
            let took = start.elapsed();
            total += took;
            if measured {
                self.frame_times.push(took);
            }
            // A presented frame's GPU keeps up, as a surface holds a game to a few frames in flight, so the
            // ring's buffers come back; out of the time.
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        }
        if measured {
            self.rounds.push(total);
        }
        for handle in handles {
            self.frames.renderer.free_mesh(handle);
        }
        for _ in 0..8 {
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            assert!(self.frames.render(gpu, view, camera, &[], &[]).is_none(), "no validation error");
        }
        assert_eq!(self.frames.renderer.stats().mesh_pool_bytes, 0, "the round's meshes went back");
    }

    fn report(&mut self, set: &str, meshes: usize, bytes: u64) {
        let us = |d: Duration| d.as_secs_f64() * 1e6;
        let percentiles = |times: &mut Vec<Duration>| {
            times.sort_unstable();
            let at = |share: usize| times[(times.len() * share / 100).min(times.len() - 1)];
            (us(at(50)), us(at(99)), us(*times.last().expect("some")))
        };
        let (upload50, upload99, upload_max) = percentiles(&mut self.uploads);
        let (frame50, frame99, frame_max) = percentiles(&mut self.frame_times);
        let (round50, _, round_max) = percentiles(&mut self.rounds);
        eprintln!(
            "mesh_uploads: {set}, {}: {meshes} meshes of {:.2} MiB, {ROUNDS} rounds; an upload p50 {upload50:.1} µs, \
             p99 {upload99:.1}, max {upload_max:.1}; a frame carrying {PER_FRAME} p50 {frame50:.1} µs, p99 \
             {frame99:.1}, max {frame_max:.1}; a round's uploads and frames p50 {:.2} ms, max {:.2}",
            if self.ring { "staging ring" } else { "write_buffer" },
            bytes as f64 / f64::from(1 << 20),
            round50 / 1e3,
            round_max / 1e3,
        );
        self.uploads.clear();
        self.frame_times.clear();
        self.rounds.clear();
    }
}

#[test]
#[ignore = "needs a GPU; a measurement, run alone (M5 D9)"]
fn mesh_upload_times_by_ring_and_by_write_buffer() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let mut sets = Vec::new();
    for stem in ["proving_ground", "proving_hub"] {
        if let Some(meshes) = zone_meshes(stem) {
            sets.push((stem.to_owned(), meshes));
        }
    }
    sets.push(("synthetic burst".to_owned(), burst()));
    let mut paths = [Uploader::new(&gpu, false), Uploader::new(&gpu, true)];
    for (name, set) in &sets {
        let bytes: u64 = set.iter().map(|mesh| mesh.len() as u64).sum();
        for path in &mut paths {
            path.round(&gpu, &view, set, false);
        }
        for round in 0..ROUNDS {
            // Each goes first every other round.
            let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
            for at in order {
                paths[at].round(&gpu, &view, set, true);
            }
        }
        for path in &mut paths {
            path.report(name, set.len(), bytes);
        }
    }
}
