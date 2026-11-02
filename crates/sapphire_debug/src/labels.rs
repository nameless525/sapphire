//! Debug draw's labels (M2 T1, `sapphire_core::debug_draw`): text beside a world point, painted on
//! egui's background layer, so the panels stay over them. egui can't read the frame's depth, so a
//! label draws over everything, whatever its style's depth; one behind the camera doesn't draw.
//! Here too is what keeps debug draw on in the builds that link this crate ([`sync_debug_draw`]).

use egui::{Color32, FontId, Pos2, Vec2};
use sapphire_core::debug_draw::{Color, DebugLabel};
use sapphire_math::Mat4;
use sapphire_render::CameraData;

/// Points from a label's point to its text.
const OFFSET: f32 = 6.0;

/// The text's size, in points.
const FONT_SIZE: f32 = 12.0;

/// Paints `labels`, seen through `camera`, on `ctx`'s screen.
pub(crate) fn paint(ctx: &egui::Context, camera: &CameraData, labels: &[DebugLabel]) {
    let painter = ctx.layer_painter(egui::LayerId::background());
    let screen = ctx.content_rect();
    let clip_from_world = camera.proj * camera.view;
    let font = FontId::monospace(FONT_SIZE);
    for label in labels {
        let Some(at) = project(clip_from_world, label, screen.size()) else { continue };
        let galley = painter.layout_no_wrap(label.text.as_str().to_owned(), font.clone(), color(label.color));
        let top_left = Pos2::new(at.x + OFFSET, at.y - galley.size().y * 0.5);
        let backing = egui::Rect::from_min_size(top_left, galley.size()).expand(1.0);
        painter.rect_filled(backing, 2.0, Color32::from_black_alpha(150));
        painter.galley(top_left, galley, Color32::WHITE);
    }
}

/// Where `label`'s point lands on a screen of `size` points, or `None` behind the camera.
fn project(clip_from_world: Mat4, label: &DebugLabel, size: Vec2) -> Option<Pos2> {
    let clip = clip_from_world * label.position.extend(1.0);
    (clip.w > 0.0).then(|| {
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        Pos2::new((x * 0.5 + 0.5) * size.x, (0.5 - y * 0.5) * size.y)
    })
}

pub(crate) fn color(color: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// Turns debug draw on or off as [`DRAW`](crate::DRAW) says, when that changes.
pub(crate) fn sync_debug_draw() {
    let on = crate::DRAW.get();
    if sapphire_core::debug_draw::enabled() != on {
        sapphire_core::debug_draw::set_enabled(on);
    }
}

#[cfg(test)]
mod tests {
    use sapphire_core::debug_draw::LabelText;
    use sapphire_math::Vec3;

    use super::*;

    fn at(position: Vec3) -> Option<Pos2> {
        let camera = CameraData::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y, 1.0, 2.0, 0.05);
        let label =
            DebugLabel { position, color: Color::WHITE, text: LabelText::format(format_args!("hit")) };
        project(camera.proj * camera.view, &label, Vec2::new(800.0, 400.0))
    }

    #[test]
    fn a_label_lands_where_its_point_projects() {
        let middle = at(Vec3::ZERO).expect("in front of the camera");
        assert!((middle.x - 400.0).abs() < 1e-3 && (middle.y - 200.0).abs() < 1e-3, "{middle:?}");
        let up_right = at(Vec3::new(1.0, 1.0, 0.0)).expect("in front of the camera");
        assert!(up_right.x > 400.0, "{up_right:?}");
        assert!(up_right.y < 200.0, "screen rows grow downward: {up_right:?}");
    }

    #[test]
    fn a_label_behind_the_camera_isnt_placed() {
        assert_eq!(at(Vec3::new(0.0, 0.0, 6.0)), None);
    }
}
