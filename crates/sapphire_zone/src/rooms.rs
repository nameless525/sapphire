//! A zone's rooms (M6 U5): where its sound takes a reverb, as presentation alone.
//!
//! A room is a box or a sphere about a position, as a trigger's volume is, naming one of the mix's reverbs. The
//! room the listener stands in chooses the reverb heard, the highest `priority` winning where rooms overlap (a
//! shelter in a street), and crossing into it fades over the room's `fade`, in seconds; standing in no room plays
//! none. A voice reaches the reverb only from the listener's side of a wall: from inside the same room, or, for
//! a room marked `outdoor`, from anywhere outdoors, since the open street and the sky over it are one space.
//!
//! ```ron
//! rooms: [
//!     // The street, all of it outdoors, under the shelter's priority.
//!     (name: "street", position: (0.0, 6.0, 0.0), volume: Box(half_extents: (40.0, 6.0, 40.0)), reverb: "street", outdoor: true),
//!     (name: "shelter", position: (-24.0, 1.75, 29.85), volume: Box(half_extents: (3.0, 1.75, 2.15)), reverb: "shelter", priority: 2, fade: 0.4),
//! ],
//! ```
//!
//! **Presentation only:** rooms cook into [`ZonePresentation::rooms`](crate::ZonePresentation::rooms), which no
//! digest reads, so adding them moves no replay. What a reverb sounds like is the mix's (`sapphire_assets::mix`,
//! its `reverbs`); a room naming one the mix hasn't plays none, which the audio system counts.

use bevy_reflect::Reflect;
use sapphire_core::Name;
use serde::{Deserialize, Serialize};

use crate::cooked::{ArchivedZonePresentation, CookedVolume, check_ids};
use crate::document::{Volume, ZoneError, check_finite, check_name, check_names, check_position, is_false};

/// The most rooms a zone may hold.
pub const MAX_ROOMS: usize = 256;
/// The longest a room's fade may take, in seconds.
pub const MAX_FADE: f32 = 10.0;

/// One room of a zone (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct ZoneRoom {
    /// Unique among the zone's rooms: its identity.
    pub name: String,
    /// The volume's center, in metres.
    pub position: [f32; 3],
    pub volume: Volume,
    /// One of the mix's reverbs, by name.
    pub reverb: String,
    /// Which room is heard where rooms overlap: the highest, and of equals the first by ID.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: u8,
    /// Part of the outdoors: a voice anywhere outdoors reaches its reverb.
    #[serde(default, skip_serializing_if = "is_false")]
    pub outdoor: bool,
    /// Seconds its reverb takes to come in, and to go when the listener leaves for no room.
    #[serde(default = "default_fade")]
    pub fade: f32,
}

