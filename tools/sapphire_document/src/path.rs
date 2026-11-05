//! Where a property is in a document: the second half of a change record's name for it.
//!
//! A path speaks of the value, not of the text: a field goes by its Rust name, whatever the file calls it,
//! and an entry of a list with names goes by its name, so it stays itself when the list is reordered.
//! [`crate::walk`] follows one through a value, and through the text beside it.

use std::fmt;
use std::str::FromStr;

/// One step down.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    /// A struct's field, or the field of an enum's variant.
    Field(String),
    /// A place in a list, an array or a tuple, or the field of a tuple variant.
    Index(usize),
    /// The entry of a list whose `name` is this.
    Key(String),
    /// What an `Option` holds.
    Some,
}

/// A way down from a document's root. Prints as `moves["light_1"].hitboxes[0].attack?.damage`, and parses
/// back from that.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PropertyPath {
    steps: Vec<Step>,
}

impl PropertyPath {
    /// The document itself.
    #[must_use]
    pub const fn root() -> Self {
        Self { steps: Vec::new() }
    }

    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// What it holds in memory, for a history's count.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.steps
            .iter()
            .map(|step| {
                size_of::<Step>()
                    + match step {
                        Step::Field(name) | Step::Key(name) => name.len(),
                        Step::Index(_) | Step::Some => 0,
                    }
            })
            .sum()
    }

    /// This path, one step further.
    #[must_use]
    pub fn join(&self, step: Step) -> Self {
        let mut steps = Vec::with_capacity(self.steps.len() + 1);
        steps.extend_from_slice(&self.steps);
        steps.push(step);
        Self { steps }
    }

    /// The first `count` steps.
    #[must_use]
    pub fn prefix(&self, count: usize) -> Self {
        Self { steps: self.steps[..count.min(self.steps.len())].to_vec() }
    }

    /// The path of what holds this, and the last step. `None` for the root.
    #[must_use]
    pub fn split_last(&self) -> Option<(Self, &Step)> {
        let (last, rest) = self.steps.split_last()?;
        Some((Self { steps: rest.to_vec() }, last))
    }

    /// What has to be unchanged for this path to still mean what it meant: the entry at its first place
    /// in a list, which a place alone can't tell from another that moved there, or the whole path if it
    /// goes by names alone. A name says which entry it is, wherever that entry went.
    #[must_use]
    pub fn owner(&self) -> Self {
        match self.steps.iter().position(|step| matches!(step, Step::Index(_))) {
            Some(place) => self.prefix(place + 1),
            None => self.clone(),
        }
    }

    /// The object this property belongs to, in the sense of the spec's `(object, property, old, new)`: as
    /// far as the path goes by names, the last entry it can call by one. Below that, places.
    #[must_use]
    pub fn object(&self) -> Self {
        let last = self.steps.iter().rposition(|step| matches!(step, Step::Key(_)));
        self.prefix(last.map_or(0, |last| last + 1))
    }
}

impl fmt::Display for PropertyPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (at, step) in self.steps.iter().enumerate() {
            match step {
                Step::Field(name) if at == 0 => f.write_str(name)?,
                Step::Field(name) => write!(f, ".{name}")?,
                Step::Index(index) => write!(f, "[{index}]")?,
                Step::Key(name) => {
                    f.write_str("[\"")?;
                    for c in name.chars() {
                        if matches!(c, '\\' | '"') {
                            f.write_str("\\")?;
                        }
                        write!(f, "{c}")?;
                    }
                    f.write_str("\"]")?;
                }
                Step::Some => f.write_str("?")?,
            }
        }
        Ok(())
    }
}

/// Text that isn't a path, and where it stopped being one.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a property path, at byte {at}")]
pub struct PathParseError {
    pub at: usize,
}

impl FromStr for PropertyPath {
    type Err = PathParseError;

    fn from_str(text: &str) -> Result<Self, PathParseError> {
        let bytes = text.as_bytes();
        let mut steps = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'?' => {
                    steps.push(Step::Some);
                    at += 1;
                }
                b'[' if bytes.get(at + 1) == Some(&b'"') => {
                    let (key, end) = key_at(text, at + 2).ok_or(PathParseError { at })?;
                    steps.push(Step::Key(key));
                    at = end;
                }
                b'[' => {
                    let close = text[at..].find(']').ok_or(PathParseError { at })? + at;
                    let index = text[at + 1..close].parse().map_err(|_| PathParseError { at })?;
                    steps.push(Step::Index(index));
                    at = close + 1;
                }
                byte => {
                    let start = if byte == b'.' { at + 1 } else { at };
                    if byte != b'.' && at != 0 {
                        return Err(PathParseError { at });
                    }
                    let length = text[start..]
                        .bytes()
                        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                        .count();
                    if length == 0 {
                        return Err(PathParseError { at });
                    }
                    steps.push(Step::Field(text[start..start + length].to_owned()));
                    at = start + length;
                }
            }
        }
        Ok(Self { steps })
    }
}

/// The key that starts at `start`, just past its opening quote, and where its `"]` ends.
fn key_at(text: &str, start: usize) -> Option<(String, usize)> {
    let mut key = String::new();
    let mut chars = text[start..].char_indices();
    while let Some((offset, c)) = chars.next() {
        match c {
            '\\' => key.push(chars.next()?.1),
            '"' => {
                let end = start + offset + 1;
                return (text.as_bytes().get(end) == Some(&b']')).then_some((key, end + 1));
            }
            c => key.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_prints_and_parses_back() {
        for text in [
            "",
            "moves",
            r#"moves["light_1"].hitboxes[0].attack?.damage"#,
            r#"effects["a \"quoted\" \\ name"].emitters[2].color[0][1].x"#,
            "fighter?.movement?.run_speed",
        ] {
            let path: PropertyPath = text.parse().expect(text);
            assert_eq!(path.to_string(), text);
        }
    }

    #[test]
    fn what_isnt_a_path_says_where() {
        for (text, at) in [("[", 0), ("a[x]", 1), ("a.", 1), ("a..b", 1), ("a?b", 2), (r#"a["k"#, 1)] {
            assert_eq!(text.parse::<PropertyPath>().expect_err(text).at, at, "{text}");
        }
    }

    #[test]
    fn an_owner_ends_at_the_first_place_and_an_object_at_the_last_name() {
        let path: PropertyPath = r#"moves["a"].hitboxes[1].attack?.damage"#.parse().expect("a path");
        assert_eq!(path.owner().to_string(), r#"moves["a"].hitboxes[1]"#);
        assert_eq!(path.object().to_string(), r#"moves["a"]"#);
        let named: PropertyPath = r#"moves["a"].startup"#.parse().expect("a path");
        assert_eq!(named.owner(), named);
    }
}
