//! What uses a texture, as a desktop cook's asset index says (M6 C2): its material, and through it the
//! character that binds the material, each in the shared pack.
//!
//! The first test cooks a scratch tree of committed sources, the Blender add-on's test character with its
//! cloth bound to the sample ground material, and reads the index that cook wrote, so it runs wherever the
//! tests do. The second reads `target/content/loose/desktop/index.json` for the look-dev character, so it is
//! ignored until `cargo xtask content` has cooked that: `cargo test -p sapphire_pipeline --test asset_index
//! -- --ignored` after it (fixed after its review, M6 backfill).

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::AssetId;
use sapphire_pipeline::audit;
use sapphire_pipeline::cook::{Cook, CookOptions, cook_dir};
use sapphire_pipeline::index::{self, AssetIndex};

/// The test character, as the add-on's export writes it (`tools/blender/test_character.py`), and its sidecar.
const CHARACTER: &str = "fixtures/character/test_character.glb";

/// The sample ground's texture and the material binding it, with their IDs as their sidecars have them.
const GROUND: [(&str, &str); 2] = [
    ("samples/environment/ground.png", "9792c615-2cbd-494c-9681-750dfbc10e31"),
    ("samples/environment/ground.material.ron", "fdb90952-4488-42d6-a474-55115aa2db08"),
];

/// The test character's ID, as its sidecar has it.
const CHARACTER_ID: &str = "427a54bb-7a15-465e-ba2a-c583c1e57380";

fn crate_file(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn id(text: &str) -> AssetId {
    text.parse().unwrap()
}

/// Copies `from` and its sidecar into `dir`, giving back the copied sidecar's path.
fn copy_with_sidecar(from: &Path, dir: &Path) -> PathBuf {
    let name = from.file_name().expect("a file").to_str().expect("a name");
    fs::copy(from, dir.join(name)).expect("copies the source");
    let sidecar = format!("{name}.meta");
    fs::copy(from.with_file_name(&sidecar), dir.join(&sidecar)).expect("copies its sidecar");
    dir.join(sidecar)
}

#[test]
fn a_cooks_index_says_what_uses_a_texture_all_the_way_up() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("asset_index");
    let _ = fs::remove_dir_all(&root);
    let (source, out, cache) = (root.join("content"), root.join("loose"), root.join("cache"));
    let characters = source.join("characters");
    let environment = source.join("environment");
    for dir in [&characters, &environment] {
        fs::create_dir_all(dir).expect("a scratch tree");
    }
    for (path, _) in GROUND {
        copy_with_sidecar(&crate_file("../../content").join(path), &environment);
    }
    // The character's cloth slot drawn with the ground material, so the texture has a model above it.
    let sidecar = copy_with_sidecar(&crate_file(CHARACTER), &characters);
    let text = fs::read_to_string(&sidecar).expect("the character's sidecar");
    assert!(text.contains("materials: {},"), "{text}");
    let bound = text.replace("materials: {},", &format!("materials: {{\"Cloth\": \"{}\"}},", GROUND[1].1));
    fs::write(&sidecar, bound).expect("binds the cloth");

    let cook = Cook::Desktop;
    let options = CookOptions { cook, cache: Some(&cache), locked: true, editor: false };
    let report = cook_dir(&source, &out, &options).expect("cooks");
    let audited = audit::write(&source, &out, cook, Some(&cache), &report).expect("audits");
    let indexed = index::write(&source, &out, cook, Some(&cache), &report, &audited.audit).expect("indexes");
    let checked = indexed.checked.expect("a cold index checks the tree");
    assert_eq!(checked.meshes.anew, 2, "the hero cooked to memory for each cook it's judged on: {checked}");
    let index = AssetIndex::read(&indexed.path).expect("reads the index back");

    let (albedo, material, character) = (id(GROUND[0].1), id(GROUND[1].1), id(CHARACTER_ID));
    assert_eq!(index.find("environment/ground.png"), Some(albedo));
    let entry = &index.assets[&albedo];
    assert_eq!((entry.kind.as_str(), entry.pack.as_str()), ("texture", "shared"));
    assert_eq!(entry.used_by, [material], "the ground material alone binds it");
    let texture = entry.texture.as_ref().expect("a texture's entry");
    assert!(texture.thumbnail.width.max(texture.thumbnail.height) <= 128, "{texture:?}");
    assert_eq!(index.assets[&material].source, "environment/ground.material.ron");

    let walk = index.graph(albedo, true);
    let reached: Vec<AssetId> = walk.iter().filter(|step| !step.repeated).map(|step| step.id).collect();
    assert_eq!(
        reached[..3],
        [albedo, material, character],
        "the texture, its material, the model drawing it"
    );
    let model = &index.assets[&character];
    assert_eq!((model.kind.as_str(), model.source.as_str()), ("mesh", "characters/test_character.glb"));
    let slots = &model.mesh.as_ref().expect("a mesh's entry").slots;
    assert!(slots.iter().any(|slot| slot == "Cloth"), "{slots:?}");
    let check = model.check.as_ref().expect("the test character is cast as a hero");
    assert_eq!((check.role.as_deref(), check.cooks.len()), (Some("hero"), 2));
    let text = index.show(&walk, true).to_string();
    assert!(text.contains(&format!("used by {material} material")), "{text}");

    // Cooked again unchanged, the whole index is the cache's: nothing is checked.
    let again = cook_dir(&source, &out, &options).expect("cooks again");
    let audited = audit::write(&source, &out, cook, Some(&cache), &again).expect("audits again");
    let indexed = index::write(&source, &out, cook, Some(&cache), &again, &audited.audit).expect("indexes");
    assert_eq!((indexed.written, indexed.checked), (audit::Written::UpToDate, None));
    let _ = fs::remove_dir_all(&root);
}

