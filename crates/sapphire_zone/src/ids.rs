//! What a zone's placements, instances, lights and atmosphere volumes are called (M6 T8a): an ID each, unique
//! across all four lists, which no edit of another entry, no reorder and no merge changes.
//!
//! An ID is text. One an editor makes on insert is short and random, eight hexadecimal digits drawn again while the
//! zone has it, so two people placing things on two branches all but never meet: a merge whose two sides did is
//! refused by [`ZoneDocument::check`], naming the field, for a person to rename one. A person may type a name in
//! its place (`"gate_lamp"`) wherever that reads better (m6-plan §3, "Smaller defaults"). A version 1 document has
//! none, and reads with IDs from its entries' places, `placement-3`, which can't be eight hexadecimal digits: so a
//! migrated document's IDs never meet those made later.
//!
//! The cook reads no ID: a zone's packs and digests are the same whatever its entries are called. Its spawners,
//! triggers, checkpoints, splines, exits, reflection probes, water bodies, rooms and weather regions have names
//! instead, unique in their lists, which an editor's paths already call them by.

use std::collections::BTreeMap;

use crate::document::{ZoneDocument, ZoneError, check_name};

/// A list of a zone document whose entries have an ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IdList {
    Placements,
    Instances,
    Lights,
    Volumes,
}

impl IdList {
    /// Every one, in the document's order.
    pub const ALL: [Self; 4] = [Self::Placements, Self::Instances, Self::Lights, Self::Volumes];

    /// The list's field in the document (`placements`).
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::Placements => "placements",
            Self::Instances => "instances",
            Self::Lights => "lights",
            Self::Volumes => "volumes",
        }
    }

    /// The list whose field is `field`.
    #[must_use]
    pub fn of_field(field: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|list| list.field() == field)
    }

    /// What a version 1 document's entry at `index` of this list reads with: `placement-3`.
    #[must_use]
    pub fn place_id(self, index: usize) -> String {
        let one = match self {
            Self::Placements => "placement",
            Self::Instances => "instance",
            Self::Lights => "light",
            Self::Volumes => "volume",
        };
        format!("{one}-{index}")
    }
}

impl ZoneDocument {
    /// Every entry's ID, by its list and its place there, in the document's order.
    pub fn ids(&self) -> impl Iterator<Item = (IdList, usize, &str)> {
        fn of<'a>(
            list: IdList,
            ids: impl Iterator<Item = &'a String>,
        ) -> impl Iterator<Item = (IdList, usize, &'a str)> {
            ids.enumerate().map(move |(at, id)| (list, at, id.as_str()))
        }
        of(IdList::Placements, self.placements.iter().map(|entry| &entry.id))
            .chain(of(IdList::Instances, self.instances.iter().map(|entry| &entry.id)))
            .chain(of(IdList::Lights, self.lights.iter().map(|entry| &entry.id)))
            .chain(of(IdList::Volumes, self.volumes.iter().map(|entry| &entry.id)))
    }

    /// Whether some entry of the zone is called `id`.
    #[must_use]
    pub fn has_id(&self, id: &str) -> bool {
        self.ids().any(|(_, _, taken)| taken == id)
    }

    /// The list and the place of the entry called `id`.
    #[must_use]
    pub fn find_id(&self, id: &str) -> Option<(IdList, usize)> {
        self.ids().find(|(_, _, taken)| *taken == id).map(|(list, at, _)| (list, at))
    }

    /// This document with every entry's ID made from its place ([`IdList::place_id`]), whatever it was: how a
    /// version 1 document reads, and what a zone built in code calls its entries.
    #[must_use]
    pub fn with_place_ids(mut self) -> Self {
        for (index, entry) in self.placements.iter_mut().enumerate() {
            entry.id = IdList::Placements.place_id(index);
        }
        for (index, entry) in self.instances.iter_mut().enumerate() {
            entry.id = IdList::Instances.place_id(index);
        }
        for (index, entry) in self.lights.iter_mut().enumerate() {
            entry.id = IdList::Lights.place_id(index);
        }
        for (index, entry) in self.volumes.iter_mut().enumerate() {
            entry.id = IdList::Volumes.place_id(index);
        }
        self
    }

    /// The ID of entry `index` of `list`, if it has one.
    #[must_use]
    pub fn id_at(&self, list: IdList, index: usize) -> Option<&str> {
        let id = match list {
            IdList::Placements => &self.placements.get(index)?.id,
            IdList::Instances => &self.instances.get(index)?.id,
            IdList::Lights => &self.lights.get(index)?.id,
            IdList::Volumes => &self.volumes.get(index)?.id,
        };
        Some(id)
    }
}

