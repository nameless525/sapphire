//! What uses the look-dev character's body albedo, as the desktop cook's asset index says (M6 C2): its
//! material, and through it the character's model, each in the shared pack.
//! Reads `target/content/loose/desktop/index.json`, so it says nothing until `cargo xtask content` has cooked
//! it, and skips an index of another version. `sapphire_pipeline::index`'s own tests and the cook's
//! (`every_cook_writes_its_index_byte_for_byte_from_any_root`) cook what they read.

#![allow(clippy::print_stdout, reason = "says why it skipped")]

use std::path::PathBuf;

use sapphire_assets::AssetId;
use sapphire_pipeline::index::{self, AssetIndex, IndexError};

fn index_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/content/loose/desktop")
        .join(index::FILE_NAME)
}

fn id(text: &str) -> AssetId {
    text.parse().unwrap()
}

#[test]
fn the_desktop_index_says_what_uses_the_look_dev_bodys_albedo() {
    let path = index_path();
    if !path.exists() {
        println!("skipped: no asset index under target/content; run `cargo xtask content` first");
        return;
    }
    let index = match AssetIndex::read(&path) {
        Ok(index) => index,
        Err(IndexError::Invalid { why, .. }) => {
            println!("skipped: {why}; run `cargo xtask content` again");
            return;
        }
        Err(error) => panic!("{error}"),
    };
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
