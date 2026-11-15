//! Material merge (M5 L15): Aster, whose sidecar merges skin, jacket and gear into one slot, against the same
//! sources cooked without that section, on each cook and every tier of it. The frame's counts are exact and
//! read back pass by pass, and the pictures are compared at three distances. Run alone with
//! `cargo test -p sapphire_render --test gpu merge:: -- --ignored --test-threads=1 --nocapture`.
//!
//! Each variant is cooked and packed from a copy of `content/combat/hero/` (her model, materials and maps;
//! not her moves, graph or playable document, which name the heroine's clips) the way the pipeline packs
//! content, and uploaded as the runtime loads a pack: every texture, every material, then the model by its
//! slots. A third upload of the unmerged model draws the merged slots in one flat colour and the rest in
//! another, which says which pixels the merge could touch: the other slots, their outlines and the face are
//! held exact outside them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{fs, process};

use glam::{Mat4, Vec3};
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, LightSplit, Material, Outline, Rim,
};
use sapphire_assets::mesh::Mesh;
use sapphire_assets::pack::PackReader;
use sapphire_assets::texture::{Swizzle, Texture, TextureTail};
use sapphire_assets::texture_level::{MAX_LEVELS, TextureLevel};
use sapphire_assets::{AssetId, AssetKind};
use sapphire_pipeline::cook::{Cook, CookOptions, cook_dir};
use sapphire_pipeline::pack::pack_cooked;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::quality::{TierId, TierTable};
use sapphire_render::texture::{GpuTexture, check_format};
use sapphire_render::{
    AmbientLight, CameraData, CharacterBody, CharacterFace, CharacterInstance, CharacterLighting,
    DirectionalLight, FrameCounts, FramePacketRing, InstanceId, MaterialDrive, MaterialHandle, MeshHandle,
    MeshInstance, PostParams, RenderSettings, Renderer, SkyGradient, ViewData,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Aster's model (`content/combat/hero/hero.vrm`).
const ASTER: &str = "2533d8d7-9400-4972-88f3-f99ff9473913";
/// The slots her sidecar merges into `body`.
const MEMBERS: [&str; 3] = ["skin", "top", "gear"];
/// Her playable kind's documents, which name clips outside the copy.
const LEFT_OUT: [&str; 4] = [".moves.ron", ".animgraph.ron", ".playable.ron", ".keypose.ron"];
const SIZE: (u32, u32) = (480, 640);
const FOV: f32 = 0.6;
/// Frames drawn before the one read: temporal AA's history settles over its jitter cycle.
const FRAMES: u32 = 12;
/// How far past the merged slots' own pixels their outline and the frame's filters may reach.
const REACH: i32 = 8;

/// The three distances, as the share of the frame's height her whole figure spans and the height on her body
/// looked at, and the share of the pixels near the merged slots that may be more than one code value off.
///
/// Close up, the atlas's level 0 is its members' blocks and the UVs move by less than a hundredth of a texel,
/// so what differs is where a merged slot meets one drawn between its members (the skirt draws between the
/// jacket and the gear, and now after both), at the hands over the skirt: 0.06 to 0.12% of the pixels near
/// the merged slots, measured. Whole, a polygon seen edge-on reads deeper levels (1.9 to 2.1%, at the cuffs).
/// Far, past [`sapphire_pipeline::merge::EXACT_LEVELS`] the atlas's levels blend a tile with its padding
/// (4.5 to 8.6%).
const DISTANCES: [(&str, f32, f32, f64); 3] =
    [("close", 2.6, 0.62, 0.002), ("whole", 0.9, 0.5, 0.03), ("far", 0.16, 0.5, 0.10)];
/// Away from the merged slots, the pixels more than one code value off: Metal's own variation, a lone pixel
/// at a silhouette now and then (`goldens.md`). The rest are within one, where bloom and temporal AA carry
/// the merged slots' difference.
const AWAY_OVER_ONE: usize = 16;

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// A directory of this test's own, gone when it ends.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire_merge_{name}_{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("content")).expect("a temp dir");
        Self(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Aster's shared pack on `cook`, from a copy of her sources, with her sidecar's merge section or without.
fn aster_pack(merged: bool, cook: Cook) -> Vec<u8> {
    let temp = Temp::new(&format!("{}_{cook}", if merged { "merged" } else { "plain" }));
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat/hero");
    let to = temp.0.join("content/hero");
    fs::create_dir_all(&to).expect("a folder");
    for entry in fs::read_dir(&from).expect("her folder") {
        let path = entry.expect("an entry").path();
        let name = path.file_name().expect("a name").to_string_lossy().into_owned();
        let source = name.strip_suffix(".meta").unwrap_or(&name);
        if LEFT_OUT.iter().any(|suffix| source.ends_with(suffix)) {
            continue;
        }
        fs::copy(&path, to.join(&name)).expect("copied");
    }
    if !merged {
        let sidecar = sapphire_pipeline::meta::sidecar_path(&to.join("hero.vrm"));
        let mut meta = sapphire_pipeline::meta::read(&sidecar).expect("her sidecar");
        let mesh = meta.importer.mesh_mut().expect("a model's sidecar");
        assert!(!mesh.merge.is_empty(), "her sidecar merges");
        mesh.merge.clear();
        fs::write(&sidecar, sapphire_serialize::to_ron(&meta).expect("a sidecar")).expect("written");
    }
    let options = CookOptions { cook, cache: None, locked: true, editor: false };
    let report = cook_dir(&temp.0.join("content"), &temp.0.join("loose"), &options).expect("it cooks");
    let packs = temp.0.join("packs");
    pack_cooked(&report, &packs, None).expect("it packs");
    fs::read(packs.join("shared.spack")).expect("the shared pack")
}

/// What a pack's textures weigh, tails and levels: all of them, those her model's materials reach whole, and
/// their tails alone, which is what stays resident however far she is.
#[derive(Debug, PartialEq)]
struct TextureBytes {
    packed: u64,
    reached: u64,
    tails: u64,
}

fn texture_bytes(pack: &[u8]) -> TextureBytes {
    let reader = PackReader::new(pack).expect("a valid pack");
    let toc = reader.toc();
    let find = |id: AssetId| toc.entries().iter().find(|entry| entry.id == id);
    let entry = |id: AssetId| find(id).expect("packed");
    let packed = toc
        .entries()
        .iter()
        .filter(|entry| matches!(entry.kind, AssetKind::Texture | AssetKind::TextureLevel))
        .map(|entry| entry.size)
        .sum();
    let mut reached = std::collections::BTreeSet::new();
    for &material in toc.dependencies(entry(ASTER.parse().unwrap())) {
        reached.extend(toc.dependencies(entry(material)).iter().copied());
    }
    let (mut whole, mut tails) = (0, 0);
    for texture in reached.into_iter().map(entry).filter(|entry| entry.kind == AssetKind::Texture) {
        tails += texture.size;
        whole += texture.size;
        let levels = (0..MAX_LEVELS).map_while(|level| find(TextureLevel::id(texture.id, level)));
        whole += levels.map(|level| level.size).sum::<u64>();
    }
    TextureBytes { packed, reached: whole, tails }
}

/// A texture of the pack whole: its tail with every level above it, as a capture draws one.
fn whole_texture(gpu: &GpuContext, reader: &PackReader<'_>, id: AssetId) -> GpuTexture {
    let bytes = reader.read(id).expect("a texture");
    let tail = TextureTail::from_ktx2(&bytes).expect("a valid texture");
    let above: Vec<_> =
        (0..tail.above).map(|level| reader.read(TextureLevel::id(id, level)).expect("a level")).collect();
    let above: Vec<&[u8]> =
        above.iter().map(|bytes| TextureLevel::read(bytes).expect("a valid level").data.as_slice()).collect();
    let levels = above.into_iter().chain(tail.texture.levels.iter().copied()).collect();
    let texture = Texture { desc: tail.whole(), levels };
    GpuTexture::upload(gpu, &texture, "texture").expect("uploaded")
}

/// One renderer with a pack uploaded: her model as its slots name, and her unmerged model's slots in flat
/// colours when `mask` is asked for.
struct Stage {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    mesh: MeshHandle,
    mask: Option<MeshHandle>,
    slots: Vec<String>,
    /// Her model's bounds' lowest and highest points.
    bounds: (Vec3, Vec3),
}

/// A flat anime character material of `rgb`, fully lit, with no rim or outline, uploaded.
fn flat(renderer: &mut Renderer, rgb: [f32; 3]) -> MaterialHandle {
    let document = AnimeCharacterMaterial {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        light_split: LightSplit { threshold: -1.0, softness: 0.0 },
        regions: vec![CharacterRegion { shade_color: [1.0; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let bytes = Material::from_document(&document, &swizzles).expect("valid").to_cooked().expect("cook");
    renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("uploaded")
}

impl Stage {
    fn new(gpu: &GpuContext, settings: RenderSettings, pack: &[u8], mask: bool) -> Self {
        let mut renderer = Renderer::new(gpu, FORMAT, settings).expect("renderer");
        let reader = PackReader::new(pack).expect("a valid pack");
        let entries = reader.toc().entries();
        let mut textures = HashMap::new();
        for entry in entries.iter().filter(|entry| entry.kind == AssetKind::Texture) {
            textures.insert(entry.id, whole_texture(gpu, &reader, entry.id));
        }
        let mut materials = HashMap::new();
        for entry in entries.iter().filter(|entry| entry.kind == AssetKind::Material) {
            let bytes = reader.read(entry.id).expect("a material");
            let material = Material::read(&bytes).expect("a valid material");
            let handle = renderer.upload_material(material, |id| textures.get(&id)).expect("uploaded");
            materials.insert(entry.id, handle);
        }
        let bytes = reader.read(ASTER.parse().unwrap()).expect("her model");
        let model = Mesh::read(&bytes).expect("a valid mesh");
        let mesh =
            renderer.upload_mesh_with_slots(model, |id| materials.get(&id).copied()).expect("uploaded");
        let slots: Vec<String> = model.materials.iter().map(|slot| slot.name.as_str().to_owned()).collect();
        let mask = mask.then(|| {
            let member = flat(&mut renderer, [1.0, 0.0, 0.0]);
            let other = flat(&mut renderer, [0.0, 1.0, 0.0]);
            let slots: Vec<MaterialHandle> = slots
                .iter()
                .map(|name| if MEMBERS.contains(&name.as_str()) { member } else { other })
                .collect();
            renderer.upload_mesh(model, &slots).expect("uploaded")
        });
        let bounds = &model.streams.bounds;
        let bounds = (
            Vec3::from_array(bounds.min.map(|value| value.to_native())),
            Vec3::from_array(bounds.max.map(|value| value.to_native())),
        );
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            mesh,
            mask,
            slots,
            bounds,
        }
    }

    /// The camera for `distance`: on +Z, looking at her, framing her figure at that share of the height.
    fn camera(&self, (_, share, at, _): (&str, f32, f32, f64)) -> CameraData {
        let (min, max) = self.bounds;
        let height = max.y - min.y;
        let target = Vec3::new(0.5 * (min.x + max.x), min.y + at * height, 0.5 * (min.z + max.z));
        let distance = 0.5 * height / (share * (0.5 * FOV).tan());
        let aspect = SIZE.0 as f32 / SIZE.1 as f32;
        CameraData::look_at(target + Vec3::Z * distance, target, Vec3::Y, FOV, aspect, 0.05)
    }

    /// Draws [`FRAMES`] frames of `mesh` seen by `camera`, listed as a character, and reads the last back,
    /// with the counts of the last.
    fn draw(
        &mut self,
        gpu: &GpuContext,
        mesh: MeshHandle,
        camera: CameraData,
    ) -> (Vec<[u8; 4]>, FrameCounts) {
        let sun = DirectionalLight {
            direction: Vec3::new(-0.45, -0.55, -0.7).normalize(),
            color: Vec3::ONE,
            intensity: 1.0,
        };
        let (min, max) = self.bounds;
        let id = InstanceId(1);
        for _ in 0..FRAMES {
            let packet = self.packets.begin();
            packet.views.push(ViewData::new(camera));
            packet.sun = sun;
            packet.sky = SkyGradient {
                zenith: Vec3::splat(0.35),
                horizon: Vec3::splat(0.5),
                ground: Vec3::splat(0.3),
            };
            packet.ambient =
                AmbientLight { sky: Vec3::splat(0.5), ground: Vec3::splat(0.4), intensity: 0.15 };
            packet.post = PostParams::default();
            // A mesh replaced under one instance: each draw sends it again.
            packet.instances.changed.push(MeshInstance { id, mesh, transform: Mat4::IDENTITY });
            packet.characters.push(CharacterInstance {
                id,
                lighting: CharacterLighting::unstyled(sun),
                face: CharacterFace::default(),
                body: CharacterBody {
                    base: Vec3::new(0.0, min.y, 0.0),
                    top: Vec3::new(0.0, max.y, 0.0),
                    radius: 0.25,
                    ..CharacterBody::default()
                },
                opacity: 1.0,
                drive: MaterialDrive::NONE,
            });
            let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
            self.renderer.prepare(gpu, self.packets.current());
            let view = self.target.create_view(&Default::default());
            let mut graph = FrameGraph::new();
            let target = graph.import_texture("target", &view);
            self.renderer.record(&mut graph, target).expect("a view and a target");
            self.graph.execute(gpu, graph).expect("a valid graph");
            let error = pollster::block_on(scope.pop());
            assert!(error.is_none(), "{error:?}");
        }
        (read_back(gpu, &self.target), self.renderer.stats().passes)
    }
}

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
    let (width, height) = (texture.width(), texture.height());
    let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.get_mapped_range(..).expect("mapped");
    mapped
        .chunks_exact(padded as usize)
        .flat_map(|row| row[..width as usize * 4].as_chunks::<4>().0.iter().copied())
        .collect()
}

/// Writes `pixels` as `<name>.png` into the folder `SAPPHIRE_MERGE_DUMP` names, if it names one: for looking
/// at a difference.
fn dump(name: &str, pixels: &[[u8; 4]]) {
    let Some(dir) = std::env::var_os("SAPPHIRE_MERGE_DUMP") else { return };
    let file = fs::File::create(Path::new(&dir).join(format!("{name}.png"))).expect("a file");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), SIZE.0, SIZE.1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("a header");
    writer.write_image_data(pixels.as_flattened()).expect("written");
}

/// The pixels within [`REACH`] of one the merged slots cover, from the mask's picture.
fn near_members(mask: &[[u8; 4]]) -> Vec<bool> {
    let (width, height) = (SIZE.0 as i32, SIZE.1 as i32);
    let member = |x: i32, y: i32| {
        let pixel = mask[(y * width + x) as usize];
        pixel[0] > 16 && pixel[0] > pixel[1]
    };
    let mut near = vec![false; mask.len()];
    for y in 0..height {
        for x in 0..width {
            if !member(x, y) {
                continue;
            }
            for ny in (y - REACH).max(0)..=(y + REACH).min(height - 1) {
                for nx in (x - REACH).max(0)..=(x + REACH).min(width - 1) {
                    near[(ny * width + nx) as usize] = true;
                }
            }
        }
    }
    near
}

/// How two pictures differ: the worst code value and the pixels more than one apart, near the merged slots
/// and away from them.
#[derive(Debug, Default)]
struct Difference {
    near_worst: u8,
    near_over_one: usize,
    near_pixels: usize,
    away_worst: u8,
    away_over_one: usize,
    away_differing: usize,
}

fn difference(a: &[[u8; 4]], b: &[[u8; 4]], near: &[bool]) -> Difference {
    let mut result = Difference::default();
    for ((a, b), &near) in a.iter().zip(b).zip(near) {
        let worst = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
        if near {
            result.near_pixels += 1;
            result.near_worst = result.near_worst.max(worst);
            result.near_over_one += usize::from(worst > 1);
        } else {
            result.away_worst = result.away_worst.max(worst);
            result.away_over_one += usize::from(worst > 1);
            result.away_differing += usize::from(worst > 0);
        }
    }
    result
}

/// The first of `pack`'s texture formats this device can't sample, if any.
fn missing_format(gpu: &GpuContext, pack: &[u8]) -> Option<String> {
    let reader = PackReader::new(pack).expect("a valid pack");
    let toc = reader.toc();
    toc.entries().iter().filter(|entry| entry.kind == AssetKind::Texture).find_map(|entry| {
        let bytes = reader.read(entry.id).expect("a texture");
        let format = TextureTail::from_ktx2(&bytes).expect("a valid texture").whole().format;
        check_format(gpu, format).err().map(|_| format!("{format:?}"))
    })
}

/// Each cook's tiers.
fn tiers(cook: Cook) -> [TierId; 2] {
    match cook {
        Cook::Desktop => [TierId::DesktopHigh, TierId::DesktopLow],
        Cook::Mobile => [TierId::MobileHigh, TierId::MobileLow],
    }
}

/// Her merge takes two slots off every pass that draws her, as many draws as it took off her model's
/// level (a surface and its hull each), and draws every triangle it drew; the merged and the unmerged pictures
/// agree within [`DISTANCES`]' bounds near the merged slots at each distance on every tier, and within one code
/// value away from them but for [`AWAY_OVER_ONE`] pixels.
#[test]
#[ignore = "needs a GPU"]
fn a_merged_character_draws_fewer_ranges_and_the_same_picture() {
    let gpu = gpu();
    let table = TierTable::shipped();
    for cook in [Cook::Desktop, Cook::Mobile] {
        let (merged_pack, plain_pack) = (aster_pack(true, cook), aster_pack(false, cook));
        let missing = missing_format(&gpu, &merged_pack).or_else(|| missing_format(&gpu, &plain_pack));
        if let Some(format) = missing {
            // A skip must be a device without the mobile cook's ASTC (CI's lavapipe and WARP), never a
            // lost check: the desktop cook draws everywhere, and an Apple GPU draws both.
            let caps = gpu.caps();
            assert_eq!(cook, Cook::Mobile, "{}: the desktop cook's {format} is missing", caps.adapter.name);
            assert_ne!(caps.backend(), wgpu::Backend::Metal, "an Apple GPU skips nothing: {format}");
            eprintln!("{} has no {format}: skipped the {cook} cook's half", caps.adapter.name);
            continue;
        }
        let (merged_bytes, plain_bytes) = (texture_bytes(&merged_pack), texture_bytes(&plain_pack));
        eprintln!("{cook}: texture bytes merged {merged_bytes:?}, unmerged {plain_bytes:?}");
        for tier in tiers(cook) {
            let settings = table.get(tier).render_settings();
            let mut merged = Stage::new(&gpu, settings, &merged_pack, false);
            let mut plain = Stage::new(&gpu, settings, &plain_pack, false);
            // The mask draws on a renderer of its own, so the other two draw the same frames, jitter and all.
            let mut masks = Stage::new(&gpu, settings, &plain_pack, true);
            assert_eq!((merged.slots.len(), plain.slots.len()), (4, 6), "{cook}");
            assert!(merged.slots.contains(&"body".to_owned()), "{:?}", merged.slots);
            for distance in DISTANCES {
                let camera = plain.camera(distance);
                let (mask, _) = masks.draw(&gpu, masks.mask.expect("asked for"), camera);
                let near = near_members(&mask);
                let label = format!("{cook}, {tier:?}, {}", distance.0);
                // Metal's main pass varies by itself now and then (`goldens.md`): a picture past the bound away
                // from the merged slots is drawn again, both of them, twice at most.
                let mut attempts = 0;
                let (difference, plain_counts, merged_counts) = loop {
                    let (before, plain_counts) = plain.draw(&gpu, plain.mesh, camera);
                    let (after, merged_counts) = merged.draw(&gpu, merged.mesh, camera);
                    let difference = difference(&before, &after, &near);
                    let name = format!("{cook}_{tier:?}_{}", distance.0);
                    dump(&format!("{name}_unmerged"), &before);
                    dump(&format!("{name}_merged"), &after);
                    dump(&format!("{name}_mask"), &mask);
                    attempts += 1;
                    if difference.away_over_one <= AWAY_OVER_ONE || attempts == 3 {
                        break (difference, plain_counts, merged_counts);
                    }
                    eprintln!("{label}: drawn again after {difference:?}");
                };
                eprintln!("{label}: {difference:?}");
                for ((name, before), (_, after)) in plain_counts.passes().iter().zip(merged_counts.passes()) {
                    if *before == after {
                        continue;
                    }
                    eprintln!(
                        "{label}: {name}: draws {} -> {}, outline draws {} -> {}, considered {} -> {}",
                        before.draws,
                        after.draws,
                        before.outline_draws,
                        after.outline_draws,
                        before.considered,
                        after.considered
                    );
                    assert_eq!(before.triangles, after.triangles, "{label}: {name}");
                    assert_eq!(
                        before.instances - after.instances,
                        before.draws - after.draws,
                        "{label}: {name}"
                    );
                    let outlines = before.outline_draws - after.outline_draws;
                    assert!(outlines == 0 || outlines == 2, "{label}: {name}");
                    let fewer = (before.draws - after.draws, before.considered - after.considered);
                    assert_eq!(fewer, (2 + outlines, 2 + outlines), "{label}: {name}");
                }
                let main =
                    (plain_counts.main.draws - merged_counts.main.draws, merged_counts.main.outline_draws);
                assert_eq!(main, (4, 4), "{label}: the main pass draws her four slots' surfaces and hulls");
                let share = difference.near_over_one as f64 / difference.near_pixels as f64;
                assert!(share <= distance.3, "{label}: near the merged slots: {share:.4}, {difference:?}");
                assert!(difference.away_over_one <= AWAY_OVER_ONE, "{label}: away from them: {difference:?}");
            }
        }
    }
}
