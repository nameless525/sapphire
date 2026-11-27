use super::program::{BlendAxes, ConditionOp, PoseOp, ProgramFootwork, Span};
use super::*;

const SOURCE: &str = "9a29e8c0-f131-4ea4-98b9-04879740413d";
const TARGET: &str = "6f8a81b6-29c4-485f-b161-847b001d561f";

/// Everything a version 2 document can say, once.
fn document() -> String {
    format!(
        r#"(
    type: "sapphire::assets::AnimGraph",
    version: 2,
    data: (
        parameters: [
            (name: "velocity_x", from: Locomotion(SmoothedX)),
            (name: "velocity_z", from: Locomotion(SmoothedZ)),
            (name: "smoothed_speed", from: Locomotion(SmoothedSpeed)),
            (name: "grounded", from: Locomotion(Grounded)),
            (name: "landed", from: Locomotion(Landed)),
            (name: "mode", from: Traversal(Mode)),
            (name: "aim", from: Game("aim")),
        ],
        smoothing: 4.0,
        clips: [
            (name: "idle", source: "{SOURCE}", clip: "Idle_A", target: "{TARGET}"),
            (name: "walk", source: "{SOURCE}", clip: "Walking_A"),
            (name: "run", source: "{SOURCE}", clip: "Running_A"),
            (name: "rise", source: "{SOURCE}", clip: "Jump_Start"),
            (name: "rise_run", source: "{SOURCE}", clip: "Jump_Run"),
            (name: "fall", source: "{SOURCE}", clip: "Jump_Idle"),
            (name: "land", source: "{SOURCE}", clip: "Jump_Land"),
            (name: "land_run", source: "{SOURCE}", clip: "Jump_Land_Run"),
            (name: "aim_low", source: "{SOURCE}", clip: "Aim_Low"),
            (name: "aim_high", source: "{SOURCE}", clip: "Aim_High"),
            (name: "jab", source: "{TARGET}"),
            (name: "jab_behind", source: "{TARGET}", target: "{SOURCE}"),
            (name: "breathe", source: "{SOURCE}", clip: "Breathe"),
        ],
        masks: [(name: "upper", bones: [(bone: Spine, weight: 1.0), (bone: Head, weight: 0.5)])],
        sync: [(name: "feet", foot: Left, contact: Down)],
        machines: [
            (name: "locomotion", start: "ground", states: [
                (name: "ground", plays: Blend2D((x: "velocity_x", y: "velocity_z", sync: Some("feet"), samples: [
                    (clip: "idle", at: (0.0, 0.0), sync: false),
                    (clip: "walk", at: (0.0, 1.5), phase: 0.25),
                    (clip: "run", at: (0.0, 5.0)),
                ], rate: (0.75, 1.5)))),
                (name: "air", plays: Machine("air")),
                (name: "land", plays: Landing((standing: "land", moving: [(clip: "land_run", fade: (0.6, 3.8))]))),
            ], transitions: [
                (from: "ground", to: "air", when: "!grounded", decay: 8),
                (from: "air", to: "land", when: "landed && grounded", decay: 3),
                (from: "land", to: "ground", when: "finished_at(0.5) && grounded", decay: 8),
                (from: "*", to: "ground", when: "grounded && mode != Swimming", decay: 6),
            ]),
            (name: "air", start: "rise", states: [
                (name: "rise", plays: Takeoff((standing: "rise", moving: [(clip: "rise_run", fade: (0.6, 3.8))], anticipate: true))),
                (name: "fall", plays: Clip(clip: "fall", looping: true)),
            ], transitions: [
                (from: "rise", to: "fall", when: "finished", decay: 4),
            ]),
            (name: "upper", start: "aim", states: [
                (name: "aim", plays: Blend1D((parameter: "aim", samples: [(clip: "aim_low", at: -1.0), (clip: "aim_high", at: 1.0)]))),
            ]),
        ],
        layers: [
            (name: "locomotion", plays: Machine((machine: "locomotion"))),
            (name: "upper", plays: Machine((machine: "upper", mask: Some("upper"), weight: 0.5))),
            (name: "combat", plays: Combat((decay: 5, exit: 12, moves: [
                (move: "jab", clip: "jab", variants: [(from: Behind, clip: "jab_behind")]),
                (move: "wave", clip: "jab", mask: Some("upper"), decay: Some(3)),
            ]))),
            (name: "breathing", plays: Additive((clip: "breathe", mask: Some("upper"), weight: 0.5))),
            (name: "reactions", plays: Reactions([(name: "flinch", clip: "jab", weight: 0.7, fade_out: 8)])),
        ],
        events: [
            (name: "footstep", from: Foot((foot: Left, contact: Down))),
            (name: "swish", from: Clip("swish"), min_weight: 0.8),
        ],
        blink: Some((expression: "blink", every: (150, 360), close: 3, hold: 1, open: 7)),
    ),
)"#
    )
}

fn graph() -> AnimGraph {
    let mut graph = AnimGraph::from_document(&document()).expect("a good document");
    graph.resolve(|clip| AssetId::from_bytes([clip.name.len() as u8; 16]));
    graph
}

fn program() -> GraphProgram {
    graph().compile().unwrap_or_else(|error| panic!("{error}"))
}

/// The issues in `document()` with `from` replaced by `to`.
fn issues_with(from: &str, to: &str) -> Vec<GraphIssue> {
    let text = document().replacen(from, to, 1);
    assert_ne!(text, document(), "`{from}` isn't in the document");
    let graph =
        AnimGraph::from_document(&text).unwrap_or_else(|error| panic!("`{to}` doesn't parse: {error}"));
    match graph.compile() {
        Err(CompileError { issues }) => issues,
        Ok(_) => panic!("`{to}` compiled"),
    }
}

#[test]
fn a_document_reads_writes_and_reads_to_the_same_value() {
    let graph = AnimGraph::from_document(&document()).expect("reads");
    let written = graph.to_document().expect("writes");
    assert!(written.contains("version: 2"), "{written}");
    let again = AnimGraph::from_document(&written).expect("reads back");
    assert_eq!(again, graph);
    assert_eq!(again.to_document().expect("writes again"), written, "a second write is the first's bytes");
}

