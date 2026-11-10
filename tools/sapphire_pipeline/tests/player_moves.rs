//! The player's moves (M2 O3, `content/combat/player/`), read the way a cook reads them: each key-pose
//! document sampled for the shipped character, and `player.moves.ron` baked from those samples and
//! played in the simulation. What these hold is the shape a move is meant to have, not whether it
//! looks good: only eyes can judge that, and the plan's feel reviews do.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use glam::{Affine3A, Quat, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::rig::{CharacterRig, HumanBone};
use sapphire_assets::skeleton::{NO_PARENT, Skeleton};
use sapphire_core::Name;
use sapphire_gameplay::replay::compile_document;
use sapphire_gameplay::{ActorId, ActorSpawn, CurrentMove, SimConfig, Simulation, TickActions, WarpTarget};
use sapphire_moves::{ClipMotionDocument, Move, MoveDocument, MoveSet, MoveSetDocument, MoveSetError, Phase};
use sapphire_pipeline::clip::SampledClip;
use sapphire_pipeline::clip::retarget::HumanMap;
use sapphire_pipeline::import::vrm;
use sapphire_pipeline::keypose::{self, KeyPoseClip};
use sapphire_pipeline::{meta, moves};

/// What a hit forces, with no clip of their own: the character's graph draws them.
const REACTIONS: [&str; 5] = ["flinch", "stagger", "knockdown", "dazed", "defeat"];
/// How far along its +Y the blade path's middle is from either end of the blade.
const HALF_BLADE: f32 = 0.45;
/// The ball of the foot's height over the sole, at rest.
const BALL_HEIGHT: f32 = 0.038;
/// The bottom of the heel, from the ankle, at rest.
const HEEL_FROM_ANKLE: Vec3 = Vec3::new(0.0, -0.101, -0.095);

fn content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

fn name(text: &str) -> Name {
    Name::intern(text)
}

/// A document and the pose it makes at every one of its ticks.
struct Clip {
    document: KeyPoseClip,
    sampled: SampledClip,
    /// Each joint's model transform, tick by tick.
    model: Vec<Vec<Affine3A>>,
    /// Its sidecar's ID.
    id: AssetId,
}

impl Clip {
    fn ticks(&self) -> usize {
        usize::from(self.sampled.ticks)
    }

    fn root(&self, tick: usize) -> (Vec3, Quat) {
        let root = self.sampled.root.as_ref().expect("every move travels");
        (root.translation[tick], root.rotation[tick])
    }

    /// Where a point of the model is in the move's start space.
    fn in_world(&self, tick: usize, point: Vec3) -> Vec3 {
        let (at, turn) = self.root(tick);
        at + turn * point
    }

    fn joint(&self, tick: usize, joint: usize) -> Vec3 {
        Vec3::from(self.model[tick][joint].translation)
    }

    /// The ball of a foot, in the move's start space.
    fn ball(&self, tick: usize, toes: usize) -> Vec3 {
        self.in_world(tick, self.joint(tick, toes))
    }

    /// The lowest point of a foot's sole, ball or heel, over the ground.
    fn sole(&self, tick: usize, foot: usize, toes: usize) -> f32 {
        let heel = self.model[tick][foot].transform_point3(HEEL_FROM_ANKLE);
        (self.joint(tick, toes).y - BALL_HEIGHT).min(heel.y)
    }
}

/// The character and everything read from `content/combat/player/`.
struct Player {
    rig: CharacterRig,
    skeleton: Skeleton,
    /// Each key-pose document a move plays, by its file's name: the move's own.
    clips: BTreeMap<String, Clip>,
    document: MoveSetDocument,
    set: MoveSet,
    /// The moves with a clip of their own, `<name>.keypose.ron`, in the document's order. A party's assist
    /// plays another's (M2 G7), and a reaction none.
    moves: Vec<String>,
    /// Those of them with a hit.
    attacks: Vec<String>,
}

impl Player {
    fn bone(&self, bone: HumanBone) -> usize {
        usize::from(self.rig.human_joint(bone).expect("the character has it"))
    }

    fn clip(&self, name: &str) -> &Clip {
        &self.clips[name]
    }

    fn mv(&self, mv: &str) -> &Move {
        self.set.find(name(mv)).unwrap_or_else(|| panic!("{mv} is a move")).1
    }
}

fn model_of(sampled: &SampledClip, skeleton: &Skeleton, tick: usize) -> Vec<Affine3A> {
    let mut model: Vec<Affine3A> = Vec::with_capacity(skeleton.rest.len());
    for joint in 0..skeleton.rest.len() {
        let rest = &skeleton.rest[joint];
        let track = |tracks: &[(u16, Vec<Vec3>)]| {
            tracks.iter().find(|(at, _)| usize::from(*at) == joint).map(|(_, samples)| samples[tick])
        };
        let rotation = sampled
            .rotations
            .iter()
            .find(|(at, _)| usize::from(*at) == joint)
            .map_or(Quat::from_array(rest.rotation).normalize(), |(_, samples)| samples[tick]);
        let local = Affine3A::from_scale_rotation_translation(
            Vec3::from(rest.scale),
            rotation,
            track(&sampled.translations).unwrap_or(Vec3::from(rest.translation)),
        );
        let parent = skeleton.parents[joint];
        model.push(if parent == NO_PARENT { local } else { model[usize::from(parent)] * local });
    }
    model
}

