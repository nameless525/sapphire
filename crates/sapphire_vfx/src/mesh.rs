//! An effect's mesh layers (M5 L5): a generated shape the effect shows from its start, placed with it,
//! colored, patterned, scrolled, eroded and flipped through over its own life, all from the document. See
//! [`MeshLayerDesc`].
//!
//! A layer takes no random numbers, so adding one to an effect moves no spark of it or of any effect spawned
//! after it, and it resolves on its owner's clock as sprites do: frozen in its owner's hitstop, read back
//! along the owner's last step.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use glam::{Quat, Vec2, Vec3};
use sapphire_look::{VfxErosion, VfxFlipbook, VfxMesh, VfxMeshShape, VfxPattern};
use sapphire_math::det;
use serde::{Deserialize, Serialize};

use crate::curve::Curve;
use crate::document::{Problem, curve_ok, default_soft, one, one_share, require, share_ok};
use crate::world::Placement;

/// Mesh layers an effect may have.
pub const MAX_MESH_LAYERS: usize = 4;

/// The shape a layer takes (`VfxMeshShape`): generated, about a metre across, for `scale` to size.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum MeshShape {
    /// A crescent round the origin in the XZ plane, from -X through +Z to +X: a swing round the body,
    /// facing the way its effect's placement does.
    SlashArc,
    /// A flat band round the origin in the XZ plane, radii 0.8 to 1: a shockwave on the ground.
    Ring,
    /// An open column, radius 1 at its foot narrowing to 0.7 a metre up: a pillar of light.
    Column,
    /// A hemisphere of radius 1: a burst's shell.
    Dome,
    /// A square a metre a side facing +Z: a flipbook's card.
    Card,
}

impl From<MeshShape> for VfxMeshShape {
    fn from(shape: MeshShape) -> Self {
        match shape {
            MeshShape::SlashArc => Self::SlashArc,
            MeshShape::Ring => Self::Ring,
            MeshShape::Column => Self::Column,
            MeshShape::Dome => Self::Dome,
            MeshShape::Card => Self::Card,
        }
    }
}

/// A generated pattern (`VfxPattern`): a coverage that tiles across the shape's UVs.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum Pattern {
    /// Everywhere 1.
    #[default]
    Solid,
    /// Lanes of streaks along `u`, for scrolling along a sweep.
    Streaks,
    /// Soft value noise: an erosion's mask.
    Noise,
    /// A 4 by 4 flipbook of a burst.
    Burst,
}

impl From<Pattern> for VfxPattern {
    fn from(pattern: Pattern) -> Self {
        match pattern {
            Pattern::Solid => Self::Solid,
            Pattern::Streaks => Self::Streaks,
            Pattern::Noise => Self::Noise,
            Pattern::Burst => Self::Burst,
        }
    }
}

fn noise() -> Pattern {
    Pattern::Noise
}

fn unit_tiling() -> [f32; 2] {
    [1.0, 1.0]
}

fn unit_scale() -> Curve<[f32; 3]> {
    Curve(vec![(0.0, [1.0, 1.0, 1.0])])
}

/// An erosion: the layer shows only where its mask is over a threshold that moves over the layer's life,
/// with an edge of its own color just over it.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Erosion {
    #[serde(default = "noise")]
    pub mask: Pattern,
    /// The mask's repeats across the shape's UVs. It doesn't scroll.
    #[serde(default = "unit_tiling")]
    pub tiling: [f32; 2],
    /// How far it has eroded over the layer's life: 0 whole, 1 gone.
    pub threshold: Curve<f32>,
    /// The edge's width in the mask's values, 0 to 1: 0 has none.
    #[serde(default)]
    pub edge: f32,
    /// The edge's linear color, relative to the display.
    #[serde(default)]
    pub edge_color: [f32; 3],
}

/// When a flipbook turns its frames.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum FrameTiming {
    /// Every frame once over the layer's life, evenly.
    #[default]
    Life,
    /// `rate` frames a second on the owner's clock, from the first again if `looped`, or holding the last.
    PerSecond { rate: f32, looped: bool },
}

