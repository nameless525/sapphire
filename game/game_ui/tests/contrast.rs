//! Sufficient contrast (M4 A8): every screen's text against what's drawn under it, as WCAG 2.2's success
//! criterion 1.4.3 measures it, and as Apple's Sufficient Contrast label asks of an app's text (`docs/store/
//! apple.md`). A text's color is held to 4.5:1 against its plate, or 3:1 where it's large: 18 points, or 14
//! points and bold (CSS's 24 and 18.66 pixels), a UI point standing for a CSS pixel.
//!
//! **What's measured is what's drawn.** Each screen, alone and settled at 1280×720 in English, draws into a
//! list as a frame does, with focus on each of its rows in turn, so a row's rest look and its focused look
//! both count. Each of a text's glyphs is a sample: everything drawn under its middle before it (the shapes'
//! fields, fills, gradients, outlines, cuts and sweeps, each as `ui.wgsl` draws it, in linear light) is its
//! plate. Shadows aren't a plate, as WCAG doesn't count them. A plate with no opaque fill under it lets the
//! world through, which can be anything from black to white, so its worst case counts; with nothing drawn
//! under it at all, a text over the world is 1:1 at worst. A press's own look, held only while a press lasts,
//! isn't measured.
//!
//! **What fails is named, not changed:** a color is a look call. [`KNOWN_SHORT`] lists every text that falls
//! short today with its ratio; the test holds every other text to its bar and every listed one to still being
//! short, so a fix has to take it off the list.

mod support;

use game_ui::boot::BootModel;
use game_ui::menus::{GameMenus, HudModel, MenuScreen, ScreenModel};
use game_ui::screen::{ScreenStack, StackSetup};
use game_ui::wipe::WipeModel;
use sapphire_assets::ui::UiDocument;
use sapphire_look::{UI_COVERAGE, UI_FIELD, UI_MULTI_FIELD, UI_PICTURE, UiAtlas, UiDrawList, UiQuad};
use sapphire_platform::{Scancode, Timestamp};
use sapphire_ui::{Atlas, UiTree};
use support::walk::{Device, Walk};
use support::{assets, content, document_in, table};

/// The frame every screen is measured at, in points.
const SIZE: (u32, u32) = (1280, 720);
/// WCAG 1.4.3's bars.
const NORMAL: f32 = 4.5;
const LARGE: f32 = 3.0;

/// Every text short of its bar today: its screen, its node, and its worst ratio as measured (held within 0.1).
/// Each is a look call, reported in M4 A8's hand-back: the title's tagline, every row's amber value on its
/// focused gradient, and the HUD's texts, drawn over the world with no plate (1:1 at worst, their shadows not
/// counting), its banner and the subtitle band at its default.
const KNOWN_SHORT: &[(&str, &str, f32)] = &[
    ("title", "tagline", 3.20),
    ("settings", "language-value", 3.78),
    ("settings", "display-value", 3.87),
    ("settings", "graphics-value", 3.87),
    ("settings", "sound-value", 3.87),
    ("settings", "subtitles-value", 3.87),
    ("settings", "controls-value", 3.87),
    ("settings", "privacy-value", 3.87),
    ("display", "size-value", 3.78),
    ("display", "text-value", 3.78),
    ("display", "shake-value", 3.80),
    ("display", "flashes-value", 3.78),
    ("display", "camera-value", 3.80),
    ("display", "timing-value", 3.83),
    ("graphics", "quality-value", 3.74),
    ("graphics", "cap-value", 3.75),
    ("graphics", "dynamic-value", 3.83),
    ("graphics", "saver-value", 3.83),
    ("sound", "volume-value", 3.80),
    ("sound", "music-value", 3.80),
    ("sound", "effects-value", 3.80),
    ("sound", "voice-value", 3.80),
    ("sound", "mono-value", 3.83),
    ("sound", "rumble-value", 3.84),
    ("subtitles", "shown-value", 3.84),
    ("subtitles", "size-value", 3.78),
    ("subtitles", "background-value", 3.78),
    ("subtitles", "speakers-value", 3.84),
    ("controls", "auto-target-value", 3.78),
    ("controls", "hold-light-value", 3.82),
    ("controls", "hold-heavy-value", 3.82),
    ("controls", "hold-dodge-value", 3.82),
    ("controls", "hold-skill-value", 3.82),
    ("controls", "hold-lock-on-value", 3.82),
    ("controls", "remap-value", 3.87),
    ("remap_pad", "light-value", 3.86),
    ("remap_pad", "heavy-value", 3.86),
    ("remap_pad", "dodge-value", 3.86),
    ("remap_pad", "skill-value", 3.86),
    ("remap_pad", "ultimate-value", 3.85),
    ("remap_pad", "swap-value", 3.85),
    ("remap_pad", "lock-on-value", 3.85),
    ("remap_pad", "jump-value", 3.85),
    ("remap_keys", "move-forward-value", 3.86),
    ("remap_keys", "move-back-value", 3.86),
    ("remap_keys", "move-left-value", 3.86),
    ("remap_keys", "move-right-value", 3.87),
    ("remap_keys", "light-value", 3.87),
    ("remap_keys", "heavy-value", 3.87),
    ("remap_keys", "dodge-value", 3.81),
    ("remap_keys", "skill-value", 3.87),
    ("remap_keys", "ultimate-value", 3.86),
    ("remap_keys", "swap-value", 3.84),
    ("remap_keys", "lock-on-value", 3.87),
    ("remap_keys", "jump-value", 3.80),
    ("hud", "player-name", 1.00),
    ("hud", "player-level", 1.00),
    ("hud", "enemy-name", 1.00),
    ("hud", "objective-label", 1.00),
    ("hud", "objective-text", 1.00),
    ("hud", "banner-text", 2.12),
    ("hud", "subtitle-speaker", 1.38),
    ("hud", "subtitle-line", 2.08),
];

