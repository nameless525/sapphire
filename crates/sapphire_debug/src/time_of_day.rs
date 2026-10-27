//! The time-of-day panel (M1 W1): scrub the hour, pause or run the cycle, see every resolved
//! value, and preview the character key light. [`TodControls`] carries its requests to the
//! world's [`TimeOfDay`].
//!
//! # The panel
//!
//! - **The day strip:** the base timeline across 24 hours, a column per quarter hour: the sky
//!   from zenith to horizon, and under it the key light's color and strength. Ticks mark the
//!   keys and a line the current hour. Clicking or dragging on it scrubs the hour.
//! - **The character preview:** a sphere in a skin tone, in front of the sky gradient, lit the
//!   way the anime character shader lights a character: a crisp two-tone split under the
//!   first character's resolved light rig (M1 C2: its key light, rim scale and stylized GI,
//!   the sphere taking the skin tint), an authored warm shade on the dark side and a rim. Seen
//!   from +Z in world axes, as the sandbox's camera starts. Without characters it previews time
//!   of day's character key from the world key light's direction, with plain ambient. It is
//!   rendered on the CPU into a 96 × 96 texture only when the values it reads change, so it
//!   costs nothing while the hour and the camera stand still, and, while either moves and the
//!   panel is open, one render and upload a frame: about 55 µs of CPU in a release build on an
//!   M5, with sRGB encoding through a table (direct encoding took 210 µs).
//! - **Resolved values:** the key light and where it comes from, the sun and moon, the sky and its
//!   layers (the discs and their glows, star brightness, the clouds' tones and coverage; M1 W2),
//!   ambient, fog (each layer's color and density, the far layer's as the horizon it tints, the
//!   sun's inscatter and the characters' share; M1 W3), shadow tint, character light, day/night
//!   factor and post-processing (with the grades post's two LUTs blend, and the compositing
//!   layers: the diffusion glow, characters' bloom, gradient layers and the lens flare, M1 P2),
//!   zone volumes and the cutscene, each character's resolved light rig, and the resolve's own
//!   CPU time against its 0.05 ms budget.

use std::sync::LazyLock;

use egui::{Color32, Mesh, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions, Ui, pos2, vec2};
use sapphire_math::Vec3;
use sapphire_math::color::linear_to_srgb;
use sapphire_render::{CharacterGi, CharacterInstance, CharacterLighting, DirectionalLight, SkyDisc};
use sapphire_world::tod::{DAY_HOURS, TimeOfDay, TodFrame, TodLook};

use crate::budget::format_ms;

/// Columns of the day strip: one per quarter hour.
const STRIP_COLUMNS: usize = 96;
/// The character preview's size in texels.
const PREVIEW_SIZE: usize = 96;
/// The day lengths the panel offers, in seconds: a quick look at the whole cycle, a 24-minute
/// day (Genshin's), and an hour.
const DAY_LENGTHS: [(f32, &str); 3] = [(120.0, "2 min"), (1440.0, "24 min"), (3600.0, "1 h")];
/// The resolve's CPU budget (M1 plan, W1).
const BUDGET: std::time::Duration = std::time::Duration::from_micros(50);

/// Developer control of time of day: the panel records what it wants, and the app applies it
/// to the world's [`TimeOfDay`] before each fixed step ([`Self::apply`]), the same hook as
/// [`TimeControls`](crate::TimeControls). It never allocates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TodControls {
    hours: Option<f32>,
    paused: Option<bool>,
    day_length: Option<Option<f32>>,
}

impl TodControls {
    /// Jumps to `hours`.
    pub fn scrub(&mut self, hours: f32) {
        self.hours = Some(hours);
    }

    /// Holds or releases the cycle (not the world).
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = Some(paused);
    }

    /// Runs a day in `seconds` of world time, or stops the cycle.
    pub fn set_day_length(&mut self, seconds: Option<f32>) {
        self.day_length = Some(seconds);
    }

    /// Applies what the panel asked for since the last call.
    pub fn apply(&mut self, time_of_day: &mut TimeOfDay) {
        let Self { hours, paused, day_length } = std::mem::take(self);
        let clock = time_of_day.clock_mut();
        if let Some(seconds) = day_length {
            clock.set_day_length(seconds);
        }
        if let Some(hours) = hours {
            clock.set_hours(hours);
        }
        if let Some(paused) = paused
            && !clock.set_paused(paused)
        {
            tracing::warn!(target: crate::LOG_TARGET, "time of day's clock has no room for a pause");
        }
    }
}

