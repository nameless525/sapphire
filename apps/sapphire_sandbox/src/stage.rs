//! The stage (`--stage floor|pillar`, M1 R4): a floor under the character and an environment
//! caster beside it, generated in code like the swatches and the stalk, so the sandbox has
//! shadows to look at in a controlled frame: a character's contact shadow on the ground, an
//! environment shadow across a character and over faces. The courtyard (`--environment`, W4)
//! shows them in a world.
//!
//! - **The pedestal splits off.** The sample mannequin stands on a pedestal that is part of its
//!   own mesh, and a character never receives its own shadow, so its feet's contact shadow
//!   couldn't fall on it. On the stage the character draws the mannequin without its pedestal
//!   (its `Pedestal` material slot's submeshes left out, the streams as cooked, so the figure
//!   draws exactly as before), and the pedestal is an environment instance of its own, at rest
//!   while the figure turns: it receives the character's contact shadow, and casts its own.
//! - **The floor**, [`FLOOR_SIZE`] meters square, at the pedestal's foot.
//! - **A pillar** (`pillar`), a square stone post standing on the floor toward the key light
//!   from the subject (the character, or the face rig with `--faces`), sized so its shadow falls
//!   across it: on the character from its feet to its chest, so the edge crosses its body; over
//!   every head of the face rig, from far enough away to stay out of the rig's view.
//! - **Placed once,** for the key light at the hour the scene starts (`--time-of-day`): a running
//!   day moves the sun, and the shadows with it.
//! - **Materials:** the environment family (W4), a warm floor and grey stone with a soft wrap and
//!   no highlight, so shadows on them keep time of day's tint. The pedestal keeps the
//!   mannequin's anime character material: an environment instance drawn with that family.

use sapphire_assets::material::{EnvironmentMaterial, Material, Terminator};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::{MeshHandle, Renderer};

/// The floor's side, in meters: long sunset shadows stay on it.
pub(crate) const FLOOR_SIZE: f32 = 64.0;
/// The material slot of the sample mannequin's pedestal (`sapphire_pipeline`'s samples).
const PEDESTAL_SLOT: &str = "Pedestal";

/// What the stage holds besides the floor and the pedestal (`--stage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StageLayout {
    Floor,
    Pillar,
}

impl StageLayout {
    /// The layout `--stage` names.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "floor" => Some(Self::Floor),
            "pillar" => Some(Self::Pillar),
            _ => None,
        }
    }
}

/// A stage piece's mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    Floor,
    Pillar,
    Pedestal,
}

/// The stage's meshes on a renderer: its own, and the character split from its pedestal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StageMeshes {
    pub floor: MeshHandle,
    pub pillar: MeshHandle,
    /// The character without its pedestal, which the turntable draws on the stage.
    pub figure: MeshHandle,
    /// The pedestal alone; `None` for a character without one.
    pub pedestal: Option<MeshHandle>,
}

impl StageMeshes {
    /// The mesh `piece` draws; a pedestal piece exists only with a pedestal mesh.
    pub fn get(&self, piece: Piece) -> MeshHandle {
        match piece {
            Piece::Floor => self.floor,
            Piece::Pillar => self.pillar,
            Piece::Pedestal => self.pedestal.expect("a pedestal piece is placed only with a pedestal"),
        }
    }
}

/// What the stage is built around: where its subject stands, and how the pillar's shadow covers
/// it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Subject {
    /// The point on the floor below the subject's middle.
    pub base: Vec3,
    /// The height (world space) the pillar's shadow covers the subject up to.
    pub shade_height: f32,
    /// How far from the subject the pillar stands.
    pub pillar_distance: f32,
    pub pillar_width: f32,
}

/// Uploads the floor and the pillar, each with its embedded material, and the `character`
/// split from its pedestal (see the module docs).
pub(crate) fn upload_stage(renderer: &mut Renderer, character: &Mesh) -> Result<StageMeshes, AppInitError> {
    let (figure, pedestal) = split_pedestal(character);
    Ok(StageMeshes {
        floor: upload(renderer, &floor_mesh())?,
        pillar: upload(renderer, &pillar_mesh())?,
        figure: upload(renderer, &figure)?,
        pedestal: pedestal.as_ref().map(|pedestal| upload(renderer, pedestal)).transpose()?,
    })
}

/// Uploads the `character` without its pedestal: the stage's figure, and the rim rig's
/// (`crate::rims`).
pub(crate) fn upload_figure(renderer: &mut Renderer, character: &Mesh) -> Result<MeshHandle, AppInitError> {
    upload(renderer, &split_pedestal(character).0)
}

/// Uploads `mesh` with its embedded materials, as cooked.
pub(crate) fn upload(renderer: &mut Renderer, mesh: &Mesh) -> Result<MeshHandle, AppInitError> {
    let bytes = mesh.to_cooked().map_err(AppInitError::msg)?;
    let mesh = Mesh::read(&bytes)?;
    let materials = renderer.upload_embedded_materials(mesh)?;
    Ok(renderer.upload_mesh(mesh, &materials)?)
}

