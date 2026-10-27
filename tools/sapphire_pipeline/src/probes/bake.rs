//! The probe bake proper: brick choice, surfels and their bounces, then each probe's rays (see
//! the [module docs](super)).

use std::num::NonZeroUsize;

use glam::{Vec2, Vec3};
use sapphire_assets::probes::{
    BRICK_CELLS, BRICK_PROBES, BRICK_SIDE, EMPTY_BRICK, MAX_BRICKS_PER_AXIS, ProbeData, ProbeGrid,
};
use sapphire_math::sh::{self, Projection, SkyTransfer};

use super::bvh::{Bvh, Hit};

/// A surface's material, as far as light bouncing off it goes: W4's environment shading,
/// diffuse only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakeMaterial {
    /// Linear RGB: the base color times its map's mean.
    pub albedo: Vec3,
    /// Linear RGB light it emits, as authored: each key shows its own share
    /// ([`Self::emission_under`]).
    pub emission: Vec3,
    /// The share of the emission that shines at every hour, 0 to 1; the rest follows each key's
    /// emission scale (M1 W1's keyed emission), as the renderer draws it.
    pub emission_steady: f32,
    /// The lit side's wrapped falloff.
    pub wrap: f32,
    /// The material's tint on the key light it keeps in shadow (times the key's own).
    pub shadow_tint: Vec3,
    /// Seen from either side, as foliage's alpha-tested cards are drawn (M1 W6): a ray reaching
    /// its back meets its front, never the inside of geometry, and the leak test's rays pass it
    /// (a leaf is no wall).
    pub two_sided: bool,
    /// Where it is cut out, as the renderer's alpha test cuts it: an index into
    /// [`BakeScene::cutouts`]. `None` covers everywhere.
    pub cutout: Option<u32>,
}

impl Default for BakeMaterial {
    /// A mid grey that emits nothing.
    fn default() -> Self {
        Self {
            albedo: Vec3::splat(0.5),
            emission: Vec3::ZERO,
            emission_steady: 0.0,
            wrap: 0.0,
            shadow_tint: Vec3::ONE,
            two_sided: false,
            cutout: None,
        }
    }
}

impl BakeMaterial {
    /// The light it emits under `key`: the key's emission scale on all but the steady share, as
    /// `environment.wgsl` scales it, so a lantern dark by day lights nothing then.
    pub fn emission_under(&self, key: &BakeKey) -> Vec3 {
        self.emission * (key.emission + (1.0 - key.emission) * self.emission_steady)
    }
}

/// A triangle in world space, counter-clockwise seen from the side it faces, with its corners'
/// shading normals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakeTriangle {
    pub positions: [Vec3; 3],
    pub normals: [Vec3; 3],
    /// Its corners' texture coordinates, which a cutout reads.
    pub uvs: [Vec2; 3],
    /// Index into [`BakeScene::materials`].
    pub material: u32,
}

/// Everything the grid sees.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BakeScene {
    pub triangles: Vec<BakeTriangle>,
    pub materials: Vec<BakeMaterial>,
    pub cutouts: Vec<Cutout>,
}

/// Where an alpha-tested material is drawn: its coverage (the color map's alpha times the base
/// color's) against its cutoff, read at a point's texture coordinates from the nearest texel,
/// repeating, as the renderer's alpha test reads the map's first level.
#[derive(Clone, Debug, PartialEq)]
pub struct Cutout {
    pub width: u32,
    pub height: u32,
    /// Coverage per texel, row by row from the top.
    pub coverage: Vec<f32>,
    pub cutoff: f32,
}

impl Cutout {
    /// Whether the material is drawn at `uv`.
    pub fn covers(&self, uv: Vec2) -> bool {
        let texel = |coordinate: f32, size: u32| {
            (((coordinate - coordinate.floor()) * size as f32) as u32).min(size.saturating_sub(1))
        };
        let (x, y) = (texel(uv.x, self.width), texel(uv.y, self.height));
        self.coverage.get((y * self.width + x) as usize).is_some_and(|&coverage| coverage >= self.cutoff)
    }
}

