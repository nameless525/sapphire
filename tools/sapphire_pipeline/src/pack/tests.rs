use std::sync::Arc;

use sapphire_assets::material::{AnimeCharacterMaterial, Material};
use sapphire_assets::mesh::Mesh;
use sapphire_assets::pack::PackReader;
use sapphire_assets::{AssetBudgets, AssetServer, Handle, MemoryPackSource};

use super::*;
use crate::cook::{Cook, CookOptions, cook_dir};
use crate::material::MaterialSettings;
use crate::meta::{AssetMeta, ImportSettings, MeshSettings, sidecar_path};
use crate::samples;
use crate::test_util::{TempDir, read_aligned};
use crate::texture::{ColorSettings, TextureClass, TextureSettings};

const ALBEDO: &str = "5b7e1c2d-3a4f-4b6c-8d9e-0f1a2b3c4d5e";
const HAIR: &str = "9a8b7c6d-5e4f-4a3b-9c2d-1e0f9a8b7c6d";
const MANNEQUIN: &str = "1f2e3d4c-5b6a-4978-8695-a4b3c2d1e0f9";

fn id(text: &str) -> AssetId {
    text.parse().unwrap()
}

/// A content tree, its cooks' loose outputs, packs and cache, in one scratch directory.
struct Scratch {
    dir: TempDir,
}

impl Scratch {
    /// The mannequin with its hair slot bound to a material whose base color is a texture.
    fn new(name: &str) -> Self {
        let scratch = Self { dir: TempDir::new(name) };
        scratch.write_albedo(1);
        let material =
            AnimeCharacterMaterial { base_color_map: Some(id(ALBEDO)), ..AnimeCharacterMaterial::default() };
        let path = scratch.source().join("hair.material.ron");
        fs::write(&path, sapphire_serialize::to_ron(&material).unwrap()).unwrap();
        write_sidecar(&path, HAIR, ImportSettings::Material(MaterialSettings::default()));
        let path = scratch.source().join("mannequin.glb");
        fs::write(&path, samples::mannequin_glb()).unwrap();
        let mut settings = MeshSettings::default();
        settings.materials.insert("Hair".into(), id(HAIR));
        write_sidecar(&path, MANNEQUIN, ImportSettings::Gltf(settings));
        scratch
    }

    fn source(&self) -> PathBuf {
        self.dir.path().join("content")
    }

    fn packs(&self, cook: Cook) -> PathBuf {
        self.dir.path().join("packs").join(cook.name())
    }

    fn cache(&self) -> PathBuf {
        self.dir.path().join("cache")
    }

    /// A 16 × 16 PNG albedo, its texels from `seed`.
    fn write_albedo(&self, seed: u8) {
        let path = self.source().join("albedo.png");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let image = ::image::RgbaImage::from_fn(16, 16, |x, y| {
            ::image::Rgba([(x * 16) as u8 ^ seed, (y * 16) as u8, seed, 255])
        });
        image.save_with_format(&path, ::image::ImageFormat::Png).unwrap();
        write_sidecar(
            &path,
            ALBEDO,
            ImportSettings::Texture(TextureSettings { class: TextureClass::Color(ColorSettings::default()) }),
        );
    }

    fn cook(&self, cook: Cook) -> CookReport {
        let cache = self.cache();
        let options = CookOptions { cook, cache: Some(&cache), locked: false };
        cook_dir(&self.source(), &self.dir.path().join("loose").join(cook.name()), &options).unwrap()
    }

    /// Cooks and packs, with the cache or without.
    fn pack(&self, cook: Cook, cached: bool) -> (CookReport, PackReport) {
        let report = self.cook(cook);
        let cache = self.cache();
        let packed = pack_cooked(&report, &self.packs(cook), cached.then_some(cache.as_path())).unwrap();
        (report, packed)
    }

    fn shared(&self, cook: Cook) -> Vec<u8> {
        fs::read(self.packs(cook).join("shared.spack")).unwrap()
    }
}

