//! The tables held, the current chain and the switch.

use std::sync::atomic::{AtomicU64, Ordering};

use icu_decimal::DecimalFormatter;
use icu_decimal::options::{DecimalFormatterOptions, GroupingStrategy};
use icu_locale_core::Locale;
use icu_plurals::PluralRules;
use sapphire_strings::{Entry, Pattern, StringTable};

use crate::evaluate::Walk;
use crate::{Arg, LocStats, LocaleSettings};

/// A locale that can't be switched to, or a table that can't be held.
#[derive(Debug, thiserror::Error)]
pub enum UnknownLocale {
    #[error("no table holds locale `{0}`")]
    NoTable(String),
    #[error("locale `{locale}` falls back to `{fallback}`, and no table holds that")]
    NoFallback { locale: String, fallback: String },
    #[error("`{0}` isn't a locale tag")]
    BadTag(String),
    #[error("no plural rules or number format for locale `{0}`")]
    NoData(String),
}

/// One table, with what its locale's rules need.
pub(crate) struct Loaded {
    table: StringTable,
    /// Made when the mode's on, so a shipped game never holds a second copy.
    pseudo: Option<StringTable>,
    pub(crate) plurals: PluralRules,
    pub(crate) grouped: DecimalFormatter,
    pub(crate) ungrouped: DecimalFormatter,
}

impl Loaded {
    fn new(table: StringTable, pseudo: bool) -> Result<Self, UnknownLocale> {
        let no_data = || UnknownLocale::NoData(table.locale.clone());
        let locale =
            Locale::try_from_str(&table.locale).map_err(|_| UnknownLocale::BadTag(table.locale.clone()))?;
        let plurals = PluralRules::try_new_cardinal((&locale).into()).map_err(|_| no_data())?;
        let formatter = |grouping: GroupingStrategy| {
            DecimalFormatter::try_new((&locale).into(), DecimalFormatterOptions::from(grouping))
                .map_err(|_| no_data())
        };
        let grouped = formatter(GroupingStrategy::Auto)?;
        let ungrouped = formatter(GroupingStrategy::Never)?;
        let pseudo = pseudo.then(|| table.pseudolocalized());
        Ok(Self { table, pseudo, plurals, grouped, ungrouped })
    }

    /// The table keys are answered from.
    pub(crate) fn active(&self) -> &StringTable {
        self.pseudo.as_ref().unwrap_or(&self.table)
    }

    /// Selector values are semantic, even while visible text is pseudo-localized.
    pub(crate) fn semantic(&self) -> &StringTable {
        &self.table
    }

    fn heap_bytes(&self) -> u64 {
        (self.table.heap_bytes() + self.pseudo.as_ref().map_or(0, StringTable::heap_bytes)) as u64
    }
}

/// What's counted through a shared reference, since a lookup takes one.
#[derive(Default)]
pub(crate) struct Counters {
    lookups: AtomicU64,
    pub(crate) unknown_keys: AtomicU64,
    pub(crate) missing_args: AtomicU64,
    not_plain: AtomicU64,
    pub(crate) overruns: AtomicU64,
    pub(crate) bad_numbers: AtomicU64,
}

pub(crate) fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Answers keys in the current text locale. See the [crate docs](crate).
pub struct Localizer {
    tables: Vec<Loaded>,
    /// Indices into `tables`: the text locale's, then its fallback chain's. Empty until its tables are
    /// all here.
    chain: Vec<usize>,
    settings: LocaleSettings,
    pseudo: bool,
    generation: u64,
    switches: u64,
    counters: Counters,
}

impl Localizer {
    /// A localizer with no tables, which answers every key with itself until [`Self::install`] brings
    /// the tables `settings` names.
    pub fn new(settings: LocaleSettings) -> Self {
        Self {
            tables: Vec::new(),
            chain: Vec::new(),
            settings,
            pseudo: false,
            generation: 0,
            switches: 0,
            counters: Counters::default(),
        }
    }