/// A flipbook: the pattern is a sheet of frames, `columns` by `rows` in reading order, of which the layer
/// shows `frames` in turn.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flipbook {
    pub columns: u32,
    pub rows: u32,
    pub frames: u32,
    #[serde(default)]
    pub timing: FrameTiming,
    /// Blend each frame into the next as it turns. Off, frames step, as an anime effect's drawn frames do.
    #[serde(default)]
    pub blend: bool,
}

/// The most columns or rows a flipbook's sheet has.
const MAX_GRID: u32 = 8;

/// A mesh layer of an effect: what [`VfxMesh`] it resolves to at each moment of its life. Every field but
/// `shape`, `lifetime` and `color` has a default, so a plain one is three lines.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshLayerDesc {
    /// What errors call it.
    #[serde(default)]
    pub name: String,
    pub shape: MeshShape,
    /// Seconds on the owner's clock from the effect's start before it shows.
    #[serde(default)]
    pub delay: f32,
    /// Seconds it shows.
    pub lifetime: f32,
    /// Metres from the effect's placement, in the placement's frame.
    #[serde(default)]
    pub offset: [f32; 3],
    /// Degrees about the placement's X, Y and Z axes, applied Z, then X, then Y.
    #[serde(default)]
    pub rotation: [f32; 3],
    /// Degrees a second about its own Y axis, after `rotation`: a ring's spin, a slash's follow-through.
    #[serde(default)]
    pub spin: f32,
    /// Metres along the shape's axes over its life.
    #[serde(default = "unit_scale")]
    pub scale: Curve<[f32; 3]>,
    /// Linear color over its life, relative to the display: a mesh is never lit.
    pub color: Curve<[f32; 3]>,
    /// Its opacity over its life, over the pattern's coverage and the shape's own fades.
    #[serde(default = "one")]
    pub alpha: Curve<f32>,
    /// As an emitter's: 1 pure light, 0 a surface that covers.
    #[serde(default = "one_share")]
    pub additive: f32,
    /// As an emitter's: over how many metres it fades into the surface behind it, where the frame has depth.
    #[serde(default = "default_soft")]
    pub soft: f32,
    #[serde(default)]
    pub pattern: Pattern,
    /// The pattern's repeats across the shape's UVs.
    #[serde(default = "unit_tiling")]
    pub tiling: [f32; 2],
    /// How fast the pattern scrolls toward `+u` and `+v`, in UVs a second on the owner's clock: `+u` runs along
    /// a slash's sweep from its tail to its head, and round a ring or a column the way a positive `spin` turns.
    #[serde(default)]
    pub scroll: [f32; 2],
    #[serde(default)]
    pub erosion: Option<Erosion>,
    #[serde(default)]
    pub flipbook: Option<Flipbook>,
    /// Which meshes draw when more want to than the tier's budget holds: the highest first, and of equal
    /// priority those of the effects spawned first.
    #[serde(default)]
    pub priority: u8,
}

fn finite2(value: [f32; 2]) -> bool {
    value.iter().all(|c| c.is_finite())
}

impl MeshLayerDesc {
    /// When it ends, seconds from its effect's start.
    pub(crate) fn end(&self) -> f32 {
        self.delay + self.lifetime
    }

