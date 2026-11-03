//! What a key can show, without running it: for a cook that has a document naming the key and needs
//! to know which characters its fonts must hold.

use std::collections::{BTreeSet, HashSet};

use crate::table::{
    ArgValue, Expression, NUMBER_CHARACTERS, Part, Pattern, Span, StringTable,
};

/// What [`key_shows`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyShows {
    /// Every character any arm of any select can show, with what numbers are written in if a number
    /// or a variable shows. Not what a variable holds, which only its caller knows.
    pub characters: BTreeSet<char>,
    /// The variables the caller must pass: those of messages, not the literals a term is given.
    pub variables: BTreeSet<String>,
}

/// What `key` can show, looked up as the runtime does: along `chain` from its head, a reference along
/// the chain from where the message referring to it was found. None if no table of `chain` holds it.
/// A reference that nothing holds shows nothing here, and `sapphire_pipeline::loc` refuses it.
#[must_use]
pub fn key_shows(chain: &[&StringTable], key: &str) -> Option<KeyShows> {
    let mut walk = Walk { chain, shows: KeyShows::default(), visited: HashSet::new() };
    let (link, table, pattern) = walk.find(0, key)?;
    walk.pattern(link, table, pattern, true);
    Some(walk.shows)
}

struct Walk<'a> {
    chain: &'a [&'a StringTable],
    shows: KeyShows,
    /// What's been walked, and whether in a term's scope: a table's references can form a loop, which
    /// the runtime stops by depth.
    visited: HashSet<(usize, &'a str, bool)>,
}

impl<'a> Walk<'a> {
    fn find(&self, from: usize, key: &str) -> Option<(usize, &'a StringTable, Pattern)> {
        self.chain.iter().enumerate().skip(from).find_map(|(link, table)| {
            table.entry(key).map(|entry| (link, *table, entry.pattern))
        })
    }

    fn numbers(&mut self) {
        self.shows.characters.extend(NUMBER_CHARACTERS.chars());
    }

    fn text(&mut self, text: &str) {
        self.shows.characters.extend(text.chars());
    }

    fn variable(&mut self, table: &StringTable, name: Span, message: bool) {
        // A variable can hold a number, whatever else it holds.
        self.numbers();
        if message {
            self.shows.variables.insert(table.str(name).to_owned());
        }
    }

    fn pattern(&mut self, link: usize, table: &'a StringTable, pattern: Pattern, message: bool) {
        let run = match pattern {
            Pattern::Plain(span) => {
                self.text(table.str(span));
                return;
            }
            Pattern::Parts(run) => run,
        };
        for part in table.parts_of(run) {
            match *part {
                Part::Text(span) => self.text(table.str(span)),
                Part::Place(expression) => self.expression(link, table, expression, message),
                Part::Select(index) => {
                    let Some(select) = table.selects.get(index as usize) else { continue };
                    // A term's attribute chooses, and doesn't show.
                    match select.selector {
                        Expression::Variable(name) => self.selector(table, name, message),
                        Expression::NumberVariable { name, .. } => self.selector(table, name, message),
                        _ => {}
                    }
                    for variant in table.variants_of(select.variants) {
                        self.pattern(link, table, variant.pattern, message);
                    }
                }
            }
        }
    }

    fn selector(&mut self, table: &StringTable, name: Span, message: bool) {
        if message {
            self.shows.variables.insert(table.str(name).to_owned());
        }
    }

    fn expression(&mut self, link: usize, table: &'a StringTable, expression: Expression, message: bool) {
        match expression {
            Expression::Variable(name) | Expression::NumberVariable { name, .. } => {
                self.variable(table, name, message);
            }
            Expression::Number { .. } => self.numbers(),
            Expression::Message(key) => self.reference(link, table.str(key), message),
            Expression::Term { key, args } => {
                for arg in table.args_of(args) {
                    match arg.value {
                        ArgValue::Text(span) => self.text(table.str(span)),
                        ArgValue::Number { .. } => self.numbers(),
                    }
                }
                self.reference(link, table.str(key), false);
            }
        }
    }

