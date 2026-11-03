//! A move set's bytes on the road every asset takes: the packer's and a decode job's check.

use sapphire_assets::{AssetError, AssetKind};
use sapphire_moves::{MoveSetDocument, MoveSetError};

const SLICE: &str = include_str!("../../sapphire_moves/test_content/slice.moves.ron");

/// A number nothing else in the test content's cooked bytes holds, as the first move's startup.
const STARTUP: u32 = 3331;

/// One changed number in a good archive: every pointer and length still right, so a bytecheck can't
/// tell. Summing the phases panicked, or in a release build wrapped, where this should say it's damaged.
#[test]
fn a_phase_no_document_can_hold_is_refused_and_doesnt_panic() {
    let mut document = MoveSetDocument::parse(SLICE).expect("parses");
    document.moves[0].startup = STARTUP;
    let cooked = document.compile().expect("compiles").to_cooked().expect("cooks");
    assert_eq!(AssetKind::MoveSet.references(&cooked).ok(), Some(Vec::new()), "good as it is");

    let places: Vec<usize> = cooked
        .windows(4)
        .enumerate()
        .filter(|(_, bytes)| *bytes == STARTUP.to_le_bytes())
        .map(|at| at.0)
        .collect();
    let [place] = places[..] else { panic!("{STARTUP} should be there once, not at {places:?}") };

    for phase in [u32::MAX, u32::MAX - 1, u32::MAX - 26, 3601] {
        let mut damaged = cooked.clone();
        damaged[place..place + 4].copy_from_slice(&phase.to_le_bytes());
        let refused = AssetKind::MoveSet.references(&damaged);
        assert!(
            matches!(&refused, Err(AssetError::MoveSet(MoveSetError::Damaged(text))) if text.contains("phases")),
            "a startup of {phase}: {refused:?}"
        );
    }
}
