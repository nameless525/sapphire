//! The bodies of what a script can call, one for each row of `api.toml`: plain Rust on a hook's frame, and
//! numbers in and out. `bindings/generated.rs` wraps each, so that's where the rules for a binding are kept, and
//! none of them has to be remembered here.
//!
//! **What a number means.** A wrong kind of argument never gets here: the wrapper makes it a fault and a
//! zero. A number that can't be what it's asked to be is a fault too, when a command would act on it,
//! since a hook asking for nonsense shouldn't half happen. A query of someone who isn't there reads as
//! nothing, because an actor can be gone by the time a hook asks.

use sapphire_gameplay::script::{ActorFacts, BOARD_SLOTS};
use sapphire_gameplay::{ActorId, HookFrame, ScriptCommand};

/// `value` if it's exactly a `u32`. A plain cast would make -1, what a query says for nobody, into 0.
fn whole(value: f64) -> Option<u32> {
    let whole = value as u32;
    (f64::from(whole) == value).then_some(whole)
}

fn facts(frame: &HookFrame, actor: f64) -> Option<&ActorFacts> {
    frame.actor(whole(actor)?)
}

/// `value` as an actor, or a fault: `None` means the hook's commands are already lost.
fn actor_id(frame: &mut HookFrame, actor: f64) -> Option<ActorId> {
    let id = whole(actor).and_then(ActorId::from_raw);
    frame.fault |= id.is_none();
    id
}

fn target_number(target: Option<ActorId>) -> f64 {
    target.map_or(-1.0, |target| f64::from(target.raw()))
}

pub(crate) fn tick(frame: &mut HookFrame) -> f64 {
    frame.tick as f64
}

pub(crate) fn health(frame: &mut HookFrame, actor: f64) -> f64 {
    facts(frame, actor).map_or(0.0, |facts| f64::from(facts.health))
}

pub(crate) fn max_health(frame: &mut HookFrame, actor: f64) -> f64 {
    facts(frame, actor).map_or(0.0, |facts| f64::from(facts.max_health))
}

pub(crate) fn is_defeated(frame: &mut HookFrame, actor: f64) -> f64 {
    facts(frame, actor).map_or(0.0, |facts| f64::from(u8::from(facts.defeated)))
}

pub(crate) fn faction(frame: &mut HookFrame, actor: f64) -> f64 {
    facts(frame, actor).map_or(0.0, |facts| f64::from(facts.faction))
}

pub(crate) fn position(frame: &mut HookFrame, actor: f64) -> (f64, f64, f64) {
    facts(frame, actor).map_or((0.0, 0.0, 0.0), |facts| {
        let at = facts.position;
        (f64::from(at.x), f64::from(at.y), f64::from(at.z))
    })
}

pub(crate) fn soft_lock_target(frame: &mut HookFrame, actor: f64) -> f64 {
    target_number(facts(frame, actor).and_then(|facts| facts.soft_lock))
}

pub(crate) fn brain_target(frame: &mut HookFrame, actor: f64) -> f64 {
    target_number(facts(frame, actor).and_then(|facts| facts.brain_target))
}

pub(crate) fn random(frame: &mut HookFrame) -> f64 {
    // A draw moves the simulation's numbers, which a read-only frame mustn't.
    if frame.is_read_only() {
        frame.fault = true;
        return 0.0;
    }
    f64::from(frame.rng.unit_f32())
}

pub(crate) fn board(frame: &mut HookFrame, slot: f64) -> f64 {
    match whole(slot).map(|slot| slot as usize).filter(|&slot| slot < BOARD_SLOTS) {
        Some(slot) => frame.board[slot],
        None => {
            frame.fault = true;
            0.0
        }
    }
}

pub(crate) fn set_board(frame: &mut HookFrame, slot: f64, value: f64) {
    if frame.is_read_only() {
        frame.fault = true;
        return;
    }
    match whole(slot).map(|slot| slot as usize).filter(|&slot| slot < BOARD_SLOTS) {
        Some(slot) => frame.board[slot] = value,
        None => frame.fault = true,
    }
}

pub(crate) fn flag(frame: &mut HookFrame, name: f64) -> f64 {
    match whole(name).and_then(|place| frame.name(place as usize)) {
        Some(name) => f64::from(frame.flag(name)),
        None => {
            frame.fault = true;
            0.0
        }
    }
}