/// The panel's state across frames.
#[derive(Default)]
pub(crate) struct TodPanel {
    /// The day strip's colors and the timeline they were made from (its address and revision).
    strip: Option<((usize, u64), Vec<StripColumn>)>,
    preview: Option<(PreviewInputs, TextureHandle)>,
}

#[derive(Clone, Copy)]
struct StripColumn {
    zenith: Color32,
    horizon: Color32,
    key: Color32,
}

/// What the character preview is drawn from.
#[derive(Clone, Copy, PartialEq)]
struct PreviewInputs {
    look: TodLook,
    /// The first character's light rig, if the frame has characters.
    lighting: Option<CharacterLighting>,
}

/// The characters the values list shows.
const LISTED_CHARACTERS: usize = 4;

/// Shows the panel for `time_of_day` and the frame's `characters`, recording what the player
/// asks for in `controls`.
pub(crate) fn panel(
    ui: &mut Ui,
    state: &mut TodPanel,
    controls: &mut TodControls,
    time_of_day: Option<&TimeOfDay>,
    characters: &[CharacterInstance],
) {
    let Some(tod) = time_of_day else {
        ui.weak("The app runs no time of day.");
        return;
    };
    let frame = tod.frame();
    let clock = tod.clock();
    ui.horizontal(|ui| {
        ui.label(RichText::new(clock_time(frame.hours)).monospace().strong());
        let cycle = match clock.day_length() {
            Some(seconds) => format!("a day every {}", duration_text(seconds)),
            None => "time stands still".to_owned(),
        };
        ui.weak(cycle);
    });
    ui.horizontal_wrapped(|ui| {
        let paused = clock.is_paused();
        if ui
            .button(if paused { "Resume" } else { "Pause" })
            .on_hover_text("Holds the cycle, not the world")
            .clicked()
        {
            controls.set_paused(!paused);
        }
        let keys = tod.timeline().keys();
        let previous = keys.iter().rev().map(|key| key.time).find(|&time| time < frame.hours - 1e-3);
        let next = keys.iter().map(|key| key.time).find(|&time| time > frame.hours + 1e-3);
        if ui.button("Previous key").clicked() {
            controls.scrub(previous.unwrap_or(keys[keys.len() - 1].time));
        }
        if ui.button("Next key").clicked() {
            controls.scrub(next.unwrap_or(keys[0].time));
        }
        ui.separator();
        ui.label("Cycle");
        let current = clock.day_length();
        if ui.selectable_label(current.is_none(), "off").clicked() {
            controls.set_day_length(None);
        }
        for (seconds, label) in DAY_LENGTHS {
            let chosen = current.is_some_and(|length| (length - seconds).abs() < 0.5);
            if ui.selectable_label(chosen, label).clicked() {
                controls.set_day_length(Some(seconds));
            }
        }
    });
    day_strip(ui, state, controls, tod);
    let lighting = characters.first().map(|character| character.lighting);
    ui.horizontal(|ui| {
        preview(ui, state, frame, lighting);
        ui.vertical(|ui| values(ui, tod, characters));
    });
}

