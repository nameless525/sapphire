//! A choice's or a branch's `when`: a small language over flags, which the cook turns into a Luau function
//! for the script VM to run as a hook.
//!
//! ```text
//! flag.ward_lit >= 1 and not (flag.guide_mood == 2 or flag.visits > 3)
//! ```
//!
//! A condition reads flags and whole numbers, compares them (`==`, `~=`, `<`, `<=`, `>`, `>=`), and joins
//! the comparisons with `and`, `or`, `not` and brackets. Nothing else parses, so a condition can't call,
//! loop, write or draw, and it is always true or false: a flag alone, or a number alone, isn't a condition,
//! since Luau would call any number true. [`Condition::parse`] says where a condition goes wrong.

use std::fmt::Write as _;

/// The longest condition, in bytes.
pub const MAX_CONDITION: usize = 512;

/// How deep brackets and `not`s may nest: past any condition anyone means, and short of a stack.
const MAX_DEPTH: usize = 32;

/// A parsed condition: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    Compare { left: Operand, op: Compare, right: Operand },
    Not(Box<Condition>),
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
}

/// One side of a comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operand {
    /// A flag, by its registered name.
    Flag(String),
    Number(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

impl Compare {
    const fn luau(self) -> &'static str {
        match self {
            Self::Equal => "==",
            Self::NotEqual => "~=",
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Flag(String),
    Number(i32),
    Compare(Compare),
    And,
    Or,
    Not,
    Open,
    Close,
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn tokens(text: &str) -> Result<Vec<Token>, String> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        let rest = &text[at..];
        if byte.is_ascii_whitespace() {
            at += 1;
            continue;
        }
        let (token, length) = if let Some(after) = rest.strip_prefix("flag.") {
            let name_length = after.bytes().take_while(|&byte| is_name_byte(byte)).count();
            if name_length == 0 {
                return Err(format!("`flag.` at byte {at} names no flag"));
            }
            (Token::Flag(after[..name_length].to_owned()), 5 + name_length)
        } else if byte.is_ascii_digit() || (byte == b'-' && bytes.get(at + 1).is_some_and(u8::is_ascii_digit))
        {
            let length = 1 + rest[1..].bytes().take_while(u8::is_ascii_digit).count();
            let number = rest[..length]
                .parse::<i32>()
                .map_err(|_| format!("{} is past what a flag holds", &rest[..length]))?;
            (Token::Number(number), length)
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let length = rest.bytes().take_while(|&byte| is_name_byte(byte)).count();
            let token = match &rest[..length] {
                "and" => Token::And,
                "or" => Token::Or,
                "not" => Token::Not,
                word => {
                    return Err(format!(
                        "`{word}` isn't something a condition reads: it reads `flag.<name>`, whole numbers, \
                         comparisons, `and`, `or`, `not` and brackets"
                    ));
                }
            };
            (token, length)
        } else {
            let two = rest.get(..2).unwrap_or_default();
            match (two, byte) {
                ("==", _) => (Token::Compare(Compare::Equal), 2),
                ("~=", _) => (Token::Compare(Compare::NotEqual), 2),
                ("!=", _) => return Err("Luau's \"not equal\" is `~=`".to_owned()),
                ("<=", _) => (Token::Compare(Compare::LessOrEqual), 2),
                (">=", _) => (Token::Compare(Compare::GreaterOrEqual), 2),
                (_, b'<') => (Token::Compare(Compare::Less), 1),
                (_, b'>') => (Token::Compare(Compare::Greater), 1),
                (_, b'(') => (Token::Open, 1),
                (_, b')') => (Token::Close, 1),
                _ => {
                    let shown = rest.chars().next().unwrap_or_default();
                    return Err(format!("{shown:?} at byte {at} isn't something a condition reads"));
                }
            }
        };
        tokens.push(token);
        at += length;
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, token: &Token) -> bool {
        let found = self.peek() == Some(token);
        self.at += usize::from(found);
        found
    }

    fn or(&mut self, depth: usize) -> Result<Condition, String> {
        let mut left = self.and(depth)?;
        while self.eat(&Token::Or) {
            left = Condition::Or(Box::new(left), Box::new(self.and(depth)?));
        }
        Ok(left)
    }

    fn and(&mut self, depth: usize) -> Result<Condition, String> {
        let mut left = self.not(depth)?;
        while self.eat(&Token::And) {
            left = Condition::And(Box::new(left), Box::new(self.not(depth)?));
        }
        Ok(left)
    }

    fn not(&mut self, depth: usize) -> Result<Condition, String> {
        if depth > MAX_DEPTH {
            return Err(format!("it nests deeper than {MAX_DEPTH}"));
        }
        if self.eat(&Token::Not) {
            return Ok(Condition::Not(Box::new(self.not(depth + 1)?)));
        }
        if self.eat(&Token::Open) {
            let inner = self.or(depth + 1)?;
            if !self.eat(&Token::Close) {
                return Err("a bracket isn't closed".to_owned());
            }
            return Ok(inner);
        }
        let left = self.operand()?;
        let Some(Token::Compare(op)) = self.peek().cloned() else {
            return Err("a flag or a number on its own isn't true or false: compare it".to_owned());
        };
        self.at += 1;
        let right = self.operand()?;
        Ok(Condition::Compare { left, op, right })
    }

    fn operand(&mut self) -> Result<Operand, String> {
        let operand = match self.peek() {
            Some(Token::Flag(name)) => Operand::Flag(name.clone()),
            Some(Token::Number(number)) => Operand::Number(*number),
            Some(other) => return Err(format!("expected a flag or a number, and found {other:?}")),
            None => return Err("it ends where a flag or a number should be".to_owned()),
        };
        self.at += 1;
        Ok(operand)
    }
}

impl Condition {
    /// Reads a condition, saying what's wrong with it in words a document's author can act on.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_CONDITION {
            return Err(format!("{} bytes, past the limit of {MAX_CONDITION}", text.len()));
        }
        let mut parser = Parser { tokens: tokens(text)?, at: 0 };
        if parser.tokens.is_empty() {
            return Err("it's empty".to_owned());
        }
        let condition = parser.or(0)?;
        if let Some(extra) = parser.peek() {
            return Err(format!("{extra:?} follows a whole condition"));
        }
        Ok(condition)
    }

    /// Whether it holds, reading each flag through `flag`: what its Luau does, for a tool or a test that
    /// has no VM.
    pub fn holds(&self, flag: &impl Fn(&str) -> i32) -> bool {
        let value = |operand: &Operand| match operand {
            Operand::Flag(name) => flag(name),
            Operand::Number(number) => *number,
        };
        match self {
            Self::Compare { left, op, right } => {
                let (left, right) = (value(left), value(right));
                match op {
                    Compare::Equal => left == right,
                    Compare::NotEqual => left != right,
                    Compare::Less => left < right,
                    Compare::LessOrEqual => left <= right,
                    Compare::Greater => left > right,
                    Compare::GreaterOrEqual => left >= right,
                }
            }
            Self::Not(inner) => !inner.holds(flag),
            Self::And(left, right) => left.holds(flag) && right.holds(flag),
            Self::Or(left, right) => left.holds(flag) || right.holds(flag),
        }
    }

    /// Every flag it reads, in the order it reads them, repeats and all.
    pub fn flags(&self) -> Vec<&str> {
        let mut flags = Vec::new();
        self.each_flag(&mut |name| flags.push(name));
        flags
    }

    fn each_flag<'a>(&'a self, out: &mut impl FnMut(&'a str)) {
        match self {
            Self::Compare { left, right, .. } => {
                for operand in [left, right] {
                    if let Operand::Flag(name) = operand {
                        out(name);
                    }
                }
            }
            Self::Not(inner) => inner.each_flag(out),
            Self::And(left, right) | Self::Or(left, right) => {
                left.each_flag(out);
                right.each_flag(out);
            }
        }
    }

    /// The condition as a Luau expression, each flag read as `sapphire.flag(place)`, `place` being where
    /// `place_of` says the chunk's `names` list holds it, counting from 1.
    pub fn to_luau(&self, place_of: &impl Fn(&str) -> usize, out: &mut String) {
        match self {
            Self::Compare { left, op, right } => {
                out.push('(');
                write_operand(left, place_of, out);
                let _ = write!(out, " {} ", op.luau());
                write_operand(right, place_of, out);
                out.push(')');
            }
            Self::Not(inner) => {
                out.push_str("(not ");
                inner.to_luau(place_of, out);
                out.push(')');
            }
            Self::And(left, right) | Self::Or(left, right) => {
                let joint = if matches!(self, Self::And(..)) { " and " } else { " or " };
                out.push('(');
                left.to_luau(place_of, out);
                out.push_str(joint);
                right.to_luau(place_of, out);
                out.push(')');
            }
        }
    }
}