pub(crate) fn set_flag(frame: &mut HookFrame, name: f64, value: f64) {
    let name = whole(name).and_then(|place| frame.name(place as usize));
    // Exactly an `i32`, so 1.5, a NaN or 2^31 is the script's mistake rather than a rounding.
    let value = Some(value as i32).filter(|&whole| f64::from(whole) == value);
    match (name, value) {
        (Some(name), Some(value)) => frame.push(ScriptCommand::SetFlag { name, value }),
        _ => frame.fault = true,
    }
}

pub(crate) fn slow_motion(frame: &mut HookFrame, per_mille: f64, steps: f64) {
    let steps = whole(steps).and_then(|steps| u16::try_from(steps).ok());
    match (whole(per_mille), steps) {
        (Some(per_mille), Some(steps)) => frame.push(ScriptCommand::SlowMotion { per_mille, steps }),
        _ => frame.fault = true,
    }
}

pub(crate) fn mark(frame: &mut HookFrame, value: f64) {
    frame.push(ScriptCommand::Mark(value));
}

pub(crate) fn force_move(frame: &mut HookFrame, actor: f64, name: f64) {
    let Some(actor) = actor_id(frame, actor) else { return };
    match whole(name).and_then(|place| frame.name(place as usize)) {
        Some(name) => frame.push(ScriptCommand::ForceMove { actor, name }),
        None => frame.fault = true,
    }
}

pub(crate) fn give_brain(frame: &mut HookFrame, actor: f64) {
    if let Some(actor) = actor_id(frame, actor) {
        frame.push(ScriptCommand::GiveBrain { actor });
    }
}

pub(crate) fn give_soft_lock(frame: &mut HookFrame, actor: f64) {
    if let Some(actor) = actor_id(frame, actor) {
        frame.push(ScriptCommand::GiveSoftLock { actor });
    }
}

#[cfg(test)]
mod tests {
    use sapphire_core::Name;
    use sapphire_gameplay::script::ActorFacts;
    use sapphire_math::Vec3;

    use super::*;

    fn frame() -> HookFrame {
        let mut frame = HookFrame::new(2);
        frame.set_actors([ActorFacts {
            id: ActorId::from_raw(3).unwrap(),
            health: 40,
            max_health: 100,
            defeated: true,
            faction: 2,
            position: Vec3::new(1.0, 2.0, 3.0),
            soft_lock: ActorId::from_raw(7),
            brain_target: None,
        }]);
        frame
    }

    #[test]
    fn queries_read_who_is_there_and_nothing_of_anyone_else() {
        let mut frame = frame();
        assert_eq!(health(&mut frame, 3.0), 40.0);
        assert_eq!(max_health(&mut frame, 3.0), 100.0);
        assert_eq!(is_defeated(&mut frame, 3.0), 1.0);
        assert_eq!(faction(&mut frame, 3.0), 2.0);
        assert_eq!(position(&mut frame, 3.0), (1.0, 2.0, 3.0));
        assert_eq!(soft_lock_target(&mut frame, 3.0), 7.0);
        assert_eq!(brain_target(&mut frame, 3.0), -1.0);
        for nobody in [4.0, 0.0, -1.0, 3.5, f64::NAN, f64::INFINITY] {
            assert_eq!(health(&mut frame, nobody), 0.0, "{nobody}");
            assert_eq!(soft_lock_target(&mut frame, nobody), -1.0, "{nobody}");
        }
        assert!(!frame.fault, "a query of someone gone isn't the script's mistake");
    }

    /// `brain_target`'s -1 is the likeliest wrong number a script hands a command, and a plain cast would
    /// aim it at actor 0 or, worse, on through a table.
    #[test]
    fn a_command_at_nobody_is_a_fault() {
        for nobody in [-1.0, 0.0, 1.5, f64::NAN, 1e12] {
            let mut frame = frame();
            give_brain(&mut frame, nobody);
            assert!(frame.fault && frame.queued().is_empty(), "{nobody}");
        }
    }

    #[test]
    fn a_command_at_someone_gone_is_asked_for_all_the_same() {
        let mut frame = frame();
        give_soft_lock(&mut frame, 9.0);
        assert!(!frame.fault);
        assert_eq!(frame.queued(), [ScriptCommand::GiveSoftLock { actor: ActorId::from_raw(9).unwrap() }]);
    }

