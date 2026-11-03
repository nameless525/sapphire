//! Reading what can be read from a settings file that won't parse: a small scan for its
//! `name: value` entries that needs no valid document around them.
//!
//! Each entry's value is judged by the real parser afterward, in `settings`, so nothing here has to
//! understand RON's values. It only has to find where one ends, and to keep one bad line from
//! taking the lines after it: a missing comma ends an entry at the next line that looks like one,
//! a bracket left open ends it the same way, and a line that's no entry at all is skipped.

/// One `name: value` found in a file. The value is the raw text, and may be nonsense.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry<'a> {
    pub(crate) name: &'a str,
    pub(crate) value: &'a str,
}

/// The entries of the struct in a document's `data`. If there's no `data` to be found, the whole
/// text is scanned, so a file with a damaged envelope still gives its fields up.
pub(crate) fn data_entries(text: &str) -> Vec<Entry<'_>> {
    entries(data_body(text).unwrap_or(text))
}

/// The text after `data: (`, to the end: a close bracket damaged or misplaced inside must not end
/// the scan, so nothing here looks for the matching one.
fn data_body(text: &str) -> Option<&str> {
    text.match_indices("data").find_map(|(at, _)| {
        let standalone = !text[..at].chars().next_back().is_some_and(is_name_char);
        let rest = text[at + "data".len()..].trim_start().strip_prefix(':')?.trim_start();
        standalone.then_some(rest)?.strip_prefix('(')
    })
}

/// Every entry in `body`, in order. Whatever isn't one is passed over: separators, stray close
/// brackets, comments, and a line that doesn't start with a name and a colon.
fn entries(body: &str) -> Vec<Entry<'_>> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let rest = &body[at..];
        let Some(first) = rest.chars().next() else { break };
        if first.is_whitespace() || matches!(first, ',' | ')' | ']' | '}') {
            at += first.len_utf8();
        } else if let Some(skip) = comment_len(rest) {
            at += skip;
        } else if let Some((name, after_colon)) = name_and_colon(rest) {
            let value_start = at + after_colon;
            let value_end = value_start + value_len(&body[value_start..], column(body, at));
            found.push(Entry { name, value: body[value_start..value_end].trim() });
            at = value_end;
        } else {
            at += line_len(rest);
        }
    }
    found
}

/// The length of what's left of the line, past its newline.
fn line_len(text: &str) -> usize {
    text.find('\n').map_or(text.len(), |newline| newline + 1)
}

