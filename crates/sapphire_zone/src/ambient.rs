//! A zone's ambient characters (M5 C10): the people who stand about its streets and walk them, as its
//! presentation alone. Nothing about them reaches the simulation, a replay or a zone's digest.
//!
//! ```ron
//! ambient: Some((
//!     // Who appears, and the gestures they make, are drawn from this seed and the zone's data alone.
//!     seed: 7,
//!     // The one graph every one of them plays: stand, shift, a gesture, walk, and stop.
//!     graph: "5b0e7c1a-3f7d-4e8b-9a52-6c1d2e3f4a5b",
//!     // Who may appear: each a skinned mesh, and its joints by name where they aren't these.
//!     cast: [
//!         (name: "porter", mesh: "9d2c1e4b-7a3f-4c5d-8e6f-0a1b2c3d4e5f"),
//!         (name: "weaver", mesh: "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d", head: "head", feet: ("foot.l", "foot.r")),
//!     ],
//!     // Someone standing: where, facing which way, who (a pick from the cast without `who`), and the
//!     // graph's moves they make now and then, one every 8 to 16 seconds.
//!     posts: [
//!         (name: "well", position: (3.0, 0.0, 2.0), yaw_degrees: 200.0, who: Some("porter"),
//!          gestures: ["look_around"], every: (8.0, 16.0)),
//!     ],
//!     // People walking a loop, or there and back along an open line: how many, how fast, and who.
//!     walks: [
//!         (name: "market_lap", points: [(-8.0, 0.0, -8.0), (8.0, 0.0, -8.0), (8.0, 0.0, 8.0)], closed: true,
//!          pace: 1.3, count: 3),
//!     ],
//! )),
//! ```
//!
//! **Its own asset.** The cook writes the section as an asset of the zone's own, a sub-asset beside its
//! cells (`town.sambient`, [`AMBIENT_EXTENSION`]), never into the cooked zone, so a zone without the section
//! cooks to the bytes it did before there was one, and one with it differs from them only by that asset in
//! its pack. The stage finds it by the zone's ID (`zone.sub_asset(`[`AMBIENT_PART`]`)`) when the zone opens:
//! a few kilobytes, read whole, so a walker crossing cells and a post in a cell not yet streamed are the
//! stage's to place, by where they stand against its streaming.
//!
//! **What isn't here:** a collider. A post blocks nothing; if one should, the zone's `colliders` document is
//! where its collider goes, which is a change to the simulation, and moves the zone's digest, by choice.

use std::collections::BTreeSet;

use bevy_reflect::{Reflect, TypePath};
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::AlignedVec;
use serde::{Deserialize, Serialize};

use crate::document::{ZoneError, check_name, check_position};

/// Conventional file extension for a zone's cooked ambient characters.
pub const AMBIENT_EXTENSION: &str = "sambient";

/// What a zone's ambient characters' sub-asset is called among a zone's: its ID is the zone's
/// `sub_asset(AMBIENT_PART)`. Never changed.
pub const AMBIENT_PART: &str = "ambient";

/// The most kinds of person a zone's cast may hold.
pub const MAX_CAST: usize = 16;
/// The most posts a zone may hold.
pub const MAX_POSTS: usize = 512;
/// The most walks a zone may hold.
pub const MAX_WALKS: usize = 512;
/// The most people one walk may hold.
pub const MAX_WALKERS: u16 = 64;
/// The most people a zone may hold in all, standing and walking.
pub const MAX_PEOPLE: usize = 1024;
/// The most gestures a post may make.
pub const MAX_GESTURES: usize = 8;
/// The most points a walk may hold.
pub const MAX_WALK_POINTS: usize = 256;
/// The slowest and fastest a walk may go, in metres a second.
pub const PACE: (f32, f32) = (0.2, 3.0);
/// The shortest leg of a walk, in metres: what a turn needs to read as one.
pub const MIN_LEG: f32 = 0.25;
/// The soonest and latest a post's gestures may come, in seconds apart.
pub const EVERY: (f32, f32) = (1.0, 600.0);
/// A cooked post or walk with no `who`: a pick from the cast, by the zone's seed.
pub const ANYONE: u16 = u16::MAX;

