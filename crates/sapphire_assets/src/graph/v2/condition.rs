//! A transition's condition: a closed grammar over the graph's parameters, parsed and checked at cook
//! (and on live link), compiled to a few postfix [`ConditionOp`]s.
//!
//! ```text
//! condition  = or
//! or         = and { "||" and }
//! and        = unary { "&&" unary }
//! unary      = "!" unary | primary
//! primary    = "(" or ")" | "true" | "false" | "finished" | "finished_at" "(" number ")"
//!            | comparison | parameter
//! comparison = operand ( "<" | "<=" | ">" | ">=" | "==" | "!=" ) operand
//! operand    = parameter | number | mode | "true" | "false"
//! ```
//!
//! A number parameter compares by `<`, `<=`, `>` and `>=` with a number or another number parameter;
//! `==` on numbers is refused, since two numbers that ought to agree seldom do. A truth parameter reads
//! alone or compares by `==` and `!=`. A traversal mode compares by `==` and `!=` with a mode's name
//! (`None`, `Climbing`, `Gliding`, `Swimming`). `finished` holds once the state's clip has played to its
//! end, and `finished_at(p)` too, the next state starting its gait at phase `p`; that one says where
//! the next state starts, so it stands only at the top, joined by `&&`, and once.

use super::program::{Comparison, ConditionOp};
use super::{ParameterKind, TraversalMode};

/// The most ops one condition compiles to.
pub const MAX_CONDITION_OPS: usize = 32;
/// The deepest a condition's evaluation stack goes: what a runtime's fixed stack holds.
pub const MAX_CONDITION_DEPTH: usize = 8;
/// The most tokens a condition may have, held before it's parsed so that no text can recurse the parser
/// deep: a condition of [`MAX_CONDITION_OPS`] steps needs fewer than this.
pub const MAX_CONDITION_TOKENS: usize = 192;
/// The longest a condition may be, in bytes, held before it's read.
pub const MAX_CONDITION_BYTES: usize = 2048;

/// A parsed condition, before its names are checked.
#[derive(Clone, Debug, PartialEq)]
enum Expr {
    Constant(bool),
    Finished,
    FinishedAt(f32),
    Parameter(String),
    Compare(Operand, Comparison, Operand),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
enum Operand {
    Parameter(String),
    Number(f32),
    Truth(bool),
    Mode(TraversalMode),
}

/// A condition, compiled.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub ops: Vec<ConditionOp>,
    /// How deep its stack goes.
    pub depth: usize,
    /// Which parameters it reads, a bit each.
    pub reads: u32,
    /// Whether it asks whether the state has finished.
    pub finished: bool,
    /// `finished_at`'s phase.
    pub phase: Option<f32>,
}

/// Parses and compiles `text` against the graph's parameters, `(name, kind)` by index. The error says
/// what's wrong, quoting where.
pub fn compile(text: &str, parameters: &[(&str, ParameterKind)]) -> Result<Compiled, String> {
    let tokens = lex(text)?;
    let mut parser = Parser { tokens: &tokens, at: 0 };
    let expr = parser.or()?;
    if let Some(token) = parser.tokens.get(parser.at) {
        return Err(format!("`{}` follows a whole condition", token.text));
    }
    let phase = top_phase(&expr)?;
    let mut out = Compiled { ops: Vec::new(), depth: 0, reads: 0, finished: false, phase };
    emit(&expr, parameters, &mut out)?;
    if out.ops.len() > MAX_CONDITION_OPS {
        return Err(format!("{} steps, past the limit of {MAX_CONDITION_OPS}", out.ops.len()));
    }
    out.depth = depth(&out.ops);
    if out.depth > MAX_CONDITION_DEPTH {
        return Err(format!("nests {} deep, past the limit of {MAX_CONDITION_DEPTH}", out.depth));
    }
    Ok(out)
}

/// How deep `ops`' stack goes, evaluated in order.
pub(super) fn depth(ops: &[ConditionOp]) -> usize {
    let (mut now, mut most) = (0usize, 0usize);
    for op in ops {
        match op {
            ConditionOp::And | ConditionOp::Or => now = now.saturating_sub(1),
            ConditionOp::Not => {}
            _ => now += 1,
        }
        most = most.max(now);
    }
    most
}

