//! A tick's input as plain data: what the app gives [`crate::Simulation::step`] and a replay stores.

use sapphire_ecs::prelude::*;
use sapphire_math::Vec2;

use crate::actors::ActorId;

/// A set of pressed buttons: the combat slice's verbs, as the app's bindings map them. Which button
/// does what is the move system's business.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Buttons(u16);

impl Buttons {
    /// Nothing pressed.
    pub const NONE: Self = Self(0);
    pub const LIGHT: Self = Self(1 << 0);
    pub const HEAVY: Self = Self(1 << 1);
    pub const DODGE: Self = Self(1 << 2);
    pub const SKILL: Self = Self(1 << 3);
    pub const ULTIMATE: Self = Self(1 << 4);
    pub const SWAP: Self = Self(1 << 5);
    pub const LOCK_ON: Self = Self(1 << 6);
    pub const JUMP: Self = Self(1 << 7);
    /// No button: this tick's input came from a thumb on a screen, so a soft lock weighs its choice by
    /// [`TargetingTuning::touch`](crate::targeting::TargetingTuning::touch). A tick says, rather than a
    /// run, because a player can pick up a pad halfway through a fight. No intent ever names it.
    pub const TOUCH: Self = Self(1 << 8);

    /// Every button there is. Adding one is a change to the byte layout's meaning: older replays
    /// still read, and an older build refuses a new file's button rather than ignoring it.
    pub const ALL: Self = Self((1 << 9) - 1);

    /// The set these bits name, or `None` if a bit names no button.
    #[must_use]
    pub const fn from_bits(bits: u16) -> Option<Self> {
        if bits & !Self::ALL.0 == 0 { Some(Self(bits)) } else { None }
    }

    /// The set as bits, as the byte layout stores it.
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Whether every button of `other` is in this set. The empty set is in every set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether nothing is pressed.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The buttons in this set and not in `other`: what was pressed since `other`.
    #[must_use]
    pub const fn minus(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// The buttons in either set.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOr for Buttons {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

/// A stick quantized to whole steps, −127 to 127 on each axis, so 0 is exactly the middle and the
/// range is the same both ways. It's world-space: +x is world +X and +y is world +Z, since the
/// simulation never reads a camera. The app turns the device's stick by the camera's yaw first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Stick {
    x: i8,
    y: i8,
}

impl Stick {
    /// At rest.
    pub const CENTER: Self = Self { x: 0, y: 0 };

    /// Steps per unit of [`Self::from_axes`] and [`Self::to_vec2`].
    const STEPS: f32 = 127.0;

    /// A stick at these steps, with −128 read as −127.
    #[must_use]
    pub const fn new(x: i8, y: i8) -> Self {
        Self { x: if x == i8::MIN { -127 } else { x }, y: if y == i8::MIN { -127 } else { y } }
    }

    /// Quantizes axes, each within [−1, 1]: what the app calls when it fills in a tick, once it has
    /// turned the device's stick by the camera's yaw. The nearest step, past the range is its end,
    /// and NaN is the middle.
    #[must_use]
    pub fn from_axes(x: f32, y: f32) -> Self {
        let step = |axis: f32| (axis.clamp(-1.0, 1.0) * Self::STEPS).round() as i8;
        Self { x: step(x), y: step(y) }
    }

    /// The x axis's steps: +127 is fully toward world +X.
    #[must_use]
    pub const fn x(self) -> i8 {
        self.x
    }

    /// The y axis's steps: +127 is fully toward world +Z.
    #[must_use]
    pub const fn y(self) -> i8 {
        self.y
    }

    /// Each axis within [−1, 1]: an exact division of whole numbers. A full tilt rounded axis by axis
    /// can be up to half a step per axis long of 1, at most 1.0056, so read a speed with a limit.
    #[must_use]
    pub fn to_vec2(self) -> Vec2 {
        Vec2::new(f32::from(self.x) / Self::STEPS, f32::from(self.y) / Self::STEPS)
    }
}

/// One actor's input for one tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ActorActions {
    pub buttons: Buttons,
    pub stick: Stick,
}

impl ActorActions {
    /// Nothing pressed and the stick at rest: any actor a tick doesn't mention.
    pub const NEUTRAL: Self = Self { buttons: Buttons::NONE, stick: Stick::CENTER };

    fn is_neutral(self) -> bool {
        self == Self::NEUTRAL
    }
}