    /// Holds `table`, replacing any of its locale, as hot reload does. Whatever shows text resolves
    /// again ([`Self::generation`]). Refused if its tag isn't a locale's, and then nothing changes.
    pub fn install(&mut self, table: StringTable) -> Result<(), UnknownLocale> {
        let loaded = Loaded::new(table, self.pseudo)?;
        let locale = loaded.table.locale.as_str();
        match self.tables.iter().position(|held| held.table.locale == locale) {
            Some(index) => self.tables[index] = loaded,
            None => self.tables.push(loaded),
        }
        // A chain that can't be built yet is fine here: its other tables may be on their way.
        self.chain = self.chain_of(&self.settings.text).unwrap_or_default();
        self.generation += 1;
        Ok(())
    }

    /// The tables' locales, for a language menu.
    pub fn locales(&self) -> impl Iterator<Item = &str> {
        self.tables.iter().map(|loaded| loaded.table.locale.as_str())
    }

    fn chain_of(&self, locale: &str) -> Result<Vec<usize>, UnknownLocale> {
        let index_of = |wanted: &str| self.tables.iter().position(|held| held.table.locale == wanted);
        let head = index_of(locale).ok_or_else(|| UnknownLocale::NoTable(locale.to_owned()))?;
        let mut chain = vec![head];
        for fallback in &self.tables[head].table.fallback {
            let next = index_of(fallback).ok_or_else(|| UnknownLocale::NoFallback {
                locale: locale.to_owned(),
                fallback: fallback.clone(),
            })?;
            chain.push(next);
        }
        Ok(chain)
    }

    /// Switches the text language, with no restart: the next key answers in it. The voice language
    /// stays. If the locale's tables aren't all here, it's refused and nothing changes.
    pub fn set_text_locale(&mut self, locale: &str) -> Result<(), UnknownLocale> {
        self.chain = self.chain_of(locale)?;
        locale.clone_into(&mut self.settings.text);
        self.generation += 1;
        self.switches += 1;
        Ok(())
    }

    /// Sets the voice language. Text doesn't move, so nothing needs to resolve again.
    pub fn set_voice_locale(&mut self, locale: &str) {
        locale.clone_into(&mut self.settings.voice);
    }

    /// Takes on both of a player's languages at once, as loading settings does. If the text language
    /// is refused, neither changes.
    pub fn apply(&mut self, settings: &LocaleSettings) -> Result<(), UnknownLocale> {
        self.set_text_locale(&settings.text)?;
        self.set_voice_locale(&settings.voice);
        Ok(())
    }

    /// The languages in force, to save.
    pub fn settings(&self) -> &LocaleSettings {
        &self.settings
    }

    /// The text language's tag.
    pub fn text_locale(&self) -> &str {
        &self.settings.text
    }

    /// The voice language's tag.
    pub fn voice_locale(&self) -> &str {
        &self.settings.voice
    }

    /// Goes up whenever a key might answer differently: a switch, a table replaced, the pseudo mode.
    /// Whatever holds resolved text compares it, and resolves again when it moved.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Turns pseudo-localization on or off, at any time. Every locale's messages come back accented,
    /// longer and framed (`sapphire_strings::pseudolocalize`), with that locale's plurals and numbers.
    /// A string given to layout in this mode that fails [`is_framed`](crate::is_framed) isn't in the
    /// tables. It costs a second copy of each table while it's on, and no cooked data.
    pub fn set_pseudo(&mut self, on: bool) {
        if self.pseudo == on {
            return;
        }
        self.pseudo = on;
        for loaded in &mut self.tables {
            loaded.pseudo = on.then(|| loaded.table.pseudolocalized());
        }
        self.generation += 1;
    }

    /// Whether [`Self::set_pseudo`] is on.
    pub fn pseudo(&self) -> bool {
        self.pseudo
    }

