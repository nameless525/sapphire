//! A zone's atmosphere volumes (M6 V1): places with a look of their own, a cave's, an inn's or a forest's air,
//! as presentation alone.
//!
//! A volume is a [`ZoneShape`] about its position naming a look: a partial day, a `TodTimeline` document keying
//! only the groups the place changes (its fog, ambient, shadow tint, exposure), by its path from this document's
//! directory, as `day` is. Never `grade`, which the cook refuses: post blends two grades, a day holds two between
//! its keys, and a volume's third would jump in and out at its edge. Volumes of one priority apply in the
//! document's order. It's a biome preset many zones share (`content/looks/biomes/`: a cave, a
//! warm interior, a forest's haze) or a look of the zone's own. Inside its shape the look lies over the day and
//! the weather's state at `strength`, and fades out over `blend` metres around it, by the camera's distance from
//! the shape; volumes apply from the lowest `priority` up, and of equals by ID, each over what the ones before it
//! left; a cutscene and the tool lie over them all (m6-plan §3, "Atmosphere and exposure").
//!
//! ```ron
//! volumes: [
//!     (name: "grotto", position: (12.0, 2.0, -30.0), shape: Ellipsoid(radii: (14.0, 6.0, 10.0), yaw: 20.0), look: "../../looks/biomes/cave_day.ron", blend: 6.0),
//!     (name: "inn", position: (-8.0, 2.5, 4.0), shape: Box(half_extents: (5.0, 2.5, 4.0)), look: "inn_day.ron", blend: 1.5, strength: 0.9, priority: 1),
//! ],
//! ```
//!
//! **Presentation only:** volumes cook into [`ZonePresentation::atmosphere`](crate::ZonePresentation), which no
//! digest reads, so adding them moves no replay. The cook reads each look the volumes name once, checks it
//! reads as a day, and holds its text in the zone; the stage parses each once, off the frame's thread, and
//! places the volumes in time of day when the zone's look comes on.

use bevy_reflect::Reflect;
use sapphire_core::Name;
use serde::{Deserialize, Serialize};

use crate::cooked::{ArchivedZonePresentation, check_ids};
use crate::document::{ZoneError, check_distinct, check_names, check_path, check_position};
use crate::shape::ZoneShape;

/// The most atmosphere volumes a zone may hold (spec §3.4).
pub const MAX_VOLUMES: usize = 64;
/// The widest a volume's look may fade out around it, in metres.
pub const MAX_BLEND: f32 = 256.0;

/// One atmosphere volume of a zone (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct AtmosphereVolume {
    /// Unique among the zone's volumes: its identity.
    pub name: String,
    /// The shape's centre, in metres.
    pub position: [f32; 3],
    pub shape: ZoneShape,
    /// A `TodTimeline` document, from this document's directory: a biome preset or a look of the zone's own.
    pub look: String,
    /// Metres outside the shape over which its look fades out, 0 for a hard edge, up to [`MAX_BLEND`].
    pub blend: f32,
    /// Its look's weight inside the shape, 0 to 1: below 1 it tints what lies under it rather than replacing it.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub strength: f32,
    /// Which applies later, over the others, where volumes overlap: the highest.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: u8,
}

