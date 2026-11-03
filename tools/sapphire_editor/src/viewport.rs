//! The viewport: the real renderer, through `sapphire_stage`, in a pane, on the window's device.
//!
//! There's one route through the renderer, the stage's: [`stand`] makes the stage a game's fight stands on, and
//! [`StageView::draw`] hands it a texture of the pane's size. A frame runs in this order: egui's pass says where
//! the pane is ([`ViewportPane`], which holds nothing of the GPU's), the stage draws at that size, and egui
//! paints. So a resize shows in its own frame. Nothing here steps a simulation at rest, and a stage draws only
//! when something's changed, until temporal AA has settled: an editor left open costs what it did.
//!
//! Everything on the GPU is the window's to drop: a lost device takes the [`StageView`] with it, and the next
//! is made from what the editor holds, which is the documents. `tests/viewport.rs` and
//! `tests/viewport_stage.rs` hold what makes it right, on a real GPU and with no window.

use std::path::Path;
use std::sync::Arc;

use game_combat::combat::CombatOption;
use game_combat::fight::{Fight, fight_stage};
use sapphire_gameplay::replay::LiveMoves;
use sapphire_math::{Vec2, Vec3};
use sapphire_moves::MoveSet;
use sapphire_platform::{AppInitError, Host};
use sapphire_render::quality::QualitySettings;
use sapphire_render::{CameraData, PipelineInventory, VfxFrame};
use sapphire_rhi::GpuContext;
use sapphire_rhi::wgpu;
use sapphire_stage::scene::Scene;
use sapphire_stage::{Content, ContentChanges, FrameLaps, SceneShow, Stage, StageDesc};

/// The format a viewport draws in: an sRGB target, which encodes what post returns (`post.wgsl`).
pub const VIEWPORT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The format egui samples it in. `egui_wgpu` takes a native texture's values as sRGB-encoded already, and
/// documents `Rgba8Unorm` only, so it needs a plain view of the same bytes. An sRGB view decodes on
/// sampling, and egui then treats the linear values as encoded: the frame comes out too dark.
pub const VIEWPORT_SAMPLED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Where in a pane a viewport shows, and the size of texture that fills it: on whole pixels, since a pane's edges
/// fall between them when its shares do, and a frame sampled off the grid moves every pixel in it.
#[must_use]
pub fn on_pixels(pane: egui::Rect, pixels_per_point: f32) -> (egui::Rect, (u32, u32)) {
    use egui::emath::GuiRounding as _;
    let rect = pane.round_to_pixels(pixels_per_point);
    let size = rect.size() * pixels_per_point;
    (rect, (size.x.round() as u32, size.y.round() as u32))
}

/// The texture a viewport's frame draws into.
#[derive(Debug)]
pub struct ViewportTarget {
    pub texture: wgpu::Texture,
    /// What the engine renders to.
    pub render_view: wgpu::TextureView,
    /// What egui samples.
    pub sampled_view: wgpu::TextureView,
}

impl ViewportTarget {
    #[must_use]
    pub fn new(device: &wgpu::Device, (width, height): (u32, u32)) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: VIEWPORT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[VIEWPORT_SAMPLED_FORMAT],
        });
        let render_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampled_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(VIEWPORT_SAMPLED_FORMAT),
            ..wgpu::TextureViewDescriptor::default()
        });
        Self { texture, render_view, sampled_view }
    }

    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }
}

/// How many frames a change draws, for temporal AA to settle on what it shows.
pub const SETTLING_FRAMES: u32 = 16;

/// What the fight in a viewport is: nobody's playing it.
#[must_use]
pub fn the_fight() -> CombatOption {
    CombatOption::Fight { script: None, party: false }
}

/// A camera from outside the game: looks at `focus` from `distance` away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutsideView {
    pub focus: Vec3,
    /// Radians about up, with 0 looking down -Z, as [`sapphire_stage::orbit::OrbitCamera`] reckons it.
    pub yaw: f32,
    /// Radians above level.
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
}

impl OutsideView {
    /// How far a view starts from what it's looking at, since a frame doesn't say.
    const DISTANCE: f32 = 5.0;
    const PITCH_LIMIT: f32 = 1.5;
    const RADIANS_PER_POINT: f32 = 0.005;
    const DOLLY_PER_POINT: f32 = 0.002;

    /// The view `camera` has.
    #[must_use]
    pub fn of(camera: &CameraData) -> Self {
        let forward = -camera.view.row(2).truncate();
        Self {
            focus: camera.position + forward * Self::DISTANCE,
            yaw: (-forward.x).atan2(-forward.z),
            pitch: (-forward.y).clamp(-1.0, 1.0).asin(),
            distance: Self::DISTANCE,
            fov_y: 2.0 * (1.0 / camera.proj.y_axis.y).atan(),
        }
    }

