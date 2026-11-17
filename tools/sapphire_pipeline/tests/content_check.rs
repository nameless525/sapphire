//! `content/` passes `sapphire_pipeline check` (M6 C1), the rules an artist's tools and CI apply alike: the
//! cast at its roles (spec §3.4's rows, the hero's and the proposed enemy, boss and ambient rows), every map
//! an anime character material names within the phones' 1K a side (M5 M1's part budget), and the content
//! rules. A new character comes under it by naming its role in its sidecar (`budget: (role: Hero)`); a map
//! over the budget gets a `max_size` in its sidecar. The benchmark's cast isn't in `content/`: its own tests
//! hold it to the same rows (`sapphire_pipeline::bench::cast`).

use std::path::{Path, PathBuf};

use sapphire_pipeline::check::{self, Role, Rule};
use sapphire_pipeline::cook::Cook;

fn repository(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).expect("tools/<crate>").join(relative)
}

#[test]
fn the_content_passes_its_check() {
    let report = check::check(&repository("content")).unwrap();
    assert!(report.passes(), "{report}");

    // The zone's boss is held by its role alone, never by name: a zone's boss is content its code never names
    // (`sapphire_gameplay`'s `bosses_are_content`).
    let roles: Vec<(&str, Role)> =
        report.meshes.iter().map(|mesh| (mesh.source.as_str(), mesh.role)).collect();
    let bosses: Vec<&str> =
        roles.iter().filter(|(_, role)| *role == Role::Boss).map(|(source, _)| *source).collect();
    assert_eq!(bosses.len(), 1, "one boss, under combat/: {bosses:?}\n{report}");
    assert!(bosses[0].starts_with("combat/"), "{bosses:?}");
    let cast: Vec<(&str, Role)> = roles.iter().copied().filter(|(_, role)| *role != Role::Boss).collect();
    assert_eq!(
        cast,
        [
            ("ambient/dockhand/dockhand.glb", Role::Ambient),
            ("ambient/lady/lady.glb", Role::Ambient),
            ("ambient/porter/porter.glb", Role::Ambient),
            ("combat/ember_lancer/ember_lancer.glb", Role::Enemy),
            ("combat/enemy/warrior.glb", Role::Enemy),
            ("combat/hero/hero.vrm", Role::Hero),
            ("combat/lancer/lancer.glb", Role::Enemy),
            ("combat/wren/wren.vrm", Role::Hero),
            ("lookdev/character/character.vrm", Role::Hero),
        ],
        "{report}"
    );

    // What's waived, and only that: the look-dev character's 13 slots and its 155 joints on the phones, and
    // Aster's 200 there, until M6 C8 cuts mobile skeletons.
    let waived: Vec<(&str, Rule, Option<Cook>)> = report
        .findings
        .iter()
        .filter(|finding| finding.waived.is_some())
        .map(|finding| (finding.source.as_str(), finding.rule, finding.cook))
        .collect();
    assert_eq!(
        waived,
        [
            ("combat/hero/hero.vrm", Rule::Joints, Some(Cook::Mobile)),
            ("lookdev/character/character.vrm", Rule::Joints, Some(Cook::Mobile)),
            ("lookdev/character/character.vrm", Rule::Slots, Some(Cook::Desktop)),
            ("lookdev/character/character.vrm", Rule::Slots, Some(Cook::Mobile)),
        ],
        "{report}"
    );

    // The look-dev character's, the fighters' and the samples' maps: a walk that found none proves nothing.
    assert!(report.character_maps >= 30, "{} character maps", report.character_maps);
}
