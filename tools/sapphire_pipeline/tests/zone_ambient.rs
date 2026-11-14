//! The shipped zones' ambient characters (M5 C10) against the content they name. A cook holds a cast's
//! meshes and graph to their kinds only where its tree has them, since the benchmark's district names this
//! tree's, so a mistyped ID would leave a zone without its crowd and no cook would say. This does.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use sapphire_assets::AssetId;
use sapphire_pipeline::meta::{self, ImportSettings, SourceTree};
use sapphire_zone::ZoneDocument;

#[test]
fn every_zones_ambient_characters_name_what_content_holds() {
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    // A generated map's inputs have no sidecar: they're part of its document's asset.
    let mut tree = SourceTree::scan(&content).expect("the content tree");
    let sidecars = tree.sidecars.clone();
    tree.sources.retain(|source| sidecars.contains(&meta::sidecar_path(source)));
    let resolved = meta::resolve(&tree, true).expect("sidecars");
    let importers: BTreeMap<AssetId, &ImportSettings> =
        resolved.assets.iter().map(|asset| (asset.meta.id, &asset.meta.importer)).collect();
    let mut crowds = 0;
    for asset in &resolved.assets {
        if !matches!(asset.meta.importer, ImportSettings::Zone(_)) {
            continue;
        }
        let text = fs::read_to_string(&asset.path).expect("a zone document");
        let document: ZoneDocument = sapphire_serialize::from_ron(&text).expect("it reads");
        let Some(ambient) = &document.ambient else { continue };
        crowds += 1;
        let zone = asset.path.display();
        assert!(
            matches!(importers.get(&ambient.graph), Some(ImportSettings::AnimGraph(_))),
            "{zone}: `ambient.graph` {} is no animation graph in content",
            ambient.graph
        );
        for member in &ambient.cast {
            // A model whose skin plays on a skeleton: its own or, as the shipped cast's, another's.
            let model = importers.get(&member.mesh).and_then(|importer| importer.mesh());
            assert!(
                model.is_some(),
                "{zone}: cast member {:?}'s mesh {} is no model in content",
                member.name,
                member.mesh
            );
        }
    }
    assert!(crowds > 0, "the proving hub has people");
}