    #[must_use]
    pub fn forward(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(-sin_yaw * cos_pitch, -sin_pitch, -cos_yaw * cos_pitch)
    }

    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.focus - self.forward() * self.distance
    }

    /// Turns by a drag of `points`, and comes closer by a scroll of `dolly` points.
    pub fn moved(&mut self, points: Vec2, dolly: f32) {
        self.yaw -= points.x * Self::RADIANS_PER_POINT;
        self.pitch =
            (self.pitch + points.y * Self::RADIANS_PER_POINT).clamp(-Self::PITCH_LIMIT, Self::PITCH_LIMIT);
        self.distance = (self.distance * (-dolly * Self::DOLLY_PER_POINT).exp()).clamp(0.5, 200.0);
    }
}

/// What plays in a viewport: a fight, as the game stands it, and the view from outside it if there is one.
#[derive(Default)]
pub struct ViewShow {
    pub fight: Fight,
    /// Takes over from the game's own camera.
    pub outside: Option<OutsideView>,
}

impl SceneShow for ViewShow {
    fn declare_pipelines(&self, inventory: &mut PipelineInventory) {
        self.fight.declare_pipelines(inventory);
    }

    fn step(&mut self, scene: &mut Scene, laps: &mut FrameLaps) {
        self.fight.step(scene, laps);
    }

    fn end_steps(&mut self, scene: &mut Scene, dt: f32) {
        self.fight.end_steps(scene, dt);
    }

    fn follow_content(&mut self, scene: &mut Scene, content: &mut Content, changes: ContentChanges) {
        self.fight.follow_content(scene, content, changes);
    }

    fn animate(&mut self, scene: &mut Scene, content: &Content, alpha: f32) {
        self.fight.animate(scene, content, alpha);
    }

    fn present(&mut self, scene: &mut Scene, alpha: f32, aspect: Option<f32>) {
        self.fight.present(scene, alpha, aspect);
        if let Some(view) = &self.outside {
            let mut camera = scene.camera_mut();
            camera.set_view(view.eye(), view.forward(), view.fov_y);
            camera.set_shake(Vec3::ZERO, Vec3::ZERO);
        }
    }

    fn write_effects(&mut self, frame: &mut VfxFrame, alpha: f32) {
        self.fight.write_effects(frame, alpha);
    }

    fn draw_debug(&mut self) {
        self.fight.draw_debug();
    }
}

/// A stage on `gpu` with `host`'s content, and the game's fight stood on it, with everything it can draw built.
pub fn stand(host: &Host, gpu: &Arc<GpuContext>) -> Result<Stage<ViewShow>, AppInitError> {
    let desc = fight_stage(StageDesc::new(host, gpu, VIEWPORT_FORMAT, QualitySettings::default()));
    let mut content = Content::new(host, desc.character, desc.sky)?;
    let mut show = ViewShow::default();
    let combat = show.fight.prepare_stock_combat(&the_fight(), &mut content)?;
    let mut stage =
        Stage::new_with_content(gpu.instance().clone(), Arc::clone(gpu), desc, content)?.with_show(show);
    let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
    show.fight.stand_prepared(combat, scene, content, renderer, gpu, quality.active().id)?;
    scene.prewarm(&*show, renderer);
    Ok(stage)
}

/// The part of a viewport a pass of the interface sees, with no GPU behind it.
#[derive(Debug, Default)]
pub struct ViewportPane {
    /// The pixels the last pass that showed the pane asked for.
    wanted: Option<(u32, u32)>,
    /// Where that pass put them.
    rect: Option<egui::Rect>,
    /// What egui knows the frame by, once there is one.
    texture: Option<egui::TextureId>,
    /// Why there's nothing to show, or what's wrong with what is.
    note: Option<String>,
    /// More frames are coming, so egui should run again.
    settling: bool,
    /// How far the view's been dragged and scrolled since the engine last asked, in points.
    dragged: egui::Vec2,
    scrolled: f32,
    /// A double click: back to the game's camera.
    back_to_game: bool,
    /// A person asked for another try at what failed.
    try_again: bool,
}

impl ViewportPane {
    pub(crate) fn ui(&mut self, ui: &mut egui::Ui) {
        let (rect, size) = on_pixels(ui.available_rect_before_wrap(), ui.pixels_per_point());
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Viewport"));
        self.rect = Some(rect);
        self.wanted = (size.0 > 0 && size.1 > 0).then_some(size);
        if response.dragged() {
            self.dragged += response.drag_delta();
        }
        if response.hovered() {
            self.scrolled += ui.input(|input| input.smooth_scroll_delta.y);
        }
        self.back_to_game |= response.double_clicked();
        if let Some(texture) = self.texture {
            let whole = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(texture, rect, whole, egui::Color32::WHITE);
            if let Some(note) = &self.note {
                ui.painter().text(
                    rect.left_top() + egui::vec2(6.0, 6.0),
                    egui::Align2::LEFT_TOP,
                    note,
                    egui::FontId::proportional(12.0),
                    egui::Color32::YELLOW,
                );
            }
        } else {
            let why = self.note.as_deref().unwrap_or("The viewport draws when a window has a GPU");
            crate::preview::empty(ui.painter(), rect, why);
            self.try_again |= response.clicked();
        }
        if self.settling {
            ui.ctx().request_repaint();
        }
    }

