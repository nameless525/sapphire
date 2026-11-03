//! A fight's HUD over its scene, with no GPU.

use std::ops::{Deref, DerefMut};

use sapphire_math::{Mat4, Vec3};
use sapphire_render::{FramePacket, MeshHandle, PostVariants};
use sapphire_time::TICK_HZ;
use sapphire_vfx::VfxBudget;

use super::*;
use game_combat::arena::{ArenaGround, ArenaShot};
use game_combat::enemy::{self, EnemyMeshes};
use game_combat::fight::Arena;
use sapphire_stage::RenderPose;
use sapphire_stage::lookdev::{self, TownMeshes};
use sapphire_stage::scene::{Figure, LookDev};

const BOUNDS: [[f32; 3]; 2] = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];

/// A scene and a fight with its HUD on it, which reads as the scene it's on.
struct Staged {
    scene: Scene,
    show: ShownFight,
}

impl Deref for Staged {
    type Target = Scene;

    fn deref(&self) -> &Scene {
        &self.scene
    }
}

impl DerefMut for Staged {
    fn deref_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }
}

impl Staged {
    fn frame_packet(&mut self, alpha: f32, size: Option<(u32, u32)>) -> &FramePacket {
        self.scene.frame_packet(alpha, size, &mut self.show)
    }

    fn set_hud(&mut self, hud: FightHud) {
        self.show.hud = Some(hud);
    }

    fn hud(&self) -> Option<&FightHud> {
        self.show.hud.as_ref()
    }

    fn play_cues(&mut self, budget: VfxBudget) -> Result<(), String> {
        self.show.fight.show.play_cues(&mut self.scene, budget)
    }

    fn follow_simulation(&mut self, sim: &Simulation, cast: &[sapphire_gameplay::ActorId]) {
        let followed = self.show.fight.show.follow_simulation(&mut self.scene, sim, cast);
        self.show.hear(followed);
    }

    fn show_cues(&mut self, sim: &Simulation) {
        self.show.fight.show.show_cues(&mut self.scene, sim);
    }

    fn end_cue_frame(
        &mut self,
        dt: f32,
        view_dt: f32,
        alpha: f32,
        rumble: &mut dyn sapphire_cues::RumbleSink,
    ) {
        self.show.fight.show.end_cue_frame(&mut self.scene, dt, view_dt, alpha, rumble);
    }

    fn drive_arena_camera(&mut self, dt: f32, look: Vec2, zoom: f32) -> bool {
        self.show.fight.show.drive_camera(dt, look, zoom)
    }
}

/// The arena's stage with stand-in meshes, framed by `shot`, and the figure it holds the character by.
fn arena_stage(shot: ArenaShot) -> (Staged, Figure) {
    let plain = MeshHandle::from_raw(1);
    let town = TownMeshes {
        plants: [plain; lookdev::Plant::ALL.len()],
        mountains: [plain; lookdev::MOUNTAINS.len()],
    };
    let figure = Figure { head: Vec3::new(0.0, 1.45, 0.0), ..Figure::MANNEQUIN };
    let pose = RenderPose { palette: vec![Default::default(); 2], morph_weights: vec![] };
    let look = LookDev { town, fog_cards: None, pose, figure };
    let cooked = sapphire_gameplay::arena::cooked_colliders().expect("it cooks");
    let set = sapphire_colliders::ColliderSet::read(&cooked).expect("it reads");
    let ground = ArenaGround::new(set).expect("the stage stands on it");
    let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
    let enemy = EnemyMeshes { warrior: MeshHandle::from_raw(2), blade: MeshHandle::from_raw(3) };
    let blade_in_model = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0));
    let held = enemy::Held {
        pose: RenderPose { palette: vec![Default::default(); 2], morph_weights: vec![] },
        figure: Figure { head: Vec3::new(0.0, 2.2, 0.0), ..Figure::MANNEQUIN },
        blade: blade_in_model,
        hand: 0,
    };
    let mut show = ShownFight::default();
    show.fight.show.spawn_arena(&mut scene, Arena { enemy, held, shot, look, ground });
    (Staged { scene, show }, figure)
}

