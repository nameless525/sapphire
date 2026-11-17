//! SDF face maps: one threshold map from a sequence of threshold masks.
//!
//! Anime faces aren't shaded by their normals. An artist paints the face's lit region at a
//! series of light angles, front to side (or bakes a start from Blender: see
//! `docs/blender-mask-bake.md`), and the face shader thresholds a single map against the
//! light's angle instead: each texel holds the angle at which it turns to shadow
//! (`sapphire_assets::character_maps`). Between two consecutive masks the generator places
//! that angle by signed-distance interpolation, as nagakagachi's SDF transition blending and
//! akasaki1211's tool do (research notes, character rendering §3.1):
//!
//! - A texel lit by mask *i* (at angle *aᵢ*) and in shadow in mask *i + 1* lies in the band
//!   between the two masks' edges. With *d₀* its distance to mask *i*'s edge and *d₁* its
//!   distance to mask *i + 1*'s lit region, it turns at `aᵢ + (aᵢ₊₁ − aᵢ) · d₀ / (d₀ + d₁)`:
//!   at mask *i*'s angle where it touches mask *i*'s edge, at mask *i + 1*'s where it touches
//!   that one, in proportion between. The map's iso-lines sweep smoothly from one mask's edge
//!   to the next, and the masks may sit at any angles, not only equal steps.
//! - Distances are exact Euclidean distances (Felzenszwalb and Huttenlocher's transform)
//!   from the texel's center to the nearest texel on the other side, less half a texel: the
//!   edge between a lit and a shadowed texel lies halfway between their centers.
//! - Texels in shadow in the first mask hold 0 (never lit), and texels lit in every mask hold
//!   1 (lit until the light is behind the head): before the first mask's angle and after the
//!   last one's, the nearest mask's shape holds.
//!
//! Thresholded at a mask's angle, the map reproduces that mask exactly: its lit texels hold
//! values above the angle's, its shadowed ones values at or below it. The one exception is a
//! mask at 180°, the top of the range: a texel lit there holds 1, which reads as shadow with
//! the light exactly behind. The cooked format rounds values: at 16 bits (desktop) that
//! never moves an edge visibly, while 8 bits (mobile) can flip texels right along an edge
//! when two masks are close in angle but their edges are far apart.
//!
//! The horizontal masks make the R channel. A document may add vertical masks for a light
//! rising toward overhead (WuWa's top light), which make the G channel of a two-channel map.
//!
//! # Monotonic masks
//!
//! One threshold per texel describes only a lit region that shrinks as the light turns: once
//! in shadow, a texel stays in shadow. A later mask that lights texels an earlier mask
//! shadows can't be represented, so the generator checks every consecutive pair and fails,
//! naming both masks, how many texels break the rule and where, unless the document allows
//! that many per pair (`allowed_violations`, for stray anti-aliased pixels). Allowed
//! violations follow the first mask that shadows them, so the map then reproduces each mask's
//! overlap with the masks before it.
//!
//! # Masks
//!
//! A mask is lit where its first channel (gray, or red) is at least one half. How its edges
//! are read is the document's `edges` ([`Edges`]):
//!
//! - `Binary`, the default: anti-aliased edges are binarized at one half, and an edge lies
//!   halfway between a lit and a shadowed texel, so a mask's resolution sets its edge's
//!   precision.
//! - `AntiAliased` (M6 C6): a gray texel along an edge places the edge inside it, by how much of
//!   it is lit, through Gustavson and Strand's anti-aliased distance transform (`distance`'s
//!   `anti_aliased_distances`). The same texels are lit and turn between the same masks as
//!   binary's, so each mask still holds at its angle texel for texel as generated; what moves
//!   is where between texels the map's values cross each angle, which is where the shader's
//!   filtered read puts the shadow's edge. A texel the edge runs right through holds a value
//!   just above its mask's, which the cook's rounding can take to the mask's own, so it turns
//!   at that angle: half lit, it is right either way. Measured on masks painted at 512² against
//!   the same shapes at 2048², the edge's mean error falls from about 0.12 texels to about 0.02
//!   (`anti_aliased_masks_place_edges_closer_to_a_fine_reference`). A mask painted with hard
//!   edges gains nothing from it.
//!
//! Leave everything outside the face's UV islands in shadow in every mask: a mask with no shadow
//! gives the band after it no edge to sweep from (as one with nothing lit gives the band
//! before it no edge to sweep to), so that band turns all at once, at the later mask's
//! angle.