/// One time-of-day key's light: what the environment is lit by in that key's bake.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakeKey {
    pub hours: f32,
    /// Unit vector toward the key light (the sun, or the moon at night).
    pub to_light: Vec3,
    /// Linear RGB: its color × intensity.
    pub radiance: Vec3,
    /// What the key light keeps inside shadows.
    pub shadow_tint: Vec3,
    /// How much of materials' authored emission shows (time of day's emission key, M1 W1's
    /// keyed emission; [`BakeMaterial::emission_under`]).
    pub emission: f32,
    /// The sky's radiance, L2 per channel ([`sapphire_look::sky::sky_radiance`]).
    pub sky: [[f32; sh::L2]; 3],
}

/// How a grid is baked: the document's settings for one cook.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakeSettings {
    /// The box probes fill.
    pub min: Vec3,
    pub max: Vec3,
    /// Meters between probes.
    pub spacing: f32,
    /// 4 (L1) or 9 (L2).
    pub visibility_coefficients: u32,
    /// Rays each probe casts.
    pub probe_rays: u32,
    /// Rays each surfel casts.
    pub surfel_rays: u32,
    /// Meters between surfels.
    pub surfel_spacing: f32,
    /// Light bounces between surfaces, at least 1.
    pub bounces: u32,
    /// Scale on the bounce light probes record.
    pub bounce_strength: f32,
}

/// Why a bake can't run.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum BakeError {
    #[error("the bake box {min}..{max} is empty or not finite")]
    Bounds { min: Vec3, max: Vec3 },
    #[error("a spacing of {0} m between probes")]
    Spacing(f32),
    #[error("the bake box needs {bricks} bricks along an axis at {spacing} m; at most {MAX_BRICKS_PER_AXIS}")]
    TooLarge { bricks: u32, spacing: f32 },
    #[error("{0}")]
    Settings(&'static str),
    #[error("no geometry near the bake box: no bricks to keep")]
    Empty,
}

/// How far surfels reach past the bake box: light from just outside it still bounces in.
const SURFEL_MARGIN: f32 = 6.0;
/// Most surfels one triangle gets.
const MAX_SURFELS_PER_TRIANGLE: u32 = 4096;
/// Meters rays start off the surface they leave, against self-hits.
const RAY_OFFSET: f32 = 2e-3;
/// The share of a probe's rays that may see backfaces before it counts as inside geometry.
const MAX_BACKFACE_SHARE: f32 = 0.1;

/// A surface sample that carries light between bounces.
#[derive(Clone, Copy, Debug)]
struct Surfel {
    position: Vec3,
    /// Shading normal.
    normal: Vec3,
    /// The triangle's own normal, which rays leave along.
    face: Vec3,
    material: u32,
}

/// The surface a surfel's or probe's ray hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// A surfel: the nearest on the triangle hit.
    Surfel(u32),
    /// A triangle without surfels, outside their reach: entry of the far hits.
    Far(u32),
    /// The back of a one-sided surface, or under the surfel's own.
    Blocked,
}

/// The scene's rays: through the BVH, passing where a cutout leaves a card clear.
struct Rays<'a> {
    bvh: &'a Bvh,
    scene: &'a BakeScene,
}

impl Rays<'_> {
    /// Whether the material at `hit` is drawn there.
    fn covers(&self, hit: &Hit) -> bool {
        let triangle = &self.scene.triangles[hit.triangle as usize];
        material(self.scene, triangle.material).cutout.is_none_or(|cutout| {
            let [a, b, c] = triangle.uvs;
            let uv = a * (1.0 - hit.u - hit.v) + b * hit.u + c * hit.v;
            self.scene.cutouts[cutout as usize].covers(uv)
        })
    }

    /// The nearest drawn surface on the ray.
    fn closest(&self, origin: Vec3, dir: Vec3, t_max: f32) -> Option<Hit> {
        self.bvh.closest(origin, dir, t_max, &|hit| self.covers(hit))
    }

    /// Whether a drawn surface lies on the ray.
    fn occluded(&self, origin: Vec3, dir: Vec3, t_max: f32) -> bool {
        self.bvh.occluded(origin, dir, t_max, &|hit| self.covers(hit))
    }

    /// The nearest wall on the ray, for the leak test: a drawn surface but a two-sided card.
    fn closest_wall(&self, origin: Vec3, dir: Vec3, t_max: f32) -> Option<Hit> {
        self.bvh.closest(origin, dir, t_max, &|hit| {
            !material(self.scene, self.scene.triangles[hit.triangle as usize].material).two_sided
                && self.covers(hit)
        })
    }
}

