//! What a screen's preview shows (M3 E5): a `.ui.ron` at one of the three aspects spec §5.17 names, in a language,
//! with stand-in values for what its paths read.
//!
//! A screen's model is the document's model file, which is what a cook checks the paths against ([`UiModel`]): each
//! path a kind of value, rather than a type, which a document of a game's own doesn't name. [`model_of`] builds a
//! value of that shape by reflection, with what a person types in for each. A whole number binds as an integer
//! and any other as a float, as a game's `i32` and `f32` do, so the text a screen formats is what the game's
//! would be. A list holds one item. One of the game's own screens starts on its sample model's values
//! ([`read_from`]), which is what its goldens show. None of it needs a GPU, so its tests run anywhere.

use std::collections::BTreeMap;

use bevy_reflect::list::DynamicList;
use bevy_reflect::structs::DynamicStruct;
use bevy_reflect::{ParsedPath, PartialReflect, ReflectPath};
use sapphire_assets::AssetId;
use sapphire_assets::ui::{UiDocument, UiModel, UiValueKind};

/// The aspects a screen's previewed at: those the game's screens' goldens hold, at their sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Aspect {
    /// 4:3, a tablet's.
    Standard,
    /// 16:9.
    #[default]
    Wide,
    /// 21:9.
    Ultrawide,
}

impl Aspect {
    pub const ALL: [Self; 3] = [Self::Standard, Self::Wide, Self::Ultrawide];

    /// The frame it draws, in pixels: what `game-<screen>-tablet`, `-wide` and `-ultrawide` capture.
    #[must_use]
    pub const fn size(self) -> (u32, u32) {
        match self {
            Self::Standard => (1024, 768),
            Self::Wide => (1280, 720),
            Self::Ultrawide => (1680, 720),
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Standard => "4:3",
            Self::Wide => "16:9",
            Self::Ultrawide => "21:9",
        }
    }
}

/// The languages a cook's tables hold (`content/loc/`).
pub const LANGUAGES: [&str; 2] = ["en", "ja"];

/// What a path reads, typed in.
#[derive(Clone, Debug, PartialEq)]
pub enum StandIn {
    Number(f64),
    Bool(bool),
    Text(String),
}

impl StandIn {
    /// What a path of `kind` reads before anyone types anything.
    #[must_use]
    pub const fn of(kind: UiValueKind) -> Self {
        match kind {
            UiValueKind::Number => Self::Number(0.0),
            UiValueKind::Bool => Self::Bool(false),
            UiValueKind::Text => Self::Text(String::new()),
        }
    }

    fn to_reflect(&self) -> Box<dyn PartialReflect> {
        match self {
            // A screen formats an integer and a float differently, so a whole number is an integer.
            Self::Number(value) if value.fract() == 0.0 && value.abs() < 9.0e15 => Box::new(*value as i64),
            Self::Number(value) => Box::new(*value),
            Self::Bool(value) => Box::new(*value),
            Self::Text(value) => Box::new(value.clone()),
        }
    }
}

/// Each path of `model`, reading what it does before anyone types anything.
#[must_use]
pub fn stand_ins(model: &UiModel) -> Vec<(String, StandIn)> {
    model.fields.iter().map(|field| (field.path.clone(), StandIn::of(field.kind))).collect()
}

/// Sets each of `values` to what `sample` reads at its path, where it reads a number, a flag or a text there: a
/// game's model as it stands, such as `game_ui::menus::MenuScreen::sample_model`.
pub fn read_from(values: &mut [(String, StandIn)], sample: &dyn PartialReflect) {
    for (path, value) in values {
        if let Some(read) = stand_in_at(sample, path) {
            *value = read;
        }
    }
}

/// What `model` reads at `path`, as a stand-in.
fn stand_in_at(model: &dyn PartialReflect, path: &str) -> Option<StandIn> {
    let value = ParsedPath::parse(path).ok()?.reflect_element(model).ok()?;
    macro_rules! numbers {
        ($($number:ty),*) => {
            $(if let Some(&number) = value.try_downcast_ref::<$number>() {
                return Some(StandIn::Number(number as f64));
            })*
        };
    }
    numbers!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize, f32, f64);
    if let Some(&flag) = value.try_downcast_ref::<bool>() {
        return Some(StandIn::Bool(flag));
    }
    value.try_downcast_ref::<String>().map(|text| StandIn::Text(text.clone()))
}

/// A value of the shape `values`' paths say, each reading its stand-in: `a.b` a field of a field, `a[*]` a list of
/// one.
#[must_use]
pub fn model_of(values: &[(String, StandIn)]) -> Box<dyn PartialReflect> {
    let mut root = Shape::Struct(BTreeMap::new());
    for (path, value) in values {
        root.insert(&steps(path), value);
    }
    root.to_reflect()
}

/// One step of a path: a field, then how many lists deep its value is.
fn steps(path: &str) -> Vec<(&str, usize)> {
    path.split('.')
        .map(|step| {
            let name = step.trim_end_matches("[*]");
            (name, (step.len() - name.len()) / "[*]".len())
        })
        .collect()
}

/// A model's shape, built up path by path.
enum Shape {
    Value(StandIn),
    Struct(BTreeMap<String, Shape>),
    List(Box<Shape>),
}