/// Bytes per actor in [`TickActions`]'s layout.
const ENTRY_LEN: usize = 8;

/// An entry of [`TickActions`]: an actor's raw ID, 0 in a slot not in use, and its input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct Entry {
    actor: u32,
    actions: ActorActions,
}

/// One tick's input for the whole world: what the app gives [`crate::Simulation::step`] and a replay
/// stores.
///
/// The app turns `sapphire_input`'s actions into these before the step, so the simulation never
/// sees a device, and a replay needs nothing else. A stick's floats never reach the simulation, so
/// two runs can't differ by how a device rounds.
///
/// Copies freely and never allocates: at most [`Self::CAPACITY`] actors have input in a tick. Enemies
/// don't count, since their brains drive them from inside the simulation (spec §5.22).
///
/// # The byte layout
///
/// [`Self::write_to`] writes, in layout version [`Self::LAYOUT_VERSION`]:
///
/// | Bytes | What |
/// |---|---|
/// | 1 | how many actors have input this tick |
/// | 8 each | the actor's ID (`u32`), its buttons (`u16`), both little-endian, then the stick's x and y (`i8`) |
///
/// Actors come in ID order, each once, and neutral actors aren't listed, so one set of inputs has
/// one encoding. Reading refuses anything else, unknown buttons included. A container (a replay
/// file) records the version, and a change to this layout takes a new one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TickActions {
    /// In ID order, with unused slots zeroed, so equal input compares and hashes equal.
    entries: [Entry; Self::CAPACITY],
    len: u8,
}

/// [`TickActions`] has no room for another actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a tick holds input for at most {} actors", TickActions::CAPACITY)]
pub struct TickActionsFull;

/// Bytes that aren't a [`TickActions`] in this layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ActionsError {
    #[error("the actions stop short: {needed} bytes needed, {found} left")]
    Truncated { needed: usize, found: usize },
    #[error("{count} actors in a tick, past the limit of {}", TickActions::CAPACITY)]
    TooManyActors { count: u8 },
    #[error("actor ID {raw} can't be: IDs start at 1")]
    NoSuchActor { raw: u32 },
    #[error("actor {raw} is out of order or twice in one tick")]
    OutOfOrder { raw: u32 },
    #[error("actor {raw} is listed with neutral input, which isn't written")]
    NeutralListed { raw: u32 },
    #[error("actor {raw} presses unknown buttons {bits:#06x}")]
    UnknownButtons { raw: u32, bits: u16 },
    #[error("actor {raw}'s stick reads −128, below the range")]
    StickOutOfRange { raw: u32 },
}

impl TickActions {
    /// Most actors with input in one tick.
    pub const CAPACITY: usize = 8;

    /// The version of the byte layout above.
    pub const LAYOUT_VERSION: u32 = 1;

    /// Longest encoding of one tick, in bytes.
    pub const MAX_ENCODED_LEN: usize = 1 + Self::CAPACITY * ENTRY_LEN;

    /// No input at all.
    pub const NEUTRAL: Self =
        Self { entries: [Entry { actor: 0, actions: ActorActions::NEUTRAL }; Self::CAPACITY], len: 0 };

    /// Gives `actor` this input. Neutral input clears it, so a tick's value doesn't depend on how it
    /// was filled in.
    pub fn set(&mut self, actor: ActorId, actions: ActorActions) -> Result<(), TickActionsFull> {
        let raw = actor.raw();
        let len = usize::from(self.len);
        match self.entries[..len].binary_search_by_key(&raw, |entry| entry.actor) {
            Ok(index) if actions.is_neutral() => {
                self.entries[index..len].rotate_left(1);
                self.entries[len - 1] = Entry::default();
                self.len -= 1;
            }
            Ok(index) => self.entries[index].actions = actions,
            Err(_) if actions.is_neutral() => {}
            Err(index) => {
                if len == Self::CAPACITY {
                    return Err(TickActionsFull);
                }
                self.entries[index..=len].rotate_right(1);
                self.entries[index] = Entry { actor: raw, actions };
                self.len += 1;
            }
        }
        Ok(())
    }

    /// `actor`'s input: neutral if the tick doesn't mention it.
    #[must_use]
    pub fn get(&self, actor: ActorId) -> ActorActions {
        let listed = &self.entries[..usize::from(self.len)];
        match listed.binary_search_by_key(&actor.raw(), |entry| entry.actor) {
            Ok(index) => listed[index].actions,
            Err(_) => ActorActions::NEUTRAL,
        }
    }