/// `finished_at`'s phase, if it stands at the top joined by `&&`; an error if it's anywhere else.
fn top_phase(expr: &Expr) -> Result<Option<f32>, String> {
    fn conjuncts<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
        if let Expr::And(a, b) = expr {
            conjuncts(a, out);
            conjuncts(b, out);
        } else {
            out.push(expr);
        }
    }
    fn anywhere(expr: &Expr) -> usize {
        match expr {
            Expr::FinishedAt(_) => 1,
            Expr::Not(inner) => anywhere(inner),
            Expr::And(a, b) | Expr::Or(a, b) => anywhere(a) + anywhere(b),
            _ => 0,
        }
    }
    let mut top = Vec::new();
    conjuncts(expr, &mut top);
    let phases: Vec<f32> = top
        .iter()
        .filter_map(|expr| if let Expr::FinishedAt(phase) = expr { Some(*phase) } else { None })
        .collect();
    if anywhere(expr) != phases.len() {
        return Err(
            "`finished_at` says where the next state starts, so it stands only at the top, joined by `&&`"
                .to_owned(),
        );
    }
    match phases.as_slice() {
        [] => Ok(None),
        [phase] => Ok(Some(*phase)),
        _ => Err("`finished_at` more than once".to_owned()),
    }
}

fn emit(expr: &Expr, parameters: &[(&str, ParameterKind)], out: &mut Compiled) -> Result<(), String> {
    match expr {
        Expr::Constant(value) => out.ops.push(ConditionOp::Constant(*value)),
        Expr::Finished => {
            out.finished = true;
            out.ops.push(ConditionOp::Finished);
        }
        Expr::FinishedAt(phase) => {
            if !(0.0..1.0).contains(phase) {
                return Err(format!("`finished_at({phase})` isn't a phase, 0 up to 1"));
            }
            out.finished = true;
            out.ops.push(ConditionOp::Finished);
        }
        Expr::Parameter(name) => {
            let (index, kind) = parameter(name, parameters, out)?;
            if kind != ParameterKind::Truth {
                return Err(format!("`{name}` is a {}, not a truth: compare it", kind_name(kind)));
            }
            out.ops.push(ConditionOp::Truth(index));
        }
        Expr::Compare(left, comparison, right) => compare(left, *comparison, right, parameters, out)?,
        Expr::Not(inner) => {
            emit(inner, parameters, out)?;
            out.ops.push(ConditionOp::Not);
        }
        Expr::And(a, b) | Expr::Or(a, b) => {
            emit(a, parameters, out)?;
            emit(b, parameters, out)?;
            out.ops.push(if matches!(expr, Expr::And(..)) { ConditionOp::And } else { ConditionOp::Or });
        }
    }
    Ok(())
}

fn compare(
    left: &Operand,
    comparison: Comparison,
    right: &Operand,
    parameters: &[(&str, ParameterKind)],
    out: &mut Compiled,
) -> Result<(), String> {
    use Operand::{Mode, Number, Parameter, Truth};
    let equality = matches!(comparison, Comparison::Equal | Comparison::NotEqual);
    // A literal on the left reads as the same test the other way round.
    let (left, comparison, right) = match (left, right) {
        (Number(_) | Truth(_) | Mode(_), Parameter(_)) => (right, comparison.flipped(), left),
        _ => (left, comparison, right),
    };
    let Parameter(name) = left else {
        return Err("a comparison of two values that aren't parameters".to_owned());
    };
    let (index, kind) = parameter(name, parameters, out)?;
    let refuse = |what: &str| Err(format!("`{name}` is a {}: {what}", kind_name(kind)));
    match (kind, right) {
        (ParameterKind::Number, Number(value)) => {
            if equality {
                return refuse("compare it with `<`, `<=`, `>` or `>=`, since exact numbers seldom agree");
            }
            if !value.is_finite() {
                return Err(format!("{value} isn't a number"));
            }
            out.ops.push(ConditionOp::Compare { parameter: index, comparison, value: *value });
        }
        (ParameterKind::Number | ParameterKind::Truth, Parameter(other)) => {
            let (other_index, other_kind) = parameter(other, parameters, out)?;
            if other_kind != kind {
                return Err(format!(
                    "`{name}` is a {} and `{other}` a {}",
                    kind_name(kind),
                    kind_name(other_kind)
                ));
            }
            if (kind == ParameterKind::Number) == equality {
                return refuse(if equality {
                    "compare it with `<`, `<=`, `>` or `>=`, since exact numbers seldom agree"
                } else {
                    "compare it with `==` or `!=`"
                });
            }
            out.ops.push(ConditionOp::Between { left: index, comparison, right: other_index });
        }
        (ParameterKind::Truth, Truth(value)) => {
            if !equality {
                return refuse("compare it with `==` or `!=`");
            }
            out.ops.push(ConditionOp::Truth(index));
            if *value != (comparison == Comparison::Equal) {
                out.ops.push(ConditionOp::Not);
            }
        }
        (ParameterKind::Mode, Mode(mode)) => {
            if !equality {
                return refuse("compare it with `==` or `!=`");
            }
            out.ops.push(ConditionOp::Mode { parameter: index, mode: mode.code() });
            if comparison == Comparison::NotEqual {
                out.ops.push(ConditionOp::Not);
            }
        }
        (_, Parameter(_)) => return refuse("it compares only with a mode's name"),
        (_, Number(_)) => return refuse("it doesn't compare with a number"),
        (_, Truth(_)) => return refuse("it doesn't compare with `true` or `false`"),
        (_, Mode(_)) => return refuse("it doesn't compare with a mode"),
    }
    Ok(())
}

