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
            asked.push(Asked::Remove(node.path.clone()));
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
        if let Ok(number) = leaf.literal.parse::<f64>() {
            let is_int = !leaf.literal.contains(['.', 'e', 'E']);
            let decimals = leaf.literal.split_once('.').map_or(2, |(_, fraction)| fraction.len().max(2));
            let mut shown = number;
            let drag = if is_int {
                egui::DragValue::new(&mut shown).speed(0.25).max_decimals(0)
            } else {
                egui::DragValue::new(&mut shown).speed(0.01).fixed_decimals(decimals)
            };
            let response = ui.add(drag);
            self.number_gesture(&response, &label, id);
            if response.changed() {
                let literal =
                    if is_int { format!("{}", shown.round() as i64) } else { format!("{shown:.decimals$}") };
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
                    node.leaf = Some(Leaf { literal, written: written.is_some() });
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
    let at = match nodes.iter().position(|node| node.label == label) {
        Some(at) => at,
        None => {
            nodes.push(Node {
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
    if rest.is_empty() { Some(node) } else { place_of(&mut node.children, document, &below, rest) }
}
