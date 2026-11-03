//! What the move system says of a document, as a device would.

use sapphire_moves::{Issue, MoveSet, MoveSetDocument, MoveSetError};

/// What the move system says of a document: its issues, and the set it became.
#[derive(Clone, Debug, Default)]
pub struct Verdict {
    /// Everything wrong with it, each naming its move and track.
    pub issues: Vec<Issue>,
    /// What it compiled to. `None` with issues, or with moves that name clips and no cook to read their
    /// tables from.
    pub set: Option<MoveSet>,
}

/// Compiles `document` as live tuning does (`sapphire_gameplay::replay::compile_document`), reading what
/// its moves' clips gave them in `running`, the set a cook made of it. With moves that name clips and no
/// cook, it can only check what the document can say by itself.
#[must_use]
pub fn judge(document: &MoveSetDocument, running: Option<&MoveSet>) -> Verdict {
    let compiled = match running {
        Some(running) => document.compile_over(running),
        None if document.names_clips() => return Verdict { issues: document.check(), set: None },
        None => document.compile(),
    };
    match compiled {
        Ok(set) => Verdict { issues: Vec::new(), set: Some(set) },
        Err(MoveSetError::Invalid(issues)) => Verdict { issues, set: None },
        Err(error) => Verdict {
            issues: vec![Issue { in_move: None, track: "moves", message: error.to_string() }],
            set: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edits::test_files::PLAYER;

    #[test]
    fn a_verdict_names_move_and_track_by_the_documents_own_words() {
        let document = MoveSetDocument::parse(&PLAYER.replacen(
            "from: 7,\n                        to: 11",
            "from: 12,\n                        to: 11",
            1,
        ))
        .expect("parses");
        let verdict = judge(&document, None);
        let issue = verdict.issues.iter().find(|issue| issue.track == "hitboxes").expect("the issue");
        assert_eq!(issue.in_move.as_deref(), Some("light_1"));
        assert_eq!(issue.message, "window 12..11 is empty");
        assert!(verdict.set.is_none());
    }
}