/// A zone's ambient characters, as its document writes them (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct ZoneAmbient {
    /// What the picks of who stands where, and of which gesture comes when, are drawn from.
    #[serde(default)]
    pub seed: u32,
    /// The animation graph every one of them plays, by its sidecar's ID.
    pub graph: AssetId,
    /// Who may appear.
    pub cast: Vec<CastMember>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub posts: Vec<Post>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub walks: Vec<Walk>,
}

/// One kind of person: a skinned mesh, and its joints a figure follows, by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct CastMember {
    /// What a post or a walk names it by. Unique within the cast.
    pub name: String,
    /// Its mesh, by its sidecar's ID: skinned, its skin naming the skeleton the graph's clips play on.
    pub mesh: AssetId,
    /// The joint its head turns about: what its light rig and face follow.
    #[serde(default = "head", skip_serializing_if = "is_head")]
    pub head: String,
    /// Its feet's joints, left then right: where it stands.
    #[serde(default = "feet", skip_serializing_if = "are_feet")]
    pub feet: [String; 2],
}

/// Someone standing in one place.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Post {
    /// Unique among the zone's posts.
    pub name: String,
    /// Where their feet are, in the zone's space.
    pub position: [f32; 3],
    /// Which way they face: degrees about +Y from +Z, counter-clockwise seen from above.
    #[serde(default)]
    pub yaw_degrees: f32,
    /// Who stands here, by a cast member's name: a pick from the cast without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    /// The graph's moves they make now and then, by name: none, and they only stand and shift.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gestures: Vec<String>,
    /// How far apart their gestures come, least and most, in seconds.
    #[serde(default = "every", skip_serializing_if = "is_every")]
    pub every: [f32; 2],
}

/// People walking a line, round and round.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Walk {
    /// Unique among the zone's walks.
    pub name: String,
    /// The line, in the zone's space. An open one is walked there and back, turning at each end.
    pub points: Vec<[f32; 3]>,
    /// Whether its last point goes on to its first.
    #[serde(default, skip_serializing_if = "is_false")]
    pub closed: bool,
    /// How fast they walk, in metres a second.
    #[serde(default = "pace")]
    pub pace: f32,
    /// Who walks it, by a cast member's name: a pick from the cast for each walker without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    /// How many walk it, spread evenly round it.
    #[serde(default = "one")]
    pub count: u16,
}

fn head() -> String {
    "head".to_owned()
}

fn is_head(name: &String) -> bool {
    name == "head"
}

fn feet() -> [String; 2] {
    ["foot.l".to_owned(), "foot.r".to_owned()]
}

fn are_feet(names: &[String; 2]) -> bool {
    *names == feet()
}

fn every() -> [f32; 2] {
    [8.0, 16.0]
}

fn is_every(every: &[f32; 2]) -> bool {
    *every == self::every()
}

fn pace() -> f32 {
    1.2
}

