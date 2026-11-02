//! Pillar 3 (`docs/m2-plan.md` §2): a hit lands everywhere at once. A real fight, recorded and replayed
//! through G3's resolve, with every real sink a cue fires into but the pad: if any of these moves a
//! frame late, a hit stops feeling like one.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_core::Name;
use sapphire_cues::CueTable;
use sapphire_moves::{HIT_LANDED, MoveSetDocument};
use sapphire_render::PostEffects;
use support::Fight;

/// At the fight's first hit, every sink goes from nothing to firing in one frame, the same one in which
/// hitstop starts.
#[test]
fn one_hit_reaches_every_sink_in_one_frame() {
    let mut fight = Fight::new("fight");
    let mut before = None;
    let hit = loop {
        let reached = fight.frame(1).expect("the fight lands a hit");
        if reached.hits > 0 {
            break reached;
        }
        before = Some(reached);
    };
    let before = before.expect("a frame before it");
    assert_eq!(before.particles, 0, "no effects yet");
    assert_eq!(before.screen_layers, 0, "no screen effect yet");
    assert_eq!(before.post, Some(PostEffects::NONE));
    assert_eq!(before.shakes, 0, "no shake yet");
    assert_eq!(before.rumbles, 0, "no rumble yet");
    assert_eq!(before.in_hitstop, 0, "nobody frozen yet");
    assert!(!before.view_moved);

    assert!(hit.particles > 0, "the spark shows on the hit's frame: {hit:?}");
    assert!(hit.screen_layers > 0, "the screen effect was pushed: {hit:?}");
    assert_ne!(hit.post, Some(PostEffects::NONE), "and post draws it");
    assert_eq!(hit.shakes, 1, "the shake: {hit:?}");
    assert!(hit.view_moved, "and the view moved");
    assert_eq!(hit.voices, 1, "the sound started on the mixer: {hit:?}");
    assert_eq!(hit.rumbles, 1, "the pad was told: {hit:?}");
    assert_eq!(hit.in_hitstop, 2, "and both fighters froze: {hit:?}");
    assert_eq!(fight.cues.stats().unmatched, 0);
}

/// Not just the first: every frame of a whole fight with a hit in it fires all of them, and nothing fires
/// a screen effect or the pad without one.
#[test]
fn every_hit_of_a_fight_reaches_every_sink_in_its_own_frame() {
    let mut fight = Fight::new("fight");
    let mut hits = 0;
    while let Some(reached) = fight.frame(1) {
        if reached.hits > 0 {
            hits += reached.hits;
            assert!(reached.screen_layers > 0, "{reached:?}");
            assert!(reached.shakes > 0, "{reached:?}");
            assert!(reached.voices > 0, "{reached:?}");
            assert!(reached.rumbles > 0, "{reached:?}");
            assert!(reached.particles > 0, "{reached:?}");
        } else {
            assert_eq!(reached.screen_layers, 0, "{reached:?}");
        }
    }
    assert!(hits > 20, "a fight's worth: {hits}");
    let stats = fight.cues.stats();
    assert_eq!(stats.effects_dropped, 0);
    assert_eq!(stats.sounds_dropped, 0);
}

/// Mobile-Low runs two steps a frame, so a hit can fall on the first and its cues be gone by the end of
/// the second. It all still fires in that frame, and a whole fight fires the same as at one step a frame.
#[test]
fn a_frame_of_two_steps_loses_nothing() {
    let mut one = Fight::new("fight");
    while one.frame(1).is_some() {}
    let mut two = Fight::new("fight");
    let mut on_first_step = 0;
    // By hand, to catch a hit on the first of the two.
    while let Some(first) = two.step() {
        let second = two.step().unwrap_or(0);
        if first > 0 && second == 0 {
            on_first_step += 1;
            assert!(!two.cues.sounds().is_empty(), "its sound waits for the frame");
        }
        two.end_frame();
    }
    assert!(on_first_step > 0, "the fight has hits on a frame's first step");

    assert_eq!(one.cues.stats(), two.cues.stats(), "every cue, whichever step it fell on");
    assert_eq!(one.screen.stats().pushed, two.screen.stats().pushed);
    assert_eq!(one.voices_started(), two.voices_started());
    assert_eq!(one.cues.stats().sounds as usize, one.voices_started());
}

/// Every cue the recorded fights publish has a row, if only to say it fires nothing, so a new cue in the
/// test moves shows up here rather than going quietly unheard.
/// Every `.moves.ron` under `folder`.
fn move_sets_in(folder: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(folder).expect("reads") {
        let path = entry.expect("reads").path();
        if path.is_dir() {
            move_sets_in(&path, found);
        } else if path.to_string_lossy().ends_with(".moves.ron") {
            found.push(path);
        }
    }
}

/// What the shipped move sets can publish that no recording has yet: a move's timed cues and its attacks'
/// own. A row may fire nothing, but it has to say so.
#[test]
fn every_cue_a_shipped_move_set_can_publish_has_a_row() {
    const LANDED: &str = "hit_landed";
    assert_eq!(HIT_LANDED, Name::new(LANDED));
    let table = CueTable::default_table();
    let has_row = |name: &str| table.rows().iter().any(|row| row.cue == name);
    let mut files = Vec::new();
    move_sets_in(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat"), &mut files);
    assert!(!files.is_empty(), "there are move sets to read");
    let mut missing = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("reads");
        let set = MoveSetDocument::parse(&text).expect("a move set");
        for mv in &set.moves {
            let timed = mv.cues.iter().map(|cue| cue.name.as_str());
            let attacks = mv.hitboxes.iter().filter_map(|hitbox| hitbox.attack.as_ref());
            let landed = attacks.map(|attack| attack.cue.as_deref().unwrap_or(LANDED));
            for name in timed.chain(landed).filter(|name| !has_row(name)) {
                missing.push(format!("{}: {}: {name}", file.display(), mv.name));
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
    // What the player's moves publish.
    for name in
        ["swing_windup", "swing", "step", "finisher_land", "dodge_start", "skid", "skill_flash", "skill_hit"]
    {
        assert!(has_row(name), "{name}");
    }
}

#[test]
fn every_cue_the_recordings_publish_has_a_row() {
    for name in
        ["fight", "string", "dodge-dash", "dodge-counter", "cancel-windows", "melee-play", "melee-sweep"]
    {
        let mut fight = Fight::new(name);
        let mut unmatched = Vec::new();
        while fight.step().is_some() {
            let seen = fight.cues.stats().unmatched;
            if seen as usize != unmatched.len() {
                let sim = fight.player.simulation();
                unmatched.push(format!("{}: {:?}", sim.tick(), sim.cues().collect::<Vec<_>>()));
            }
            fight.end_frame();
        }
        assert!(unmatched.is_empty(), "{name}: {}", unmatched.join("\n"));
        assert!(fight.cues.stats().cues > 0, "{name} publishes cues");
    }
}
