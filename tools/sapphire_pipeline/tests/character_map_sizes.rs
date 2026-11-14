//! Spec §3.4's budget for a character's maps, 2K a part on desktop and 1K on mobile (M5 M1): every map an
//! anime character material in `content/` binds cooks within it on each cook, by its sidecar's
//! `max_size`. A new character's maps come under this test by being bound to a character material; one
//! over the budget gets a `max_size` in its sidecar (a texture's, a procedural texture's or a generated
//! character map's). The benchmark's cast isn't in `content/`: its own test holds its maps to the same
//! budget (`sapphire_pipeline::bench::cast`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::AssetId;
use sapphire_assets::material::AnyMaterialDocument;
use sapphire_pipeline::character_maps;
use sapphire_pipeline::cook::Cook;
use sapphire_pipeline::import::image::{SourceFormat, header_size};
use sapphire_pipeline::material;
use sapphire_pipeline::meta::{self, ImportSettings, SourceAsset, SourceTree};
use sapphire_pipeline::procedural;
use sapphire_pipeline::texture::{self, TextureSettings};

/// The longest side a character part's map may cook to, per cook (spec §3.4).
const BUDGET: [(Cook, u32); 2] = [(Cook::Desktop, 2048), (Cook::Mobile, 1024)];

fn repository(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).expect("tools/<crate>").join(relative)
}

#[test]
fn every_character_map_cooks_within_its_budget() {
    let content = repository("content");
    // A generated map's inputs have no sidecar: they're part of its document's asset.
    let mut tree = SourceTree::scan(&content).unwrap();
    let sidecars = tree.sidecars.clone();
    tree.sources.retain(|source| sidecars.contains(&meta::sidecar_path(source)));
    let resolved = meta::resolve(&tree, true).unwrap();
    let cooked: Vec<&SourceAsset> = resolved.assets.iter().filter(|asset| asset.meta.cook).collect();
    let by_id: HashMap<AssetId, &SourceAsset> = cooked.iter().map(|asset| (asset.meta.id, *asset)).collect();

    let mut maps: Vec<&SourceAsset> = Vec::new();
    for asset in cooked.iter().filter(|asset| matches!(asset.meta.importer, ImportSettings::Material(_))) {
        let document = material::read(&fs::read(&asset.path).unwrap()).unwrap();
        if !matches!(document, AnyMaterialDocument::AnimeCharacter(_)) {
            continue;
        }
        for id in document.textures().into_iter().flatten() {
            let map = by_id
                .get(&id)
                .unwrap_or_else(|| panic!("{}: no cooked source has {id}", asset.path.display()));
            if !maps.iter().any(|known| known.meta.id == id) {
                maps.push(map);
            }
        }
    }
    // The look-dev character's, the fighters' and the samples': a walk that found none proves nothing.
    assert!(maps.len() >= 30, "{} character maps", maps.len());

    let mut over = Vec::new();
    for map in maps {
        let bytes = fs::read(&map.path).unwrap();
        let name = map.path.strip_prefix(&content).unwrap().display();
        for (cook, budget) in BUDGET {
            let (width, height) = cooked_size(&map.path, &bytes, &map.meta.importer, cook);
            if width.max(height) > budget {
                over.push(format!("{name} ({cook}): {width} × {height}, over {budget}"));
            }
        }
    }
    assert!(
        over.is_empty(),
        "character maps over spec §3.4's budget; cap each in its sidecar:\n{}",
        over.join("\n")
    );
}

/// The size `cook` stores for the map at `path`, planned as its cook plans it.
fn cooked_size(path: &Path, bytes: &[u8], importer: &ImportSettings, cook: Cook) -> (u32, u32) {
    let name = path.display();
    match importer {
        ImportSettings::Texture(settings) => {
            let format = SourceFormat::from_path(path).unwrap_or_else(|| panic!("{name}: not an image"));
            let (width, height) = header_size(bytes, format).unwrap_or_else(|| panic!("{name}: no header"));
            texture::cooked_size(settings, cook, width, height)
                .unwrap_or_else(|error| panic!("{name}: {error}"))
        }
        ImportSettings::ProceduralTexture(settings) => {
            procedural::cooked_size(bytes, settings, cook).unwrap_or_else(|error| panic!("{name}: {error}"))
        }
        // A generated map is the size of what it's generated from, under its sidecar's cap.
        ImportSettings::CharacterMap(settings, max_size) => {
            let map = character_maps::generate(path, bytes, settings)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            let settings = TextureSettings { class: map.class, max_size: *max_size };
            texture::cooked_size(&settings, cook, map.image.width, map.image.height)
                .unwrap_or_else(|error| panic!("{name}: {error}"))
        }
        other => panic!("{name} is bound as a map but imports as {other:?}"),
    }
}
