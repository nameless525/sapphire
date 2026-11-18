//! Rooms (M6 U5): where a reverb plays, as plain data a zone's presentation gives.
//!
//! The listener's room chooses the reverb the mix plays ([`Room::reverb`], one of the mix's by name), and a voice
//! reaches it only from the listener's own space ([`Rooms::space_at`]): the same room, or for an outdoor room
//! anywhere outdoors, which is also wherever no room is. Of rooms that overlap, the higher priority holds a point,
//! and of equal ones the first listed, as Unreal's audio volumes do with their priority.

use sapphire_core::Name;
use sapphire_math::Vec3;

/// A room's shape, about its position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RoomShape {
    /// Along the zone's axes.
    Box {
        half_extents: Vec3,
    },
    Sphere {
        radius: f32,
    },
}

/// One room.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    /// Its centre, in its zone's space.
    pub position: Vec3,
    pub shape: RoomShape,
    /// Of rooms that overlap, the highest holds a point.
    pub priority: u8,
    /// The mix's reverb it plays, by name: one the mix hasn't plays none, counted.
    pub reverb: Name,
    /// Open to the sky: any voice outdoors reaches its reverb, and its voices reach any outdoor room's.
    pub outdoor: bool,
    /// Seconds its reverb takes to come in as the listener enters, or to go as the listener leaves for no room.
    pub fade: f32,
}

impl Room {
    fn holds(&self, point: Vec3) -> bool {
        let offset = point - self.position;
        match self.shape {
            RoomShape::Box { half_extents } => offset.abs().cmple(half_extents).all(),
            RoomShape::Sphere { radius } => offset.length_squared() <= radius * radius,
        }
    }
}

/// A frame's rooms: a zone's, and where its origin lies in the world the listener and the emitters stand in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rooms<'a> {
    pub rooms: &'a [Room],
    pub origin: Vec3,
}

impl Rooms<'_> {
    /// No rooms: everywhere is outdoors, and no reverb plays.
    pub const NONE: Rooms<'static> = Rooms { rooms: &[], origin: Vec3::ZERO };

    /// The room holding `point`, in the world, by its place in [`Self::rooms`].
    #[must_use]
    pub fn at(&self, point: Vec3) -> Option<usize> {
        let local = point - self.origin;
        let mut best: Option<usize> = None;
        for (index, room) in self.rooms.iter().enumerate() {
            if room.holds(local) && best.is_none_or(|best| room.priority > self.rooms[best].priority) {
                best = Some(index);
            }
        }
        best
    }

    /// The space `point` is in: its room, or outdoors in an outdoor room or none.
    #[must_use]
    pub fn space_at(&self, point: Vec3) -> Space {
        match self.at(point) {
            Some(room) if !self.rooms[room].outdoor => Space::Room(room),
            _ => Space::Outdoors,
        }
    }
}

/// Where a sound is, as rooms share a reverb: inside a room, or outdoors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Room(usize),
    Outdoors,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(position: Vec3, shape: RoomShape, priority: u8, outdoor: bool) -> Room {
        Room { position, shape, priority, reverb: Name::new("r"), outdoor, fade: 0.5 }
    }

    #[test]
    fn the_highest_priority_holds_a_point_and_the_first_of_equals() {
        let rooms = [
            room(Vec3::ZERO, RoomShape::Box { half_extents: Vec3::splat(10.0) }, 1, true),
            room(
                Vec3::new(2.0, 0.0, 0.0),
                RoomShape::Box { half_extents: Vec3::new(1.0, 2.0, 1.0) },
                2,
                false,
            ),
            room(Vec3::new(2.0, 0.0, 0.0), RoomShape::Sphere { radius: 1.5 }, 2, false),
            room(Vec3::new(-5.0, 0.0, 0.0), RoomShape::Sphere { radius: 1.0 }, 0, false),
        ];
        let at = |origin: Vec3, point: Vec3| Rooms { rooms: &rooms, origin }.at(point);
        assert_eq!(
            at(Vec3::ZERO, Vec3::new(2.0, 1.9, 0.0)),
            Some(1),
            "the higher, and of two equal the first"
        );
        assert_eq!(at(Vec3::ZERO, Vec3::new(3.4, 0.0, 0.0)), Some(2), "the sphere reaches past the box");
        assert_eq!(at(Vec3::ZERO, Vec3::new(-5.0, 0.0, 0.0)), Some(0), "a lower one inside gives way");
        assert_eq!(at(Vec3::ZERO, Vec3::new(0.0, 11.0, 0.0)), None);
        assert_eq!(
            at(Vec3::new(0.0, -20.0, 0.0), Vec3::new(2.0, -20.0, 0.0)),
            Some(1),
            "about the zone's origin"
        );
    }

    #[test]
    fn outdoor_rooms_and_none_are_one_space() {
        let rooms = [
            room(Vec3::ZERO, RoomShape::Sphere { radius: 2.0 }, 0, false),
            room(Vec3::new(10.0, 0.0, 0.0), RoomShape::Sphere { radius: 2.0 }, 0, true),
        ];
        let rooms = Rooms { rooms: &rooms, origin: Vec3::ZERO };
        assert_eq!(rooms.space_at(Vec3::ZERO), Space::Room(0));
        assert_eq!(rooms.space_at(Vec3::new(10.0, 0.0, 0.0)), Space::Outdoors);
        assert_eq!(rooms.space_at(Vec3::new(30.0, 0.0, 0.0)), Space::Outdoors);
        assert_eq!(Rooms::NONE.space_at(Vec3::ZERO), Space::Outdoors);
    }
}