/// Half a second: about a step through a doorway.
fn default_fade() -> f32 {
    0.5
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_zero(value: &u8) -> bool {
    *value == 0
}

/// A room, cooked: see [`ZoneRoom`].
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedRoom {
    /// The name's hash ([`Name`]).
    pub id: u64,
    pub name: String,
    pub position: [f32; 3],
    pub volume: CookedVolume,
    pub reverb: String,
    pub priority: u8,
    pub outdoor: bool,
    pub fade: f32,
}

impl ArchivedCookedRoom {
    pub fn position(&self) -> [f32; 3] {
        self.position.each_ref().map(|value| value.to_native())
    }
}

/// Whether a document's rooms are some a cook can take, naming the field of what isn't.
pub(crate) fn check(rooms: &[ZoneRoom]) -> Result<(), ZoneError> {
    if rooms.len() > MAX_ROOMS {
        let problem = format!("{} rooms is more than the {MAX_ROOMS} a zone may hold", rooms.len());
        return Err(ZoneError::invalid("rooms", problem));
    }
    check_names("rooms", rooms.iter().map(|room| room.name.as_str()))?;
    for (index, room) in rooms.iter().enumerate() {
        let invalid = |name: &str| {
            let field = format!("rooms[{index}].{name}");
            move |problem: String| ZoneError::invalid(field, problem)
        };
        check_position(room.position).map_err(invalid("position"))?;
        room.volume.cooked().check().map_err(invalid("volume"))?;
        check_name(&room.reverb).map_err(invalid("reverb"))?;
        check_fade(room.fade).map_err(invalid("fade"))?;
    }
    Ok(())
}

fn check_fade(fade: f32) -> Result<(), String> {
    check_finite(fade)?;
    if !(0.0..=MAX_FADE).contains(&fade) {
        return Err(format!("{fade} s isn't from 0 to {MAX_FADE} s"));
    }
    Ok(())
}

/// A checked document's rooms, cooked, by ascending ID.
pub(crate) fn cook(rooms: &[ZoneRoom]) -> Vec<CookedRoom> {
    let mut cooked: Vec<CookedRoom> = rooms
        .iter()
        .map(|room| CookedRoom {
            id: Name::new(&room.name).as_u64(),
            name: room.name.clone(),
            position: room.position,
            volume: room.volume.cooked(),
            reverb: room.reverb.clone(),
            priority: room.priority,
            outdoor: room.outdoor,
            fade: room.fade,
        })
        .collect();
    cooked.sort_unstable_by_key(|room| room.id);
    cooked
}

/// Whether a cooked zone's rooms are ones a cook would write, whatever wrote them.
pub(crate) fn validate(presentation: &ArchivedZonePresentation) -> Result<(), ZoneError> {
    let rooms = &presentation.rooms;
    if rooms.len() > MAX_ROOMS {
        let problem = format!("{} rooms is more than the {MAX_ROOMS} a zone may hold", rooms.len());
        return Err(ZoneError::invalid("rooms", problem));
    }
    check_ids("rooms", rooms.iter().map(|room| (room.id.to_native(), room.name.as_str())))?;
    for room in rooms.iter() {
        let invalid = |problem: String| ZoneError::invalid(format!("room `{}`", room.name.as_str()), problem);
        check_position(room.position()).map_err(invalid)?;
        room.volume.to_native().check().map_err(invalid)?;
        check_name(room.reverb.as_str()).map_err(invalid)?;
        check_fade(room.fade.to_native()).map_err(invalid)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{ZoneDocument, ZoneError};

    fn document(rooms: &str) -> Result<ZoneDocument, ZoneError> {
        ZoneDocument::from_ron(&format!(
            r#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 32.0), rooms: [{rooms}]))"#
        ))
    }

    fn hall(more: &str) -> String {
        format!(
            r#"(name: "hall", position: (0.0, 2.0, 0.0), volume: Sphere(radius: 5.0), reverb: "shelter"{more})"#
        )
    }

    #[test]
    fn a_room_takes_its_defaults_and_writes_none_but_its_fade() {
        let document = document(&hall("")).expect("a good document");
        let room = &document.rooms[0];
        assert_eq!((room.priority, room.outdoor, room.fade), (0, false, 0.5));
        let text = sapphire_serialize::to_ron(&document).expect("writes");
        assert!(
            !text.contains("priority") && !text.contains("outdoor") && text.contains("fade: 0.5"),
            "{text}"
        );
        assert_eq!(super::cook(&document.rooms)[0].fade, 0.5);
    }

    #[test]
    fn a_bad_room_is_refused_by_its_field() {
        let cases = [
            (hall(", fade: 11.0"), "rooms[0].fade"),
            (hall(", fade: -0.1"), "rooms[0].fade"),
            (hall("").replace(r#""shelter""#, r#""""#), "rooms[0].reverb"),
            (hall("").replace("radius: 5.0", "radius: 0.0"), "rooms[0].volume"),
            (hall("").replace("(0.0, 2.0, 0.0)", "(1e9, 2.0, 0.0)"), "rooms[0].position"),
            (format!("{}, {}", hall(""), hall("")), "rooms[1].name"),
        ];
        for (rooms, field) in cases {
            let message = document(&rooms).expect_err(field).to_string();
            assert!(message.contains(field), "`{message}` must name `{field}`");
        }
        let message = document(&hall(", echo: 1.0")).expect_err("unknown").to_string();
        assert!(message.contains("echo"), "{message}");
    }
}
