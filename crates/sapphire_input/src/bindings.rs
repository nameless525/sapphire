//! The bindings document: actions, the contexts that bind sources to them, and the touch
//! gesture thresholds, authored as RON (`sapphire_serialize::document`).

use bevy_reflect::Reflect;
use sapphire_reflect::Versioned;
use sapphire_serialize::ReflectDocument;
use serde::{Deserialize, Serialize};

use crate::modifier::Modifier;
use crate::source::Source;
use crate::trigger::Trigger;

/// Everything an [`InputMap`](crate::InputMap) is compiled from. As a document:
///
/// ```ron
/// (
///     type: "sapphire::input::InputBindings",
///     version: 1,
///     data: (
///         actions: [
///             (name: "move", kind: Axis2D),
///             (name: "jump", kind: Button),
///         ],
///         contexts: [(
///             name: "explore",
///             bindings: [
///                 (action: "move", source: PadStick(Left), modifiers: [DeadZone(lower: 0.2)]),
///                 (action: "move", source: Key(W), modifiers: [Swizzle]),
///                 (action: "jump", source: Key(Space), trigger: Pressed, name: "jump.keyboard"),
///             ],
///         )],
///     ),
/// )
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
#[reflect(Document)]
pub struct InputBindings {
    pub actions: Vec<ActionDesc>,
    pub contexts: Vec<ContextDesc>,
    #[serde(default)]
    pub gestures: GestureSettings,
}

impl Versioned for InputBindings {
    const VERSION: u32 = 1;
}

/// Something the player can do, independent of what drives it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub struct ActionDesc {
    /// Unique within the document; code looks actions up by it.
    pub name: String,
    pub kind: ActionKind,
    /// Whether a context that binds this action takes those sources away from lower-priority
    /// contexts (Enhanced Input's "consume input"). On by default.
    #[serde(default = "consumes_by_default", skip_serializing_if = "is_true")]
    pub consume: bool,
}

/// The shape of an action's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum ActionKind {
    /// On or off: the value is 1 while triggered.
    Button,
    /// One axis: bindings' x components add up.
    Axis1D,
    /// Two axes: bindings' values add up.
    Axis2D,
}

/// A named set of bindings, enabled and disabled as a unit at a priority (Combat, Explore,
/// Menu, Cutscene).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub struct ContextDesc {
    /// Unique within the document.
    pub name: String,
    pub bindings: Vec<BindingDesc>,
}

/// One source driving one action, through modifiers and a trigger.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub struct BindingDesc {
    /// The action's name.
    pub action: String,
    pub source: Source,
    /// Applied in order to the source's value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<Modifier>,
    #[serde(default, skip_serializing_if = "Trigger::is_down")]
    pub trigger: Trigger,
    /// An action that must be triggered for this binding to fire: `LeftCtrl`'s action for
    /// `Ctrl+S`. While it is, the chord also blocks bindings of the same source without it, in
    /// the same and lower-priority contexts, so `Ctrl+S` doesn't also fire plain `S`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chord: Option<String>,
    /// The magnitude the modified value must reach to count as actuated. Default: 0.5 for
    /// button actions, and any nonzero value for axes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actuation: Option<f32>,
    /// A name players rebind this binding by ([`InputSystem::rebind`](crate::InputSystem::rebind)),
    /// unique within the document, such as `"dodge.keyboard"`. Bindings without one can't be
    /// rebound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl BindingDesc {
    /// A binding with no modifiers, trigger `Down` and nothing else set.
    pub fn new(action: impl Into<String>, source: Source) -> Self {
        Self {
            action: action.into(),
            source,
            modifiers: Vec::new(),
            trigger: Trigger::Down,
            chord: None,
            actuation: None,
            name: None,
        }
    }
}

/// Thresholds for the touch gestures. Distances are window points, so they hold across
/// screen densities.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
#[serde(default)]
pub struct GestureSettings {
    /// A lone finger that moves further than this from where it landed is dragging, and no
    /// longer a press.
    pub tap_slop: f32,
    /// A swipe covers at least this distance...
    pub swipe_min_distance: f32,
    /// ...within this many seconds of the finger landing.
    pub swipe_max_seconds: f32,
}

impl Default for GestureSettings {
    fn default() -> Self {
        Self { tap_slop: 10.0, swipe_min_distance: 60.0, swipe_max_seconds: 0.3 }
    }
}

impl GestureSettings {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        let positive = |value: f32| value > 0.0 && value.is_finite();
        if positive(self.tap_slop) && positive(self.swipe_min_distance) && positive(self.swipe_max_seconds) {
            Ok(())
        } else {
            Err("gesture thresholds must be positive")
        }
    }
}

fn consumes_by_default() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}