/// The day strip: the base timeline over 24 hours, scrubbed by clicking or dragging.
fn day_strip(ui: &mut Ui, state: &mut TodPanel, controls: &mut TodControls, tod: &TimeOfDay) {
    let source = (std::ptr::from_ref(tod.timeline()) as usize, tod.revision());
    if state.strip.as_ref().is_none_or(|(made_from, _)| *made_from != source) {
        let columns = (0..STRIP_COLUMNS)
            .map(|column| {
                let hours = (column as f32 + 0.5) * DAY_HOURS / STRIP_COLUMNS as f32;
                let look = tod.timeline().evaluate(hours);
                let key = look.key_light();
                StripColumn {
                    zenith: display(look.sky.zenith, look.exposure),
                    horizon: display(look.sky.horizon, look.exposure),
                    key: display(key.color * key.intensity, look.exposure),
                }
            })
            .collect();
        state.strip = Some((source, columns));
    }
    let Some((_, columns)) = &state.strip else { return };

    let width = ui.available_width().max(240.0);
    let (response, painter) = ui.allocate_painter(vec2(width, 52.0), Sense::click_and_drag());
    let rect = response.rect;
    let sky = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + 34.0));
    let key = Rect::from_min_max(pos2(rect.min.x, sky.max.y + 2.0), pos2(rect.max.x, sky.max.y + 10.0));
    let x = |hours: f32| rect.min.x + hours / DAY_HOURS * rect.width();
    let column_width = rect.width() / STRIP_COLUMNS as f32;
    let mut mesh = Mesh::default();
    for (index, column) in columns.iter().enumerate() {
        let (left, right) =
            (rect.min.x + index as f32 * column_width, rect.min.x + (index + 1) as f32 * column_width);
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(left, sky.min.y), column.zenith);
        mesh.colored_vertex(pos2(right, sky.min.y), column.zenith);
        mesh.colored_vertex(pos2(left, sky.max.y), column.horizon);
        mesh.colored_vertex(pos2(right, sky.max.y), column.horizon);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 3, base + 2);
        painter.rect_filled(Rect::from_x_y_ranges(left..=right, key.y_range()), 0.0, column.key);
    }
    painter.add(egui::Shape::mesh(mesh));
    let weak = ui.visuals().weak_text_color();
    for key in tod.timeline().keys() {
        let at = x(key.time);
        painter
            .line_segment([pos2(at, sky.min.y), pos2(at, sky.min.y + 5.0)], Stroke::new(2.0, Color32::WHITE));
    }
    for hour in [0.0, 6.0, 12.0, 18.0] {
        painter.text(
            pos2(x(hour) + 2.0, key.max.y + 1.0),
            egui::Align2::LEFT_TOP,
            clock_time(hour),
            egui::FontId::monospace(9.0),
            weak,
        );
    }
    let now = x(tod.frame().hours);
    painter.line_segment([pos2(now, sky.min.y), pos2(now, key.max.y)], Stroke::new(2.0, Color32::WHITE));
    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let hours = ((pointer.x - rect.min.x) / rect.width()).clamp(0.0, 0.9999) * DAY_HOURS;
        controls.scrub(hours);
    }
    response.on_hover_text("The day: sky (zenith to horizon) and the key light. Click or drag to scrub.");
}

/// The character preview: drawn again only when its inputs change.
fn preview(ui: &mut Ui, state: &mut TodPanel, frame: &TodFrame, lighting: Option<CharacterLighting>) {
    let inputs = PreviewInputs { look: frame.look, lighting };
    match &mut state.preview {
        Some((drawn, _)) if *drawn == inputs => {}
        Some((drawn, texture)) => {
            texture.set(render_preview(&frame.look, lighting.as_ref()), TextureOptions::LINEAR);
            *drawn = inputs;
        }
        None => {
            let texture = ui.ctx().load_texture(
                "sapphire_debug.tod_preview",
                render_preview(&frame.look, lighting.as_ref()),
                TextureOptions::LINEAR,
            );
            state.preview = Some((inputs, texture));
        }
    }
    if let Some((_, texture)) = &state.preview {
        let hover = if lighting.is_some() {
            "The first character's light rig on a skin-toned sphere, in the anime two-tone model"
        } else {
            "Time of day's character key on a skin-toned sphere, in the anime two-tone model"
        };
        ui.add(egui::Image::new((texture.id(), vec2(PREVIEW_SIZE as f32, PREVIEW_SIZE as f32))))
            .on_hover_text(hover);
    }
}