fn player() -> &'static Player {
    static PLAYER: OnceLock<Player> = OnceLock::new();
    PLAYER.get_or_init(|| {
        let folder = content().join("combat/player");
        let vrm_path = content().join("lookdev/character/character.vrm");
        let vrm_bytes = std::fs::read(vrm_path).expect("the character");
        let rig = vrm::import_rig(&vrm_bytes, None).expect("the rig");
        let imported = vrm::import(&vrm_bytes, None).expect("the character");
        let skeleton = Skeleton::new(&imported.mesh.skin.as_ref().expect("skinned").joints);
        let map = HumanMap::from_rig(&rig);

        let text = std::fs::read_to_string(folder.join("player.moves.ron")).expect("the moves");
        let document = MoveSetDocument::parse(&text).expect("they parse");
        let with_a_clip = document.moves.iter().filter(|mv| mv.motion.clip.is_some());
        let own = |mv: &&MoveDocument| folder.join(format!("{}.keypose.ron", mv.name)).is_file();
        let moves: Vec<String> = with_a_clip.filter(own).map(|mv| mv.name.clone()).collect();
        let attacks = document.moves.iter().filter(|mv| moves.contains(&mv.name) && !mv.hitboxes.is_empty());
        let attacks: Vec<String> = attacks.map(|mv| mv.name.clone()).collect();

        let mut clips = BTreeMap::new();
        for mv in &moves {
            let path = folder.join(format!("{mv}.keypose.ron"));
            let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{mv}: {error}"));
            let document = keypose::read(&bytes).unwrap_or_else(|error| panic!("{mv}: {error:?}"));
            let sampled = keypose::sample(&document, mv, &skeleton, &map, &[])
                .unwrap_or_else(|error| panic!("{mv}: {error:?}"));
            let model =
                (0..=usize::from(sampled.ticks)).map(|tick| model_of(&sampled, &skeleton, tick)).collect();
            let id = meta::read(&meta::sidecar_path(&path)).expect("its sidecar").id;
            clips.insert(mv.clone(), Clip { document, sampled, model, id });
        }

        let set = cook_against(&document, &clips, &skeleton, &rig).expect("it compiles against its clips");
        Player { rig, skeleton, clips, document, set, moves, attacks }
    })
}

/// The key-pose document whose ID a move's `source` names, among `clips`.
fn played_by<'a>(clips: &'a BTreeMap<String, Clip>, motion: &ClipMotionDocument) -> Option<&'a Clip> {
    clips.values().find(|clip| clip.id.to_string() == motion.source)
}

/// `document` baked from `clips`, as a cook bakes it.
fn cook_against(
    document: &MoveSetDocument,
    clips: &BTreeMap<String, Clip>,
    skeleton: &Skeleton,
    rig: &CharacterRig,
) -> Result<MoveSet, MoveSetError> {
    let map = HumanMap::from_rig(rig);
    document.compile_with(|mv| {
        let Some(motion) = mv.motion.clip.as_ref() else { return Ok(None) };
        let clip = played_by(clips, motion).ok_or("its `source` is a move's key-pose document")?;
        let input = moves::TableInput {
            sampled: &clip.sampled,
            skeleton,
            map: Some(&map),
            constraints: &rig.constraints,
        };
        moves::bake(motion, &input).map(Some).map_err(|error| error.to_string())
    })
}

// ---- Live tuning (M2 T2): what a running app, with no clips to bake from, makes of the document.

fn bytes(set: &MoveSet) -> Vec<u8> {
    set.to_cooked().expect("writes").into_vec()
}

/// The document as a person has it, comments and all.
fn moves_text() -> String {
    std::fs::read_to_string(content().join("combat/player/player.moves.ron")).expect("the moves")
}

/// `light_1`'s cut a tick later, in the file as a person edits it, with the move as long as its clip.
fn a_later_first_cut() -> String {
    let text = moves_text();
    let phases = "startup: 7,\n                active: 4,\n                recovery: 23,";
    let later = "startup: 8,\n                active: 4,\n                recovery: 22,";
    let hit = "from: 7,\n                        to: 11,";
    let later_hit = "from: 8,\n                        to: 12,";
    assert!(text.contains(phases) && text.contains(hit), "the edit is still there to make");
    text.replacen(phases, later, 1).replacen(hit, later_hit, 1)
}