fn parameter(
    name: &str,
    parameters: &[(&str, ParameterKind)],
    out: &mut Compiled,
) -> Result<(u8, ParameterKind), String> {
    let index = parameters
        .iter()
        .position(|(declared, _)| *declared == name)
        .ok_or_else(|| format!("reads `{name}`, which `parameters` doesn't name"))?;
    let bit = u32::try_from(index).ok().and_then(|index| 1_u32.checked_shl(index));
    let bit = bit.ok_or_else(|| format!("reads `{name}`, past the first {} parameters", u32::BITS))?;
    out.reads |= bit;
    Ok((index as u8, parameters[index].1))
}

fn kind_name(kind: ParameterKind) -> &'static str {
    match kind {
        ParameterKind::Number => "number",
        ParameterKind::Truth => "truth",
        ParameterKind::Mode => "traversal mode",
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Name,
    Number(f32),
    Symbol,
}

#[derive(Clone, Debug)]
struct Token<'a> {
    kind: Kind,
    text: &'a str,
}

fn lex(text: &str) -> Result<Vec<Token<'_>>, String> {
    if text.len() > MAX_CONDITION_BYTES {
        return Err(format!("{} bytes long, past the limit of {MAX_CONDITION_BYTES}", text.len()));
    }
    // A condition is ASCII: the first character that isn't is refused whole, never sliced through.
    if let Some(other) = text.chars().find(|c| !c.is_ascii()) {
        return Err(format!("`{other}` isn't part of a condition"));
    }
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        let start = at;
        if byte.is_ascii_whitespace() {
            at += 1;
            continue;
        }
        let kind = if byte.is_ascii_alphabetic() || byte == b'_' {
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
            Kind::Name
        } else if byte.is_ascii_digit() || (byte == b'-' && bytes.get(at + 1).is_some_and(u8::is_ascii_digit))
        {
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == b'.') {
                at += 1;
            }
            // An exponent, as Rust writes a very large or small number.
            if at < bytes.len() && (bytes[at] == b'e' || bytes[at] == b'E') {
                at += 1;
                if at < bytes.len() && (bytes[at] == b'-' || bytes[at] == b'+') {
                    at += 1;
                }
                while at < bytes.len() && bytes[at].is_ascii_digit() {
                    at += 1;
                }
            }
            let number = &text[start..at];
            Kind::Number(number.parse().map_err(|_| format!("`{number}` isn't a number"))?)
        } else {
            let two = text.get(at..at + 2).unwrap_or_default();
            at += if ["&&", "||", "==", "!=", "<=", ">="].contains(&two) { 2 } else { 1 };
            let symbol = &text[start..at];
            if !["&&", "||", "==", "!=", "<=", ">=", "<", ">", "!", "(", ")"].contains(&symbol) {
                return Err(format!("`{symbol}` isn't part of a condition"));
            }
            Kind::Symbol
        };
        tokens.push(Token { kind, text: &text[start..at] });
        if tokens.len() > MAX_CONDITION_TOKENS {
            return Err(format!("more than {MAX_CONDITION_TOKENS} tokens, past the limit"));
        }
    }
    if tokens.is_empty() {
        return Err("an empty condition".to_owned());
    }
    Ok(tokens)
}

struct Parser<'t, 'a> {
    tokens: &'t [Token<'a>],
    at: usize,
}

