//! A three-way merge by lines, as git's own text merge does it: what a document falls back to when there's no
//! model to merge it by (a kind this crate doesn't know, a side that doesn't read, or sides at different
//! versions), and what carries their comments onto a merge by path. A conflict is marked as git marks one, in
//! the diff3 style, with the base between the two sides.

use similar::{Algorithm, DiffOp, capture_diff_slices};

/// What a line merge came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lines {
    pub text: String,
    /// The line of `text` each conflict's marker starts on, counting from 1.
    pub conflicts: Vec<usize>,
}

/// Merges `ours` and `theirs`, each a change of `base`, line by line. A run of lines only one side changed takes
/// that side's; a run both changed alike takes it once; a run they changed differently is a conflict, marked.
#[must_use]
pub fn merge(base: &str, ours: &str, theirs: &str) -> Lines {
    let (text, conflicts) = merge_bytes(base.as_bytes(), ours.as_bytes(), theirs.as_bytes());
    let text =
        String::from_utf8(text).expect("UTF-8 lines, split at their newlines, and ASCII markers stay UTF-8");
    Lines { text, conflicts }
}

/// [`merge`] of files that needn't be UTF-8 text: what git's driver falls back to when a side can't be read as a
/// document. The merged bytes, and the line each conflict's marker starts on.
#[must_use]
pub fn merge_bytes(base: &[u8], ours: &[u8], theirs: &[u8]) -> (Vec<u8>, Vec<usize>) {
    let (base, ours, theirs) = (split(base), split(ours), split(theirs));
    let (to_ours, to_theirs) = (matches(&base, &ours), matches(&base, &theirs));
    let newline = newline_of(&ours);
    let mut merged = Merging { text: Vec::new(), line: 1, conflicts: Vec::new(), newline };
    let (mut b, mut o, mut t) = (0, 0, 0);
    loop {
        // Lines all three hold in step.
        let mut stable = 0;
        while b + stable < base.len()
            && to_ours[b + stable] == Some(o + stable)
            && to_theirs[b + stable] == Some(t + stable)
        {
            stable += 1;
        }
        if stable > 0 {
            merged.take(&base[b..b + stable]);
            (b, o, t) = (b + stable, o + stable, t + stable);
            continue;
        }
        if b == base.len() && o == ours.len() && t == theirs.len() {
            break;
        }
        // The next base line both sides still hold, and what each did before it.
        let next = (b..base.len()).find_map(|at| Some((at, to_ours[at]?, to_theirs[at]?)));
        let (nb, no, nt) = next.unwrap_or((base.len(), ours.len(), theirs.len()));
        merged.chunk(&base[b..nb], &ours[o..no], &theirs[t..nt]);
        (b, o, t) = (nb, no, nt);
    }
    (merged.text, merged.conflicts)
}

struct Merging {
    text: Vec<u8>,
    line: usize,
    conflicts: Vec<usize>,
    newline: &'static str,
}

impl Merging {
    fn take(&mut self, lines: &[&[u8]]) {
        for line in lines {
            self.text.extend_from_slice(line);
            self.line += 1;
        }
    }

    fn chunk(&mut self, base: &[&[u8]], ours: &[&[u8]], theirs: &[&[u8]]) {
        if ours == base || ours == theirs {
            self.take(theirs);
        } else if theirs == base {
            self.take(ours);
        } else {
            self.conflicts.push(self.line);
            self.marker("<<<<<<< ours");
            self.section(ours);
            self.marker("||||||| base");
            self.section(base);
            self.marker("=======");
            self.section(theirs);
            self.marker(">>>>>>> theirs");
        }
    }

    fn marker(&mut self, marker: &str) {
        self.text.extend_from_slice(marker.as_bytes());
        self.text.extend_from_slice(self.newline.as_bytes());
        self.line += 1;
    }

    /// A side's lines, ending on a newline so the marker after them starts its own line.
    fn section(&mut self, lines: &[&[u8]]) {
        self.take(lines);
        if !self.text.ends_with(b"\n") && !self.text.is_empty() {
            self.text.extend_from_slice(self.newline.as_bytes());
        }
    }
}

/// Each line with its newline, the last one's left as it is.
fn split(text: &[u8]) -> Vec<&[u8]> {
    text.split_inclusive(|byte| *byte == b'\n').collect()
}

/// The file's newline, as its first line ends.
fn newline_of(lines: &[&[u8]]) -> &'static str {
    if lines.first().is_some_and(|line| line.ends_with(b"\r\n")) { "\r\n" } else { "\n" }
}

/// For each line of `base`, the line of `side` the diff matched it to.
fn matches(base: &[&[u8]], side: &[&[u8]]) -> Vec<Option<usize>> {
    let mut to = vec![None; base.len()];
    for op in capture_diff_slices(Algorithm::Myers, base, side) {
        if let DiffOp::Equal { old_index, new_index, len } = op {
            for at in 0..len {
                to[old_index + at] = Some(new_index + at);
            }
        }
    }
    to
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "a\nb\nc\nd\ne\n";

    #[test]
    fn changes_apart_merge_and_one_side_alone_is_taken() {
        let merged = merge(BASE, "a\nB\nc\nd\ne\n", "a\nb\nc\nD\ne\nf\n");
        assert_eq!(merged, Lines { text: "a\nB\nc\nD\ne\nf\n".to_owned(), conflicts: vec![] });
        assert_eq!(merge(BASE, BASE, "x\n").text, "x\n");
        assert_eq!(merge(BASE, "a\nB\nc\nd\ne\n", "a\nB\nc\nd\ne\n").text, "a\nB\nc\nd\ne\n");
    }

    #[test]
    fn a_line_both_changed_is_marked_with_the_base_between() {
        let merged = merge(BASE, "a\nb\nours\nd\ne\n", "a\nb\ntheirs\nd\ne\n");
        assert_eq!(
            merged.text,
            "a\nb\n<<<<<<< ours\nours\n||||||| base\nc\n=======\ntheirs\n>>>>>>> theirs\nd\ne\n"
        );
        assert_eq!(merged.conflicts, [3]);
    }

    #[test]
    fn a_last_line_with_no_newline_still_leaves_markers_on_lines_of_their_own() {
        let merged = merge("a\nb", "a\nours", "a\ntheirs");
        assert_eq!(merged.text, "a\n<<<<<<< ours\nours\n||||||| base\nb\n=======\ntheirs\n>>>>>>> theirs\n");
        let crlf = merge("a\r\nb\r\n", "a\r\nx\r\n", "a\r\ny\r\n");
        assert!(crlf.text.contains("=======\r\ny\r\n>>>>>>> theirs\r\n"), "{:?}", crlf.text);
    }
}
