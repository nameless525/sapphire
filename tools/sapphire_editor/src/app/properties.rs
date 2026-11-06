//! The inspector any document gets: every property, from the document's reflection alone, so a new kind of
//! document needs nothing here.
//!
//! It's a tree of what the walker finds, kept until the text changes, since walking a large document isn't
//! for every frame. A leaf is edited as RON spelled as it's typed; what a file leaves out shows its default
//! and is written the first time it's changed. An enum picks its variant, an `Option` turns on and off, a list
//! grows by a new entry made from its type and an entry can go. What the document's parser refuses stays
//! refused, and says so. All of it is one document's history, so undo needs nothing from here.

use egui::{Color32, RichText, Ui};
use sapphire_assets::AssetId;
use sapphire_document::write;
use sapphire_document::{AnyDocument, PropertyKind, PropertyPath, Step, walk};

use super::{Editor, Notice, Typed, TypedAt};

/// A place in the tree. One can be several things at once: an `Option` of an enum with fields, say.
#[derive(Debug)]
struct Node {
    step: Step,
    label: String,
    /// The path of what's here, past any `Some`.
    path: PropertyPath,
    /// An `Option`'s path, and whether it's on.
    option: Option<(PropertyPath, bool)>,
    /// An enum's variant, and the others it could be.
    variant: Option<(String, Vec<String>)>,
    /// It's a list, so it can grow, and its entries can go.
    list: bool,
    /// A value: as the file writes it, or its default, if it doesn't.
    leaf: Option<Leaf>,
    children: Vec<Node>,
}

#[derive(Debug)]
struct Leaf {
    literal: String,
    written: bool,
    number: Option<NumberKind>,
}

/// The tree of one document's text.
#[derive(Debug, Default)]
pub(super) struct PropertiesPanel {
    of: Option<(AssetId, String)>,
    nodes: Vec<Node>,
}

/// What a click asked for, done once the tree isn't being read.
enum Asked {
    Toggle(PropertyPath, bool),
    Pick(PropertyPath, String),
    Add(PropertyPath),
    Remove(PropertyPath),
}

impl Editor {
    pub(super) fn properties_panel(&mut self, ui: &mut Ui) {
        let Some(id) = self.current else {
            ui.label("Open a document.");
            return;
        };
        let Some(document) = self.documents.any(id) else { return };
        if let Some(why) = document.failure_message() {
            ui.colored_label(Color32::from_rgb(255, 120, 110), format!("This can't be edited: {why}"));
            return;
        }
        for issue in document.issues().iter().take(8) {
            ui.colored_label(Color32::from_rgb(255, 130, 115), issue);
        }
        let fresh =
            self.properties.of.as_ref().is_some_and(|(of, text)| *of == id && text == document.text());
        if !fresh {
            self.properties.nodes = tree_of(document);
            self.properties.of = Some((id, document.text().to_owned()));
        }
        let nodes = std::mem::take(&mut self.properties.nodes);
        let mut asked = Vec::new();
        egui::ScrollArea::vertical().id_salt("properties").auto_shrink([false, false]).show(ui, |ui| {
            for node in &nodes {
                self.property_node(ui, id, node, false, &mut asked);
            }
        });
        self.properties.nodes = nodes;
        for asked in asked {
            self.do_asked(id, asked);
        }
    }