/// Bakes the grid. Deterministic: the same scene, keys and settings give the same bits, whatever
/// the machine's thread count.
pub fn bake(scene: &BakeScene, keys: &[BakeKey], settings: &BakeSettings) -> Result<ProbeGrid, BakeError> {
    validate(settings)?;
    let bvh = Bvh::new(scene.triangles.iter().map(|triangle| triangle.positions).collect());
    let layout = Layout::new(settings, &bvh)?;
    let rays = Rays { bvh: &bvh, scene };
    let surfels = place_surfels(scene, settings);
    let surfel_light = light_surfels(scene, keys, settings, &rays, &surfels);
    let probes = bake_probes(scene, keys, settings, &rays, &layout, &surfels, &surfel_light);
    Ok(layout.into_grid(settings, keys, probes))
}

fn validate(settings: &BakeSettings) -> Result<(), BakeError> {
    let (min, max) = (settings.min, settings.max);
    if !(min.is_finite() && max.is_finite() && min.cmplt(max).all()) {
        return Err(BakeError::Bounds { min, max });
    }
    if !(settings.spacing.is_finite() && settings.spacing > 0.0) {
        return Err(BakeError::Spacing(settings.spacing));
    }
    if !matches!(settings.visibility_coefficients, 4 | 9) {
        return Err(BakeError::Settings("visibility is L1 (4 coefficients) or L2 (9)"));
    }
    if settings.probe_rays < 16 || settings.surfel_rays < 16 {
        return Err(BakeError::Settings("probes and surfels cast at least 16 rays"));
    }
    if !(settings.surfel_spacing.is_finite() && settings.surfel_spacing > 0.0) {
        return Err(BakeError::Settings("surfels need a spacing above 0"));
    }
    if settings.bounces == 0 {
        return Err(BakeError::Settings("at least one bounce"));
    }
    if !(settings.bounce_strength.is_finite() && settings.bounce_strength >= 0.0) {
        return Err(BakeError::Settings("the bounce strength is 0 or more"));
    }
    Ok(())
}

/// The grid's bricks and probes.
struct Layout {
    origin: Vec3,
    spacing: f32,
    bricks: [u32; 3],
    /// Per brick position, its kept brick or [`EMPTY_BRICK`].
    table: Vec<u32>,
    /// Per kept brick, its probes.
    brick_probes: Vec<u32>,
    /// Per probe, its grid coordinates.
    probes: Vec<[u32; 3]>,
}

impl Layout {
    /// Chooses the bricks: those whose box, grown by one cell on every side, any triangle passes
    /// through. Every surface then has probes around it (on both sides of a thin wall), and empty
    /// air and the solid ground beneath get none.
    fn new(settings: &BakeSettings, bvh: &Bvh) -> Result<Self, BakeError> {
        let spacing = settings.spacing;
        let brick_size = spacing * BRICK_CELLS as f32;
        let extent = settings.max - settings.min;
        let bricks = extent.to_array().map(|e| (e / brick_size).ceil().max(1.0) as u32);
        if let Some(&most) = bricks.iter().max()
            && most > MAX_BRICKS_PER_AXIS
        {
            return Err(BakeError::TooLarge { bricks: most, spacing });
        }
        let origin = settings.min;
        let mut table = Vec::with_capacity(bricks.iter().map(|&n| n as usize).product());
        let mut kept = Vec::new();
        for z in 0..bricks[2] {
            for y in 0..bricks[1] {
                for x in 0..bricks[0] {
                    let corner = origin + Vec3::new(x as f32, y as f32, z as f32) * brick_size;
                    let grown = Vec3::splat(spacing);
                    if bvh.overlaps_box(corner - grown, corner + Vec3::splat(brick_size) + grown) {
                        table.push(kept.len() as u32);
                        kept.push([x, y, z]);
                    } else {
                        table.push(EMPTY_BRICK);
                    }
                }
            }
        }
        if kept.is_empty() {
            return Err(BakeError::Empty);
        }
        // Every probe of a kept brick, indexed in grid order (x fastest), each once.
        let side = bricks.map(|n| n * BRICK_CELLS + 1);
        let linear = |[x, y, z]: [u32; 3]| (x + side[0] * (y + side[1] * z)) as usize;
        let mut index = vec![u32::MAX; side.iter().map(|&n| n as usize).product()];
        for brick in &kept {
            for local in brick_locals() {
                index[linear(global(*brick, local))] = 0;
            }
        }
        let mut probes = Vec::new();
        for z in 0..side[2] {
            for y in 0..side[1] {
                for x in 0..side[0] {
                    let slot = &mut index[linear([x, y, z])];
                    if *slot == 0 {
                        *slot = probes.len() as u32;
                        probes.push([x, y, z]);
                    }
                }
            }
        }
        let brick_probes = kept
            .iter()
            .flat_map(|&brick| brick_locals().map(move |local| global(brick, local)))
            .map(|g| index[linear(g)])
            .collect();
        Ok(Self { origin, spacing, bricks, table, brick_probes, probes })
    }