    fn reference(&mut self, from: usize, key: &'a str, message: bool) {
        if !self.visited.insert((from, key, message)) {
            return;
        }
        if let Some((link, table, pattern)) = self.find(from, key) {
            self.pattern(link, table, pattern, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Entry, Run, Select, Variant, VariantKey};

    /// A table whose pool holds each of `texts` in turn, spans given back in the same order.
    fn pool(table: &mut StringTable, text: &str) -> Span {
        let start = table.pool.len() as u32;
        table.pool.push_str(text);
        Span { start, len: text.len() as u32 }
    }

    fn entry(table: &mut StringTable, key: &str, pattern: Pattern) {
        let key = pool(table, key);
        table.entries.push(Entry { key, pattern });
        let keys = table.pool.clone();
        table.entries.sort_by(|a, b| {
            keys[a.key.start as usize..(a.key.start + a.key.len) as usize]
                .cmp(&keys[b.key.start as usize..(b.key.start + b.key.len) as usize])
        });
    }

    fn parts(table: &mut StringTable, parts: &[Part]) -> Pattern {
        let start = table.parts.len() as u32;
        table.parts.extend_from_slice(parts);
        Pattern::Parts(Run { start, len: parts.len() as u32 })
    }

    fn characters(shows: &KeyShows) -> String {
        shows.characters.iter().collect()
    }

    #[test]
    fn a_plain_message_shows_its_text_and_needs_nothing() {
        let mut table = StringTable::default();
        let text = pool(&mut table, "あい");
        entry(&mut table, "title", Pattern::Plain(text));
        let shows = key_shows(&[&table], "title").unwrap();
        assert_eq!(characters(&shows), "あい");
        assert!(shows.variables.is_empty());
        assert_eq!(key_shows(&[&table], "other"), None);
    }

    #[test]
    fn every_arm_shows_and_a_selector_is_needed_without_showing() {
        let mut table = StringTable::default();
        let (one, other, name) = (pool(&mut table, "x"), pool(&mut table, "y"), pool(&mut table, "kind"));
        let start = table.variants.len() as u32;
        table.variants.push(Variant { key: VariantKey::Number(1.0), pattern: Pattern::Plain(one) });
        table.variants.push(Variant { key: VariantKey::Number(2.0), pattern: Pattern::Plain(other) });
        table.selects.push(Select {
            selector: Expression::Variable(name),
            variants: Run { start, len: 2 },
            default: 1,
        });
        let pattern = parts(&mut table, &[Part::Select(0)]);
        entry(&mut table, "pick", pattern);
        let shows = key_shows(&[&table], "pick").unwrap();
        assert_eq!(characters(&shows), "xy");
        assert_eq!(shows.variables.iter().collect::<Vec<_>>(), ["kind"]);
    }

    #[test]
    fn a_variable_can_hold_a_number_and_is_needed() {
        let mut table = StringTable::default();
        let name = pool(&mut table, "count");
        let pattern = parts(&mut table, &[Part::Place(Expression::Variable(name))]);
        entry(&mut table, "left", pattern);
        let shows = key_shows(&[&table], "left").unwrap();
        assert_eq!(characters(&shows), ",-.0123456789");
        assert!(shows.variables.contains("count"));
    }

    #[test]
    fn a_reference_goes_down_the_chain_and_a_term_s_variables_are_its_own() {
        let mut english = StringTable::default();
        let (word, inner) = (pool(&mut english, "E"), pool(&mut english, "case"));
        entry(&mut english, "only-english", Pattern::Plain(word));
        let term = parts(&mut english, &[Part::Place(Expression::Variable(inner))]);
        entry(&mut english, "-term", term);
        let mut japanese = StringTable::default();
        let (reference, term_key) = (pool(&mut japanese, "only-english"), pool(&mut japanese, "-term"));
        let pattern = parts(
            &mut japanese,
            &[
                Part::Place(Expression::Message(reference)),
                Part::Place(Expression::Term { key: term_key, args: Run::default() }),
            ],
        );
        entry(&mut japanese, "both", pattern);
        let shows = key_shows(&[&japanese, &english], "both").unwrap();
        assert!(shows.characters.contains(&'E'));
        assert!(shows.variables.is_empty(), "{:?}", shows.variables);
        // Not from a chain without English.
        assert!(!key_shows(&[&japanese], "both").unwrap().characters.contains(&'E'));
    }

    #[test]
    fn a_loop_of_references_ends() {
        let mut table = StringTable::default();
        let (a, b, text) = (pool(&mut table, "a"), pool(&mut table, "b"), pool(&mut table, "z"));
        let first = parts(&mut table, &[Part::Place(Expression::Message(b)), Part::Text(text)]);
        entry(&mut table, "a", first);
        let second = parts(&mut table, &[Part::Place(Expression::Message(a))]);
        entry(&mut table, "b", second);
        assert_eq!(characters(&key_shows(&[&table], "a").unwrap()), "z");
    }
}
