//! The look at each switch of a level of detail (M5 Z2, the last line of the M5 plan's exit test 3): for
//! every step of a cooked chain, on every shipped tier, the renderer is walked through the step's
//! hysteresis to prove where it switches each way, and at each of those two distances the view is
//! drawn with the level on each side of the switch and with level 0 (what the close-up's forced finest
//! level draws there). The look test's measures (`sapphire_testkit::look`) read each picture as
//! `look_tiers` reads its scales, and its gates hold the two sides to level 0 and to each other. Run alone
//! with `cargo test -p sapphire_render --test gpu render::level_switches:: -- --ignored --test-threads=1
//! --nocapture`.
//!
//! The subjects are `look_tiers`' sphere cooked through the pipeline as the shipped content cooks:
//!
//! - **Characters (M5 D5):** the look test's material (a hard two-tone split, a black line 5.5 px wide at
//!   1080 rows at every distance), listed as a character the stage lets choose
//!   ([`CharacterDetail::ByError`]), with each chain a shipped character cooks with, read from its
//!   sidecar: the enemies' (every kind of enemy, the bosses and the look-dev character cook the default;
//!   the ambient cast its first two steps) and the heroes'. Errors are a share of the extent, so a
//!   switch's picture is the same at any size. The desktop cook's for the desktop tiers, the phones' (its
//!   first level dropped) for the phones'.
//! - **Static meshes (M5 D4):** the default chain under the environment material, which draws no outline
//!   at any level (`outline_draws` is 0 on both sides), with its terminator a hard step (`width` 0, as a
//!   prop that reads like a character asks). Anything shading as a character draws level 0 at every
//!   distance, so no static switch carries an outline. The environment's two tones aren't a step: its lit
//!   side rises from the shadow tone as N·L does, so the edge measure finds none on a large sphere and the
//!   slope on a small one. Its response is held by the edge's width and the gradients' spread against
//!   level 0's and across the switch, not by a hard step's bound.
//! - **Cells (M5 D6):** the same, cooked as a zone cooks a cell (`zone::optimize_place`, its border
//!   locked) with the benchmark district's `cell_levels`, read from its document. A cell's level against
//!   another's is the only switch a cell makes: there is no merged level. A cell is environment, so no
//!   outline at either level, its response held alone.
//!
//! The gates are `look_tiers`': a line within 5% of level 0's strength (and the coarser within 5% of the
//! finer's), its contrast above 0.97 of level 0's on a tier with temporal AA and 0.75 on the phones; a
//! hard edge under 1.8 px with temporal AA, under 4.0 on a phone's upscale; one edge within 1.3 times
//! another. The gradients' spread is held to D5's switch gate, 5%. The pictures draw each level as a mesh
//! of that one level (`levels`' `one_level`), so both sides are the same view, as `LevelsMode::Fixed`
//! would hold them; `character_levels` proves a level draws what the level alone draws.

use sapphire_assets::material::{EnvironmentMaterial, MaterialDocument, Terminator};
use sapphire_assets::mesh::Lod;
use sapphire_pipeline::bake::{self, OutlineSettings};
use sapphire_pipeline::bench::district::BenchDistrict;
use sapphire_pipeline::cook::Cook;
use sapphire_pipeline::mesh::{ImportedMaterial, ImportedMesh};
use sapphire_pipeline::meta::{AssetMeta, ImportSettings};
use sapphire_pipeline::optimize::{self, LodSettings};
use sapphire_pipeline::zone;
use sapphire_render::quality::{QualityTier, TierId, TierTable};
use sapphire_render::{
    CharacterBody, CharacterDetail, CharacterFace, CharacterInstance, CharacterLighting, MaterialDrive,
    PassCounts, RenderStats,
};
use sapphire_testkit::golden::Image;
use sapphire_testkit::look::{Edge, Lines, Region, Spread, darkness, luminance};

use super::levels::one_level;
use super::*;
use crate::look_tiers as look;

/// The benchmark district's document, whose `cell_levels` a zone's cells cook with.
const DISTRICT: &str = include_str!("../../../../../bench/district/district.ron");

/// The level-choice hysteresis: an instance goes coarser only when the coarser level is good enough
/// within the allowance over this (`levels::COARSER_MARGIN`, which the walk proves).
const COARSER_MARGIN: f32 = 1.25;

