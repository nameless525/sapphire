//! Spot lights' shadows (M5 L16), planned on the CPU: which flagged spots cast this frame, where
//! each one's tile looks, and whether the tile must be drawn again or can be sampled as an
//! earlier frame drew it.
//!
//! - **Who:** a [`LightKind::Spot`] whose author set `shadow`, that can light something, whose cone
//!   is at most [`MAX_HALF_ANGLE`] round its axis, and whose cone reaches the main view's frustum
//!   or lies within [`VIEW_MARGIN`] of it. Of those the nearest the camera take the tier's tiles
//!   ([`ShadowSettings::spots`]), ties to the earlier light, a cone outside the view ranked further
//!   by [`TURN_PENALTY`] yield bands per radian the camera would turn to bring it in. The last
//!   chosen yields to the first left out as their ranks meet, over a quarter of its range
//!   ([`YIELD_SHARE`], its yield band): its shadow's weight falls to 0 before its tile changes
//!   hands, so a shadow fades rather than pops. A cone outside the view yields the same way as its
//!   penalty passes its band, so its weight is 0 a few degrees out, and every weight moves
//!   continuously as cones enter and leave the view (review F1, 2026-11-15: a nearer cone
//!   entering used to take the last tile at once). A chosen spot outside the view still has its
//!   tile drawn, cached as any other. Nothing but the tiles' cache is kept between frames.
//! - **Where:** a perspective view from the light along its axis, square, wide enough that its
//!   cone keeps [`TILE_PADDING`] texels clear of the tile's edges, from [`SPOT_NEAR`] (geometry
//!   nearer the light, its own housing, casts nothing) out to the light's range ([`SpotFit`]).
//!   Reverse-Z, as every map here is.
//! - **Cached:** a tile keeps the fit it was drawn with and a signature of what cast into it
//!   (every caster in its cone: slot, mesh, level and transform). It is drawn again only when it
//!   must be: its light was just given it, moved or changed its cone; a caster in its cone moved,
//!   came or went or changed level; or something in its cone moves by itself (a character, a
//!   skinned or morphed instance, foliage in the wind), which draws it every frame.
//!
//! [`ShadowSettings::spots`]: super::ShadowSettings::spots

use glam::{Mat4, Vec3, Vec4};

use super::MAX_SPOT_SHADOWS;
use super::fit::TILE_PADDING;
use crate::bounds::Sphere;
use crate::{CameraData, LightKind, LocalLight};

/// How near the light a spot's view begins, in meters: whatever is nearer it (a lamp's housing,
/// its glass) casts nothing.
pub(crate) const SPOT_NEAR: f32 = 0.2;
/// The widest cone a spot casts a shadow from, as its outer half-angle: 60°, so 120° across. A wider
/// one's perspective would spend its tile's texels at the edges; it casts nothing.
pub(crate) const MAX_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_3;
/// Over what share of its range the last chosen spot's shadow fades as the first left out comes as
/// near the camera: they trade the tile at weight 0.
pub(crate) const YIELD_SHARE: f32 = 0.25;
/// How far outside the main view a flagged spot's cone stays a candidate, as the angle the camera
/// would turn to bring it in (its cone sphere's distance past the frustum over its distance from
/// the camera): 90°.
pub(crate) const VIEW_MARGIN: f32 = std::f32::consts::FRAC_PI_2;
/// The penalty on the rank of a spot whose cone is outside the view, in its yield bands per radian of
/// that turn: its own weight is 0 an eighth of a radian (7°) out, and at [`VIEW_MARGIN`] it ranks
/// over twelve bands further than its distance, past the lights in view it could otherwise displace
/// at once. A quarter-degree turn moves its rank by 0.035 bands, so no weight steps by more than
/// about 0.05.
pub(crate) const TURN_PENALTY: f32 = 8.0;

/// Where a spot light's tile looks: a square perspective view from `position` along `direction`,
/// `tangent` the tangent of its half-angle, from [`SPOT_NEAR`] to `far` (the light's range),
/// `resolution` texels across.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpotFit {
    pub position: Vec3,
    /// Unit length.
    pub direction: Vec3,
    pub tangent: f32,
    pub far: f32,
    pub resolution: u32,
}

/// Why a light casts no spot shadow, or the fit it casts through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SpotCast {
    /// Not a spot whose author asked for a shadow, or one that can't light anything.
    No,
    /// Flagged, but wider than 120° across.
    TooWide,
    Fit(SpotFit),
}

