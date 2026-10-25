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
//! A mask is lit where its first channel (gray, or red) is at least one half; anti-aliased
//! edges are binarized there, so a mask's resolution sets its edge's precision. Leave
//! everything outside the face's UV islands in shadow in every mask: a mask with no shadow
//! gives the band after it no edge to sweep from (as one with nothing lit gives the band
//! before it no edge to sweep to), so that band turns all at once, at the later mask's
//! angle.

use std::fmt;

use bevy_reflect::TypePath;
use sapphire_assets::character_maps::face_sdf::{MAX_ANGLE_DEGREES, encode_angle};
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use super::distance::squared_distances;
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
}

impl Versioned for FaceSdfMap {
    const VERSION: u32 = 1;
}

fn is_zero(value: &u32) -> bool {
    *value == 0
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
        values.push(threshold_channel(channel, masks, map.allowed_violations, inputs, &mut size)?);
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

/// One channel's threshold values, from its masks. `size` is the map's size, set by the
/// first mask read.
fn threshold_channel(
    channel: SdfChannel,
    masks: &[ThresholdMask],
    allowed: u32,
    inputs: &Inputs<'_>,
    size: &mut Option<[u32; 2]>,
) -> Result<Vec<f32>, MapError> {
    let mut builder: Option<ChannelBuilder> = None;
    // One mask in memory at a time: a face map can have dozens.
    for mask in masks {
        let (path, image) = inputs.image(&mask.path)?;
        let expected = *size.get_or_insert([image.width, image.height]);
        check_size(&path, &image, expected)?;
        let builder = builder.get_or_insert_with(|| ChannelBuilder::new(expected[0], expected[1]));
        builder.push(mask.angle, image.pixels.iter().map(|pixel| pixel[0] >= 0.5).collect());
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
    /// Every texel's value so far: set when the texel turns to shadow, or at the end.
    values: Vec<f32>,
    /// The masks fed so far.
    count: usize,
    /// The last mask as given, for the monotonicity check.
    last: Vec<bool>,
    /// Texels lit in every mask so far: the last mask, monotonic by construction.
    lit: Vec<bool>,
    /// The last mask's stored angle value.
    last_value: f32,
    violations: Vec<PairViolation>,
}

impl ChannelBuilder {
    fn new(width: u32, height: u32) -> Self {
        let texels = width as usize * height as usize;
        Self {
            width,
            height,
            values: vec![0.0; texels],
            count: 0,
            last: Vec::new(),
            lit: vec![true; texels],
            last_value: 0.0,
            violations: Vec::new(),
        }
    }

    /// Adds the mask for a light at `degrees`: whether each texel is lit, row-major.
    fn push(&mut self, degrees: f32, mask: Vec<bool>) {
        let value = encode_angle(degrees);
        let (width, height) = (self.width as usize, self.height as usize);
        if self.count > 0 {
            self.check_pair(&mask);
        }
        let next: Vec<bool> = self.lit.iter().zip(&mask).map(|(&lit, &now)| lit && now).collect();
        let turned = |i: usize| self.lit[i] && !next[i];
        if self.count == 0 {
            // Shadowed by the first mask: never lit. The values are already 0.
        } else if (0..next.len()).any(turned) {
            // Distance to the previous lit region's edge, and to this one's lit region.
            let inside = squared_distances(width, height, |i| !self.lit[i]);
            let outside = squared_distances(width, height, |i| next[i]);
            let (low, high) = (self.last_value, value);
            let span = f64::from(high) - f64::from(low);
            for i in (0..next.len()).filter(|&i| turned(i)) {
                let (to_last, to_next) = (edge_distance(inside[i]), edge_distance(outside[i]));
                let t = if to_last.is_finite() && to_next.is_finite() {
                    to_last / (to_last + to_next)
                } else {
                    // No shadow in the last mask, or nothing lit in this one: no edge to
                    // sweep from or to, so the band turns at once, at this mask's angle.
                    1.0
                };
                // Strictly above the last mask's value, so the texel is still lit at that
                // mask's angle even where `t` is too small for f32 to show.
                self.values[i] = ((f64::from(low) + t * span) as f32).max(low.next_up()).min(high);
            }
        }
        self.lit = next;
        self.last = mask;
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
        let mut builder = ChannelBuilder::new(masks[0].1.width, masks[0].1.height);
        for (degrees, mask) in masks {
            builder.push(*degrees, mask.lit.clone());
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