fn write_sidecar(source: &Path, id: &str, importer: ImportSettings) {
    let meta = AssetMeta { id: id.parse().unwrap(), importer };
    fs::write(sidecar_path(source), sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
}

fn status(report: &PackReport) -> (PackStatus, usize) {
    let [pack] = &report.packs[..] else { panic!("one pack: {report:?}") };
    (pack.status, pack.encoded)
}

#[test]
fn packs_hold_every_cooked_asset_as_cooked() {
    let scratch = Scratch::new("pack_contents");
    let (report, packed) = scratch.pack(Cook::Desktop, true);
    let [pack] = &packed.packs[..] else { panic!("one pack") };
    assert_eq!((pack.name.as_str(), pack.assets, pack.status), (SHARED, 3, PackStatus::Packed));
    let bytes = scratch.shared(Cook::Desktop);
    assert_eq!(pack.bytes, bytes.len() as u64);
    let reader = PackReader::new(&bytes).unwrap();
    let toc = reader.toc();
    for asset in &report.assets {
        let entry = toc.find(asset.id).expect("packed");
        assert_eq!(reader.read(asset.id).unwrap().as_slice(), fs::read(&asset.output).unwrap(), "as cooked");
        let (kind, codec, dependencies) = match asset.id {
            id if id == self::id(MANNEQUIN) => (AssetKind::Mesh, Codec::Zstd, vec![self::id(HAIR)]),
            id if id == self::id(HAIR) => (AssetKind::Material, Codec::Lz4, vec![self::id(ALBEDO)]),
            _ => (AssetKind::Texture, Codec::Zstd, vec![]),
        };
        assert_eq!(
            (entry.kind, entry.codec, toc.dependencies(entry)),
            (kind, codec, dependencies.as_slice())
        );
    }
    // Compression pays: the pack is smaller than its cooked files.
    let loose: u64 = report.assets.iter().map(|asset| fs::metadata(&asset.output).unwrap().len()).sum();
    assert!(pack.bytes < loose, "{} bytes packed from {loose}", pack.bytes);

    // The runtime loads the mannequin by ID, its material and texture first.
    let source = Arc::new(MemoryPackSource::new());
    source.insert(SHARED, bytes);
    let mut server = AssetServer::new(source, AssetBudgets::DESKTOP);
    server.mount(SHARED).unwrap();
    let mesh: Handle<Mesh> = server.load(id(MANNEQUIN)).unwrap();
    let mut order = Vec::new();
    server
        .finish(&mesh, |upload| {
            order.push(upload.kind());
            if let Some(mesh) = upload.get::<Mesh>() {
                assert_eq!(mesh.materials.len(), 5);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(order, [AssetKind::Texture, AssetKind::Material, AssetKind::Mesh]);
    let material: Handle<Material> = server.load(id(HAIR)).unwrap();
    let cooked =
        Material::from_cooked(&read_aligned(&scratch.dir.path().join("loose/desktop/hair.smat"))).unwrap();
    assert_eq!(server.get(&material).unwrap().params(), cooked.params.as_slice());
}

#[test]
fn packing_is_deterministic_and_cached() {
    let scratch = Scratch::new("pack_cache");
    let (_, first) = scratch.pack(Cook::Desktop, true);
    assert_eq!(status(&first), (PackStatus::Packed, 3));
    let bytes = scratch.shared(Cook::Desktop);
    let path = scratch.packs(Cook::Desktop).join("shared.spack");
    let written = fs::metadata(&path).unwrap().modified().unwrap();

    // Nothing changed: the pack's recorded hash matches, so nothing is read or written.
    assert_eq!(status(&scratch.pack(Cook::Desktop, true).1), (PackStatus::UpToDate, 0));
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), written);

    // A deleted or hand-edited pack is rebuilt from cached encodings, byte for byte.
    fs::remove_file(&path).unwrap();
    assert_eq!(status(&scratch.pack(Cook::Desktop, true).1), (PackStatus::Packed, 0));
    assert!(scratch.shared(Cook::Desktop) == bytes);
    fs::write(&path, b"edited by hand").unwrap();
    assert_eq!(status(&scratch.pack(Cook::Desktop, true).1), (PackStatus::Packed, 0));
    assert!(scratch.shared(Cook::Desktop) == bytes);
    let written = fs::metadata(&path).unwrap().modified().unwrap();

    // Without a cache every asset is encoded again, to the same bytes, and the file isn't
    // rewritten.
    assert_eq!(status(&scratch.pack(Cook::Desktop, false).1), (PackStatus::Packed, 3));
    assert!(scratch.shared(Cook::Desktop) == bytes);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), written);

    // A changed texture re-encodes it alone: the material recooks (its key covers the texture's),
    // but to the same bytes, so its encoding is reused, and the mesh is unchanged.
    scratch.write_albedo(9);
    assert_eq!(status(&scratch.pack(Cook::Desktop, true).1), (PackStatus::Packed, 1));
    assert!(scratch.shared(Cook::Desktop) != bytes);

    // The mobile cook packs its own outputs. Its material cooks to the desktop's bytes (a color
    // texture has no swizzle in either cook), so that encoding is shared.
    let (_, mobile) = scratch.pack(Cook::Mobile, true);
    assert_eq!(status(&mobile), (PackStatus::Packed, 2));
    assert!(scratch.shared(Cook::Mobile) != scratch.shared(Cook::Desktop));
}

#[test]
fn files_no_pack_claims_are_removed() {
    let scratch = Scratch::new("pack_prune");
    let packs = scratch.packs(Cook::Desktop);
    fs::create_dir_all(packs.join("samples")).unwrap();
    for stale in ["old.spack", "old.spack.tmp", "samples/mannequin.smesh", "readme.txt"] {
        fs::write(packs.join(stale), b"stale").unwrap();
    }
    let (_, report) = scratch.pack(Cook::Desktop, true);
    assert_eq!(
        report.removed,
        [packs.join("old.spack"), packs.join("old.spack.tmp"), packs.join("samples/mannequin.smesh")]
    );
    assert!(!packs.join("samples").exists(), "emptied directories go");
    assert!(packs.join("readme.txt").exists(), "files of other kinds stay");
}