impl SpotFit {
    /// The fit of `light`'s tile of `resolution` texels, if it is a flagged spot that can cast.
    pub(crate) fn of(light: &LocalLight, resolution: u32) -> SpotCast {
        let LightKind::Spot { direction, outer_angle, shadow: true, .. } = light.kind else {
            return SpotCast::No;
        };
        let valid = light.position.is_finite()
            && light.range.is_finite()
            && light.range > SPOT_NEAR
            && light.intensity > 0.0
            && outer_angle.is_finite()
            && outer_angle > 0.0;
        let Some(direction) = direction.try_normalize().filter(|_| valid) else { return SpotCast::No };
        if outer_angle > MAX_HALF_ANGLE {
            return SpotCast::TooWide;
        }
        // The cone's circle inscribed in the square, padded on every side.
        let texels = resolution as f32;
        let tangent = outer_angle.tan() * texels / (texels - 2.0 * TILE_PADDING);
        SpotCast::Fit(Self { position: light.position, direction, tangent, far: light.range, resolution })
    }

    /// World to the light's view: it looks down −Z along the spot's axis.
    pub(crate) fn view(&self) -> Mat4 {
        let up = if self.direction.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        glam::camera::rh::view::look_to_mat4(self.position, self.direction, up)
    }

    /// The light's view to the tile's clip space: square, reverse-Z, depth 1 at [`SPOT_NEAR`] and 0
    /// at `far`.
    pub(crate) fn projection(&self) -> Mat4 {
        let (near, far) = (SPOT_NEAR, self.far);
        let scale = 1.0 / self.tangent;
        let depth = near / (far - near);
        Mat4::from_cols(
            Vec4::new(scale, 0.0, 0.0, 0.0),
            Vec4::new(0.0, scale, 0.0, 0.0),
            Vec4::new(0.0, 0.0, depth, -1.0),
            Vec4::new(0.0, 0.0, depth * far, 0.0),
        )
    }

    /// World to the tile's clip space.
    pub(crate) fn clip_from_world(&self) -> Mat4 {
        self.projection() * self.view()
    }

    /// The camera the spot pass renders the tile through.
    pub(crate) fn camera(&self) -> CameraData {
        CameraData { view: self.view(), proj: self.projection(), position: self.position }
    }

    /// One texel's width, in meters, per meter from the light along its axis.
    pub(crate) fn texel_per_meter(&self) -> f32 {
        2.0 * self.tangent / self.resolution as f32
    }

    /// Whether a world-space `sphere` can cast into the tile: it reaches into the view's frustum.
    pub(crate) fn holds(&self, sphere: &Sphere) -> bool {
        let at = self.view().transform_point3(sphere.center);
        let depth = -at.z;
        let radius = sphere.radius;
        if depth + radius < SPOT_NEAR || depth - radius > self.far {
            return false;
        }
        // The side planes through the light: x = ±tangent × depth, and y alike.
        let length = (1.0 + self.tangent * self.tangent).sqrt();
        let reach = radius * length;
        at.x.abs() - self.tangent * depth <= reach && at.y.abs() - self.tangent * depth <= reach
    }

    /// The sphere around the light's cone (of its outer half-angle, out to its range), for testing
    /// whether its light reaches a view.
    pub(crate) fn cone_sphere(light: &LocalLight) -> Option<Sphere> {
        let LightKind::Spot { direction, outer_angle, .. } = light.kind else { return None };
        let direction = direction.try_normalize()?;
        let (sin, cos) = outer_angle.sin_cos();
        let range = light.range;
        Some(if outer_angle > std::f32::consts::FRAC_PI_4 {
            Sphere { center: light.position + direction * (range * cos), radius: range * sin }
        } else {
            let radius = range / (2.0 * cos);
            Sphere { center: light.position + direction * radius, radius }
        })
    }

    /// The finest texel this tile has anywhere over `sphere`, in meters: at its point nearest the
    /// light, no nearer than [`SPOT_NEAR`].
    pub(crate) fn texel_at(&self, sphere: &Sphere) -> f32 {
        let distance = (sphere.center.distance(self.position) - sphere.radius).max(SPOT_NEAR);
        self.texel_per_meter() * distance
    }
}

