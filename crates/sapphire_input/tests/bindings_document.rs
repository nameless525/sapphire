//! Bindings documents: every part of the vocabulary survives a RON round trip, loads by type
//! name through the registry, and compiles.

use sapphire_input::{
    ActionDesc, ActionKind, Axes, BindingDesc, ContextDesc, DeadZoneShape, Gesture, GestureSettings,
    InputBindings, InputMap, Key, Modifier, MouseButton, PadAxis, PadButton, Source, Stick, SwipeDirection,
    Trigger,
};
use sapphire_reflect::TypeRegistry;

fn everything() -> InputBindings {
    let action = |name: &str, kind| ActionDesc { name: name.to_owned(), kind, consume: true };
    let sources = [
        Source::Key(Key::Space),
        Source::Key(Key::Hid(100)),
        Source::Mouse(MouseButton::Right),
        Source::Mouse(MouseButton::Other(7)),
        Source::MouseMotion,
        Source::MouseWheel,
        Source::MouseWheelHorizontal,
        Source::Pad(PadButton::RightShoulder),
        Source::PadAxis(PadAxis::LeftTrigger),
        Source::PadStick(Stick::Right),
        Source::Touch(Gesture::Press),
        Source::Touch(Gesture::Drag),
        Source::Touch(Gesture::Pinch),
        Source::Touch(Gesture::Swipe(SwipeDirection::Left)),
        Source::Virtual("attack".to_owned()),
    ];
    let mut bindings: Vec<_> = sources.into_iter().map(|source| BindingDesc::new("look", source)).collect();
    bindings.push(BindingDesc {
        modifiers: vec![
            Modifier::DeadZone { lower: 0.1, upper: 0.9, shape: DeadZoneShape::Axial },
            Modifier::ResponseCurve { exponent: 2.0 },
            Modifier::Scale([1.5, -2.0]),
            Modifier::Negate(Axes::XY),
            Modifier::Swizzle,
            Modifier::ScaleByDeltaTime,
        ],
        trigger: Trigger::Hold { seconds: 0.4 },
        chord: Some("guard".to_owned()),
        actuation: Some(0.3),
        name: Some("charge.keyboard".to_owned()),
        ..BindingDesc::new("charge", Source::Key(Key::J))
    });
    for trigger in [
        Trigger::Pressed,
        Trigger::Released,
        Trigger::Tap { seconds: 0.2 },
        Trigger::DoubleTap { seconds: 0.3 },
    ] {
        bindings
            .push(BindingDesc { trigger, ..BindingDesc::new("guard", Source::Pad(PadButton::LeftShoulder)) });
    }
    InputBindings {
        actions: vec![
            action("look", ActionKind::Axis2D),
            action("charge", ActionKind::Button),
            ActionDesc { consume: false, ..action("guard", ActionKind::Axis1D) },
        ],
        contexts: vec![
            ContextDesc { name: "combat".to_owned(), bindings },
            ContextDesc { name: "menu".to_owned(), bindings: Vec::new() },
        ],
        gestures: GestureSettings { tap_slop: 12.0, swipe_min_distance: 80.0, swipe_max_seconds: 0.25 },
    }
}

#[test]
fn documents_round_trip() {
    let bindings = everything();
    let text = sapphire_serialize::to_ron(&bindings).expect("write");
    assert!(
        text.starts_with("(\n    type: \"sapphire::input::InputBindings\",\n    version: 1,\n"),
        "{text}"
    );
    assert_eq!(sapphire_serialize::from_ron::<InputBindings>(&text).expect("read"), bindings);
    InputMap::from_ron(&text).expect("compiles");
}

#[test]
fn documents_load_by_type_name() {
    let mut registry = TypeRegistry::new();
    sapphire_input::register_types(&mut registry);
    let text = sapphire_serialize::to_ron(&everything()).expect("write");
    let loaded = sapphire_serialize::from_ron_dynamic(&registry, &text).expect("load");
    assert_eq!(loaded.downcast_ref::<InputBindings>(), Some(&everything()));
}

#[test]
fn authored_documents_leave_defaults_out() {
    let text = r#"(
        type: "sapphire::input::InputBindings",
        version: 1,
        data: (
            actions: [(name: "move", kind: Axis2D)],
            contexts: [(name: "explore", bindings: [
                (action: "move", source: PadStick(Left), modifiers: [DeadZone()]),
            ])],
        ),
    )"#;
    let bindings = sapphire_serialize::from_ron::<InputBindings>(text).expect("read");
    let binding = &bindings.contexts[0].bindings[0];
    assert_eq!(binding.trigger, Trigger::Down);
    assert_eq!(
        binding.modifiers,
        [Modifier::DeadZone { lower: 0.2, upper: 1.0, shape: DeadZoneShape::Radial }]
    );
    assert!(bindings.actions[0].consume, "actions consume by default");
    assert_eq!(bindings.gestures, GestureSettings::default());

    // Writing leaves the defaults out again.
    let written = sapphire_serialize::to_ron(&bindings).expect("write");
    assert!(
        !written.contains("trigger") && !written.contains("consume") && !written.contains("chord"),
        "{written}"
    );
}

#[test]
fn compile_errors_point_into_the_document() {
    let text = r#"(type: "sapphire::input::InputBindings", version: 1, data: (
        actions: [(name: "jump", kind: Button)],
        contexts: [(name: "explore", bindings: [(action: "jump", source: Key(Spcae))])],
    ))"#;
    let error = InputMap::from_ron(text).expect_err("misspelled key");
    assert!(error.to_string().contains("3:"), "{error}");
}
