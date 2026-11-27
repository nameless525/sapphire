//! A version 2 graph's [`GraphProgram`] bound to a character: the same [`GraphBinding`] a version 1
//! graph resolves to, which the one [`GraphPlayer`](super::GraphPlayer) plays. A converted graph binds
//! to what its version 1 document does, rule for rule, so it plays the same poses bit for bit
//! (`graph::v2_tests`).

use sapphire_assets::clip::Marker;
use sapphire_assets::graph::v2::program::{BlendAxes, GraphProgram, PoseOp, ProgramEntry};
use sapphire_assets::graph::v2::{self, Contact, Foot, LayerBlend, LocomotionValue, ParameterSource};
use sapphire_assets::rig::{ArchivedCharacterRig, HumanBone};
use sapphire_assets::skeleton::ArchivedSkeleton;
use sapphire_core::Name;

use super::ground_footwork::FootworkClips;
use super::{
    AcceleratingCatch, Added, Axes, BlendPoint, BoundEvent, BoundMachine, ClipSource, EventFrom,
    GraphBindError, GraphBinding, Guard, LayerOp, MachineLayer, Motion, MoveEntry, OneShot, Sample, Space,
    State, StateChange, Value, bind_blink, bind_mask,
};

impl GraphBinding {
    /// Resolves a compiled `program` for this character, as [`Self::new`] resolves a version 1 graph:
    /// its `skeleton`, its `rig` if masks, a blink or footwork need one, and how many morph targets its
    /// mesh has. Every clip must be loaded and cooked for this skeleton. A program read from bytes
    /// has passed [`GraphProgram::validate`]; one built any other way is checked here.
    pub fn from_program(
        program: &GraphProgram,
        skeleton: &ArchivedSkeleton,
        rig: Option<&ArchivedCharacterRig>,
        morph_targets: usize,
        clips: &(impl ClipSource + ?Sized),
    ) -> Result<Self, GraphBindError> {
        program.validate().map_err(GraphBindError::InvalidProgram)?;
        for clip in &program.clips {
            let name = || clip.name.clone();
            let cooked = clips
                .clip(clip.id)
                .ok_or_else(|| GraphBindError::MissingClip { name: name(), id: clip.id })?;
            cooked.check_skeleton(skeleton).map_err(|source| GraphBindError::WrongSkeleton {
                name: name(),
                id: clip.id,
                source,
            })?;
        }
        let masks =
            program.masks.iter().map(|mask| bind_mask(mask, skeleton, rig)).collect::<Result<Vec<_>, _>>()?;
        let find_joint = |bone| {
            rig?.humanoid
                .iter()
                .find(|entry| entry.bone == bone)
                .map(|entry| usize::from(entry.joint.to_native()))
                .filter(|&joint| joint < skeleton.len())
        };
        let parameters: Vec<Value> = program
            .parameters
            .iter()
            .scan(0u8, |slots, parameter| {
                Some(match &parameter.from {
                    ParameterSource::Locomotion(value) => Value::Locomotion(*value),
                    ParameterSource::Traversal(_) => Value::Mode,
                    ParameterSource::Game(_) => {
                        *slots += 1;
                        Value::Slot(*slots - 1)
                    }
                })
            })
            .collect();
        let slots = program
            .parameters
            .iter()
            .filter_map(|parameter| match &parameter.from {
                ParameterSource::Game(name) => Some(Name::intern(name)),
                _ => None,
            })
            .collect();
        let reads = |register: u8, wanted: LocomotionValue| matches!(parameters.get(usize::from(register)), Some(Value::Locomotion(value)) if *value == wanted);

        // The states, each one op: what it plays.
        let mut footwork: Option<&v2::program::ProgramFootwork> = None;
        let mut carries = Vec::with_capacity(program.states.len());
        let mut states = Vec::with_capacity(program.states.len());
        for state in &program.states {
            let op =
                program.ops[state.ops.range()].first().ok_or(GraphBindError::InvalidProgram("a state"))?;
            let mut carry = false;
            let motion = match *op {
                PoseOp::Clip { clip, looping, sync, .. } => Motion::Clip { clip, looping, sync },
                PoseOp::Blend { blend, .. } => {
                    let blend = &program.blends[usize::from(blend)];
                    let axes = match blend.axes {
                        // v1's blend space reads the smoothed velocity itself.
                        BlendAxes::Two(x, y)
                            if reads(x, LocomotionValue::SmoothedX)
                                && reads(y, LocomotionValue::SmoothedZ) =>
                        {
                            Axes::Velocity
                        }
                        BlendAxes::Two(x, y) => Axes::Two(x, y),
                        BlendAxes::One(axis) => Axes::One(axis),
                    };
                    let authored = &program.samples[blend.samples.range()];
                    let points: Vec<BlendPoint> =
                        authored.iter().map(|sample| BlendPoint::new(sample.at[0], sample.at[1])).collect();
                    let samples = authored
                        .iter()
                        .zip(&points)
                        .map(|(sample, point)| Sample {
                            clip: sample.clip,
                            at: *point,
                            // Feet match a speed only over the velocity: elsewhere a synced clip plays
                            // at its own rate.
                            speed: if matches!(axes, Axes::Velocity) { point.speed() } else { 0.0 },
                            phase: sample.phase,
                            sync: sample.sync,
                        })
                        .collect();
                    if let Some(index) = blend.footwork {
                        let own = &program.footwork[usize::from(index)];
                        if footwork.is_some_and(|bound| bound != own) {
                            return Err(GraphBindError::Footworks);
                        }
                        footwork = Some(own);
                    }
                    Motion::Blend(Space {
                        points,
                        samples,
                        rate: blend.rate,
                        axes,
                        group: blend.sync,
                        footwork: blend.footwork.is_some(),
                    })
                }
                PoseOp::Entry { entry, .. } => {
                    let entry = &program.entries[usize::from(entry)];
                    carry = entry.carry;
                    let shots = one_shot(entry)?;
                    if entry.landing { Motion::Touchdown(shots) } else { Motion::Takeoff(shots) }
                }
                PoseOp::Machine { machine, .. } => Motion::Machine(machine),
                _ => return Err(GraphBindError::InvalidProgram("a state plays a layer's op")),
            };
            carries.push(carry);
            states.push(State { name: Name::intern(&state.name), machine: state.machine, motion });
        }
        let mut machines: Vec<BoundMachine> = program
            .machines
            .iter()
            .map(|machine| BoundMachine {
                name: Name::intern(&machine.name),
                start: machine.start,
                played_by: None,
            })
            .collect();
        for (index, state) in states.iter().enumerate() {
            if let Motion::Machine(inner) = state.motion {
                machines[usize::from(inner)].played_by = Some(index as u16);
            }
        }

        let mut changes = Vec::with_capacity(program.transitions.len());
        for (index, machine) in program.machines.iter().enumerate() {
            for transition in &program.transitions[machine.transitions.range()] {
                changes.push(StateChange {
                    machine: index as u16,
                    from: transition.from,
                    to: transition.to,
                    guard: Guard::Program { condition: transition.condition, finished: transition.finished },
                    decay: f32::from(transition.decay),
                    rules: transition.rules,
                    phase: transition.phase,
                });
            }
        }

        // The layers, bottom first: the first plays the base machine over the whole body.
        let mut ops = Vec::with_capacity(usize::from(program.layers.len));
        let mut base = None;
        let mut layer = 0u8;
        let layer_ops = program.layer_ops();
        let mut index = 0;
        while let Some(&op) = layer_ops.get(index) {
            index += 1;
            match op {
                PoseOp::Machine { machine, .. } if base.is_none() => {
                    base = Some(machine);
                    ops.push(LayerOp::Base);
                }
                PoseOp::Machine { machine, .. } => {
                    let Some(&PoseOp::Layer { mask, blend, weight, .. }) = layer_ops.get(index) else {
                        return Err(GraphBindError::InvalidProgram("a layer's machine lays onto nothing"));
                    };
                    index += 1;
                    ops.push(LayerOp::Machine(MachineLayer {
                        machine,
                        slot: layer,
                        mask,
                        additive: blend == LayerBlend::Additive,
                        weight,
                    }));
                }
                PoseOp::Combat { .. } => ops.push(LayerOp::Combat),
                PoseOp::Additive { additive, .. } => ops.push(LayerOp::Additive(additive)),
                PoseOp::Reactions { .. } => ops.push(LayerOp::Reactions),
                _ => return Err(GraphBindError::InvalidProgram("a layer's op")),
            }
            layer += 1;
        }
        let base = base.ok_or(GraphBindError::InvalidProgram("no layer plays a machine"))?;
        if ops.first().is_none_or(|op| !matches!(op, LayerOp::Base)) {
            return Err(GraphBindError::InvalidProgram("the first layer plays a machine"));
        }

        let mut moves: Vec<MoveEntry> = program
            .moves
            .iter()
            .map(|row| MoveEntry {
                name: Name::intern(&row.name),
                clip: row.clip,
                mask: row.mask,
                decay: f32::from(row.decay),
            })
            .collect();
        moves.sort_unstable_by_key(|entry| entry.name);
        let additive = program
            .additive
            .iter()
            .map(|layer| Added {
                name: Name::intern(&layer.name),
                clip: layer.clip,
                mask: layer.mask,
                weight: layer.weight,
                fade_out: 0.0,
                restart: 0.0,
            })
            .collect();
        let reactions = program
            .reactions
            .iter()
            .map(|layer| Added {
                name: Name::intern(&layer.name),
                clip: layer.clip,
                mask: layer.mask,
                weight: layer.weight,
                fade_out: f32::from(layer.fade_out),
                restart: f32::from(layer.restart),
            })
            .collect();
        let blink = program.blink.as_ref().map(|blink| bind_blink(blink, rig, morph_targets)).transpose()?;
        let sync = program.sync.iter().map(|group| (bone(group.foot), marker(group.contact))).collect();
        let events = program
            .events
            .iter()
            .map(|event| BoundEvent {
                name: Name::intern(&event.name),
                from: match &event.from {
                    v2::program::ProgramEventSource::Foot { foot, contact } => {
                        EventFrom::Foot { foot: bone(*foot), marker: marker(*contact) }
                    }
                    v2::program::ProgramEventSource::Clip(name) => EventFrom::Clip(name.clone()),
                },
                min_weight: event.min_weight,
            })
            .collect();

        let resume_joints =
            [HumanBone::LeftFoot, HumanBone::LeftToes, HumanBone::RightFoot, HumanBone::RightToes]
                .map(find_joint);
        let takeoff =
            states.iter().position(|state| matches!(state.motion, Motion::Takeoff(_))).map(|i| i as u16);
        let takeoff_carry = takeoff.filter(|&state| carries[usize::from(state)]).and_then(|_| {
            Some([
                find_joint(HumanBone::UpperChest)?,
                find_joint(HumanBone::LeftShoulder)?,
                find_joint(HumanBone::LeftUpperArm)?,
            ])
        });
        let footwork = footwork.and_then(|footwork| {
            let (hips, spine) = find_joint(HumanBone::Hips).zip(find_joint(HumanBone::Spine))?;
            let clips = [footwork.brake, footwork.launch];
            Some(FootworkClips::of(clips, footwork.gaits.clone(), skeleton, hips, spine))
        });
        let landing_aware = changes.iter().any(|change| change.rules.lands)
            || states.iter().any(|state| matches!(state.motion, Motion::Touchdown(_)));
        let mut binding = Self {
            clips: program.clips.iter().map(|clip| clip.id).collect(),
            masks,
            machines,
            states,
            changes,
            conditions: program.conditions.clone(),
            parameters,
            slots,
            sync,
            events,
            ops,
            start: 0,
            smoothing: program.smoothing,
            landing_aware,
            target: Some(program.target),
            takeoff,
            resume_joints,
            resume_hips: find_joint(HumanBone::Hips),
            takeoff_carry,
            footwork,
            moves,
            combat_exit: program.combat.map_or(0.0, |combat| f32::from(combat.exit)),
            additive,
            reactions,
            blink,
        };
        binding.start = binding.leaf(binding.machines[usize::from(base)].start);
        Ok(binding)
    }
}

/// A takeoff's or a landing's clips: standing, moving, running and an accelerating catch.
fn one_shot(entry: &ProgramEntry) -> Result<OneShot, GraphBindError> {
    let moving = entry.moving.first().ok_or(GraphBindError::InvalidProgram("an entry of no moving clip"))?;
    let running = entry.moving.get(1);
    let accelerating = entry.moving.get(2);
    Ok(OneShot {
        clips: [
            entry.standing,
            moving.clip,
            running.map_or(0, |sample| sample.clip),
            accelerating.map_or(0, |sample| sample.clip),
        ],
        departure: moving.fade,
        touchdown: running.map(|sample| sample.fade),
        anticipate: entry.anticipate,
        accelerating: accelerating.and_then(|sample| {
            Some(AcceleratingCatch { departure: sample.short_lead?, touchdown: sample.fade })
        }),
    })
}

fn bone(foot: Foot) -> HumanBone {
    match foot {
        Foot::Left => HumanBone::LeftFoot,
        Foot::Right => HumanBone::RightFoot,
    }
}

fn marker(contact: Contact) -> Marker {
    match contact {
        Contact::Down => Marker::Down,
        Contact::Up => Marker::Up,
    }
}