/// What exit test 2 asks of the real moves: a saved timing needs no cook, and is what a cook makes of it.
#[test]
fn a_saved_timing_is_what_a_cook_makes_with_no_cook() {
    let player = player();
    let (clips, skeleton, rig) = (&player.clips, &player.skeleton, &player.rig);

    let unchanged = compile_document(&moves_text(), Some(&player.set)).expect("no cook needed");
    assert!(unchanged.motion_as_cooked);
    assert_eq!(bytes(&unchanged.set), bytes(&player.set), "saving it unedited is no edit");

    let edited = a_later_first_cut();
    let live = compile_document(&edited, Some(&player.set)).expect("no cook needed");
    let cooked = cook_against(&MoveSetDocument::parse(&edited).expect("parses"), clips, skeleton, rig);
    assert_eq!(bytes(&live.set), bytes(&cooked.expect("it cooks")));
    assert_ne!(bytes(&live.set), bytes(&player.set));
    let (_, light) = live.set.find(name("light_1")).expect("there");
    assert_eq!(light.startup, 8);
    assert_eq!(light.hitboxes[0].span.from, 8);
    assert_eq!(light.tables.ticks(), 34, "its clip's length");
}

/// The same for every move at once, through the text the timeline editor writes: it reads back whole.
#[test]
fn every_moves_timing_can_be_saved_with_no_cook() {
    let player = player();
    let (clips, skeleton, rig) = (&player.clips, &player.skeleton, &player.rig);
    let written = player.document.to_text().expect("writes");
    let same = compile_document(&written, Some(&player.set)).expect("reads back");
    assert_eq!(bytes(&same.set), bytes(&player.set));

    let mut document = player.document.clone();
    for mv in document.moves.iter_mut().filter(|mv| !REACTIONS.contains(&mv.name.as_str())) {
        assert!(mv.motion.clip.is_some(), "{}: the route this tests", mv.name);
        assert!(mv.recovery > 1, "{}: there's a tick to move", mv.name);
        mv.startup += 1;
        mv.recovery -= 1;
        for hitbox in &mut mv.hitboxes {
            hitbox.from += 1;
            hitbox.to += 1;
        }
    }
    let text = document.to_text().expect("writes");
    let live = compile_document(&text, Some(&player.set)).expect("no cook needed");
    let cooked = cook_against(&document, clips, skeleton, rig).expect("it cooks");
    assert_eq!(bytes(&live.set), bytes(&cooked));
}

/// What only a cook can do says so, in the words a cook uses where it can. An edit of a `clip` block is the
/// one thing a running app can't see, and its reply says motion stays as last cooked: that's held here too.
#[test]
fn what_a_saved_document_cant_change_waits_for_a_cook() {
    let player = player();
    let (clips, skeleton, rig) = (&player.clips, &player.skeleton, &player.rig);
    let text = moves_text();

    // A longer move is a longer clip.
    let longer = text.replacen("recovery: 23,", "recovery: 24,", 1);
    let refused = compile_document(&longer, Some(&player.set)).expect_err("its clip is shorter").to_string();
    let parsed = MoveSetDocument::parse(&longer).expect("parses");
    let cook_says = cook_against(&parsed, clips, skeleton, rig).expect_err("so says a cook").to_string();
    assert_eq!(refused, cook_says);
    assert!(refused.contains("light_1") && refused.contains("the clip is 34 ticks long"), "{refused}");

    // A new path has no table here.
    let mut path = player.document.clone();
    let clip = path.moves[0].motion.clip.as_mut().expect("a clip");
    clip.paths[0].name = "off_hand".to_owned();
    path.moves[0].hitboxes[0].path = "off_hand".to_owned();
    let refused = path.compile_over(&player.set).expect_err("no such table").to_string();
    assert!(refused.contains("path `off_hand`: only a cook"), "{refused}");

    // A moved socket isn't seen until a cook, whose bytes are then a new revision's.
    let mut socket = player.document.clone();
    socket.moves[0].motion.clip.as_mut().expect("a clip").paths[0].offset[2] += 0.1;
    let live = compile_document(&socket.to_text().expect("writes"), Some(&player.set)).expect("reads");
    assert!(live.motion_as_cooked);
    assert_eq!(bytes(&live.set), bytes(&player.set));
    let cooked = cook_against(&socket, clips, skeleton, rig).expect("it cooks");
    assert_ne!(bytes(&cooked), bytes(&player.set));
}

// ---- What each clip has to be for the move document.