fn one() -> u16 {
    1
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl ZoneAmbient {
    /// Checks everything that needs nothing outside the document, naming the field under `ambient`.
    pub fn check(&self) -> Result<(), ZoneError> {
        let invalid =
            |field: String| move |problem: String| ZoneError::invalid(format!("ambient.{field}"), problem);
        let people = self.people();
        if people > MAX_PEOPLE {
            let problem = format!("{people} people is more than the {MAX_PEOPLE} a zone may hold");
            return Err(ZoneError::invalid("ambient", problem));
        }
        for (list, count, most) in [
            ("cast", self.cast.len(), MAX_CAST),
            ("posts", self.posts.len(), MAX_POSTS),
            ("walks", self.walks.len(), MAX_WALKS),
        ] {
            if count > most {
                let problem = format!("{count} is more than the {most} it may hold");
                return Err(ZoneError::invalid(format!("ambient.{list}"), problem));
            }
        }
        if self.cast.is_empty() && people > 0 {
            return Err(ZoneError::invalid(
                "ambient.cast",
                "nobody's in it, and its posts and walks need someone",
            ));
        }
        check_names("cast", self.cast.iter().map(|member| member.name.as_str()))?;
        for (index, member) in self.cast.iter().enumerate() {
            let field = |name: &str| format!("cast[{index}].{name}");
            check_name(&member.head).map_err(invalid(field("head")))?;
            for (side, foot) in member.feet.iter().enumerate() {
                check_name(foot).map_err(invalid(format!("cast[{index}].feet[{side}]")))?;
            }
        }
        check_names("posts", self.posts.iter().map(|post| post.name.as_str()))?;
        for (index, post) in self.posts.iter().enumerate() {
            let field = |name: &str| format!("posts[{index}].{name}");
            check_position(post.position).map_err(invalid(field("position")))?;
            check_finite(post.yaw_degrees).map_err(invalid(field("yaw_degrees")))?;
            self.check_who(post.who.as_deref()).map_err(invalid(field("who")))?;
            if post.gestures.len() > MAX_GESTURES {
                let problem =
                    format!("{} is more than the {MAX_GESTURES} a post may make", post.gestures.len());
                return Err(ZoneError::invalid(format!("ambient.{}", field("gestures")), problem));
            }
            for (at, gesture) in post.gestures.iter().enumerate() {
                check_name(gesture).map_err(invalid(format!("posts[{index}].gestures[{at}]")))?;
            }
            check_every(post.every).map_err(invalid(field("every")))?;
        }
        check_names("walks", self.walks.iter().map(|walk| walk.name.as_str()))?;
        for (index, walk) in self.walks.iter().enumerate() {
            let field = |name: &str| format!("walks[{index}].{name}");
            check_walk_points(&walk.points, walk.closed).map_err(invalid(field("points")))?;
            check_pace(walk.pace).map_err(invalid(field("pace")))?;
            self.check_who(walk.who.as_deref()).map_err(invalid(field("who")))?;
            check_count(walk.count).map_err(invalid(field("count")))?;
        }
        Ok(())
    }

    /// How many people it places in all.
    #[must_use]
    pub fn people(&self) -> usize {
        self.posts.len() + self.walks.iter().map(|walk| usize::from(walk.count)).sum::<usize>()
    }

    fn check_who(&self, who: Option<&str>) -> Result<(), String> {
        match who {
            Some(who) if !self.cast.iter().any(|member| member.name == who) => {
                Err(format!("`{who}` isn't one of the cast"))
            }
            _ => Ok(()),
        }
    }

    fn cast_index(&self, who: Option<&str>) -> u16 {
        who.and_then(|who| self.cast.iter().position(|member| member.name == who))
            .map_or(ANYONE, |index| index as u16)
    }

    /// The cooked section, from a document that has passed its check.
    pub fn cook(&self) -> Result<Ambient, ZoneError> {
        self.check()?;
        let ambient = Ambient {
            seed: self.seed,
            graph: self.graph,
            cast: self
                .cast
                .iter()
                .map(|member| CookedCastMember {
                    name: member.name.clone(),
                    mesh: member.mesh,
                    head: member.head.clone(),
                    feet: member.feet.clone(),
                })
                .collect(),
            posts: self
                .posts
                .iter()
                .map(|post| CookedPost {
                    name: post.name.clone(),
                    position: post.position,
                    yaw: post.yaw_degrees.to_radians(),
                    cast: self.cast_index(post.who.as_deref()),
                    gestures: post.gestures.clone(),
                    every: post.every,
                })
                .collect(),
            walks: self
                .walks
                .iter()
                .map(|walk| CookedWalk {
                    name: walk.name.clone(),
                    points: walk.points.clone(),
                    closed: walk.closed,
                    pace: walk.pace,
                    cast: self.cast_index(walk.who.as_deref()),
                    count: walk.count,
                })
                .collect(),
        };
        Ok(ambient)
    }

    /// The meshes and the graph it names, which a cook holds to their kinds.
    /// Each is named by its field under `ambient`: `graph`, or `cast[2].mesh`.
    pub fn named(&self) -> impl Iterator<Item = (String, AssetId)> + '_ {
        let cast = self.cast.iter().enumerate();
        std::iter::once(("graph".to_owned(), self.graph))
            .chain(cast.map(|(index, member)| (format!("cast[{index}].mesh"), member.mesh)))
    }
}

/// A zone's cooked ambient characters (`.sambient`): an rkyv archive read in place, whole, as a zone opens.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::zone"]
pub struct Ambient {
    pub seed: u32,
    pub graph: AssetId,
    pub cast: Vec<CookedCastMember>,
    pub posts: Vec<CookedPost>,
    pub walks: Vec<CookedWalk>,
}

impl Versioned for Ambient {
    /// 1: M5 C10.
    const VERSION: u32 = 1;
}