/// What a subject is drawn as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    /// Listed as a character the stage lets choose, under the look test's material.
    Character,
    /// A static mesh under the environment material, its terminator a hard step.
    Environment,
}

/// The sphere cooked with `chain` for `cook`: as a zone cooks a cell's mesh if `cell`, else as any mesh.
fn cooked(family: Family, chain: &LodSettings, cook: Cook, cell: bool) -> Mesh {
    let (vertices, indices) = look::sphere_geometry();
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let generated = match family {
        Family::Character => look::sphere_material(),
        Family::Environment => AnimeCharacterMaterial::default(),
    };
    let mut mesh = ImportedMesh {
        authored_outline: vec![false; vertices.len()],
        lods: vec![Lod { vertex_count: vertices.len() as u32, error: 0.0, submeshes }],
        materials: vec![ImportedMaterial { name: "sphere".to_owned(), generated }],
        vertices,
        indices,
        ..ImportedMesh::default()
    };
    if cell {
        let settings = zone::ZoneSettings { lods: chain.clone(), ..Default::default() };
        bake::outline_normals(&mut mesh, &settings.outline);
        zone::optimize_place(&mut mesh, &settings, None, cook);
    } else {
        let outline = OutlineSettings::default();
        bake::outline_normals(&mut mesh, &outline);
        optimize::optimize(&mut mesh, chain, outline.weld_tolerance(), cook);
    }
    mesh.pack(PositionFormat::Float32, &Default::default()).expect("packs")
}

/// The cook a tier's content comes from.
fn cook_of(tier: TierId) -> Cook {
    match tier {
        TierId::DesktopHigh | TierId::DesktopLow => Cook::Desktop,
        TierId::MobileHigh | TierId::MobileLow => Cook::Mobile,
    }
}

/// The sphere's silhouette radius in output pixels, its middle `distance` m away, on `height` rows.
fn radius_px(distance: f32, height: u32) -> f32 {
    (1.0 / (distance * distance - 1.0).sqrt()) / (look::FOV.to_radians() * 0.5).tan() * height as f32 / 2.0
}

/// One tier's renderer drawing one subject at a time, its middle at the origin, from +Z.
struct Subject {
    family: Family,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    texture: wgpu::Texture,
    environment: Option<MaterialHandle>,
    scale: f32,
    shown: Option<InstanceId>,
    next: u64,
}

impl Subject {
    fn new(gpu: &GpuContext, tier: &QualityTier, family: Family, width: u32) -> Self {
        let mut renderer = Renderer::new(gpu, FORMAT, tier.render_settings()).expect("renderer");
        let environment = (family == Family::Environment).then(|| {
            let document = EnvironmentMaterial {
                base_color: [look::GREY[0], look::GREY[1], look::GREY[2], 1.0],
                terminator: Terminator { width: 0.0, ..Terminator::default() },
                ..EnvironmentMaterial::default()
            };
            let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
            let bytes =
                Material::from_document(&document, &swizzles).expect("a material").to_cooked().unwrap();
            renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload")
        });
        let height = look::output(tier.id).1;
        Self {
            family,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            texture: target(gpu, FORMAT, (width, height)),
            environment,
            scale: tier.resolution.base_scale(height),
            shown: None,
            next: 0,
        }
    }

    fn upload(&mut self, mesh: &Mesh) -> MeshHandle {
        match self.environment {
            None => upload(&mut self.renderer, mesh),
            Some(material) => {
                let bytes = mesh.to_cooked().expect("cook");
                self.renderer.upload_mesh(Mesh::read(&bytes).expect("read"), &[material]).expect("upload")
            }
        }
    }

