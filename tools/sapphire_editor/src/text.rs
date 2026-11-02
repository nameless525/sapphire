//! RON as text: where each value of a document sits, down to the byte.
//!
//! A person writes these files too, with comments and a layout that say things, so the editor never
//! writes one out from its values. The text is what it edits, and this is how it finds its way: [`scan`]
//! reads a document into a tree of byte ranges and nothing else. What the values mean stays with
//! `sapphire_moves`' parser, and [`crate::edits`] holds the two together, so a place this got wrong can't
//! become a wrong edit.

use std::ops::Range;

/// How deep [`scan`] goes: well past any document, and short of a stack overflow on a file that isn't one.
const MAX_DEPTH: u32 = 128;

/// A value, and where it sits. A named value's span starts at its name: `Capsule(radius: 0.2)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub span: Range<usize>,
    pub kind: Kind,
}

/// What a value holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A number, a string, a character or a bare name.
    Atom,
    /// `(a: 1, b: 2)`, with or without a name.
    Struct(Vec<Field>),
    /// `(1, 2)` or `[1, 2]`, with or without a name.
    Seq(Vec<Node>),
    /// `{ a: 1 }`.
    Map(Vec<(Node, Node)>),
}

/// A struct's field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    /// Where its name starts: the start of the text that says it.
    pub start: usize,
    pub value: Node,
}

impl Node {
    /// A struct's field, by name.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&Self> {
        self.fields().iter().find(|field| field.name == name).map(|field| &field.value)
    }

    /// A struct's fields, in the order they're written. None for anything else.
    #[must_use]
    pub fn fields(&self) -> &[Field] {
        match &self.kind {
            Kind::Struct(fields) => fields,
            _ => &[],
        }
    }

    /// A list's or a tuple's items. None for anything else.
    #[must_use]
    pub fn items(&self) -> &[Self] {
        match &self.kind {
            Kind::Seq(items) => items,
            _ => &[],
        }
    }

    /// The text of this value.
    #[must_use]
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.span.clone()]
    }

    /// This atom as a string's contents, if it's a plain string: what a name in a document reads as.
    #[must_use]
    pub fn string<'a>(&self, source: &'a str) -> Option<&'a str> {
        self.text(source).strip_prefix('"')?.strip_suffix('"')
    }
}

/// Where the text stopped making sense to [`scan`]. A file the document's own parser took and this
/// didn't is a bug here, and it leaves the file read-only rather than guessed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanError {
    pub at: usize,
    pub what: &'static str,
}

/// Reads a document into a tree of where its values are.
pub fn scan(source: &str) -> Result<Node, ScanError> {
    let mut scanner = Scanner { source, at: 0 };
    scanner.skip_trivia();
    while scanner.source[scanner.at..].starts_with("#!") {
        scanner.inner_attribute()?;
        scanner.skip_trivia();
    }
    let root = scanner.value(0)?;
    scanner.skip_trivia();
    if scanner.at != source.len() {
        return Err(scanner.error("text after the document's end"));
    }
    Ok(root)
}

/// What follows `at`, past spaces and comments on any line.
#[must_use]
pub fn skip_trivia(source: &str, at: usize) -> usize {
    let mut scanner = Scanner { source, at };
    scanner.skip_trivia();
    scanner.at
}

struct Scanner<'a> {
    source: &'a str,
    at: usize,
}