    fn position(&self, probe: [u32; 3]) -> Vec3 {
        self.origin + Vec3::new(probe[0] as f32, probe[1] as f32, probe[2] as f32) * self.spacing
    }

    fn into_grid(self, settings: &BakeSettings, keys: &[BakeKey], probes: Vec<BakedProbe>) -> ProbeGrid {
        let half = |value: f32| half::f16::from_f32(value).to_bits();
        let order = settings.visibility_coefficients as usize;
        let probes = dilate(&self, probes);
        ProbeGrid {
            origin: self.origin.to_array(),
            spacing: self.spacing,
            bricks: self.bricks,
            brick_table: self.table,
            brick_probes: self.brick_probes,
            key_hours: keys.iter().map(|key| key.hours).collect(),
            visibility_coefficients: settings.visibility_coefficients,
            visibility: probes
                .iter()
                .flat_map(|probe| probe.visibility[..order].iter().map(|&v| half(v)))
                .collect(),
            bounce: probes.iter().flat_map(|probe| probe.bounce.iter().map(|&v| half(v))).collect(),
            probe_data: probes.iter().map(|probe| probe.data.to_bits()).collect(),
        }
    }
}

/// A brick's probes' local coordinates, x fastest.
fn brick_locals() -> impl Iterator<Item = [u32; 3]> {
    (0..BRICK_PROBES).map(|i| [i % BRICK_SIDE, (i / BRICK_SIDE) % BRICK_SIDE, i / (BRICK_SIDE * BRICK_SIDE)])
}

fn global(brick: [u32; 3], local: [u32; 3]) -> [u32; 3] {
    [0, 1, 2].map(|axis| brick[axis] * BRICK_CELLS + local[axis])
}

