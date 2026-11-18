//! The shipped cues as version 3 trees (M6 U4a) pick what their version 2 lists did, pinned here, start for start:
//! each start draws its three numbers, and the sound it plays is the list's at the first number's remainder by its
//! length, the rule a list was played by. A footstep switches on the surface, and every surface it's asked with plays
//! stone's, the one case there is.

use std::path::{Path, PathBuf};

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Cue, CueNode};
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::{
    AudioFrame, AudioSystem, CueRandom, CueRequest, CueRng, CueTarget, Listener, MockBackend, MockCall,
    ParameterValue, Rooms, SURFACE,
};
use sapphire_core::Name;

/// Each shipped cue's list as its version 2 document named it, in order, by its path under `content/`: what its
/// tree must play as, pinned here so a migration that moved a sound would show.
const VERSION_2_LISTS: &[(&str, &[&str])] = &[
    (
        "audio/body_fall.cue.ron",
        &[
            "c809c414-06c1-427c-9e02-eefbb416362f",
            "c43cb1f9-8221-404f-a627-bfd883d9921a",
            "cc5659e7-9b0b-4681-a62e-3b8f4e96e3da",
            "a08af5a0-b008-4429-a6ce-37a14a41f27a",
            "2676b21b-4b59-4e77-a249-c8932609924f",
        ],
    ),
    (
        "audio/click.cue.ron",
        &["ad3e7ae5-c55b-4d88-ac49-7399a1bb8963", "a9d7b59e-e5cd-46bf-a451-82bc7393bfee"],
    ),
    (
        "audio/finish.cue.ron",
        &[
            "4f008635-c078-4eb9-97cc-685c825f81a3",
            "755a5791-846b-4be9-94f6-e20c07ed154f",
            "84f562b4-c48d-458b-9670-0fb42dec8560",
            "7c2d817d-aecc-4ef5-8e4a-93219fe04f42",
            "67253d58-e308-4587-a6df-9589abe4e44b",
        ],
    ),
    (
        "audio/hit.cue.ron",
        &[
            "d42dee34-66b1-4881-9cdc-fae3c88401ae",
            "08ee89b5-53c9-4b0c-82ab-cd505aba4461",
            "522e7afd-4878-4e5b-bae7-d47ec8a1eb77",
            "123708e5-d958-4c3c-b95c-fa22e8dc847c",
            "6735d0ec-8290-4ec1-8e2c-b458b810e8c2",
        ],
    ),
    (
        "audio/hit_heavy.cue.ron",
        &[
            "777260c4-e322-4f49-9a56-69565c28b7a0",
            "0e14a582-77a5-4b7d-802a-02fd9e44ce69",
            "8278ada5-1bc2-4c18-ae9f-31f93beb36ff",
            "71cef129-9401-413f-a80a-e352056d300f",
            "888ae611-e1d1-4760-913e-3ae13212c0b3",
        ],
    ),
    (
        "audio/hit_light.cue.ron",
        &[
            "050c7d4c-0c6f-49d1-b977-8741fa916335",
            "05be4dff-d970-42ad-b417-ebdc762c59e4",
            "6eed52da-1f33-47f3-bc86-5150a2ff3d82",
            "5c14d5d3-6654-4e08-a6d0-9cb2feafd43d",
            "fde4093a-174e-4f83-bd32-c7266c4b5a82",
        ],
    ),
    (
        "audio/hit_medium.cue.ron",
        &[
            "777260c4-e322-4f49-9a56-69565c28b7a0",
            "0e14a582-77a5-4b7d-802a-02fd9e44ce69",
            "8278ada5-1bc2-4c18-ae9f-31f93beb36ff",
            "71cef129-9401-413f-a80a-e352056d300f",
            "888ae611-e1d1-4760-913e-3ae13212c0b3",
        ],
    ),
    (
        "audio/perfect_dodge.cue.ron",
        &[
            "f4fab061-b3da-4559-a683-452e7b627465",
            "f478dc54-8b38-47b7-a293-6456ccff2aed",
            "71de2cd2-7c48-45aa-836c-b74d8d95b339",
            "a1f5d1f8-c2cf-4204-a502-99b205664b3b",
            "56374f65-c921-4c1d-ab38-43a2e1ecf2e1",
        ],
    ),
    (
        "audio/poise_break.cue.ron",
        &[
            "4f008635-c078-4eb9-97cc-685c825f81a3",
            "755a5791-846b-4be9-94f6-e20c07ed154f",
            "84f562b4-c48d-458b-9670-0fb42dec8560",
            "7c2d817d-aecc-4ef5-8e4a-93219fe04f42",
            "67253d58-e308-4587-a6df-9589abe4e44b",
        ],
    ),
    (
        "audio/skid.cue.ron",
        &[
            "b6dd6b1f-1eb7-43e3-8ed9-49ded116db1c",
            "2335d401-1961-4411-8190-59a1d4e3ebe9",
            "9b379deb-aa7d-4b2f-8159-b39a982efe2b",
            "4b4b02d9-d31e-4477-aa2e-1cee335c363d",
            "3cd19ff2-fcc7-443a-a86e-ce99ce7ca406",
        ],
    ),
    (
        "audio/step.cue.ron",
        &[
            "b6dd6b1f-1eb7-43e3-8ed9-49ded116db1c",
            "2335d401-1961-4411-8190-59a1d4e3ebe9",
            "9b379deb-aa7d-4b2f-8159-b39a982efe2b",
            "4b4b02d9-d31e-4477-aa2e-1cee335c363d",
            "3cd19ff2-fcc7-443a-a86e-ce99ce7ca406",
        ],
    ),
    (
        "audio/swing.cue.ron",
        &["66fe7f41-fcfd-43ac-9bc5-1c9ac00d21db", "4c334802-41a9-4417-b9e4-86d2aa5cffe7"],
    ),
    ("vo/en/guide_greet.cue.ron", &["e7310479-f739-4644-a40b-df0045b33f19"]),
    ("vo/en/perfect-dodge.cue.ron", &["868d5969-ea82-4ac9-836e-8a63a5daef19"]),
    ("vo/en/swap-in.cue.ron", &["43f2881d-f499-4ff8-8629-14fcae5f3e09"]),
    ("vo/ja/guide_greet.cue.ron", &["ac3761db-59af-4faa-9a6f-f72e6c7dd684"]),
    ("vo/ja/perfect-dodge.cue.ron", &["87e3bd86-d202-4d31-ba26-4c7f18da2a8c"]),
    ("vo/ja/swap-in.cue.ron", &["8b3529b7-72fd-4c9a-b105-a0665d2999fc"]),
];