/// A move's clip has its name and its ID, and is as long as the move's phases: a cook refuses less,
/// and this says so in the plain words of what a person edits. Every move in `player.moves.ron` is held:
/// one with a `<name>.keypose.ron` beside it plays that, an assist plays another move's (M2 G7), and only
/// a reaction a hit forces has none.
#[test]
fn each_move_plays_the_clip_of_its_name() {
    let player = player();
    assert!(!player.attacks.is_empty(), "the document's moves: {:?}", player.moves);
    for mv in &player.document.moves {
        if REACTIONS.contains(&mv.name.as_str()) {
            assert!(mv.motion.clip.is_none(), "{}: a graph draws it, so nothing bakes from a clip", mv.name);
            continue;
        }
        let motion = mv.motion.clip.as_ref().unwrap_or_else(|| {
            panic!(
                "{}: a move names a clip, `<name>.keypose.ron` beside the document, or is a reaction",
                mv.name
            )
        });
        let clip = played_by(&player.clips, motion)
            .unwrap_or_else(|| panic!("{}: its `source` is no move's key-pose document", mv.name));
        if let Some(own) = player.clips.get(&mv.name) {
            assert_eq!(motion.source, own.id.to_string(), "{}: its `source` is its document", mv.name);
        }
        assert_eq!(
            (mv.startup + mv.active + mv.recovery) as usize,
            clip.ticks(),
            "{}: frame data and clip",
            mv.name
        );
    }
}

/// The simulation's own copy of the cooked set (`sapphire_gameplay::replay::player_moves`), which a
/// replay reads with no cooker to make it. A cook of the document makes those bytes, on every
/// architecture, so a replay's `tables_hash` holds across them. `SAPPHIRE_BAKE_MOVES=1` writes them.
#[test]
fn the_simulations_copy_of_the_set_is_what_a_cook_makes() {
    let player = player();
    let map = HumanMap::from_rig(&player.rig);
    let text = std::fs::read(content().join("combat/player/player.moves.ron")).expect("the moves");
    let cooked = moves::cook(&text, |motion| {
        let clip = player
            .clips
            .values()
            .find(|clip| clip.id.to_string() == motion.source)
            .expect("its `source` is one of the documents");
        moves::bake(
            motion,
            &moves::TableInput {
                sampled: &clip.sampled,
                skeleton: &player.skeleton,
                map: Some(&map),
                constraints: &player.rig.constraints,
            },
        )
    })
    .expect("the document cooks");
    let copy =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/sapphire_gameplay/baked/player.smoves");
    if std::env::var_os("SAPPHIRE_BAKE_MOVES").is_some() {
        std::fs::write(&copy, &cooked).expect("writes");
    }
    let held = std::fs::read(&copy).expect("the simulation's copy");
    assert!(
        held == cooked,
        "the document moved: SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy, \
         then cargo xtask replay --update"
    );
}

/// `blade` is the same socket in every move, so one blade can stand for all of them.
#[test]
fn every_attack_cuts_along_one_blade() {
    let player = player();
    let sockets: Vec<_> = player
        .document
        .moves
        .iter()
        .filter(|mv| player.attacks.contains(&mv.name))
        .flat_map(|mv| mv.motion.clip.as_ref().expect("a clip").paths.iter())
        .map(|path| (path.name.clone(), path.follows.clone(), path.offset, path.rotation))
        .collect();
    assert_eq!(sockets.len(), player.attacks.len(), "one path each: {sockets:?}");
    assert!(sockets.iter().all(|socket| *socket == sockets[0] && socket.0 == "blade"), "{sockets:?}");
}

/// Pillar 1 (`docs/m2-plan.md` §2): a press to the first visibly moving frame in at most 3 ticks. A
/// press starts a move on its step, so its clip's tick 2 is the third frame.
#[test]
fn a_press_moves_the_character_within_three_ticks() {
    let player = player();
    for mv in &player.moves {
        let clip = player.clip(mv);
        let moved = (0..player.skeleton.rest.len())
            .map(|joint| {
                clip.in_world(2, clip.joint(2, joint)).distance(clip.in_world(0, clip.joint(0, joint)))
            })
            .fold(0.0, f32::max);
        assert!(moved > 0.08, "{mv}: moved {moved} m by tick 2");
    }
}

/// The blade's tip in the move's start space: along the path the hit rides.
fn tip(player: &Player, mv: &str, tick: usize) -> Vec3 {
    let table = &player.mv(mv).tables;
    let root = table.root_samples()[tick];
    let path = table.path(name("blade")).expect("it has a blade").samples()[tick];
    let point = Vec3::from(path.translation) + Quat::from_array(path.rotation) * (Vec3::Y * HALF_BLADE);
    Vec3::from(root.translation) + Quat::from_rotation_y(root.yaw) * point
}