/// A color in linear light, premultiplied by its alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Linear {
    rgb: [f32; 3],
    alpha: f32,
}

impl Linear {
    /// A display-encoded color with straight alpha, as a quad holds one.
    fn of(color: [u8; 4]) -> Self {
        let decode = |code: u8| {
            let value = f32::from(code) / 255.0;
            if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
        };
        let alpha = f32::from(color[3]) / 255.0;
        Self { rgb: [0, 1, 2].map(|at| decode(color[at])), alpha }.straight_to_premultiplied()
    }

    fn straight_to_premultiplied(self) -> Self {
        Self { rgb: self.rgb.map(|value| value * self.alpha), alpha: self.alpha }
    }

    fn mix(self, other: Self, share: f32) -> Self {
        let lerp = |a: f32, b: f32| a + (b - a) * share;
        Self {
            rgb: [0, 1, 2].map(|at| lerp(self.rgb[at], other.rgb[at])),
            alpha: lerp(self.alpha, other.alpha),
        }
    }

    fn scaled(self, by: f32) -> Self {
        Self { rgb: self.rgb.map(|value| value * by), alpha: self.alpha * by }
    }

    /// This over `under`, which is opaque.
    fn over(self, under: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|at| self.rgb[at] + under[at] * (1.0 - self.alpha))
    }
}

/// WCAG's relative luminance of an opaque color in linear light.
fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

