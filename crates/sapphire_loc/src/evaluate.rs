//! Works a pattern out into a buffer.

use icu_decimal::input::Decimal;
use icu_plurals::PluralCategory;
use sapphire_strings::{
    ArgValue, Expression, MAX_DEPTH, MAX_STEPS, NumberFormat, Part, Pattern, Span, StringTable, TermArg,
    VariantKey,
};
use writeable::Writeable as _;

use crate::localizer::{Loaded, Localizer, count};
use crate::{Arg, Value};

/// What a pattern's `$name` reads.
#[derive(Clone, Copy)]
enum Scope<'a> {
    /// The caller's values: a message's, and those of a message it references.
    Caller(&'a [Arg<'a>]),
    /// Only what a term was given, in the table that gave it.
    Term(&'a StringTable, &'a [TermArg]),
}

/// What a `$name` was bound to, in either scope.
#[derive(Clone, Copy)]
enum Bound<'a> {
    Text(&'a str),
    Integer(i64),
    Float(f64),
}

/// What a select chooses by.
enum Selection<'a> {
    Text(&'a str),
    Number { decimal: Decimal, value: f64 },
    Nothing,
}

/// One format under way.
pub(crate) struct Walk<'a, 'o> {
    localizer: &'a Localizer,
    args: &'a [Arg<'a>],
    out: &'o mut String,
    steps: u32,
    stopped: bool,
}

impl<'a, 'o> Walk<'a, 'o> {
    pub(crate) fn new(localizer: &'a Localizer, args: &'a [Arg<'a>], out: &'o mut String) -> Self {
        Self { localizer, args, out, steps: 0, stopped: false }
    }

    pub(crate) fn message(mut self, link: usize, pattern: Pattern) {
        let args = self.args;
        self.pattern(link, pattern, Scope::Caller(args), 0);
    }

    /// Counts a step of the format, and says whether it may go on. Past either limit it's a loop or a
    /// table no cook made: what's been written stays, and it's counted once.
    fn step(&mut self, depth: u32) -> bool {
        self.steps += 1;
        if depth < MAX_DEPTH && self.steps <= MAX_STEPS {
            return !self.stopped;
        }
        if !self.stopped {
            self.stopped = true;
            count(&self.localizer.counters().overruns);
        }
        false
    }

    fn pattern(&mut self, link: usize, pattern: Pattern, scope: Scope<'a>, depth: u32) {
        if !self.step(depth) {
            return;
        }
        let Some(loaded) = self.localizer.link(link) else { return };
        let table = loaded.active();
        let run = match pattern {
            Pattern::Plain(span) => return self.out.push_str(table.str(span)),
            Pattern::Parts(run) => run,
        };
        for part in table.parts_of(run) {
            if !self.step(depth) {
                return;
            }
            match *part {
                Part::Text(span) => self.out.push_str(table.str(span)),
                Part::Place(expression) => self.place(link, loaded, expression, scope, depth),
                Part::Select(index) => self.select(link, loaded, index as usize, scope, depth),
            }
        }
    }

    fn place(
        &mut self,
        link: usize,
        loaded: &'a Loaded,
        expression: Expression,
        scope: Scope<'a>,
        depth: u32,
    ) {
        let table = loaded.active();
        match expression {
            Expression::Variable(name) => {
                self.variable(loaded, table.str(name), NumberFormat::default(), scope)
            }
            Expression::NumberVariable { name, format } => {
                self.variable(loaded, table.str(name), format, scope);
            }
            Expression::Number { value, format } => self.float(loaded, value, format),
            Expression::Message(key) => self.reference(link, table, key, None, scope, depth),
            Expression::Term { key, args } => {
                self.reference(link, table, key, Some(table.args_of(args)), scope, depth);
            }
        }
    }

    fn variable(&mut self, loaded: &Loaded, name: &str, format: NumberFormat, scope: Scope<'a>) {
        match bound(name, scope) {
            Some(Bound::Text(text)) => self.out.push_str(text),
            Some(Bound::Integer(value)) => self.decimal(loaded, &integer_decimal(value, format), format),
            Some(Bound::Float(value)) => self.float(loaded, value, format),
            None => {
                count(&self.localizer.counters().missing_args);
                self.out.push_str("{$");
                self.out.push_str(name);
                self.out.push('}');
            }
        }
    }

    /// A reference, searched for from `link` on. A term's `$name`s read what it was given here, and
    /// a message's what its caller gave.
    fn reference(
        &mut self,
        link: usize,
        table: &'a StringTable,
        key: Span,
        args: Option<&'a [TermArg]>,
        scope: Scope<'a>,
        depth: u32,
    ) {
        let key = table.str(key);
        let Some((holder, entry)) = self.localizer.find(link, key) else {
            count(&self.localizer.counters().unknown_keys);
            self.out.push('{');
            self.out.push_str(key);
            self.out.push('}');
            return;
        };
        let scope = args.map_or(scope, |args| Scope::Term(table, args));
        self.pattern(holder, entry.pattern, scope, depth + 1);
    }

    fn select(&mut self, link: usize, loaded: &'a Loaded, index: usize, scope: Scope<'a>, depth: u32) {
        let table = loaded.active();
        let Some(select) = table.selects.get(index) else { return };
        let variants = table.variants_of(select.variants);
        let selection = self.selection(link, table, select.selector, scope);
        let chosen = match &selection {
            Selection::Text(text) => variants
                .iter()
                .find(|variant| matches!(variant.key, VariantKey::Name(name) if table.str(name) == *text)),
            // An exact number before a plural category, wherever each is written: `[0]` after `[other]`
            // still means zero.
            Selection::Number { decimal, value } => {
                let exact = variants.iter().find(|variant| variant.key == VariantKey::Number(*value));
                exact.or_else(|| {
                    let category = plural_name(loaded.plurals.category_for(decimal));
                    variants.iter().find(|variant| {
                        matches!(variant.key, VariantKey::Name(name) if table.str(name) == category)
                    })
                })
            }
            Selection::Nothing => None,
        };
        if let Some(variant) = chosen.or(variants.get(select.default as usize)) {
            self.pattern(link, variant.pattern, scope, depth + 1);
        }
    }

    fn selection(
        &mut self,
        link: usize,
        table: &'a StringTable,
        selector: Expression,
        scope: Scope<'a>,
    ) -> Selection<'a> {
        match selector {
            Expression::Variable(name) => {
                self.selection_of_variable(table.str(name), NumberFormat::default(), scope)
            }
            Expression::NumberVariable { name, format } => {
                self.selection_of_variable(table.str(name), format, scope)
            }
            Expression::Number { value, format } => number_selection(value, format),
            // A term's attribute, whose plain text is what's chosen by.
            Expression::Term { key, .. } => {
                let found = self.localizer.find_semantic(link, table.str(key));
                let Some((holder, entry)) = found else { return Selection::Nothing };
                match (entry.pattern, self.localizer.link(holder)) {
                    (Pattern::Plain(span), Some(loaded)) => Selection::Text(loaded.semantic().str(span)),
                    _ => Selection::Nothing,
                }
            }
            Expression::Message(_) => Selection::Nothing,
        }
    }