/// A skin-toned sphere in front of the sky, lit as the anime character shader lights
/// characters: a crisp split of N·L into the lit albedo and an authored warm shade under
/// `lighting`'s key light, its stylized GI as skin takes it, and a rim on the lit side. Without
/// a rig: time of day's character key from the key light's direction, and plain hemispherical
/// ambient. Exposure applies; the grade doesn't.
fn render_preview(look: &TodLook, lighting: Option<&CharacterLighting>) -> egui::ColorImage {
    // Linear skin and the authored shade multiplier anime skin takes on its dark side.
    const ALBEDO: Vec3 = Vec3::new(0.94, 0.66, 0.52);
    const SHADE: Vec3 = Vec3::new(0.78, 0.56, 0.62);
    const RADIUS: f32 = 0.72;
    let unstyled = CharacterLighting {
        key: DirectionalLight { direction: look.key_light().direction, ..DirectionalLight::default() },
        ..CharacterLighting::unstyled(DirectionalLight::default())
    };
    let (key, rim_scale, gi) = match lighting {
        Some(lighting) => (lighting.key, lighting.rim, lighting.gi),
        None => {
            let character = look.character_key();
            let key =
                DirectionalLight { color: character.color, intensity: character.intensity, ..unstyled.key };
            (key, unstyled.rim, CharacterGi::default())
        }
    };
    let toward_light = (-key.direction).try_normalize().unwrap_or(Vec3::Y);
    let light = key.color * key.intensity;
    let size = PREVIEW_SIZE as f32;
    let display = Display::new(look.exposure);
    // A split about a texel wide, as `crisp_step` makes it on screen.
    let edge = 1.5 / (size * RADIUS * 0.5);
    let pixels = (0..PREVIEW_SIZE * PREVIEW_SIZE)
        .map(|index| {
            let (x, y) = ((index % PREVIEW_SIZE) as f32, (index / PREVIEW_SIZE) as f32);
            let (u, v) = ((x + 0.5) / size * 2.0 - 1.0, 1.0 - (y + 0.5) / size * 2.0);
            let radius_squared = (u * u + v * v) / (RADIUS * RADIUS);
            let color = if radius_squared > 1.0 {
                sky(look, v)
            } else {
                // Seen from +Z, as the sandbox's camera sees the scene: world axes as they are.
                let normal = Vec3::new(u / RADIUS, v / RADIUS, (1.0 - radius_squared).sqrt());
                let lit = sapphire_math::ease::smoothstep(-edge, edge, normal.dot(toward_light));
                let diffuse = (ALBEDO * SHADE).lerp(ALBEDO, lit) * light;
                let ambient = ALBEDO * character_gi(look, &gi, normal);
                let rim = if 1.0 - normal.z > 0.75 {
                    light * ALBEDO.lerp(Vec3::ONE, 0.5) * 0.3 * rim_scale * (0.25 + 0.75 * lit)
                } else {
                    Vec3::ZERO
                };
                diffuse + ambient + rim
            };
            display.color(color)
        })
        .collect();
    egui::ColorImage::new([PREVIEW_SIZE, PREVIEW_SIZE], pixels)
}

/// The stylized GI skin facing `normal` takes from `look`'s ambient light, as the anime
/// character shader's `character_gi` gives it (the preview sphere is its own head, so the
/// sphere weight changes nothing): the normal bent toward the camera (+Z), the hemisphere, the
/// HSV clamp and the skin tint.
fn character_gi(look: &TodLook, gi: &CharacterGi, normal: Vec3) -> Vec3 {
    let ambient = &look.ambient;
    let bent = (normal + Vec3::Z * gi.camera_bias).try_normalize().unwrap_or(normal);
    let hemisphere = ambient.ground.lerp(ambient.sky, bent.y * 0.5 + 0.5) * ambient.intensity * gi.intensity;
    hsv_clamp(hemisphere, gi.max_saturation, gi.max_value) * gi.skin_tint
}

/// `color` with its HSV saturation held to `max_saturation` and its value to `max_value`, hue
/// kept (`hsv_clamp` in `anime_character.wgsl`).
fn hsv_clamp(color: Vec3, max_saturation: f32, max_value: f32) -> Vec3 {
    let value = color.max_element().max(1e-12);
    let saturation = 1.0 - color.min_element() / value;
    if saturation <= max_saturation && value <= max_value {
        return color;
    }
    let kept = if saturation > max_saturation { max_saturation / saturation } else { 1.0 };
    Vec3::splat(value).lerp(color, kept) * (value.min(max_value) / value)
}