    /// Shows `mesh` alone, as a new instance from a cut, one frame at each of `distances` (its middle
    /// from the camera, in meters), and returns each frame's stats.
    fn draw(&mut self, gpu: &GpuContext, mesh: MeshHandle, distances: &[f32]) -> Vec<RenderStats> {
        let id = InstanceId(self.next);
        self.next += 1;
        let (width, height) = (self.texture.width(), self.texture.height());
        let mut stats = Vec::new();
        for (frame, &distance) in distances.iter().enumerate() {
            let packet = self.packets.begin();
            let camera = CameraData::look_at(
                Vec3::new(0.0, 0.0, distance),
                Vec3::ZERO,
                Vec3::Y,
                look::FOV.to_radians(),
                width as f32 / height as f32,
                0.05,
            );
            packet.views.push(ViewData { cut: frame == 0, ..ViewData::new(camera) });
            // As `look_tiers` lights its sphere: from +X turned up by its tilt, the sky and the lit side one grey.
            let toward = Vec3::new(look::TILT.cos(), look::TILT.sin(), 0.0);
            packet.sun = DirectionalLight { direction: -toward, color: Vec3::ONE, intensity: 1.0 };
            packet.sky =
                SkyGradient { zenith: Vec3::splat(0.5), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.5) };
            packet.ambient =
                AmbientLight { sky: Vec3::splat(0.5), ground: Vec3::splat(0.5), intensity: 0.12 };
            packet.post = look::passthrough();
            if frame == 0 {
                packet.instances.removed.extend(self.shown.replace(id));
                packet.instances.changed.push(MeshInstance { id, mesh, transform: Mat4::IDENTITY });
            }
            if self.family == Family::Character {
                // Unstyled, it is lit as the look test's sphere is, by the world's key.
                packet.characters.push(CharacterInstance {
                    id,
                    lighting: CharacterLighting::unstyled(packet.sun),
                    face: CharacterFace::default(),
                    body: CharacterBody::capsule(Vec3::NEG_Y, Vec3::Y, 1.0)
                        .with_detail(CharacterDetail::ByError),
                    opacity: 1.0,
                    drive: MaterialDrive::NONE,
                });
            }
            let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
            self.renderer.set_render_scale(self.scale);
            self.renderer.prepare(gpu, self.packets.current());
            let view = self.texture.create_view(&Default::default());
            let mut graph = FrameGraph::new();
            let target = graph.import_texture("target", &view);
            self.renderer.record(&mut graph, target);
            self.graph.execute(gpu, graph).expect("a valid graph");
            assert!(pollster::block_on(scope.pop()).is_none(), "no validation error");
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("frame complete");
            stats.push(self.renderer.stats());
        }
        stats
    }
}

/// The level the main view drew the subject at: the one level it counts.
fn level_of(main: &PassCounts) -> Option<usize> {
    let mut drawn = main.levels.iter().enumerate().filter(|(_, count)| **count > 0);
    let (level, _) = drawn.next()?;
    drawn.next().is_none().then_some(level)
}

/// What one picture of the sphere measures, in output pixels, as `look_tiers`' `Look` measures it, at
/// the sphere's size `distance` m away.
#[derive(Clone, Copy, Debug)]
struct Look {
    /// The line at the lit side's silhouette, over rows through the sphere's middle half.
    lines: Lines,
    /// The terminator's mean 10–90% width and steepest pixel over the same rows, where it is a step:
    /// none where the lit side rises from the shadow tone as N·L does, as the environment's does.
    edge: Option<(f32, f32)>,
    /// The gradients' spread over the band round the terminator through the same rows (`Spread::width`),
    /// which D5's switch gates hold to 5% (spec §5.12, "As built (M5 D5)").
    spread: f32,
}

