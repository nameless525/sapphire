//! [`InputMap`]: a bindings document checked and compiled into flat tables.

use std::ops::Range;

use sapphire_serialize::DocumentError;

use crate::bindings::{ActionKind, BindingDesc, GestureSettings, InputBindings};
use crate::modifier::Modifier;
use crate::source::{Gesture, MouseButton, PadAxis, PadButton, Source, Stick};
use crate::trigger::TriggerSpec;

/// An action in an [`InputMap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionId(u16);

impl ActionId {
    pub(crate) fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// A context in an [`InputMap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContextId(u16);

impl ContextId {
    pub(crate) fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// An on-screen control: a [`Source::Virtual`] name that bindings read, driven through
/// [`InputSystem::inject`](crate::InputSystem::inject).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualControl(u16);

impl VirtualControl {
    pub(crate) fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// A bindings document that can't be compiled.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MapError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error("two actions are named `{0}`")]
    DuplicateAction(String),
    #[error("two contexts are named `{0}`")]
    DuplicateContext(String),
    #[error("two bindings are named `{0}`")]
    DuplicateBinding(String),
    #[error("context `{context}`, binding {binding}: no action is named `{action}`")]
    UnknownAction { context: String, binding: usize, action: String },
    #[error("context `{context}`, binding {binding}: {reason}")]
    InvalidBinding { context: String, binding: usize, reason: &'static str },
    #[error("chords form a cycle through action `{0}`")]
    ChordCycle(String),
    #[error("{0}")]
    InvalidGestures(&'static str),
    #[error("more than {} actions, contexts or on-screen controls", u16::MAX)]
    TooMany,
}

/// A source, resolved for fast lookup against device state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceId {
    Key(u16),
    Mouse(Option<u32>),
    MouseMotion,
    MouseWheel,
    MouseWheelHorizontal,
    Pad(u32),
    PadAxis(PadAxis),
    PadStick(Stick),
    Touch(Gesture),
    Virtual(VirtualControl),
}

/// Motion that accumulates between updates, one accumulator each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Channel {
    Pointer,
    Wheel,
    Drag,
    Pinch,
}

impl Channel {
    pub const COUNT: usize = 4;