use std::fmt;

use bevy_reflect::TypePath;
use sapphire_assets::character_maps::face_sdf::{MAX_ANGLE_DEGREES, encode_angle};
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use super::distance::{anti_aliased_distances, squared_distances};
use super::{GeneratedMap, Inputs, MapError, check_size, with_image_importer};
use crate::cache::Stage;
use crate::import::image::{ChannelLayout, SourceImage};
use crate::texture::{SdfChannels, SdfPrecision, SdfSettings, TextureClass};

/// This importer, as cache keys know it: reading the document and its masks, and generating
/// the threshold map. Bump the version whenever the map changes for the same inputs.
pub const IMPORTER: Stage = Stage { name: "face sdf", version: with_image_importer(1) };

/// A `.facesdf.ron` document: a face's threshold masks, per channel, in increasing angle
/// order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct FaceSdfMap {
    /// The R channel: a light turning from head-forward (0°) toward head-right (90°).
    pub horizontal: Vec<ThresholdMask>,
    /// The G channel, if any: a light rising from head-forward (0°) toward overhead (90°).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vertical: Vec<ThresholdMask>,
    /// Texels per pair of consecutive masks that may break monotonicity (see the
    /// [module docs](self)). 0 by default.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub allowed_violations: u32,
    /// How the masks' edges are read (M6 C6): [`Edges::Binary`] by default.
    #[serde(default, skip_serializing_if = "Edges::is_binary")]
    pub edges: Edges,
}

/// A document from before `edges` reads as one with `edges: Binary`, the same map, so the field
/// needs no new version.
impl Versioned for FaceSdfMap {
    const VERSION: u32 = 1;
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// How a face map's masks' edges are read (M6 C6): see the [module docs](self)' "Masks".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edges {
    /// Each mask is lit where it is at least one half, and its edges lie halfway between texels:
    /// the masks' resolution sets the edges' precision.
    #[default]
    Binary,
    /// A mask's gray texels along an edge place the edge within them, by how much of the texel is
    /// lit: sub-texel edges from anti-aliased masks.
    AntiAliased,
}

impl Edges {
    fn is_binary(&self) -> bool {
        *self == Self::Binary
    }
}

/// One threshold mask: the face's lit region for a light at `angle`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThresholdMask {
    /// The light's angle from head-forward, in degrees, 0 to 180.
    pub angle: f32,
    /// The mask image, relative to the document.
    pub path: String,
}

impl FaceSdfMap {
    /// Every mask path, horizontal then vertical.
    pub fn inputs(&self) -> Vec<String> {
        self.horizontal.iter().chain(&self.vertical).map(|mask| mask.path.clone()).collect()
    }

    /// The masks of each channel the map has, with the channel.
    fn channels(&self) -> impl Iterator<Item = (SdfChannel, &[ThresholdMask])> {
        let vertical = (!self.vertical.is_empty()).then_some((SdfChannel::Vertical, &self.vertical[..]));
        [(SdfChannel::Horizontal, &self.horizontal[..])].into_iter().chain(vertical)
    }

    /// Checks the mask lists: at least two per channel, at angles that increase within
    /// `[0°, 180°]`.
    pub fn validate(&self) -> Result<(), FaceSdfError> {
        if self.horizontal.len() < 2 {
            return Err(FaceSdfError::TooFewMasks {
                channel: SdfChannel::Horizontal,
                count: self.horizontal.len(),
            });
        }
        if self.vertical.len() == 1 {
            return Err(FaceSdfError::TooFewMasks { channel: SdfChannel::Vertical, count: 1 });
        }
        for (channel, masks) in self.channels() {
            for (index, mask) in masks.iter().enumerate() {
                if !(0.0..=MAX_ANGLE_DEGREES).contains(&mask.angle) {
                    return Err(FaceSdfError::AngleRange { channel, mask: mask.clone() });
                }
                if let Some(previous) = index.checked_sub(1).map(|i| &masks[i])
                    && mask.angle <= previous.angle
                {
                    return Err(FaceSdfError::AngleOrder {
                        channel,
                        previous: previous.clone(),
                        mask: mask.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Import settings for face SDF documents: how the map cooks. Its channel count is the
/// document's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FaceSdfSettings {
    pub mips: bool,
    /// The desktop cook's precision; the mobile cook is always 8-bit.
    pub precision: SdfPrecision,
}

impl Default for FaceSdfSettings {
    fn default() -> Self {
        let SdfSettings { mips, precision, .. } = SdfSettings::default();
        Self { mips, precision }
    }
}

/// A channel of a face map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdfChannel {
    Horizontal,
    Vertical,
}

impl fmt::Display for SdfChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        })
    }
}

/// Texels a mask lights though the mask before it shadows them.
#[derive(Clone, Debug, PartialEq)]
pub struct Violation {
    pub earlier: ThresholdMask,
    pub later: ThresholdMask,
    /// How many texels break monotonicity.
    pub texels: u64,
    /// Their bounding box, in texels from the top left, inclusive.
    pub min: [u32; 2],
    pub max: [u32; 2],
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { earlier, later, texels, min, max } = self;
        write!(
            f,
            "{} ({}°) lights {texels} texel(s) that {} ({}°) shadows, within x {}..={}, y {}..={}",
            later.path, later.angle, earlier.path, earlier.angle, min[0], max[0], min[1], max[1]
        )
    }
}