/// The sky gradient behind the preview, at height `v` (−1 bottom, 1 top), as `sky_color` in
/// the renderer's `frame.wgsl` shapes it.
fn sky(look: &TodLook, v: f32) -> Vec3 {
    let sky = &look.sky;
    if v >= 0.0 {
        sky.horizon.lerp(sky.zenith, v.sqrt())
    } else {
        sky.horizon.lerp(sky.ground, (-v * 5.0).min(1.0))
    }
}

/// The resolved values, group by group, and the characters' light rigs.
fn values(ui: &mut Ui, tod: &TimeOfDay, characters: &[CharacterInstance]) {
    let look = &tod.frame().look;
    let exposure = look.exposure;
    egui::Grid::new("tod_values").num_columns(2).striped(true).show(ui, |ui| {
        let key = look.key_light();
        ui.label("Key light");
        ui.horizontal(|ui| {
            swatch(ui, key.color * key.intensity, exposure);
            let source = if key.direction == look.sun.direction {
                "the sun"
            } else if key.direction == look.moon.direction {
                "the moon"
            } else {
                "twilight: sun and moon"
            };
            ui.monospace(format!("{:.2}, {}, from {source}", key.intensity, position(&key)));
        });
        ui.end_row();
        for (name, body) in [("Sun", &look.sun), ("Moon", &look.moon)] {
            ui.label(name);
            ui.horizontal(|ui| {
                swatch(ui, body.color * body.intensity, exposure);
                ui.monospace(format!("{:.2}, {}", body.intensity, position(body)));
            });
            ui.end_row();
        }
        ui.label("Sky");
        ui.horizontal(|ui| {
            for color in [look.sky.zenith, look.sky.horizon, look.sky.ground] {
                swatch(ui, color, exposure);
            }
            ui.weak("zenith, horizon, ground");
        });
        ui.end_row();
        for (name, disc) in [("Sun disc", &look.sun_disc), ("Moon disc", &look.moon_disc)] {
            ui.label(name);
            ui.horizontal(|ui| {
                swatch(ui, disc.color * disc.intensity, exposure);
                swatch(ui, disc.glow, exposure);
                ui.monospace(disc_text(disc));
            });
            ui.end_row();
        }
        ui.label("Stars");
        ui.monospace(format!("{:.2}", look.stars));
        ui.end_row();
        ui.label("Nebula");
        ui.horizontal(|ui| {
            let nebula = &look.nebula;
            for color in [nebula.clouds, nebula.wisps, nebula.accents] {
                swatch(ui, color, exposure);
            }
        });
        ui.end_row();
        ui.label("Clouds");
        ui.horizontal(|ui| {
            let clouds = &look.clouds;
            for color in [clouds.lit, clouds.shade, clouds.rim] {
                swatch(ui, color, exposure);
            }
            ui.monospace(format!("cover {:.2}, cirrus {:.2}", clouds.coverage, clouds.cirrus));
        });
        ui.end_row();
        ui.label("Ambient");
        ui.horizontal(|ui| {
            let ambient = &look.ambient;
            swatch(ui, ambient.sky * ambient.intensity, exposure);
            swatch(ui, ambient.ground * ambient.intensity, exposure);
            ui.monospace(format!("{:.2}", ambient.intensity));
        });
        ui.end_row();
        // The fog as the renderer draws it (M1 W3): each layer's color, the far layer's as the
        // horizon it tints, and the sun's inscatter, then their densities and shapes.
        let fog = &look.fog;
        ui.label("Fog");
        ui.horizontal(|ui| {
            for color in [fog.near.color, fog.far.color * look.sky.horizon, fog.height.color, fog.sun_inscatter] {
                swatch(ui, color, exposure);
            }
            ui.monospace(format!(
                "near {:.4}, far {:.4}, height {:.3}",
                fog.near.density, fog.far.density, fog.height.density
            ));
        });
        ui.end_row();
        ui.label("");
        ui.monospace(format!(
            "from {:.0} m and {:.0} m; height ×e^(−{:.2}·(y − {:.1} m)); characters {:.0}%",
            fog.near.start,
            fog.far.start,
            fog.height.falloff,
            fog.height.base,
            fog.characters * 100.0
        ));
        ui.end_row();
        ui.label("Shadow tint");
        ui.horizontal(|ui| {
            swatch(ui, look.shadow_tint, 0.0);
            ui.monospace(rgb(look.shadow_tint));
        });
        ui.end_row();
        ui.label("Character key");
        ui.horizontal(|ui| {
            let light = look.character_key();
            swatch(ui, light.color * light.intensity, exposure);
            ui.monospace(format!("{:.2}", light.intensity));
        });
        ui.end_row();
        for (index, character) in characters.iter().take(LISTED_CHARACTERS).enumerate() {
            let lighting = &character.lighting;
            ui.label(if index == 0 { "Character rigs" } else { "" });
            ui.horizontal(|ui| {
                let key = &lighting.key;
                swatch(ui, key.color * key.intensity, exposure);
                ui.monospace(format!("{:.2}, {}", key.intensity, position(key)));
            });
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                let gi = &lighting.gi;
                swatch(ui, gi.skin_tint, 0.0);
                let value =
                    if gi.max_value.is_finite() { format!("{:.2}", gi.max_value) } else { "∞".to_owned() };
                ui.monospace(format!(
                    "GI ×{:.2}, camera bias {:.2}, saturation ≤ {:.2}, value ≤ {value}; rim ×{:.2}",
                    gi.intensity, gi.camera_bias, gi.max_saturation, lighting.rim
                ));
            });
            ui.end_row();
        }
        if characters.len() > LISTED_CHARACTERS {
            ui.label("");
            ui.weak(format!("and {} more characters", characters.len() - LISTED_CHARACTERS));
            ui.end_row();
        }
        if let Some(shot) = &tod.frame().shot_rig {
            ui.label("Shot rig");
            ui.monospace(format!("{:.0}% over the characters' own", shot.weight * 100.0));
            ui.end_row();
        }
        ui.label("Day/night");
        ui.monospace(format!("{:.2}", look.day_night));
        ui.end_row();
        // How much of the world's authored emission shows (M1 W1's keyed emission).
        ui.label("Emission");
        ui.monospace(format!("×{:.2}", look.emission));
        ui.end_row();
        let post = look.post();
        ui.label("Post");
        ui.monospace(format!(
            "exposure {:+.2}, bloom {:.2} over {:.2}, vignette {:.2}",
            post.exposure, post.bloom.intensity, post.bloom.threshold, post.vignette.intensity
        ));
        ui.end_row();
        ui.label("Compositing");
        let gradients = post.gradients.iter().filter(|layer| layer.opacity > 0.0).count();
        ui.monospace(format!(
            "diffusion {:.2} over {:.2}, characters' bloom ×{:.2} at ×{:.2}, {gradients} gradients, flare {:.2}",
            post.diffusion.intensity,
            post.diffusion.threshold,
            post.character_bloom.threshold,
            post.character_bloom.strength,
            post.lens_flare.intensity
        ));
        ui.end_row();
        ui.label("Grade");
        let grades = if post.grade_blend > 0.0 {
            format!(
                "two LUTs, {:.0}% B (saturation {:.2} to {:.2})",
                post.grade_blend * 100.0,
                post.grade_a.saturation,
                post.grade_b.saturation
            )
        } else {
            format!("one LUT (saturation {:.2})", post.grade_a.saturation)
        };
        ui.monospace(grades);
        ui.end_row();
        let stats = tod.stats();
        ui.label("Zones");
        ui.monospace(format!("{} of {} volumes weigh in", stats.active_volumes, stats.volumes));
        ui.end_row();
        ui.label("Cutscene");
        ui.monospace(match tod.cutscene() {
            Some((track, seconds)) => {
                format!("{:.0}% at {seconds:.1} of {:.1} s", stats.cutscene_weight * 100.0, track.duration())
            }
            None => "none".to_owned(),
        });
        ui.end_row();
        if stats.dropped_grade_weight > 0.0 {
            ui.label("Grades dropped");
            ui.monospace(format!(
                "{:.0}% of the picture: more than two grades",
                stats.dropped_grade_weight * 100.0
            ));
            ui.end_row();
        }
        ui.label("Resolve");
        let segment = stats.segment;
        let keys = tod.timeline().keys();
        let used = stats.resolve_time;
        let color = if used > BUDGET { crate::ui::OVER } else { crate::ui::GOOD };
        ui.horizontal(|ui| {
            ui.label(RichText::new(format_ms(used)).monospace().color(color));
            let hour =
                |index: usize| keys.get(index).map_or_else(|| "?".to_owned(), |key| clock_time(key.time));
            ui.monospace(format!(
                "of {}; keys {} to {}, {:.2}",
                format_ms(BUDGET),
                hour(segment.from),
                hour(segment.to),
                segment.weight
            ));
        });
        ui.end_row();
    });
}