impl Scanner<'_> {
    fn error(&self, what: &'static str) -> ScanError {
        ScanError { at: self.at, what }
    }

    fn rest(&self) -> &[u8] {
        &self.source.as_bytes()[self.at..]
    }

    fn peek(&self) -> Option<u8> {
        self.rest().first().copied()
    }

    /// Past whitespace and comments. RON's block comments nest.
    fn skip_trivia(&mut self) {
        loop {
            match self.rest() {
                [byte, ..] if byte.is_ascii_whitespace() => self.at += 1,
                [b'/', b'/', ..] => {
                    while self.peek().is_some_and(|byte| byte != b'\n') {
                        self.at += 1;
                    }
                }
                [b'/', b'*', ..] => {
                    self.at += 2;
                    let mut depth = 1;
                    while depth > 0 {
                        match self.rest() {
                            [] => return,
                            [b'/', b'*', ..] => {
                                depth += 1;
                                self.at += 2;
                            }
                            [b'*', b'/', ..] => {
                                depth -= 1;
                                self.at += 2;
                            }
                            _ => self.at += 1,
                        }
                    }
                }
                _ => return,
            }
        }
    }

    /// `#![enable(implicit_some)]`, which only RON's own parser needs.
    fn inner_attribute(&mut self) -> Result<(), ScanError> {
        self.at += 2;
        self.skip_trivia();
        if self.peek() != Some(b'[') {
            return Err(self.error("an attribute without its brackets"));
        }
        let mut depth = 0u32;
        loop {
            match self.peek() {
                Some(b'[') => depth += 1,
                Some(b']') => {
                    depth -= 1;
                    if depth == 0 {
                        self.at += 1;
                        return Ok(());
                    }
                }
                Some(b'"') => {
                    self.string()?;
                    continue;
                }
                Some(_) => {}
                None => return Err(self.error("an attribute with no end")),
            }
            self.at += 1;
        }
    }

    fn value(&mut self, depth: u32) -> Result<Node, ScanError> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deep"));
        }
        self.skip_trivia();
        let start = self.at;
        let Some(first) = self.peek() else { return Err(self.error("a value is missing")) };
        match first {
            b'(' => self.parenthesized(start, depth),
            b'[' => {
                self.at += 1;
                let items = self.items(b']', depth)?;
                Ok(Node { span: start..self.at, kind: Kind::Seq(items) })
            }
            b'{' => {
                self.at += 1;
                self.map(start, depth)
            }
            b'"' => {
                self.string()?;
                Ok(Node { span: start..self.at, kind: Kind::Atom })
            }
            b'\'' => {
                self.character()?;
                Ok(Node { span: start..self.at, kind: Kind::Atom })
            }
            b'-' | b'+' | b'.' | b'0'..=b'9' => {
                self.number();
                Ok(Node { span: start..self.at, kind: Kind::Atom })
            }
            byte if is_ident_byte(byte) => self.named(start, depth),
            _ => Err(self.error("something that isn't a value")),
        }
    }

    /// A bare name, a raw or byte string, or a name with a body.
    fn named(&mut self, start: usize, depth: u32) -> Result<Node, ScanError> {
        if let Some(found) = self.raw_or_byte_string()? {
            return Ok(found);
        }
        self.ident();
        let before_body = self.at;
        self.skip_trivia();
        if self.peek() == Some(b'(') {
            return self.parenthesized(start, depth);
        }
        self.at = before_body;
        Ok(Node { span: start..self.at, kind: Kind::Atom })
    }

    fn raw_or_byte_string(&mut self) -> Result<Option<Node>, ScanError> {
        let start = self.at;
        let rest = self.rest();
        let after_b = usize::from(rest.first() == Some(&b'b'));
        if rest.get(after_b) == Some(&b'"') {
            self.at += after_b;
            self.string()?;
            return Ok(Some(Node { span: start..self.at, kind: Kind::Atom }));
        }
        if rest.get(after_b) == Some(&b'r') {
            let hashes = rest[after_b + 1..].iter().take_while(|&&byte| byte == b'#').count();
            if rest.get(after_b + 1 + hashes) == Some(&b'"') {
                self.at += after_b + 1 + hashes + 1;
                let closing: Vec<u8> =
                    std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
                while !self.rest().starts_with(&closing) {
                    if self.peek().is_none() {
                        return Err(self.error("a raw string with no end"));
                    }
                    self.at += 1;
                }
                self.at += closing.len();
                return Ok(Some(Node { span: start..self.at, kind: Kind::Atom }));
            }
        }
        Ok(None)
    }

    /// `(`, a struct if it starts `name:` and a tuple if it doesn't.
    fn parenthesized(&mut self, start: usize, depth: u32) -> Result<Node, ScanError> {
        self.at += 1;
        self.skip_trivia();
        let mut look = Scanner { source: self.source, at: self.at };
        let is_struct = look.peek().is_some_and(is_ident_byte) && {
            look.ident();
            look.skip_trivia();
            look.peek() == Some(b':')
        };
        if !is_struct {
            let items = self.items(b')', depth)?;
            return Ok(Node { span: start..self.at, kind: Kind::Seq(items) });
        }
        let mut fields = Vec::new();
        loop {
            self.skip_trivia();
            if self.peek() == Some(b')') {
                self.at += 1;
                return Ok(Node { span: start..self.at, kind: Kind::Struct(fields) });
            }
            let field_start = self.at;
            if !self.peek().is_some_and(is_ident_byte) {
                return Err(self.error("a struct's field without a name"));
            }
            self.ident();
            let name = self.source[field_start..self.at].to_owned();
            self.skip_trivia();
            if self.peek() != Some(b':') {
                return Err(self.error("a field's name without its colon"));
            }
            self.at += 1;
            let value = self.value(depth + 1)?;
            fields.push(Field { name, start: field_start, value });
            self.after_item(b')')?;
        }
    }

    /// A list's or a tuple's items, up to and past `close`.
    fn items(&mut self, close: u8, depth: u32) -> Result<Vec<Node>, ScanError> {
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            if self.peek() == Some(close) {
                self.at += 1;
                return Ok(items);
            }
            items.push(self.value(depth + 1)?);
            self.after_item(close)?;
        }
    }

    fn map(&mut self, start: usize, depth: u32) -> Result<Node, ScanError> {
        let mut entries = Vec::new();
        loop {
            self.skip_trivia();
            if self.peek() == Some(b'}') {
                self.at += 1;
                return Ok(Node { span: start..self.at, kind: Kind::Map(entries) });
            }
            let key = self.value(depth + 1)?;
            self.skip_trivia();
            if self.peek() != Some(b':') {
                return Err(self.error("a map's key without its colon"));
            }
            self.at += 1;
            let value = self.value(depth + 1)?;
            entries.push((key, value));
            self.after_item(b'}')?;
        }
    }

    /// Past the comma after an item, or up to the closer if it has none.
    fn after_item(&mut self, close: u8) -> Result<(), ScanError> {
        self.skip_trivia();
        match self.peek() {
            Some(b',') => {
                self.at += 1;
                Ok(())
            }
            Some(byte) if byte == close => Ok(()),
            _ => Err(self.error("something between two values that isn't a comma")),
        }
    }

    fn ident(&mut self) {
        while self.peek().is_some_and(is_ident_byte) {
            self.at += 1;
        }
    }

    /// A sign may start a number or follow its exponent.
    fn number(&mut self) {
        let start = self.at;
        while let Some(byte) = self.peek() {
            let sign = matches!(byte, b'-' | b'+')
                && (self.at == start || matches!(self.source.as_bytes()[self.at - 1], b'e' | b'E'));
            if !(sign || byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_')) {
                return;
            }
            self.at += 1;
        }
    }

    fn string(&mut self) -> Result<(), ScanError> {
        self.at += 1;
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(());
                }
                Some(b'\\') => self.at += 2,
                Some(_) => self.at += 1,
                None => return Err(self.error("a string with no end")),
            }
            if self.at > self.source.len() {
                self.at = self.source.len();
                return Err(self.error("a string with no end"));
            }
        }
    }

    fn character(&mut self) -> Result<(), ScanError> {
        self.at += 1;
        loop {
            match self.peek() {
                Some(b'\'') => {
                    self.at += 1;
                    return Ok(());
                }
                Some(b'\\') => self.at += 2,
                Some(_) => self.at += 1,
                None => return Err(self.error("a character with no end")),
            }
            if self.at > self.source.len() {
                self.at = self.source.len();
                return Err(self.error("a character with no end"));
            }
        }
    }
}

const fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_says_where_each_value_is_past_comments_and_strings() {
        let text = r#"// before ) ( "
(
    /* a /* nested */ comment */
    name: "a ) string, with \" in it", // after
    at: 7,
    shape: Capsule(radius: 0.25, length: 1e-2),
    list: [(1.0, -2.5), Bone("Hips"), Light,],
    empty: [],
    map: { "a": 1 },
)
"#;
        let root = scan(text).expect("a document");
        assert_eq!(root.field("at").expect("at").text(text), "7");
        assert_eq!(root.field("name").expect("name").string(text), Some(r#"a ) string, with \" in it"#));
        let shape = root.field("shape").expect("shape");
        assert_eq!(shape.text(text), "Capsule(radius: 0.25, length: 1e-2)");
        assert_eq!(shape.field("length").expect("length").text(text), "1e-2");
        let list = root.field("list").expect("list").items();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].items()[1].text(text), "-2.5");
        assert_eq!(list[1].text(text), r#"Bone("Hips")"#);
        assert_eq!(list[2].text(text), "Light");
        assert!(root.field("empty").expect("empty").items().is_empty());
        assert!(matches!(root.field("map").expect("map").kind, Kind::Map(_)));
    }

    #[test]
    fn what_isnt_a_document_says_where_it_stopped() {
        for (text, at) in [("(a: 1", 5), ("(a 1)", 3), ("(1 2)", 3), ("(a: 1) b", 7), ("[\"no end]", 9)] {
            assert_eq!(scan(text).expect_err(text).at, at, "{text}");
        }
    }

    #[test]
    fn a_file_nested_past_reason_is_refused_rather_than_overflowing() {
        let text = "[".repeat(10_000);
        assert_eq!(scan(&text).expect_err("too deep").what, "nested too deep");
    }

    #[test]
    fn signs_only_start_a_number_or_follow_its_exponent() {
        let text = "(-1, +2.5, 1e-3, 1E+3, 0x1f, 1_000)";
        let root = scan(text).expect("a tuple");
        let items: Vec<&str> = root.items().iter().map(|item| item.text(text)).collect();
        assert_eq!(items, ["-1", "+2.5", "1e-3", "1E+3", "0x1f", "1_000"]);
    }
}