/// A real party fight, by the script that swaps back to someone hurt: the bar shows a loss for a hit,
/// and none for a swap to someone with less.
#[test]
fn a_swap_doesnt_read_as_a_hit_on_the_huds_bar() {
    use sapphire_gameplay::replay::{Recording, SCENARIOS, find_scenario, find_script};
    use sapphire_gameplay::{ActorActions, ActorId, Buttons};
    use sapphire_math::Vec2;
    use sapphire_ui::{HudLayout, Insets, UiMetrics};

    use crate::hud::{HudMode, HudOptions};

    let target = (1280, 720);
    let bar = HudLayout::new(UiMetrics::of_pixels(target, 1.0, Insets::default())).player.hp;
    let (mut scene, _) = arena_stage(ArenaShot::Follow);
    let live = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
    scene.set_hud(FightHud::for_fight(live));
    let party = find_scenario(&SCENARIOS, game_combat::arena::PARTY_SCENARIO).expect("listed");
    let mut fight = Recording::new(party, party.config).expect("sets up");
    let cast = fight.cast().to_vec();
    // Its schedule goes round to the first again, and pressing on brings back those the warrior hurt.
    let script = find_script("party-swap").expect("listed").actions;
    let mut source = |tick: u64, cast: &[ActorId]| {
        let mut actions = script(tick, cast);
        if tick >= 352 && tick.is_multiple_of(2) {
            let swap = ActorActions { buttons: Buttons::SWAP, ..ActorActions::NEUTRAL };
            actions.set(cast[0], swap).expect("one actor fits a tick");
        }
        actions
    };
    let mut on_field = cast[0];
    let (mut swaps_down, mut losses) = (0, 0);
    for _ in 0..1200 {
        fight.step(&mut source).expect("short");
        let sim = fight.simulation();
        scene.follow_simulation(sim, &cast);
        let quads = &scene.frame_packet(1.0, Some(target)).ui.quads;
        // What the bar shows lost: a range that starts past its left end, on the bar itself.
        let lost = quads
            .iter()
            .filter(|quad| {
                let center =
                    Vec2::from(quad.origin) + (Vec2::from(quad.axis_x) + Vec2::from(quad.axis_y)) * 0.5;
                quad.clip[0] > 0.0 && (center - bar.center()).length() < 1e-2
            })
            .count();
        scene.end_frame();

        let share = |id| {
            let fighter = sim.actor(id).expect("in the party").fighter;
            fighter.health() as f32 / fighter.max_health() as f32
        };
        let now = sim.party().on_field(cast[0]);
        if now != on_field {
            if share(now) < share(on_field) {
                swaps_down += 1;
                assert_eq!(lost, 0, "tick {}: a swap to someone with less", sim.tick());
            }
            on_field = now;
        }
        losses += lost;
    }
    assert!(swaps_down > 0, "the script swaps to someone hurt");
    assert!(losses > 0, "and the bar does show a hit");
}

/// What a fight set going ends with it. A spark in the air, a screen effect, a shake and a hit's number
/// all belong to where the last fight's actors stood, so the new fight's first frame, the one the
/// view cuts on, shows none of them.
#[test]
fn a_new_fight_shows_nothing_the_last_one_set_going() {
    use crate::hud::{HudMode, HudOptions};

    let option = game_combat::combat::CombatOption::Fight { script: Some("idle".to_owned()), party: false };
    let mut combat = game_combat::combat::Combat::new(&option).expect("sets up");
    let cast = combat.cast().to_vec();
    let (mut scene, _) = arena_stage(ArenaShot::Director);
    scene.play_cues(VfxBudget::DESKTOP_LOW).expect("the shipped table resolves");
    scene.set_hud(FightHud::for_fight(HudOptions { mode: HudMode::Live, ..HudOptions::default() }));
    let step = |scene: &mut Staged, combat: &mut game_combat::combat::Combat| {
        combat.step(&mut game_combat::combat::no_bindings);
        scene.follow_simulation(combat.simulation(), &cast);
        scene.show_cues(combat.simulation());
        show_damage(scene.show.hud.as_mut().expect("a HUD"), &scene.scene, combat.simulation());
        scene.end_cue_frame(1.0 / TICK_HZ as f32, 1.0 / TICK_HZ as f32, 1.0, &mut ());
    };
    // How far the view's right has left level, whether the frame draws no effects, its screen
    // effects, and the numbers its HUD draws.
    let frame = |scene: &mut Staged| {
        scene.drive_arena_camera(1.0 / TICK_HZ as f32, Vec2::ZERO, 0.0);
        let packet = scene.frame_packet(1.0, Some((640, 360)));
        let view = packet.main_view().expect("a view").camera.view;
        let shown = (view.inverse().x_axis.y.abs(), packet.vfx.is_empty(), packet.post.effects);
        let (hud, _) = scene.hud().expect("set").last_frame();
        scene.end_frame();
        (shown, hud.damage_numbers)
    };
    while !combat.simulation().cues().any(|cue| cue.hit.is_some()) {
        let _ = frame(&mut scene);
        step(&mut scene, &mut combat);
        assert!(combat.simulation().tick() < 2000, "someone lands a hit by now");
    }
    let director = scene.show.fight.show.arena_mut().expect("built").director().expect("this shot has one");
    let finish = director.tuning().shake_id("finish").expect("a shipped preset");
    director.fire_shake(finish, 1.0);
    let ((_, empty, effects), numbers) = frame(&mut scene);
    assert!(!empty, "the spark's up");
    assert_ne!(effects, sapphire_render::PostEffects::NONE, "the screen effect's up");
    assert!(numbers > 0, "the number's up");
    let most = (0..6).map(|_| frame(&mut scene).0.0).fold(0.0_f32, f32::max);
    assert!(most > 1e-3, "the shake's up: {most}");

    combat.restart();
    step(&mut scene, &mut combat);
    assert_eq!(combat.simulation().tick(), 1, "the new fight's first step");
    let ((tilt, empty, effects), numbers) = frame(&mut scene);
    assert!(empty, "no spark of the last fight");
    assert_eq!(effects, sapphire_render::PostEffects::NONE, "no screen effect of the last fight");
    assert!(tilt < 1e-5, "no shake of the last fight: {tilt}");
    assert_eq!(numbers, 0, "no number of the last fight");
}