/// A disc's readout: its brightness and its glow's size.
fn disc_text(disc: &SkyDisc) -> String {
    if disc.intensity <= 0.0 && disc.glow_radius <= 0.0 {
        return "hidden".to_owned();
    }
    format!("{:.1}, glow {:.0}°", disc.intensity, disc.glow_radius.to_degrees())
}

/// A color swatch: linear light, exposed, as the display would show it untoned.
fn swatch(ui: &mut Ui, color: Vec3, exposure: f32) {
    let (rect, response) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, display(color, exposure));
    response.on_hover_text(rgb(color));
}

/// Linear light exposed by `exposure` stops, clamped and encoded for display.
fn display(color: Vec3, exposure: f32) -> Color32 {
    Display::new(exposure).color(color)
}

/// Linear light to display colors: exposure, a clamp, then sRGB encoding through a table, so the
/// preview's 9,216 texels cost no transcendental functions.
struct Display {
    scale: f32,
}

impl Display {
    fn new(exposure: f32) -> Self {
        Self { scale: exposure.exp2() }
    }

    fn color(&self, linear: Vec3) -> Color32 {
        let exposed = linear * self.scale;
        Color32::from_rgb(encode(exposed.x), encode(exposed.y), encode(exposed.z))
    }
}

/// Entries in [`encode`]'s table: steps of linear light fine enough that the darks, where sRGB
/// is steepest, stay within a code value of direct encoding.
const ENCODE_STEPS: usize = 4096;