/// A spot light chosen this frame: the light's index in the packet, its fit, its shadow's weight
/// and its tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChosenSpot {
    pub light: u32,
    pub fit: SpotFit,
    pub weight: f32,
    pub tile: u8,
    /// Its tile is drawn this frame.
    pub redraw: bool,
}

/// What a tile of the spot layer holds, as last drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SpotTile {
    /// The fit it was drawn with, while it holds that drawing.
    pub drawn: Option<SpotFit>,
    /// What cast into it ([`Signature`]).
    pub signature: u64,
}

/// A spot's candidate while choosing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    pub light: u32,
    pub fit: SpotFit,
    /// From the camera to the light, plus its penalty.
    pub rank: f32,
    /// How much further it ranks for its cone being outside the view ([`TURN_PENALTY`]), meters.
    pub penalty: f32,
}

/// Chooses this frame's spots into `chosen` from `lights` for `camera` (at most `count`), each with
/// its weight, and gives each a tile of `tiles`: the one that holds its fit where one does, so a
/// still light keeps its drawing, else a free one. `candidates` is scratch. Returns how many flagged
/// spots were too wide to cast.
pub(crate) fn choose(
    lights: &[LocalLight],
    camera: &CameraData,
    (count, resolution): (usize, u32),
    tiles: &[SpotTile; MAX_SPOT_SHADOWS],
    candidates: &mut Vec<Candidate>,
    chosen: &mut Vec<ChosenSpot>,
) -> u32 {
    candidates.clear();
    chosen.clear();
    let mut too_wide = 0;
    if count == 0 {
        return 0;
    }
    let planes = frustum_planes(camera.proj * camera.view);
    for (index, light) in lights.iter().enumerate() {
        let fit = match SpotFit::of(light, resolution) {
            SpotCast::No => continue,
            SpotCast::TooWide => {
                too_wide += 1;
                continue;
            }
            SpotCast::Fit(fit) => fit,
        };
        let Some(sphere) = SpotFit::cone_sphere(light) else { continue };
        // How far the cone lies past the frustum's farthest plane, and the turn that brings it in.
        let outside = planes
            .iter()
            .map(|plane| -(plane.truncate().dot(sphere.center) + plane.w) - sphere.radius)
            .fold(0.0f32, f32::max);
        let turn = outside / sphere.center.distance(camera.position).max(sphere.radius).max(1e-3);
        if turn <= VIEW_MARGIN {
            let penalty = TURN_PENALTY * turn * yield_band(&fit);
            let rank = light.position.distance(camera.position) + penalty;
            candidates.push(Candidate { light: index as u32, fit, rank, penalty });
        }
    }
    // In place; equal distances keep the packet's order.
    candidates.sort_unstable_by(|a, b| a.rank.total_cmp(&b.rank).then(a.light.cmp(&b.light)));
    let count = count.min(MAX_SPOT_SHADOWS);
    let next = candidates.get(count).map(|left_out| left_out.rank);
    let mut taken = [false; MAX_SPOT_SHADOWS];
    for candidate in candidates.iter().take(count) {
        // It yields to the first left out, and outside the view to its own penalty: in view, with
        // none left out, 1.
        let band = yield_band(&candidate.fit);
        let own = band - candidate.penalty;
        let room = next.map_or(own, |next| (next - candidate.rank).min(own));
        let weight = smoothstep(0.0, band, room);
        // The tile that holds this very fit keeps it.
        let held = (0..count).find(|&tile| !taken[tile] && tiles[tile].drawn == Some(candidate.fit));
        let tile = held.map_or(u8::MAX, |tile| {
            taken[tile] = true;
            tile as u8
        });
        chosen.push(ChosenSpot { light: candidate.light, fit: candidate.fit, weight, tile, redraw: false });
    }
    // The rest take the free tiles, those holding nothing first.
    for spot in chosen.iter_mut().filter(|spot| spot.tile == u8::MAX) {
        let free =
            |empty: bool| (0..count).find(|&tile| !taken[tile] && tiles[tile].drawn.is_none() == empty);
        if let Some(tile) = free(true).or_else(|| free(false)) {
            taken[tile] = true;
            spot.tile = tile as u8;
        }
    }
    too_wide
}

/// Over what distance in rank a spot's shadow yields: a quarter of its range ([`YIELD_SHARE`]).
fn yield_band(fit: &SpotFit) -> f32 {
    (YIELD_SHARE * fit.far).max(1e-3)
}

