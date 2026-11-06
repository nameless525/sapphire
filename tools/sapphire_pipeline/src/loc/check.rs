//! What a cook checks across a set of tables, and what it lists: see the
//! [module docs](super).
//!
//! Each table runs with its chain, and a reference looks along the chain from the table it's in, as
//! `sapphire_loc` does, so everything here is checked per chain: an English message standing in for
//! Japanese must still resolve from English on.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use sapphire_strings::{Expression, MAX_DEPTH, MAX_STEPS, Part, Pattern, StringTable, VariantKey};

/// A key a locale lacks, shown from another down its chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Untranslated {
    pub locale: String,
    pub key: String,
    /// The locale that serves it.
    pub served_by: String,
}

/// A key a locale holds that nothing down its chain does: a translation of something gone, or of a key
/// not yet written where it should be first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stale {
    pub locale: String,
    pub key: String,
}

/// What a cook lists, in locale and key order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocReport {
    pub untranslated: Vec<Untranslated>,
    pub stale: Vec<Stale>,
}

/// Tables that can't ship together.
#[derive(Debug, thiserror::Error)]
pub enum LocCheckError {
    #[error("a .ftl source says it's locale `{locale}`, and no `{locale}.locale.ron` states that: {}", path.display())]
    NoLocaleDocument { locale: String, path: PathBuf },
    #[error("locale `{locale}` has two documents: {} and {}", first.display(), second.display())]
    DuplicateLocale { locale: String, first: PathBuf, second: PathBuf },
    #[error("locale `{locale}` falls back to `{fallback}`, which no locale document states")]
    UnknownFallback { locale: String, fallback: String },
    #[error("locale `{locale}` names `{fallback}` twice in its chain, or itself")]
    RepeatedFallback { locale: String, fallback: String },
    #[error(
        "`{key}` in locale `{holder}` (reading `{locale}`) references `{reference}`, which its chain doesn't hold"
    )]
    MissingReference { locale: String, holder: String, key: String, reference: String },
    #[error("in locale `{locale}`, references go round: {path}")]
    ReferenceCycle { locale: String, path: String },
    #[error("in locale `{locale}`, `{key}` goes {depth} references or selects deep, past {MAX_DEPTH}")]
    TooDeep { locale: String, key: String, depth: u32 },
    #[error("in locale `{locale}`, `{key}` takes {steps} steps to format, past {MAX_STEPS}")]
    TooManySteps { locale: String, key: String, steps: u32 },
    #[error("`{key}` uses `${variable}` in locale `{has}` and not in `{lacks}`: a caller passes one set")]
    VariableMismatch { key: String, variable: String, has: String, lacks: String },
    #[error(
        "`${variable}` is a number in `{key}` of locale `{number}` and chosen by name in locale `{text}`"
    )]
    SelectorMismatch { key: String, variable: String, number: String, text: String },
    #[error("cooked table {} can't be read", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: Box<sapphire_strings::StringsError>,
    },
}

/// Checks `tables` against each other, and says what's untranslated. See the [module docs](super).
pub fn check(tables: &[StringTable]) -> Result<LocReport, LocCheckError> {
    let by_locale: BTreeMap<&str, &StringTable> =
        tables.iter().map(|table| (table.locale.as_str(), table)).collect();
    let mut report = LocReport::default();
    for (&locale, table) in &by_locale {
        let mut chain: Vec<&StringTable> = vec![table];
        for fallback in &table.fallback {
            if *fallback == locale || table.fallback.iter().filter(|name| *name == fallback).count() > 1 {
                return Err(LocCheckError::RepeatedFallback {
                    locale: locale.to_owned(),
                    fallback: fallback.clone(),
                });
            }
            let next = by_locale.get(fallback.as_str()).ok_or_else(|| LocCheckError::UnknownFallback {
                locale: locale.to_owned(),
                fallback: fallback.clone(),
            })?;
            chain.push(next);
        }
        Chain::new(&chain).check()?;
        list(&chain, &mut report);
    }
    Ok(report)
}

/// Adds what the head of `chain` lacks and what it alone holds.
fn list(chain: &[&StringTable], report: &mut LocReport) {
    let [head, rest @ ..] = chain else { return };
    if rest.is_empty() {
        return;
    }
    let mut served: BTreeMap<&str, &str> = BTreeMap::new();
    for table in rest {
        for entry in &table.entries {
            served.entry(table.str(entry.key)).or_insert(&table.locale);
        }
    }
    for (key, served_by) in &served {
        if head.entry(key).is_none() {
            report.untranslated.push(Untranslated {
                locale: head.locale.clone(),
                key: (*key).to_owned(),
                served_by: (*served_by).to_owned(),
            });
        }
    }
    for entry in &head.entries {
        let key = head.str(entry.key);
        if !served.contains_key(key) {
            report.stale.push(Stale { locale: head.locale.clone(), key: key.to_owned() });
        }
    }
}