    /// The font family names the text language shows `role` in, first choice first: a table without
    /// the role falls back along its chain, as a key does. Names only, for `sapphire_text`'s family
    /// lists; whether a cooked font answers to one is the font cook's check.
    pub fn font_chain(&self, role: &str) -> Option<&[String]> {
        self.chain.iter().find_map(|&index| self.tables[index].table.font_chain(role))
    }

    /// What's in force and what's gone wrong (rule R9).
    pub fn stats(&self) -> LocStats {
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let counters = &self.counters;
        LocStats {
            lookups: read(&counters.lookups),
            unknown_keys: read(&counters.unknown_keys),
            missing_args: read(&counters.missing_args),
            not_plain: read(&counters.not_plain),
            overruns: read(&counters.overruns),
            bad_numbers: read(&counters.bad_numbers),
            switches: self.switches,
            tables: self.tables.len() as u64,
            table_bytes: self.tables.iter().map(Loaded::heap_bytes).sum(),
        }
    }

    /// `key`'s entry, looking along the chain from `from` on: which link of the chain has it, and it.
    pub(crate) fn find(&self, from: usize, key: &str) -> Option<(usize, &Entry)> {
        self.find_in(from, key, Loaded::active)
    }

    pub(crate) fn find_semantic(&self, from: usize, key: &str) -> Option<(usize, &Entry)> {
        self.find_in(from, key, Loaded::semantic)
    }

    fn find_in(
        &self,
        from: usize,
        key: &str,
        table: impl Fn(&Loaded) -> &StringTable,
    ) -> Option<(usize, &Entry)> {
        let links = self.chain.get(from..)?;
        links.iter().enumerate().find_map(|(link, &index)| {
            let entry = table(&self.tables[index]).entry(key)?;
            Some((from + link, entry))
        })
    }

    /// The table of a link of the chain.
    pub(crate) fn link(&self, link: usize) -> Option<&Loaded> {
        self.tables.get(*self.chain.get(link)?)
    }

    pub(crate) fn counters(&self) -> &Counters {
        &self.counters
    }

    /// A message with no placeables, as it's held: no allocation. Other keys come back as themselves,
    /// and a stat counts them. A message with placeables is one of those: it wants [`Self::resolve`].
    pub fn text<'a>(&'a self, key: &'a str) -> &'a str {
        count(&self.counters.lookups);
        let Some((link, entry)) = self.find(0, key) else {
            count(&self.counters.unknown_keys);
            return key;
        };
        match (entry.pattern, self.link(link)) {
            (Pattern::Plain(span), Some(loaded)) => loaded.active().str(span),
            _ => {
                count(&self.counters.not_plain);
                key
            }
        }
    }

    /// Appends `key`'s message to `out`, worked out with `args`, or the key itself if no table has it.
    pub fn format(&self, key: &str, args: &[Arg<'_>], out: &mut String) {
        count(&self.counters.lookups);
        let Some((link, entry)) = self.find(0, key) else {
            count(&self.counters.unknown_keys);
            out.push_str(key);
            return;
        };
        Walk::new(self, args, out).message(link, entry.pattern);
    }

    /// `key`'s message, by whichever [`Self::text`] or [`Self::format`] it needs: a plain one as it's
    /// held, leaving `scratch` alone, and any other written over `scratch`. It's what a bound UI node
    /// calls: it doesn't have to know which it has, and only what changes allocates.
    pub fn resolve<'a>(&'a self, key: &'a str, args: &[Arg<'_>], scratch: &'a mut String) -> &'a str {
        count(&self.counters.lookups);
        let Some((link, entry)) = self.find(0, key) else {
            count(&self.counters.unknown_keys);
            return key;
        };
        if let (Pattern::Plain(span), Some(loaded)) = (entry.pattern, self.link(link)) {
            return loaded.active().str(span);
        }
        scratch.clear();
        Walk::new(self, args, &mut *scratch).message(link, entry.pattern);
        scratch
    }
}