/// A step's cues stand until the next step, and a recording stopped at its limit or a replay past its
/// end takes none, so the frames after it read the same hits. They show once, and a new fight's show
/// whatever tick they land on.
#[test]
fn a_steps_hits_show_once_however_often_theyre_read() {
    use crate::hud::{HudMode, HudOptions};

    let option = game_combat::combat::CombatOption::Fight { script: Some("idle".to_owned()), party: false };
    let mut combat = game_combat::combat::Combat::new(&option).expect("sets up");
    let cast = combat.cast().to_vec();
    let (mut scene, _) = arena_stage(ArenaShot::Director);
    scene.set_hud(FightHud::for_fight(HudOptions { mode: HudMode::Live, ..HudOptions::default() }));
    let numbers = |scene: &mut Staged| {
        scene.drive_arena_camera(1.0 / TICK_HZ as f32, Vec2::ZERO, 0.0);
        let _ = scene.frame_packet(1.0, Some((640, 360)));
        let (hud, _) = scene.hud().expect("set").last_frame();
        scene.end_frame();
        hud.damage_numbers
    };
    while !combat.simulation().cues().any(|cue| cue.hit.is_some()) {
        let _ = numbers(&mut scene);
        combat.step(&mut game_combat::combat::no_bindings);
        scene.follow_simulation(combat.simulation(), &cast);
        assert!(combat.simulation().tick() < 2000, "someone lands a hit by now");
    }

    show_damage(scene.show.hud.as_mut().expect("a HUD"), &scene.scene, combat.simulation());
    let once = numbers(&mut scene);
    assert!(once > 0, "the number's up");
    for _ in 0..5 {
        show_damage(scene.show.hud.as_mut().expect("a HUD"), &scene.scene, combat.simulation());
    }
    assert_eq!(numbers(&mut scene), once, "the same tick's hits again");

    // A script's fight lands the same hit on the same tick, and it's a new one.
    let tick = combat.simulation().tick();
    combat.restart();
    combat.step(&mut game_combat::combat::no_bindings);
    scene.follow_simulation(combat.simulation(), &cast);
    while combat.simulation().tick() < tick {
        let _ = numbers(&mut scene);
        combat.step(&mut game_combat::combat::no_bindings);
        scene.follow_simulation(combat.simulation(), &cast);
    }
    show_damage(scene.show.hud.as_mut().expect("a HUD"), &scene.scene, combat.simulation());
    assert_eq!(numbers(&mut scene), once, "the new fight's");
}

#[test]
fn a_shown_fight_declares_what_it_plays() {
    use crate::hud::HudOptions;

    let (mut scene, _) = arena_stage(ArenaShot::Follow);
    let bare = scene.scene.declared_pipelines(&scene.show);
    assert!(!bare.effects && !bare.ui, "neither cues nor a HUD yet");
    scene.play_cues(VfxBudget::DESKTOP_LOW).expect("the shipped table resolves");
    scene.set_hud(FightHud::for_fight(HudOptions::default()));
    let declared = scene.scene.declared_pipelines(&scene.show);
    assert!(declared.effects, "cues play effects");
    assert!(declared.ui, "a HUD draws an interface");
    // The cues' screen effects go through their post variants. A `dev` build declares every preset's
    // from the start, since the overlay's effects panel fires any of them (`Scene::effect_variants`),
    // so there the cues add nothing a preset couldn't already push.
    assert_ne!(declared.post, PostVariants::NONE, "the cues' screen effects go through their post variants");
    if cfg!(feature = "dev") {
        assert_eq!(declared.post, bare.post, "a dev build already declares every preset's variants");
    } else {
        assert_eq!(bare.post, PostVariants::NONE, "a fight without cues pushes no screen effect");
    }
}