    pub(crate) fn problem(&self) -> Result<(), Problem> {
        require(self.delay.is_finite() && self.delay >= 0.0, "delay", "can't be negative")?;
        require(self.lifetime.is_finite() && self.lifetime > 0.0, "lifetime", "must be above 0")?;
        require(self.offset.iter().all(|c| c.is_finite()), "offset", "must be finite")?;
        require(self.rotation.iter().all(|c| c.is_finite()), "rotation", "must be finite")?;
        require(self.spin.is_finite(), "spin", "must be finite")?;
        curve_ok(&self.scale, "scale")?;
        require(self.scale.never_negative(), "scale", "can't be negative")?;
        curve_ok(&self.color, "color")?;
        require(self.color.never_negative(), "color", "can't be negative")?;
        curve_ok(&self.alpha, "alpha")?;
        require(self.alpha.never_negative() && self.alpha.at_most_one(), "alpha", "must be 0 to 1")?;
        require(share_ok(self.additive), "additive", "must be 0 to 1")?;
        require(self.soft.is_finite() && self.soft > 0.0, "soft", "must be above 0")?;
        require(finite2(self.tiling) && self.tiling.iter().all(|c| *c > 0.0), "tiling", "must be above 0")?;
        require(finite2(self.scroll), "scroll", "must be finite")?;
        if let Some(erosion) = &self.erosion {
            require(
                erosion.mask != Pattern::Burst,
                "erosion.mask",
                "can't be a flipbook's sheet: its frames' borders would erode first",
            )?;
            require(
                finite2(erosion.tiling) && erosion.tiling.iter().all(|c| *c > 0.0),
                "erosion.tiling",
                "must be above 0",
            )?;
            curve_ok(&erosion.threshold, "erosion.threshold")?;
            require(
                erosion.threshold.never_negative() && erosion.threshold.at_most_one(),
                "erosion.threshold",
                "must be 0 to 1",
            )?;
            require(erosion.edge.is_finite() && erosion.edge >= 0.0, "erosion.edge", "can't be negative")?;
            require(erosion.edge <= 1.0, "erosion.edge", "must be at most 1")?;
            require(
                erosion.edge_color.iter().all(|c| c.is_finite() && *c >= 0.0),
                "erosion.edge_color",
                "can't be negative",
            )?;
        }
        match self.flipbook {
            Some(book) => {
                require(
                    (1..=MAX_GRID).contains(&book.columns) && (1..=MAX_GRID).contains(&book.rows),
                    "flipbook",
                    "`columns` and `rows` must be 1 to 8",
                )?;
                require(
                    (2..=book.columns * book.rows).contains(&book.frames),
                    "flipbook",
                    "`frames` must be 2 to `columns` times `rows`",
                )?;
                require(
                    VfxPattern::from(self.pattern).grid() == Some((book.columns, book.rows)),
                    "flipbook",
                    "its grid must be the pattern's: `Burst` is 4 by 4",
                )?;
                require(
                    self.tiling == [1.0, 1.0] && self.scroll == [0.0, 0.0],
                    "flipbook",
                    "a frame fills the shape: leave out `tiling` and `scroll`",
                )?;
                if let FrameTiming::PerSecond { rate, .. } = book.timing {
                    require(rate.is_finite() && rate > 0.0, "flipbook", "a rate must be above 0")?;
                }
            }
            None => require(
                VfxPattern::from(self.pattern).grid().is_none(),
                "pattern",
                "is a sheet of frames: it needs a `flipbook`",
            )?,
        }
        Ok(())
    }

    /// What it shows `age` seconds into its effect's life, placed at `placement`, if it shows then.
    pub(crate) fn resolve(&self, placement: &Placement, age: f32) -> Option<VfxMesh> {
        let local = age - self.delay;
        if !(0.0..self.lifetime).contains(&local) {
            return None;
        }
        let t = local / self.lifetime;
        let [x, y, z] = self.rotation.map(f32::to_radians);
        let turn = det::quat_mul(
            det::quat_from_rotation_y(y),
            det::quat_mul(det::quat_from_rotation_x(x), det::quat_from_rotation_z(z)),
        );
        let spin = det::quat_from_rotation_y((self.spin * local).to_radians());
        let rotation = det::quat_normalize(det::quat_mul(placement.rotation, det::quat_mul(turn, spin)));
        let flipbook = self.flipbook.map(|book| frames_at(book, t, local));
        Some(VfxMesh {
            shape: self.shape.into(),
            position: placement.position + det::quat_rotate(placement.rotation, Vec3::from(self.offset)),
            rotation: if rotation.is_finite() { rotation } else { Quat::IDENTITY },
            scale: self.scale.sample_vec3(t),
            color: self.color.sample_vec3(t),
            alpha: self.alpha.sample(t),
            additive: self.additive,
            soft: self.soft,
            pattern: self.pattern.into(),
            tiling: Vec2::from(self.tiling),
            scroll: Vec2::new(
                (self.scroll[0] * local).rem_euclid(1.0),
                (self.scroll[1] * local).rem_euclid(1.0),
            ),
            erosion: self.erosion.as_ref().map(|erosion| VfxErosion {
                mask: erosion.mask.into(),
                tiling: Vec2::from(erosion.tiling),
                threshold: erosion.threshold.sample(t).clamp(0.0, 1.0),
                edge: erosion.edge,
                edge_color: Vec3::from(erosion.edge_color),
            }),
            flipbook,
        })
    }
}

