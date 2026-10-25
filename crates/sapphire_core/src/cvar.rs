//! Console variables (spec §5.3): typed settings that any crate declares as statics and that the
//! dev console (and later config files, live link and the command line) reads and changes by
//! name.
//!
//! ```
//! use sapphire_core::cvar::{Cvar, CvarRegistry};
//!
//! static BLOOM: Cvar<bool> = Cvar::new("r.bloom", true, "Bloom on or off.");
//! static EXPOSURE: Cvar<f32> =
//!     Cvar::with_range("r.exposure", 0.0, -8.0, 8.0, "Exposure offset in stops.");
//!
//! // The app registers every crate's cvars once at startup.
//! let mut cvars = CvarRegistry::new();
//! cvars.register_all(&[&BLOOM, &EXPOSURE]).unwrap();
//!
//! // A console sets them by name, from text.
//! cvars.set("r.exposure", "1.5").unwrap();
//! assert_eq!(EXPOSURE.get(), 1.5);
//! assert!(cvars.set("r.exposure", "20").is_err(), "out of range");
//! ```
//!
//! # Design
//!
//! - **Declared where they are read.** A cvar is a `static` in the crate that uses it, so code
//!   reads it directly ([`Cvar::get`]: one relaxed atomic load) with no lookup and no dependency
//!   on the dev-only crates that change it. This is the "thin facade" of spec §5.20: a runtime
//!   crate declares cvars against `sapphire_core`, and only development builds link a console.
//! - **Registered explicitly.** There is no global or link-time registration (`inventory`); each
//!   crate exports its cvars and the app adds them to a [`CvarRegistry`], as with reflected types
//!   (`sapphire_reflect`).
//! - **Typed.** Values are [`CvarType`]s: `bool`, `i32`, `u32`, `f32`, or an enum of named choices
//!   that implements the trait. [`CvarKind`] and [`CvarValue`] describe them to UIs without
//!   generics, so a console can edit any cvar with the right widget.
//! - **Shipping builds** have no console or live link, so nothing writes a cvar there and every
//!   read returns its default: in effect a read-only value, at the cost of one atomic load.
//!   Turning them into compile-time constants is later work, should a profile ever ask for it.
//!
//! Cvars are independent settings: a write is visible to every thread soon after, with no
//! ordering between different cvars. Code that must react to a change (rebuilding a pipeline,
//! say) compares the value with the one it last used.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Set in a cvar's stored word once the cvar has been set; the low 32 bits hold the value's bits.
const SET: u64 = 1 << 32;

/// A typed console variable. Declare it as a `static`; see the [module docs](self).
pub struct Cvar<T> {
    name: &'static str,
    help: &'static str,
    default: T,
    range: Option<(T, T)>,
    /// 0 while unset (the value is `default`), else [`SET`] | the value's bits.
    value: AtomicU64,
}

impl<T: CvarType> Cvar<T> {
    /// A cvar starting at `default`. `name` is lowercase dot-separated parts, such as
    /// `r.bloom_levels` (checked when it is registered).
    pub const fn new(name: &'static str, default: T, help: &'static str) -> Self {
        Self { name, help, default, range: None, value: AtomicU64::new(0) }
    }

    /// A cvar limited to `min..=max`: values outside are rejected, and UIs show a slider.
    pub const fn with_range(name: &'static str, default: T, min: T, max: T, help: &'static str) -> Self {
        Self { name, help, default, range: Some((min, max)), value: AtomicU64::new(0) }
    }

    /// The current value.
    #[inline]
    pub fn get(&self) -> T {
        let word = self.value.load(Relaxed);
        if word & SET == 0 { self.default } else { T::from_bits(word as u32) }
    }

    /// Sets the value, if it is valid and in range.
    pub fn set(&self, value: T) -> Result<(), CvarError> {
        if !value.is_valid() {
            return Err(CvarError::Invalid { name: self.name, value: value.to_value() });
        }
        if let Some((min, max)) = self.range
            && !(min <= value && value <= max)
        {
            return Err(CvarError::OutOfRange {
                name: self.name,
                value: value.to_value(),
                min: min.to_value(),
                max: max.to_value(),
            });
        }
        self.value.store(SET | u64::from(value.to_bits()), Relaxed);
        Ok(())
    }