/// Starts drawn of each cue.
const STARTS: usize = 1000;

fn content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

/// Every shipped cue document, by path, read: the game's and each voice language's.
fn shipped() -> Vec<(PathBuf, Cue)> {
    let mut folders = vec![content().join("audio")];
    for entry in std::fs::read_dir(content().join("vo")).expect("the voice folder") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            folders.push(path);
        }
    }
    let mut cues = Vec::new();
    for folder in folders {
        for entry in std::fs::read_dir(&folder).expect("a folder") {
            let path = entry.expect("an entry").path();
            if path.to_string_lossy().ends_with(".cue.ron") {
                let text = std::fs::read_to_string(&path).expect("reads");
                let cue =
                    Cue::from_document(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                assert!(text.contains("version: 3,"), "{} isn't at version 3", path.display());
                cues.push((path, cue));
            }
        }
    }
    cues.sort_by(|a, b| a.0.cmp(&b.0));
    cues
}

/// The sounds `cue`'s starts play with `seed`, asked as `request` makes them, `STARTS` of them.
fn played(cue: &Cue, seed: u64, request: impl Fn(AssetId) -> CueRequest) -> Vec<AssetId> {
    const ID: AssetId = AssetId::from_text("0a0a0a0a-0000-4000-8000-000000000001");
    let mut system = AudioSystem::new(MockBackend::new());
    let silence = DecodedSound { sample_rate: 48_000, channels: 1, samples: vec![0.0; 480] };
    for sound in cue.sounds() {
        system.add_sound(sound, &silence);
    }
    system.add_cue(ID, cue.clone());
    let mut rng = CueRng::new(seed);
    let mut sounds = Vec::with_capacity(STARTS);
    for _ in 0..STARTS {
        let asked = [request(ID)];
        let frame = AudioFrame {
            dt: 1.0 / 60.0,
            listener: Listener::default(),
            world_time_scale: 1.0,
            snapshots: &[],
            emitters: &[],
            cues: &asked,
            lines: &[],
            speech: None,
            music: None,
            rooms: Rooms::NONE,
        };
        system.update(&frame, &mut rng);
        // Each voice ends as soon as it has started, so no limit or cap ever stands in the way.
        for call in system.backend_mut().take_calls() {
            if let MockCall::StartVoice { slot, sound, .. } = call {
                sounds.push(sound);
                system.backend_mut().finish(slot);
            }
        }
    }
    sounds
}