/// Linear `[0, 1]` (clamped; NaN is 0) to an sRGB-encoded byte, through a table.
fn encode(value: f32) -> u8 {
    static TABLE: LazyLock<[u8; ENCODE_STEPS]> = LazyLock::new(|| {
        std::array::from_fn(|step| {
            (linear_to_srgb(step as f32 / (ENCODE_STEPS - 1) as f32) * 255.0).round() as u8
        })
    });
    // The cast saturates, and takes NaN to 0.
    let step = (value.clamp(0.0, 1.0) * (ENCODE_STEPS - 1) as f32).round() as usize;
    TABLE[step.min(ENCODE_STEPS - 1)]
}

fn rgb(color: Vec3) -> String {
    format!("({:.2}, {:.2}, {:.2})", color.x, color.y, color.z)
}

/// Where a light comes from: its elevation above the horizon and its azimuth (0° toward +Z, the
/// sandbox camera's side; positive toward +X, the right).
fn position(light: &DirectionalLight) -> String {
    let toward = (-light.direction).try_normalize().unwrap_or(Vec3::Y);
    // Adding 0 turns -0 into 0, which would print as "-0°".
    let elevation = toward.y.clamp(-1.0, 1.0).asin().to_degrees() + 0.0;
    let azimuth = toward.x.atan2(toward.z).to_degrees() + 0.0;
    format!("elevation {elevation:+.0}°, azimuth {azimuth:+.0}°")
}