/// How a message uses a variable, as far as what its caller passes goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Use {
    /// Shown, or chosen by with only a default: either kind of value does.
    Any,
    /// Formatted with `NUMBER`, or chosen by plural category or exact number.
    Number,
    /// Chosen by a name no plural category has.
    Text,
}

const PLURAL_CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

/// What walking a pattern found, for a limit's check.
#[derive(Clone, Copy, Default)]
struct Reach {
    depth: u32,
    steps: u32,
}

struct Chain<'a> {
    links: &'a [&'a StringTable],
    /// `(link, key)`, once walked. `None` while being walked, so meeting it again is a loop.
    walked: HashMap<(usize, &'a str), Option<Reach>>,
    /// Message summaries keep each variable's first non-Any use in traversal order. Terms
    /// have their own argument scope and never contribute to a caller's summary.
    variable_uses: HashMap<(usize, &'a str), BTreeMap<&'a str, Use>>,
}

impl<'a> Chain<'a> {
    fn new(links: &'a [&'a StringTable]) -> Self {
        Self { links, walked: HashMap::new(), variable_uses: HashMap::new() }
    }

    /// Every entry of every link, as this chain's head would run it.
    fn check(&mut self) -> Result<(), LocCheckError> {
        let links = self.links;
        for (link, table) in links.iter().enumerate() {
            for entry in &table.entries {
                let mut path = Vec::new();
                self.entry(link, table.str(entry.key), &mut path, 1)?;
            }
        }
        self.variables()
    }

    fn head(&self) -> String {
        self.links[0].locale.clone()
    }

    /// `key` in `link`: its reach, checked against the limits.
    fn entry(
        &mut self,
        link: usize,
        key: &'a str,
        path: &mut Vec<&'a str>,
        depth: u32,
    ) -> Result<Reach, LocCheckError> {
        path.push(key);
        match self.walked.get(&(link, key)) {
            Some(Some(reach)) => {
                let reach = *reach;
                path.pop();
                return Ok(reach);
            }
            Some(None) => {
                let start = path.iter().position(|step| *step == key).unwrap_or(0);
                return Err(LocCheckError::ReferenceCycle {
                    locale: self.head(),
                    path: path[start..].join(" -> "),
                });
            }
            None => {}
        }
        self.walked.insert((link, key), None);
        let table = self.links[link];
        let entry = table.entry(key).expect("walked keys are found first");
        let reach = self.pattern(link, key, entry.pattern, path, depth)?;
        if reach.depth > MAX_DEPTH {
            let (locale, key) = (self.head(), key.to_owned());
            return Err(LocCheckError::TooDeep { locale, key, depth: reach.depth });
        }
        if reach.steps > MAX_STEPS {
            let (locale, key) = (self.head(), key.to_owned());
            return Err(LocCheckError::TooManySteps { locale, key, steps: reach.steps });
        }
        self.walked.insert((link, key), Some(reach));
        path.pop();
        Ok(reach)
    }

    /// Finds `reference` from `link` on, which is where a format will.
    fn find(&self, link: usize, reference: &str) -> Option<(usize, &'a str)> {
        self.links.iter().enumerate().skip(link).find_map(|(found, table)| {
            let entry = table.entry(reference)?;
            Some((found, table.str(entry.key)))
        })
    }

    /// A pattern's reach: the format's own count, one step for the pattern and one for each part, one
    /// level for each reference or variant, and of a select's variants the most any takes.
    fn pattern(
        &mut self,
        link: usize,
        key: &'a str,
        pattern: Pattern,
        path: &mut Vec<&'a str>,
        depth: u32,
    ) -> Result<Reach, LocCheckError> {
        // Bound the active walk before following another reference or select. Checking the
        // resulting reach only on unwind lets a forward chain exhaust the host stack first.
        if depth > MAX_DEPTH {
            return Err(LocCheckError::TooDeep {
                locale: self.head(),
                key: path.first().copied().unwrap_or(key).to_owned(),
                depth,
            });
        }
        let table = self.links[link];
        let mut reach = Reach { depth: 1, steps: 1 };
        let Pattern::Parts(run) = pattern else { return Ok(reach) };
        for part in table.parts_of(run) {
            reach.steps = reach.steps.saturating_add(1);
            let child = match *part {
                Part::Text(_) => None,
                Part::Place(expression) => self.expression(link, key, expression, path, depth)?,
                Part::Select(index) => {
                    let select = table.selects[index as usize];
                    // A term's attribute it chooses by has to be there, though a format reads only its text.
                    self.expression(link, key, select.selector, path, depth)?;
                    let mut most = Reach::default();
                    for variant in table.variants_of(select.variants) {
                        let child = self.pattern(link, key, variant.pattern, path, depth + 1)?;
                        most.depth = most.depth.max(child.depth);
                        most.steps = most.steps.max(child.steps);
                    }
                    Some(most)
                }
            };
            if let Some(child) = child {
                reach.depth = reach.depth.max(child.depth.saturating_add(1));
                reach.steps = reach.steps.saturating_add(child.steps);
            }
        }
        Ok(reach)
    }

    fn expression(
        &mut self,
        link: usize,
        key: &'a str,
        expression: Expression,
        path: &mut Vec<&'a str>,
        depth: u32,
    ) -> Result<Option<Reach>, LocCheckError> {
        let table = self.links[link];
        let reference = match expression {
            Expression::Message(reference) | Expression::Term { key: reference, .. } => table.str(reference),
            _ => return Ok(None),
        };
        let Some((found, stored)) = self.find(link, reference) else {
            return Err(LocCheckError::MissingReference {
                locale: self.head(),
                holder: table.locale.clone(),
                key: key.to_owned(),
                reference: reference.to_owned(),
            });
        };
        self.entry(found, stored, path, depth + 1).map(Some)
    }

    /// Holds each key the head has to what the next link holding it uses: one set of variables, each
    /// of one kind. Terms are left out: their arguments are their own business in each locale.
    fn variables(&mut self) -> Result<(), LocCheckError> {
        let links = self.links;
        let head = links[0];
        for entry in &head.entries {
            let key = head.str(entry.key);
            if key.starts_with('-') {
                continue;
            }
            let Some((found, _)) = self.find(1, key) else { continue };
            self.collect_uses(0, key);
            self.collect_uses(found, key);
            let mine = &self.variable_uses[&(0, key)];
            let theirs = &self.variable_uses[&(found, key)];
            let other = &links[found].locale;
            for (variable, use_of) in mine {
                let Some(their_use) = theirs.get(variable) else {
                    return Err(LocCheckError::VariableMismatch {
                        key: key.to_owned(),
                        variable: (*variable).to_owned(),
                        has: head.locale.clone(),
                        lacks: other.clone(),
                    });
                };
                let (number, text) = match (use_of, their_use) {
                    (Use::Number, Use::Text) => (&head.locale, other),
                    (Use::Text, Use::Number) => (other, &head.locale),
                    _ => continue,
                };
                return Err(LocCheckError::SelectorMismatch {
                    key: key.to_owned(),
                    variable: (*variable).to_owned(),
                    number: number.clone(),
                    text: text.clone(),
                });
            }
            if let Some(variable) = theirs.keys().find(|variable| !mine.contains_key(*variable)) {
                return Err(LocCheckError::VariableMismatch {
                    key: key.to_owned(),
                    variable: (*variable).to_owned(),
                    has: other.clone(),
                    lacks: head.locale.clone(),
                });
            }
        }
        Ok(())
    }

    /// The variables `key` in `link` uses, through the messages it references: they read the same
    /// caller. Loops were refused before this runs.
    fn collect_uses(&mut self, link: usize, key: &'a str) {
        if self.variable_uses.contains_key(&(link, key)) {
            return;
        }
        let mut uses = BTreeMap::new();
        let table = self.links[link];
        if let Some(entry) = table.entry(key) {
            self.pattern_uses(link, entry.pattern, &mut uses);
        }
        self.variable_uses.insert((link, key), uses);
    }

    fn pattern_uses(&mut self, link: usize, pattern: Pattern, uses: &mut BTreeMap<&'a str, Use>) {
        let table = self.links[link];
        let Pattern::Parts(run) = pattern else { return };
        for part in table.parts_of(run) {
            match *part {
                Part::Text(_) => {}
                Part::Place(expression) => self.expression_uses(link, expression, Use::Any, uses),
                Part::Select(index) => {
                    let select = table.selects[index as usize];
                    let variants = table.variants_of(select.variants);
                    let by = variants.iter().fold(Use::Any, |by, variant| match variant.key {
                        VariantKey::Number(_) => Use::Number,
                        VariantKey::Name(name) if table.str(name) == "other" => by,
                        VariantKey::Name(name) if PLURAL_CATEGORIES.contains(&table.str(name)) => {
                            if by == Use::Text { Use::Text } else { Use::Number }
                        }
                        VariantKey::Name(_) => Use::Text,
                    });
                    self.expression_uses(link, select.selector, by, uses);
                    for variant in variants {
                        self.pattern_uses(link, variant.pattern, uses);
                    }
                }
            }
        }
    }

    fn expression_uses(
        &mut self,
        link: usize,
        expression: Expression,
        by: Use,
        uses: &mut BTreeMap<&'a str, Use>,
    ) {
        let table = self.links[link];
        let (name, used) = match expression {
            Expression::Variable(name) => (table.str(name), by),
            Expression::NumberVariable { name, .. } => (table.str(name), Use::Number),
            Expression::Message(key) => {
                if let Some((found, stored)) = self.find(link, table.str(key)) {
                    self.collect_uses(found, stored);
                    for (&name, &used) in &self.variable_uses[&(found, stored)] {
                        merge_use(uses, name, used);
                    }
                }
                return;
            }
            Expression::Number { .. } | Expression::Term { .. } => return,
        };
        merge_use(uses, name, used);
    }
}

