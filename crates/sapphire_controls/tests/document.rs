//! The default bindings document, and what a bad one says.

mod support;

use sapphire_controls::{
    BUTTON_ACTIONS, COMBAT_CONTEXT, CombatControls, ControlsError, DEFAULT_BINDINGS, MOVE_ACTION,
    PAUSE_CONTEXT, default_bindings,
};
use sapphire_gameplay::Buttons;
use sapphire_input::{ActionKind, InputBindings, InputMap, Source};

fn combat() -> &'static str {
    COMBAT_CONTEXT
}

#[test]
fn the_default_bindings_load() {
    let document = default_bindings().expect("parses");
    InputMap::new(document.clone()).expect("compiles");
    CombatControls::with_default_bindings().expect("has what combat reads");
    assert_eq!(
        document.contexts.iter().map(|context| context.name.as_str()).collect::<Vec<_>>(),
        [COMBAT_CONTEXT, PAUSE_CONTEXT]
    );
}

#[test]
fn the_table_covers_every_button() {
    let mut all = Buttons::NONE;
    for (button, _) in BUTTON_ACTIONS {
        assert!(!all.contains(button), "{button:?} is listed twice");
        all = all | button;
    }
    // A thumb's no button anyone presses: which device played goes through the source.
    assert_eq!(all, Buttons::ALL.minus(Buttons::TOUCH), "a new button needs an action");
}

/// The names a binding of this kind goes by in the default document.
fn is_pad(source: &Source) -> bool {
    matches!(source, Source::Pad(_) | Source::PadAxis(_) | Source::PadStick(_))
}

fn is_keyboard(source: &Source) -> bool {
    matches!(source, Source::Key(_))
}