impl Look {
    fn of(pixels: &[[u8; 4]], (width, height): (u32, u32), distance: f32) -> Self {
        let at = |x: u32, y: u32| pixels[(y * width + x) as usize];
        let (cx, cy, radius) = (width as f32 / 2.0, height as f32 / 2.0, radius_px(distance, height));
        let line_px = look::LINE_WIDTH * height as f32 / 1080.0;
        // `look_tiers`' 16 pixels each side of the terminator, inside the half radius on a small sphere,
        // so the profile's flat sides stay off its silhouette.
        let half = (radius * 0.5).clamp(4.0, 16.0) as u32;
        let rows_half = look::ROWS as i32 / 2;
        let rows: Vec<u32> = (-rows_half..=rows_half)
            .map(|i| (cy + i as f32 * radius / (2 * rows_half) as f32) as u32)
            .collect();
        let mut darknesses = Vec::new();
        let mut edge = Some((0.0, 0.0));
        for &y in &rows {
            let dy = y as f32 + 0.5 - cy;
            let silhouette = cx + (radius * radius - dy * dy).sqrt();
            let sky = luminance(at(width - 1, y));
            let from = (silhouette - 4.0) as u32;
            let to = ((silhouette + 2.0 * line_px + 8.0) as u32).min(width - 1);
            darknesses.extend((from..to).map(|x| darkness(luminance(at(x, y)), sky)));
            let terminator = (cx + dy * look::TILT.tan()) as u32;
            let profile: Vec<f32> =
                (terminator - half..terminator + half).map(|x| luminance(at(x, y))).collect();
            edge = edge.zip(Edge::of(&profile, 0.05)).map(|((width, steepest), row)| {
                (width + row.width / rows.len() as f32, steepest + row.steepest / rows.len() as f32)
            });
        }
        // The band the profiles cross: the terminator's slant over the rows, and `half` either side.
        let slant = 0.5 * radius * look::TILT.tan();
        let band = Region {
            x: (cx - slant - half as f32) as u32,
            y: (cy - 0.5 * radius) as u32,
            width: (2.0 * (slant + half as f32)) as u32,
            height: radius as u32,
        };
        let image = Image { width, height, rgba: pixels.iter().flatten().copied().collect() };
        let spread = Spread::of(&image, band, |_, _| false).expect("the band lies in the picture").width();
        Self { lines: Lines::from_darkness(darknesses), edge, spread }
    }

    /// The line's pixels covered and spread over, a row.
    fn line(&self) -> (f32, f32) {
        (self.lines.strength / look::ROWS as f32, self.lines.width() / look::ROWS as f32)
    }
}

/// One switch's two distances: where the main view goes coarser and where it comes back finer, as the
/// nearest point's distance, in meters.
struct Switch {
    coarser: f32,
    finer: f32,
}

impl Switch {
    /// The step to a level of error `error` (meters) on `height` output rows at `bias`: good enough when
    /// the error covers at most 2^bias output pixels at the nearest point, coarser only within that over
    /// the margin (`levels::choose`).
    fn to(error: f32, height: u32, bias: f32) -> Self {
        let pixels_per_meter = height as f32 * 0.5 / (look::FOV.to_radians() * 0.5).tan();
        let finer = error * pixels_per_meter / bias.exp2();
        Self { coarser: COARSER_MARGIN * finer, finer }
    }
}