/// The length of the comment `text` starts with. An unclosed block comment runs to the line's end,
/// not the file's, so it can't swallow what follows.
fn comment_len(text: &str) -> Option<usize> {
    if text.starts_with("//") {
        Some(line_len(text))
    } else if text.starts_with("/*") {
        Some(text.find("*/").map_or_else(|| line_len(text), |close| close + 2))
    } else {
        None
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The name `text` starts with, and where what follows its colon begins.
fn name_and_colon(text: &str) -> Option<(&str, usize)> {
    let name_len = text.find(|c| !is_name_char(c)).unwrap_or(text.len());
    let name = &text[..name_len];
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let after_name = &text[name_len..];
    let colon = after_name.find(|c: char| c != ' ' && c != '\t')?;
    let tail = after_name[colon..].strip_prefix(':')?;
    // A path such as `a::b` is no entry.
    (!tail.starts_with(':')).then_some((name, name_len + colon + 1))
}

/// How far `at` is along its line.
fn column(text: &str, at: usize) -> usize {
    text[..at].rsplit('\n').next().map_or(0, |line| line.chars().count())
}

/// How long the value that `text` starts with is. `indent` is its entry's column.
fn value_len(text: &str, indent: usize) -> usize {
    let mut depth = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => {
                // A string that's still open at the line's end is damage: settings never hold a
                // raw newline, since the writer escapes it.
                let mut escaped = false;
                while let Some(&(_, inner)) = chars.peek() {
                    if inner == '\n' {
                        break;
                    }
                    chars.next();
                    match inner {
                        '"' if !escaped => break,
                        '\\' if !escaped => escaped = true,
                        _ => escaped = false,
                    }
                }
            }
            '/' if comment_len(&text[at..]).is_some() => {
                // Up to a line comment's newline, not past it: the next line may start an entry.
                let comment = &text[at..at + comment_len(&text[at..]).unwrap_or(0)];
                let end = at + comment.strip_suffix('\n').unwrap_or(comment).len();
                while chars.peek().is_some_and(|&(next, _)| next < end) {
                    chars.next();
                }
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return at,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => return at,
            '\n' if next_line_is_an_entry(&text[at + 1..], indent) => return at,
            _ => {}
        }
    }
    text.len()
}

/// Whether the next line with anything on it starts an entry no deeper than the current one. A
/// nested struct's fields sit deeper, so they stay inside their value.
fn next_line_is_an_entry(text: &str, indent: usize) -> bool {
    let start = text.len() - text.trim_start().len();
    let line = &text[start..];
    let column = text[..start].rsplit('\n').next().map_or(0, |spaces| spaces.chars().count());
    column <= indent && name_and_colon(line).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(text: &str) -> Vec<(&str, &str)> {
        data_entries(text).into_iter().map(|entry| (entry.name, entry.value)).collect()
    }

    #[test]
    fn a_pretty_document_gives_its_top_level_fields() {
        let text = "(\n    type: \"x::Settings\",\n    version: 1,\n    data: (\n        volume: 0.5,\n        name: \"a, (b\",\n        keys: (\n            jump: Space,\n        ),\n        list: [\n            1,\n            2,\n        ],\n    ),\n)\n";
        assert_eq!(
            pairs(text),
            [
                ("volume", "0.5"),
                ("name", "\"a, (b\""),
                ("keys", "(\n            jump: Space,\n        )"),
                ("list", "[\n            1,\n            2,\n        ]"),
            ]
        );
    }

    #[test]
    fn a_missing_comma_costs_nothing() {
        let text = "(data: (\n    a: 1\n    b: 2,\n    c: 3\n))";
        assert_eq!(pairs(text), [("a", "1"), ("b", "2"), ("c", "3")]);
    }

    #[test]
    fn a_line_of_junk_is_passed_over() {
        let text = "(data: (\n    a: 1,\n    %%% what\n    b: 2,\n))";
        assert_eq!(pairs(text), [("a", "1"), ("b", "2")]);
    }

    #[test]
    fn a_bracket_left_open_ends_at_the_next_entry() {
        let text = "(data: (\n    a: (1, 2,\n    b: 2,\n    c: [\n    d: 4,\n))";
        assert_eq!(pairs(text), [("a", "(1, 2,"), ("b", "2"), ("c", "["), ("d", "4")]);
    }

    #[test]
    fn a_stray_close_bracket_doesnt_end_the_scan() {
        let text = "(data: (\n    a: 1),\n    b: 2,\n))";
        assert_eq!(pairs(text), [("a", "1"), ("b", "2")]);
    }

    #[test]
    fn a_string_left_open_ends_at_its_line() {
        let text = "(data: (\n    a: \"open,\n    b: 2,\n))";
        assert_eq!(pairs(text), [("a", "\"open,"), ("b", "2")]);
    }

    #[test]
    fn comments_hide_what_they_hold() {
        let text = "(data: (\n    // z: 1,\n    a: /* , */ 1, // (\n    /* y: 2, */ b: 2,\n))";
        assert_eq!(pairs(text), [("a", "/* , */ 1"), ("b", "2")]);
    }

    #[test]
    fn a_comment_doesnt_hide_a_missing_comma() {
        let text = "(data: (\n    a: 1 // note\n    b: 2,\n))";
        assert_eq!(pairs(text), [("a", "1 // note"), ("b", "2")]);
    }

    #[test]
    fn a_damaged_envelope_still_gives_the_fields() {
        let text = "garbage\n    volume: 0.5,\n    music: 0.25,\n";
        assert_eq!(pairs(text), [("volume", "0.5"), ("music", "0.25")]);
    }

    #[test]
    fn data_is_only_the_field_of_that_name() {
        let text = "(metadata: (a: 1), data: (b: 2))";
        assert_eq!(pairs(text), [("b", "2")]);
    }

    #[test]
    fn nothing_at_all_gives_nothing() {
        assert_eq!(pairs(""), []);
        assert_eq!(pairs("\u{fffd}\u{fffd}\n)))"), []);
    }

    #[test]
    fn a_cut_off_file_gives_what_came_before() {
        let text = "(data: (\n    a: 1,\n    b: 2";
        assert_eq!(pairs(text), [("a", "1"), ("b", "2")]);
    }
}