/// What cast into a tile, folded into one word (64-bit FNV-1a over each caster's slot, mesh, level
/// and transform, in slot order): a tile whose signature holds draws the same depth again.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Signature(pub u64);

impl Default for Signature {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Signature {
    pub(crate) fn word(&mut self, word: u32) {
        for byte in word.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    /// A caster: its slot, mesh, level and transform.
    pub(crate) fn caster(&mut self, slot: u32, mesh: u32, level: u8, transform: &Mat4) {
        self.word(slot);
        self.word(mesh);
        self.word(u32::from(level));
        transform.to_cols_array().iter().for_each(|value| self.word(value.to_bits()));
    }
}

/// The view frustum's planes, facing inward, normalized: left, right, bottom, top and near (a
/// reverse-Z projection's far plane is at infinity).
fn frustum_planes(clip_from_world: Mat4) -> [Vec4; 5] {
    let row = |index| clip_from_world.row(index);
    let planes = [row(3) + row(0), row(3) - row(0), row(3) + row(1), row(3) - row(1), row(3) - row(2)];
    planes.map(|plane| plane / plane.truncate().length().max(f32::MIN_POSITIVE))
}

/// WGSL's `smoothstep`.
fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lamp(position: Vec3, outer_degrees: f32, shadow: bool) -> LocalLight {
        LocalLight {
            position,
            color: Vec3::ONE,
            intensity: 2.0,
            range: 8.0,
            kind: LightKind::Spot {
                direction: Vec3::NEG_Y,
                inner_angle: 0.5 * outer_degrees.to_radians(),
                outer_angle: outer_degrees.to_radians(),
                shadow,
            },
        }
    }

    /// A camera at the street's start, looking down it (−Z).
    fn camera() -> CameraData {
        CameraData::look_at(
            Vec3::new(0.0, 1.6, 4.0),
            Vec3::new(0.0, 1.0, -20.0),
            Vec3::Y,
            0.9,
            16.0 / 9.0,
            0.05,
        )
    }

    fn fit(light: &LocalLight) -> SpotFit {
        match SpotFit::of(light, 1024) {
            SpotCast::Fit(fit) => fit,
            other => panic!("{other:?}"),
        }
    }

    /// The tile's projection puts the cone's edge just inside the padding, depth 1 at the near
    /// plane and 0 at the range, and the frustum holds what it shows.
    #[test]
    fn a_spots_view_holds_its_cone_inside_the_padding() {
        let light = lamp(Vec3::new(0.0, 4.0, 0.0), 40.0, true);
        let fit = fit(&light);
        let clip = fit.clip_from_world();
        let project = |point: Vec3| clip.project_point3(point);
        assert!((project(Vec3::new(0.0, 4.0 - SPOT_NEAR, 0.0)).z - 1.0).abs() < 1e-5);
        assert!(project(Vec3::new(0.0, 4.0 - 8.0, 0.0)).z.abs() < 1e-5);
        // A point on the cone's edge, 3 m down: inside the tile by the padding, in texels.
        let edge = Vec3::new(3.0 * 40f32.to_radians().tan(), 1.0, 0.0);
        let ndc = project(edge);
        let texels_inside = (1.0 - ndc.x.abs().max(ndc.y.abs())) * 0.5 * 1024.0;
        assert!((texels_inside - TILE_PADDING).abs() < 0.05, "{texels_inside}");
        assert!(fit.holds(&Sphere { center: edge, radius: 0.0 }));
        assert!(!fit.holds(&Sphere { center: Vec3::new(5.0, 1.0, 0.0), radius: 0.5 }), "outside the cone");
        assert!(fit.holds(&Sphere { center: Vec3::new(4.0, 1.0, 0.0), radius: 1.5 }), "reaching in");
        assert!(!fit.holds(&Sphere { center: Vec3::new(0.0, 4.1, 0.0), radius: 0.05 }), "nearer than near");
        assert!(!fit.holds(&Sphere { center: Vec3::new(0.0, -5.0, 0.0), radius: 0.5 }), "past its range");
    }