/// What a version 2 list played with `seed`: the first of each start's three numbers, by its remainder.
fn listed(list: &[AssetId], seed: u64) -> Vec<AssetId> {
    let mut rng = CueRng::new(seed);
    (0..STARTS)
        .map(|_| {
            let (pick, _, _) = (rng.next_u32(), rng.next_unit(), rng.next_unit());
            list[pick as usize % list.len()]
        })
        .collect()
}

#[test]
fn every_shipped_cue_picks_as_its_list_did() {
    let cues = shipped();
    let names: Vec<_> = cues
        .iter()
        .map(|(path, _)| path.strip_prefix(content()).expect("under content").to_owned())
        .collect();
    let pinned: Vec<_> = VERSION_2_LISTS.iter().map(|(name, _)| PathBuf::from(name)).collect();
    assert_eq!(names, pinned, "a cue added or gone: pin its list");
    for ((path, cue), (_, ids)) in cues.iter().zip(VERSION_2_LISTS) {
        // The tree names the version 2 list's sounds, in its order: one random node over them, or one sound, or
        // for a footstep the one case's.
        let list: Vec<_> = ids.iter().map(|id| AssetId::from_text(id)).collect();
        assert_eq!(cue.sounds(), list, "{}", path.display());
        let flat = |node: &CueNode| match node {
            CueNode::Sound(_) => true,
            CueNode::Random(random) => {
                random.weights.is_empty()
                    && random.avoid_repeat == 0
                    && random.of.iter().all(|node| matches!(node, CueNode::Sound(_)))
            }
            _ => false,
        };
        let shaped = match &cue.play {
            CueNode::Switch(switch) => switch.cases.len() == 1 && flat(&switch.cases[0].play),
            node => flat(node),
        };
        assert!(shaped, "{} isn't a list's shape: {:?}", path.display(), cue.play);
        for seed in [1, 2, 77] {
            let expected = listed(&list, seed);
            let flat = played(cue, seed, |id| CueRequest::new(id, CueTarget::Flat));
            assert_eq!(flat, expected, "{} with seed {seed}", path.display());
            let as_v2 = Cue { play: CueNode::one_of(list.clone()), ..cue.clone() };
            assert_eq!(played(&as_v2, seed, |id| CueRequest::new(id, CueTarget::Flat)), expected);
        }
    }
}

#[test]
fn a_footstep_switches_on_the_surface_and_has_only_stone_yet() {
    let (_, step) =
        shipped().into_iter().find(|(path, _)| path.ends_with("audio/step.cue.ron")).expect("a step");
    let CueNode::Switch(switch) = &step.play else { panic!("{:?}", step.play) };
    assert_eq!((switch.on.as_str(), switch.default.as_str()), ("surface", "stone"));
    assert_eq!(Name::new(&switch.on), SURFACE);
    let (_, ids) = VERSION_2_LISTS.iter().find(|(name, _)| *name == "audio/step.cue.ron").expect("pinned");
    let expected = listed(&ids.iter().map(|id| AssetId::from_text(id)).collect::<Vec<_>>(), 5);
    for surface in ["stone", "grass", "metal"] {
        let on =
            |id| CueRequest::new(id, CueTarget::Flat).with(SURFACE, ParameterValue::Case(Name::new(surface)));
        assert_eq!(played(&step, 5, on), expected, "{surface}");
    }
}