/// Spreads surfels over every triangle within reach of the bake box, about `surfel_spacing`
/// apart, at fixed points of a low-discrepancy sequence, but where a cutout leaves a card clear.
/// A two-sided card's surfel faces the side its shading normal does. Returns them, and each
/// triangle's range.
fn place_surfels(scene: &BakeScene, settings: &BakeSettings) -> Surfels {
    let reach_min = settings.min - Vec3::splat(SURFEL_MARGIN);
    let reach_max = settings.max + Vec3::splat(SURFEL_MARGIN);
    let mut surfels = Vec::new();
    let mut ranges = Vec::with_capacity(scene.triangles.len());
    for triangle in &scene.triangles {
        let [a, b, c] = triangle.positions;
        let (min, max) = (a.min(b).min(c), a.max(b).max(c));
        let first = surfels.len() as u32;
        let cross = (b - a).cross(c - a);
        let area = 0.5 * cross.length();
        if min.cmpgt(reach_max).any() || max.cmplt(reach_min).any() || area <= 0.0 {
            ranges.push((first, 0));
            continue;
        }
        let face = cross.normalize();
        let count = ((area / (settings.surfel_spacing * settings.surfel_spacing)).ceil() as u32)
            .clamp(1, MAX_SURFELS_PER_TRIANGLE);
        let material = material(scene, triangle.material);
        let cutout = material.cutout.map(|cutout| &scene.cutouts[cutout as usize]);
        for i in 0..count {
            // The R2 sequence (Roberts 2018), folded onto the triangle by the square root map.
            let r1 = fract(0.5 + i as f32 * 0.754_877_7);
            let r2 = fract(0.5 + i as f32 * 0.569_840_3);
            let su = r1.sqrt();
            let weights = [1.0 - su, su * (1.0 - r2), su * r2];
            let uv =
                triangle.uvs[0] * weights[0] + triangle.uvs[1] * weights[1] + triangle.uvs[2] * weights[2];
            if cutout.is_some_and(|cutout| !cutout.covers(uv)) {
                continue;
            }
            let position = a * weights[0] + b * weights[1] + c * weights[2];
            let shading = triangle.normals[0] * weights[0]
                + triangle.normals[1] * weights[1]
                + triangle.normals[2] * weights[2];
            let face = if material.two_sided && shading.dot(face) < 0.0 { -face } else { face };
            let normal = shading.try_normalize().filter(|n| n.dot(face) > 0.0).unwrap_or(face);
            surfels.push(Surfel { position, normal, face, material: triangle.material });
        }
        ranges.push((first, surfels.len() as u32 - first));
    }
    Surfels { surfels, ranges }
}

fn fract(x: f32) -> f32 {
    x - x.floor()
}

struct Surfels {
    surfels: Vec<Surfel>,
    /// Per triangle, its surfels' first index and count.
    ranges: Vec<(u32, u32)>,
}

impl Surfels {
    /// Where a ray that hit `hit` in `scene` ended: the hit triangle's nearest surfel, or a far
    /// hit (on a two-sided card, either side).
    fn target(&self, scene: &BakeScene, hit: &Hit, point: Vec3, far: &mut Vec<FarHit>) -> Target {
        let triangle = &scene.triangles[hit.triangle as usize];
        if !hit.front && !material(scene, triangle.material).two_sided {
            return Target::Blocked;
        }
        let (first, count) = self.ranges[hit.triangle as usize];
        if count == 0 {
            let [a, b, c] = triangle.positions;
            let face = (b - a).cross(c - a).normalize_or(Vec3::Y);
            let normal = if hit.front { face } else { -face };
            far.push(FarHit { triangle: hit.triangle, point, normal });
            return Target::Far(far.len() as u32 - 1);
        }
        let mut best = first;
        let mut best_distance = f32::INFINITY;
        for index in first..first + count {
            let distance = self.surfels[index as usize].position.distance_squared(point);
            if distance < best_distance {
                best_distance = distance;
                best = index;
            }
        }
        Target::Surfel(best)
    }
}

/// A hit on a triangle out of the surfels' reach, lit directly when it's needed.
#[derive(Clone, Copy, Debug)]
struct FarHit {
    triangle: u32,
    point: Vec3,
    /// The side of the triangle the ray met.
    normal: Vec3,
}

/// The light every surfel sends out, per key (RGB), after the bounces; and what far hits send.
struct SurfelLight {
    /// Per surfel, per key.
    surfels: Vec<Vec3>,
    keys: usize,
}

impl SurfelLight {
    fn get(&self, surfel: u32, key: usize) -> Vec3 {
        self.surfels[surfel as usize * self.keys + key]
    }
}

/// What a surfel's rays found: its sky visibility and where each ray went.
struct SurfelRays {
    visibility: [f32; sh::L2],
    /// (target, cosine weight) per ray that hit a surface's front.
    hits: Vec<(Target, f32)>,
    far: Vec<FarHit>,
}