/// `mesh` without its pedestal's submeshes, and the pedestal's alone (`None` without a
/// [`PEDESTAL_SLOT`]): every LOD's submeshes split by material slot, the streams and slots kept as
/// they are, so each part draws exactly as it did in the whole.
fn split_pedestal(mesh: &Mesh) -> (Mesh, Option<Mesh>) {
    let Some(slot) = mesh.materials.iter().position(|slot| slot.name == PEDESTAL_SLOT) else {
        return (mesh.clone(), None);
    };
    let part = |keep: &dyn Fn(&Submesh) -> bool| {
        let mut part = mesh.clone();
        for lod in &mut part.lods {
            lod.submeshes.retain(|submesh| keep(submesh));
        }
        part
    };
    let slot = slot as u32;
    (part(&|submesh| submesh.material != slot), Some(part(&|submesh| submesh.material == slot)))
}

/// Where each piece of `layout` stands around `subject`, for a key light toward `toward_light`:
/// the floor first, then the pedestal (with `pedestal`), then the pillar.
pub(crate) fn placements(
    layout: StageLayout,
    subject: &Subject,
    pedestal: bool,
    toward_light: Vec3,
) -> Vec<(Piece, Mat4)> {
    let floor = subject.base.y;
    let mut pieces = vec![(
        Piece::Floor,
        Mat4::from_scale_rotation_translation(
            Vec3::new(FLOOR_SIZE, 1.0, FLOOR_SIZE),
            Quat::IDENTITY,
            Vec3::new(subject.base.x, floor, subject.base.z),
        ),
    )];
    if pedestal {
        pieces.push((Piece::Pedestal, Mat4::IDENTITY));
    }
    // The light's direction across the floor, and how fast its rays climb per meter across it.
    let toward = toward_light.normalize_or(Vec3::Y);
    let level = Vec3::new(toward.x, 0.0, toward.z);
    let across = level.normalize_or(Vec3::Z);
    let climb = (toward.y / level.length().max(1e-3)).max(0.0);
    if layout == StageLayout::Pillar {
        // A ray from the subject at the shade height toward the light passes the pillar's top:
        // everything below it is in the pillar's shadow.
        let distance = subject.pillar_distance;
        let height = (subject.shade_height - floor + distance * climb).clamp(0.5, 24.0);
        let width = subject.pillar_width;
        let at = subject.base + across * distance;
        let place = Vec3::new(at.x, floor, at.z);
        let size = Vec3::new(width, height, width);
        pieces.push((Piece::Pillar, Mat4::from_scale_rotation_translation(size, Quat::IDENTITY, place)));
    }
    pieces
}

/// A square of side 1 at the origin, facing up: the floor before its placement's scale.
fn floor_geometry() -> (Vec<Vertex>, Vec<u32>) {
    let vertices = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)]
        .into_iter()
        .map(|(x, z)| vertex([x, 0.0, z], [0.0, 1.0, 0.0], [x + 0.5, z + 0.5]))
        .collect();
    (vertices, vec![0, 2, 1, 0, 3, 2])
}

/// The stage's stone in `base_color`: a soft wrap, a narrow terminator widening with distance,
/// and no highlight.
fn stone(base_color: [f32; 3]) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [base_color[0], base_color[1], base_color[2], 1.0],
        roughness: 0.9,
        specular_clamp: 0.0,
        wrap: 0.3,
        terminator: Terminator { width: 0.04, growth: 0.006, band: None },
        ..EnvironmentMaterial::default()
    }
}

/// The floor's mesh: its geometry in a warm stone.
fn floor_mesh() -> Mesh {
    let (vertices, indices) = floor_geometry();
    mesh("floor", &vertices, indices, &stone([0.62, 0.58, 0.5]))
}

/// A box of 1 × 1 × 1 standing on the origin, with hard edges: a pillar before its placement's
/// scale.
fn pillar_geometry() -> (Vec<Vertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    // Each face: its normal and two axes spanning it, counter-clockwise seen from outside.
    let faces = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    for (normal, u, v) in faces {
        let first = vertices.len() as u32;
        for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            let p = normal * 0.5 + u * a + v * b + Vec3::new(0.0, 0.5, 0.0);
            vertices.push(vertex(p.to_array(), normal.to_array(), [a + 0.5, 0.5 - b]));
        }
        indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
    (vertices, indices)
}

/// The pillar's mesh: its geometry in grey stone.
fn pillar_mesh() -> Mesh {
    let (vertices, indices) = pillar_geometry();
    mesh("pillar", &vertices, indices, &stone([0.55, 0.55, 0.58]))
}