/// Contact agrees with the document: whatever else moves, the blade is at its fastest in the hit, and
/// fast enough there to read as a cut.
#[test]
fn the_blade_is_fastest_on_the_hit() {
    let player = player();
    for mv in &player.attacks {
        let found = player.mv(mv);
        let speed: Vec<f32> = (0..found.ticks() as usize)
            .map(|tick| tip(player, mv, tick + 1).distance(tip(player, mv, tick)))
            .collect();
        let fastest = (0..speed.len()).max_by(|a, b| speed[*a].total_cmp(&speed[*b])).expect("ticks");
        let (first, last) = (found.startup as usize, (found.startup + found.active) as usize);
        assert!(
            (first..last).contains(&fastest),
            "{mv}: fastest at {fastest}, hits on {first}..{last}: {speed:?}"
        );
        for hit in found.hitboxes.iter() {
            let (from, to) = (hit.span.from as usize, hit.span.to as usize);
            let fastest = speed[from..to].iter().copied().fold(0.0, f32::max);
            assert!(fastest > 0.25, "{mv}: {:?} peaks at {fastest} m a tick", hit.name());
        }
    }
}

/// Each cut comes through in front of the character, far enough to be worth stepping in for.
#[test]
fn every_hit_reaches_forward() {
    let player = player();
    for mv in &player.attacks {
        let found = player.mv(mv);
        let table = &found.tables;
        for hit in found.hitboxes.iter() {
            let reach = (hit.span.from..hit.span.to)
                .map(|tick| {
                    let path = table.path(name("blade")).expect("a blade").samples()[tick as usize];
                    let point = Vec3::from(path.translation)
                        + Quat::from_array(path.rotation) * (Vec3::Y * HALF_BLADE);
                    // Ahead of where the character stands, as it faced when the move began.
                    (Quat::from_rotation_y(table.root_samples()[tick as usize].yaw) * point).z
                })
                .fold(f32::MIN, f32::max);
            assert!(reach > 0.9, "{mv}: {:?} reaches {reach} m ahead", hit.name());
        }
    }
}

// ---- Bodies.

/// How far each bone may turn from rest at a key, in degrees: generous, to catch a broken joint rather
/// than a stylish one. A key's the furthest a bone goes, as bones blend the short way between keys.
fn limit(bone: HumanBone) -> f32 {
    use HumanBone::*;
    match bone {
        Hips => 62.0,
        Spine => 20.0,
        Chest => 30.0,
        UpperChest => 24.0,
        Neck => 45.0,
        Head => 55.0,
        LeftShoulder | RightShoulder => 20.0,
        LeftUpperArm | RightUpperArm => 170.0,
        LeftLowerArm | RightLowerArm => 150.0,
        LeftHand | RightHand => 180.0,
        LeftUpperLeg | RightUpperLeg => 125.0,
        LeftLowerLeg | RightLowerLeg => 145.0,
        LeftFoot | RightFoot => 90.0,
        LeftToes | RightToes => 70.0,
        // Fingers.
        _ => 110.0,
    }
}

#[test]
fn no_joint_leaves_human_limits() {
    let player = player();
    for mv in &player.moves {
        for key in &player.clip(mv).document.keys {
            for (bone, turn) in &key.pose {
                let degrees = 2.0 * turn.quat().w.abs().min(1.0).acos().to_degrees();
                assert!(
                    degrees <= limit(*bone),
                    "{mv} at {}: {bone:?} turns {degrees} degrees, past {}",
                    key.tick,
                    limit(*bone)
                );
            }
            // The hinges only bend one way. Knees: +X. Right elbow: +Y; left: -Y.
            let hinge = |bone: HumanBone| key.pose.get(&bone).copied();
            for bone in [HumanBone::LeftLowerLeg, HumanBone::RightLowerLeg] {
                if let Some(turn) = hinge(bone) {
                    assert!(turn.x >= -5.0, "{mv} at {}: {bone:?} bends backward: {turn:?}", key.tick);
                }
            }
            if let Some(turn) = hinge(HumanBone::RightLowerArm) {
                assert!(turn.y >= -5.0, "{mv} at {}: the right elbow: {turn:?}", key.tick);
            }
            if let Some(turn) = hinge(HumanBone::LeftLowerArm) {
                assert!(turn.y <= 5.0, "{mv} at {}: the left elbow: {turn:?}", key.tick);
            }
        }
    }
}

/// Root motion stays on the ground and doesn't jump: a controller can follow it.
#[test]
fn root_motion_doesnt_jump() {
    let player = player();
    for mv in &player.moves {
        let clip = player.clip(mv);
        for tick in 1..=clip.ticks() {
            let (at, turn) = clip.root(tick);
            let (was, was_turned) = clip.root(tick - 1);
            assert_eq!(at.y, 0.0, "{mv} at {tick}: the controller holds the ground");
            let step = at.distance(was);
            assert!(step < 0.42, "{mv} at {tick}: {step} m in a tick");
            let turned = (was_turned.inverse() * turn).w.abs().min(1.0).acos().to_degrees() * 2.0;
            assert!(turned < 50.0, "{mv} at {tick}: {turned} degrees in a tick");
        }
        let (start, turned) = clip.root(0);
        assert_eq!((start, turned), (Vec3::ZERO, Quat::IDENTITY), "{mv} starts on the identity");
    }
}