/// A cast member, as cooked.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedCastMember {
    pub name: String,
    pub mesh: AssetId,
    pub head: String,
    pub feet: [String; 2],
}

/// A post, as cooked: its turn in radians, and who by their place in the cast, or [`ANYONE`].
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedPost {
    pub name: String,
    pub position: [f32; 3],
    pub yaw: f32,
    pub cast: u16,
    pub gestures: Vec<String>,
    pub every: [f32; 2],
}

/// A walk, as cooked: who by their place in the cast, or [`ANYONE`].
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedWalk {
    pub name: String,
    pub points: Vec<[f32; 3]>,
    pub closed: bool,
    pub pace: f32,
    pub cast: u16,
    pub count: u16,
}

impl Ambient {
    /// The section as bytes, for a pack.
    pub fn to_cooked(&self) -> Result<AlignedVec, ZoneError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value the stage reads.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedAmbient, ZoneError> {
        let ambient = sapphire_serialize::access::<Self>(bytes)?;
        ambient.validate()?;
        Ok(ambient)
    }
}

impl ArchivedAmbient {
    /// What it names: its graph and its cast's meshes, which ship in its zone's pack.
    pub fn named(&self) -> Vec<AssetId> {
        std::iter::once(self.graph).chain(self.cast.iter().map(|member| member.mesh)).collect()
    }

    /// Checks everything the stage reads, whatever wrote these bytes.
    pub fn validate(&self) -> Result<(), ZoneError> {
        let invalid = |field: String| move |problem: String| ZoneError::invalid(field, problem);
        if self.cast.len() > MAX_CAST || self.posts.len() > MAX_POSTS || self.walks.len() > MAX_WALKS {
            return Err(ZoneError::invalid("ambient", "more cast, posts or walks than a zone may hold"));
        }
        let people = self.posts.len()
            + self.walks.iter().map(|walk| usize::from(walk.count.to_native())).sum::<usize>();
        if people > MAX_PEOPLE {
            return Err(ZoneError::invalid("ambient", format!("{people} people is more than {MAX_PEOPLE}")));
        }
        for (index, member) in self.cast.iter().enumerate() {
            check_name(member.name.as_str()).map_err(invalid(format!("cast[{index}].name")))?;
            check_name(member.head.as_str()).map_err(invalid(format!("cast[{index}].head")))?;
            for (side, foot) in member.feet.iter().enumerate() {
                check_name(foot.as_str()).map_err(invalid(format!("cast[{index}].feet[{side}]")))?;
            }
        }
        let cast = self.cast.len();
        let check_cast = |index: u16| -> Result<(), String> {
            if (index == ANYONE && cast > 0) || usize::from(index) < cast {
                Ok(())
            } else {
                Err(format!("{index} is no one of a cast of {cast}"))
            }
        };
        for (index, post) in self.posts.iter().enumerate() {
            let field = |name: &str| format!("posts[{index}].{name}");
            check_name(post.name.as_str()).map_err(invalid(field("name")))?;
            check_position(post.position.map(|value| value.to_native()))
                .map_err(invalid(field("position")))?;
            check_finite(post.yaw.to_native()).map_err(invalid(field("yaw")))?;
            check_cast(post.cast.to_native()).map_err(invalid(field("cast")))?;
            if post.gestures.len() > MAX_GESTURES {
                return Err(ZoneError::invalid(field("gestures"), "too many"));
            }
            for (at, gesture) in post.gestures.iter().enumerate() {
                check_name(gesture.as_str()).map_err(invalid(format!("posts[{index}].gestures[{at}]")))?;
            }
            check_every(post.every.map(|value| value.to_native())).map_err(invalid(field("every")))?;
        }
        for (index, walk) in self.walks.iter().enumerate() {
            let field = |name: &str| format!("walks[{index}].{name}");
            check_name(walk.name.as_str()).map_err(invalid(field("name")))?;
            let points: Vec<[f32; 3]> =
                walk.points.iter().map(|point| point.map(|value| value.to_native())).collect();
            check_walk_points(&points, walk.closed).map_err(invalid(field("points")))?;
            check_pace(walk.pace.to_native()).map_err(invalid(field("pace")))?;
            check_cast(walk.cast.to_native()).map_err(invalid(field("cast")))?;
            check_count(walk.count.to_native()).map_err(invalid(field("count")))?;
        }
        Ok(())
    }
}