    fn property_node(
        &mut self,
        ui: &mut Ui,
        id: AssetId,
        node: &Node,
        in_list: bool,
        asked: &mut Vec<Asked>,
    ) {
        if node.children.is_empty() {
            ui.horizontal(|ui| self.node_header(ui, id, node, in_list, asked));
            return;
        }
        let key = ui.make_persistent_id(("property", id, node.path.to_string()));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), key, false)
            .show_header(ui, |ui| self.node_header(ui, id, node, in_list, asked))
            .body(|ui| {
                for child in &node.children {
                    self.property_node(ui, id, child, node.list, asked);
                }
            });
    }

    fn node_header(&mut self, ui: &mut Ui, id: AssetId, node: &Node, in_list: bool, asked: &mut Vec<Asked>) {
        let written = node.leaf.as_ref().is_none_or(|leaf| leaf.written);
        let label = if written {
            RichText::new(&node.label)
        } else {
            RichText::new(&node.label).color(Color32::from_gray(130))
        };
        ui.label(label).on_hover_text(node.path.to_string());
        if let Some((path, on)) = &node.option {
            let mut now = *on;
            if ui.checkbox(&mut now, "").on_hover_text("Whether it's there").changed() {
                asked.push(Asked::Toggle(path.clone(), now));
            }
        }
        if let Some((variant, others)) = &node.variant {
            egui::ComboBox::from_id_salt(("variant", id, node.path.to_string()))
                .selected_text(variant)
                .show_ui(ui, |ui| {
                    for other in others {
                        if ui.selectable_label(other == variant, other).clicked() && other != variant {
                            asked.push(Asked::Pick(node.path.clone(), other.clone()));
                        }
                    }
                });
        }
        if let Some(leaf) = &node.leaf {
            self.leaf_widget(ui, id, node, leaf);
        }
        if node.list && ui.small_button("+").on_hover_text("Add an entry").clicked() {
            asked.push(Asked::Add(node.path.clone()));
        }
        if in_list && ui.small_button("x").on_hover_text("Remove this entry").clicked() {
            let path = node.option.as_ref().map_or(&node.path, |(path, _)| path);
            asked.push(Asked::Remove(path.clone()));
        }
    }

    fn leaf_widget(&mut self, ui: &mut Ui, id: AssetId, node: &Node, leaf: &Leaf) {
        let label = format!("edit {}", node.label);
        if leaf.literal == "true" || leaf.literal == "false" {
            let mut on = leaf.literal == "true";
            if ui.checkbox(&mut on, "").changed() {
                self.set_property(id, &label, &node.path, &on.to_string());
            }
            return;
        }
        if let Some(kind) = leaf.number
            && let Some((response, literal)) = number_widget(ui, &leaf.literal, kind)
        {
            self.number_gesture(&response, &label, id);
            if let Some(literal) = literal {
                self.set_property(id, &label, &node.path, &literal);
            }
            return;
        }
        let field = egui::Id::unique(("property value", id, node.path.to_string()));
        let at = TypedAt::Property { document: id, path: node.path.clone(), label: node.label.clone() };
        self.text_field(ui, field, at, &leaf.literal);
    }

    fn set_property(&mut self, id: AssetId, label: &str, path: &PropertyPath, literal: &str) {
        if !self.take_typed() {
            return;
        }
        let Some(document) = self.documents.any_mut(id) else { return };
        let result = document.set_spelled(label, path, literal);
        self.after_property_edit(id, result.map(|_| ()).map_err(|error| error.to_string()));
    }

    fn do_asked(&mut self, id: AssetId, asked: Asked) {
        if !self.take_typed() {
            return;
        }
        let Some(document) = self.documents.any_mut(id) else { return };
        let result = match &asked {
            Asked::Toggle(path, on) => {
                let (label, variant) = if *on { ("turn on", "Some") } else { ("turn off", "None") };
                document.pick_variant(label, path, variant)
            }
            Asked::Pick(path, name) => document.pick_variant("pick a variant", path, name),
            Asked::Add(path) => document.insert_blank("add an entry", path),
            Asked::Remove(path) => document.remove_entry("remove an entry", path),
        };
        self.after_property_edit(id, result.map(|_| ()).map_err(|error| error.to_string()));
    }

    pub(super) fn after_property_edit(&mut self, id: AssetId, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.notice = None;
                self.link.clear_push();
                if self.moves == Some(id) {
                    self.refresh();
                    self.check_selection();
                }
            }
            Err(text) => self.notice = Some(Notice { text, trouble: true }),
        }
    }

    /// A text field over `literal`, as [`Typed`] holds it: on the screen, and in the text when it lets go.
    pub(super) fn text_field(&mut self, ui: &mut Ui, field: egui::Id, at: TypedAt, literal: &str) {
        let typed = self.typed.as_ref();
        let here = typed.filter(|typed| typed.field == field);
        let waiting_elsewhere = typed.is_some_and(|typed| typed.refused.is_some() && typed.field != field);
        let mut buffer = here.map_or_else(|| literal.to_owned(), |typed| typed.text.clone());
        let mut edit = egui::TextEdit::singleline(&mut buffer).id(field).desired_width(150.0);
        if here.is_some_and(|typed| typed.refused.is_some()) {
            edit = edit.text_color(Color32::from_rgb(255, 120, 110));
        }
        if ui.add_enabled(!waiting_elsewhere, edit).changed() {
            self.keep_typing(Typed { at, field, text: buffer, refused: None }, literal);
        }
    }
}