impl Parser<'_, '_> {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.at).map(|token| token.text)
    }

    fn eat(&mut self, symbol: &str) -> bool {
        let found = self.peek() == Some(symbol);
        self.at += usize::from(found);
        found
    }

    fn expect(&mut self, symbol: &str) -> Result<(), String> {
        if self.eat(symbol) {
            Ok(())
        } else {
            Err(match self.peek() {
                Some(found) => format!("`{symbol}` expected, `{found}` found"),
                None => format!("`{symbol}` expected at the end"),
            })
        }
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut expr = self.and()?;
        while self.eat("||") {
            expr = Expr::Or(Box::new(expr), Box::new(self.and()?));
        }
        Ok(expr)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut expr = self.unary()?;
        while self.eat("&&") {
            expr = Expr::And(Box::new(expr), Box::new(self.unary()?));
        }
        Ok(expr)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat("!") { Ok(Expr::Not(Box::new(self.unary()?))) } else { self.primary() }
    }

    fn primary(&mut self) -> Result<Expr, String> {
        if self.eat("(") {
            let expr = self.or()?;
            self.expect(")")?;
            return Ok(expr);
        }
        match self.peek() {
            Some("finished") => {
                self.at += 1;
                return Ok(Expr::Finished);
            }
            Some("finished_at") => {
                self.at += 1;
                self.expect("(")?;
                let Some(Token { kind: Kind::Number(phase), .. }) = self.tokens.get(self.at).cloned() else {
                    return Err("`finished_at` takes a phase, a number".to_owned());
                };
                self.at += 1;
                self.expect(")")?;
                return Ok(Expr::FinishedAt(phase));
            }
            _ => {}
        }
        let left = self.operand()?;
        let comparison = match self.peek() {
            Some("<") => Comparison::Less,
            Some("<=") => Comparison::AtMost,
            Some(">") => Comparison::Greater,
            Some(">=") => Comparison::AtLeast,
            Some("==") => Comparison::Equal,
            Some("!=") => Comparison::NotEqual,
            _ => {
                return match left {
                    Operand::Parameter(name) => Ok(Expr::Parameter(name)),
                    Operand::Truth(value) => Ok(Expr::Constant(value)),
                    Operand::Number(value) => Err(format!("{value} alone isn't a condition")),
                    Operand::Mode(mode) => Err(format!("`{}` alone isn't a condition", mode.name())),
                };
            }
        };
        self.at += 1;
        let right = self.operand()?;
        Ok(Expr::Compare(left, comparison, right))
    }

    fn operand(&mut self) -> Result<Operand, String> {
        let Some(token) = self.tokens.get(self.at) else {
            return Err("a condition that stops short".to_owned());
        };
        self.at += 1;
        match (&token.kind, token.text) {
            (Kind::Number(value), _) => Ok(Operand::Number(*value)),
            (Kind::Name, "true") => Ok(Operand::Truth(true)),
            (Kind::Name, "false") => Ok(Operand::Truth(false)),
            (Kind::Name, "finished" | "finished_at") => Err(format!("`{}` doesn't compare", token.text)),
            (Kind::Name, name) => match TraversalMode::ALL.into_iter().find(|mode| mode.name() == name) {
                Some(mode) => Ok(Operand::Mode(mode)),
                None if name.starts_with(|c: char| c.is_ascii_lowercase()) => {
                    Ok(Operand::Parameter(name.to_owned()))
                }
                None => Err(format!("`{name}` is neither a parameter nor a traversal mode")),
            },
            (Kind::Symbol, symbol) => Err(format!("`{symbol}` where a value goes")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMETERS: &[(&str, ParameterKind)] = &[
        ("speed", ParameterKind::Number),
        ("grounded", ParameterKind::Truth),
        ("mode", ParameterKind::Mode),
        ("aim", ParameterKind::Number),
        ("landed", ParameterKind::Truth),
    ];

    fn ops(text: &str) -> Vec<ConditionOp> {
        compile(text, PARAMETERS).unwrap_or_else(|error| panic!("`{text}`: {error}")).ops
    }

    fn error(text: &str) -> String {
        compile(text, PARAMETERS).expect_err(text)
    }

    #[test]
    fn conditions_compile_to_postfix_ops() {
        assert_eq!(ops("grounded"), [ConditionOp::Truth(1)]);
        assert_eq!(ops("!grounded"), [ConditionOp::Truth(1), ConditionOp::Not]);
        assert_eq!(
            ops("!grounded && speed >= 1.5"),
            [
                ConditionOp::Truth(1),
                ConditionOp::Not,
                ConditionOp::Compare { parameter: 0, comparison: Comparison::AtLeast, value: 1.5 },
                ConditionOp::And,
            ]
        );
        assert_eq!(
            ops("2 > speed"),
            [ConditionOp::Compare { parameter: 0, comparison: Comparison::Less, value: 2.0 }],
            "a number on the left turns the test round",
        );
        assert_eq!(ops("mode != Climbing"), [ConditionOp::Mode { parameter: 2, mode: 1 }, ConditionOp::Not]);
        assert_eq!(ops("grounded == false"), [ConditionOp::Truth(1), ConditionOp::Not]);
        assert_eq!(
            ops("aim < speed"),
            [ConditionOp::Between { left: 3, comparison: Comparison::Less, right: 0 }]
        );
        assert_eq!(ops("true"), [ConditionOp::Constant(true)]);
    }

    #[test]
    fn and_binds_tighter_than_or_and_brackets_group() {
        let loose = ops("landed || grounded && speed > 1");
        assert_eq!(loose.last(), Some(&ConditionOp::Or));
        let grouped = ops("(landed || grounded) && speed > 1");
        assert_eq!(grouped.last(), Some(&ConditionOp::And));
    }

    #[test]
    fn what_a_condition_reads_and_asks_is_recorded() {
        let compiled =
            compile("finished_at(0.65) && grounded && speed > -0.5", PARAMETERS).expect("compiles");
        assert_eq!(compiled.reads, 0b11);
        assert!(compiled.finished);
        assert_eq!(compiled.phase, Some(0.65));
        assert_eq!(compiled.depth, 2);
        let plain = compile("finished", PARAMETERS).expect("compiles");
        assert!(plain.finished && plain.phase.is_none() && plain.reads == 0);
    }

    #[test]
    fn a_stack_too_deep_or_a_condition_too_long_is_refused() {
        let deep = "(grounded || (grounded || (grounded || (grounded || (grounded || (grounded || (grounded || (grounded || grounded))))))))";
        assert!(error(deep).contains("deep"), "{}", error(deep));
        let long = vec!["grounded"; MAX_CONDITION_OPS / 2 + 1].join(" && ");
        assert!(error(&long).contains("steps"), "{}", error(&long));
        // Thousands of `!` or `(` are refused before the parser recurses into them.
        let nots = format!("{}grounded", "!".repeat(1000));
        assert!(error(&nots).contains("tokens"), "{}", error(&nots));
        let brackets = format!("{}grounded", "(".repeat(5000));
        assert!(error(&brackets).contains("bytes long"), "{}", error(&brackets));
    }

    #[test]
    fn every_misuse_is_named() {
        let rows: &[(&str, &str)] = &[
            ("", "empty"),
            ("speed", "compare it"),
            ("speed == 1.0", "seldom agree"),
            ("grounded < 1", "doesn't compare with a number"),
            ("grounded > landed", "`==` or `!=`"),
            ("mode > Climbing", "`==` or `!=`"),
            ("mode == 1", "doesn't compare with a number"),
            ("speed == Gliding", "doesn't compare with a mode"),
            ("speed < grounded", "a number and `grounded` a truth"),
            ("flying", "`parameters` doesn't name"),
            ("Flying", "neither a parameter"),
            ("1 < 2", "aren't parameters"),
            ("3", "alone isn't a condition"),
            ("grounded &&", "stops short"),
            ("(grounded", "`)` expected"),
            ("grounded landed", "follows a whole condition"),
            ("grounded & landed", "isn't part of a condition"),
            ("speed \u{2265} 1.0", "`\u{2265}` isn't part of a condition"),
            ("grounded && \u{e9}", "isn't part of a condition"),
            ("finished_at(1.0)", "isn't a phase"),
            ("!finished_at(0.5)", "only at the top"),
            ("grounded || finished_at(0.5)", "only at the top"),
            ("finished_at(0.1) && finished_at(0.2)", "more than once"),
            ("finished_at(speed)", "takes a phase"),
            ("finished == true", "follows a whole condition"),
            ("speed < finished", "doesn't compare"),
            ("speed < 1.2.3", "isn't a number"),
        ];
        for (text, expected) in rows {
            let message = error(text);
            assert!(message.contains(expected), "`{text}` gave `{message}`, wanting `{expected}`");
        }
    }
}