fn ratio(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn ramp(value: f32, edge: f32, width: f32) -> f32 {
    ((value - edge) / width + 0.5).clamp(0.0, 1.0)
}

/// `ui.wgsl`'s three-slice.
fn slice(x: f32, cap: f32) -> f32 {
    if cap <= 0.0 {
        return x;
    }
    let cap = cap.max(1e-6);
    if x < cap {
        x / cap * 0.5
    } else if x > 1.0 - cap {
        1.0 - (1.0 - x) / cap * 0.5
    } else {
        0.5
    }
}

/// A single-channel field's value at `uv`, filtered as the renderer's sampler is.
fn field_at(atlas: &UiAtlas, uv: [f32; 2]) -> f32 {
    let (width, height) = (atlas.width as i64, atlas.height as i64);
    let x = uv[0] * atlas.width as f32 - 0.5;
    let y = uv[1] * atlas.height as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let texel = |x: i64, y: i64| {
        let (x, y) = (x.clamp(0, width - 1), y.clamp(0, height - 1));
        f32::from(atlas.texels[(y * width + x) as usize]) / 255.0
    };
    let (x0, y0) = (x0 as i64, y0 as i64);
    let top = texel(x0, y0) * (1.0 - fx) + texel(x0 + 1, y0) * fx;
    let bottom = texel(x0, y0 + 1) * (1.0 - fx) + texel(x0 + 1, y0 + 1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Where `point` is across and down `quad`, 0 to 1 along its own axes, if it's on it.
fn local(quad: &UiQuad, point: [f32; 2]) -> Option<[f32; 2]> {
    let [ax, ay] = [quad.axis_x, quad.axis_y];
    let det = ax[0] * ay[1] - ax[1] * ay[0];
    if det.abs() < 1e-9 {
        return None;
    }
    let d = [point[0] - quad.origin[0], point[1] - quad.origin[1]];
    let s = (d[0] * ay[1] - d[1] * ay[0]) / det;
    let t = (ax[0] * d[1] - ax[1] * d[0]) / det;
    ((0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t)).then_some([s, t])
}

/// What a shape's quad lays at `point`, as `ui.wgsl` shades it, premultiplied.
fn shade(quad: &UiQuad, atlas: &UiAtlas, point: [f32; 2]) -> Linear {
    let Some([s, t]) = local(quad, point) else { return Linear::default() };
    let across = slice(s, quad.slice);
    let uv = [quad.uv[0] + (quad.uv[2] - quad.uv[0]) * across, quad.uv[1] + (quad.uv[3] - quad.uv[1]) * t];
    let field = field_at(atlas, uv);
    let mut color = Linear::of(quad.color);
    if quad.gradient != [0.0; 3] {
        let share = (quad.gradient[0] * s + quad.gradient[1] * t + quad.gradient[2]).clamp(0.0, 1.0);
        color = color.mix(Linear::of(quad.color_end), share);
    }
    // No screen-space slope here: an edge is as sharp as its softness lets it be.
    let width = (quad.softness * 2.0).max(1e-3);
    let inner = ramp(field, quad.edge, width);
    let mut shaded = color.scaled(inner);
    if quad.outline_color[3] != 0 {
        let outer = ramp(field, quad.edge - quad.outline, width);
        let line = Linear::of(quad.outline_color).scaled((outer - inner).max(0.0));
        shaded = Linear {
            rgb: [0, 1, 2].map(|at| shaded.rgb[at] + line.rgb[at]),
            alpha: shaded.alpha + line.alpha,
        };
    }
    let cut = (quad.clip[0] <= 0.0 || s >= quad.clip[0]) && (quad.clip[1] >= 1.0 || s <= quad.clip[1]);
    let swept = quad.sweep <= 0.0 || {
        let turn = ((s - 0.5).atan2(-(t - 0.5)) / std::f32::consts::TAU).rem_euclid(1.0);
        turn < quad.sweep
    };
    if cut && swept { shaded } else { Linear::default() }
}

/// Whether quad `at` of `list` draws at `point`, under the masks.
fn unmasked(list: &UiDrawList, at: usize, point: [f32; 2]) -> bool {
    let mask = list.masks.iter().take_while(|mask| mask.first as usize <= at).last();
    mask.and_then(|mask| mask.rect)
        .is_none_or(|[x0, y0, x1, y1]| (x0..=x1).contains(&point[0]) && (y0..=y1).contains(&point[1]))
}

/// What's under `point` before quad `before`: the plate over the world as black and as white, the two ends of
/// what it can be.
fn plate(list: &UiDrawList, before: usize, point: [f32; 2]) -> [[f32; 3]; 2] {
    let atlas = list.atlas.as_deref().expect("a list with quads has an atlas");
    let mut ends = [[0.0; 3], [1.0; 3]];
    for (at, quad) in list.quads[..before].iter().enumerate() {
        if !unmasked(list, at, point) {
            continue;
        }
        match quad.texels {
            UI_FIELD => {
                let shaded = shade(quad, atlas, point);
                ends = ends.map(|under| shaded.over(under));
            }
            // An inset's picture is a scene's, as the world is: anything.
            UI_PICTURE if local(quad, point).is_some() => ends = [[0.0; 3], [1.0; 3]],
            _ => {}
        }
    }
    ends
}

/// A text's worst contrast at one glyph: drawn in `color` over the plate's two ends. Where the text is lighter
/// than one end and darker than the other, there's a world it can't be read on.
fn worst(color: [u8; 4], ends: [[f32; 3]; 2]) -> f32 {
    let text = Linear::of(color);
    let [low, high] = ends.map(|under| {
        let (plate, text) = (luminance(under), luminance(text.over(under)));
        (text - plate, ratio(text, plate))
    });
    if low.0.signum() != high.0.signum() { 1.0 } else { low.1.min(high.1) }
}

/// One text, measured.
#[derive(Clone, Debug)]
struct Measured {
    screen: String,
    node: String,
    ratio: f32,
    large: bool,
    /// What was showing when it was worst: the row with focus.
    focus: Option<String>,
}

impl Measured {
    fn bar(&self) -> f32 {
        if self.large { LARGE } else { NORMAL }
    }
}

/// Whether a style of `points` and `weight` is WCAG's large text.
fn is_large(points: f32, weight: f32) -> bool {
    points >= 24.0 || (points >= 18.66 && weight >= 700.0)
}

/// Every text `tree` shows, measured against what `list` draws under it, into `measured` (the worst of each
/// node's kept). `scale` is pixels per point.
fn measure(
    screen: &str,
    document: &UiDocument,
    tree: &UiTree,
    list: &UiDrawList,
    scale: f32,
    measured: &mut Vec<Measured>,
) {
    let focus = tree.focus_name().map(str::to_owned);
    for (name, pixels) in tree.text_pixels() {
        let node = document.nodes.iter().find(|node| node.name == name).expect("the document's node");
        let text = node.text.as_ref().expect("a text");
        let weight =
            document.styles.iter().find(|style| style.name == text.style).map_or(400.0, |s| s.weight);
        let rect = tree.screen_rect(tree.find(name).expect("its node"));
        let glyphs: Vec<usize> = list
            .quads
            .iter()
            .enumerate()
            .filter(|(_, quad)| matches!(quad.texels, UI_COVERAGE | UI_MULTI_FIELD))
            .filter(|(_, quad)| quad.color[..3] == text.color[..3] && quad.color[3] > 0)
            .filter(|(_, quad)| {
                let middle = middle(quad);
                (rect.min.x - 1.0..=rect.max.x + 1.0).contains(&middle[0])
                    && (rect.min.y - 1.0..=rect.max.y + 1.0).contains(&middle[1])
            })
            .map(|(at, _)| at)
            .collect();
        let Some(&first) = glyphs.first() else { continue };
        let ratio = glyphs
            .iter()
            .map(|&at| {
                let quad = &list.quads[at];
                worst(quad.color, plate(list, first, middle(quad)))
            })
            .fold(f32::INFINITY, f32::min);
        let one = Measured {
            screen: screen.to_owned(),
            node: name.to_owned(),
            ratio,
            large: is_large(pixels / scale, weight),
            focus: focus.clone(),
        };
        match measured.iter_mut().find(|held| held.screen == screen && held.node == name) {
            Some(held) if held.ratio > one.ratio => *held = one,
            Some(_) => {}
            None => measured.push(one),
        }
    }
}

fn middle(quad: &UiQuad) -> [f32; 2] {
    [
        quad.origin[0] + (quad.axis_x[0] + quad.axis_y[0]) * 0.5,
        quad.origin[1] + (quad.axis_x[1] + quad.axis_y[1]) * 0.5,
    ]
}

/// Each of the game's screens alone, settled, with focus on each of its rows in turn by the keyboard's down.
fn measure_menus(measured: &mut Vec<Measured>) {
    let documents = support::menus();
    let mut walk = Walk::new(assets(), "en", false, SIZE, Device::Keyboard);
    for entry in &documents.screens {
        let screen = MenuScreen::named(&entry.name);
        let document = document_in(&entry.name, &[content().join("ui")]);
        let menus: &mut GameMenus = &mut walk.menus;
        menus.close_all();
        menus.close(MenuScreen::HUD);
        walk.settle();
        walk.menus.open(screen);
        if entry.model == ScreenModel::Hud {
            let hud = walk.menus.model_mut::<HudModel>(screen).expect("the HUD's model");
            hud.has_banner = true;
            hud.can_skip = true;
            hud.has_objective = true;
            hud.objective = "hud-objective-arena".to_owned();
            hud.has_enemy = true;
            hud.enemy = "enemy-warrior".to_owned();
            hud.follow_subtitle(Some("subtitle-court-gate-warning"), Some("speaker-warden"));
        }
        walk.settle();
        let scale = walk.menus.settings().scale();
        let mut seen = Vec::new();
        loop {
            let tree = walk.tree(screen);
            measure(&entry.name, &document, tree, &walk.list, scale, measured);
            let focus = tree.focus_name().map(str::to_owned);
            if focus.is_none() || seen.contains(&focus) || seen.len() > 40 {
                break;
            }
            seen.push(focus);
            walk.key(Scancode::DOWN);
            for _ in 0..12 {
                walk.frame();
            }
            if !walk.menus.is_open(screen) {
                break;
            }
        }
    }
}

/// The boot screen and the zone's wipe, which the menus don't hold: each on a stack of its own, as the game
/// draws them, settled, the boot at a sample's step and the wipe covered and slow.
fn measure_boot_and_wipe(measured: &mut Vec<Measured>) {
    for name in ["boot", "zone_wipe"] {
        let support::Screens { assets, .. } = assets();
        let setup = StackSetup { locale: "en".to_owned(), pseudo: false, scale: 1.0 };
        let mut stack =
            ScreenStack::new(vec![table("en"), table("ja")], assets.fonts, Atlas::paint(), setup).unwrap();
        let document = document_in(name, &[content().join("ui")]);
        let model: Box<dyn bevy_reflect::PartialReflect> = if name == "boot" {
            Box::new(BootModel::sample())
        } else {
            Box::new(WipeModel { slow: true, progress: 0.5 })
        };
        let at = |frame: u64| Timestamp::from_nanos(1_000_000_000 + frame * 16_666_667);
        stack.push(&document, model, at(0), Vec::new()).unwrap();
        let mut list = UiDrawList::default();
        for frame in 1..=90 {
            stack.set_now(at(frame));
            list.reset();
            stack.draw(SIZE, &mut list);
            if frame == 1 && name == "zone_wipe" {
                stack.tree_mut(0).fire("cover", at(frame));
            }
        }
        measure(name, &document, stack.tree(0), &list, 1.0, measured);
    }
}

#[test]
fn every_screens_text_has_sufficient_contrast_against_its_plate() {
    let mut measured = Vec::new();
    measure_menus(&mut measured);
    measure_boot_and_wipe(&mut measured);
    for screen in
        support::menus().screens.iter().map(|entry| entry.name.as_str()).chain(["boot", "zone_wipe"])
    {
        assert!(measured.iter().any(|one| one.screen == screen), "{screen}: no text measured");
    }

    let mut report = String::new();
    for one in &measured {
        let short = one.ratio < one.bar();
        report.push_str(&format!(
            "{}{}/{}: {:.2}:1 against {}:1{}{}\n",
            if short { "SHORT " } else { "" },
            one.screen,
            one.node,
            one.ratio,
            one.bar(),
            if one.large { " (large)" } else { "" },
            one.focus.as_deref().map(|focus| format!(", focus on {focus}")).unwrap_or_default(),
        ));
    }

    let mut wrong = Vec::new();
    for one in &measured {
        let known = KNOWN_SHORT.iter().find(|(screen, node, _)| *screen == one.screen && *node == one.node);
        match known {
            None if one.ratio < one.bar() => wrong.push(format!(
                "{}/{} is {:.2}:1, short of {}:1",
                one.screen,
                one.node,
                one.ratio,
                one.bar()
            )),
            Some(_) if one.ratio >= one.bar() => wrong.push(format!(
                "{}/{} passes now at {:.2}:1: take it off the list",
                one.screen, one.node, one.ratio
            )),
            Some((_, _, was)) if (one.ratio - was).abs() > 0.1 => {
                wrong.push(format!("{}/{} moved from {was:.1} to {:.2}:1", one.screen, one.node, one.ratio))
            }
            _ => {}
        }
    }
    for (screen, node, _) in KNOWN_SHORT {
        if !measured.iter().any(|one| one.screen == *screen && one.node == *node) {
            wrong.push(format!("{screen}/{node} is listed and wasn't measured"));
        }
    }
    assert!(wrong.is_empty(), "{}\n\nall:\n{report}", wrong.join("\n"));
}

/// The measure itself: an opaque plate gives the ratio its colors give, a plate the world shows through takes
/// its worst world, and text with nothing under it can't be read on a world its own color.
#[test]
fn the_measure_reads_ratios_as_wcag_does() {
    // White on black is 21:1, and #767676 on white the familiar 4.54:1.
    assert!((worst([255; 4], [[0.0; 3]; 2]) - 21.0).abs() < 0.01);
    let grey = Linear::of([118, 118, 118, 255]).rgb;
    assert!((ratio(luminance(grey), 1.0) - 4.54).abs() < 0.01);
    // Nothing under it: a world as light as the text.
    assert_eq!(worst([240, 246, 255, 255], [[0.0; 3], [1.0; 3]]), 1.0);
    // Half black over the world: white text, at worst over a white world.
    let half = Linear::of([0, 0, 0, 128]);
    let ends = [[0.0; 3], [1.0; 3]].map(|world| half.over(world));
    let at_worst = worst([255; 4], ends);
    assert!(at_worst > 1.5 && at_worst < 2.5, "{at_worst}");
    assert!(
        is_large(24.0, 400.0) && is_large(18.7, 700.0) && !is_large(18.7, 500.0) && !is_large(20.0, 400.0)
    );
}