/// A drag uses the reflected type, never a guess based on how its current value was spelled.
#[derive(Clone, Copy, Debug)]
pub(super) enum NumberKind {
    Integer,
    Float,
}

pub(super) fn number_kind(value: &dyn bevy_reflect::PartialReflect) -> Option<NumberKind> {
    match value.reflect_type_path() {
        "u8" | "u16" | "u32" | "i8" | "i16" | "i32" => Some(NumberKind::Integer),
        "f32" | "f64" => Some(NumberKind::Float),
        // Wider integers and opaque numeric wrappers retain their exact text, including small values.
        _ => None,
    }
}

fn number_literal(value: f64, kind: NumberKind, decimals: usize, exponent: bool) -> String {
    if matches!(kind, NumberKind::Integer) {
        return format!("{}", value.round() as i64);
    }
    let fixed = format!("{value:.decimals$}");
    if !exponent && fixed.parse::<f64>().ok() == Some(value) {
        fixed
    } else if exponent || (value != 0.0 && (value.abs() < 0.0001 || value.abs() >= 1e16)) {
        format!("{value:e}")
    } else {
        value.to_string()
    }
}

pub(super) fn number_widget(
    ui: &mut Ui,
    literal: &str,
    kind: NumberKind,
) -> Option<(egui::Response, Option<String>)> {
    let number = literal.parse::<f64>().ok()?;
    if !number.is_finite() {
        return None;
    }
    let exponent = literal.contains(['e', 'E']);
    let decimals = literal.split_once('.').map_or(2, |(_, fraction)| fraction.len().max(2));
    let mut shown = number;
    let drag = match kind {
        NumberKind::Integer => egui::DragValue::new(&mut shown).speed(0.25).max_decimals(0),
        NumberKind::Float => egui::DragValue::new(&mut shown)
            .speed(0.01)
            .min_decimals(decimals)
            .custom_formatter(move |value, _| number_literal(value, kind, decimals, exponent)),
    };
    let response = ui.add(drag);
    let changed = response.changed().then(|| number_literal(shown, kind, decimals, exponent));
    Some((response, changed))
}

/// The tree a document's properties make.
fn tree_of(document: &dyn AnyDocument) -> Vec<Node> {
    let mut root = Vec::new();
    let Some(value) = document.value_reflect() else { return root };
    let text = document.text();
    for property in document.properties() {
        let steps = property.path.steps();
        let Some(node) = place_of(&mut root, value, &PropertyPath::root(), steps) else { continue };
        let found = walk::get(value, &property.path).ok();
        match property.kind {
            PropertyKind::Option => {
                let on = found.is_some_and(|value| !walk::is_none(value));
                node.option = Some((property.path.clone(), on));
            }
            PropertyKind::List => node.list = true,
            PropertyKind::Enum => {
                let info = found.and_then(|value| value.get_represented_type_info());
                let names = match info {
                    Some(bevy_reflect::TypeInfo::Enum(info)) => {
                        info.iter().map(|variant| variant.name().to_owned()).collect()
                    }
                    _ => Vec::new(),
                };
                let now = found.and_then(walk::variant).unwrap_or_default().to_owned();
                node.variant = Some((now, names));
            }
            PropertyKind::Leaf => {
                let written = property.written.clone();
                let literal = written
                    .as_ref()
                    .map(|span| text[span.clone()].to_owned())
                    .or_else(|| found.and_then(|value| write::canonical(value, document.registry()).ok()));
                if let Some(literal) = literal {
                    node.leaf = Some(Leaf {
                        literal,
                        written: written.is_some(),
                        number: found.and_then(number_kind),
                    });
                }
            }
        }
    }
    root
}