/// Every switch of `mesh`'s chain on `tier`, drawn as `family`: each is walked to prove where it is taken
/// each way, then drawn at those distances at level 0 and the levels either side, and held to the look
/// test's gates. Prints a row a picture set and returns what failed.
fn switches(gpu: &GpuContext, tier: &QualityTier, family: Family, mesh: &Mesh, label: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let errors: Vec<f32> = mesh.lods.iter().map(|lod| lod.error).collect();
    let bounds = mesh.streams.bounds;
    let bounding = (Vec3::from(bounds.max) - Vec3::from(bounds.min)).length() * 0.5;
    let height = look::output(tier.id).1;
    let bias = tier.render_settings().levels.bias;
    let steps: Vec<Switch> = errors[1..].iter().map(|&error| Switch::to(error, height, bias)).collect();
    // Wide enough for the nearest picture's line and the sky past it, and at least the look test's width.
    let line_px = look::LINE_WIDTH * height as f32 / 1080.0;
    let nearest = steps.iter().map(|step| step.finer + bounding).fold(f32::INFINITY, f32::min);
    let width = ((2.0 * (radius_px(nearest, height) + 2.0 * line_px + 24.0)) as u32)
        .next_multiple_of(64)
        .max(look::output(tier.id).0);
    let mut subject = Subject::new(gpu, tier, family, width);
    let size = (width, height);
    let full = subject.upload(mesh);
    let singles: Vec<MeshHandle> =
        (0..errors.len()).map(|level| subject.upload(&one_level(mesh, level))).collect();
    let temporal = tier.antialiasing.temporal;
    let frames = if temporal { look::TAA_FRAMES } else { 2 };
    // The look test's gates: lines within 5% of their strength and holding their contrast (a phone's upscale
    // softens them), a hard edge under its bound, and the phones' three tenths between one edge and another.
    let edge_bound = if temporal {
        1.8
    } else if subject.scale == 1.0 {
        1.2
    } else {
        4.0
    };
    let contrast_floor = if temporal { 0.97 } else { 0.75 };
    for (fine, step) in steps.iter().enumerate() {
        let coarse = fine + 1;
        let what = format!("{label} on {} ({bias} bias), level {fine} to {coarse}", tier.id);
        // The walk, from a fresh instance: finer short of the finer distance; held there to just short of
        // the coarser one; coarser just past it; held coarser back to just past the finer one; finer short
        // of it.
        let walk = [
            0.97 * step.finer,
            0.99 * step.coarser,
            1.01 * step.coarser,
            1.01 * step.finer,
            0.99 * step.finer,
        ];
        let distances: Vec<f32> = walk.iter().map(|nearest| nearest + bounding).collect();
        let levels: Vec<Option<usize>> =
            subject.draw(gpu, full, &distances).iter().map(|stats| level_of(&stats.passes.main)).collect();
        let expected = [fine, fine, coarse, coarse, fine].map(Some);
        if levels != expected {
            failures.push(format!(
                "{what}: the walk drew levels {levels:?}, not {expected:?}, at {distances:?} m"
            ));
        }
        for (direction, nearest) in [("coarser", step.coarser), ("finer", step.finer)] {
            let distance = nearest + bounding;
            let mut picture = |level: usize| {
                let stats = subject.draw(gpu, singles[level], &vec![distance; frames as usize]);
                let outlines = stats.last().expect("frames").passes.main.outline_draws;
                (Look::of(&read_back(gpu, &subject.texture), size, distance), outlines)
            };
            let (whole, whole_outlines) = picture(0);
            let finer_side = if fine == 0 { (whole, whole_outlines) } else { picture(fine) };
            let sides = [(fine, finer_side), (coarse, picture(coarse))];
            let at = format!(
                "{what}, {direction} at {distance:.2} m ({:.0} px radius)",
                radius_px(distance, height)
            );
            let [(_, (fine_look, _)), (_, (coarse_look, _))] = sides;
            let looks = [whole, fine_look, coarse_look];
            let figures = |f: &dyn Fn(&Look) -> String| looks.iter().map(f).collect::<Vec<_>>().join(" / ");
            eprintln!(
                "{at} (level 0 / {fine} / {coarse}): line covered {} px, spread over {} px, contrast {}; two-tone edge \
                 {} px, steepest {}; gradient spread {}",
                figures(&|look| format!("{:.2}", look.line().0)),
                figures(&|look| format!("{:.2}", look.line().1)),
                figures(&|look| format!("{:.3}", look.lines.contrast())),
                figures(&|look| look.edge.map_or("none".to_owned(), |(width, _)| format!("{width:.2}"))),
                figures(&|look| look
                    .edge
                    .map_or("none".to_owned(), |(_, steepest)| format!("{steepest:.2}"))),
                figures(&|look| format!("{:.3}", look.spread)),
            );
            let mut check = |held: bool, why: String| {
                if !held {
                    failures.push(format!("{at}: {why}"));
                }
            };
            for (level, (look, outlines)) in std::iter::once((0, (whole, whole_outlines))).chain(sides) {
                match family {
                    Family::Character => {
                        check(outlines > 0, format!("level {level} draws no outline"));
                        let strength = look.lines.strength / whole.lines.strength;
                        let contrast = look.lines.contrast() / whole.lines.contrast();
                        check(
                            (0.95..1.05).contains(&strength),
                            format!("level {level}'s line {strength:.3} of level 0's"),
                        );
                        check(
                            contrast > contrast_floor,
                            format!("level {level}'s contrast {contrast:.3} of level 0's"),
                        );
                        check(look.edge.is_some(), format!("level {level} shows no two-tone step"));
                    }
                    Family::Environment => {
                        check(outlines == 0, format!("level {level} draws {outlines} outlines"))
                    }
                }
                if let (Some((width, _)), Some((whole_width, _))) = (look.edge, whole.edge) {
                    // The bound is a hard step's. The environment's lit side rises from the shadow tone as N·L
                    // does, so on a small sphere the profile reads that slope (steepest about 0.1), at every
                    // level alike: held to level 0's and across the switch, as a step is.
                    if family == Family::Character {
                        check(width < edge_bound, format!("level {level}'s edge {width:.2} px"));
                    }
                    let edges = width.max(whole_width) / width.min(whole_width);
                    check(
                        edges < 1.3,
                        format!("level {level}'s edge {width:.2} px against level 0's {whole_width:.2}"),
                    );
                }
                check(
                    look.edge.is_some() == whole.edge.is_some(),
                    format!("level {level}'s edge {:?} against level 0's {:?}", look.edge, whole.edge),
                );
                let spread = look.spread / whole.spread;
                check(
                    (0.95..=1.05).contains(&spread),
                    format!("level {level}'s gradient spread {spread:.3} of level 0's"),
                );
            }
            if family == Family::Character {
                let strength = coarse_look.lines.strength / fine_look.lines.strength;
                check(
                    (0.95..1.05).contains(&strength),
                    format!("the coarser line {strength:.3} of the finer's"),
                );
            }
            if let (Some((fine_width, _)), Some((coarse_width, _))) = (fine_look.edge, coarse_look.edge) {
                let edges = coarse_width.max(fine_width) / coarse_width.min(fine_width);
                check(edges < 1.3, format!("the edges {fine_width:.2} and {coarse_width:.2} px"));
            }
            let spread = coarse_look.spread / fine_look.spread;
            check(
                (0.95..=1.05).contains(&spread),
                format!("the coarser gradient spread {spread:.3} of the finer's"),
            );
        }
    }
    failures
}