/// Lights every surfel under each key, then bounces the light between them.
fn light_surfels(
    scene: &BakeScene,
    keys: &[BakeKey],
    settings: &BakeSettings,
    rays: &Rays<'_>,
    surfels: &Surfels,
) -> SurfelLight {
    let directions = sh::fibonacci_directions(settings.surfel_rays);
    let solid_angle = 4.0 * std::f32::consts::PI / settings.surfel_rays as f32;
    let transfers: Vec<[SkyTransfer; 3]> =
        keys.iter().map(|key| key.sky.each_ref().map(SkyTransfer::new)).collect();
    // Each surfel's rays, on its own frame turned by its index so neighbors don't share a
    // pattern.
    let found: Vec<SurfelRays> = parallel_map(surfels.surfels.len(), |index| {
        let surfel = &surfels.surfels[index];
        let frame = turned(index as u32 ^ 0x5eed_0001);
        let origin = surfel.position + surfel.face * RAY_OFFSET;
        let mut visibility = Projection::<{ sh::L2 }>::default();
        let mut hits = Vec::new();
        let mut far = Vec::new();
        for &direction in &directions {
            let d = frame(direction);
            // Below its own surface, the surfel sees itself.
            if d.dot(surfel.face) <= 0.0 {
                continue;
            }
            match rays.closest(origin, d, f32::INFINITY) {
                None => visibility.add(d, 1.0, solid_angle),
                Some(hit) => {
                    let target = surfels.target(scene, &hit, origin + d * hit.t, &mut far);
                    let cosine = surfel.normal.dot(d).max(0.0);
                    if target != Target::Blocked && cosine > 0.0 {
                        // Irradiance in the renderer's normalization: ∫ L cos dω / π.
                        hits.push((target, cosine * solid_angle / std::f32::consts::PI));
                    }
                }
            }
        }
        SurfelRays { visibility: visibility.coefficients, hits, far }
    });
    // The light each surfel and far hit receives straight from the key light and the sky.
    let direct: Vec<Vec3> = parallel_map(surfels.surfels.len() * keys.len(), |slot| {
        let (index, key) = (slot / keys.len(), slot % keys.len());
        let surfel = &surfels.surfels[index];
        let material = material(scene, surfel.material);
        let sky = sky_irradiance(&transfers[key], &found[index].visibility, surfel.normal);
        let origin = surfel.position + surfel.face * RAY_OFFSET;
        material.albedo * (key_light(rays, &keys[key], material, origin, surfel.normal) + sky)
            + material.emission_under(&keys[key])
    });
    let far: Vec<Vec<Vec3>> = parallel_map(surfels.surfels.len(), |index| {
        found[index]
            .far
            .iter()
            .flat_map(|far| {
                let material = material(scene, scene.triangles[far.triangle as usize].material);
                let (normal, origin) = (far.normal, far.point + far.normal * RAY_OFFSET);
                keys.iter().zip(&transfers).map(move |(key, transfer)| {
                    // Out of the surfels' reach: the open sky, unoccluded.
                    let sky = sky_irradiance(transfer, &sh::project_constant(1.0), normal);
                    material.albedo * (key_light(rays, key, material, origin, normal) + sky)
                        + material.emission_under(key)
                })
            })
            .collect()
    });
    // Bounces: each pass, every surfel receives what the others sent in the last.
    let mut light = direct.clone();
    for _ in 1..settings.bounces.max(1) {
        light = parallel_map(surfels.surfels.len() * keys.len(), |slot| {
            let (index, key) = (slot / keys.len(), slot % keys.len());
            let surfel = &surfels.surfels[index];
            let albedo = material(scene, surfel.material).albedo;
            let mut bounce = Vec3::ZERO;
            for &(target, weight) in &found[index].hits {
                let sent = match target {
                    Target::Surfel(other) => light[other as usize * keys.len() + key],
                    Target::Far(far_index) => far[index][far_index as usize * keys.len() + key],
                    Target::Blocked => Vec3::ZERO,
                };
                bounce += sent * weight;
            }
            direct[slot] + albedo * bounce
        });
    }
    SurfelLight { surfels: light, keys: keys.len() }
}

fn material(scene: &BakeScene, index: u32) -> BakeMaterial {
    scene.materials.get(index as usize).copied().unwrap_or_default()
}