/// The node for `steps`, made if it isn't there. A `Some` is no place of its own: what's inside an `Option`
/// shows at the `Option`.
fn place_of<'t>(
    nodes: &'t mut Vec<Node>,
    document: &dyn bevy_reflect::PartialReflect,
    above: &PropertyPath,
    steps: &[Step],
) -> Option<&'t mut Node> {
    let (first, rest) = steps.split_first()?;
    let label = match first {
        // As the file calls it.
        Step::Field(name) => walk::get(document, above)
            .map_or(name.as_str(), |holder| walk::written_name(holder, name))
            .to_owned(),
        Step::Key(name) => name.clone(),
        Step::Index(at) => at.to_string(),
        Step::Some => return None,
    };
    let at = match nodes.iter().position(|node| node.step == *first) {
        Some(at) => at,
        None => {
            nodes.push(Node {
                step: first.clone(),
                label,
                path: above.join(first.clone()),
                option: None,
                variant: None,
                list: false,
                leaf: None,
                children: Vec::new(),
            });
            nodes.len() - 1
        }
    };
    let node = &mut nodes[at];
    let mut below = above.join(first.clone());
    let rest = match rest.strip_prefix(std::slice::from_ref(&Step::Some)) {
        Some(rest) => {
            below = below.join(Step::Some);
            rest
        }
        None => rest,
    };
    if rest.is_empty() {
        node.path = below;
        Some(node)
    } else {
        place_of(&mut node.children, document, &below, rest)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
    use sapphire_document::{Document, Schema, registry_of};
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
    #[reflect(Serialize, Deserialize)]
    enum Choice {
        First,
        Second,
    }

    #[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
    #[reflect(Serialize, Deserialize)]
    struct Fixture {
        choice: Option<Choice>,
        entries: Option<Vec<u32>>,
    }

    #[test]
    fn option_enum_and_list_actions_address_the_contained_value() {
        let mut document = Document::open(
            "(choice: Some(First), entries: Some([7]))".to_owned(),
            Schema::<Fixture> {
                parse: |text| ron::from_str(text).map_err(|error| error.to_string()),
                check: |_| Vec::new(),
                importer: None,
            },
            Arc::new(registry_of::<Fixture>()),
        );
        let nodes = tree_of(&document);
        let choice = nodes.iter().find(|node| node.label == "choice").expect("choice");
        let entries = nodes.iter().find(|node| node.label == "entries").expect("entries");
        document.pick_variant("pick", &choice.path, "Second").expect("the contained enum");
        document.insert_blank("add", &entries.path).expect("the contained list");
        assert_eq!(document.value().expect("value").choice, Some(Choice::Second));
        assert_eq!(document.value().expect("value").entries, Some(vec![7, 0]));
        let (outer, _) = choice.option.as_ref().expect("option control");
        document.pick_variant("off", outer, "None").expect("the outer option");
        assert_eq!(document.value().expect("value").choice, None);
    }

    #[test]
    fn equal_display_labels_keep_distinct_property_steps() {
        let mut nodes = Vec::new();
        place_of(&mut nodes, &0_u32, &PropertyPath::root(), &[Step::Key("0".to_owned())]).expect("key");
        place_of(&mut nodes, &0_u32, &PropertyPath::root(), &[Step::Index(0)]).expect("index");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].label, nodes[1].label);
        assert_ne!(nodes[0].path, nodes[1].path);
    }

    #[test]
    fn numeric_controls_follow_reflected_types_and_round_trip_fractional_and_small_values() {
        assert!(number_kind(&0_u64).is_none(), "a small u64 still needs exact text for larger edits");
        assert!(matches!(number_kind(&1.0_f32), Some(NumberKind::Float)));
        for value in [1e-30, 1.0, 2.5, 0.1234567890123456, 1e30] {
            let literal = number_literal(value, NumberKind::Float, 2, false);
            assert_eq!(literal.parse::<f64>().expect("float literal").to_bits(), value.to_bits());
        }
        assert_eq!(number_literal(2.0, NumberKind::Float, 2, false), "2.00");
        assert_eq!(number_literal(2.5, NumberKind::Float, 2, false), "2.50");
    }
}