fn one() -> f32 {
    1.0
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_one(value: &f32) -> bool {
    *value == 1.0
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_zero(value: &u8) -> bool {
    *value == 0
}

/// A zone's atmosphere volumes, cooked: each look they name as its text, once, and the volumes by ascending ID.
#[derive(Clone, Debug, Default, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedAtmosphere {
    /// The `TodTimeline` documents the volumes name, as RON text, in the order the document first names them.
    pub looks: Vec<String>,
    pub volumes: Vec<CookedAtmosphereVolume>,
}

/// An atmosphere volume, cooked: see [`AtmosphereVolume`].
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedAtmosphereVolume {
    /// The name's hash ([`Name`]).
    pub id: u64,
    /// Its place in the document's list. Volumes of one priority apply in this order, earliest first, so a
    /// rename never reorders them.
    pub order: u32,
    pub name: String,
    pub position: [f32; 3],
    pub shape: ZoneShape,
    /// Its look, in [`CookedAtmosphere::looks`].
    pub look: u32,
    pub blend: f32,
    pub strength: f32,
    pub priority: u8,
}

impl ArchivedCookedAtmosphere {
    /// The volumes and their looks as plain values: what a reader takes off the frame's thread to parse the looks.
    #[must_use]
    pub fn to_native(&self) -> CookedAtmosphere {
        CookedAtmosphere {
            looks: self.looks.iter().map(|text| text.as_str().to_owned()).collect(),
            volumes: self
                .volumes
                .iter()
                .map(|volume| CookedAtmosphereVolume {
                    id: volume.id.to_native(),
                    order: volume.order.to_native(),
                    name: volume.name.as_str().to_owned(),
                    position: volume.position.each_ref().map(|value| value.to_native()),
                    shape: volume.shape.to_native(),
                    look: volume.look.to_native(),
                    blend: volume.blend.to_native(),
                    strength: volume.strength.to_native(),
                    priority: volume.priority,
                })
                .collect(),
        }
    }
}

/// Every look `volumes` name, each once in the order first named: the files a cook reads, and the order
/// [`cook`] takes their texts in.
pub(crate) fn looks(volumes: &[AtmosphereVolume]) -> Vec<&str> {
    let mut looks: Vec<&str> = Vec::new();
    for volume in volumes {
        if !looks.contains(&volume.look.as_str()) {
            looks.push(&volume.look);
        }
    }
    looks
}

/// Whether a document's volumes are some a cook can take, naming the field of what isn't. Whether each look
/// reads as a day is the cook's to check, which reads the file.
pub(crate) fn check(volumes: &[AtmosphereVolume]) -> Result<(), ZoneError> {
    if volumes.len() > MAX_VOLUMES {
        let problem = format!("{} volumes is more than the {MAX_VOLUMES} a zone may hold", volumes.len());
        return Err(ZoneError::invalid("volumes", problem));
    }
    check_names("volumes", volumes.iter().map(|volume| volume.name.as_str()))?;
    for (index, volume) in volumes.iter().enumerate() {
        let invalid = |name: &str| {
            let field = format!("volumes[{index}].{name}");
            move |problem: String| ZoneError::invalid(field, problem)
        };
        check_position(volume.position).map_err(invalid("position"))?;
        volume.shape.check().map_err(invalid("shape"))?;
        check_path(&volume.look).map_err(invalid("look"))?;
        check_blend(volume.blend).map_err(invalid("blend"))?;
        check_strength(volume.strength).map_err(invalid("strength"))?;
    }
    Ok(())
}

fn check_blend(blend: f32) -> Result<(), String> {
    if blend.is_finite() && (0.0..=MAX_BLEND).contains(&blend) {
        Ok(())
    } else {
        Err(format!("{blend} m isn't from 0 to {MAX_BLEND} m"))
    }
}

fn check_strength(strength: f32) -> Result<(), String> {
    if strength.is_finite() && (0.0..=1.0).contains(&strength) {
        Ok(())
    } else {
        Err(format!("{strength} isn't from 0 to 1"))
    }
}

/// A checked document's volumes, cooked with the `texts` of their [`looks`], in that order.
pub(crate) fn cook(volumes: &[AtmosphereVolume], texts: Vec<String>) -> Result<CookedAtmosphere, ZoneError> {
    let named = looks(volumes);
    if texts.len() != named.len() {
        let problem = format!("the cook read {} looks for the {} its volumes name", texts.len(), named.len());
        return Err(ZoneError::invalid("volumes", problem));
    }
    let mut cooked: Vec<CookedAtmosphereVolume> = volumes
        .iter()
        .zip(0..)
        .map(|(volume, order)| CookedAtmosphereVolume {
            id: Name::new(&volume.name).as_u64(),
            order,
            name: volume.name.clone(),
            position: volume.position,
            shape: volume.shape,
            look: named.iter().position(|&look| look == volume.look).expect("one of its looks") as u32,
            blend: volume.blend,
            strength: volume.strength,
            priority: volume.priority,
        })
        .collect();
    cooked.sort_unstable_by_key(|volume| volume.id);
    check_distinct("volumes", &cooked, |volume| (volume.id, &volume.name))?;
    Ok(CookedAtmosphere { looks: texts, volumes: cooked })
}

/// Whether a cooked zone's volumes are ones a cook would write, whatever wrote them: each look named and none
/// empty, the volumes by ascending ID, their orders each place in the list once, each good.
pub(crate) fn validate(presentation: &ArchivedZonePresentation) -> Result<(), ZoneError> {
    let atmosphere = &presentation.atmosphere;
    let volumes = &atmosphere.volumes;
    if volumes.len() > MAX_VOLUMES {
        let problem = format!("{} volumes is more than the {MAX_VOLUMES} a zone may hold", volumes.len());
        return Err(ZoneError::invalid("volumes", problem));
    }
    check_ids("volumes", volumes.iter().map(|volume| (volume.id.to_native(), volume.name.as_str())))?;
    let looks = atmosphere.looks.len();
    let mut placed = vec![false; volumes.len()];
    for volume in volumes.iter() {
        let invalid =
            |problem: String| ZoneError::invalid(format!("volume `{}`", volume.name.as_str()), problem);
        check_position(volume.position.each_ref().map(|value| value.to_native())).map_err(invalid)?;
        volume.shape.to_native().check().map_err(invalid)?;
        check_blend(volume.blend.to_native()).map_err(invalid)?;
        check_strength(volume.strength.to_native()).map_err(invalid)?;
        let look = volume.look.to_native() as usize;
        if look >= looks {
            return Err(invalid(format!("its look {look} isn't one of the zone's {looks}")));
        }
        let order = volume.order.to_native() as usize;
        match placed.get_mut(order) {
            Some(place @ false) => *place = true,
            _ => return Err(invalid(format!("its order {order} isn't a place of its own in the list"))),
        }
    }
    for (index, text) in atmosphere.looks.iter().enumerate() {
        if text.is_empty() {
            return Err(ZoneError::invalid(format!("volumes' look {index}"), "the document's text is empty"));
        }
        if !volumes.iter().any(|volume| volume.look.to_native() as usize == index) {
            return Err(ZoneError::invalid(format!("volumes' look {index}"), "no volume names it"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{ZoneDocument, ZoneError};

    fn document(volumes: &str) -> Result<ZoneDocument, ZoneError> {
        ZoneDocument::from_ron(&format!(
            r#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 32.0), volumes: [{volumes}]))"#
        ))
    }

    fn grotto(more: &str) -> String {
        format!(
            r#"(name: "grotto", position: (12.0, 2.0, -30.0), shape: Ellipsoid(radii: (14.0, 6.0, 10.0), yaw: 20.0), look: "../looks/biomes/cave_day.ron", blend: 6.0{more})"#
        )
    }

    #[test]
    fn a_volume_takes_its_defaults_and_writes_only_what_it_sets() {
        let read = document(&grotto("")).expect("a good document");
        let volume = &read.volumes[0];
        assert_eq!((volume.strength, volume.priority, volume.blend), (1.0, 0, 6.0));
        let text = sapphire_serialize::to_ron(&read).expect("writes");
        assert!(
            !text.contains("strength") && !text.contains("priority") && text.contains("blend: 6.0"),
            "{text}"
        );
        assert_eq!(ZoneDocument::from_ron(&text).expect("reads back"), read);
        let tinted = document(&grotto(", strength: 0.0, priority: 3")).expect("strength 0 tints nothing");
        assert_eq!((tinted.volumes[0].strength, tinted.volumes[0].priority), (0.0, 3));
        let unblended = document(&grotto("").replace(", blend: 6.0", ""));
        assert!(unblended.is_err(), "a volume says how its look fades out");
    }

    /// Two volumes naming one preset cook it once, each pointing at it, by ascending ID.
    #[test]
    fn volumes_naming_one_look_share_its_text() {
        let inn = r#"(name: "inn", position: (0.0, 2.0, 0.0), shape: Box(half_extents: (4.0, 2.0, 3.0)), look: "inn_day.ron", blend: 1.0, priority: 1)"#;
        let cellar = r#"(name: "cellar", position: (0.0, -2.0, 0.0), shape: Sphere(radius: 3.0), look: "../looks/biomes/cave_day.ron", blend: 0.5)"#;
        let document = document(&format!("{}, {inn}, {cellar}", grotto(""))).expect("a good document");
        assert_eq!(super::looks(&document.volumes), ["../looks/biomes/cave_day.ron", "inn_day.ron"]);
        let texts = vec!["(cave)".to_owned(), "(inn)".to_owned()];
        let cooked = super::cook(&document.volumes, texts.clone()).expect("cooks");
        assert_eq!(cooked.looks, texts);
        assert!(cooked.volumes.windows(2).all(|pair| pair[0].id < pair[1].id), "by ascending ID");
        let look = |name: &str| cooked.volumes.iter().find(|volume| volume.name == name).expect("there").look;
        assert_eq!((look("grotto"), look("cellar"), look("inn")), (0, 0, 1));
        assert!(super::cook(&document.volumes, texts[..1].to_vec()).is_err(), "a look's text missing");
    }

    #[test]
    fn a_bad_volume_is_refused_by_its_field() {
        let cases = [
            (grotto("").replace("blend: 6.0", "blend: -1.0"), "volumes[0].blend"),
            (grotto("").replace("blend: 6.0", "blend: 300.0"), "volumes[0].blend"),
            (grotto(", strength: 1.5"), "volumes[0].strength"),
            (grotto(", strength: -0.1"), "volumes[0].strength"),
            (grotto("").replace("radii: (14.0, 6.0, 10.0)", "radii: (14.0, 0.0, 10.0)"), "volumes[0].shape"),
            (
                grotto("").replace("Ellipsoid(radii: (14.0, 6.0, 10.0), yaw: 20.0)", "Sphere(radius: -1.0)"),
                "volumes[0].shape",
            ),
            (grotto("").replace("../looks/biomes/cave_day.ron", ""), "volumes[0].look"),
            (grotto("").replace("../looks/biomes/cave_day.ron", "/looks/cave_day.ron"), "volumes[0].look"),
            (grotto("").replace("(12.0, 2.0, -30.0)", "(1e9, 2.0, -30.0)"), "volumes[0].position"),
            (format!("{}, {}", grotto(""), grotto("")), "volumes[1].name"),
        ];
        for (volumes, field) in cases {
            let message = document(&volumes).expect_err(field).to_string();
            assert!(message.contains(field), "`{message}` must name `{field}`");
        }
        let message = document(&grotto(", density: 1.0")).expect_err("unknown").to_string();
        assert!(message.contains("density"), "{message}");
        let crowded: Vec<String> = (0..=super::MAX_VOLUMES)
            .map(|index| grotto("").replace("grotto", &format!("grotto_{index}")))
            .collect();
        let message = document(&crowded.join(", ")).expect_err("65").to_string();
        assert!(message.contains("`volumes`") && message.contains("more than the 64"), "{message}");
    }
}