/// The key light reaching a surface at `origin` with shading normal `normal`, through W4's
/// environment shading: wrapped Lambert on the lit side, and the shadow tint's share kept in
/// shadow (environment.wgsl's `key_light`).
fn key_light(rays: &Rays<'_>, key: &BakeKey, material: BakeMaterial, origin: Vec3, normal: Vec3) -> Vec3 {
    if key.radiance.max_element() <= 0.0 {
        return Vec3::ZERO;
    }
    let n_dot_l = normal.dot(key.to_light);
    let lit = if n_dot_l > 0.0 && !rays.occluded(origin, key.to_light, f32::INFINITY) { 1.0 } else { 0.0 };
    let wrapped = ((n_dot_l + material.wrap) / (1.0 + material.wrap)).clamp(0.0, 1.0);
    let keep = key.shadow_tint * material.shadow_tint;
    key.radiance * keep.lerp(Vec3::ONE, lit * wrapped)
}

/// The sky light a surface facing `normal` receives through `visibility`.
fn sky_irradiance(transfer: &[SkyTransfer; 3], visibility: &[f32], normal: Vec3) -> Vec3 {
    Vec3::from(transfer.each_ref().map(|channel| sh::evaluate(&channel.apply(visibility), normal).max(0.0)))
}

/// One probe's bake.
#[derive(Clone, Debug, PartialEq)]
struct BakedProbe {
    /// L2 (the first 4 for L1).
    visibility: [f32; sh::L2],
    /// Per key, red's L1, then green's, then blue's: irradiance.
    bounce: Vec<f32>,
    data: ProbeData,
}

fn bake_probes(
    scene: &BakeScene,
    keys: &[BakeKey],
    settings: &BakeSettings,
    rays: &Rays<'_>,
    layout: &Layout,
    surfels: &Surfels,
    light: &SurfelLight,
) -> Vec<BakedProbe> {
    let directions = sh::fibonacci_directions(settings.probe_rays);
    let solid_angle = 4.0 * std::f32::consts::PI / settings.probe_rays as f32;
    let transfers: Vec<[SkyTransfer; 3]> =
        keys.iter().map(|key| key.sky.each_ref().map(SkyTransfer::new)).collect();
    parallel_map(layout.probes.len(), |index| {
        let position = layout.position(layout.probes[index]);
        let frame = turned(index as u32 ^ 0x9e0b_e5a1);
        let mut visibility = Projection::<{ sh::L2 }>::default();
        let mut bounce = vec![Projection::<{ sh::L1 }>::default(); keys.len() * 3];
        let mut backfaces = 0u32;
        let mut far = Vec::new();
        for &direction in &directions {
            let d = frame(direction);
            let Some(hit) = rays.closest(position, d, f32::INFINITY) else {
                visibility.add(d, 1.0, solid_angle);
                continue;
            };
            let point = position + d * hit.t;
            let target = surfels.target(scene, &hit, point, &mut far);
            if target == Target::Blocked {
                backfaces += 1;
                continue;
            }
            for (key, transfer) in transfers.iter().enumerate() {
                let radiance = match target {
                    Target::Surfel(surfel) => light.get(surfel, key),
                    Target::Far(index) => {
                        let normal = far[index as usize].normal;
                        let material = material(scene, scene.triangles[hit.triangle as usize].material);
                        let origin = point + normal * RAY_OFFSET;
                        let sky = sky_irradiance(transfer, &sh::project_constant(1.0), normal);
                        material.albedo * (key_light(rays, &keys[key], material, origin, normal) + sky)
                            + material.emission_under(&keys[key])
                    }
                    Target::Blocked => Vec3::ZERO,
                };
                for (channel, value) in radiance.to_array().into_iter().enumerate() {
                    bounce[key * 3 + channel].add(d, value, solid_angle);
                }
            }
        }
        let free = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z].map(|axis| {
            let reach = settings.spacing;
            let distance = rays.closest_wall(position, axis, reach * 1.01).map_or(reach, |hit| hit.t);
            ProbeData::quantize(distance, settings.spacing)
        });
        let valid = (backfaces as f32) <= MAX_BACKFACE_SHARE * settings.probe_rays as f32;
        BakedProbe {
            visibility: visibility.coefficients,
            bounce: bounce
                .iter()
                .flat_map(|projection| {
                    sh::irradiance(projection.coefficients).map(|c| c * settings.bounce_strength)
                })
                .collect(),
            data: ProbeData { free, valid },
        }
    })
}