#[test]
fn a_document_reads_every_section() {
    let graph = graph();
    assert_eq!(graph.parameters[6].from, ParameterSource::Game("aim".to_owned()));
    assert_eq!(graph.smoothing, 4.0);
    assert_eq!(graph.sync[0].contact, Contact::Down);
    let Motion::Blend2D(ground) = &graph.machines[0].states[0].plays else { panic!("a 2D blend") };
    assert_eq!((ground.x.as_str(), ground.sync.as_deref()), ("velocity_x", Some("feet")));
    let Motion::Takeoff(rise) = &graph.machines[1].states[0].plays else { panic!("a takeoff") };
    assert!(rise.anticipate && !rise.carry);
    let LayerMotion::Machine(upper) = &graph.layers[1].plays else { panic!("a machine layer") };
    assert_eq!((upper.blend, upper.weight), (LayerBlend::Override, 0.5));
    let LayerMotion::Combat(combat) = &graph.layers[2].plays else { panic!("combat") };
    assert_eq!(combat.moves[0].variants[0].from, Side::Behind);
    let LayerMotion::Reactions(reactions) = &graph.layers[4].plays else { panic!("reactions") };
    assert_eq!(reactions[0].restart, 4, "the default");
    assert_eq!(graph.events[0].min_weight, 0.5, "the default");
}

#[test]
fn the_program_lays_out_states_layers_and_registers() {
    let program = program();
    assert_eq!(program.machines.len(), 3);
    let [locomotion, air, upper] = [0, 1, 2].map(|index| &program.machines[index]);
    assert_eq!(
        (locomotion.register, air.register, upper.register),
        (0, 0, 1),
        "a nested machine writes its player's"
    );
    assert_eq!((locomotion.depth, air.depth, upper.depth), (1, 2, 1));
    assert_eq!(locomotion.states, Span { start: 0, len: 3 });
    assert_eq!(program.states[usize::from(air.start)].name, "rise");
    assert_eq!(program.state_ops(1), [PoseOp::Machine { machine: 1, into: 0 }]);
    assert_eq!(
        program.layer_ops(),
        [
            PoseOp::Machine { machine: 0, into: 0 },
            PoseOp::Machine { machine: 2, into: 1 },
            PoseOp::Layer { from: 1, onto: 0, mask: Some(0), blend: LayerBlend::Override, weight: 0.5 },
            PoseOp::Combat { onto: 0 },
            PoseOp::Additive { additive: 0, onto: 0 },
            PoseOp::Reactions { onto: 0 },
        ]
    );
    let PoseOp::Blend { blend, .. } = program.state_ops(0)[0] else { panic!("a blend") };
    let ground = &program.blends[usize::from(blend)];
    assert_eq!((ground.axes, ground.sync, ground.samples.len), (BlendAxes::Two(0, 1), Some(0), 3));
    let PoseOp::Blend { blend, into: 1 } = program.state_ops(program.machines[2].states.start)[0] else {
        panic!("the upper machine's blend, into its register")
    };
    assert_eq!(program.blends[usize::from(blend)].axes, BlendAxes::One(6));
    assert_eq!(program.transitions(0)[3].from, None, "any state");
    assert_eq!(program.transitions(0)[2].phase, Some(0.5));
    assert_eq!(program.transitions(0)[1].reads, 0b11000, "`landed` and `grounded`");
    assert_eq!(program.parameter("aim"), Some(6));
    assert_eq!(program.reaction("flinch"), Some(0));
    assert_eq!(program.clip_ids().count(), 13);
}

/// The known graph's worst case, worked out by hand: the locomotion machine's costliest state is the
/// landing (its two clips, then again for the catch), the upper layer's 1D blend adds two, a masked
/// move one, breathing and a reaction one each.
#[test]
fn the_worst_case_cost_is_computed() {
    let cost = program().cost;
    assert_eq!(cost.clips_sampled, 4 + 2 + 1 + 1 + 1);
    // An entry's pose, its sample and its catch's pose above the output.
    assert_eq!(cost.registers, 3);
    assert_eq!(cost.ops, 6 + 6);
    // From `air` or `land`: its own transition and the any-state one, 3 and 4 steps.
    assert_eq!(cost.condition_ops, 7);
    assert_eq!(cost.condition_depth, 2);
    assert_eq!(cost.machine_depth, 2);
}

