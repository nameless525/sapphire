use super::*;

fn id(n: u8) -> AssetId {
    format!("00000000-0000-4000-8000-0000000000{n:02x}").parse().unwrap()
}

fn asset(kind: &str, source: &str, part: bool, uses: &[u8], used_by: &[u8]) -> IndexedAsset {
    IndexedAsset {
        kind: kind.to_owned(),
        source: source.to_owned(),
        output: source.to_owned(),
        name: String::new(),
        part,
        pack: "shared".to_owned(),
        bytes: 1,
        uses: uses.iter().map(|&n| id(n)).collect(),
        used_by: used_by.iter().map(|&n| id(n)).collect(),
        texture: None,
        mesh: None,
        material: None,
        check: None,
    }
}

/// A texture with a level, used by two materials, both used by one mesh, which a zone uses.
fn index() -> AssetIndex {
    let assets = BTreeMap::from([
        (id(1), asset("texture", "body/albedo.png", false, &[2], &[3, 4])),
        (id(2), asset("texture level", "body/albedo.png", true, &[], &[1])),
        (id(3), asset("material", "body/skin.material.ron", false, &[1], &[5])),
        (id(4), asset("material", "body/cloth.material.ron", false, &[1], &[5])),
        (id(5), asset("mesh", "body/body.glb", false, &[3, 4], &[6])),
        (id(6), asset("zone", "town/town.zone.ron", false, &[5], &[])),
    ]);
    AssetIndex {
        version: VERSION,
        cook: "desktop".to_owned(),
        tiers: tiers(Cook::Desktop).map(str::to_owned).to_vec(),
        kinds: BTreeMap::new(),
        packs: BTreeMap::new(),
        check: CheckSummary::default(),
        assets,
    }
}

fn walk(index: &AssetIndex, root: u8, reverse: bool) -> Vec<(u8, u32, bool)> {
    let number = |step: &Step| (0..=255).find(|&n| id(n) == step.id).unwrap();
    index.graph(id(root), reverse).iter().map(|step| (number(step), step.depth, step.repeated)).collect()
}

#[test]
fn the_reverse_graph_says_what_uses_a_texture_all_the_way_up() {
    let index = index();
    assert_eq!(
        walk(&index, 1, true),
        [(1, 0, false), (3, 1, false), (5, 2, false), (6, 3, false), (4, 1, false), (5, 2, true)],
        "both materials, the mesh once, walked from the first, and the zone"
    );
    let text = index.show(&index.graph(id(1), true), true).to_string();
    assert!(
        text.contains(
            "  used by 00000000-0000-4000-8000-000000000003 material body/skin.material.ron [shared]"
        )
    );
    assert!(text.ends_with("4 asset(s) use it, all the way down"), "{text}");
    assert!(text.contains("(as above)"));
}

#[test]
fn the_forward_graph_says_what_an_asset_uses() {
    let index = index();
    assert_eq!(
        walk(&index, 5, false),
        [(5, 0, false), (3, 1, false), (1, 2, false), (2, 3, false), (4, 1, false), (1, 2, true)]
    );
    assert_eq!(walk(&index, 2, false), [(2, 0, false)], "a level uses nothing");
}

#[test]
fn an_asset_is_found_by_its_id_or_its_sources_own_asset() {
    let index = index();
    assert_eq!(index.find("00000000-0000-4000-8000-000000000005"), Some(id(5)));
    assert_eq!(index.find("body/albedo.png"), Some(id(1)), "the texture, not its level");
    assert_eq!(index.find("body\\body.glb"), Some(id(5)), "a path as Windows writes it");
    assert_eq!(index.find("00000000-0000-4000-8000-0000000000ff"), None, "not in this cook");
    assert_eq!(index.find("body/none.png"), None);
}

#[test]
fn an_index_reads_back_as_written_and_another_version_is_refused() {
    let dir = crate::test_util::TempDir::new("index_read");
    let path = dir.path().join(FILE_NAME);
    let index = index();
    fs::write(&path, index.to_json(&path).unwrap()).unwrap();
    assert_eq!(AssetIndex::read(&path).unwrap(), index);

    let old = AssetIndex { version: VERSION + 1, ..index };
    fs::write(&path, old.to_json(&path).unwrap()).unwrap();
    let error = AssetIndex::read(&path).unwrap_err().to_string();
    assert!(error.contains("cook again"), "{error}");
    fs::write(&path, format!("{{\"version\": {VERSION}, \"cook\": 3}}")).unwrap();
    assert!(matches!(AssetIndex::read(&path), Err(IndexError::Unreadable { .. })));
}

#[test]
fn bumping_a_stage_the_index_holds_or_changing_a_row_misses_the_kept_index() {
    let assets = vec![(id(1), "body/albedo.png".to_owned())];
    let sidecars = vec![("body/albedo.png".to_owned(), vec![1_u8, 2, 3])];
    let rows = check::ROWS;
    let kept = key(&STAGES, &rows, Cook::Desktop, &assets, &sidecars);
    assert!(
        kept == key(&STAGES, &rows, Cook::Desktop, &assets, &sidecars),
        "the same inputs keep the same key"
    );
    assert_eq!(
        STAGES.map(|stage| stage.name),
        [INDEXER.name, audit::AUDITOR.name, check::CHECKER.name, pack::PACKER.name, pack::REACH.name]
    );
    for at in 0..STAGES.len() {
        let mut bumped = STAGES;
        bumped[at].version += 1;
        assert!(key(&bumped, &rows, Cook::Desktop, &assets, &sidecars) != kept, "{} bumped", STAGES[at].name);
    }
    let mut changed = rows;
    changed[0][1].triangles += 1;
    assert!(key(&STAGES, &changed, Cook::Desktop, &assets, &sidecars) != kept, "a changed row");
    assert!(key(&STAGES, &rows, Cook::Mobile, &assets, &sidecars) != kept, "each cook its own index");
}