/// Feet don't go through the ground, and every move starts and ends with both on it.
#[test]
fn feet_keep_to_the_ground() {
    let player = player();
    let feet = [
        (player.bone(HumanBone::LeftFoot), player.bone(HumanBone::LeftToes)),
        (player.bone(HumanBone::RightFoot), player.bone(HumanBone::RightToes)),
    ];
    for mv in &player.moves {
        let clip = player.clip(mv);
        for tick in 0..=clip.ticks() {
            for (foot, toes) in feet {
                let sole = clip.sole(tick, foot, toes);
                assert!(sole > -0.04, "{mv} at {tick}: a sole {sole} m under the ground");
            }
        }
        for tick in [0, clip.ticks()] {
            for (foot, toes) in feet {
                let sole = clip.sole(tick, foot, toes);
                assert!(sole < 0.06, "{mv} at {tick}: a sole {sole} m off the ground");
            }
        }
    }
}

/// Where a document says a foot bears weight, the ball of that foot stays put in the world, whatever
/// the root does: what a foot sliding on the ground would break. `bones` are the left and right toes.
#[test]
fn a_foot_bearing_weight_stays_put() {
    use HumanBone::{LeftToes as L, RightToes as R};
    const PLANTED: [(&str, HumanBone, usize, usize); 14] = [
        ("light_1", L, 0, 11),
        ("light_1", R, 11, 34),
        ("light_2", R, 0, 11),
        ("light_2", L, 11, 36),
        ("light_3", R, 0, 9),
        ("light_3", L, 19, 52),
        ("dash_attack", L, 14, 34),
        ("dodge", L, 0, 2),
        ("dodge", R, 19, 24),
        ("skill", L, 8, 21),
        ("skill", L, 31, 52),
        // The ultimate: both feet down while the hands gather and hold, then the finisher's landing.
        ("ultimate", L, 12, 54),
        ("ultimate", R, 12, 54),
        ("ultimate", L, 65, 144),
    ];
    let player = player();
    for (mv, bone, from, to) in PLANTED {
        let clip = player.clip(mv);
        let toes = player.bone(bone);
        let start = clip.ball(from, toes);
        for tick in from..=to {
            let slid = (clip.ball(tick, toes) - start) * Vec3::new(1.0, 0.0, 1.0);
            assert!(slid.length() < 0.05, "{mv}: {bone:?} slid {} m by {tick}, from {from}", slid.length());
        }
    }
}

/// Each hit starts where the last one's cancel window leaves the body, so a string flows rather than
/// blends: every joint of the arm, the legs and the spine within a few centimetres of where the next
/// clip starts, once the last has settled, and not far off at the window's first tick.
#[test]
fn each_hit_starts_where_the_last_settles() {
    use HumanBone::*;
    let player = player();
    let bones = [
        Hips,
        Chest,
        Head,
        LeftHand,
        RightHand,
        LeftLowerArm,
        RightLowerArm,
        LeftFoot,
        RightFoot,
        LeftLowerLeg,
        RightLowerLeg,
    ];
    for (mv, next, settled, window) in [("light_1", "light_2", 22, 14), ("light_2", "light_3", 22, 14)] {
        let (from, to) = (player.clip(mv), player.clip(next));
        for bone in bones {
            let joint = player.bone(bone);
            let apart = from.joint(settled, joint).distance(to.joint(0, joint));
            assert!(apart < 0.02, "{mv} at {settled} to {next}: {bone:?} {apart} m apart");
            let apart = from.joint(window, joint).distance(to.joint(0, joint));
            assert!(apart < 0.25, "{mv} at {window} to {next}: {bone:?} {apart} m apart");
        }
    }
}

// ---- The document, played.

fn lone() -> (Simulation, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    let moves =
        sim.add_move_set(MoveSet::from_cooked(&player().set.to_cooked().expect("cooks")).expect("reads"));
    let actor = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    // Full, as a built-in fight starts, so the ultimate can be pressed (M3 Q3c).
    assert!(sim.fill_energy(actor), "the player has a meter");
    (sim, actor)
}

fn current(sim: &Simulation, actor: ActorId) -> Option<CurrentMove> {
    sim.actor(actor).expect("alive").moves.current
}

fn step(sim: &mut Simulation) {
    sim.step(&TickActions::NEUTRAL);
}

/// Presses `intent`, steps once, and says the move that started.
fn press(sim: &mut Simulation, actor: ActorId, intent: &str) -> Option<CurrentMove> {
    assert!(sim.press_intent(actor, name(intent)), "{intent} is an intent");
    step(sim);
    current(sim, actor)
}

/// Steps until `mv` is at `tick`.
fn run_to(sim: &mut Simulation, actor: ActorId, mv: &str, tick: u64) {
    for _ in 0..400 {
        let running = current(sim, actor).unwrap_or_else(|| panic!("{mv} stopped before {tick}"));
        assert_eq!(running.name, name(mv), "at {tick}");
        if running.tick() >= tick {
            return;
        }
        step(sim);
    }
    panic!("{mv} never reached {tick}");
}