    /// How many actors have input this tick.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Whether every actor is neutral this tick.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Each actor with input, in ID order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (ActorId, ActorActions)> + '_ {
        self.entries[..usize::from(self.len)].iter().map(|entry| {
            let id = ActorId::from_raw(entry.actor).expect("listed entries have real actors");
            (id, entry.actions)
        })
    }

    /// Appends this tick's bytes to `out`.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.push(self.len);
        for entry in &self.entries[..usize::from(self.len)] {
            out.extend_from_slice(&entry.actor.to_le_bytes());
            out.extend_from_slice(&entry.actions.buttons.bits().to_le_bytes());
            out.extend_from_slice(&entry.actions.stick.x().to_le_bytes());
            out.extend_from_slice(&entry.actions.stick.y().to_le_bytes());
        }
    }

    /// Reads one tick from the front of `bytes`, and returns it with the bytes after it.
    pub fn read_from(bytes: &[u8]) -> Result<(Self, &[u8]), ActionsError> {
        let (&count, rest) = bytes.split_first().ok_or(ActionsError::Truncated { needed: 1, found: 0 })?;
        if usize::from(count) > Self::CAPACITY {
            return Err(ActionsError::TooManyActors { count });
        }
        let needed = usize::from(count) * ENTRY_LEN;
        let (listed, rest) =
            rest.split_at_checked(needed).ok_or(ActionsError::Truncated { needed, found: rest.len() })?;

        let mut tick = Self { len: count, ..Self::NEUTRAL };
        let mut previous = 0;
        let (listed, _) = listed.as_chunks::<ENTRY_LEN>();
        for (slot, &[a, b, c, d, low, high, x, y]) in tick.entries.iter_mut().zip(listed) {
            let raw = u32::from_le_bytes([a, b, c, d]);
            let bits = u16::from_le_bytes([low, high]);
            let (x, y) = (i8::from_le_bytes([x]), i8::from_le_bytes([y]));
            if raw == 0 {
                return Err(ActionsError::NoSuchActor { raw });
            }
            if raw <= previous {
                return Err(ActionsError::OutOfOrder { raw });
            }
            previous = raw;
            let buttons = Buttons::from_bits(bits).ok_or(ActionsError::UnknownButtons { raw, bits })?;
            if x == i8::MIN || y == i8::MIN {
                return Err(ActionsError::StickOutOfRange { raw });
            }
            let actions = ActorActions { buttons, stick: Stick { x, y } };
            if actions.is_neutral() {
                return Err(ActionsError::NeutralListed { raw });
            }
            *slot = Entry { actor: raw, actions };
        }
        Ok((tick, rest))
    }
}

/// This tick's input, set by [`crate::Simulation::step`] and read once, in [`read_actions`].
#[derive(Resource, Debug, Default)]
pub(crate) struct CurrentActions(pub(crate) TickActions);

/// An actor's input this tick and last. Edges are read from these, in simulated state, so a press
/// is seen once however a replay splits its frames.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controls {
    pub current: ActorActions,
    pub previous: ActorActions,
}

impl Controls {
    /// The buttons that went down this tick.
    #[must_use]
    pub const fn pressed(&self) -> Buttons {
        self.current.buttons.minus(self.previous.buttons)
    }
}

/// [`sapphire_ecs::SimSet::Input`]: hands each live actor its input. An ID the tick mentions that
/// isn't alive any more is ignored. A party's input reaches whoever's on the field, whichever member's
/// on it: [`crate::Party::input_actor`].
pub(crate) fn read_actions(
    actions: Res<'_, CurrentActions>,
    table: Res<'_, crate::ActorTable>,
    party: Res<'_, crate::Party>,
    mut controls: Query<'_, '_, &mut Controls>,
) {
    for (id, entity) in table.iter() {
        let Ok(mut controls) = controls.get_mut(entity) else { continue };
        controls.previous = controls.current;
        controls.current = party.input_of(id).map_or(ActorActions::NEUTRAL, |input| actions.0.get(input));
    }
}

#[cfg(test)]
mod tests {
    use sapphire_moves::Button;

    use super::*;

    fn id(raw: u32) -> ActorId {
        ActorId::from_raw(raw).unwrap()
    }

    fn press(buttons: Buttons) -> ActorActions {
        ActorActions { buttons, stick: Stick::CENTER }
    }