    /// Only a flagged, valid spot of at most 120° across casts.
    #[test]
    fn only_flagged_spots_up_to_120_degrees_cast() {
        let at = Vec3::new(0.0, 4.0, 0.0);
        assert!(matches!(SpotFit::of(&lamp(at, 60.0, true), 1024), SpotCast::Fit(_)));
        assert_eq!(SpotFit::of(&lamp(at, 60.5, true), 1024), SpotCast::TooWide);
        assert_eq!(SpotFit::of(&lamp(at, 40.0, false), 1024), SpotCast::No);
        let point = LocalLight { kind: LightKind::Point, ..lamp(at, 40.0, true) };
        assert_eq!(SpotFit::of(&point, 1024), SpotCast::No);
        let dark = LocalLight { intensity: 0.0, ..lamp(at, 40.0, true) };
        assert_eq!(SpotFit::of(&dark, 1024), SpotCast::No);
    }

    fn run(
        lights: &[LocalLight],
        count: usize,
        tiles: &[SpotTile; MAX_SPOT_SHADOWS],
    ) -> (Vec<ChosenSpot>, u32) {
        let mut chosen = Vec::new();
        let too_wide = choose(lights, &camera(), (count, 1024), tiles, &mut Vec::new(), &mut chosen);
        (chosen, too_wide)
    }

    /// The nearest flagged spots the view sees take the tiles; a light behind the camera, one
    /// unflagged and one too wide take none; phones choose none.
    #[test]
    fn the_nearest_flagged_spots_in_view_take_the_tiles() {
        let street: Vec<LocalLight> =
            (0..6).map(|index| lamp(Vec3::new(1.5, 4.0, -6.0 * index as f32), 40.0, true)).collect();
        let mut lights =
            vec![lamp(Vec3::new(0.0, 4.0, 30.0), 40.0, true), lamp(Vec3::new(0.0, 4.0, -1.0), 40.0, false)];
        lights.push(lamp(Vec3::new(0.0, 4.0, -2.0), 70.0, true));
        lights.extend(street);
        let tiles = [SpotTile::default(); MAX_SPOT_SHADOWS];
        let (chosen, too_wide) = run(&lights, 4, &tiles);
        assert_eq!(too_wide, 1);
        let picked: Vec<u32> = chosen.iter().map(|spot| spot.light).collect();
        assert_eq!(picked, [3, 4, 5, 6], "the four nearest of the street, nearest first");
        let given: Vec<u8> = chosen.iter().map(|spot| spot.tile).collect();
        assert_eq!(given, [0, 1, 2, 3]);
        assert!(chosen.iter().all(|spot| spot.weight == 1.0), "the fifth is a whole 6 m further");
        let (two, _) = run(&lights, 2, &tiles);
        assert_eq!(two.len(), 2);
        assert!(run(&lights, 0, &tiles).0.is_empty(), "phones cast none");
    }

    /// As a fifth lamp comes as near as the fourth, the fourth's shadow fades out, continuously,
    /// and they trade the tile at weight 0; a still light keeps its tile as others come and go.
    #[test]
    fn a_fifth_lamp_coming_nearer_fades_the_fourth_out_before_it_takes_its_tile() {
        let mut lights: Vec<LocalLight> =
            (0..4).map(|index| lamp(Vec3::new(1.5, 4.0, -6.0 * index as f32), 40.0, true)).collect();
        lights.push(lamp(Vec3::new(-1.5, 4.0, -30.0), 40.0, true));
        let mut tiles = [SpotTile::default(); MAX_SPOT_SHADOWS];
        let (first, _) = run(&lights, 4, &tiles);
        for spot in &first {
            tiles[spot.tile as usize].drawn = Some(spot.fit);
        }
        // The fifth walks in toward the camera along its own line, in 5 cm steps, from past the
        // fourth (18 m down the street) to between it and the third (12 m).
        let fourth_rank = lights[3].position.distance(camera().position);
        let mut last = 1.0f32;
        let mut traded = false;
        for step in 0..300 {
            let z = -30.0 + 0.05 * step as f32;
            lights[4].position.z = z;
            let (chosen, _) = run(&lights, 4, &tiles);
            let weight_of =
                |light: u32| chosen.iter().find(|spot| spot.light == light).map(|spot| spot.weight);
            // The three nearest keep their tiles throughout.
            for spot in chosen.iter().filter(|spot| spot.light < 3) {
                assert_eq!(spot.tile, first[spot.light as usize].tile);
            }
            let fourth = weight_of(3);
            let fifth = weight_of(4);
            match (fourth, fifth) {
                (Some(weight), None) => {
                    assert!(weight <= last + 1e-6 && last - weight < 0.08, "step {step}: {last} to {weight}");
                    last = weight;
                }
                (None, Some(weight)) => {
                    if !traded {
                        assert!(last < 0.01 && weight < 0.01, "they trade at weight 0: {last}, {weight}");
                        traded = true;
                    }
                }
                other => panic!("one of the two holds the last tile: {other:?}"),
            }
        }
        assert!(traded);
        assert!(lights[4].position.distance(camera().position) < fourth_rank);
    }