/// Pressing as fast as the buttons go runs the whole string at its fastest: every hit starts as soon as
/// the last one's window opens, and the finisher plays out.
#[test]
fn mashing_runs_the_string() {
    let (mut sim, actor) = lone();
    let mut started: Vec<(Name, u32)> = Vec::new();
    for tick in 0..200 {
        sim.press_intent(actor, name("light"));
        step(&mut sim);
        let running = current(&sim, actor).expect("always in a move");
        if started.last().map(|(name, _)| *name) != Some(running.name) {
            started.push((running.name, tick));
        }
        if started.len() == 4 {
            break;
        }
    }
    let names: Vec<Name> = started.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, [name("light_1"), name("light_2"), name("light_3"), name("light_1")], "{started:?}");
    // A press from neutral reads its tick 1 on the step it lands; a cancel starts at the instant the
    // window opens. Either way the next hit starts as soon as it's allowed to.
    let window =
        |mv: &str| player().mv(mv).cancels.iter().map(|window| window.span.from).max().expect("a window");
    assert_eq!(started[1].1 - started[0].1, window("light_1") - 1, "{started:?}");
    assert_eq!(started[2].1 - started[1].1, window("light_2"), "{started:?}");
    assert_eq!(started[3].1 - started[2].1, window("light_3"), "{started:?}");
}

/// A press any time after the move starts and before its window closes isn't dropped: it's what lets a
/// player mash on the beat rather than exactly in the window (pillar 1).
#[test]
fn a_press_during_a_cut_carries_into_the_next() {
    for (mv, next, closes) in [("light_1", "light_2", 28), ("light_2", "light_3", 30)] {
        for tick in 1..closes {
            let (mut sim, actor) = lone();
            if mv == "light_1" {
                press(&mut sim, actor, "light");
            } else {
                press(&mut sim, actor, "light");
                run_to(&mut sim, actor, "light_1", 14);
                press(&mut sim, actor, "light");
            }
            run_to(&mut sim, actor, mv, tick);
            sim.press_intent(actor, name("light"));
            for _ in 0..40 {
                step(&mut sim);
                if current(&sim, actor).is_some_and(|running| running.name == name(next)) {
                    break;
                }
            }
            let running = current(&sim, actor).expect("still in a move");
            assert_eq!(running.name, name(next), "a press at {mv}'s tick {tick}");
        }
    }
}

/// A dodge gets out of every attack from early on, the finisher's wind-up included.
#[test]
fn a_dodge_gets_out_of_every_attack() {
    for (mv, from) in [("light_1", 2), ("light_2", 2), ("light_3", 4), ("dash_attack", 2)] {
        let ticks = u64::from(player().mv(mv).ticks());
        for tick in (from..ticks).step_by(5) {
            let (mut sim, actor) = lone();
            if mv == "dash_attack" {
                press(&mut sim, actor, "dodge");
                run_to(&mut sim, actor, "dodge", 9);
                press(&mut sim, actor, "light");
            } else {
                press(&mut sim, actor, "light");
                if mv != "light_1" {
                    run_to(&mut sim, actor, "light_1", 14);
                    press(&mut sim, actor, "light");
                }
                if mv == "light_3" {
                    run_to(&mut sim, actor, "light_2", 14);
                    press(&mut sim, actor, "light");
                }
            }
            run_to(&mut sim, actor, mv, tick);
            let running = press(&mut sim, actor, "dodge").expect("in a move");
            assert_eq!(running.name, name("dodge"), "{mv} at {tick}");
        }
    }
}

/// Nothing gets out of the ultimate: it plays to its end beneath its cinematic, whatever is pressed.
#[test]
fn nothing_gets_out_of_the_ultimate() {
    let ticks = u64::from(player().mv("ultimate").ticks());
    let (mut sim, actor) = lone();
    let started = press(&mut sim, actor, "ultimate").map(|running| (running.name, running.tick()));
    assert_eq!(started, Some((name("ultimate"), 1)));
    let intents = ["dodge", "light", "skill", "ultimate"];
    for tick in 2..ticks {
        let intent = intents[tick as usize % intents.len()];
        let running = press(&mut sim, actor, intent).expect("still in it");
        assert_eq!((running.name, running.tick()), (name("ultimate"), tick), "{intent} at {tick}");
    }
}

