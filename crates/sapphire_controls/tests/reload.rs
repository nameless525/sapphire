//! Bindings edited while the game runs: a new binding reads at once, an old one's gone, and a document that
//! can't play combat is refused with what's playing kept.

mod support;

use sapphire_controls::{
    COMBAT_CONTEXT, CombatControls, ControlsError, DEFAULT_BINDINGS, PAUSE_CONTEXT, default_bindings,
};
use sapphire_gameplay::Buttons;
use sapphire_input::{InputMap, RebindError};
use sapphire_platform::Scancode;
use support::{Rig, key};

fn compiled(text: &str) -> InputMap {
    InputMap::from_ron(text).expect("compiles")
}

/// Presses `scancode` and lets it go, and says whether a step read the press as a light attack.
fn swings(rig: &mut Rig, scancode: Scancode) -> bool {
    rig.frame(&[key(scancode, true)]);
    let read = rig.steps(1)[0].contains(Buttons::LIGHT);
    rig.frame(&[key(scancode, false)]);
    let _ = rig.steps(1);
    read
}

#[test]
fn a_new_binding_reads_at_once_and_the_old_one_is_gone() {
    let mut rig = Rig::new();
    assert!(swings(&mut rig, Scancode::J));

    let edited = DEFAULT_BINDINGS.replace("source: Key(J)", "source: Key(U)");
    rig.controls.replace_bindings(compiled(&edited)).expect("it has what combat reads");
    assert!(!swings(&mut rig, Scancode::J), "the old key's gone");
    assert!(swings(&mut rig, Scancode::U), "the new one reads");
}

/// An action moved in the document, as adding one does, is read at its new place: ids don't survive an edit.
#[test]
fn what_combat_reads_is_found_again_in_a_document_that_moved_it() {
    let mut rig = Rig::new();
    let edited = DEFAULT_BINDINGS.replacen(
        "(name: \"light\", kind: Button),",
        "(name: \"an_apps_own\", kind: Button),\n            (name: \"light\", kind: Button),",
        1,
    );
    rig.controls.replace_bindings(compiled(&edited)).expect("it has what combat reads");
    assert!(swings(&mut rig, Scancode::J));
}

/// The combat context stays on, and a pause stays a pause: nothing about an edit lets a paused fight go on.
#[test]
fn enabled_contexts_stay_enabled() {
    let mut rig = Rig::new();
    rig.controls.set_paused(true);
    rig.frame(&[]);
    rig.controls.replace_bindings(compiled(DEFAULT_BINDINGS)).expect("the same bindings");
    assert!(rig.controls.is_paused());
    let map = rig.controls.input().map();
    let pause = map.context(PAUSE_CONTEXT).expect("there");
    assert!(rig.controls.input().is_enabled(pause));
    let combat = map.context(COMBAT_CONTEXT).expect("there");
    assert!(!rig.controls.input().is_enabled(combat));
    rig.controls.set_paused(false);
    rig.frame(&[]);
    assert!(
        rig.controls.input().is_enabled(rig.controls.input().map().context(COMBAT_CONTEXT).expect("there"))
    );
    assert!(swings(&mut rig, Scancode::J));
}

#[test]
fn a_document_that_cant_play_combat_is_refused_and_what_plays_stays() {
    let mut rig = Rig::new();

    let mut lacking = default_bindings().expect("parses");
    lacking.actions.retain(|action| action.name != "light");
    for context in &mut lacking.contexts {
        context.bindings.retain(|binding| binding.action != "light");
    }
    let refused = rig.controls.replace_bindings(InputMap::new(lacking).expect("compiles"));
    assert!(matches!(refused, Err(ControlsError::MissingAction("light"))), "{refused:?}");
    assert!(swings(&mut rig, Scancode::J), "still on what it had");

    let mut no_pause = default_bindings().expect("parses");
    no_pause.contexts.retain(|context| context.name != PAUSE_CONTEXT);
    let refused = rig.controls.replace_bindings(InputMap::new(no_pause).expect("compiles"));
    assert!(matches!(refused, Err(ControlsError::MissingContext(PAUSE_CONTEXT))), "{refused:?}");
    assert!(swings(&mut rig, Scancode::J), "still on what it had");
}

/// An app's own context, enabled through `input_mut`, is the app's to keep: bindings that drop it are refused.
#[test]
fn bindings_that_lose_an_enabled_context_are_refused() {
    let mut document = default_bindings().expect("parses");
    let mut own = document.contexts[0].clone();
    own.name = "an_apps_own".to_owned();
    own.bindings.clear();
    document.contexts.push(own);
    let mut controls = CombatControls::new(InputMap::new(document).expect("compiles")).expect("plays");
    let context = controls.input().map().context("an_apps_own").expect("there");
    controls.input_mut().enable_context(context, 5);

    let refused = controls.replace_bindings(compiled(DEFAULT_BINDINGS));
    let Err(ControlsError::Rebind(RebindError::LostContext(name))) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(name, "an_apps_own");
    assert!(controls.input().map().context("an_apps_own").is_some(), "the old map stays");
}