/// Runs each of `subjects` on every shipped tier, cooked for the tier's class, and fails with every switch
/// that didn't hold.
fn every_tier(subjects: impl Fn(TierId) -> Vec<(Family, Mesh, &'static str)>) {
    let gpu = gpu();
    let table = TierTable::shipped();
    let failures: Vec<String> = table
        .tiers()
        .iter()
        .flat_map(|tier| subjects(tier.id).into_iter().map(move |subject| (tier, subject)))
        .flat_map(|(tier, (family, mesh, label))| switches(&gpu, tier, family, &mesh, label))
        .collect();
    assert!(failures.is_empty(), "{} switches failed:\n{}", failures.len(), failures.join("\n"));
}

/// The chain a shipped model's sidecar cooks it with.
fn shipped_chain(sidecar: &str) -> LodSettings {
    let meta: AssetMeta = sapphire_serialize::from_ron(sidecar).expect("a sidecar");
    match meta.importer {
        ImportSettings::Gltf(mesh) => mesh.lods,
        ImportSettings::Vrm(vrm) => vrm.mesh.lods,
        _ => panic!("not a model's sidecar"),
    }
}

/// A character's line and two-tone edge at every step of the shipped characters' two chains, on every tier:
/// the enemies' (every kind of enemy, the bosses and the look-dev character cook the default chain; the
/// ambient cast cooks its first two steps) and the heroes'.
#[test]
#[ignore = "needs a GPU"]
fn a_characters_line_and_two_tone_edge_hold_at_each_switch() {
    let enemy = shipped_chain(include_str!("../../../../../content/combat/enemy/warrior.glb.meta"));
    let hero = shipped_chain(include_str!("../../../../../content/combat/hero/hero.vrm.meta"));
    assert_eq!(enemy, LodSettings::default(), "the enemies cook the default chain");
    every_tier(|tier| {
        let character =
            |chain, label| (Family::Character, cooked(Family::Character, chain, cook_of(tier), false), label);
        vec![character(&enemy, "enemy"), character(&hero, "hero")]
    });
}

/// A static mesh's two-tone response at every step of the default chain, on every tier, with no outline at any
/// level.
#[test]
#[ignore = "needs a GPU"]
fn a_static_meshs_two_tone_response_holds_at_each_switch() {
    every_tier(|tier| {
        vec![(
            Family::Environment,
            cooked(Family::Environment, &LodSettings::default(), cook_of(tier), false),
            "static",
        )]
    });
}

/// A zone cell's two-tone response at every step of the benchmark district's chain, cooked as a zone cooks a
/// cell, on every tier, with no outline at any level.
#[test]
#[ignore = "needs a GPU"]
fn a_cells_two_tone_response_holds_at_each_switch() {
    let district: BenchDistrict = sapphire_serialize::from_ron(DISTRICT).expect("the district's document");
    every_tier(|tier| {
        vec![(
            Family::Environment,
            cooked(Family::Environment, &district.cell_levels, cook_of(tier), true),
            "cell",
        )]
    });
}