fn vertex(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Vertex {
    Vertex {
        position,
        normal,
        outline_normal: normal,
        uv,
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    }
}

/// `vertices` and `indices` as a mesh drawn with `material`, embedded.
fn mesh(name: &str, vertices: &[Vertex], indices: Vec<u32>, material: &EnvironmentMaterial) -> Mesh {
    let swizzles = [Swizzle::IDENTITY; EnvironmentMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(material, &swizzles).expect("the stage's materials are valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    Mesh::from_vertices(
        vertices,
        indices,
        submeshes,
        vec![MaterialSlot::embedded(name, material)],
        PositionFormat::Unorm16,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUBJECT: Subject = Subject {
        base: Vec3::new(0.0, 0.0, 0.0),
        shade_height: 1.4,
        pillar_distance: 1.4,
        pillar_width: 0.5,
    };

    #[test]
    fn meshes_cook_and_face_outward() {
        for mesh in [floor_mesh(), pillar_mesh()] {
            let bytes = mesh.to_cooked().expect("cook");
            Mesh::read(&bytes).expect("a valid mesh");
        }
        // Every triangle winds counter-clockwise around its vertices' normal: up for the floor,
        // out of the box for the pillar.
        for (vertices, indices) in [floor_geometry(), pillar_geometry()] {
            for triangle in indices.as_chunks::<3>().0 {
                let [a, b, c] = triangle.map(|index| &vertices[index as usize]);
                let (pa, pb, pc) = (Vec3::from(a.position), Vec3::from(b.position), Vec3::from(c.position));
                let face = (pb - pa).cross(pc - pa).normalize();
                assert!(face.abs_diff_eq(Vec3::from(a.normal), 1e-6), "{triangle:?}");
            }
        }
        assert!(floor_geometry().0.iter().all(|vertex| vertex.normal == [0.0, 1.0, 0.0]));
        let (vertices, indices) = pillar_geometry();
        assert_eq!(vertices.len(), 24);
        for triangle in indices.as_chunks::<3>().0 {
            let center =
                triangle.iter().map(|&index| Vec3::from(vertices[index as usize].position)).sum::<Vec3>()
                    / 3.0;
            let normal = Vec3::from(vertices[triangle[0] as usize].normal);
            assert!(normal.dot(center - Vec3::new(0.0, 0.5, 0.0)) > 0.0, "outward: {triangle:?}");
        }
    }

    #[test]
    fn the_pedestal_splits_off_by_its_material_slot() {
        // The pillar as a stand-in character: one slot, renamed the pedestal's, plus a figure's.
        let mut mesh = pillar_mesh();
        let figure_slot = MaterialSlot { name: "Outfit".into(), ..mesh.materials[0].clone() };
        mesh.materials[0].name = PEDESTAL_SLOT.into();
        mesh.materials.push(figure_slot);
        mesh.lods[0].submeshes = vec![
            Submesh { first_index: 0, index_count: 12, material: 1 },
            Submesh { first_index: 12, index_count: 24, material: 0 },
        ];
        let (figure, pedestal) = split_pedestal(&mesh);
        let pedestal = pedestal.expect("a pedestal slot");
        assert_eq!(figure.lods[0].submeshes, [Submesh { first_index: 0, index_count: 12, material: 1 }]);
        assert_eq!(pedestal.lods[0].submeshes, [Submesh { first_index: 12, index_count: 24, material: 0 }]);
        assert_eq!(
            (figure.streams.clone(), figure.materials.clone()),
            (mesh.streams.clone(), mesh.materials.clone())
        );
        assert_eq!(pedestal.streams, mesh.streams, "the streams as cooked");
        let (whole, none) = split_pedestal(&floor_mesh());
        assert_eq!((whole, none), (floor_mesh(), None), "no pedestal: the whole mesh");
    }

    #[test]
    fn the_pillar_stands_toward_the_light_and_shades_the_subject_to_its_height() {
        let toward = Vec3::new(-0.55, 0.7, 0.45).normalize();
        let pieces = placements(StageLayout::Pillar, &SUBJECT, true, toward);
        assert_eq!(
            pieces.iter().map(|(piece, _)| *piece).collect::<Vec<_>>(),
            [Piece::Floor, Piece::Pedestal, Piece::Pillar]
        );
        let (scale, _, at) = pieces[2].1.to_scale_rotation_translation();
        assert!((at.y - SUBJECT.base.y).abs() < 1e-6, "on the floor");
        let toward_xz = Vec3::new(toward.x, 0.0, toward.z).normalize();
        assert!((at - SUBJECT.base).normalize().dot(toward_xz) > 0.999, "toward the light");
        // A ray from the subject at the shade height toward the light just clears the top.
        let ray = Vec3::new(SUBJECT.base.x, SUBJECT.shade_height, SUBJECT.base.z);
        let at_pillar =
            ray + toward * (SUBJECT.pillar_distance / Vec3::new(toward.x, 0.0, toward.z).length());
        assert!((at_pillar.y - (at.y + scale.y)).abs() < 1e-3);
        assert_eq!(placements(StageLayout::Floor, &SUBJECT, false, toward).len(), 1);
    }

    #[test]
    fn layouts_are_named_as_the_command_line_spells_them() {
        assert_eq!(StageLayout::from_name("floor"), Some(StageLayout::Floor));
        assert_eq!(StageLayout::from_name("pillar"), Some(StageLayout::Pillar));
        assert_eq!(StageLayout::from_name("arena"), None);
    }
}