/// The frames a flipbook shows `t` of the way through its layer's life, `local` seconds in.
fn frames_at(book: Flipbook, t: f32, local: f32) -> VfxFlipbook {
    let last = book.frames - 1;
    let (at, looped) = match book.timing {
        FrameTiming::Life => (t * book.frames as f32, false),
        FrameTiming::PerSecond { rate, looped } => (local * rate, looped),
    };
    let whole = at.floor();
    let (frame, next) = if looped {
        let frame = (whole as u64 % u64::from(book.frames)) as u32;
        (frame, (frame + 1) % book.frames)
    } else {
        let frame = (whole.max(0.0) as u32).min(last);
        (frame, (frame + 1).min(last))
    };
    let blend = if book.blend && next != frame { at - whole } else { 0.0 };
    VfxFlipbook { columns: book.columns, rows: book.rows, frame, next, blend }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectId, EffectLibrary, OwnerId, OwnerSteps, VfxBudget, VfxWorld};
    use sapphire_look::VfxFrame;

    /// A version 2 document of one effect, `layers` its mesh layers' text and `emitters` its emitters'.
    fn document(emitters: &str, layers: &str) -> String {
        format!(
            r#"(
                type: "sapphire::vfx::EffectLibrary",
                version: 2,
                data: (effects: [(name: "slash", emitters: [{emitters}], meshes: [{layers}])]),
            )"#
        )
    }

    const SPARKS: &str = r#"(
        name: "sparks",
        spawn: Burst(count: 6),
        lifetime: (0.3, 0.6),
        motion: (speed: (2.0, 4.0), spread: 60.0),
        size: [(0.0, 0.05)],
        color: [(0.0, (1.0, 1.0, 1.0))],
    )"#;

    const WHITE: &str = "color: [(0.0, (1.0, 1.0, 1.0))]";

    fn library(layers: &str) -> EffectLibrary {
        EffectLibrary::from_ron(&document("", layers)).expect("valid")
    }

    fn layer(library: &EffectLibrary) -> &MeshLayerDesc {
        &library.effect(EffectId(0)).expect("an effect").meshes[0]
    }

    fn error(layers: &str) -> String {
        EffectLibrary::from_ron(&document("", layers)).expect_err("invalid").to_string()
    }

    /// The smallest layer is its shape, lifetime and color; everything else takes its default and is off.
    #[test]
    fn a_plain_layer_is_three_lines() {
        let library = library(r#"(shape: Ring, lifetime: 0.4, color: [(0.0, (1.0, 0.8, 0.5))])"#);
        let ring = layer(&library);
        assert_eq!(ring.shape, MeshShape::Ring);
        assert_eq!((ring.delay, ring.spin, ring.scroll, ring.tiling), (0.0, 0.0, [0.0; 2], [1.0; 2]));
        assert_eq!(
            (ring.pattern, ring.erosion.is_none(), ring.flipbook.is_none()),
            (Pattern::Solid, true, true)
        );
        assert_eq!((ring.additive, ring.priority), (1.0, 0));
        assert!(library.has_meshes());
    }

    /// Every mistake names the effect, the layer and the field, with why.
    #[test]
    fn every_bad_layer_field_is_named_with_its_reason() {
        let cases = [
            ("lifetime: 0.0", "`lifetime` must be above 0"),
            ("lifetime: 1.0, delay: -1.0", "`delay` can't be negative"),
            ("lifetime: 1.0, additive: 2.0", "`additive` must be 0 to 1"),
            ("lifetime: 1.0, soft: 0.0", "`soft` must be above 0"),
            ("lifetime: 1.0, tiling: (0.0, 1.0)", "`tiling` must be above 0"),
            ("lifetime: 1.0, scale: [(0.0, (-1.0, 1.0, 1.0))]", "`scale` can't be negative"),
            ("lifetime: 1.0, alpha: [(0.0, 2.0)]", "`alpha` must be 0 to 1"),
            (
                "lifetime: 1.0, erosion: Some((threshold: [(0.0, 0.0)], edge: -0.1))",
                "`erosion.edge` can't be negative",
            ),
            ("lifetime: 1.0, erosion: Some((threshold: [(0.0, 1.5)]))", "`erosion.threshold` must be 0 to 1"),
            (
                "lifetime: 1.0, pattern: Burst, flipbook: Some((columns: 3, rows: 4, frames: 12))",
                "`flipbook` its grid must be the pattern's",
            ),
            (
                "lifetime: 1.0, pattern: Burst, flipbook: Some((columns: 0, rows: 4, frames: 2))",
                "`flipbook` `columns` and `rows` must be 1 to 8",
            ),
            (
                "lifetime: 1.0, pattern: Burst, flipbook: Some((columns: 4, rows: 4, frames: 17))",
                "`flipbook` `frames` must be 2 to `columns` times `rows`",
            ),
            ("lifetime: 1.0, pattern: Burst", "`pattern` is a sheet of frames"),
            ("lifetime: 1.0, flipbook: Some((columns: 4, rows: 4, frames: 16))", "`flipbook` its grid"),
            (
                "lifetime: 1.0, pattern: Burst, scroll: (1.0, 0.0), flipbook: Some((columns: 4, rows: 4, frames: 16))",
                "`flipbook` a frame fills the shape",
            ),
        ];
        for (fields, reason) in cases {
            let text = format!(r#"(name: "arc", shape: SlashArc, {WHITE}, {fields})"#);
            let error = error(&text);
            assert!(
                error.contains("effect `slash`, mesh layer `arc`") && error.contains(reason),
                "{fields}: {error}"
            );
        }
        let unnamed = error(&format!("(shape: Ring, lifetime: -1.0, {WHITE})"));
        assert!(unnamed.contains("mesh layer 0"), "{unnamed}");
    }

    /// A layer must say its shape: the error names it, with where in the document; and the rest of the
    /// document's rules hold for layers too.
    #[test]
    fn a_layer_without_a_shape_is_refused_with_where() {
        let error_text = error(&format!(r#"(name: "arc", lifetime: 1.0, {WHITE})"#));
        assert!(error_text.contains("shape") && error_text.contains(':'), "{error_text}");
        let misspelled = error(&format!("(shape: Ring, lifetime: 1.0, {WHITE}, scrol: (1.0, 0.0))"));
        assert!(misspelled.contains("scrol"), "{misspelled}");
        let one = format!("(shape: Ring, lifetime: 1.0, {WHITE})");
        let many = [one.as_str(); MAX_MESH_LAYERS + 1].join(", ");
        assert!(error(&many).contains("more than 4 mesh layers"));
    }

    /// A version 1 document still loads as it did, and a library with no mesh layers writes none, so it
    /// reads back the same.
    #[test]
    fn version_1_documents_load_unchanged_and_write_no_layers() {
        let text = document(SPARKS, "").replace("version: 2", "version: 1").replace(", meshes: []", "");
        assert!(!text.contains("meshes"));
        let library = EffectLibrary::from_ron(&text).expect("a version 1 document");
        assert!(!library.has_meshes());
        let written = library.to_ron().expect("written");
        assert!(!written.contains("meshes"), "{written}");
        assert_eq!(EffectLibrary::from_ron(&written).expect("read back"), library);
    }

    /// The pattern's offset is the scroll's speed times the layer's own time, wrapped to a repeat; erosion's
    /// threshold and the color follow its life.
    #[test]
    fn scroll_erosion_and_color_follow_the_layers_own_time() {
        let library = library(
            r#"(shape: SlashArc, delay: 0.1, lifetime: 1.0, scroll: (1.5, -0.25), offset: (0.0, 1.0, 0.0),
                color: [(0.0, (1.0, 0.0, 0.0)), (1.0, (0.0, 0.0, 1.0))],
                erosion: Some((threshold: [(0.0, 0.0), (1.0, 1.0)], edge: 0.1, edge_color: (2.0, 2.0, 2.0))))"#,
        );
        let arc = layer(&library);
        let turned = det::quat_from_rotation_y(std::f32::consts::FRAC_PI_2);
        let placement = Placement { position: Vec3::X, rotation: turned };
        assert!(arc.resolve(&placement, 0.05).is_none(), "not before its delay");
        assert!(arc.resolve(&placement, 1.1).is_none(), "not after its life");
        let mesh = arc.resolve(&placement, 0.1 + 0.5).expect("halfway");
        let local = 0.5_f32;
        let scroll = Vec2::new((1.5 * local).rem_euclid(1.0), (-0.25 * local).rem_euclid(1.0));
        assert!((mesh.scroll - scroll).length() < 1e-5, "{:?}", mesh.scroll);
        assert!((mesh.scroll - Vec2::new(0.75, 0.875)).length() < 1e-4, "{:?}", mesh.scroll);
        assert!((mesh.color - Vec3::new(0.5, 0.0, 0.5)).length() < 1e-5);
        let erosion = mesh.erosion.expect("eroding");
        assert!((erosion.threshold - 0.5).abs() < 1e-5 && erosion.edge == 0.1);
        assert!((mesh.position - Vec3::new(1.0, 1.0, 0.0)).length() < 1e-5, "{:?}", mesh.position);
        // Its +Z turns with the placement.
        assert!((mesh.rotation * Vec3::Z - Vec3::X).length() < 1e-5);
    }

    #[test]
    fn a_flipbook_turns_by_life_or_by_rate() {
        let by_life = library(&format!(
            "(shape: Card, lifetime: 1.6, {WHITE}, pattern: Burst, flipbook: Some((columns: 4, rows: 4, frames: 16)))"
        ));
        let book = layer(&by_life);
        let frame = |age: f32| {
            book.resolve(&Placement::default(), age).and_then(|mesh| mesh.flipbook).expect("frames")
        };
        for index in 0..16 {
            let at = frame((index as f32 + 0.5) * 0.1);
            assert_eq!((at.frame, at.blend), (index, 0.0), "steps, unblended");
        }
        let by_rate = library(&format!(
            "(shape: Card, lifetime: 10.0, {WHITE}, pattern: Burst, flipbook: Some((columns: 4, rows: 4, frames: 4, \
             timing: PerSecond(rate: 8.0, looped: true), blend: true)))"
        ));
        let book = layer(&by_rate);
        let at = |age: f32| {
            book.resolve(&Placement::default(), age).and_then(|mesh| mesh.flipbook).expect("frames")
        };
        // 5.5 frames in: frame 1 of 4 again, half way to 2.
        let middle = at(5.5 / 8.0);
        assert_eq!((middle.frame, middle.next), (1, 2));
        assert!((middle.blend - 0.5).abs() < 1e-4, "{}", middle.blend);
        let wrap = at(3.5 / 8.0);
        assert_eq!((wrap.frame, wrap.next), (3, 0));
    }

    fn world(layers: &str, emitters: &str, budget: VfxBudget) -> VfxWorld {
        VfxWorld::new(EffectLibrary::from_ron(&document(emitters, layers)).expect("valid"), budget, 7)
    }

    fn step(world: &mut VfxWorld, seconds: f32) -> VfxFrame {
        world.advance(&OwnerSteps { world: seconds, owners: &[] });
        let mut frame = VfxFrame::default();
        world.write_frame(&mut frame, 1.0);
        frame
    }

    /// A layer holds its effect until it has played, even past a stop, and an effect of layers alone ends
    /// when they have.
    #[test]
    fn layers_hold_their_effect_until_they_have_played() {
        let mut world = world(&format!("(shape: Ring, lifetime: 0.5, {WHITE})"), "", VfxBudget::DESKTOP_LOW);
        let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).expect("spawned");
        world.stop(handle);
        assert_eq!(step(&mut world, 0.25).meshes.len(), 1);
        assert!(world.is_playing(handle));
        let frame = step(&mut world, 0.3);
        assert!(frame.meshes.is_empty() && !world.is_playing(handle), "{:?}", frame.stats);
    }

    /// Adding a layer to an effect moves none of its particles: a layer takes no random numbers.
    #[test]
    fn a_layer_moves_no_spark() {
        let ring = format!("(shape: Ring, lifetime: 0.5, {WHITE})");
        let frames = |layers: &str| {
            let mut world = world(layers, SPARKS, VfxBudget::DESKTOP_LOW);
            world.spawn(EffectId(0), OwnerId::WORLD, Placement::default());
            (0..4).map(|_| step(&mut world, 1.0 / 60.0).sprites).collect::<Vec<_>>()
        };
        assert_eq!(frames(""), frames(&ring));
    }

    /// Past the budget, the highest priority draw, and of equal priority the oldest effects'; the rest are
    /// counted.
    #[test]
    fn past_the_budget_the_lowest_priority_go_first() {
        let layers = format!(
            "(shape: Ring, lifetime: 1.0, {WHITE}, priority: 1), (shape: Dome, lifetime: 1.0, {WHITE}), \
             (shape: Column, lifetime: 1.0, {WHITE}, priority: 2)"
        );
        let budget = VfxBudget { max_meshes: 4, ..VfxBudget::DESKTOP_LOW };
        let mut world = world(&layers, "", budget);
        for x in 0..3 {
            world.spawn(EffectId(0), OwnerId::WORLD, Placement::at(Vec3::X * x as f32));
        }
        let frame = step(&mut world, 0.1);
        let shapes: Vec<_> = frame.meshes.iter().map(|mesh| (mesh.shape, mesh.position.x as i32)).collect();
        // Three columns (priority 2), then the oldest effect's ring (1); no dome (0).
        assert_eq!(
            shapes,
            [
                (VfxMeshShape::Ring, 0),
                (VfxMeshShape::Column, 0),
                (VfxMeshShape::Column, 1),
                (VfxMeshShape::Column, 2)
            ]
        );
        assert_eq!((frame.stats.meshes, frame.stats.dropped_meshes), (4, 5));
    }

    /// An owner in hitstop holds its layers exactly where they are.
    #[test]
    fn hitstop_holds_a_layer() {
        let layers = format!("(shape: SlashArc, lifetime: 1.0, scroll: (2.0, 0.0), {WHITE})");
        let mut world = world(&layers, "", VfxBudget::DESKTOP_LOW);
        let fighter = OwnerId(9);
        world.spawn(EffectId(0), fighter, Placement::default());
        let mut frame = VfxFrame::default();
        world.advance(&OwnerSteps { world: 0.1, owners: &[(fighter, 0.1)] });
        world.write_frame(&mut frame, 1.0);
        let before = frame.meshes[0];
        world.advance(&OwnerSteps { world: 0.1, owners: &[(fighter, 0.0)] });
        world.write_frame(&mut frame, 0.5);
        assert_eq!(frame.meshes[0], before);
    }
}