    /// Back to the default.
    pub fn reset(&self) {
        self.value.store(0, Relaxed);
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn help(&self) -> &'static str {
        self.help
    }

    pub fn default(&self) -> T {
        self.default
    }

    /// `(min, max)`, inclusive, for a cvar made [`with_range`](Self::with_range).
    pub fn range(&self) -> Option<(T, T)> {
        self.range
    }
}

impl<T: CvarType> fmt::Debug for Cvar<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cvar({} = {})", self.name, self.get().to_value())
    }
}

/// What a cvar holds, for UIs that edit cvars without knowing their Rust types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CvarKind {
    Bool,
    Int,
    UInt,
    Float,
    /// One of these names, stored as its index.
    Choice(&'static [&'static str]),
}

impl CvarKind {
    /// What text the cvar accepts, for messages.
    pub fn expected(self) -> &'static str {
        match self {
            Self::Bool => "true or false",
            Self::Int => "an integer",
            Self::UInt => "a non-negative integer",
            Self::Float => "a number",
            Self::Choice(_) => "one of its choices",
        }
    }
}

/// A cvar's value without its Rust type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CvarValue {
    Bool(bool),
    Int(i32),
    UInt(u32),
    Float(f32),
    /// The chosen name's index, and the names.
    Choice {
        index: u32,
        names: &'static [&'static str],
    },
}

impl fmt::Display for CvarValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Bool(value) => write!(f, "{value}"),
            Self::Int(value) => write!(f, "{value}"),
            Self::UInt(value) => write!(f, "{value}"),
            // Rust prints the shortest text that reads back as the same f32.
            Self::Float(value) => write!(f, "{value}"),
            Self::Choice { index, names } => match names.get(index as usize) {
                Some(name) => f.write_str(name),
                None => write!(f, "#{index}"),
            },
        }
    }
}

/// A type a [`Cvar`] can hold. Implemented for `bool`, `i32`, `u32` and `f32`; implement it for
/// an enum to make a choice cvar ([`CvarKind::Choice`], stored as the variant's index).
///
/// Values live in 32 bits, so a cvar is one lock-free atomic.
pub trait CvarType: Copy + PartialOrd + Send + Sync + 'static {
    const KIND: CvarKind;

    fn to_bits(self) -> u32;
    /// Only ever called with bits from [`Self::to_bits`].
    fn from_bits(bits: u32) -> Self;
    /// Reads console text, or `None` if it isn't a value of this type.
    fn parse(text: &str) -> Option<Self>;
    fn to_value(self) -> CvarValue;
    /// The value of `value`, or `None` if it is another kind.
    fn from_value(value: CvarValue) -> Option<Self>;

    /// Whether the value may be stored at all, whatever the range (floats must be finite).
    fn is_valid(self) -> bool {
        true
    }
}

impl CvarType for bool {
    const KIND: CvarKind = CvarKind::Bool;

    fn to_bits(self) -> u32 {
        u32::from(self)
    }

    fn from_bits(bits: u32) -> Self {
        bits != 0
    }

    fn parse(text: &str) -> Option<Self> {
        const TRUE: [&str; 4] = ["true", "1", "on", "yes"];
        const FALSE: [&str; 4] = ["false", "0", "off", "no"];
        let matches = |words: &[&str]| words.iter().any(|word| word.eq_ignore_ascii_case(text));
        if matches(&TRUE) {
            Some(true)
        } else if matches(&FALSE) {
            Some(false)
        } else {
            None
        }
    }

    fn to_value(self) -> CvarValue {
        CvarValue::Bool(self)
    }

    fn from_value(value: CvarValue) -> Option<Self> {
        match value {
            CvarValue::Bool(value) => Some(value),
            _ => None,
        }
    }
}

impl CvarType for i32 {
    const KIND: CvarKind = CvarKind::Int;

    fn to_bits(self) -> u32 {
        self as u32
    }

    fn from_bits(bits: u32) -> Self {
        bits as i32
    }