fn write_operand(operand: &Operand, place_of: &impl Fn(&str) -> usize, out: &mut String) {
    match operand {
        Operand::Flag(name) => {
            let _ = write!(out, "sapphire.flag({})", place_of(name));
        }
        Operand::Number(number) => {
            let _ = write!(out, "{number}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luau(text: &str) -> String {
        let condition = Condition::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"));
        let mut out = String::new();
        condition.to_luau(&|name| if name == "a" { 1 } else { 2 }, &mut out);
        out
    }

    #[test]
    fn conditions_read_as_luau_with_and_binding_tighter_than_or() {
        assert_eq!(luau("flag.a >= 1"), "(sapphire.flag(1) >= 1)");
        assert_eq!(luau("-3 < flag.b"), "(-3 < sapphire.flag(2))");
        assert_eq!(
            luau("flag.a == 1 or flag.b ~= 0 and not flag.a > 2"),
            "((sapphire.flag(1) == 1) or ((sapphire.flag(2) ~= 0) and (not (sapphire.flag(1) > 2))))"
        );
        assert_eq!(
            luau("(flag.a == 1 or flag.b == 2) and flag.a <= 5"),
            "(((sapphire.flag(1) == 1) or (sapphire.flag(2) == 2)) and (sapphire.flag(1) <= 5))"
        );
    }

    #[test]
    fn it_lists_the_flags_it_reads() {
        let condition = Condition::parse("flag.x > 1 and (flag.y == 0 or flag.x < 9)").unwrap();
        assert_eq!(condition.flags(), ["x", "y", "x"]);
    }

    /// What a condition can't hold, and that each says why rather than reaching the compiler.
    #[test]
    fn anything_but_flags_numbers_and_comparisons_is_refused() {
        let refused = [
            ("", "empty"),
            ("flag.a", "compare it"),
            ("1", "compare it"),
            ("flag.a + 1 > 2", "isn't something"),
            ("flag.a != 1", "~="),
            ("print(1) == 1", "`print`"),
            ("flag.a == \"x\"", "isn't something"),
            ("flag. == 1", "names no flag"),
            ("(flag.a == 1", "isn't closed"),
            ("flag.a == 1)", "follows"),
            ("flag.a == 1 and", "ends"),
            ("flag.a == 99999999999", "past what a flag holds"),
            ("flag.a == 1 flag.b == 2", "follows"),
            ("flag.a == flag.b == 1", "follows"),
        ];
        for (text, said) in refused {
            let error = Condition::parse(text).expect_err(text);
            assert!(error.contains(said), "{text:?}: {error}");
        }
        let deep = format!("{}flag.a == 1{}", "(".repeat(40), ")".repeat(40));
        assert!(Condition::parse(&deep).unwrap_err().contains("nests"));
        let nots = format!("{}flag.a == 1", "not ".repeat(40));
        assert!(Condition::parse(&nots).unwrap_err().contains("nests"));
        assert!(Condition::parse(&"flag.a == 1 and ".repeat(40)).unwrap_err().contains("bytes"));
    }
}