/// The dodge's windows are where its leap is, and an attack out of its end is the dash attack, which
/// homes in on a target.
#[test]
fn a_dodge_leaves_on_time_and_leads_to_the_dash_attack() {
    let (mut sim, actor) = lone();
    let mut safe = 0;
    let mut perfect = 0;
    press(&mut sim, actor, "dodge");
    while let Some(running) = current(&sim, actor) {
        let view = sim.actor(actor).expect("alive");
        safe += u32::from(view.moves.windows.invulnerable);
        perfect += u32::from(view.moves.windows.perfect_dodge);
        assert!(running.tick() < 24);
        step(&mut sim);
    }
    assert_eq!(safe, 13, "217 ms");
    assert_eq!(perfect, 6, "100 ms");
    let end = sim.actor(actor).expect("alive").position;
    assert!((end.z + 2.4).abs() < 0.02, "2.4 m back: {end}");

    let (mut sim, actor) = lone();
    assert!(sim.set_warp_target(actor, WarpTarget::Point(Vec3::new(0.0, 0.0, 4.0))));
    press(&mut sim, actor, "dodge");
    run_to(&mut sim, actor, "dodge", 9);
    let leaving = sim.actor(actor).expect("alive").position;
    let running = press(&mut sim, actor, "light").expect("in a move");
    assert_eq!(running.name, name("dash_attack"));
    run_to(&mut sim, actor, "dash_attack", 10);
    let arrived = sim.actor(actor).expect("alive").position;
    assert!(arrived.z - leaving.z > 2.0, "it covers ground: {leaving} to {arrived}");
    assert!(arrived.z < 4.0, "and stops short of the target: {arrived}");
}

/// The string lands on a fighter it's aimed at: three hits, the damage and poise they say, with the
/// document's numbers on the scale the warrior was tuned to.
#[test]
fn the_string_lands_on_a_fighter_in_reach() {
    let mut sim = Simulation::new(SimConfig::default());
    let moves =
        sim.add_move_set(MoveSet::from_cooked(&player().set.to_cooked().expect("cooks")).expect("reads"));
    let attacker = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    let target = ActorSpawn {
        position: Vec3::new(0.0, 0.0, 1.5),
        yaw: std::f32::consts::PI,
        moves: Some(moves),
        ..ActorSpawn::default()
    };
    let target = sim.spawn(target).expect("room");
    step(&mut sim);
    let whole = sim.actor(target).expect("alive").fighter.health();
    assert!(whole > 0, "the character is a fighter");
    let mut started = Vec::new();
    for _ in 0..200 {
        assert!(sim.press_intent(attacker, name("light")), "light is an intent");
        // The target keeps pace, as a warped attack would hold it in reach.
        step(&mut sim);
        if let Some(running) = current(&sim, attacker)
            && started.last() != Some(&running.name)
        {
            started.push(running.name);
        }
        if started.len() == 3 && current(&sim, attacker).is_some_and(|running| running.tick() >= 30) {
            break;
        }
    }
    assert_eq!(started, [name("light_1"), name("light_2"), name("light_3")]);
    let view = sim.actor(target).expect("alive");
    assert_eq!(whole - view.fighter.health(), 6 + 8 + 14, "every hit of the string landed once");
    assert_eq!(view.fighter.max_poise() - view.fighter.poise(), 8 + 10 + 18);
}

/// Every move plays out from its first tick to its last, through its phases in order, and leaves the
/// character free, standing where its root motion said.
#[test]
fn every_move_plays_out_cleanly() {
    for (mv, intent) in
        [("light_1", "light"), ("dodge", "dodge"), ("skill", "skill"), ("ultimate", "ultimate")]
    {
        let (mut sim, actor) = lone();
        let found = player().mv(mv);
        let mut phases: Vec<Phase> = Vec::new();
        press(&mut sim, actor, intent);
        while let Some(running) = current(&sim, actor) {
            if phases.last() != Some(&running.phase) {
                phases.push(running.phase);
            }
            step(&mut sim);
        }
        // A dodge's one tick of startup is over by the step it starts.
        let expected = if mv == "dodge" { &[Phase::Active, Phase::Recovery][..] } else { &ALL_PHASES[..] };
        assert_eq!(phases, expected, "{mv}");
        let end = sim.actor(actor).expect("alive").position;
        let root = found.tables.root_samples()[found.ticks() as usize];
        assert!(
            (end.z - root.translation[2]).abs() < 0.01 && (end.x - root.translation[0]).abs() < 0.01,
            "{mv}: ended at {end}, its root at {:?}",
            root.translation
        );
    }
}

const ALL_PHASES: [Phase; 3] = [Phase::Startup, Phase::Active, Phase::Recovery];

/// The skill's whole turn comes home: the character faces where it did, having spun through both hits.
#[test]
fn the_skill_turns_all_the_way_round_between_its_hits() {
    let found = player().mv("skill");
    let yaw = |tick: u32| found.tables.root_samples()[tick as usize].yaw.to_degrees();
    assert!((yaw(12)).abs() < 1.0, "not before the first hit: {}", yaw(12));
    assert!(yaw(21) > 270.0, "most of the way by the second: {}", yaw(21));
    assert!((yaw(found.ticks()) - 360.0).abs() < 1.0, "and all the way home: {}", yaw(found.ticks()));
}