fn check_finite(value: f32) -> Result<(), String> {
    if value.is_finite() { Ok(()) } else { Err(format!("{value} isn't finite")) }
}

fn check_pace(pace: f32) -> Result<(), String> {
    let (least, most) = PACE;
    if pace.is_finite() && (least..=most).contains(&pace) {
        Ok(())
    } else {
        Err(format!("{pace} m/s isn't from {least} to {most}: a person walks"))
    }
}

fn check_count(count: u16) -> Result<(), String> {
    if (1..=MAX_WALKERS).contains(&count) {
        Ok(())
    } else {
        Err(format!("{count} isn't from 1 to {MAX_WALKERS}"))
    }
}

fn check_every([least, most]: [f32; 2]) -> Result<(), String> {
    let (soonest, latest) = EVERY;
    if least.is_finite() && most.is_finite() && soonest <= least && least <= most && most <= latest {
        Ok(())
    } else {
        Err(format!("({least}, {most}) s must rise from at least {soonest} to at most {latest}"))
    }
}

/// Whether a walk's points make one, or what's wrong with them: each leg long enough to turn on.
fn check_walk_points(points: &[[f32; 3]], closed: bool) -> Result<(), String> {
    let least = if closed { 3 } else { 2 };
    if !(least..=MAX_WALK_POINTS).contains(&points.len()) {
        let shape = if closed { "a loop" } else { "a line" };
        return Err(format!(
            "{} points isn't from {least} to {MAX_WALK_POINTS}: {shape} needs {least}",
            points.len()
        ));
    }
    for point in points {
        check_position(*point)?;
    }
    let closing = closed.then(|| [points[points.len() - 1], points[0]]);
    for (leg, [from, to]) in points.windows(2).map(|pair| [pair[0], pair[1]]).chain(closing).enumerate() {
        let length = (0..3).map(|axis| (to[axis] - from[axis]) * (to[axis] - from[axis])).sum::<f32>().sqrt();
        if length < MIN_LEG {
            return Err(format!("leg {leg} is {length} m long, shorter than {MIN_LEG} m"));
        }
    }
    Ok(())
}