    /// Where the last pass put the frame, on whole pixels.
    #[must_use]
    pub fn rect(&self) -> Option<egui::Rect> {
        self.rect
    }

    /// Why there's nothing to show, or what's wrong with what is.
    #[must_use]
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// Whether more frames are coming with nothing more changing.
    #[must_use]
    pub const fn is_settling(&self) -> bool {
        self.settling
    }

    /// The GPU's gone, and egui's textures with it.
    pub fn forget_gpu(&mut self) {
        self.texture = None;
        self.settling = false;
        self.note = Some("The GPU went away: the viewport comes back with the next one".to_owned());
    }
}

/// What a viewport's engine half takes from the editor, each frame: [`crate::Editor::viewport_frame`].
pub struct ViewportFrame<'a> {
    pub pane: &'a mut ViewportPane,
    /// Whether there's a pane at all, rather than one a tab away.
    pub open: bool,
    /// The cook's folder, if the editor's found one.
    pub content: Option<&'a Path>,
    /// The document of moves it has open, by file name, as it last compiled.
    pub moves: Option<(&'a str, &'a MoveSet)>,
}

/// A viewport's engine half: everything on the device, made when it's first shown and dropped with the device
/// or the pane. It takes the device each frame from what owns it, which draws egui on it too.
#[derive(Default)]
pub struct StageView {
    stage: Option<Stage<ViewShow>>,
    target: Option<ViewportTarget>,
    texture: Option<egui::TextureId>,
    /// The moves the fight stands on, as the document last compiled.
    moves: Option<(String, MoveSet)>,
    /// Frames left to draw of the last change.
    settling: u32,
    /// How many have been drawn.
    frames: u64,
    /// What it was stood from couldn't be, and no one's asked it to try again.
    failed: bool,
}

impl StageView {
    /// The stage, if one's been stood.
    pub fn stage(&mut self) -> Option<&mut Stage<ViewShow>> {
        self.stage.as_mut()
    }