    fn parse(text: &str) -> Option<Self> {
        text.parse().ok()
    }

    fn to_value(self) -> CvarValue {
        CvarValue::Int(self)
    }

    fn from_value(value: CvarValue) -> Option<Self> {
        match value {
            CvarValue::Int(value) => Some(value),
            _ => None,
        }
    }
}

impl CvarType for u32 {
    const KIND: CvarKind = CvarKind::UInt;

    fn to_bits(self) -> u32 {
        self
    }

    fn from_bits(bits: u32) -> Self {
        bits
    }

    fn parse(text: &str) -> Option<Self> {
        text.parse().ok()
    }

    fn to_value(self) -> CvarValue {
        CvarValue::UInt(self)
    }

    fn from_value(value: CvarValue) -> Option<Self> {
        match value {
            CvarValue::UInt(value) => Some(value),
            _ => None,
        }
    }
}

impl CvarType for f32 {
    const KIND: CvarKind = CvarKind::Float;

    fn to_bits(self) -> u32 {
        self.to_bits()
    }

    fn from_bits(bits: u32) -> Self {
        f32::from_bits(bits)
    }

    fn parse(text: &str) -> Option<Self> {
        text.parse().ok().filter(|value: &f32| value.is_finite())
    }

    fn to_value(self) -> CvarValue {
        CvarValue::Float(self)
    }

    fn from_value(value: CvarValue) -> Option<Self> {
        match value {
            CvarValue::Float(value) => Some(value),
            _ => None,
        }
    }

    fn is_valid(self) -> bool {
        self.is_finite()
    }
}

/// A [`Cvar`] of any type, as a [`CvarRegistry`] holds it.
pub trait AnyCvar: Sync {
    fn name(&self) -> &'static str;
    fn help(&self) -> &'static str;
    fn kind(&self) -> CvarKind;
    fn value(&self) -> CvarValue;
    fn default_value(&self) -> CvarValue;
    /// `(min, max)`, inclusive, if the cvar has a range.
    fn range(&self) -> Option<(CvarValue, CvarValue)>;
    /// Sets a value of the cvar's own kind.
    fn set_value(&self, value: CvarValue) -> Result<(), CvarError>;
    /// Sets the value from console text.
    fn set_text(&self, text: &str) -> Result<(), CvarError>;
    fn reset(&self);

    fn is_default(&self) -> bool {
        self.value() == self.default_value()
    }
}

impl<T: CvarType> AnyCvar for Cvar<T> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn help(&self) -> &'static str {
        self.help
    }

    fn kind(&self) -> CvarKind {
        T::KIND
    }

    fn value(&self) -> CvarValue {
        self.get().to_value()
    }

    fn default_value(&self) -> CvarValue {
        self.default.to_value()
    }

    fn range(&self) -> Option<(CvarValue, CvarValue)> {
        self.range.map(|(min, max)| (min.to_value(), max.to_value()))
    }

    fn set_value(&self, value: CvarValue) -> Result<(), CvarError> {
        let typed =
            T::from_value(value).ok_or(CvarError::WrongKind { name: self.name, kind: T::KIND, value })?;
        self.set(typed)
    }

    fn set_text(&self, text: &str) -> Result<(), CvarError> {
        let value = T::parse(text.trim()).ok_or_else(|| CvarError::Parse {
            name: self.name,
            kind: T::KIND,
            text: text.to_owned(),
        })?;
        self.set(value)
    }

    fn reset(&self) {
        Cvar::reset(self);
    }
}

impl fmt::Debug for dyn AnyCvar + '_ {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} = {}", self.name(), self.value())
    }
}