/// `HH:MM`.
fn clock_time(hours: f32) -> String {
    let minutes = (hours * 60.0).round() as u32 % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn duration_text(seconds: f32) -> String {
    if seconds >= 120.0 { format!("{:.0} min", seconds / 60.0) } else { format!("{seconds:.0} s") }
}

#[cfg(test)]
mod tests {
    use sapphire_time::ScaledClock;
    use sapphire_world::tod::default_day;

    use super::*;

    fn time_of_day() -> (ScaledClock, TimeOfDay) {
        let mut world = ScaledClock::new();
        let clock = world.new_local_clock();
        (world, TimeOfDay::new(default_day(), clock, 10.0).expect("complete"))
    }

    #[test]
    fn controls_apply_once_and_allocate_nothing() {
        let (mut world, mut tod) = time_of_day();
        let mut controls = TodControls::default();
        controls.set_day_length(Some(24.0));
        controls.scrub(18.0);
        controls.set_paused(true);
        let ((), counts) = sapphire_core::alloc::count_in_scope(|| controls.apply(&mut tod));
        if sapphire_core::alloc::PER_THREAD_COUNTS {
            assert_eq!(counts.allocations, 0);
        }
        assert_eq!(controls, TodControls::default(), "applied requests are cleared");
        assert_eq!(tod.clock().hours(0.0), 18.0);
        assert!(tod.clock().is_paused());
        world.tick();
        tod.tick(&world);
        assert_eq!(tod.clock().hours(0.0), 18.0, "paused");
        controls.set_paused(false);
        controls.apply(&mut tod);
        world.tick();
        tod.tick(&world);
        assert!(tod.clock().hours(0.0) > 18.0, "running: a day in 24 s");
    }

    #[test]
    fn readouts() {
        assert_eq!(clock_time(18.5), "18:30");
        assert_eq!(clock_time(23.999), "00:00");
        assert_eq!(duration_text(1440.0), "24 min");
        let setting =
            DirectionalLight { direction: Vec3::new(-1.0, 0.0, 0.0), ..DirectionalLight::default() };
        assert_eq!(position(&setting), "elevation +0°, azimuth +90°", "on the horizon to the right");
        assert_eq!(disc_text(&SkyDisc::default()), "hidden");
        let sun = SkyDisc { intensity: 40.0, glow_radius: 22f32.to_radians(), ..SkyDisc::default() };
        assert_eq!(disc_text(&sun), "40.0, glow 22°");
        assert_eq!(display(Vec3::ONE, 0.0), Color32::WHITE);
        assert_eq!(display(Vec3::splat(0.5), 1.0), Color32::WHITE, "one stop up");
        assert_eq!(display(Vec3::splat(f32::NAN), 0.0), Color32::BLACK);
        // The table stays within a code value of direct encoding, darks included.
        for step in 0..=10_000 {
            let linear = step as f32 / 10_000.0;
            let direct = (linear_to_srgb(linear) * 255.0).round();
            assert!((f32::from(encode(linear)) - direct).abs() <= 1.0, "{linear}");
        }
    }

    #[test]
    fn the_preview_splits_into_lit_and_shade() {
        let (_, tod) = time_of_day();
        let image = render_preview(&tod.frame().look, None);
        assert_eq!(image.size, [PREVIEW_SIZE, PREVIEW_SIZE]);
        // The 10:00 sun comes from the upper left, toward the viewer: the sphere's upper left is
        // lit, its lower right in shade, and the corner is sky.
        let at = |x: usize, y: usize| image.pixels[y * PREVIEW_SIZE + x];
        let brightness = |color: Color32| u32::from(color.r()) + u32::from(color.g()) + u32::from(color.b());
        let (lit, shade) = (at(36, 36), at(62, 62));
        assert!(brightness(lit) > brightness(shade) + 60, "{lit:?} vs {shade:?}");
        assert_ne!(at(0, 0), lit);

        // A character's rig turns the split: its key from the lower right.
        let mut lighting = CharacterLighting::unstyled(DirectionalLight::default());
        lighting.key.direction = Vec3::new(-0.7, 0.7, 0.1);
        let rigged = render_preview(&tod.frame().look, Some(&lighting));
        let at = |x: usize, y: usize| rigged.pixels[y * PREVIEW_SIZE + x];
        assert!(brightness(at(62, 62)) > brightness(at(36, 36)) + 60, "lit from the lower right");
    }

    #[test]
    fn the_preview_clamp_matches_the_shaders() {
        let orange = Vec3::new(0.8, 0.2, 0.1);
        assert_eq!(hsv_clamp(orange, 1.0, f32::INFINITY), orange, "within: unchanged");
        let held = hsv_clamp(orange, 0.5, 0.4);
        let expected = Vec3::new(0.4, 0.4 - 0.3 * 0.5 / 0.875, 0.4 - 0.35 * 0.5 / 0.875);
        assert!(held.abs_diff_eq(expected, 1e-6), "{held}");
    }
}