impl Shape {
    fn insert(&mut self, steps: &[(&str, usize)], value: &StandIn) {
        let Self::Struct(fields) = self else { return };
        let Some(((name, lists), rest)) = steps.split_first() else { return };
        let mut at =
            fields.entry((*name).to_owned()).or_insert_with(|| Self::leaf_or_struct(*lists, rest, value));
        for _ in 0..*lists {
            at = match at {
                Self::List(item) => item.as_mut(),
                _ => return,
            };
        }
        if rest.is_empty() {
            *at = Self::Value(value.clone());
        } else {
            at.insert(rest, value);
        }
    }

    /// What a new field holds: lists `lists` deep around a value or a struct.
    fn leaf_or_struct(lists: usize, rest: &[(&str, usize)], value: &StandIn) -> Self {
        let mut shape =
            if rest.is_empty() { Self::Value(value.clone()) } else { Self::Struct(BTreeMap::new()) };
        for _ in 0..lists {
            shape = Self::List(Box::new(shape));
        }
        shape
    }

    fn to_reflect(&self) -> Box<dyn PartialReflect> {
        match self {
            Self::Value(value) => value.to_reflect(),
            Self::Struct(fields) => {
                let mut value = DynamicStruct::default();
                for (name, field) in fields {
                    value.insert_boxed(name.as_str(), field.to_reflect());
                }
                Box::new(value)
            }
            Self::List(item) => {
                let mut list = DynamicList::default();
                list.push_box(item.to_reflect());
                Box::new(list)
            }
        }
    }
}

/// What the viewport shows of a screen being edited.
#[derive(Clone, Copy)]
pub struct ScreenShown<'a> {
    /// The document's ID, which the cook's screen has.
    pub id: AssetId,
    /// As it last read.
    pub document: &'a UiDocument,
    /// What its paths may read, from its model file.
    pub model: &'a UiModel,
    pub values: &'a [(String, StandIn)],
    pub aspect: Aspect,
    pub language: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read<'a>(model: &'a dyn PartialReflect, path: &str) -> &'a dyn PartialReflect {
        let path = ParsedPath::parse(path).expect("a path");
        path.reflect_element(model).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Each path reads its value, nested, in a list or not, with whole numbers as integers.
    #[test]
    fn a_model_reads_what_was_typed_at_each_path() {
        let values = vec![
            ("level".to_owned(), StandIn::Number(90.0)),
            ("health".to_owned(), StandIn::Number(0.65)),
            ("show_badge".to_owned(), StandIn::Bool(true)),
            ("party.leader".to_owned(), StandIn::Text("Ren".to_owned())),
            ("party.size".to_owned(), StandIn::Number(3.0)),
            ("rows[*].label".to_owned(), StandIn::Text("first".to_owned())),
            ("tags[*]".to_owned(), StandIn::Text("new".to_owned())),
        ];
        let model = model_of(&values);
        let model = model.as_partial_reflect();
        assert_eq!(read(model, "level").try_downcast_ref::<i64>(), Some(&90));
        assert_eq!(read(model, "health").try_downcast_ref::<f64>(), Some(&0.65));
        assert_eq!(read(model, "show_badge").try_downcast_ref::<bool>(), Some(&true));
        assert_eq!(read(model, "party.leader").try_downcast_ref::<String>().map(String::as_str), Some("Ren"));
        assert_eq!(read(model, "party.size").try_downcast_ref::<i64>(), Some(&3));
        assert_eq!(
            read(model, "rows[0].label").try_downcast_ref::<String>().map(String::as_str),
            Some("first")
        );
        assert_eq!(read(model, "tags[0]").try_downcast_ref::<String>().map(String::as_str), Some("new"));
    }

    const CHARACTER_MODEL: &str = include_str!("../../../content/ui/character.model.ron");
    const HUD_MODEL: &str = include_str!("../../../content/ui/hud.model.ron");

    /// A model file's paths start as zero, false and empty.
    #[test]
    fn stand_ins_start_empty() {
        let model = UiModel::from_document(HUD_MODEL).expect("reads");
        let values = stand_ins(&model);
        assert_eq!(values.len(), model.fields.len());
        assert!(values.iter().any(|(path, value)| path == "touch" && *value == StandIn::Bool(false)));
        assert!(values.iter().any(|(path, value)| path == "level" && *value == StandIn::Number(0.0)));
        assert!(
            values.iter().any(|(path, value)| path == "player" && *value == StandIn::Text(String::new()))
        );
    }

    /// A sample model's numbers, flags and texts become the stand-ins at their paths, and the model they build
    /// reads them back as the sample's own types would.
    #[test]
    fn a_sample_model_says_what_the_stand_ins_start_at() {
        #[derive(bevy_reflect::Reflect)]
        struct Sample {
            name: String,
            level: i32,
            hp: i32,
            max_hp: i32,
            attack: i32,
            defense: i32,
            crit: i32,
            detail: String,
        }
        let sample = Sample {
            name: "character-name-wanderer".to_owned(),
            level: 12,
            hp: 6480,
            max_hp: 6480,
            attack: 412,
            defense: 268,
            crit: 8,
            detail: "character-detail-attack".to_owned(),
        };
        let mut values = stand_ins(&UiModel::from_document(CHARACTER_MODEL).expect("reads"));
        read_from(&mut values, &sample);
        assert!(values.contains(&("level".to_owned(), StandIn::Number(12.0))), "{values:?}");
        assert!(values.contains(&("name".to_owned(), StandIn::Text("character-name-wanderer".to_owned()))));
        let model = model_of(&values);
        assert_eq!(read(&*model, "hp").try_downcast_ref::<i64>(), Some(&6480));
        assert_eq!(
            read(&*model, "detail").try_downcast_ref::<String>().map(String::as_str),
            Some("character-detail-attack")
        );
    }
}