/// Combining ordered summaries is the same as replaying every use: Any never replaces a typed
/// use, and the first non-Any use wins. In particular, conflicting uses must not be sorted by type.
fn merge_use<'a>(uses: &mut BTreeMap<&'a str, Use>, name: &'a str, used: Use) {
    let held = uses.entry(name).or_insert(used);
    if *held == Use::Any {
        *held = used;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loc::{LocaleDocument, compile};

    fn table(locale: &str, fallback: &[&str], text: &str) -> StringTable {
        let document = LocaleDocument {
            fallback: fallback.iter().map(|name| (*name).to_owned()).collect(),
            ..LocaleDocument::default()
        };
        compile(locale, &document, &[(PathBuf::from("a.ftl"), text.to_owned())]).unwrap()
    }

    #[test]
    fn a_forward_chain_stops_before_descending_past_the_depth_budget() {
        let mut source = String::new();
        for index in 0..64 {
            source.push_str(&format!("m{index:03} = {{ m{:03} }}\n", index + 1));
        }
        // The absent final reference would be reached by an unbounded recursive walk. No large
        // stack or exhaustion witness is needed: the checker must reject before looking there.
        let tables = [table("en", &[], &source)];
        let error = check(&tables).unwrap_err();
        assert!(matches!(error, LocCheckError::TooDeep { key, depth, .. }
            if key == "m000" && depth == MAX_DEPTH + 1));
        let mut source = String::from("m031 = end\n");
        for index in 0..31 {
            source.push_str(&format!("m{index:03} = {{ m{:03} }}\n", index + 1));
        }
        assert!(check(&[table("en", &[], &source)]).is_ok(), "exactly MAX_DEPTH is accepted");
    }

    #[test]
    fn summaries_preserve_first_typed_use_scope_and_error_order() {
        let source = concat!(
            "-term = { $private }\n",
            "a = { $n } { b } { c } { b } { -term(private: \"x\") }\n",
            "b = { NUMBER($n) }\n",
            "c = { $n ->\n    [named] { $z }\n   *[other] { $a }\n}\n",
            "reverse = { c } { b }\n",
        );
        let en = table("en", &[], source);
        let links = [&en];
        let mut chain = Chain::new(&links);
        chain.check().unwrap();
        chain.collect_uses(0, "a");
        assert_eq!(
            chain.variable_uses[&(0, "a")],
            BTreeMap::from([("a", Use::Any), ("n", Use::Number), ("z", Use::Any),])
        );
        chain.collect_uses(0, "reverse");
        assert_eq!(chain.variable_uses[&(0, "reverse")].get("n"), Some(&Use::Text));
        let ja = table("ja", &["en"], "a = { $z }\n");
        let error = check(&[en, ja]).unwrap_err().to_string();
        assert_eq!(error, "`a` uses `$a` in locale `en` and not in `ja`: a caller passes one set");
    }

    #[test]
    fn repeated_select_branches_share_variable_summaries() {
        let mut source = String::from("m014 = { NUMBER($n) }\n");
        for index in 0..14 {
            source.push_str(&format!(
                "m{index:03} = {{ $choice ->\n    [a] {{ m{next:03} }}\n   *[other] {{ m{next:03} }}\n}}\n",
                next = index + 1,
            ));
        }
        let en = table("en", &[], &source);
        let ja = table("ja", &["en"], &source);
        assert_eq!(check(&[en.clone(), ja]).unwrap(), LocReport::default());
        let links = [&en];
        let mut chain = Chain::new(&links);
        chain.check().unwrap();
        chain.collect_uses(0, "m000");
        assert_eq!(chain.variable_uses.len(), 15);
        assert_eq!(
            chain.variable_uses[&(0, "m000")],
            BTreeMap::from([("choice", Use::Text), ("n", Use::Number),])
        );
    }
}