    #[test]
    fn buttons_are_sets_with_fixed_bits() {
        let both = Buttons::LIGHT | Buttons::DODGE;
        assert_eq!(both.bits(), 0b101);
        assert!(both.contains(Buttons::DODGE));
        assert!(!both.contains(Buttons::HEAVY));
        assert!(both.contains(Buttons::NONE));
        assert_eq!(both.minus(Buttons::LIGHT), Buttons::DODGE);
        assert!(Buttons::NONE.is_empty());
        assert_eq!(Buttons::from_bits(0x1ff), Some(Buttons::ALL));
        assert_eq!(Buttons::from_bits(0x200), None);
        // These bits are in recorded replays.
        assert_eq!(
            [
                Buttons::LIGHT,
                Buttons::HEAVY,
                Buttons::DODGE,
                Buttons::SKILL,
                Buttons::ULTIMATE,
                Buttons::SWAP,
                Buttons::LOCK_ON,
                Buttons::JUMP,
            ]
            .map(Buttons::bits),
            [1, 2, 4, 8, 16, 32, 64, 128]
        );
        // And a document names them by the same bits.
        assert_eq!(
            [
                Button::Light,
                Button::Heavy,
                Button::Dodge,
                Button::Skill,
                Button::Ultimate,
                Button::Swap,
                Button::LockOn,
                Button::Jump,
            ]
            .map(Buttons::from),
            [
                Buttons::LIGHT,
                Buttons::HEAVY,
                Buttons::DODGE,
                Buttons::SKILL,
                Buttons::ULTIMATE,
                Buttons::SWAP,
                Buttons::LOCK_ON,
                Buttons::JUMP,
            ]
        );
    }

    #[test]
    fn sticks_quantize_symmetrically() {
        assert_eq!(Stick::from_axes(0.0, 0.0), Stick::CENTER);
        assert_eq!(Stick::from_axes(1.0, -1.0), Stick::new(127, -127));
        assert_eq!(Stick::from_axes(5.0, -5.0), Stick::new(127, -127));
        assert_eq!(Stick::from_axes(f32::NAN, 0.5), Stick::new(0, 64));
        assert_eq!(Stick::new(i8::MIN, 3), Stick::new(-127, 3));
        assert_eq!(Stick::new(127, -127).to_vec2(), Vec2::new(1.0, -1.0));
        // Whatever the axis, a step round trips.
        for step in -127..=127_i8 {
            let stick = Stick::new(step, step);
            let axes = stick.to_vec2();
            assert_eq!(Stick::from_axes(axes.x, axes.y), stick, "{step}");
        }
    }

    #[test]
    fn a_ticks_value_doesnt_depend_on_how_it_was_filled_in() {
        let mut a = TickActions::NEUTRAL;
        a.set(id(5), press(Buttons::LIGHT)).unwrap();
        a.set(id(2), press(Buttons::HEAVY)).unwrap();
        a.set(id(9), press(Buttons::DODGE)).unwrap();

        let mut b = TickActions::NEUTRAL;
        b.set(id(9), press(Buttons::SKILL)).unwrap();
        b.set(id(1), press(Buttons::SKILL)).unwrap();
        b.set(id(5), press(Buttons::LIGHT)).unwrap();
        b.set(id(1), ActorActions::NEUTRAL).unwrap();
        b.set(id(2), press(Buttons::HEAVY)).unwrap();
        b.set(id(9), press(Buttons::DODGE)).unwrap();
        b.set(id(7), ActorActions::NEUTRAL).unwrap();

        assert_eq!(a, b);
        assert_eq!(a.get(id(5)), press(Buttons::LIGHT));
        assert_eq!(a.get(id(3)), ActorActions::NEUTRAL);
        let order: Vec<u32> = a.iter().map(|(id, _)| id.raw()).collect();
        assert_eq!(order, [2, 5, 9]);
        assert_eq!(a.len(), 3);
        assert!(!a.is_empty());
    }