    /// The texture its frames are in.
    #[must_use]
    pub fn frame(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|target| &target.texture)
    }

    /// How many frames the stage has drawn.
    #[must_use]
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// The viewport's share of a frame, after egui's pass and before it paints. Takes what the pass asked for,
    /// and draws if anything's changed.
    pub fn draw(
        &mut self,
        gpu: &Arc<GpuContext>,
        painter: &mut egui_wgpu::Renderer,
        frame: ViewportFrame<'_>,
    ) {
        let ViewportFrame { pane, open, content, moves } = frame;
        if !open {
            self.close(painter, pane);
            return;
        }
        let Some(size) = pane.wanted.take() else {
            pane.settling = false;
            return;
        };
        if self.stage.is_none() && !self.stood(gpu, content, pane) {
            return;
        }
        self.follow(moves, pane);
        self.look(pane, size);
        self.draw_at(gpu, painter, pane, size);
    }

    /// Makes the stage, if that's not failed since a person last asked. Says whether there is one.
    fn stood(&mut self, gpu: &Arc<GpuContext>, content: Option<&Path>, pane: &mut ViewportPane) -> bool {
        if self.failed && !std::mem::take(&mut pane.try_again) {
            return false;
        }
        let Some(content) = content else {
            pane.note = Some("No cook to draw: `cargo xtask content` makes one".to_owned());
            return false;
        };
        match stand(&Host::with_content_at(content), gpu) {
            Ok(stage) => {
                self.stage = Some(stage);
                self.moves = None;
                self.failed = false;
                pane.note = None;
                true
            }
            Err(error) => {
                tracing::warn!(%error, "the viewport has no stage");
                pane.note = Some(format!("{error}. Click to try again"));
                self.failed = true;
                false
            }
        }
    }

    /// Starts the fight over on the document's moves, when they've changed.
    fn follow(&mut self, moves: Option<(&str, &MoveSet)>, pane: &mut ViewportPane) {
        let same = match (&self.moves, moves) {
            (None, None) => true,
            (Some((name, kept)), Some((open, set))) => name == open && kept == set,
            _ => false,
        };
        if same {
            return;
        }
        let Some(stage) = &mut self.stage else { return };
        let mut live = LiveMoves::default();
        if let Some((name, set)) = moves {
            live.remember(name, set.clone());
        }
        let Stage { show, scene, content, .. } = stage;
        let started = show.fight.start_on(&the_fight(), live, scene, content);
        pane.note = started.err();
        self.moves = moves.map(|(name, set)| (name.to_owned(), set.clone()));
        self.settling = SETTLING_FRAMES;
    }

    /// Takes what a person did to the view.
    fn look(&mut self, pane: &mut ViewportPane, (width, height): (u32, u32)) {
        let Some(stage) = &mut self.stage else { return };
        let (dragged, scrolled) = (std::mem::take(&mut pane.dragged), std::mem::take(&mut pane.scrolled));
        if std::mem::take(&mut pane.back_to_game) {
            stage.show.outside = None;
            self.settling = SETTLING_FRAMES;
        }
        if dragged == egui::Vec2::ZERO && scrolled == 0.0 {
            return;
        }
        let aspect = width as f32 / height as f32;
        let outside = match stage.show.outside {
            Some(view) => view,
            None => OutsideView::of(&stage.scene.camera().camera_data(aspect)),
        };
        let outside = stage.show.outside.insert(outside);
        outside.moved(Vec2::new(dragged.x, dragged.y), scrolled);
        self.settling = SETTLING_FRAMES;
    }

    fn draw_at(
        &mut self,
        gpu: &Arc<GpuContext>,
        painter: &mut egui_wgpu::Renderer,
        pane: &mut ViewportPane,
        size: (u32, u32),
    ) {
        let Some(stage) = &mut self.stage else { return };
        if self.target.as_ref().is_none_or(|target| target.size() != size) {
            let target = self.target.insert(ViewportTarget::new(gpu.device(), size));
            match self.texture {
                // The ID stays, so what the pass laid out shows the new texture.
                Some(id) => painter.update_egui_texture_from_wgpu_texture(
                    gpu.device(),
                    &target.sampled_view,
                    wgpu::FilterMode::Nearest,
                    id,
                ),
                None => {
                    self.texture = Some(painter.register_native_texture(
                        gpu.device(),
                        &target.sampled_view,
                        wgpu::FilterMode::Nearest,
                    ));
                }
            }
            self.settling = SETTLING_FRAMES;
        }
        if self.settling > 0
            && let Some(target) = &self.target
        {
            self.settling -= 1;
            self.frames += 1;
            if let Err(error) = stage.render(&target.render_view, 1.0, |_, _| {}) {
                pane.note = Some(error.to_string());
                self.settling = 0;
            }
            stage.scene.end_frame();
        }
        // A pass that laid out no image has this from the next, which `settling` asks for.
        pane.texture = self.texture;
        pane.settling = self.settling > 0;
    }

    fn close(&mut self, painter: &mut egui_wgpu::Renderer, pane: &mut ViewportPane) {
        if let Some(id) = self.texture.take() {
            painter.free_texture(&id);
        }
        *self = Self::default();
        pane.texture = None;
        pane.wanted = None;
        pane.settling = false;
    }
}

#[cfg(test)]
mod tests {
    use sapphire_stage::orbit::OrbitCamera;

    use super::*;

    /// A camera's view and the view made from it are the same view, so taking over moves nothing.
    #[test]
    fn taking_a_view_over_leaves_it_where_it_was() {
        let mut camera = OrbitCamera::shot(Vec3::new(1.0, 1.5, -2.0), 0.7, 0.2, 6.0, 0.8);
        let before = camera.camera_data(1.5);
        let view = OutsideView::of(&before);
        camera.set_view(view.eye(), view.forward(), view.fov_y);
        let after = camera.camera_data(1.5);
        assert!(before.position.distance(after.position) < 1e-4);
        assert!(before.view.abs_diff_eq(after.view, 1e-3), "{:?} against {:?}", before.view, after.view);
        assert!(before.proj.abs_diff_eq(after.proj, 1e-3), "{:?} against {:?}", before.proj, after.proj);
    }

    /// A drag turns it about what it looks at, and a scroll brings it closer, within limits.
    #[test]
    fn a_drag_turns_about_the_focus() {
        let mut view = OutsideView { focus: Vec3::ZERO, yaw: 0.0, pitch: 0.0, distance: 4.0, fov_y: 0.8 };
        view.moved(Vec2::new(100.0, 0.0), 0.0);
        assert!((view.eye().length() - 4.0).abs() < 1e-5);
        assert!(view.eye().z.abs() < 4.0);
        view.moved(Vec2::new(0.0, 1e6), 0.0);
        assert!(view.pitch <= OutsideView::PITCH_LIMIT);
        view.moved(Vec2::ZERO, 1e6);
        assert!(view.distance >= 0.5);
    }
}
