//! Posing the fight's fighters each frame (M5 C1): who is posed, at what level, and where.
//!
//! - **A retained list.** A frame gathers its fighters' components from the world into a list it keeps from
//!   one frame to the next, empty ([`reuse_list`]), decides each one's level, prepares each one's work on
//!   the frame's thread in the roster's order (what reads the arena, the simulation and the timeline), runs
//!   the work, then hands what it made back in the roster's order: blades to hands, counts to the frame's
//!   stats. Once the list has held a frame's fighters, none of this allocates.
//! - **Jobs.** A frame with [`JOBS_FROM`] fighters to pose or more runs their work through `evaluate_all`, in
//!   jobs on the compute pool; fewer run one after another on the frame's thread, where they cost less than
//!   handing them out and allocate nothing. Each fighter's work writes only its own components, so the poses
//!   are the same either way, at any thread count. The job system allocates a few small blocks a call
//!   (`sapphire_jobs`' docs), bounded by its threads, not the crowd.
//! - **The simulation's own pair** (`Simulation::overhead_presentation_pair`, the fixed fighters) shows the
//!   bodies the fixed step solved, which it reads its hits from. Nothing here thins them: they publish
//!   every frame, as before. Neither does it thin the one on field, nor a fighter a timeline's track or a
//!   conversation's role names. Everyone else takes the tier's levels ([`sapphire_stage::anim_levels`]),
//!   which decide when presentation poses them and never what the simulation steps.

use std::time::Duration;

use sapphire_anim::{AnimStats, FootContact, Mouth, SpringWork, evaluate_all, reuse_list};
use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_gameplay::Simulation;
use sapphire_math::Mat4;
use sapphire_sequencer::CharacterCue;
use sapphire_stage::RenderPose;
use sapphire_stage::anim_graph::{ClipTable, FixedBodyRenderer, GraphDriver, GraphEvaluation};
use sapphire_stage::anim_levels::{AnimPace, LevelClock};
use sapphire_stage::animate::{SpringKey, SpringView};
use sapphire_stage::content::Content;
use sapphire_stage::scene::{Figure, Scene};
use sapphire_time::Stopwatch;
use sapphire_world::light_rig::CharacterLightRig;

use super::{FightShow, spring_key};
use crate::arena::{ArenaActors, FighterKind};
use crate::{faces, timeline};

/// How many fighters a frame poses before it hands them to jobs: fewer cost less on the frame's own thread
/// than handing them out, and allocate nothing at all.
pub(super) const JOBS_FROM: usize = 12;

/// One fighter's components for a frame, gathered from the world before any is posed.
pub(super) struct Gathered<'f> {
    index: usize,
    driver: Mut<'f, GraphDriver>,
    render: Mut<'f, RenderPose>,
    figure: Mut<'f, Figure>,
    rig: Mut<'f, CharacterLightRig>,
    /// Whether it can play into its pose: not for a moment while a new version of its mesh lands.
    fits: bool,
    /// What its role's track says, if a timeline's names it.
    cue: Option<CharacterCue>,
    pace: AnimPace,
}

/// What a fighter's job does this frame.
#[expect(
    clippy::large_enum_variant,
    reason = "the jobs live in a list kept from frame to frame, so boxing a graph's evaluation would \
              allocate for each fighter each frame, where the list's size costs nothing"
)]
pub(super) enum FighterJob<'f> {
    /// Its graph's frame, with its IK and springs.
    Graph { index: usize, evaluation: GraphEvaluation<'f>, clips: &'f ClipTable<'f> },
    /// The body the simulation solved for it, published, with its springs.
    Fixed {
        index: usize,
        renderer: &'f mut FixedBodyRenderer,
        driver: Mut<'f, GraphDriver>,
        render: Mut<'f, RenderPose>,
        figure: Mut<'f, Figure>,
        rig: Mut<'f, CharacterLightRig>,
        view: &'f SpringView,
        key: SpringKey,
        made: (bool, SpringWork, Duration),
    },
}

/// Whether a timeline's or a conversation's `role` names the fighter at `index`.
fn names(arena: &ArenaActors, role: Name, index: usize) -> bool {
    timeline::fighter_of_role(role, arena) == Some(index)
}