/// Errors in a face SDF document's masks.
#[derive(Debug, thiserror::Error)]
pub enum FaceSdfError {
    #[error("the {channel} channel has {count} mask(s); a face map needs at least two per channel")]
    TooFewMasks { channel: SdfChannel, count: usize },
    #[error("{channel} mask {} is at {}°; angles go from 0° (head-forward) to 180°", mask.path, mask.angle)]
    AngleRange { channel: SdfChannel, mask: ThresholdMask },
    #[error(
        "{channel} mask {} at {}° doesn't come after {} at {}°: list masks by increasing angle",
        mask.path, mask.angle, previous.path, previous.angle
    )]
    AngleOrder { channel: SdfChannel, previous: ThresholdMask, mask: ThresholdMask },
    #[error(
        "the {channel} masks must only lose lit texels as the light turns, but{}\n(the document allows {allowed} texel(s) per pair: see `allowed_violations`)",
        list(violations)
    )]
    NotMonotonic { channel: SdfChannel, allowed: u32, violations: Vec<Violation> },
}

fn list(violations: &[Violation]) -> String {
    violations.iter().map(|violation| format!("\n  - {violation}")).collect()
}

/// Reads the masks `map` names and generates its threshold map.
pub(super) fn generate(
    map: &FaceSdfMap,
    settings: &FaceSdfSettings,
    inputs: &Inputs<'_>,
) -> Result<GeneratedMap, MapError> {
    map.validate()?;
    let mut size = None;
    let mut values = Vec::new();
    for (channel, masks) in map.channels() {
        values.push(threshold_channel(channel, masks, map, inputs, &mut size)?);
    }
    let [width, height] = size.expect("validated: every channel has masks");
    let (layout, pixels, channels) = match &values[..] {
        [horizontal] => {
            (ChannelLayout::Gray, horizontal.iter().map(|&h| [h, h, h, 1.0]).collect(), SdfChannels::One)
        }
        [horizontal, vertical] => (
            // Gray + alpha: the two logical channels the SDF class takes.
            ChannelLayout::GrayAlpha,
            horizontal.iter().zip(vertical).map(|(&h, &v)| [h, h, h, v]).collect(),
            SdfChannels::Two,
        ),
        _ => unreachable!("a face map has one or two channels"),
    };
    let image = SourceImage::new(width, height, layout, pixels).expect("one value per texel");
    let class =
        TextureClass::Sdf(SdfSettings { mips: settings.mips, channels, precision: settings.precision });
    Ok(GeneratedMap { image, class })
}

/// One channel's threshold values, from its masks (`map`'s, which says how to read them).
/// `size` is the map's size, set by the first mask read.
fn threshold_channel(
    channel: SdfChannel,
    masks: &[ThresholdMask],
    map: &FaceSdfMap,
    inputs: &Inputs<'_>,
    size: &mut Option<[u32; 2]>,
) -> Result<Vec<f32>, MapError> {
    let allowed = map.allowed_violations;
    let mut builder: Option<ChannelBuilder> = None;
    // One mask in memory at a time: a face map can have dozens.
    for mask in masks {
        let (path, image) = inputs.image(&mask.path)?;
        let expected = *size.get_or_insert([image.width, image.height]);
        check_size(&path, &image, expected)?;
        let builder = builder.get_or_insert_with(|| ChannelBuilder::new(expected[0], expected[1], map.edges));
        builder.push(mask.angle, image.pixels.iter().map(|pixel| pixel[0]).collect());
    }
    let (values, pairs) = builder.expect("validated: at least two masks").finish();
    let violations: Vec<Violation> = pairs
        .into_iter()
        .filter(|pair| pair.texels > u64::from(allowed))
        .map(|pair| Violation {
            earlier: masks[pair.later - 1].clone(),
            later: masks[pair.later].clone(),
            texels: pair.texels,
            min: pair.min,
            max: pair.max,
        })
        .collect();
    if violations.is_empty() {
        Ok(values)
    } else {
        Err(FaceSdfError::NotMonotonic { channel, allowed, violations }.into())
    }
}