    pub fn index(self) -> usize {
        self as usize
    }
}

impl SourceId {
    /// The motion accumulator this source reads, if it is one.
    pub fn channel(self) -> Option<Channel> {
        match self {
            Self::MouseMotion => Some(Channel::Pointer),
            Self::MouseWheel | Self::MouseWheelHorizontal => Some(Channel::Wheel),
            Self::Touch(Gesture::Drag) => Some(Channel::Drag),
            Self::Touch(Gesture::Pinch) => Some(Channel::Pinch),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Action {
    pub name: String,
    pub kind: ActionKind,
    pub consume: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Context {
    pub name: String,
    pub bindings: Range<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct Binding {
    pub action: ActionId,
    pub context: ContextId,
    pub source: SourceId,
    pub modifiers: Range<usize>,
    pub trigger: TriggerSpec,
    pub chord: Option<ActionId>,
    pub threshold: f32,
}

/// A compiled bindings document: every name resolved, every parameter checked, and the actions
/// ordered so that chords are evaluated before the bindings that depend on them.
#[derive(Clone, Debug)]
pub struct InputMap {
    document: InputBindings,
    actions: Vec<Action>,
    /// Every action, chord dependencies first.
    order: Vec<ActionId>,
    contexts: Vec<Context>,
    bindings: Vec<Binding>,
    modifiers: Vec<Modifier>,
    virtuals: Vec<String>,
    /// The most chord blockers active bindings can need at once (see `InputSystem`).
    blocker_capacity: usize,
}

impl InputMap {
    /// Compiles a bindings document.
    pub fn new(document: InputBindings) -> Result<Self, MapError> {
        compile(document)
    }

    /// Reads and compiles a bindings document in RON.
    pub fn from_ron(text: &str) -> Result<Self, MapError> {
        Self::new(sapphire_serialize::from_ron(text)?)
    }

    /// The document this map was compiled from, including any rebinding since: what settings
    /// persist.
    pub fn document(&self) -> &InputBindings {
        &self.document
    }

    pub fn action(&self, name: &str) -> Option<ActionId> {
        self.actions.iter().position(|a| a.name == name).map(|i| ActionId(i as u16))
    }

    pub fn context(&self, name: &str) -> Option<ContextId> {
        self.contexts.iter().position(|c| c.name == name).map(|i| ContextId(i as u16))
    }

    pub fn virtual_control(&self, name: &str) -> Option<VirtualControl> {
        self.virtuals.iter().position(|v| v == name).map(|i| VirtualControl(i as u16))
    }

    pub fn action_name(&self, action: ActionId) -> &str {
        &self.actions[action.index()].name
    }

    pub fn action_kind(&self, action: ActionId) -> ActionKind {
        self.actions[action.index()].kind
    }

    pub fn context_name(&self, context: ContextId) -> &str {
        &self.contexts[context.index()].name
    }

    pub fn action_count(&self) -> usize {
        self.actions.len()
    }

    pub fn context_count(&self) -> usize {
        self.contexts.len()
    }

    pub(crate) fn gestures(&self) -> &GestureSettings {
        &self.document.gestures
    }

    pub(crate) fn actions(&self) -> &[Action] {
        &self.actions
    }

    pub(crate) fn order(&self) -> &[ActionId] {
        &self.order
    }

    pub(crate) fn contexts(&self) -> &[Context] {
        &self.contexts
    }

    pub(crate) fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    pub(crate) fn modifiers(&self, binding: &Binding) -> &[Modifier] {
        &self.modifiers[binding.modifiers.clone()]
    }

    pub(crate) fn virtual_count(&self) -> usize {
        self.virtuals.len()
    }

    pub(crate) fn blocker_capacity(&self) -> usize {
        self.blocker_capacity
    }

    /// A copy of the document with the binding named `name` reading `source`, and the source it
    /// read before; `None` if no binding has that name.
    pub(crate) fn rebound(&self, name: &str, source: Source) -> Option<(InputBindings, Source)> {
        let mut document = self.document.clone();
        let binding = document
            .contexts
            .iter_mut()
            .flat_map(|context| &mut context.bindings)
            .find(|binding| binding.name.as_deref() == Some(name))?;
        let previous = std::mem::replace(&mut binding.source, source);
        Some((document, previous))
    }
}

fn id(index: usize) -> Result<u16, MapError> {
    u16::try_from(index).map_err(|_| MapError::TooMany)
}

fn compile(document: InputBindings) -> Result<InputMap, MapError> {
    document.gestures.validate().map_err(MapError::InvalidGestures)?;

    let mut actions = Vec::with_capacity(document.actions.len());
    for desc in &document.actions {
        if actions.iter().any(|a: &Action| a.name == desc.name) {
            return Err(MapError::DuplicateAction(desc.name.clone()));
        }
        id(actions.len())?;
        actions.push(Action { name: desc.name.clone(), kind: desc.kind, consume: desc.consume });
    }
    let find_action = |name: &str| actions.iter().position(|a| a.name == name).map(|i| ActionId(i as u16));

    let mut contexts = Vec::with_capacity(document.contexts.len());
    let mut bindings = Vec::new();
    let mut modifiers = Vec::new();
    let mut virtuals: Vec<String> = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    for (context_index, desc) in document.contexts.iter().enumerate() {
        if contexts.iter().any(|c: &Context| c.name == desc.name) {
            return Err(MapError::DuplicateContext(desc.name.clone()));
        }
        let context = ContextId(id(context_index)?);
        let start = bindings.len();
        for (index, binding) in desc.bindings.iter().enumerate() {
            let invalid =
                |reason| MapError::InvalidBinding { context: desc.name.clone(), binding: index, reason };
            let unknown = |action: &str| MapError::UnknownAction {
                context: desc.name.clone(),
                binding: index,
                action: action.to_owned(),
            };
            let action = find_action(&binding.action).ok_or_else(|| unknown(&binding.action))?;
            let chord = match &binding.chord {
                Some(chord) => Some(find_action(chord).ok_or_else(|| unknown(chord))?),
                None => None,
            };
            if let Some(name) = &binding.name {
                if names.contains(&name.as_str()) {
                    return Err(MapError::DuplicateBinding(name.clone()));
                }
                names.push(name);
            }
            for modifier in &binding.modifiers {
                modifier.validate().map_err(invalid)?;
            }
            let trigger = TriggerSpec::compile(binding.trigger).map_err(invalid)?;
            let threshold = binding_threshold(binding, actions[action.index()].kind).map_err(invalid)?;
            let modifier_start = modifiers.len();
            modifiers.extend_from_slice(&binding.modifiers);
            bindings.push(Binding {
                action,
                context,
                source: resolve(&binding.source, &mut virtuals)?,
                modifiers: modifier_start..modifiers.len(),
                trigger,
                chord,
                threshold,
            });
        }
        contexts.push(Context { name: desc.name.clone(), bindings: start..bindings.len() });
    }

    let order = evaluation_order(&actions, &bindings)?;
    let blocker_capacity = bindings
        .iter()
        .filter(|chorded| chorded.chord.is_some())
        .map(|chorded| bindings.iter().filter(|b| b.chord.is_none() && b.source == chorded.source).count())
        .sum();
    Ok(InputMap { document, actions, order, contexts, bindings, modifiers, virtuals, blocker_capacity })
}

fn binding_threshold(binding: &BindingDesc, kind: ActionKind) -> Result<f32, &'static str> {
    match binding.actuation {
        Some(threshold) if !(threshold >= 0.0 && threshold.is_finite()) => {
            Err("the actuation threshold must be zero or more")
        }
        Some(threshold) => Ok(threshold),
        // Half a trigger pull presses a button; an axis counts any movement past its dead zone.
        None if kind == ActionKind::Button => Ok(0.5),
        None => Ok(0.0),
    }
}

fn resolve(source: &Source, virtuals: &mut Vec<String>) -> Result<SourceId, MapError> {
    Ok(match source {
        Source::Key(key) => SourceId::Key(key.scancode().0),
        Source::Mouse(button) => SourceId::Mouse(MouseButton::bit(*button)),
        Source::MouseMotion => SourceId::MouseMotion,
        Source::MouseWheel => SourceId::MouseWheel,
        Source::MouseWheelHorizontal => SourceId::MouseWheelHorizontal,
        Source::Pad(button) => SourceId::Pad(PadButton::bit(*button)),
        Source::PadAxis(axis) => SourceId::PadAxis(*axis),
        Source::PadStick(stick) => SourceId::PadStick(*stick),
        Source::Touch(gesture) => SourceId::Touch(*gesture),
        Source::Virtual(name) => {
            let index = match virtuals.iter().position(|v| v == name) {
                Some(index) => index,
                None => {
                    virtuals.push(name.clone());
                    virtuals.len() - 1
                }
            };
            SourceId::Virtual(VirtualControl(id(index)?))
        }
    })
}

/// Orders the actions so that every chord is evaluated before the actions that read it: the
/// chorded binding's own action, and every action with an unchorded binding of the same source,
/// which the chord blocks. Kahn's algorithm, stable in document order.
fn evaluation_order(actions: &[Action], bindings: &[Binding]) -> Result<Vec<ActionId>, MapError> {
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for chorded in bindings {
        let Some(chord) = chorded.chord else { continue };
        edges.push((chord.index(), chorded.action.index()));
        for blocked in bindings.iter().filter(|b| b.chord.is_none() && b.source == chorded.source) {
            edges.push((chord.index(), blocked.action.index()));
        }
    }
    let mut incoming = vec![0usize; actions.len()];
    for &(_, to) in &edges {
        incoming[to] += 1;
    }
    let mut order = Vec::with_capacity(actions.len());
    let mut placed = vec![false; actions.len()];
    while order.len() < actions.len() {
        let Some(next) = (0..actions.len()).find(|&i| !placed[i] && incoming[i] == 0) else {
            let stuck = (0..actions.len()).find(|&i| !placed[i]).expect("some action is unplaced");
            return Err(MapError::ChordCycle(actions[stuck].name.clone()));
        };
        placed[next] = true;
        order.push(ActionId(next as u16));
        for &(from, to) in &edges {
            if from == next {
                incoming[to] -= 1;
            }
        }
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{ActionDesc, ContextDesc};
    use crate::source::Key;
    use crate::trigger::Trigger;

    fn action(name: &str, kind: ActionKind) -> ActionDesc {
        ActionDesc { name: name.to_owned(), kind, consume: true }
    }

    fn document(actions: Vec<ActionDesc>, bindings: Vec<BindingDesc>) -> InputBindings {
        InputBindings {
            actions,
            contexts: vec![ContextDesc { name: "main".to_owned(), bindings }],
            gestures: GestureSettings::default(),
        }
    }

    #[test]
    fn chords_are_ordered_before_what_they_gate() {
        // "save" is listed first but reads "ctrl", and "back" is blocked by it.
        let map = InputMap::new(document(
            vec![
                action("save", ActionKind::Button),
                action("back", ActionKind::Button),
                action("ctrl", ActionKind::Button),
            ],
            vec![
                BindingDesc {
                    chord: Some("ctrl".to_owned()),
                    ..BindingDesc::new("save", Source::Key(Key::S))
                },
                BindingDesc::new("back", Source::Key(Key::S)),
                BindingDesc::new("ctrl", Source::Key(Key::LeftCtrl)),
            ],
        ))
        .expect("valid");
        let names: Vec<_> = map.order().iter().map(|&a| map.action_name(a)).collect();
        assert_eq!(names, ["ctrl", "save", "back"]);
        assert_eq!(map.blocker_capacity(), 1);
    }

    #[test]
    fn errors_name_what_is_wrong() {
        let a = || vec![action("a", ActionKind::Button), action("b", ActionKind::Button)];
        let cycle = document(
            a(),
            vec![
                BindingDesc { chord: Some("b".to_owned()), ..BindingDesc::new("a", Source::Key(Key::A)) },
                BindingDesc { chord: Some("a".to_owned()), ..BindingDesc::new("b", Source::Key(Key::B)) },
            ],
        );
        assert!(matches!(InputMap::new(cycle), Err(MapError::ChordCycle(_))));

        let unknown = document(a(), vec![BindingDesc::new("jump", Source::Key(Key::Space))]);
        assert!(matches!(InputMap::new(unknown), Err(MapError::UnknownAction { binding: 0, .. })));

        let duplicate =
            document(vec![action("a", ActionKind::Button), action("a", ActionKind::Axis1D)], vec![]);
        assert!(matches!(InputMap::new(duplicate), Err(MapError::DuplicateAction(_))));

        let hold = BindingDesc {
            trigger: Trigger::Hold { seconds: -1.0 },
            ..BindingDesc::new("a", Source::Key(Key::A))
        };
        assert!(matches!(InputMap::new(document(a(), vec![hold])), Err(MapError::InvalidBinding { .. })));

        let named =
            |key| BindingDesc { name: Some("same".to_owned()), ..BindingDesc::new("a", Source::Key(key)) };
        let twice = document(a(), vec![named(Key::A), named(Key::B)]);
        assert!(matches!(InputMap::new(twice), Err(MapError::DuplicateBinding(_))));
    }

    #[test]
    fn thresholds_default_by_action_kind() {
        let map = InputMap::new(document(
            vec![action("fire", ActionKind::Button), action("look", ActionKind::Axis2D)],
            vec![
                BindingDesc::new("fire", Source::PadAxis(PadAxis::RightTrigger)),
                BindingDesc::new("look", Source::PadStick(Stick::Right)),
                BindingDesc { actuation: Some(0.8), ..BindingDesc::new("fire", Source::Key(Key::F)) },
            ],
        ))
        .expect("valid");
        let thresholds: Vec<_> = map.bindings().iter().map(|b| b.threshold).collect();
        assert_eq!(thresholds, [0.5, 0.0, 0.8]);
    }
}