#[test]
fn a_graph_past_the_clip_cap_is_refused() {
    // A ground blend of fourteen samples, then the upper layer's two, a masked move, breathing and a
    // reaction: nineteen a frame.
    let samples: Vec<String> =
        (0..11).map(|index| format!(r#"(clip: "idle", at: ({index}.0, 9.0))"#)).collect();
    let text = document().replacen(
        r#"(clip: "run", at: (0.0, 5.0)),"#,
        &format!(r#"(clip: "run", at: (0.0, 5.0)), {},"#, samples.join(", ")),
        1,
    );
    let graph = AnimGraph::from_document(&text).expect("parses");
    let issues = graph.check();
    assert!(
        issues.iter().any(|issue| issue.path == "layers" && issue.message.contains("samples up to 19 clips")),
        "{issues:?}"
    );
}

/// The cap's edge: the test graph's ground blend grown by `extra` samples. Its landing costs 4, so the
/// ground sets the machine's cost from 4 samples on, one clip a sample.
fn cost_with_extra_ground_samples(extra: usize) -> Result<GraphProgram, CompileError> {
    let samples: Vec<String> =
        (0..extra).map(|index| format!(r#"(clip: "idle", at: ({index}.0, 9.0))"#)).collect();
    let text = document().replacen(
        r#"(clip: "run", at: (0.0, 5.0)),"#,
        &format!(r#"(clip: "run", at: (0.0, 5.0)), {},"#, samples.join(", ")),
        1,
    );
    let mut graph = AnimGraph::from_document(&text).expect("parses");
    graph.resolve(|clip| AssetId::from_bytes([clip.name.len() as u8; 16]));
    graph.compile()
}

#[test]
fn a_graph_at_the_clip_cap_compiles_and_one_past_it_is_refused() {
    let at = cost_with_extra_ground_samples(4).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(usize::from(at.cost.clips_sampled), program::MAX_CLIPS_SAMPLED);
    let Err(CompileError { issues }) = cost_with_extra_ground_samples(5) else { panic!("13 compiled") };
    assert!(
        issues.iter().any(|issue| issue.path == "layers" && issue.message.contains("samples up to 13 clips")),
        "{issues:?}"
    );
}

#[test]
fn conditions_hold_over_the_value_registers() {
    let program = program();
    let transitions = program.transitions(0);
    // velocity_x, velocity_z, smoothed_speed, grounded, landed, mode, aim.
    let mut values = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    assert!(!program.holds(&transitions[0], &values, false));
    values[3] = 0.0;
    assert!(program.holds(&transitions[0], &values, false), "airborne");
    values[4] = 1.0;
    assert!(!program.holds(&transitions[1], &values, false), "landed in the air isn't a landing");
    values[3] = 1.0;
    assert!(program.holds(&transitions[1], &values, false));
    assert!(!program.holds(&transitions[2], &values, false));
    assert!(program.holds(&transitions[2], &values, true), "finished");
    values[5] = 3.0;
    assert!(!program.holds(&transitions[3], &values, false), "swimming");
    values[5] = 1.0;
    assert!(program.holds(&transitions[3], &values, false), "climbing");
    assert!(!program.holds(&transitions[3], &values[..2], false), "a missing register reads 0");
}

#[test]
fn a_move_row_is_found_by_name_with_its_variants() {
    let program = program();
    let jab = program.move_row("jab").expect("jab");
    assert_eq!(jab.clip_from(None), 10);
    assert_eq!(jab.clip_from(Some(Side::Behind)), 11);
    assert_eq!(jab.clip_from(Some(Side::Left)), 10, "no variant from the left");
    assert_eq!(program.move_row("wave").map(|row| (row.mask, row.decay)), Some((Some(0), 3)));
    assert!(program.move_row("kick").is_none());
}

#[test]
fn a_cooked_program_reads_back_the_same_and_damage_is_refused() {
    let program = program();
    let cooked = program.to_cooked().expect("cooks");
    assert_eq!(GraphProgram::read(&cooked).expect("reads"), program);
    let damaged = |break_it: &dyn Fn(&mut GraphProgram)| {
        let mut bad = program.clone();
        break_it(&mut bad);
        let cooked = bad.to_cooked().expect("cooks");
        matches!(GraphProgram::read(&cooked), Err(GraphError::Damaged(_)))
    };
    assert!(damaged(&|bad| bad.ops[0] = PoseOp::Clip { clip: 99, looping: false, sync: None, into: 0 }));
    assert!(damaged(&|bad| bad.layers.len += 1));
    assert!(damaged(&|bad| bad.conditions[0] = ConditionOp::And));
    assert!(damaged(&|bad| bad.conditions[0] = ConditionOp::Truth(40)));
    assert!(damaged(&|bad| bad.cost.registers = 2));
    assert!(damaged(&|bad| bad.moves.reverse()));
    assert!(damaged(&|bad| bad.machines[0].start = 4));
    assert!(damaged(&|bad| bad.ops[1] = PoseOp::Machine { machine: 1, into: 1 }));
    // Structure, not only range: air's `fall` playing air would recurse for ever.
    assert!(damaged(&|bad| bad.ops[4] = PoseOp::Machine { machine: 1, into: 0 }));
    assert!(damaged(&|bad| bad.cost.clips_sampled = 13));
    assert!(damaged(&|bad| bad.events = vec![bad.events[0].clone(); 17]));
    assert!(damaged(&|bad| {
        let takeoff = bad.entries.iter().position(|entry| !entry.landing).expect("a takeoff");
        let sample = bad.entries[takeoff].moving[0];
        bad.entries[takeoff].moving.push(sample);
    }));
    assert!(damaged(&|bad| bad.footwork.push(ProgramFootwork {
        brake: [0; 2],
        launch: [0; 2],
        gaits: vec![]
    })));
    // A state or a transition that belongs to another machine.
    assert!(damaged(&|bad| bad.states[0].machine = 1));
    assert!(damaged(&|bad| bad.transitions[0].to = 4));
    // The runtime's game slots are an array, and a line's weights divide by its samples' gaps.
    assert!(damaged(&|bad| {
        let slot = bad.parameters.iter().find(|parameter| matches!(parameter.from, ParameterSource::Game(_)));
        let slot = slot.expect("a game slot").clone();
        // One past the limit, and still well inside the registers'.
        bad.parameters.extend(std::iter::repeat_n(slot, MAX_GAME_SLOTS));
        assert!(bad.parameters.len() <= MAX_PARAMETERS);
    }));
    assert!(damaged(&|bad| {
        let line =
            bad.blends.iter().find(|blend| matches!(blend.axes, BlendAxes::One(_))).expect("a 1D blend");
        let first = line.samples.range().start;
        bad.samples[first + 1].at[0] = bad.samples[first].at[0];
    }));
    // Places that aren't finite, or a plane's place twice.
    let line_at = |bad: &mut GraphProgram, sample: usize, at: f32| {
        let line =
            bad.blends.iter().find(|blend| matches!(blend.axes, BlendAxes::One(_))).expect("a 1D blend");
        let first = line.samples.range().start;
        bad.samples[first + sample].at[0] = at;
    };
    assert!(damaged(&|bad| line_at(bad, 0, f32::NEG_INFINITY)));
    assert!(damaged(&|bad| line_at(bad, 1, f32::NAN)));
    let plane = |bad: &GraphProgram| {
        let plane =
            bad.blends.iter().find(|blend| matches!(blend.axes, BlendAxes::Two(..))).expect("a 2D blend");
        plane.samples.range().start
    };
    assert!(damaged(&|bad| {
        let first = plane(bad);
        bad.samples[first + 1].at[1] = f32::INFINITY;
    }));
    assert!(damaged(&|bad| {
        let first = plane(bad);
        bad.samples[first + 1].at = bad.samples[first].at;
    }));
}

/// The runtime keeps one footwork for the graph: a second blend's that differs is refused at its path.
#[test]
fn a_second_blend_with_other_footwork_is_refused() {
    let footwork = |launch: &str| {
        format!(
            r#"footwork: Some((brake: ("idle", "idle"), launch: ("{launch}", "{launch}"), gaits: [(clip: "walk", contact: 0.5)]))"#
        )
    };
    let ground = format!("rate: (0.75, 1.5), {})))", footwork("walk"));
    let fall = |launch: &str| {
        format!(
            r#"(name: "fall", plays: Blend2D((x: "velocity_x", y: "velocity_z", samples: [(clip: "fall", at: (0.0, 0.0))], {})))"#,
            footwork(launch)
        )
    };
    let text = |launch: &str| {
        document().replacen("rate: (0.75, 1.5))))", &ground, 1).replacen(
            r#"(name: "fall", plays: Clip(clip: "fall", looping: true))"#,
            &fall(launch),
            1,
        )
    };
    let compile = |launch: &str| {
        let mut graph = AnimGraph::from_document(&text(launch)).unwrap_or_else(|error| panic!("{error}"));
        graph.resolve(|clip| AssetId::from_bytes([clip.name.len() as u8; 16]));
        graph.compile()
    };
    compile("walk").unwrap_or_else(|error| panic!("the same footwork twice is one: {error}"));
    let Err(CompileError { issues }) = compile("run") else { panic!("another footwork compiled") };
    assert!(
        issues.iter().any(|issue| issue.path == r#"machines["air"].states["fall"].plays.footwork"#
            && issue.to_string().contains("an earlier blend's footwork")),
        "{issues:?}"
    );
}

#[test]
fn node_paths_name_every_machine_state_and_layer() {
    let paths: Vec<String> = graph().node_paths().collect();
    assert_eq!(paths.len(), 3 + 3 + 2 + 1 + 5);
    assert_eq!(paths[0], r#"machines["locomotion"]"#);
    assert_eq!(paths[1], r#"machines["locomotion"].states["ground"]"#);
    assert_eq!(paths.last().map(String::as_str), Some(r#"layers["reactions"]"#));
}

/// Each row breaks one thing; the refusal must name its path exactly and say why.
#[test]
fn refusals_name_the_path_of_what_they_refuse() {
    let rows: &[(&str, &str, &str, &str)] = &[
        // Parameters.
        (r#"(name: "aim", from"#, r#"(name: "Aim", from"#, r#"parameters["Aim"]"#, "lower-case"),
        (r#"(name: "aim", from"#, r#"(name: "finished", from"#, r#"parameters["finished"]"#, "grammar"),
        (r#"(name: "landed", from"#, r#"(name: "grounded", from"#, r#"parameters["grounded"]"#, "twice"),
        (
            r#"from: Locomotion(Landed)"#,
            r#"from: Locomotion(Grounded)"#,
            r#"parameters["landed"].from"#,
            "bound to this already",
        ),
        (r#"Game("aim")"#, r#"Game("")"#, r#"parameters["aim"].from"#, "no name"),
        ("smoothing: 4.0", "smoothing: -1.0", "smoothing", "half-life"),
        // Clips, masks and groups.
        (r#"(name: "walk", source"#, r#"(name: "idle", source"#, r#"clips["idle"]"#, "twice"),
        ("weight: 0.5)])", "weight: 1.5)])", r#"masks["upper"].bones[1].weight"#, "1.5"),
        // A machine's states.
        (r#"start: "ground""#, r#"start: "grounded""#, r#"machines["locomotion"].start"#, "no such state"),
        (
            r#"(name: "land", plays"#,
            r#"(name: "*", plays"#,
            r#"machines["locomotion"].states["*"]"#,
            "any state",
        ),
        (
            r#"(clip: "run", at"#,
            r#"(clip: "sprint", at"#,
            r#"machines["locomotion"].states["ground"].plays.samples[2].clip"#,
            "`sprint`",
        ),
        (
            "at: (0.0, 5.0)",
            "at: (0.0, 1.5)",
            r#"machines["locomotion"].states["ground"].plays.samples[2].at"#,
            "neither can be exact",
        ),
        (
            "phase: 0.25",
            "phase: 1.5",
            r#"machines["locomotion"].states["ground"].plays.samples[1].phase"#,
            "1.5",
        ),
        (
            "rate: (0.75, 1.5)",
            "rate: (1.5, 0.75)",
            r#"machines["locomotion"].states["ground"].plays.rate"#,
            "range",
        ),
        (
            r#"y: "velocity_z""#,
            r#"y: "grounded""#,
            r#"machines["locomotion"].states["ground"].plays.y"#,
            "isn't a number",
        ),
        (
            r#"y: "velocity_z""#,
            r#"y: "velocity_x""#,
            r#"machines["locomotion"].states["ground"].plays.y"#,
            "two parameters",
        ),
        (
            r#"sync: Some("feet")"#,
            r#"sync: Some("hands")"#,
            r#"machines["locomotion"].states["ground"].plays.sync"#,
            "`hands`",
        ),
        (
            "fade: (0.6, 3.8))]))",
            "fade: (3.8, 0.6))]))",
            r#"machines["locomotion"].states["land"].plays.moving[0].fade"#,
            "increasing",
        ),
        (
            r#"moving: [(clip: "land_run", fade: (0.6, 3.8))]"#,
            r#"moving: []"#,
            r#"machines["locomotion"].states["land"].plays.moving"#,
            "one to 3",
        ),
        (
            r#"moving: [(clip: "rise_run", fade: (0.6, 3.8))]"#,
            r#"moving: [(clip: "rise_run", fade: (0.6, 3.8)), (clip: "rise_run", fade: (0.6, 3.8))]"#,
            r#"machines["air"].states["rise"].plays.moving"#,
            "one to 1",
        ),
        (
            "(clip: \"aim_high\", at: 1.0)",
            "(clip: \"aim_high\", at: -2.0)",
            r#"machines["upper"].states["aim"].plays.samples[1].at"#,
            "increasing order",
        ),
        (
            r#"(parameter: "aim""#,
            r#"(parameter: "aiming""#,
            r#"machines["upper"].states["aim"].plays.parameter"#,
            "`aiming`",
        ),
        // A machine's transitions.
        (r#"to: "land""#, r#"to: "sky""#, r#"machines["locomotion"].transitions[1].to"#, "`sky`"),
        (
            r#"from: "air", to"#,
            r#"from: "sky", to"#,
            r#"machines["locomotion"].transitions[1].from"#,
            "`sky`",
        ),
        ("decay: 8)", "decay: 500)", r#"machines["locomotion"].transitions[0].decay"#, "500"),
        (
            r#""!grounded""#,
            r#""!grounded &&""#,
            r#"machines["locomotion"].transitions[0].when"#,
            "stops short",
        ),
        (r#""!grounded""#, r#""!flying""#, r#"machines["locomotion"].transitions[0].when"#, "`flying`"),
        (r#""!grounded""#, r#""finished""#, r#"machines["locomotion"].transitions[0].when"#, "never does"),
        // Where machines play.
        (
            r#"(name: "air", plays: Machine("air"))"#,
            r#"(name: "air", plays: Machine("fly"))"#,
            r#"machines["locomotion"].states["air"].plays"#,
            "`fly`",
        ),
        (
            r#"(name: "air", plays: Machine("air"))"#,
            r#"(name: "air", plays: Clip(clip: "fall"))"#,
            r#"machines["air"]"#,
            "plays nowhere",
        ),
        (
            r#"Machine((machine: "upper""#,
            r#"Machine((machine: "locomotion""#,
            r#"machines["locomotion"]"#,
            "plays in 2 places",
        ),
        (
            r#"(name: "fall", plays: Clip(clip: "fall", looping: true))"#,
            r#"(name: "fall", plays: Machine("air"))"#,
            r#"machines["air"]"#,
            "plays itself",
        ),
        // Layers.
        (
            r#"(name: "locomotion", plays: Machine((machine: "locomotion")))"#,
            r#"(name: "locomotion", plays: Machine((machine: "locomotion", weight: 0.5)))"#,
            r#"layers["locomotion"].plays"#,
            "whole body",
        ),
        (
            r#"mask: Some("upper"), weight: 0.5)))"#,
            r#"mask: Some("legs"), weight: 0.5)))"#,
            r#"layers["upper"].plays.mask"#,
            "`legs`",
        ),
        ("exit: 12", "exit: 500", r#"layers["combat"].plays.exit"#, "500"),
        (
            r#"(move: "wave", clip: "jab""#,
            r#"(move: "jab", clip: "jab""#,
            r#"layers["combat"].plays.moves["jab"]"#,
            "twice",
        ),
        (
            r#"(from: Behind, clip: "jab_behind")"#,
            r#"(from: Behind, clip: "jab_back")"#,
            r#"layers["combat"].plays.moves["jab"].variants[0].clip"#,
            "`jab_back`",
        ),
        (
            r#"(from: Behind, clip: "jab_behind")"#,
            r#"(from: Behind, clip: "jab_behind"), (from: Behind, clip: "jab")"#,
            r#"layers["combat"].plays.moves["jab"].variants[1].from"#,
            "second clip",
        ),
        (
            r#"(clip: "breathe", mask: Some("upper"), weight: 0.5)"#,
            r#"(clip: "breathe", mask: Some("upper"), weight: 2.0)"#,
            r#"layers["breathing"].plays.weight"#,
            "2",
        ),
        (
            r#"(name: "flinch", clip: "jab""#,
            r#"(name: "flinch", clip: "ow""#,
            r#"layers["reactions"].plays["flinch"].clip"#,
            "`ow`",
        ),
        (r#"(name: "breathing", plays"#, r#"(name: "combat", plays"#, r#"layers["combat"]"#, "twice"),
        // Events and the blink.
        (
            r#"(name: "swish", from: Clip("swish")"#,
            r#"(name: "a swish", from: Clip("swish")"#,
            r#"events["a swish"]"#,
            "printable",
        ),
        ("min_weight: 0.8", "min_weight: 1.8", r#"events["swish"].min_weight"#, "1.8"),
        ("every: (150, 360)", "every: (360, 150)", "blink.every", "range"),
        // A takeoff or a landing reads the base's flight, and a seek in the air starts at the one takeoff.
        (
            r#"(name: "fall", plays: Clip(clip: "fall", looping: true))"#,
            r#"(name: "fall", plays: Takeoff((standing: "fall", moving: [(clip: "rise_run", fade: (0.6, 3.8))])))"#,
            r#"machines["air"].states["fall"].plays"#,
            "a second takeoff",
        ),
        (
            r#"(name: "aim", plays: Blend1D((parameter: "aim", samples: [(clip: "aim_low", at: -1.0), (clip: "aim_high", at: 1.0)])))"#,
            r#"(name: "aim", plays: Landing((standing: "land", moving: [(clip: "land_run", fade: (0.6, 3.8))])))"#,
            r#"machines["upper"].states["aim"].plays"#,
            "a landing plays only in the first layer's",
        ),
        (
            r#"(name: "aim", plays: Blend1D((parameter: "aim", samples: [(clip: "aim_low", at: -1.0), (clip: "aim_high", at: 1.0)])))"#,
            r#"(name: "aim", plays: Takeoff((standing: "rise", moving: [(clip: "rise_run", fade: (0.6, 3.8))])))"#,
            r#"machines["upper"].states["aim"].plays"#,
            "a takeoff plays only in the first layer's",
        ),
    ];
    for (from, to, path, word) in rows {
        let issues = issues_with(from, to);
        assert!(
            issues.iter().any(|issue| issue.path == *path && issue.to_string().contains(word)),
            "`{to}` gave {issues:?}, wanting `{path}` saying `{word}`",
        );
    }
}

/// Conditions that once panicked the compiler: each is refused at its transition's `when`.
#[test]
fn a_condition_that_could_panic_is_refused_by_its_path() {
    let when = r#"machines["locomotion"].transitions[0].when"#;
    let refused = |issues: &[GraphIssue], word: &str| {
        assert!(
            issues.iter().any(|issue| issue.path == when && issue.message.contains(word)),
            "wanting `{when}` saying `{word}`, got {issues:?}"
        );
    };
    refused(&issues_with(r#""!grounded""#, "\"speed \u{2265} 1.0\""), "isn't part of a condition");
    refused(&issues_with(r#""!grounded""#, &format!("\"{}grounded\"", "!".repeat(1000))), "tokens");
    refused(&issues_with(r#""!grounded""#, &format!("\"{}grounded\"", "(".repeat(5000))), "bytes long");
    // A 33rd parameter: the count is refused, and a condition reading it is refused, not shifted past
    // the 32 bits of what a transition reads.
    let games: Vec<String> =
        (0..26).map(|index| format!(r#"(name: "g{index}", from: Game("g{index}"))"#)).collect();
    let text = document()
        .replacen(
            r#"(name: "aim", from: Game("aim")),"#,
            &format!(r#"(name: "aim", from: Game("aim")), {},"#, games.join(", ")),
            1,
        )
        .replacen(r#""!grounded""#, r#""g25 > 0.0""#, 1);
    let issues = AnimGraph::from_document(&text).expect("parses").check();
    refused(&issues, "past the first 32 parameters");
    assert!(
        issues.iter().any(|issue| issue.path == "parameters" && issue.message.contains("33")),
        "{issues:?}"
    );
}

#[test]
fn too_many_of_a_thing_is_refused_by_its_list() {
    let combats = issues_with(
        r#"(name: "breathing", plays: Additive"#,
        r#"(name: "second", plays: Combat(())), (name: "breathing", plays: Additive"#,
    );
    assert!(
        combats.iter().any(|issue| issue.path == "layers" && issue.message.contains("2 combat layers")),
        "{combats:?}"
    );
    let deep = document()
        .replacen(
            r#"(name: "fall", plays: Clip(clip: "fall", looping: true)),"#,
            r#"(name: "fall", plays: Machine("m3")),"#,
            1,
        )
        .replacen(
            r#"(name: "upper", start: "aim""#,
            r#"(name: "m3", start: "a", states: [(name: "a", plays: Machine("m4"))]),
               (name: "m4", start: "a", states: [(name: "a", plays: Machine("m5"))]),
               (name: "m5", start: "a", states: [(name: "a", plays: Clip(clip: "fall"))]),
               (name: "upper", start: "aim""#,
            1,
        );
    let issues = AnimGraph::from_document(&deep).expect("parses").check();
    assert!(
        issues
            .iter()
            .any(|issue| issue.path == r#"machines["m5"]"# && issue.message.contains("5 machines deep")),
        "{issues:?}"
    );
}

#[test]
fn everything_wrong_is_named_at_once() {
    let text = document().replace(r#"start: "ground""#, r#"start: "x""#).replace("exit: 12", "exit: 999");
    let issues = AnimGraph::from_document(&text).expect("parses").check();
    assert_eq!(issues.len(), 2, "{issues:?}");
}

#[test]
fn a_malformed_document_quotes_its_line_and_an_unknown_field_is_refused() {
    let text = document().replace("smoothing: 4.0", "smoothing: \"fast\"");
    let Err(GraphError::Malformed { text: quoted, .. }) = AnimGraph::from_document(&text) else {
        panic!("it read")
    };
    assert!(quoted.contains("smoothing"), "{quoted}");
    let unknown = document().replace("smoothing: 4.0", "smoothing: 4.0, layout: (x: 1.0)");
    assert!(AnimGraph::from_document(&unknown).is_err(), "an editor's layout has no place in the graph");
    let newer = document().replace("version: 2", "version: 3");
    assert!(matches!(AnimGraph::from_document(&newer), Err(GraphError::Document(_))));
}

/// The version 1 document of the v1 module's tests: every section it can say.
fn version_1() -> String {
    format!(
        r#"(
    type: "sapphire::assets::AnimGraph",
    version: 1,
    data: (
        clips: [
            (name: "idle", source: "{SOURCE}", clip: "Idle_A", target: "{TARGET}"),
            (name: "run", source: "{SOURCE}", clip: "Running_A"),
            (name: "jab", source: "{TARGET}"),
            (name: "land", source: "{TARGET}", target: "{SOURCE}"),
        ],
        masks: [(name: "torso", bones: [(bone: Spine, weight: 1.0)])],
        locomotion: (
            start: "ground",
            smoothing: 4.0,
            states: [
                (name: "ground", plays: Blend((
                    samples: [(clip: "idle", at: (0.0, 0.0), sync: false), (clip: "run", at: (0.0, 4.7), phase: 0.25)],
                    rate: (0.75, 1.5),
                ))),
                (name: "air", plays: TakeoffBlend((standing: "run", moving: [(clip: "jab", fade: (0.6, 3.8))]))),
                (name: "land", plays: LandingBlend((standing: "land", moving: [(clip: "jab", fade: (0.6, 3.8))]))),
            ],
            transitions: [
                (from: "ground", to: "air", when: Airborne, decay: 8),
                (from: "ground", to: "air", when: AirborneMoving(1.0), decay: 8),
                (from: "air", to: "land", when: Landed, decay: 3),
                (from: "air", to: "land", when: LandedMoving(4.5), decay: 3),
                (from: "land", to: "ground", when: FinishedAtPhase(0.65384615), decay: 8),
                (from: "land", to: "ground", when: Finished, decay: 10),
                (from: "land", to: "ground", when: Moving(1.5), decay: 6),
                (from: "land", to: "ground", when: Stopped(0.5), decay: 6),
                (from: "land", to: "ground", when: ForwardBelow(0.25), decay: 6),
                (from: "ground", to: "air", when: Grounded, decay: 6),
                (from: "ground", to: "air", when: JumpPreparation(true), decay: 0),
                (from: "air", to: "ground", when: JumpPreparation(false), decay: 5),
            ],
        ),
        combat: (decay: 5, exit: 12, moves: [(move: "jab", clip: "jab"), (move: "wave", clip: "jab", mask: "torso", decay: 3)]),
        additive: [(name: "breathing", clip: "idle", mask: "torso", weight: 0.5)],
        reactions: [(name: "flinch", clip: "jab", weight: 0.7, fade_out: 8)],
        blink: Some((expression: "blink", every: (150, 360), close: 3, hold: 1, open: 7)),
    ),
)"#
    )
}

#[test]
fn a_version_1_document_converts_and_compiles() {
    let old = crate::graph::AnimGraph::from_document(&version_1()).expect("a good version 1 document");
    let graph = AnimGraph::from_document(&version_1()).expect("reads as version 2");
    assert_eq!(graph, convert::convert(&old), "reading is converting");
    let whens: Vec<&str> =
        graph.machines[0].transitions.iter().map(|transition| transition.when.as_str()).collect();
    assert_eq!(
        whens,
        [
            "!grounded",
            "!grounded && speed >= 1.0",
            "landed && grounded",
            "landed && grounded && forward >= 4.5",
            format!("finished_at({:?})", 0.653_846_15f32).as_str(),
            "finished",
            "smoothed_speed >= 1.5",
            "smoothed_speed < 0.5",
            "velocity_z < 0.25",
            "grounded",
            "preparing_jump",
            "!preparing_jump",
        ],
        "a landing graph's `Landed` is on the ground",
    );
    let names: Vec<&str> = graph.parameters.iter().map(|parameter| parameter.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "speed",
            "forward",
            "smoothed_speed",
            "velocity_x",
            "velocity_z",
            "grounded",
            "landed",
            "preparing_jump"
        ]
    );
    let layers: Vec<&str> = graph.layers.iter().map(|layer| layer.name.as_str()).collect();
    assert_eq!(layers, ["locomotion", "combat", "breathing", "reactions"]);
    let Motion::Takeoff(takeoff) = &graph.machines[0].states[1].plays else { panic!("a takeoff") };
    assert!(!takeoff.anticipate && !takeoff.carry, "not the heroine's air poses");
    let written = graph.to_document().expect("writes");
    assert_eq!(
        AnimGraph::from_document(&written).expect("reads back"),
        graph,
        "a converted graph round trips"
    );
    let mut graph = graph;
    graph.resolve(crate::graph::clip_id);
    let program = graph.compile().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(program.transitions(0)[4].phase.map(f32::to_bits), Some(0.653_846_15f32.to_bits()));
}

#[test]
fn a_v1_landed_without_a_landing_reads_landed_alone() {
    let text = version_1()
        .replace(r#"(name: "land", plays: LandingBlend((standing: "land", moving: [(clip: "jab", fade: (0.6, 3.8))]))),"#, r#"(name: "land", plays: Clip(clip: "land")),"#)
        .replace("(from: \"air\", to: \"land\", when: LandedMoving(4.5), decay: 3),", "");
    let graph = AnimGraph::from_document(&text).expect("reads");
    assert_eq!(graph.machines[0].transitions[2].when, "landed");
    // v1 started a landing only in a graph with one; this `Landed` is a plain change of state.
    assert_eq!(graph.machines[0].transitions[2].rules, TransitionRules::default());
}

#[test]
fn v1s_footwork_and_carry_say_so_in_the_document() {
    let named: String = ["brake_left", "brake_right", "launch_left", "launch_right", "walk", "jog"]
        .iter()
        .map(|name| format!(r#"(name: "{name}", source: "{SOURCE}", clip: "{name}"), "#))
        .collect();
    let jab = format!(r#"(name: "jab", source: "{TARGET}"),"#);
    let text = version_1()
        .replace(
            &jab,
            &format!(
                r#"{named}(name: "air", source: "32e5765d-fe14-552e-9361-dcb6ee8d5c36"),
                (name: "air_run", source: "e70e0e76-9728-59a9-bd18-9911fd2fb07c"), {jab}"#
            ),
        )
        .replace(
            r#"TakeoffBlend((standing: "run", moving: [(clip: "jab""#,
            r#"TakeoffAnticipating((standing: "air", moving: [(clip: "air_run""#,
        );
    let graph = AnimGraph::from_document(&text).expect("reads");
    let Motion::Blend2D(ground) = &graph.machines[0].states[0].plays else { panic!("a blend") };
    let footwork = ground.footwork.as_ref().expect("all seven names: footwork");
    assert_eq!(footwork.brake, ["brake_left".to_owned(), "brake_right".to_owned()]);
    let contacts: Vec<(&str, u32)> =
        footwork.gaits.iter().map(|gait| (gait.clip.as_str(), gait.contact.to_bits())).collect();
    assert_eq!(
        contacts,
        [
            ("walk", (24.0f32 / 42.0).to_bits()),
            ("jog", (12.0f32 / 34.0).to_bits()),
            ("run", (7.0f32 / 26.0).to_bits())
        ]
    );
    let Motion::Takeoff(takeoff) = &graph.machines[0].states[1].plays else { panic!("a takeoff") };
    assert!(takeoff.anticipate && takeoff.carry, "the heroine's air poses carry the stride");
    let mut written = AnimGraph::from_document(&graph.to_document().expect("writes")).expect("reads back");
    assert_eq!(written, graph, "the footwork's shares read back to the same bits");
    written.resolve(crate::graph::clip_id);
    written.compile().unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn conditions_compile_to_their_ops() {
    let program = program();
    let any = &program.transitions(0)[3];
    assert_eq!(
        &program.conditions[any.condition.range()],
        [
            ConditionOp::Truth(3),
            ConditionOp::Mode { parameter: 5, mode: 3 },
            ConditionOp::Not,
            ConditionOp::And
        ]
    );
    let air = &program.transitions(1)[0];
    assert_eq!(&program.conditions[air.condition.range()], [ConditionOp::Finished]);
}

#[test]
fn a_finish_out_of_what_never_finishes_converts_to_false() {
    let text = version_1()
        .replacen(
            r#"(name: "air", plays: TakeoffBlend"#,
            r#"(name: "still", plays: Clip(clip: "idle", looping: true)), (name: "air", plays: TakeoffBlend"#,
            1,
        )
        .replacen(
            "(from: \"ground\", to: \"air\", when: Airborne, decay: 8),",
            "(from: \"ground\", to: \"air\", when: Airborne, decay: 8), \
             (from: \"still\", to: \"ground\", when: Finished, decay: 4), \
             (from: \"ground\", to: \"still\", when: FinishedAtPhase(0.5), decay: 4),",
            1,
        );
    let graph = AnimGraph::from_document(&text).expect("version 1 accepts it");
    let whens: Vec<&str> =
        graph.machines[0].transitions[1..3].iter().map(|transition| transition.when.as_str()).collect();
    assert_eq!(whens, ["false", "false"], "v1 never let them hold");
    graph.compile().unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn the_converters_layers_keep_clear_of_an_additive_layers_name() {
    let text = version_1().replacen(r#"(name: "breathing", clip"#, r#"(name: "combat", clip"#, 1);
    let graph = AnimGraph::from_document(&text).expect("reads");
    let names: Vec<&str> = graph.layers.iter().map(|layer| layer.name.as_str()).collect();
    assert_eq!(names, ["locomotion", "combat_2", "combat", "reactions"]);
    graph.compile().unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn v1s_condition_keyed_rules_are_said_and_a_hand_written_transition_has_none() {
    let graph = AnimGraph::from_document(&version_1()).expect("reads");
    let none = TransitionRules::default();
    let rules: Vec<TransitionRules> =
        graph.machines[0].transitions.iter().map(|transition| transition.rules).collect();
    assert_eq!(
        rules,
        [
            none,
            TransitionRules { hands_off: true, ..none },
            TransitionRules { lands: true, ..none },
            TransitionRules { lands: true, ..none },
            TransitionRules { at_drawn: true, catch: true, ..none },
            TransitionRules { at_drawn: true, ..none },
            TransitionRules { waits_for_landing: true, ..none },
            TransitionRules { waits_for_landing: true, ..none },
            TransitionRules { waits_for_landing: true, ..none },
            none,
            TransitionRules { rebound: true, ..none },
            none,
        ]
    );
    // The same condition written by hand brings no rule; said, it does, and the program carries it.
    let program = program();
    assert!(program.transitions.iter().all(|transition| transition.rules.is_none()));
    let text = document().replacen(
        r#"when: "landed && grounded", decay: 3)"#,
        r#"when: "landed && grounded", decay: 3, rules: (lands: true))"#,
        1,
    );
    let mut said = AnimGraph::from_document(&text).expect("reads");
    assert_eq!(AnimGraph::from_document(&said.to_document().expect("writes")).expect("reads"), said);
    said.resolve(|clip| AssetId::from_bytes([clip.name.len() as u8; 16]));
    let program = said.compile().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(program.transitions[1].rules, TransitionRules { lands: true, ..none });
}

/// Each clip of an additive machine layer has its own reference, its first tick, blended as the pose is:
/// the layer samples its clips twice.
#[test]
fn an_additive_machine_layer_of_several_clips_compiles_at_twice_their_cost() {
    let text = document().replacen(
        "mask: Some(\"upper\"), weight: 0.5)))",
        "mask: Some(\"upper\"), blend: Additive, weight: 0.5)))",
        1,
    );
    let mut graph = AnimGraph::from_document(&text).expect("reads");
    graph.resolve(|clip| AssetId::from_bytes([clip.name.len() as u8; 16]));
    let added = graph.compile().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(added.cost.clips_sampled, program().cost.clips_sampled + 2);
}

/// Compiling for `Target::Simulation` refuses, by path, every node only presentation may run, and nothing else.
#[test]
fn a_simulation_compile_refuses_what_only_presentation_runs() {
    let graph = graph();
    let Err(CompileError { issues }) = graph.compile_for(Target::Simulation) else {
        panic!("it holds presentation's nodes")
    };
    let paths: Vec<&str> = issues.iter().map(|issue| issue.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            r#"parameters["mode"].from"#,
            r#"parameters["aim"].from"#,
            r#"layers["combat"].plays.moves["jab"].variants[0]"#,
            r#"events["footstep"]"#,
            r#"events["swish"]"#,
        ]
    );
    assert_eq!(graph.compile().expect("presentation runs them").target, Target::Presentation);
    // Without them, it compiles for the fixed step, and says so.
    let text = document()
        .replacen(r#"(name: "mode", from: Traversal(Mode)),"#, "", 1)
        .replacen("grounded && mode != Swimming", "grounded", 1)
        .replacen(r#"(name: "aim", from: Game("aim")),"#, r#"(name: "aim", from: Locomotion(Speed)),"#, 1)
        .replacen(r#", variants: [(from: Behind, clip: "jab_behind")]"#, "", 1)
        .replacen(
            r#"(name: "footstep", from: Foot((foot: Left, contact: Down))),
            (name: "swish", from: Clip("swish"), min_weight: 0.8),"#,
            "",
            1,
        );
    let mut plain = AnimGraph::from_document(&text).expect("reads");
    plain.resolve(|clip| AssetId::from_bytes([clip.name.len() as u8; 16]));
    let program = plain.compile_for(Target::Simulation).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(program.target, Target::Simulation);
    assert_eq!(GraphProgram::read(&program.to_cooked().expect("cooks")).expect("reads"), program);
}

#[test]
fn a_version_1_document_its_own_check_refuses_never_converts() {
    let text = version_1().replacen("when: Moving(1.5)", "when: Moving(-1.0)", 1);
    assert!(
        matches!(AnimGraph::from_document(&text), Err(GraphError::Invalid { .. })),
        "a negative speed is v1's refusal"
    );
}

#[test]
fn a_machine_played_only_from_an_orphan_is_told_so() {
    let text = document()
        .replacen(
            r#"(name: "upper", plays: Machine((machine: "upper", mask: Some("upper"), weight: 0.5))),"#,
            "",
            1,
        )
        .replacen(
            r#"(name: "upper", start: "aim""#,
            r#"(name: "lost", start: "a", states: [(name: "a", plays: Machine("upper"))]),
               (name: "upper", start: "aim""#,
            1,
        );
    let issues = AnimGraph::from_document(&text).expect("parses").check();
    let said = |path: &str, word: &str| {
        issues.iter().any(|issue| issue.path == path && issue.message.contains(word))
    };
    assert!(said(r#"machines["lost"]"#, "plays nowhere"), "{issues:?}");
    assert!(said(r#"machines["upper"]"#, "inside machines no layer reaches"), "{issues:?}");
    assert!(!said(r#"machines["upper"]"#, "inside itself"), "{issues:?}");
}