    fn selection_of_variable(&mut self, name: &str, format: NumberFormat, scope: Scope<'a>) -> Selection<'a> {
        match bound(name, scope) {
            Some(Bound::Text(text)) => Selection::Text(text),
            Some(Bound::Integer(value)) => {
                Selection::Number { decimal: integer_decimal(value, format), value: value as f64 }
            }
            Some(Bound::Float(value)) => number_selection(value, format),
            // It still chooses, by the default, but it's counted as one shown would be.
            None => {
                count(&self.localizer.counters().missing_args);
                Selection::Nothing
            }
        }
    }

    fn float(&mut self, loaded: &Loaded, value: f64, format: NumberFormat) {
        match float_decimal(value, format) {
            Some(decimal) => self.decimal(loaded, &decimal, format),
            None => {
                count(&self.localizer.counters().bad_numbers);
                self.out.push_str("NaN");
            }
        }
    }

    fn decimal(&mut self, loaded: &Loaded, decimal: &Decimal, format: NumberFormat) {
        let formatter = if format.grouping { &loaded.grouped } else { &loaded.ungrouped };
        // A `String` never refuses a write.
        let _ = formatter.format(decimal).write_to(self.out);
    }
}

fn bound<'a>(name: &str, scope: Scope<'a>) -> Option<Bound<'a>> {
    match scope {
        Scope::Caller(args) => {
            let &(_, value) = args.iter().find(|(arg, _)| *arg == name)?;
            Some(match value {
                Value::Text(text) => Bound::Text(text),
                Value::Integer(value) => Bound::Integer(value),
                Value::Float(value) => Bound::Float(value),
            })
        }
        Scope::Term(table, args) => {
            let arg = args.iter().find(|arg| table.str(arg.name) == name)?;
            Some(match arg.value {
                ArgValue::Text(span) => Bound::Text(table.str(span)),
                ArgValue::Number { value, .. } => Bound::Float(value),
            })
        }
    }
}

fn number_selection<'a>(value: f64, format: NumberFormat) -> Selection<'a> {
    float_decimal(value, format).map_or(Selection::Nothing, |decimal| Selection::Number { decimal, value })
}

fn integer_decimal(value: i64, format: NumberFormat) -> Decimal {
    let mut decimal = Decimal::from(value);
    decimal.absolute.pad_end(-i16::from(format.min_fraction));
    decimal
}

/// The most a float may hold to be written digit by digit: past it, `i128` can't be trusted to scale it.
const LARGEST_FLOAT: f64 = 1e30;

/// `value` rounded to its format's fraction digits, without ICU's float parsing (`ryu`), a dependency
/// for what scaling does here. `None` if it's not finite or too large.
fn float_decimal(value: f64, format: NumberFormat) -> Option<Decimal> {
    let scale = 10f64.powi(i32::from(format.max_fraction));
    let scaled = (value * scale).round();
    if !scaled.is_finite() || scaled.abs() >= LARGEST_FLOAT {
        return None;
    }
    let mut decimal = Decimal::from(scaled as i128);
    decimal.multiply_pow10(-i16::from(format.max_fraction));
    decimal.absolute.trim_end();
    decimal.absolute.pad_end(-i16::from(format.min_fraction));
    Some(decimal)
}

fn plural_name(category: PluralCategory) -> &'static str {
    match category {
        PluralCategory::Zero => "zero",
        PluralCategory::One => "one",
        PluralCategory::Two => "two",
        PluralCategory::Few => "few",
        PluralCategory::Many => "many",
        PluralCategory::Other => "other",
    }
}