#[test]
fn every_dependency_must_be_packed() {
    let scratch = Scratch::new("pack_missing");
    let mut report = scratch.cook(Cook::Desktop);
    report.assets.retain(|asset| asset.id != id(ALBEDO));
    let error = pack_cooked(&report, &scratch.packs(Cook::Desktop), None).unwrap_err();
    assert!(
        matches!(error, PackError::MissingDependency { id: missing, .. } if missing == id(ALBEDO)),
        "{error:?}"
    );
}

#[test]
fn packs_never_share_a_directory_with_cooked_files_or_trust_changed_ones() {
    let scratch = Scratch::new("pack_overlap");
    let report = scratch.cook(Cook::Desktop);
    let loose = scratch.dir.path().join("loose");
    let error = pack_cooked(&report, &loose, None).unwrap_err();
    assert!(matches!(error, PackError::Overlap { .. }), "{error:?}");
    assert!(report.assets.iter().all(|asset| asset.output.exists()), "nothing was pruned");

    // A cooked file edited after its cook is refused rather than packed.
    let texture = report.assets.iter().find(|asset| asset.id == id(ALBEDO)).unwrap();
    let mut bytes = fs::read(&texture.output).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&texture.output, bytes).unwrap();
    let error = pack_cooked(&report, &scratch.packs(Cook::Desktop), None).unwrap_err();
    assert!(matches!(error, PackError::Changed { .. }), "{error:?}");
}

#[test]
fn sub_assets_are_packed_and_load_by_their_own_ids() {
    use sapphire_assets::rig::CharacterRig;

    // A VRM cooks to its mesh and, as a sub-asset, its rig (M1 A6).
    let dir = TempDir::new("pack_vrm");
    let source = dir.path().join("content/characters/sample.vrm");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, crate::import::vrm::samples::vrm1().to_glb()).unwrap();
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false };
    let report = cook_dir(&dir.path().join("content"), &dir.path().join("loose"), &options).unwrap();
    let packs = dir.path().join("packs");
    pack_cooked(&report, &packs, None).unwrap();
    let bytes = fs::read(packs.join("shared.spack")).unwrap();
    let mesh_id = crate::meta::read(&sidecar_path(&source)).unwrap().id;
    let rig_id = crate::meta::sub_asset_id(mesh_id, "rig");
    {
        let reader = PackReader::new(&bytes).unwrap();
        let entry = |id| *reader.toc().find(id).expect("packed");
        assert_eq!((entry(mesh_id).kind, entry(mesh_id).codec), (AssetKind::Mesh, Codec::Zstd));
        assert_eq!((entry(rig_id).kind, entry(rig_id).codec), (AssetKind::Rig, Codec::Lz4));
    }

    // The runtime loads the rig by its derived ID; it never goes to the uploader.
    let source = Arc::new(MemoryPackSource::new());
    source.insert(SHARED, bytes);
    let mut server = AssetServer::new(source, AssetBudgets::DESKTOP);
    server.mount(SHARED).unwrap();
    let rig: Handle<CharacterRig> = server.load(rig_id).unwrap();
    server.finish(&rig, |_| Err("rigs need no upload".into())).unwrap();
    let mesh: Handle<Mesh> = server.load(mesh_id).unwrap();
    let mut joints = 0;
    server
        .finish(&mesh, |upload| {
            if let Some(mesh) = upload.get::<Mesh>() {
                joints = mesh.skin.as_ref().map_or(0, |skin| skin.joints.len());
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(server.get(&rig).unwrap().joint_count.to_native() as usize, joints);
}

#[test]
fn the_packers_name_tracks_its_encoders() {
    let packages: Vec<(&str, &str)> = include_str!("../../../../Cargo.lock")
        .split("[[package]]")
        .filter_map(|package| {
            let field = |key: &str| {
                let start = package.find(&format!("\n{key} = \""))? + key.len() + 5;
                package[start..].split('"').next()
            };
            Some((field("name")?, field("version")?))
        })
        .collect();
    let version = |name: &str| -> Vec<&str> {
        packages.iter().filter(|(package, _)| *package == name).map(|&(_, version)| version).collect()
    };
    let ([zstd_sys], [lz4_flex]) = (&version("zstd-sys")[..], &version("lz4_flex")[..]) else {
        panic!("one zstd-sys and one lz4_flex in Cargo.lock");
    };
    let zstd = zstd_sys.split("+zstd.").nth(1).expect("zstd-sys versions carry the library's");
    assert_eq!(
        PACKER.name,
        format!("spack (zstd {zstd}, lz4_flex {lz4_flex})"),
        "a new encoder may compress differently: rename the packer with it"
    );
}
