//! Values read through reflection paths, kept to compare with next frame's.

use sapphire_assets::ui::{UiColor, UiShadow, UiText};
use sapphire_loc::Value;
use sapphire_reflect::PartialReflect;
use sapphire_reflect::bevy_reflect::{ParsedPath, ReflectPath, ReflectRef};

use super::UiTreeError;

/// What a path found. Borrowed, so reading costs no allocation whatever the value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Read<'a> {
    Integer(i64),
    Float(f64),
    Bool(bool),
    Text(&'a str),
    /// The model's type has this path, and its value isn't there right now.
    Absent,
}

impl Read<'_> {
    /// As a `Shown`: what isn't there isn't shown.
    pub(super) fn as_bool(self) -> bool {
        matches!(self, Self::Bool(true))
    }

    /// As an opacity or a fill: 0 to 1, and what isn't there is 0.
    pub(super) fn as_share(self) -> f32 {
        let share = match self {
            Self::Integer(value) => value as f32,
            Self::Float(value) => value as f32,
            _ => 0.0,
        };
        if share.is_nan() { 0.0 } else { share.clamp(0.0, 1.0) }
    }
}

/// What `path` holds in `model`.
pub(super) fn read<'m>(path: &ParsedPath, model: &'m dyn PartialReflect) -> Read<'m> {
    let Ok(value) = path.reflect_element(model) else { return Read::Absent };
    macro_rules! numbers {
        ($variant:ident as $wide:ty: $($number:ty),*) => {
            $(if let Some(&number) = value.try_downcast_ref::<$number>() {
                return Read::$variant(number as $wide);
            })*
        };
    }
    numbers!(Float as f64: f32, f64);
    numbers!(Integer as i64: i32, u32, i64, u64, usize, isize, u8, i8, u16, i16);
    if let Some(&flag) = value.try_downcast_ref::<bool>() {
        return Read::Bool(flag);
    }
    if let Some(text) = value.try_downcast_ref::<String>() {
        return Read::Text(text);
    }
    if let Some(&text) = value.try_downcast_ref::<&'static str>() {
        return Read::Text(text);
    }
    if let ReflectRef::Enum(variant) = value.reflect_ref() {
        return Read::Text(variant.variant_name());
    }
    Read::Absent
}

/// A message's variable, as last read.
pub(super) struct BoundArg {
    pub(super) name: Box<str>,
    pub(super) source: Box<str>,
    path: ParsedPath,
    pub(super) failed: bool,
    held: Held,
    /// A text's value, whose capacity stays for the next.
    text: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Held {
    Integer(i64),
    /// By its bits, so a NaN doesn't read as a change every frame.
    Float(u64),
    Text,
}

impl BoundArg {
    /// Whether what `model` holds differs from what this last read, keeping it if so.
    fn read(&mut self, model: &dyn PartialReflect) -> bool {
        if self.failed {
            return false;
        }
        let text = |held: &mut Held, buffer: &mut String, value: &str| {
            if *held == Held::Text && buffer == value {
                return false;
            }
            *held = Held::Text;
            buffer.clear();
            buffer.push_str(value);
            true
        };
        let number = |held: &mut Held, value: Held| {
            let changed = *held != value;
            *held = value;
            changed
        };
        match read(&self.path, model) {
            Read::Integer(value) => number(&mut self.held, Held::Integer(value)),
            Read::Float(value) => number(&mut self.held, Held::Float(value.to_bits())),
            Read::Bool(value) => text(&mut self.held, &mut self.text, if value { "true" } else { "false" }),
            Read::Text(value) => text(&mut self.held, &mut self.text, value),
            Read::Absent => text(&mut self.held, &mut self.text, ""),
        }
    }

    /// A path that doesn't resolve stands where its value would, and one that does starts over.
    pub(super) fn set_failed(&mut self, failed: bool) {
        self.failed = failed;
        self.held = Held::Text;
        self.text.clear();
        if failed {
            self.text.push('{');
            self.text.push_str(&self.source);
            self.text.push('}');
        }
    }

    pub(super) fn value(&self) -> Value<'_> {
        match self.held {
            Held::Integer(value) => Value::Integer(value),
            Held::Float(bits) => Value::Float(f64::from_bits(bits)),
            Held::Text => Value::Text(&self.text),
        }
    }
}