#[test]
#[ignore = "reads the look-dev character's cook: run `cargo xtask content` first"]
fn the_desktop_index_says_what_uses_the_look_dev_bodys_albedo() {
    let path = crate_file("../../target/content/loose/desktop").join(index::FILE_NAME);
    let index = AssetIndex::read(&path).expect("`cargo xtask content` has cooked the index at this version");
    let albedo = index
        .find("third_party/vrm1_constraint_twist_sample/extracted/Body_00.png")
        .expect("the body's albedo is in the desktop cook");
    assert_eq!(albedo, id("dff507ce-eb5b-4b0a-af47-3d60aa6d39ab"));
    let material = id("c0493afd-d738-4efd-b711-4e19632642c9");
    let character = id("6f8a81b6-29c4-485f-b161-847b001d561f");

    let entry = &index.assets[&albedo];
    assert_eq!(entry.kind, "texture");
    assert_eq!(entry.used_by, [material], "the body's skin material alone binds it");
    let texture = entry.texture.as_ref().expect("a texture's entry");
    assert!(texture.thumbnail.width.max(texture.thumbnail.height) <= 128, "{texture:?}");
    assert_eq!(index.assets[&material].source, "lookdev/character/materials/Body_00_SKIN.material.ron");

    let walk = index.graph(albedo, true);
    let reached: Vec<AssetId> = walk.iter().filter(|step| !step.repeated).map(|step| step.id).collect();
    assert_eq!(reached[..3], [albedo, material, character], "the albedo, its material, the model drawing it");
    let model = &index.assets[&character];
    assert_eq!((model.kind.as_str(), model.source.as_str()), ("mesh", "lookdev/character/character.vrm"));
    let check = model.check.as_ref().expect("the look-dev character is cast as a hero");
    assert_eq!(check.role.as_deref(), Some("hero"));
    assert!(check.findings.iter().any(|finding| finding.rule == "slots" && finding.waived.is_some()));
    let text = index.show(&walk, true).to_string();
    assert!(text.contains("used by c0493afd-d738-4efd-b711-4e19632642c9 material"), "{text}");
}