    #[test]
    fn a_full_tick_refuses_another_actor_but_not_an_update() {
        let mut tick = TickActions::NEUTRAL;
        for raw in 1..=TickActions::CAPACITY as u32 {
            tick.set(id(raw), press(Buttons::LIGHT)).unwrap();
        }
        assert_eq!(tick.set(id(100), press(Buttons::LIGHT)), Err(TickActionsFull));
        assert_eq!(tick.set(id(100), ActorActions::NEUTRAL), Ok(()));
        assert_eq!(tick.set(id(1), press(Buttons::HEAVY)), Ok(()));
        assert_eq!(tick.get(id(1)), press(Buttons::HEAVY));
        tick.set(id(1), ActorActions::NEUTRAL).unwrap();
        assert_eq!(tick.len(), TickActions::CAPACITY - 1);
        assert_eq!(tick.set(id(100), press(Buttons::LIGHT)), Ok(()));
    }

    /// These bytes are in recorded replays: a change here needs a new [`TickActions::LAYOUT_VERSION`].
    #[test]
    fn the_byte_layout_is_pinned() {
        assert_eq!(TickActions::LAYOUT_VERSION, 1);
        assert_eq!(TickActions::MAX_ENCODED_LEN, 65);

        let mut empty = Vec::new();
        TickActions::NEUTRAL.write_to(&mut empty);
        assert_eq!(empty, [0]);

        let mut tick = TickActions::NEUTRAL;
        let dodge_right = ActorActions { buttons: Buttons::DODGE, stick: Stick::new(127, -2) };
        tick.set(id(0x0102_0304), dodge_right).unwrap();
        tick.set(id(1), press(Buttons::JUMP | Buttons::LIGHT)).unwrap();
        let mut bytes = Vec::new();
        tick.write_to(&mut bytes);
        #[rustfmt::skip]
        assert_eq!(bytes, [
            2,
            1, 0, 0, 0,  0x81, 0x00,  0, 0,
            4, 3, 2, 1,  0x04, 0x00,  127, 0xfe,
        ]);
    }

    #[test]
    fn bytes_round_trip_and_leave_the_rest() {
        let mut tick = TickActions::NEUTRAL;
        tick.set(id(3), ActorActions { buttons: Buttons::ALL, stick: Stick::new(-127, 127) }).unwrap();
        tick.set(id(4), ActorActions { buttons: Buttons::NONE, stick: Stick::new(1, 0) }).unwrap();

        let mut bytes = Vec::new();
        tick.write_to(&mut bytes);
        TickActions::NEUTRAL.write_to(&mut bytes);
        let (first, rest) = TickActions::read_from(&bytes).unwrap();
        assert_eq!(first, tick);
        let (second, rest) = TickActions::read_from(rest).unwrap();
        assert_eq!(second, TickActions::NEUTRAL);
        assert!(rest.is_empty());
    }

    #[test]
    fn bad_bytes_are_refused() {
        let read = |bytes: &[u8]| TickActions::read_from(bytes).map(|(tick, _)| tick);
        assert_eq!(read(&[]), Err(ActionsError::Truncated { needed: 1, found: 0 }));
        assert_eq!(read(&[9]), Err(ActionsError::TooManyActors { count: 9 }));
        assert_eq!(read(&[1, 1, 0]), Err(ActionsError::Truncated { needed: 8, found: 2 }));
        assert_eq!(read(&[1, 0, 0, 0, 0, 1, 0, 0, 0]), Err(ActionsError::NoSuchActor { raw: 0 }));
        assert_eq!(
            read(&[2, 2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0]),
            Err(ActionsError::OutOfOrder { raw: 2 })
        );
        assert_eq!(
            read(&[2, 2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]),
            Err(ActionsError::OutOfOrder { raw: 1 })
        );
        assert_eq!(read(&[1, 7, 0, 0, 0, 0, 0, 0, 0]), Err(ActionsError::NeutralListed { raw: 7 }));
        assert_eq!(
            read(&[1, 7, 0, 0, 0, 0, 2, 0, 0]),
            Err(ActionsError::UnknownButtons { raw: 7, bits: 0x200 })
        );
        assert_eq!(read(&[1, 7, 0, 0, 0, 1, 0, 0x80, 0]), Err(ActionsError::StickOutOfRange { raw: 7 }));
    }

    #[test]
    fn controls_see_a_press_once() {
        let mut controls = Controls::default();
        assert!(controls.pressed().is_empty());
        controls.current = press(Buttons::LIGHT);
        assert_eq!(controls.pressed(), Buttons::LIGHT);
        controls.previous = controls.current;
        assert!(controls.pressed().is_empty(), "holding isn't pressing");
        controls.current = press(Buttons::LIGHT | Buttons::DODGE);
        assert_eq!(controls.pressed(), Buttons::DODGE);
    }
}