/// Texels mask `later` lights that mask `later - 1` shadows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PairViolation {
    later: usize,
    texels: u64,
    min: [u32; 2],
    max: [u32; 2],
}

/// Builds one channel's threshold values from its masks, fed in increasing angle order.
struct ChannelBuilder {
    width: u32,
    height: u32,
    edges: Edges,
    /// Every texel's value so far: set when the texel turns to shadow, or at the end.
    values: Vec<f32>,
    /// The masks fed so far.
    count: usize,
    /// The last mask as given, for the monotonicity check.
    last: Vec<bool>,
    /// Texels lit in every mask so far: the last mask, monotonic by construction.
    lit: Vec<bool>,
    /// With anti-aliased edges, how much of each texel every mask so far lights: the least of
    /// their values. Empty with binary edges.
    cover: Vec<f32>,
    /// The last mask's stored angle value.
    last_value: f32,
    violations: Vec<PairViolation>,
}

impl ChannelBuilder {
    fn new(width: u32, height: u32, edges: Edges) -> Self {
        let texels = width as usize * height as usize;
        let cover = match edges {
            Edges::Binary => Vec::new(),
            Edges::AntiAliased => vec![1.0; texels],
        };
        Self {
            width,
            height,
            edges,
            values: vec![0.0; texels],
            count: 0,
            last: Vec::new(),
            lit: vec![true; texels],
            cover,
            last_value: 0.0,
            violations: Vec::new(),
        }
    }

    /// Adds the mask for a light at `degrees`: each texel's value, row-major, lit from one half.
    fn push(&mut self, degrees: f32, mask: Vec<f32>) {
        let value = encode_angle(degrees);
        let (width, height) = (self.width as usize, self.height as usize);
        let lit_now: Vec<bool> = mask.iter().map(|&v| v >= 0.5).collect();
        if self.count > 0 {
            self.check_pair(&lit_now);
        }
        let next: Vec<bool> = self.lit.iter().zip(&lit_now).map(|(&lit, &now)| lit && now).collect();
        // A NaN texel is in shadow, as it is binarized.
        let next_cover: Vec<f32> = self
            .cover
            .iter()
            .zip(&mask)
            .map(|(&cover, &v)| cover.min(if v.is_nan() { 0.0 } else { v }))
            .collect();
        let turned = |i: usize| self.lit[i] && !next[i];
        if self.count == 0 {
            // Shadowed by the first mask: never lit. The values are already 0.
        } else if (0..next.len()).any(turned) {
            // Distance to the previous lit region's edge, and to this one's lit region.
            let (to_last, to_next): (Vec<f64>, Vec<f64>) = match self.edges {
                Edges::Binary => (
                    squared_distances(width, height, |i| !self.lit[i])
                        .into_iter()
                        .map(edge_distance)
                        .collect(),
                    squared_distances(width, height, |i| next[i]).into_iter().map(edge_distance).collect(),
                ),
                Edges::AntiAliased => {
                    let shadow: Vec<f32> =
                        self.cover.iter().map(|&cover| 1.0 - cover.clamp(0.0, 1.0)).collect();
                    let lit: Vec<f32> = next_cover.iter().map(|&cover| cover.clamp(0.0, 1.0)).collect();
                    (
                        anti_aliased_distances(width, height, &shadow),
                        anti_aliased_distances(width, height, &lit),
                    )
                }
            };
            // No shadow in the last mask, or nothing lit in this one: no edge to sweep from or
            // to, so the band turns at once, at this mask's angle. Gray texels under one half
            // make no edge, as they are shadow binarized.
            let edges = self.lit.contains(&false) && next.contains(&true);
            let (low, high) = (self.last_value, value);
            let span = f64::from(high) - f64::from(low);
            for i in (0..next.len()).filter(|&i| turned(i)) {
                let t = if edges { to_last[i] / (to_last[i] + to_next[i]) } else { 1.0 };
                // Strictly above the last mask's value, so the texel is still lit at that
                // mask's angle even where `t` is too small for f32 to show.
                self.values[i] = ((f64::from(low) + t * span) as f32).max(low.next_up()).min(high);
            }
        }
        self.lit = next;
        self.cover = next_cover;
        self.last = lit_now;
        self.last_value = value;
        self.count += 1;
    }