fn check_names<'a>(list: &str, names: impl Iterator<Item = &'a str>) -> Result<(), ZoneError> {
    let mut seen = BTreeSet::new();
    for (index, name) in names.enumerate() {
        let field = || format!("ambient.{list}[{index}].name");
        check_name(name).map_err(|problem| ZoneError::invalid(field(), problem))?;
        if !seen.insert(name) {
            let problem = format!("`{name}` is the name of an earlier entry");
            return Err(ZoneError::invalid(field(), problem));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const MESH: &str = "9d2c1e4b-7a3f-4c5d-8e6f-0a1b2c3d4e5f";
    const GRAPH: &str = "5b0e7c1a-3f7d-4e8b-9a52-6c1d2e3f4a5b";

    /// The module docs' example.
    pub(crate) fn example() -> ZoneAmbient {
        let text = format!(
            r#"(
                seed: 7,
                graph: "{GRAPH}",
                cast: [
                    (name: "porter", mesh: "{MESH}"),
                    (name: "weaver", mesh: "{MESH}", head: "head", feet: ("foot.l", "foot.r")),
                ],
                posts: [
                    (name: "well", position: (3.0, 0.0, 2.0), yaw_degrees: 200.0, who: Some("porter"),
                     gestures: ["look_around"], every: (8.0, 16.0)),
                ],
                walks: [
                    (name: "market_lap", points: [(-8.0, 0.0, -8.0), (8.0, 0.0, -8.0), (8.0, 0.0, 8.0)], closed: true,
                     pace: 1.3, count: 3),
                ],
            )"#
        );
        ron::from_str(&text).expect("the example reads")
    }

    #[test]
    fn the_example_reads_round_trips_and_cooks() {
        let ambient = example();
        ambient.check().expect("good");
        assert_eq!(ambient.people(), 4);
        let text = ron::to_string(&ambient).expect("write");
        assert!(!text.contains("foot.l") && !text.contains("every"), "defaults are left out: {text}");
        assert_eq!(ron::from_str::<ZoneAmbient>(&text).expect("read"), ambient);

        let cooked = ambient.cook().expect("cooks");
        assert_eq!(cooked.posts[0].cast, 0);
        assert_eq!(cooked.walks[0].cast, ANYONE);
        assert!((cooked.posts[0].yaw - 200f32.to_radians()).abs() < 1e-6);
        let bytes = cooked.to_cooked().expect("bytes");
        let read = Ambient::read(&bytes).expect("reads");
        assert_eq!(read.walks[0].count, 3);
        assert_eq!(read.cast[1].feet[1].as_str(), "foot.r");
    }

    #[test]
    fn an_unknown_field_is_an_error_that_names_it() {
        let text = format!(r#"(graph: "{GRAPH}", cast: [], posst: [])"#);
        let message = ron::from_str::<ZoneAmbient>(&text).unwrap_err().to_string();
        assert!(message.contains("posst"), "{message}");
        let text = format!(r#"(graph: "{GRAPH}", cast: [(name: "a", mesh: "{MESH}", hed: "head")])"#);
        let message = ron::from_str::<ZoneAmbient>(&text).unwrap_err().to_string();
        assert!(message.contains("hed"), "{message}");
    }

    #[test]
    fn every_check_names_the_field() {
        type Tamper = fn(&mut ZoneAmbient);
        let cases: [(&str, Tamper); 17] = [
            ("ambient.cast", |a| a.cast.clear()),
            ("ambient.cast[1].name", |a| a.cast[1].name = "porter".to_owned()),
            ("ambient.cast[0].head", |a| a.cast[0].head.clear()),
            ("ambient.cast[0].feet[1]", |a| a.cast[0].feet[1].clear()),
            ("ambient.posts[0].position", |a| a.posts[0].position[0] = f32::NAN),
            ("ambient.posts[0].yaw_degrees", |a| a.posts[0].yaw_degrees = f32::INFINITY),
            ("ambient.posts[0].who", |a| a.posts[0].who = Some("nobody".to_owned())),
            ("ambient.posts[0].gestures[0]", |a| a.posts[0].gestures[0].clear()),
            ("ambient.posts[0].every", |a| a.posts[0].every = [20.0, 10.0]),
            ("ambient.posts[0].every", |a| a.posts[0].every = [0.1, 10.0]),
            ("ambient.posts[1].name", |a| a.posts.push(a.posts[0].clone())),
            ("ambient.walks[0].points", |a| a.walks[0].points.truncate(2)),
            ("ambient.walks[0].points", |a| a.walks[0].points[1] = a.walks[0].points[0]),
            ("ambient.walks[0].pace", |a| a.walks[0].pace = 9.0),
            ("ambient.walks[0].who", |a| a.walks[0].who = Some("nobody".to_owned())),
            ("ambient.walks[0].count", |a| a.walks[0].count = 0),
            ("ambient", |a| a.walks[0].count = MAX_WALKERS),
        ];
        for (field, tamper) in cases {
            let mut ambient = example();
            if field == "ambient" {
                // Past a zone's most, in walkers alone.
                for index in 0..16 {
                    a_walk(&mut ambient, index);
                }
            }
            tamper(&mut ambient);
            let message = ambient.check().expect_err(field).to_string();
            assert!(message.contains(&format!("`{field}`")), "`{message}` must name `{field}`");
        }
        let mut open = example();
        open.walks[0].closed = false;
        open.walks[0].points.truncate(2);
        open.check().expect("an open line of two points is walked there and back");
    }

    fn a_walk(ambient: &mut ZoneAmbient, index: usize) {
        let walk = Walk { name: format!("lap_{index}"), count: MAX_WALKERS, ..ambient.walks[0].clone() };
        ambient.walks.push(walk);
    }

    #[test]
    fn cooked_bytes_that_break_a_rule_are_refused() {
        let mut cooked = example().cook().expect("cooks");
        cooked.posts[0].cast = 2;
        let bytes = cooked.to_cooked().expect("bytes");
        let message = Ambient::read(&bytes).expect_err("no third").to_string();
        assert!(message.contains("posts[0].cast"), "{message}");

        let mut cooked = example().cook().expect("cooks");
        cooked.walks[0].pace = f32::NAN;
        let bytes = cooked.to_cooked().expect("bytes");
        assert!(Ambient::read(&bytes).is_err());
    }
}