/// Why a cvar couldn't be registered, found or set. A failed set leaves the value unchanged.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum CvarError {
    #[error("no cvar is named `{name}`{}", suggestion.map(|s| format!(" (did you mean `{s}`?)")).unwrap_or_default())]
    Unknown { name: String, suggestion: Option<&'static str> },
    #[error("`{name}` expects {}, got `{text}`", kind.expected())]
    Parse { name: &'static str, kind: CvarKind, text: String },
    #[error("`{name}` expects {}, got {value:?}", kind.expected())]
    WrongKind { name: &'static str, kind: CvarKind, value: CvarValue },
    #[error("`{name}` can't be {value}")]
    Invalid { name: &'static str, value: CvarValue },
    #[error("`{name}` must be within {min}..={max}, got {value}")]
    OutOfRange { name: &'static str, value: CvarValue, min: CvarValue, max: CvarValue },
    #[error("a cvar named `{0}` is already registered")]
    Duplicate(&'static str),
    #[error("cvar name `{0}` is invalid: use dot-separated parts of lowercase letters, digits and `_`")]
    InvalidName(&'static str),
}

/// Every cvar an app knows about, by name. See the [module docs](self).
#[derive(Debug, Default)]
pub struct CvarRegistry {
    /// Sorted by name.
    cvars: Vec<&'static dyn AnyCvar>,
}

impl CvarRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a cvar. Fails for an invalid name or one already registered.
    pub fn register(&mut self, cvar: &'static dyn AnyCvar) -> Result<(), CvarError> {
        let name = cvar.name();
        if !valid_name(name) {
            return Err(CvarError::InvalidName(name));
        }
        match self.position(name) {
            Ok(_) => Err(CvarError::Duplicate(name)),
            Err(index) => {
                self.cvars.insert(index, cvar);
                Ok(())
            }
        }
    }

    /// Adds every cvar in `cvars`, stopping at the first that fails.
    pub fn register_all(&mut self, cvars: &[&'static dyn AnyCvar]) -> Result<(), CvarError> {
        cvars.iter().try_for_each(|&cvar| self.register(cvar))
    }

    pub fn get(&self, name: &str) -> Option<&'static dyn AnyCvar> {
        self.position(name).ok().map(|index| self.cvars[index])
    }

    /// [`Self::get`], or an [`CvarError::Unknown`] that suggests a near name.
    pub fn find(&self, name: &str) -> Result<&'static dyn AnyCvar, CvarError> {
        self.get(name)
            .ok_or_else(|| CvarError::Unknown { name: name.to_owned(), suggestion: self.suggest(name) })
    }

    /// Sets the cvar `name` from console text.
    pub fn set(&self, name: &str, text: &str) -> Result<(), CvarError> {
        self.find(name)?.set_text(text)
    }

    /// Every cvar, by name.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &'static dyn AnyCvar> + '_ {
        self.cvars.iter().copied()
    }

    /// The cvars whose names start with `prefix`, by name.
    pub fn with_prefix<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'static dyn AnyCvar> + 'a {
        let start = self.cvars.partition_point(|cvar| cvar.name() < prefix);
        self.cvars[start..].iter().copied().take_while(move |cvar| cvar.name().starts_with(prefix))
    }

    /// Resets every cvar to its default.
    pub fn reset_all(&self) {
        for cvar in &self.cvars {
            cvar.reset();
        }
    }

    pub fn len(&self) -> usize {
        self.cvars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cvars.is_empty()
    }

    /// The registered name closest to a mistyped `name`: within two edits, or the only name
    /// that starts with it.
    pub fn suggest(&self, name: &str) -> Option<&'static str> {
        let nearest = self
            .cvars
            .iter()
            .map(|cvar| (edit_distance(name, cvar.name()), cvar.name()))
            .filter(|&(distance, _)| distance <= 2)
            .min_by_key(|&(distance, _)| distance)
            .map(|(_, name)| name);
        nearest.or_else(|| {
            let mut prefixed = self.with_prefix(name);
            prefixed.next().filter(|_| prefixed.next().is_none()).map(|cvar| cvar.name())
        })
    }

    fn position(&self, name: &str) -> Result<usize, usize> {
        self.cvars.binary_search_by(|cvar| cvar.name().cmp(name))
    }
}

/// Dot-separated, non-empty parts of `a-z`, `0-9` and `_`.
fn valid_name(name: &str) -> bool {
    name.split('.').all(|part| {
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    })
}

/// Levenshtein distance over bytes; cvar names are short ASCII.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1).min(row[j] + 1).min(diagonal + usize::from(ca != cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    static FLAG: Cvar<bool> = Cvar::new("test.flag", true, "A flag.");
    static COUNT: Cvar<i32> = Cvar::with_range("test.count", 3, -5, 5, "A count.");
    static LEVELS: Cvar<u32> = Cvar::new("test.levels", 5, "Levels.");
    static SCALE: Cvar<f32> = Cvar::with_range("test.scale", 1.0, 0.25, 4.0, "A scale.");

    /// A choice cvar's type, as a renderer debug view would be.
    #[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
    enum View {
        Lit,
        Normals,
        Overdraw,
    }

    impl CvarType for View {
        const KIND: CvarKind = CvarKind::Choice(&["lit", "normals", "overdraw"]);

        fn to_bits(self) -> u32 {
            self as u32
        }

        fn from_bits(bits: u32) -> Self {
            [Self::Lit, Self::Normals, Self::Overdraw][bits as usize]
        }

        fn parse(text: &str) -> Option<Self> {
            let CvarKind::Choice(names) = Self::KIND else { unreachable!() };
            names
                .iter()
                .position(|name| name.eq_ignore_ascii_case(text))
                .map(|index| Self::from_bits(index as u32))
        }

        fn to_value(self) -> CvarValue {
            let CvarKind::Choice(names) = Self::KIND else { unreachable!() };
            CvarValue::Choice { index: self as u32, names }
        }

        fn from_value(value: CvarValue) -> Option<Self> {
            match value {
                CvarValue::Choice { index, .. } if index < 3 => Some(Self::from_bits(index)),
                _ => None,
            }
        }
    }

    static VIEW: Cvar<View> = Cvar::new("test.view", View::Lit, "What the renderer shows.");

    #[test]
    fn typed_values_start_at_their_defaults_and_reset() {
        let local = Cvar::new("local.value", -2_i32, "");
        assert_eq!(local.get(), -2);
        local.set(7).unwrap();
        assert_eq!(local.get(), 7);
        local.set(-2).unwrap();
        assert!(AnyCvar::is_default(&local), "set back to the default value");
        local.set(i32::MIN).unwrap();
        assert_eq!(local.get(), i32::MIN, "every bit pattern round-trips");
        local.reset();
        assert_eq!(local.get(), -2);

        let float = Cvar::new("local.float", 0.5_f32, "");
        float.set(-0.0).unwrap();
        assert_eq!(float.get().to_bits(), (-0.0_f32).to_bits());
    }

    #[test]
    fn ranges_and_invalid_values_are_rejected_without_a_change() {
        let count = Cvar::with_range("local.count", 0, -5, 5, "");
        assert_eq!(
            count.set(6),
            Err(CvarError::OutOfRange {
                name: "local.count",
                value: CvarValue::Int(6),
                min: CvarValue::Int(-5),
                max: CvarValue::Int(5)
            })
        );
        count.set(5).unwrap();
        assert_eq!(count.get(), 5, "bounds are inclusive");

        let scale = Cvar::new("local.scale", 1.0_f32, "");
        assert!(matches!(scale.set(f32::NAN), Err(CvarError::Invalid { .. })));
        assert!(scale.set(f32::INFINITY).is_err());
        assert_eq!(scale.get(), 1.0);
    }

    #[test]
    fn text_parses_per_kind() {
        for (text, value) in [("TRUE", true), ("on", true), ("1", true), ("no", false), ("Off", false)] {
            assert_eq!(bool::parse(text), Some(value), "{text}");
        }
        assert_eq!(bool::parse("maybe"), None);
        assert_eq!(i32::parse("-12"), Some(-12));
        assert_eq!(i32::parse("1.5"), None);
        assert_eq!(u32::parse("-1"), None);
        assert_eq!(f32::parse("2.5e-1"), Some(0.25));
        assert_eq!(f32::parse("nan"), None);
        assert_eq!(f32::parse("inf"), None);
        assert_eq!(View::parse("Normals"), Some(View::Normals));
    }

    #[test]
    fn values_print_as_the_console_reads_them() {
        let printed = |value: CvarValue| value.to_string();
        assert_eq!(printed(CvarValue::Float(0.25)), "0.25");
        assert_eq!(printed(CvarValue::Float(1.0)), "1");
        assert_eq!(printed(CvarValue::Bool(false)), "false");
        assert_eq!(printed(View::Overdraw.to_value()), "overdraw");
        for value in [0.1_f32, 1.0 / 3.0, 16_777_217.0, -1e-7] {
            assert_eq!(f32::parse(&printed(CvarValue::Float(value))), Some(value), "{value} round-trips");
        }
    }

    fn registry() -> CvarRegistry {
        let mut registry = CvarRegistry::new();
        registry.register_all(&[&SCALE, &FLAG, &COUNT, &LEVELS, &VIEW]).unwrap();
        registry
    }

    #[test]
    fn the_registry_sets_by_name_and_lists_in_order() {
        let registry = registry();
        let names: Vec<&str> = registry.iter().map(|cvar| cvar.name()).collect();
        assert_eq!(names, ["test.count", "test.flag", "test.levels", "test.scale", "test.view"]);
        let prefixed: Vec<&str> = registry.with_prefix("test.s").map(|cvar| cvar.name()).collect();
        assert_eq!(prefixed, ["test.scale"]);
        assert_eq!(registry.with_prefix("r.").count(), 0);

        registry.set("test.view", "overdraw").unwrap();
        assert_eq!(VIEW.get(), View::Overdraw);
        registry.set("test.levels", " 3 ").unwrap();
        assert_eq!(LEVELS.get(), 3, "surrounding space is ignored");
        let view = registry.get("test.view").unwrap();
        assert_eq!(view.kind(), CvarKind::Choice(&["lit", "normals", "overdraw"]));
        view.set_value(CvarValue::Choice { index: 1, names: &[] }).unwrap();
        assert_eq!(VIEW.get(), View::Normals);
        registry.reset_all();
        assert_eq!((VIEW.get(), LEVELS.get()), (View::Lit, 5));
    }

    #[test]
    fn mistakes_are_reported_with_the_cvar_unchanged() {
        let registry = registry();
        assert_eq!(
            registry.set("test.scal", "2"),
            Err(CvarError::Unknown { name: "test.scal".into(), suggestion: Some("test.scale") })
        );
        assert_eq!(
            registry.set("test.v", "lit"),
            Err(CvarError::Unknown { name: "test.v".into(), suggestion: Some("test.view") }),
            "the only name with that prefix"
        );
        assert!(matches!(
            registry.find("render.everything"),
            Err(CvarError::Unknown { suggestion: None, .. })
        ));
        let error = registry.set("test.flag", "maybe").unwrap_err();
        assert_eq!(error.to_string(), "`test.flag` expects true or false, got `maybe`");
        assert!(matches!(registry.set("test.scale", "8"), Err(CvarError::OutOfRange { .. })));
        assert!(matches!(
            registry.get("test.flag").unwrap().set_value(CvarValue::Float(1.0)),
            Err(CvarError::WrongKind { .. })
        ));
        assert_eq!((FLAG.get(), SCALE.get()), (true, 1.0));
    }

    #[test]
    fn names_are_checked_and_unique() {
        static BAD: Cvar<bool> = Cvar::new("Render.Bloom", true, "");
        static EMPTY_PART: Cvar<bool> = Cvar::new("r..bloom", true, "");
        static AGAIN: Cvar<u32> = Cvar::new("test.levels", 1, "");
        let mut registry = registry();
        assert_eq!(registry.register(&BAD), Err(CvarError::InvalidName("Render.Bloom")));
        assert_eq!(registry.register(&EMPTY_PART), Err(CvarError::InvalidName("r..bloom")));
        assert_eq!(registry.register(&AGAIN), Err(CvarError::Duplicate("test.levels")));
        assert_eq!(registry.len(), 5);
    }

    #[test]
    fn edit_distance_counts_single_byte_edits() {
        assert_eq!(edit_distance("bloom", "bloom"), 0);
        assert_eq!(edit_distance("blom", "bloom"), 1);
        assert_eq!(edit_distance("bolom", "bloom"), 2);
        assert_eq!(edit_distance("", "abc"), 3);
    }
}