    #[test]
    fn a_name_is_a_place_in_the_chunks_list() {
        let mut frame = frame();
        frame.set_names(vec![Name::new("roar")]);
        force_move(&mut frame, 3.0, 1.0);
        assert!(!frame.fault);
        assert_eq!(
            frame.queued(),
            [ScriptCommand::ForceMove { actor: ActorId::from_raw(3).unwrap(), name: Name::new("roar") }]
        );
        for place in [0.0, 2.0, -1.0, 0.5] {
            let mut frame = self::frame();
            frame.set_names(vec![Name::new("roar")]);
            force_move(&mut frame, 3.0, place);
            assert!(frame.fault, "{place}");
        }
    }

    #[test]
    fn the_board_stops_where_it_ends() {
        let mut frame = frame();
        set_board(&mut frame, 63.0, 5.0);
        assert_eq!(board(&mut frame, 63.0), 5.0);
        assert!(!frame.fault);
        for slot in [64.0, -1.0, 0.5, f64::NAN] {
            let mut frame = self::frame();
            set_board(&mut frame, slot, 1.0);
            assert!(frame.fault, "{slot}");
            let mut frame = self::frame();
            assert_eq!(board(&mut frame, slot), 0.0);
            assert!(frame.fault, "{slot}");
        }
    }

    #[test]
    fn a_flag_is_read_and_set_by_its_place_in_the_chunks_names() {
        let mut flags = sapphire_gameplay::Flags::default();
        flags.set(Name::new("ward_lit"), 3);
        let mut frame = frame();
        frame.set_names(vec![Name::new("ward_lit"), Name::new("met_guide")]);
        frame.set_flags(&flags);
        assert_eq!(flag(&mut frame, 1.0), 3.0);
        assert_eq!(flag(&mut frame, 2.0), 0.0, "nothing set it");
        set_flag(&mut frame, 2.0, -4.0);
        assert!(!frame.fault);
        assert_eq!(frame.queued(), [ScriptCommand::SetFlag { name: Name::new("met_guide"), value: -4 }]);
        for (place, value) in [(0.0, 1.0), (3.0, 1.0), (1.0, 1.5), (1.0, f64::NAN), (1.0, 2_147_483_648.0)] {
            let mut frame = self::frame();
            frame.set_names(vec![Name::new("ward_lit")]);
            set_flag(&mut frame, place, value);
            assert!(frame.fault && frame.queued().is_empty(), "{place} {value}");
        }
        let mut frame = self::frame();
        assert_eq!(flag(&mut frame, 1.0), 0.0);
        assert!(frame.fault, "a place the chunk doesn't list");
    }

    /// What a dialogue's condition runs on: everything that would move a checksum is a fault, and does
    /// nothing.
    #[test]
    fn a_read_only_frame_refuses_whatever_would_write() {
        let mut frame = frame();
        frame.set_names(vec![Name::new("roar")]);
        frame.set_read_only(true);
        let (rng, board) = (frame.rng, frame.board);
        let writes: [fn(&mut HookFrame); 5] = [
            |frame| set_board(frame, 0.0, 1.0),
            |frame| {
                random(frame);
            },
            |frame| mark(frame, 1.0),
            |frame| force_move(frame, 3.0, 1.0),
            |frame| set_flag(frame, 1.0, 1.0),
        ];
        for write in writes {
            frame.fault = false;
            write(&mut frame);
            assert!(frame.fault);
        }
        assert!(frame.queued().is_empty());
        assert_eq!(
            (frame.rng.unit_f32(), frame.board),
            (
                {
                    let mut rng = rng;
                    rng.unit_f32()
                },
                board
            )
        );
        frame.fault = false;
        assert_eq!(health(&mut frame, 3.0), 40.0, "reading is fine");
        assert!(!frame.fault);
    }

    #[test]
    fn a_slow_motion_too_long_for_its_field_is_a_fault() {
        let mut frame = frame();
        slow_motion(&mut frame, 500.0, 65_536.0);
        assert!(frame.fault);
        let mut frame = self::frame();
        slow_motion(&mut frame, 500.0, 65_535.0);
        assert_eq!(frame.queued(), [ScriptCommand::SlowMotion { per_mille: 500, steps: 65_535 }]);
    }
}