    /// As the camera turns, a nearer flagged spot's cone entering the view (or leaving it) moves
    /// every weight continuously: the last chosen fades out before it gives up its tile, the spot
    /// arriving fades in from 0, and none steps by more than 0.08 for a quarter-degree turn, either
    /// way round, on two tiles and on four.
    #[test]
    fn a_cone_entering_the_view_fades_the_last_chosen_out() {
        let mut lights: Vec<LocalLight> =
            (0..4).map(|index| lamp(Vec3::new(1.5, 4.0, -6.0 * index as f32), 40.0, true)).collect();
        // Beside the camera and behind it on the left, both outside the view looking down the
        // street, and nearer than the street's last lamps.
        lights.push(lamp(Vec3::new(10.0, 4.0, 4.0), 40.0, true));
        lights.push(lamp(Vec3::new(-6.0, 4.0, 6.0), 40.0, true));
        let tiles = [SpotTile::default(); MAX_SPOT_SHADOWS];
        let ahead = camera();
        let forward = Vec3::new(0.0, -0.6, -24.0);
        // Quarter degrees from 100° left to 100° right.
        let quarters: Vec<i32> = (-400..=400).collect();
        for count in [2, 4] {
            let weights = |quarter: i32| {
                let turn = glam::Quat::from_rotation_y(-(quarter as f32 * 0.25).to_radians());
                let target = ahead.position + turn * forward;
                let camera = CameraData::look_at(ahead.position, target, Vec3::Y, 0.9, 16.0 / 9.0, 0.05);
                let mut chosen = Vec::new();
                choose(&lights, &camera, (count, 1024), &tiles, &mut Vec::new(), &mut chosen);
                let mut weights = [None; 6];
                for spot in &chosen {
                    weights[spot.light as usize] = Some(spot.weight);
                }
                weights
            };
            for order in [quarters.clone(), quarters.iter().rev().copied().collect()] {
                let mut last = weights(order[0]);
                for &quarter in &order[1..] {
                    let now = weights(quarter);
                    for (light, (was, is)) in last.iter().zip(&now).enumerate() {
                        let at = format!("{count} tiles, light {light}, at {}°", quarter as f32 * 0.25);
                        match (*was, *is) {
                            (Some(was), Some(is)) => assert!((is - was).abs() <= 0.08, "{at}: {was} to {is}"),
                            (Some(was), None) => assert!(was <= 0.01, "{at}: {was} to absent"),
                            (None, Some(is)) => assert!(is <= 0.01, "{at}: absent to {is}"),
                            (None, None) => {}
                        }
                    }
                    last = now;
                }
            }
        }
    }

    /// A signature follows what cast: the same casters fold to the same word, and any moved,
    /// added or changed caster to another.
    #[test]
    fn a_signature_follows_its_casters() {
        let fold = |casters: &[(u32, u32, u8, Mat4)]| {
            let mut signature = Signature::default();
            for (slot, mesh, level, transform) in casters {
                signature.caster(*slot, *mesh, *level, transform);
            }
            signature.0
        };
        let at = Mat4::from_translation(Vec3::new(1.0, 0.0, 2.0));
        let base = fold(&[(0, 3, 0, at), (4, 3, 1, Mat4::IDENTITY)]);
        assert_eq!(base, fold(&[(0, 3, 0, at), (4, 3, 1, Mat4::IDENTITY)]));
        let moved = Mat4::from_translation(Vec3::new(1.0, 0.0, 2.001));
        for other in [
            fold(&[(0, 3, 0, moved), (4, 3, 1, Mat4::IDENTITY)]),
            fold(&[(0, 3, 0, at), (4, 3, 0, Mat4::IDENTITY)]),
            fold(&[(0, 3, 0, at)]),
            fold(&[(0, 3, 0, at), (4, 3, 1, Mat4::IDENTITY), (5, 2, 0, at)]),
            fold(&[(0, 2, 0, at), (4, 3, 1, Mat4::IDENTITY)]),
        ] {
            assert_ne!(base, other);
        }
    }
}