/// Where a key chosen as the game runs comes from.
pub(super) struct BoundKey {
    pub(super) source: Box<str>,
    path: ParsedPath,
    prefix: Box<str>,
    pub(super) failed: bool,
    /// What the path last read: a key, or nothing.
    chosen: String,
    /// `chosen` isn't under the prefix.
    wrong: bool,
    /// `chosen` has been judged since it was read ([`BoundText::judge_chosen`]).
    pub(super) told: bool,
}

/// A node's text as it runs.
pub(super) struct BoundText {
    pub(super) key: Box<str>,
    pub(super) key_from: Option<BoundKey>,
    pub(super) shadow: Option<UiShadow>,
    pub(super) style: u16,
    pub(super) color: UiColor,
    pub(super) wrap: bool,
    pub(super) args: Vec<BoundArg>,
    /// What it last resolved to.
    pub(super) resolved: String,
    /// Its language or a value moved since then.
    pub(super) stale: bool,
    /// Its role's family list in `UiFonts`, or none if no chain answers.
    pub(super) family: Option<u16>,
}

impl BoundText {
    pub(super) fn new(text: &UiText, style: u16) -> Result<Self, UiTreeError> {
        let args = text
            .args
            .iter()
            .map(|arg| {
                Ok(BoundArg {
                    name: arg.name.as_str().into(),
                    source: arg.path.as_str().into(),
                    path: ParsedPath::parse(&arg.path)
                        .map_err(|error| UiTreeError::Layout(error.to_string()))?,
                    failed: false,
                    held: Held::Text,
                    text: String::new(),
                })
            })
            .collect::<Result<Vec<_>, UiTreeError>>()?;
        let key_from = text
            .key_from
            .as_ref()
            .map(|from| {
                Ok::<_, UiTreeError>(BoundKey {
                    source: from.path.as_str().into(),
                    path: ParsedPath::parse(&from.path)
                        .map_err(|error| UiTreeError::Layout(error.to_string()))?,
                    prefix: from.prefix.as_str().into(),
                    failed: false,
                    chosen: String::new(),
                    wrong: false,
                    told: false,
                })
            })
            .transpose()?;
        Ok(Self {
            key: text.key.as_str().into(),
            key_from,
            shadow: text.shadow,
            style,
            color: text.color,
            wrap: text.wrap,
            args,
            resolved: String::new(),
            stale: true,
            family: None,
        })
    }

    /// Reads its variables, and how many moved.
    pub(super) fn read(&mut self, model: &dyn PartialReflect) -> u32 {
        let mut moved = 0;
        for arg in &mut self.args {
            moved += u32::from(arg.read(model));
        }
        if let Some(from) = &mut self.key_from
            && !from.failed
        {
            let value = match read(&from.path, model) {
                Read::Text(value) => value,
                _ => "",
            };
            if from.chosen != value {
                from.chosen.clear();
                from.chosen.push_str(value);
                from.wrong = !value.is_empty() && !value.starts_with(&*from.prefix);
                from.told = false;
                moved += 1;
            }
        }
        self.stale |= moved > 0;
        moved
    }

    /// The key it shows now: a chosen one under its prefix, or else its own.
    pub(super) fn current_key(&self) -> &str {
        match &self.key_from {
            Some(from) if !from.failed && !from.wrong && !from.chosen.is_empty() => &from.chosen,
            _ => &self.key,
        }
    }

    /// The verdict on a chosen key not judged since it was read: `Some` once each time the path reads
    /// something new, holding what to say if it isn't under the prefix. A cook held only the keys under
    /// it to the fonts and the variables, so one outside may not show properly.
    pub(super) fn judge_chosen(&mut self) -> Option<Option<String>> {
        let from = self.key_from.as_mut().filter(|from| !from.failed && !from.told)?;
        from.told = true;
        Some(from.wrong.then(|| {
            format!("`{}` read `{}`, which doesn't start with `{}`", from.source, from.chosen, from.prefix)
        }))
    }

    /// Whether anything about it has a problem now: a path that doesn't read, a chosen key outside
    /// the prefix, or a role no font chain answers.
    pub(super) fn has_problem(&self) -> bool {
        self.args.iter().any(|arg| arg.failed)
            || self.key_from.as_ref().is_some_and(|from| from.failed || from.wrong)
            || self.family.is_none()
    }
}