/// Fills probes inside geometry from their open neighbors: each pass, an unfilled probe takes the
/// mean of the filled ones among its 26 neighbors, until none is left or none can be. Their
/// probe data keeps them marked invalid, which the renderer weighs them down by.
fn dilate(layout: &Layout, mut probes: Vec<BakedProbe>) -> Vec<BakedProbe> {
    let lookup: std::collections::HashMap<[u32; 3], usize> =
        layout.probes.iter().enumerate().map(|(index, &coordinates)| (coordinates, index)).collect();
    let mut filled: Vec<bool> = probes.iter().map(|probe| probe.data.valid).collect();
    loop {
        let mut next = Vec::new();
        for (index, coordinates) in layout.probes.iter().enumerate() {
            if filled[index] {
                continue;
            }
            let mut sum: Option<(BakedProbe, f32)> = None;
            for dz in -1i32..=1 {
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let neighbor = [dx, dy, dz]
                            .iter()
                            .zip(coordinates)
                            .map(|(&d, &c)| c.checked_add_signed(d))
                            .collect::<Option<Vec<u32>>>();
                        let Some(neighbor) = neighbor.and_then(|n| lookup.get(&[n[0], n[1], n[2]])) else {
                            continue;
                        };
                        if !filled[*neighbor] || *neighbor == index {
                            continue;
                        }
                        let source = &probes[*neighbor];
                        match &mut sum {
                            None => sum = Some((source.clone(), 1.0)),
                            Some((total, count)) => {
                                for (t, s) in total.visibility.iter_mut().zip(&source.visibility) {
                                    *t += s;
                                }
                                for (t, s) in total.bounce.iter_mut().zip(&source.bounce) {
                                    *t += s;
                                }
                                *count += 1.0;
                            }
                        }
                    }
                }
            }
            if let Some((mut mean, count)) = sum {
                mean.visibility.iter_mut().for_each(|v| *v /= count);
                mean.bounce.iter_mut().for_each(|v| *v /= count);
                mean.data = probes[index].data;
                next.push((index, mean));
            }
        }
        if next.is_empty() {
            break;
        }
        for (index, probe) in next {
            probes[index] = probe;
            filled[index] = true;
        }
    }
    probes
}

/// A rotation of the sphere picked by `seed`: directions mapped into an orthonormal frame around
/// a hashed axis (Duff et al. 2017's branchless frame), so each surfel or probe casts its own
/// turn of the shared ray set.
fn turned(seed: u32) -> impl Fn(Vec3) -> Vec3 {
    let hash = |mut x: u32| {
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        x ^ (x >> 16)
    };
    let u1 = (hash(seed) >> 8) as f32 / (1u32 << 24) as f32;
    let u2 = (hash(seed.wrapping_add(0x68e3_1da4)) >> 8) as f32 / (1u32 << 24) as f32;
    let z = 1.0 - 2.0 * u1;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let (sin, cos) = sapphire_math::det::sin_cos(std::f32::consts::TAU * u2);
    let n = Vec3::new(r * cos, r * sin, z);
    let sign = if n.z >= 0.0 { 1.0 } else { -1.0 };
    let a = -1.0 / (sign + n.z);
    let b = n.x * n.y * a;
    let tangent = Vec3::new(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x);
    let bitangent = Vec3::new(b, sign + n.y * n.y * a, -n.y);
    move |d: Vec3| tangent * d.x + bitangent * d.y + n * d.z
}

/// `f` over `0..count` on the machine's threads, in index order whatever their number.
fn parallel_map<T: Send>(count: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(1, NonZeroUsize::get).clamp(1, 32);
    let chunk = count.div_ceil(threads).max(1);
    let mut out: Vec<Option<T>> = (0..count).map(|_| None).collect();
    std::thread::scope(|scope| {
        for (part, slots) in out.chunks_mut(chunk).enumerate() {
            let f = &f;
            scope.spawn(move || {
                for (offset, slot) in slots.iter_mut().enumerate() {
                    *slot = Some(f(part * chunk + offset));
                }
            });
        }
    });
    out.into_iter().map(|value| value.expect("every slot was filled")).collect()
}
