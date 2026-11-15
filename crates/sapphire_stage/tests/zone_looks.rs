//! Every shipped zone's light rigs read as the stage reads them (Z3 review F4). A cook checks a zone's `rigs`
//! only by their document type, since the library's reader is `sapphire_world`'s, which the cooker can't link;
//! this reads each with `LightRigLibrary::from_ron`, and each zone's day as the stage takes it, complete.

use std::path::{Path, PathBuf};

use sapphire_world::light_rig::LightRigLibrary;
use sapphire_world::tod::TodTimeline;
use sapphire_zone::ZoneDocument;

fn zones_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/zones")
}

/// What's wrong with the look of the zone whose document is at `path`, if anything.
fn zone_look_problem(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).expect("a zone document");
    let document = ZoneDocument::from_ron(&text).expect("a zone document that reads");
    let beside = |name: &str| {
        let file = path.parent().expect("in a folder").join(name);
        std::fs::read_to_string(&file).unwrap_or_else(|error| panic!("{}: {error}", file.display()))
    };
    if let Some(rigs) = &document.rigs
        && let Err(error) = LightRigLibrary::from_ron(&beside(rigs))
    {
        return Some(format!("its rigs, {rigs}: {error}"));
    }
    if let Some(day) = &document.day {
        match TodTimeline::from_ron(&beside(day)) {
            Err(error) => return Some(format!("its day, {day}: {error}")),
            Ok(timeline) if !timeline.is_complete() => {
                return Some(format!("its day, {day}, isn't complete"));
            }
            Ok(_) => {}
        }
    }
    None
}

#[test]
fn every_shipped_zones_light_rigs_and_day_read_as_the_stage_reads_them() {
    let mut zones = 0;
    let mut named = 0;
    for folder in std::fs::read_dir(zones_dir()).expect("content/zones") {
        let folder = folder.expect("an entry").path();
        let Ok(files) = std::fs::read_dir(&folder) else { continue };
        for file in files {
            let path = file.expect("an entry").path();
            if !path.to_string_lossy().ends_with(".zone.ron") {
                continue;
            }
            zones += 1;
            let text = std::fs::read_to_string(&path).expect("a zone document");
            named += usize::from(ZoneDocument::from_ron(&text).expect("it reads").rigs.is_some());
            if let Some(problem) = zone_look_problem(&path) {
                panic!("{}: {problem}", path.display());
            }
        }
    }
    assert!(zones >= 3 && named >= 3, "{zones} zones, {named} naming rigs: the walk found the shipped ones");
}

/// The check refuses what the cook lets through: rigs of the right type naming one preset twice.
#[test]
fn rigs_the_cook_lets_through_can_still_fail_to_read() {
    let twice = r#"(
        type: "sapphire::world::LightRigLibrary",
        version: 1,
        data: (rigs: [(name: "gameplay", rig: ()), (name: "gameplay", rig: ())]),
    )"#;
    let error = LightRigLibrary::from_ron(twice).expect_err("twice named").to_string();
    assert!(error.contains("twice"), "{error}");
}