#[test]
fn every_button_and_the_stick_are_on_every_device() {
    let document = default_bindings().unwrap();
    let context = document.contexts.iter().find(|context| context.name == combat()).expect("combat");
    let mut failures = Vec::new();
    let names = BUTTON_ACTIONS.iter().map(|(_, name)| *name).chain([MOVE_ACTION]);
    for name in names {
        let bindings: Vec<_> = context.bindings.iter().filter(|binding| binding.action == name).collect();
        if !bindings.iter().any(|binding| is_pad(&binding.source)) {
            failures.push(format!("`{name}` has no gamepad binding"));
        }
        if !bindings.iter().any(|binding| is_keyboard(&binding.source)) {
            failures.push(format!("`{name}` has no keyboard binding"));
        }
        if !bindings.iter().any(|binding| binding.source == Source::Virtual(name.to_owned())) {
            failures.push(format!("`{name}` has no on-screen control of its name"));
        }
        // Rebinding goes by name, so a player can move any of them.
        for binding in &bindings {
            if !matches!(binding.source, Source::Virtual(_)) && binding.name.is_none() {
                failures.push(format!("`{name}`'s binding of {:?} has no name to rebind by", binding.source));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn no_device_gives_two_verbs_one_source() {
    let document = default_bindings().unwrap();
    for context in &document.contexts {
        for (index, binding) in context.bindings.iter().enumerate() {
            for other in &context.bindings[index + 1..] {
                assert!(
                    binding.source != other.source || binding.action == other.action,
                    "{}: {:?} drives `{}` and `{}`",
                    context.name,
                    binding.source,
                    binding.action,
                    other.action,
                );
            }
        }
    }
}

/// A broken copy of the default document's text.
fn broken(find: &str, replace: &str) -> String {
    assert!(DEFAULT_BINDINGS.contains(find), "no `{find}` to break");
    DEFAULT_BINDINGS.replacen(find, replace, 1)
}

fn error_of(text: &str) -> String {
    let map = InputMap::from_ron(text).map_err(ControlsError::from);
    map.and_then(CombatControls::new).map(|_| ()).expect_err("a bad document").to_string()
}

#[test]
fn a_malformed_document_names_the_bad_entry() {
    let line_of = |text: &str, find: &str| {
        text.lines().position(|line| line.contains(find)).expect("the text is in there") + 1
    };

    // The table holds the text to break, what to break it to, and what the error must say.
    let table: [(&str, &str, &str, &[&str]); 8] = [
        (
            "an action no binding can name",
            r#"(action: "light", source: Pad(West)"#,
            r#"(action: "lihgt", source: Pad(West)"#,
            &["combat", "binding 6", "`lihgt`"],
        ),
        (
            "a chord on nothing",
            r#"(action: "dodge", source: Pad(South), trigger: DownFromPress"#,
            r#"(action: "dodge", source: Pad(South), trigger: DownFromPress, chord: "nothing""#,
            &["combat", "`nothing`"],
        ),
        (
            "a bad trigger's time",
            r#"source: Touch(Press), trigger: Tap(seconds: 0.3)"#,
            r#"source: Touch(Press), trigger: Tap(seconds: -1.0)"#,
            &["pause", "binding 4", "durations must be positive"],
        ),
        (
            "a dead zone inside out",
            "modifiers: [DeadZone(lower: 0.15, upper: 0.95)],\n                        name: \"move.gamepad\"",
            "modifiers: [DeadZone(lower: 0.95, upper: 0.15)],\n                        name: \"move.gamepad\"",
            &["combat", "binding 0", "dead zone"],
        ),
        (
            "two bindings of one name",
            r#"name: "heavy.gamepad""#,
            r#"name: "light.gamepad""#,
            &["`light.gamepad`"],
        ),
        ("an action missing", r#"(name: "swap", kind: Button),"#, "", &["`swap`"]),
        (
            "an action of the wrong shape",
            r#"(name: "dodge", kind: Button),"#,
            r#"(name: "dodge", kind: Axis1D),"#,
            &["`dodge`", "Axis1D", "Button"],
        ),
        ("a context missing", r#"name: "pause","#, r#"name: "paused","#, &["`pause`"]),
    ];
    let mut failures = Vec::new();
    for (name, find, replace, says) in table {
        let error = error_of(&broken(find, replace));
        for said in says {
            if !error.contains(said) {
                failures.push(format!("{name}: `{error}` doesn't say {said}"));
            }
        }
    }

    // A source that isn't one doesn't parse, and the error gives the line it's on.
    let text = broken("source: Pad(West)", "source: Pad(Wset)");
    let error = error_of(&text);
    let line = line_of(&text, "Wset");
    if !error.starts_with(&format!("{line}:")) || !error.contains("Wset") {
        failures.push(format!("a source that isn't one: `{error}` doesn't give line {line} and the source"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_document_without_combat_is_refused_by_name() {
    let empty = InputBindings::default();
    let error = CombatControls::new(InputMap::new(empty).unwrap()).expect_err("nothing to read");
    assert!(matches!(error, ControlsError::MissingAction(_)), "{error}");
    assert!(matches!(
        InputMap::from_ron(&broken(r#"(name: "look", kind: Axis2D),"#, r#"(name: "look", kind: Axis1D),"#))
            .map_err(ControlsError::from)
            .and_then(CombatControls::new),
        Err(ControlsError::WrongActionKind {
            name: "look",
            found: ActionKind::Axis1D,
            expected: ActionKind::Axis2D
        })
    ));
}

#[test]
fn an_app_can_add_its_own_contexts() {
    use sapphire_input::{ActionDesc, BindingDesc, ContextDesc, Key};
    let mut document = default_bindings().unwrap();
    document.actions.push(ActionDesc {
        name: "screenshot".to_owned(),
        kind: ActionKind::Button,
        consume: true,
    });
    document.contexts.push(ContextDesc {
        name: "app".to_owned(),
        bindings: vec![BindingDesc::new("screenshot", Source::Key(Key::J))],
    });
    let mut controls = CombatControls::new(InputMap::new(document).unwrap()).unwrap();
    let app = controls.input().map().context("app").unwrap();
    controls.input_mut().enable_context(app, 50);
    let mut rig = support::Rig::on(controls);
    rig.frame(&[support::key(sapphire_platform::Scancode::J, true)]);
    assert_eq!(rig.steps(1), [Buttons::NONE], "the app's context took J from combat's light");
    let screenshot = rig.controls.input().map().action("screenshot").unwrap();
    assert!(rig.controls.input().state(screenshot).is_triggered());
    rig.frame(&[support::key(sapphire_platform::Scancode::K, true)]);
    assert_eq!(rig.steps(1), [Buttons::HEAVY], "combat keeps everything else");
}