impl FighterJob<'_> {
    /// Runs it, touching nothing but its own fighter's components. Allocates nothing.
    fn run(&mut self) {
        match self {
            Self::Graph { evaluation, clips, .. } => evaluation.run(clips),
            Self::Fixed { renderer, driver, render, figure, rig, view, key, made, .. } => {
                *made = renderer.publish(driver, render, figure, rig, view, *key);
            }
        }
    }
}

impl FightShow {
    /// Poses each fighter for this frame (see the [module docs](self)): plays its graph from what the last
    /// step said of it, solves its feet and its look at who it's fighting, or publishes the body the
    /// simulation solved for it, and moves each warrior's blade to its hand. A fighter whose mesh is
    /// changing keeps the pose it has until it fits.
    pub(super) fn animate_fighters(
        &mut self,
        scene: &mut Scene,
        player: &ClipTable<'_>,
        enemies: &[ClipTable<'_>],
        alpha: f32,
        presentation: Option<(&Content, Option<&Simulation>)>,
    ) {
        profiling::scope!("animate fighters");
        let simulation = presentation.and_then(|(_, simulation)| simulation);
        let moves = simulation.map(Simulation::move_library);
        let warrior_versions = if self.fixed_fighters[1].is_some() {
            presentation
                .and_then(|(content, _)| self.enemy_versions(content))
                .map(|(mesh, skeleton)| (mesh, 0, skeleton))
        } else {
            None
        };
        let started = Stopwatch::start();
        let mut stats = AnimStats::default();
        let mouth = self.dialogue_faces.mouth(self.said);
        let face_dt = std::mem::take(&mut self.face_dt);
        let (jobs_on, levels) = (scene.anim_jobs(), scene.level_view());
        let mut view = scene.spring_view();
        let Self {
            arena: Some(arena),
            graph_players: Some(graph_players),
            timeline,
            timeline_generation,
            dialogue_faces,
            held_ticks,
            assets,
            fixed_fighters,
            impact,
            posing,
            fighter_jobs,
            ..
        } = self
        else {
            return;
        };
        let arena = &mut **arena;
        let (generation, held_ticks, kinds) = (*timeline_generation, *held_ticks, &assets.kinds);
        let timeline = timeline.as_ref();
        let world = scene.world_mut();

        let mut gathered: Vec<Gathered<'_>> = reuse_list(std::mem::take(posing));
        for (entity, driver, render, figure, rig, _) in graph_players.iter_mut(world) {
            let Some(index) = (0..arena.fighter_count()).find(|&index| arena.entity(index) == entity) else {
                continue;
            };
            let fits = driver.fits(&render);
            gathered.push(Gathered {
                index,
                driver,
                render,
                figure,
                rig,
                fits,
                cue: None,
                pace: AnimPace::FULL,
            });
        }
        gathered.sort_unstable_by_key(|fighter| fighter.index);

        // Each fighter's level, before anyone takes a place in the springs' rotation, since a fighter held
        // or with its springs out takes none.
        for fighter in &mut gathered {
            let index = fighter.index;
            fighter.cue = timeline.and_then(|show| show.cue_of_fighter(index, arena));
            let speaks = mouth.is_some_and(|(role, _)| names(arena, role, index))
                || dialogue_faces.held().any(|(role, _)| names(arena, role, index));
            let fixed = fixed_fighters.get(index).is_some_and(Option::is_some);
            let pinned = index == 0 || fixed || fighter.cue.is_some() || speaks;
            fighter.pace = match arena.clock_ticks(index) {
                Some(ticks) if !pinned => {
                    let clock = LevelClock {
                        tick: (ticks + held_ticks).max(0.0) as u64,
                        place: u32::try_from(index).unwrap_or(u32::MAX),
                        cut: arena.is_cut(index),
                    };
                    let place = arena.drawn(index, alpha);
                    levels.pace(fighter.driver.level_mut(), place, &fighter.figure, clock)
                }
                _ => {
                    fighter.driver.level_mut().pose_full();
                    AnimPace::FULL
                }
            };
        }
        // Everyone whose springs may step takes their place in the frame's rotation before anyone runs (M5 C2).
        for fighter in &gathered {
            let steps = !fighter.fits || matches!(fighter.pace, AnimPace::Update(level) if level.springs);
            if steps && fighter.driver.has_springs() {
                view.enter(spring_key(fighter.index, arena.kind(fighter.index)));
            }
        }
        let view = &view;

        let stick_released = arena
            .player_seen()
            .is_some_and(|actor| actor.controls.current.stick == sapphire_gameplay::Stick::CENTER);
        let mut fixed_slots = fixed_fighters.each_mut().map(Option::as_mut);
        let mut work: Vec<FighterJob<'_>> = reuse_list(std::mem::take(fighter_jobs));
        for fighter in gathered.drain(..) {
            let Gathered { index, mut driver, render, figure, rig, fits, cue, pace } = fighter;
            if !fits {
                continue;
            }
            let drawn = arena.kind(index);
            let clips = match drawn {
                FighterKind::Playable => player,
                FighterKind::Enemy(kind) => {
                    let slot = kinds.get(kind).map_or(0, |kind| kind.graph_slot);
                    enemies.get(slot).unwrap_or(&enemies[0])
                }
            };
            // What its role's track says, a timeline's face before a conversation's (M5 C5).
            if let Some(face) = driver.face_mut() {
                let order = cue.and_then(|cue| cue.face).map(|face| faces::timeline_order(generation, face));
                let said = || {
                    dialogue_faces.held().find(|&(role, _)| names(arena, role, index)).map(|(_, order)| order)
                };
                faces::drive(face, face_dt, order.or_else(said));
                // The voiced line's role's mouth (M5 C6); every other's shut.
                let speaking = mouth.filter(|&(role, _)| names(arena, role, index));
                face.set_mouth(speaking.map_or(Mouth::CLOSED, |(_, mouth)| mouth));
            }
            let level = match pace {
                AnimPace::Update(level) => level,
                AnimPace::Hold { off_screen } => {
                    stats.characters += 1;
                    stats.held += 1;
                    stats.thinned += 1;
                    stats.off_screen += u32::from(off_screen);
                    if off_screen {
                        driver.rest_off_screen();
                    }
                    continue;
                }
            };
            if let Some(fixed) = fixed_slots.get_mut(index).and_then(Option::take) {
                let (content, _) = presentation.expect("fixed body presentation needs its source");
                let versions = match drawn {
                    FighterKind::Playable => {
                        content.character_kind().and_then(|kind| content.playable_versions(kind))
                    }
                    FighterKind::Enemy(_) => warrior_versions,
                };
                fixed.renderer.check_source(clips, versions);
                let simulation = simulation.expect("simulation body presentation needs its producer");
                let ready = if let Some(impact) = impact.as_mut()
                    && impact.is_held(simulation)
                {
                    let ready = fixed.renderer.read_impact(&impact.samples[index]);
                    impact.presented[index] |= ready;
                    ready
                } else {
                    let ready = fixed.renderer.read_simulation(simulation, fixed.actor, alpha);
                    if ready
                        && let Some(impact) = impact.as_ref()
                        && let Some(release) = &impact.release
                        && release.valid(impact.actors.map(|actor| simulation.body_stamp(actor)))
                    {
                        fixed.renderer.release_impact(&impact.samples[index], &release.origins[index]);
                    }
                    ready
                };
                if ready {
                    stats.characters += 1;
                    let key = spring_key(index, drawn);
                    let renderer = &mut fixed.renderer;
                    let made = (false, SpringWork::default(), Duration::ZERO);
                    work.push(FighterJob::Fixed {
                        index,
                        renderer,
                        driver,
                        render,
                        figure,
                        rig,
                        view,
                        key,
                        made,
                    });
                    continue;
                }
                if simulation.tick() != 0 {
                    stats.characters += 1;
                    continue;
                }
                // The title and a freshly reset arena can draw before the first body step.
                // At tick zero the ordinary graph poses the idle fighters without advancing combat.
                driver.player.pose_mut().clear_body_presentation();
            }
            let gather = index == 0 && stick_released && driver.has_ik();
            driver.player.request_grounded_gather(gather);
            // A level past the nearest poses on a whole tick, the latest its clock has reached; the nearest
            // between ticks, where transforms draw.
            let at = if level.interval <= 1 { alpha } else { 1.0 };
            // After that, since it spends a cut and a landing, which wait for a frame the graph plays.
            let Some(mut input) = arena.graph_input(index, at) else { continue };
            // A timeline's character track plays its row in the combat slot over whatever the simulation
            // says (M3 Q3), on a clock that goes on through a cutscene that holds the simulation's.
            input.ticks += held_ticks;
            // Only a fighter a role names has a track: `hero`, `foe`, or the first of a kind of enemy by its
            // name (M5 C5). A crowd's others play what the simulation says (the whole walk's ultimate among
            // three found this, M3 G1c).
            if let Some(combat) = cue.as_ref().and_then(timeline::staged_row) {
                input.combat = Some(combat);
            }
            stats.characters += 1;
            stats.thinned += u32::from(level.is_thinned());
            let place = arena.drawn(index, at);
            if let Some(ik) = driver.ik_mut() {
                // Feet and look rest past the tier's band for them, and come back over their fades.
                ik.set_resting(!level.ik);
                // A track's look key over who it's fighting (M5 C5).
                ik.set_look_target(cue.and_then(|cue| cue.look).or_else(|| arena.look_target(index, at)));
                // Read plants in the move's own root space so extra gameplay travel
                // cannot turn a stationary shoe into a skating shoe.
                ik.set_foot_contact(if input.locomotion.grounded && !input.defeated {
                    if input.combat.is_none() {
                        // Both fighters author heel and forefoot rolls. Pin the
                        // loaded sole landmark rather than the ankle between them.
                        FootContact::RollingStance
                    } else {
                        moves
                            .zip(input.combat)
                            .and_then(|(moves, combat)| {
                                arena.authored_foot_motion(index, combat.position, moves)
                            })
                            .map_or(FootContact::Sole, FootContact::Authored)
                    }
                } else {
                    FootContact::Sole
                });
            }
            let key = spring_key(index, drawn);
            let evaluation = GraphEvaluation::new(driver, render, figure, rig, input, place, view, key);
            work.push(FighterJob::Graph { index, evaluation, clips });
        }

        if jobs_on && work.len() >= JOBS_FROM {
            stats.jobs = u32::try_from(evaluate_all(&mut work, FighterJob::run)).unwrap_or(u32::MAX);
        } else {
            work.iter_mut().for_each(FighterJob::run);
        }

        for job in work.drain(..) {
            match job {
                FighterJob::Graph { index, evaluation, .. } => {
                    stats.teleports += evaluation.driver.player.teleports();
                    if evaluation.evaluated {
                        stats.evaluated += 1;
                        stats.joints += evaluation.driver.player.pose().joint_count() as u32;
                        stats.ik += u32::from(evaluation.driver.has_ik());
                        if let Some(hand) = arena.blade_hand(index) {
                            let hand = evaluation.driver.player.pose().model()[hand];
                            arena.hold_blade(index, Mat4::from(hand));
                        }
                    }
                    stats.springs += u32::from(evaluation.spring_work.wrote_pose);
                    stats.spring_substeps += evaluation.spring_work.substeps;
                    stats.spring_time += evaluation.spring_time;
                }
                FighterJob::Fixed { index, driver, made: (evaluated, springs, elapsed), .. } => {
                    if evaluated {
                        stats.evaluated += 1;
                        stats.joints += driver.player.pose().joint_count() as u32;
                        if index == 1
                            && let Some(hand) = arena.blade_hand(index)
                        {
                            let hand = driver.player.pose().model()[hand];
                            arena.hold_blade(index, Mat4::from(hand));
                        }
                    }
                    stats.springs += u32::from(springs.wrote_pose);
                    stats.spring_substeps += springs.substeps;
                    stats.spring_time += elapsed;
                }
            }
        }
        *posing = reuse_list(gathered);
        *fighter_jobs = reuse_list(work);
        stats.time = started.elapsed();
        scene.set_anim_stats(stats);
    }
}