/// Whether every entry has an ID and no two have one, naming the field of the first that doesn't.
pub(crate) fn check(document: &ZoneDocument) -> Result<(), ZoneError> {
    let mut seen: BTreeMap<&str, (IdList, usize)> = BTreeMap::new();
    for (list, index, id) in document.ids() {
        let field = format!("{}[{index}].id", list.field());
        check_name(id).map_err(|problem| {
            let problem = format!("{problem}: every entry has an ID, and a version 2 document writes each");
            ZoneError::invalid(field.clone(), problem)
        })?;
        if let Some((other, at)) = seen.insert(id, (list, index)) {
            let problem = format!(
                "`{id}` is the ID of `{}[{at}]` too, and an ID names one thing in the zone: rename one",
                other.field()
            );
            return Err(ZoneError::invalid(field, problem));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Quat, Vec3};
    use sapphire_math::det;

    use super::*;
    use crate::{Bounds, Instance, Placement, ZoneGeometry, document};

    const PREFAB: &str = "1ff3290e-5fa9-4f61-b891-2ca6d76b285f";

    fn read(version: u32, data: &str) -> Result<ZoneDocument, ZoneError> {
        ZoneDocument::from_ron(&format!(
            r#"(type: "sapphire::zone::ZoneDocument", version: {version}, data: (grid: (cell_size: 32.0), {data}))"#
        ))
    }

    /// Two placements, an instance and a light, each with the ID given, or none where it's empty.
    fn entries(ids: [&str; 4]) -> String {
        let id =
            |at: usize| if ids[at].is_empty() { String::new() } else { format!(r#"id: "{}", "#, ids[at]) };
        format!(
            r#"
            placements: [({}prefab: "{PREFAB}", at: (1.0, 0.0, 1.0), turn: 30.0), ({}prefab: "{PREFAB}")],
            instances: [({}mesh: "{PREFAB}", at: (2.0, 0.0, 2.0))],
            lights: [({}position: (0.0, 2.0, 0.0), color: (1.0, 1.0, 1.0), intensity: 1.0, range: 4.0)],
            "#,
            id(0),
            id(1),
            id(2),
            id(3),
        )
    }

    /// A version 1 document has no IDs and reads with its places', whatever its text says; it writes as
    /// version 2 with them, which reads back as it was.
    #[test]
    fn a_version_one_document_reads_with_ids_from_its_places() {
        let old = read(1, &entries(["", "", "", ""])).expect("version 1 reads");
        let ids: Vec<(IdList, usize, &str)> = old.ids().collect();
        assert_eq!(
            ids,
            [
                (IdList::Placements, 0, "placement-0"),
                (IdList::Placements, 1, "placement-1"),
                (IdList::Instances, 0, "instance-0"),
                (IdList::Lights, 0, "light-0"),
            ]
        );
        let said = read(1, &entries(["a", "b", "c", "d"])).expect("reads");
        assert_eq!(said, old, "a version 1 document's IDs are its places'");

        let text = sapphire_serialize::to_ron(&old).expect("write");
        assert!(text.contains("version: 2") && text.contains(r#"id: "light-0""#), "{text}");
        assert_eq!(ZoneDocument::from_ron(&text).expect("version 2 reads"), old);
        assert_eq!(old.find_id("instance-0"), Some((IdList::Instances, 0)));
        assert_eq!(old.id_at(IdList::Placements, 1), Some("placement-1"));
        assert!(!old.has_id("placement-2"));
        assert_eq!(IdList::of_field("lights"), Some(IdList::Lights));
    }

    /// Version 2 writes every ID: one left out, or one two entries share in any of the four lists, is refused
    /// by its field.
    #[test]
    fn a_version_two_document_names_each_entry_once() {
        let good = read(2, &entries(["3fa91c07", "gate_crate", "8c5d4e21", "e0a4f6b9"])).expect("reads");
        assert_eq!(good.placements[1].id, "gate_crate", "a typed name is an ID");
        for (ids, field) in [
            (["3fa91c07", "", "8c5d4e21", "e0a4f6b9"], "`placements[1].id`"),
            (["3fa91c07", "3fa91c07", "8c5d4e21", "e0a4f6b9"], "`placements[1].id`"),
            (["3fa91c07", "gate_crate", "8c5d4e21", "gate_crate"], "`lights[0].id`"),
        ] {
            let message = read(2, &entries(ids)).expect_err(field).to_string();
            assert!(message.contains(field), "`{message}` must name {field}");
        }
        let message = read(2, &entries(["a", "b", "c", "a"])).expect_err("twice").to_string();
        assert!(message.contains("`placements[0]`"), "says where the other is: {message}");
    }

    /// The cook reads no ID: renamed, a zone cooks to the same zone.
    #[test]
    fn the_cook_ignores_ids() {
        let named = read(2, &entries(["3fa91c07", "gate_crate", "8c5d4e21", "e0a4f6b9"])).expect("reads");
        let placed = read(1, &entries(["", "", "", ""])).expect("reads");
        assert_ne!(named, placed);
        let geometry = || ZoneGeometry {
            instances: vec![Bounds { min: [2.0; 3], max: [2.5; 3] }],
            ..ZoneGeometry::default()
        };
        assert_eq!(named.cook(geometry()).expect("cooks"), placed.cook(geometry()).expect("cooks"));
    }

    /// A turn cooks as it did before `rotation` was, through the very call; a rotation of `(0, t, 0)` is that
    /// turn, and the three axes apply about Z, then X, then Y.
    #[test]
    fn a_rotation_is_yaw_pitch_and_roll_and_a_turn_is_its_yaw() {
        let first = read(2, &entries(["a", "b", "c", "d"])).expect("reads").placements[0].clone();
        let placement = |turn, rotation| Placement { turn, rotation, ..first.clone() };
        let turned = placement(30.0, None);
        let before = det::quat_from_axis_angle(Vec3::Y, 30.0_f32.to_radians());
        assert_eq!(turned.orientation().to_array().map(f32::to_bits), before.to_array().map(f32::to_bits));
        // A yaw alone is its turn to the bit, at any angle: a negative one's signed zeros, and one past 360.
        let instance = |turn, rotation| Instance { turn, rotation, ..Instance::default() };
        for yaw in [30.0, -30.0, -150.0, 400.0, -0.0] {
            let turned = placement(yaw, None).orientation().to_array().map(f32::to_bits);
            assert_eq!(
                placement(0.0, Some([0.0, yaw, 0.0])).orientation().to_array().map(f32::to_bits),
                turned
            );
            let as_instance = instance(yaw, None).orientation().to_array().map(f32::to_bits);
            assert_eq!(as_instance, turned, "{yaw}");
            let yawed = instance(0.0, Some([0.0, yaw, 0.0])).orientation().to_array().map(f32::to_bits);
            assert_eq!(yawed, turned, "{yaw}");
        }

        let rotated = placement(0.0, Some([20.0, 30.0, 40.0]));
        let orientation = rotated.orientation();
        // Rolled about Z, then pitched about X, then yawed about Y, one at a time.
        let one_by_one = |point: Vec3| {
            let rolled = det::quat_rotate(det::quat_from_rotation_z(40.0_f32.to_radians()), point);
            let pitched = det::quat_rotate(det::quat_from_rotation_x(20.0_f32.to_radians()), rolled);
            det::quat_rotate(det::quat_from_rotation_y(30.0_f32.to_radians()), pitched)
        };
        for point in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(1.0, -2.0, 0.5)] {
            let turned = det::quat_rotate(orientation, point);
            assert!(
                turned.abs_diff_eq(one_by_one(point), 1e-5),
                "{point}: {turned} against {}",
                one_by_one(point)
            );
        }
        let whole = Mat4::from_scale_rotation_translation(Vec3::ONE, orientation, Vec3::new(1.0, 0.0, 1.0));
        assert!(rotated.shape_transform().abs_diff_eq(whole, 1e-6));
        assert_eq!(document::orientation(0.0, None), Quat::IDENTITY);
    }
}