    /// Records the texels `mask` lights that the last mask shadows.
    fn check_pair(&mut self, mask: &[bool]) {
        let mut pair = PairViolation { later: self.count, texels: 0, min: [u32::MAX; 2], max: [0; 2] };
        for (i, (&now, &before)) in mask.iter().zip(&self.last).enumerate() {
            if now && !before {
                let at = [(i % self.width as usize) as u32, (i / self.width as usize) as u32];
                pair.texels += 1;
                pair.min = [pair.min[0].min(at[0]), pair.min[1].min(at[1])];
                pair.max = [pair.max[0].max(at[0]), pair.max[1].max(at[1])];
            }
        }
        if pair.texels > 0 {
            self.violations.push(pair);
        }
    }

    /// The channel's values, and every pair of masks that broke monotonicity.
    fn finish(mut self) -> (Vec<f32>, Vec<PairViolation>) {
        for (value, _) in self.values.iter_mut().zip(&self.lit).filter(|(_, lit)| **lit) {
            *value = 1.0;
        }
        (self.values, self.violations)
    }
}

/// The distance from a texel's center to an edge whose nearest texel on the other side is
/// `squared` texels² away: the edge lies halfway between the two centers.
fn edge_distance(squared: f64) -> f64 {
    squared.sqrt() - 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    use sapphire_assets::character_maps::face_sdf::is_lit;

    /// A test mask: which texels of a grid are lit.
    #[derive(Clone)]
    struct Mask {
        width: u32,
        height: u32,
        lit: Vec<bool>,
    }

    impl Mask {
        fn new(width: u32, height: u32, lit: impl Fn(u32, u32) -> bool) -> Self {
            let lit =
                (0..height).flat_map(|y| (0..width).map(move |x| (x, y))).map(|(x, y)| lit(x, y)).collect();
            Self { width, height, lit }
        }

        fn at(&self, x: u32, y: u32) -> bool {
            self.lit[(y * self.width + x) as usize]
        }
    }

    /// Builds a channel from masks at their angles in degrees.
    fn build(masks: &[(f32, Mask)]) -> (Vec<f32>, Vec<PairViolation>) {
        let mut builder = ChannelBuilder::new(masks[0].1.width, masks[0].1.height, Edges::Binary);
        for (degrees, mask) in masks {
            builder.push(*degrees, mask.lit.iter().map(|&lit| if lit { 1.0 } else { 0.0 }).collect());
        }
        builder.finish()
    }

    /// Texels where `values` thresholded at `degrees` differ from `mask`, and how many of those
    /// have no neighbor across `mask`'s edge, so are more than a texel from it.
    fn mismatches(values: &[f32], degrees: f32, mask: &Mask) -> (usize, usize) {
        let (mut wrong, mut far) = (0, 0);
        for y in 0..mask.height {
            for x in 0..mask.width {
                if is_lit(values[(y * mask.width + x) as usize], degrees) == mask.at(x, y) {
                    continue;
                }
                wrong += 1;
                let neighbors =
                    [(-1, 0), (1, 0), (0, -1), (0, 1)].map(|(dx, dy)| (i64::from(x) + dx, i64::from(y) + dy));
                let near_edge = neighbors.iter().any(|&(nx, ny)| {
                    (0..i64::from(mask.width)).contains(&nx)
                        && (0..i64::from(mask.height)).contains(&ny)
                        && mask.at(nx as u32, ny as u32) != mask.at(x, y)
                });
                if !near_edge {
                    far += 1;
                }
            }
        }
        (wrong, far)
    }

    #[test]
    fn a_moving_edge_sweeps_linearly_between_masks() {
        // Lit left of an edge that moves left as the light turns: 0° lights x < 60, 30° x < 40,
        // 90° x < 10 (an uneven step, which equal-interval tools can't express).
        let edge = |edge: u32| Mask::new(64, 8, move |x, _| x < edge);
        let masks = [(0.0, edge(60)), (30.0, edge(40)), (90.0, edge(10))];
        let (values, violations) = build(&masks);
        assert!(violations.is_empty());
        // Thresholded at each mask's angle, the map is that mask, exactly.
        for (degrees, mask) in &masks {
            assert_eq!(mismatches(&values, *degrees, mask), (0, 0), "{degrees}°");
        }
        // Between masks, the edge moves linearly: halfway in angle is halfway in position.
        for (degrees, position) in [(15.0, 50), (60.0, 25), (7.5, 55)] {
            let (wrong, far) = mismatches(&values, degrees, &edge(position));
            assert_eq!(far, 0, "{degrees}°: {wrong} texels off, some by more than a texel");
        }
        // Texel by texel, values fall in proportion: x = 40..60 spans 30° down to 0°.
        for x in 40..60 {
            let expected = (60.0 - f64::from(x) - 0.5) / 20.0 * 30.0 / 180.0;
            assert!((f64::from(values[x as usize]) - expected).abs() < 1e-6, "x {x}: {}", values[x as usize]);
        }
        assert_eq!(values[0], 1.0, "lit in every mask");
        assert_eq!(values[63], 0.0, "shadowed in the first mask");
    }

    #[test]
    fn nested_circles_give_circular_iso_lines() {
        let disc = |radius: f64| {
            Mask::new(96, 96, move |x, y| (f64::from(x) - 47.5).hypot(f64::from(y) - 47.5) < radius)
        };
        let radii = [(0.0, 44.0), (20.0, 36.0), (45.0, 24.0), (80.0, 8.0)];
        let masks = radii.map(|(degrees, radius)| (degrees, disc(radius)));
        let (values, violations) = build(&masks);
        assert!(violations.is_empty());
        for (degrees, mask) in &masks {
            assert_eq!(mismatches(&values, *degrees, mask).0, 0, "{degrees}°");
        }
        // Halfway between two masks' angles, the lit region is the disc halfway between their
        // radii, to within a texel of its edge.
        for window in radii.windows(2) {
            let ((a0, r0), (a1, r1)) = (window[0], window[1]);
            let (wrong, far) = mismatches(&values, (a0 + a1) / 2.0, &disc((r0 + r1) / 2.0));
            assert_eq!(far, 0, "{}°: {wrong} texels off", (a0 + a1) / 2.0);
        }
    }

    #[test]
    fn masks_that_light_texels_again_are_violations() {
        // The 45° mask lights a 2×3 blob at (12..14, 4..7) that the 30° mask shadows.
        let blob = |x: u32, y: u32| (12..14).contains(&x) && (4..7).contains(&y);
        let masks = [
            (0.0, Mask::new(16, 16, |x, _| x < 14)),
            (30.0, Mask::new(16, 16, |x, _| x < 10)),
            (45.0, Mask::new(16, 16, |x, y| x < 8 || blob(x, y))),
        ];
        let (values, violations) = build(&masks);
        assert_eq!(violations, [PairViolation { later: 2, texels: 6, min: [12, 4], max: [13, 6] }]);
        // Allowed violations follow the first mask that shadowed them: the blob stays dark.
        assert!(!is_lit(values[5 * 16 + 12], 40.0));
        assert!((0..16).all(|y| (10..14).all(|x| values[y * 16 + x] < 30.0 / 180.0)));
    }

    #[test]
    fn masks_without_edges_switch_at_once() {
        // Fully lit at 0°: the shadow of the 60° mask arrives all at once at 60°.
        let (values, _) =
            build(&[(0.0, Mask::new(8, 2, |_, _| true)), (60.0, Mask::new(8, 2, |x, _| x < 4))]);
        assert_eq!(&values[..8], &[1.0, 1.0, 1.0, 1.0, 1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]);
        // Fully dark at 90°: the lit side stays lit until 90°.
        let (values, _) =
            build(&[(30.0, Mask::new(8, 1, |x, _| x < 4)), (90.0, Mask::new(8, 1, |_, _| false))]);
        assert_eq!(values, [0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0]);
        // Either way, both masks hold at their angles.
        assert!(is_lit(values[0], 30.0) && !is_lit(values[0], 90.0));
    }

    /// Builds a channel from masks given as values, read with `edges`.
    fn build_values(width: u32, height: u32, edges: Edges, masks: &[(f32, Vec<f32>)]) -> Vec<f32> {
        let mut builder = ChannelBuilder::new(width, height, edges);
        for (degrees, mask) in masks {
            builder.push(*degrees, mask.clone());
        }
        let (values, violations) = builder.finish();
        assert!(violations.is_empty());
        values
    }

    /// The anti-aliased edge's measurement (M6 C6): masks painted at 512² and anti-aliased against
    /// the same shapes' binary masks at 2048², the face map's edge from each, as the shader reads it
    /// (filtered bilinearly up to the reference's texels), against the reference map's, at every
    /// mask's angle between the first and the last and halfway between each pair. The error is
    /// the area the two lit regions differ by over the edge's length: the edge's mean distance
    /// from the reference's, in the 512² map's texels. `Binary` reads the same 512² masks, gray
    /// texels and all, as artists' anti-aliased masks are read without `AntiAliased`.
    ///
    /// Measured (M6 C6, the M5 Mac, any load: the numbers are exact): a band whose wavy edge
    /// sweeps across, 0.1200 texels binary against 0.0203 anti-aliased; discs that shrink,
    /// 0.1296 against 0.0202. The test holds anti-aliased edges to under a quarter of binary's
    /// error and under 0.05 texels.
    #[test]
    fn anti_aliased_masks_place_edges_closer_to_a_fine_reference() {
        const FINE: u32 = 2048;
        const SCALE: u32 = 4;
        const COARSE: u32 = FINE / SCALE;
        // Each case: its lit region at an angle (at a point in [0, 1]², y down), the masks'
        // angles, and its edge's length at an angle, in units of the square's side.
        struct Case {
            name: &'static str,
            lit: fn(f32, f64, f64) -> bool,
            angles: [f32; 5],
            length: fn(f32) -> f64,
        }
        // A wavy edge moving left as the light turns, at every slope up to about 0.8.
        fn band_edge(degrees: f32, v: f64) -> f64 {
            0.75 - 0.5 * f64::from(degrees) / 120.0 + 0.08 * (3.0 * std::f64::consts::PI * v).sin() + 0.05 * v
        }
        fn band_length(_: f32) -> f64 {
            // The edge's shape doesn't change with the angle: its length, by the midpoint rule.
            let steps = 4096;
            (0..steps)
                .map(|i| {
                    let v = (f64::from(i) + 0.5) / f64::from(steps);
                    let slope = 0.24 * std::f64::consts::PI * (3.0 * std::f64::consts::PI * v).cos() + 0.05;
                    slope.hypot(1.0) / f64::from(steps)
                })
                .sum()
        }
        fn disc_radius(degrees: f32) -> f64 {
            0.45 - 0.35 * f64::from(degrees) / 120.0
        }
        let cases = [
            Case {
                name: "band",
                lit: |degrees, u, v| u < band_edge(degrees, v),
                angles: [0.0, 25.0, 60.0, 90.0, 120.0],
                length: band_length,
            },
            Case {
                name: "discs",
                lit: |degrees, u, v| (u - 0.5).hypot(v - 0.5) < disc_radius(degrees),
                angles: [0.0, 30.0, 50.0, 85.0, 120.0],
                length: |degrees| 2.0 * std::f64::consts::PI * disc_radius(degrees),
            },
        ];
        for case in cases {
            let fine_mask = |degrees: f32| -> Vec<f32> {
                (0..FINE * FINE)
                    .map(|i| {
                        let center = |c: u32| (f64::from(c) + 0.5) / f64::from(FINE);
                        if (case.lit)(degrees, center(i % FINE), center(i / FINE)) { 1.0 } else { 0.0 }
                    })
                    .collect()
            };
            let mut fine = Vec::new();
            let mut coarse = Vec::new();
            for degrees in case.angles {
                let mask = fine_mask(degrees);
                // The fine mask's coverage of each coarse texel, in 8 bits as a PNG holds it.
                let cover = (0..COARSE * COARSE)
                    .map(|i| {
                        let (x, y) = (i % COARSE * SCALE, i / COARSE * SCALE);
                        let lit: f32 = (0..SCALE)
                            .flat_map(|dy| (0..SCALE).map(move |dx| ((y + dy) * FINE + x + dx) as usize))
                            .map(|j| mask[j])
                            .sum();
                        (lit / (SCALE * SCALE) as f32 * 255.0).round() / 255.0
                    })
                    .collect();
                fine.push((degrees, mask));
                coarse.push((degrees, cover));
            }
            let reference = build_values(FINE, FINE, Edges::Binary, &fine);
            let binary = build_values(COARSE, COARSE, Edges::Binary, &coarse);
            let anti_aliased = build_values(COARSE, COARSE, Edges::AntiAliased, &coarse);
            let interior = &case.angles[1..case.angles.len() - 1];
            let halfway = case.angles.windows(2).map(|pair| (pair[0] + pair[1]) / 2.0);
            let angles: Vec<f32> = interior.iter().copied().chain(halfway).collect();
            let error = |map: &[f32]| -> f64 {
                let total: f64 = angles
                    .iter()
                    .map(|&degrees| {
                        let wrong = (0..FINE * FINE)
                            .filter(|&i| {
                                let sampled = bilinear(map, COARSE, i % FINE, i / FINE, SCALE);
                                is_lit(sampled, degrees) != is_lit(reference[i as usize], degrees)
                            })
                            .count();
                        wrong as f64 / ((case.length)(degrees) * f64::from(FINE)) / f64::from(SCALE)
                    })
                    .sum();
                total / angles.len() as f64
            };
            let (binary, anti_aliased) = (error(&binary), error(&anti_aliased));
            eprintln!("{}: binary {binary:.4} texels, anti-aliased {anti_aliased:.4}", case.name);
            assert!(
                anti_aliased < 0.25 * binary && anti_aliased < 0.05,
                "{}: {anti_aliased} vs {binary}",
                case.name
            );
        }
    }

    /// `map` (`size`² texels) sampled bilinearly, clamped at its edges, at the center of pixel
    /// `(x, y)` of a grid `scale` times finer, as a GPU's linear filter samples it.
    fn bilinear(map: &[f32], size: u32, x: u32, y: u32, scale: u32) -> f32 {
        let at = |c: u32| (f64::from(c) + 0.5) / f64::from(scale) - 0.5;
        let (sx, sy) = (at(x), at(y));
        let (x0, y0) = (sx.floor(), sy.floor());
        let (fx, fy) = ((sx - x0) as f32, (sy - y0) as f32);
        let texel = |tx: f64, ty: f64| {
            let clamp = |c: f64| c.clamp(0.0, f64::from(size - 1)) as usize;
            map[clamp(ty) * size as usize + clamp(tx)]
        };
        let top = texel(x0, y0) * (1.0 - fx) + texel(x0 + 1.0, y0) * fx;
        let bottom = texel(x0, y0 + 1.0) * (1.0 - fx) + texel(x0 + 1.0, y0 + 1.0) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    #[test]
    fn generation_is_deterministic() {
        let masks = [
            (0.0, Mask::new(40, 40, |x, y| x * x + y < 900)),
            (40.0, Mask::new(40, 40, |x, y| x * x + 2 * y < 700)),
            (100.0, Mask::new(40, 40, |x, y| x + y < 20)),
        ];
        assert_eq!(build(&masks), build(&masks));
    }

    #[test]
    fn documents_validate_their_mask_lists() {
        let mask = |angle: f32| ThresholdMask { angle, path: format!("h_{angle}.png") };
        let map = |horizontal: Vec<ThresholdMask>, vertical: Vec<ThresholdMask>| FaceSdfMap {
            horizontal,
            vertical,
            allowed_violations: 0,
            edges: Edges::Binary,
        };
        assert!(map(vec![mask(0.0), mask(90.0)], Vec::new()).validate().is_ok());
        assert!(map(vec![mask(0.0), mask(180.0)], vec![mask(0.0), mask(45.0)]).validate().is_ok());
        assert!(matches!(
            map(vec![mask(0.0)], Vec::new()).validate(),
            Err(FaceSdfError::TooFewMasks { channel: SdfChannel::Horizontal, count: 1 })
        ));
        assert!(matches!(
            map(vec![mask(0.0), mask(90.0)], vec![mask(10.0)]).validate(),
            Err(FaceSdfError::TooFewMasks { channel: SdfChannel::Vertical, count: 1 })
        ));
        let error = map(vec![mask(0.0), mask(30.0), mask(30.0)], Vec::new()).validate().unwrap_err();
        assert!(matches!(error, FaceSdfError::AngleOrder { .. }), "{error:?}");
        assert!(error.to_string().contains("h_30.png at 30° doesn't come after h_30.png"), "{error}");
        for bad in [-1.0, 181.0, f32::NAN] {
            let error = map(vec![mask(0.0), mask(bad)], Vec::new()).validate().unwrap_err();
            assert!(matches!(error, FaceSdfError::AngleRange { .. }), "{bad}: {error:?}");
        }
    }
}
