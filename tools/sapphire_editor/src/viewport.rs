//! The viewport: the real renderer, through `sapphire_stage`, in a pane, on the window's device.
//!
//! There's one route through the renderer, the stage's: [`stand`] makes the stage a game's fight stands on, and
//! [`StageView::draw`] hands it a texture of the pane's size. A frame runs in this order: egui's pass says where
//! the pane is ([`ViewportPane`], which holds nothing of the GPU's), the stage draws at that size, and egui
//! paints. So a resize shows in its own frame. Nothing here steps a simulation at rest, and a stage draws only
//! when something's changed, until temporal AA has settled: an editor left open costs what it did.
//!
//! A run ([`crate::play`]) is another stage, made fresh: what stood goes first, the fight starts on a snapshot's
//! moves, the game's bindings play it, and stopping it or a tab away stands the viewport again. It holds copies, so
//! nothing a game does can reach a document.
//!
//! A zone (M3 E5) stands another stage, with no fight: each placement drawn on its own as its shape's mesh from an
//! editor's cook ([`sapphire_stage::scene::Scene::lay_out_zone`]), where the document has it now, and the gizmo on
//! the one selected, drawn by the stage's debug draws. An edit moves an instance: nothing is cooked, loaded or
//! built. The character stands at the zone's origin, for a measure of size.
//!
//! A screen (M3 E5) stands a stage with no fight, and draws over it as the game does, through `game_ui`'s
//! [`ScreenStack`], loaded from the cook and then shown as its document reads now, with the stand-in values the
//! screen pane holds ([`crate::screen`]), over the flat color the game's screens' goldens stand behind them. A
//! screen with a 3D inset shows the stage's character in it, as the character screen does. Its frame is the
//! aspect's size, not the pane's, shown whole in the pane: what a capture at that size draws. An edit makes a
//! new tree, and nothing's loaded or built.
//!
//! A timeline (M3 E5) plays on the stage a fight stands on, as the game plays it: cooked from its document,
//! from its start on a fresh stage, a frame of [`crate::app::RATE`] at a time with nothing drawn, to the
//! playhead, which then draws as any change does. So the frame at a playhead is the frame a capture at that
//! time draws. Forward, it plays on; back, or after an edit, it starts over on a fresh stage. The view can't
//! turn: the timeline's camera is the view.
//!
//! Everything on the GPU is the window's to drop: a lost device takes the [`StageView`] with it, and the next
//! is made from what the editor holds, which is the documents. `tests/viewport.rs` and
//! `tests/viewport_stage.rs` hold what makes it right, on a real GPU and with no window.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use game_combat::combat::{CombatOption, read_bindings};
use game_combat::documents::FightDocuments;
use game_combat::fight::{Fight, fight_stage};
use game_ui::screen::{ScreenStack, StackSetup};
use sapphire_assets::AssetId;
use sapphire_assets::ui::UiDocument;
use sapphire_controls::CombatControls;
use sapphire_gameplay::ActorId;
use sapphire_gameplay::replay::LiveMoves;
use sapphire_input::InputMap;
use sapphire_look::{UiDrawList, UiPageState};
use sapphire_math::{Vec2, Vec3};
use sapphire_moves::MoveSet;
use sapphire_platform::{AppInitError, Event, Host, Timestamp, WindowEvent};
use sapphire_render::quality::{OcclusionSetting, QualitySettings, Switch, TierId};
use sapphire_render::{CameraData, Msaa, PipelineInventory, PostVariants, RenderSettings, VfxFrame};
use sapphire_rhi::GpuContext;
use sapphire_rhi::wgpu;
use sapphire_sequencer::Origin;
use sapphire_stage::scene::{LaidOut, Scene};
use sapphire_stage::{Content, ContentChanges, FrameLaps, SceneShow, Stage, StageDesc};
use sapphire_time::{FixedStepConfig, FixedStepDriver, Stopwatch};
use sapphire_timeline::Timeline;
use sapphire_ui::Atlas;
use sapphire_zone::ZoneDocument;

use crate::gizmo::{Gizmo, ViewCamera};
use crate::play::{PlayRequest, Snapshot};
use crate::screen::{ScreenShown, StandIn, model_of};

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

/// A frame of `size` pixels in `pane`, whole: centred, as large as fits, its shape kept.
#[must_use]
pub fn fit(pane: egui::Rect, (width, height): (u32, u32)) -> egui::Rect {
    let scale = (pane.width() / width as f32).min(pane.height() / height as f32);
    egui::Rect::from_center_size(pane.center(), egui::vec2(width as f32, height as f32) * scale)
}

/// What a viewport stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stood {
    Fight,
    Zone,
    Screen,
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

/// Who the keys play.
const PLAYER: ActorId = ActorId::from_raw(1).expect("not zero");

/// The longest a frame's presentation moves by, as the game has it.
const MAX_PRESENTATION_DT: Duration = Duration::from_millis(100);

/// How many frames a change draws, for temporal AA to settle on what it shows.
pub const SETTLING_FRAMES: u32 = 16;

/// What the fight in a viewport is: nobody's playing it.
#[must_use]
pub fn the_fight() -> CombatOption {
    CombatOption::Fight { script: None, party: false }
}

/// A run's controls, on the bindings `documents` name, or Sapphire's own, as the game takes them.
pub fn controls_of(documents: &FightDocuments) -> Result<CombatControls, String> {
    let controls = match &documents.bindings {
        Some(bindings) => InputMap::new(bindings.clone())
            .map_err(|error| error.to_string())
            .and_then(|map| CombatControls::new(map).map_err(|error| error.to_string())),
        None => CombatControls::with_default_bindings().map_err(|error| error.to_string()),
    };
    controls.map_err(|error| format!("the game's bindings: {error}"))
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
        self.turned(points);
        self.distance = (self.distance * (-dolly * Self::DOLLY_PER_POINT).exp()).clamp(0.5, 200.0);
    }

    /// A view from above and aside of everything `zone` places, whole.
    #[must_use]
    pub fn of_zone(zone: &ZoneDocument) -> Self {
        let points = || zone.placements.iter().map(|placement| Vec3::from(placement.at));
        let count = zone.placements.len().max(1) as f32;
        let focus = points().fold(Vec3::ZERO, |sum, at| sum + at) / count;
        let reach = points().map(|at| at.distance(focus)).fold(0.0_f32, f32::max);
        Self { focus, yaw: 0.6, pitch: 0.5, distance: ((reach + 3.0) * 1.8).clamp(5.0, 200.0), fov_y: 0.8 }
    }

    fn turned(&mut self, points: Vec2) {
        self.yaw -= points.x * Self::RADIANS_PER_POINT;
        self.pitch =
            (self.pitch + points.y * Self::RADIANS_PER_POINT).clamp(-Self::PITCH_LIMIT, Self::PITCH_LIMIT);
    }
}

/// What plays in a viewport: a fight, as the game stands it, and the view from outside it if there is one.
#[derive(Default)]
pub struct ViewShow {
    pub fight: Fight,
    /// Takes over from the game's own camera.
    pub outside: Option<OutsideView>,
    /// The bindings, in a run.
    pub controls: Option<CombatControls>,
    /// Where the view looks across the ground, which the next tick's stick turns by.
    camera_yaw: f32,
    /// The gizmo, on a zone's placement.
    pub gizmo: Option<Gizmo>,
    /// A screen over it all.
    pub screen: Option<ScreenStack>,
}

impl SceneShow for ViewShow {
    fn declare_pipelines(&self, inventory: &mut PipelineInventory) {
        self.fight.declare_pipelines(inventory);
        inventory.ui |= self.screen.is_some();
        inventory.insets |= self.screen.as_ref().is_some_and(ScreenStack::holds_insets);
    }

    fn step(&mut self, scene: &mut Scene, laps: &mut FrameLaps) {
        let Some(controls) = &mut self.controls else {
            self.fight.step(scene, laps);
            return;
        };
        let mut stopwatch = Stopwatch::start();
        let mut bindings = controls.source(self.camera_yaw);
        read_bindings(self.fight.combat.as_mut(), &mut bindings, PLAYER);
        laps.simulation += stopwatch.lap();
        self.fight.follow_step(scene, laps);
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

    fn begin_interface(&mut self, pages: UiPageState) {
        if let Some(screen) = &mut self.screen {
            screen.follow_pages(pages);
        }
    }

    fn draw_interface(&mut self, size: (u32, u32), list: &mut UiDrawList) {
        if let Some(screen) = &mut self.screen {
            screen.draw(size, list);
        }
    }

    fn inset_shots(&self, visit: &mut dyn FnMut(sapphire_stage::InsetShot<'_>)) {
        if let Some(screen) = &self.screen {
            screen.inset_shots(visit);
        }
    }

    fn draw_debug(&mut self) {
        self.fight.draw_debug();
        if let Some(gizmo) = &self.gizmo {
            gizmo.draw();
        }
    }
}

/// A stage on `gpu` with `host`'s content, and the game's fight stood on it, with everything it can draw built and
/// its textures [whole](Stage::whole_textures).
pub fn stand(host: &Host, gpu: &Arc<GpuContext>) -> Result<Stage<ViewShow>, AppInitError> {
    // Read first, so a missing cook says how to make one here rather than in `Content::new`.
    let documents = FightDocuments::read(host.content())
        .map_err(|error| AppInitError::msg(format!("{error} (cook it with `cargo xtask content`)")))?;
    let moves = documents.live();
    stand_fight(host, gpu, &the_fight(), documents, moves).map(with_whole_textures)
}

/// `stage` with every texture brought whole before each frame draws, as a capture's are ([`Stage::whole_textures`],
/// M5 M4), rather than streaming in a level at a time as the game's do: a still view settles for
/// [`SETTLING_FRAMES`], and a level that came partway through, on a frame that depends on when the asset server's
/// reads finish, would leave temporal AA's history at a different point each time. A run streams them as the
/// game does.
fn with_whole_textures(mut stage: Stage<ViewShow>) -> Stage<ViewShow> {
    stage.whole_textures = true;
    stage
}

/// [`stand`] with `option`'s fight on `documents` and `moves`, as a run plays it: set up as the game's own is,
/// before its first tick, from a snapshot's moves over the game's documents rather than the build's.
pub fn stand_fight(
    host: &Host,
    gpu: &Arc<GpuContext>,
    option: &CombatOption,
    documents: FightDocuments,
    moves: LiveMoves,
) -> Result<Stage<ViewShow>, AppInitError> {
    let desc = fight_stage(StageDesc::new(host, gpu, VIEWPORT_FORMAT, QualitySettings::default()));
    let mut content = Content::new(host, desc.character, desc.sky)?;
    let mut show = ViewShow::default();
    show.fight.documents = documents;
    let combat = show.fight.prepare_stock_fight_on(option, moves, &mut content)?;
    let mut stage = Stage::new_with_content(Arc::clone(gpu), desc, content)?.with_show(show);
    let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
    show.fight.stand_prepared(combat, scene, content, renderer, gpu, quality.active().id)?;
    scene.prewarm(&*show, renderer);
    Ok(stage)
}

/// A stage on `gpu` with `host`'s content and no fight, for a zone to be laid out on, with what the character draws
/// through built: [`Scene::lay_out_zone`] builds what the zone's pack does. Its textures are
/// [whole](Stage::whole_textures).
pub fn stand_zone_stage(host: &Host, gpu: &Arc<GpuContext>) -> Result<Stage<ViewShow>, AppInitError> {
    let desc = StageDesc::new(host, gpu, VIEWPORT_FORMAT, QualitySettings::default());
    let content = Content::new(host, desc.character, desc.sky)?;
    let mut stage = Stage::new_with_content(Arc::clone(gpu), desc, content)?.with_show(ViewShow::default());
    let Stage { show, scene, renderer, .. } = &mut stage;
    scene.prewarm(&*show, renderer);
    // The gizmo is debug lines, whose pass is built here rather than on the frame the first one shows.
    renderer.prewarm(&PipelineInventory { debug_draw: true, ..PipelineInventory::default() });
    sapphire_core::debug_draw::set_enabled(true);
    Ok(with_whole_textures(stage))
}

/// A stage on `gpu` with `host`'s content and no fight, for a screen to draw over, with the interface's pipeline
/// and an inset's resolve built: a screen stands on a flat color, so what's behind it shows only in an inset. It
/// draws as the game's screens' goldens capture them (`desktop-high`, 4× MSAA, no shadows, occlusion or fog, the
/// plain sky and post), so an inset's character is theirs too, with its textures [whole](Stage::whole_textures) as
/// theirs are.
pub fn stand_screen_stage(host: &Host, gpu: &Arc<GpuContext>) -> Result<Stage<ViewShow>, AppInitError> {
    let quality = QualitySettings {
        tier: Some(TierId::DesktopHigh),
        shadows: Switch::Off,
        ambient_occlusion: OcclusionSetting::Off,
        ..QualitySettings::default()
    };
    let mut desc = StageDesc::new(host, gpu, VIEWPORT_FORMAT, quality);
    let msaa = if desc.settings.msaa == Msaa::Off { Msaa::X4 } else { desc.settings.msaa };
    desc.settings =
        RenderSettings { msaa, temporal: false, post_variants: PostVariants::NONE, ..desc.settings };
    let content = Content::new(host, desc.character, desc.sky)?;
    let mut stage = Stage::new_with_content(Arc::clone(gpu), desc, content)?.with_show(ViewShow::default());
    let Stage { show, scene, renderer, .. } = &mut stage;
    scene.set_plain_sky(true);
    scene.set_fog(false);
    scene.set_plain_post(true);
    scene.prewarm(&*show, renderer);
    renderer.prewarm(&PipelineInventory { ui: true, insets: true, ..PipelineInventory::default() });
    Ok(with_whole_textures(stage))
}

/// The screens for `shown`, from the cook's screen of its ID, showing what its document reads now over the flat
/// color the game's screens' goldens stand behind them: as the sandbox shows the game's screens (`--ui-plain`), on a
/// clock standing well past anything a screen does as it opens.
pub fn screen_stack(stage: &mut Stage<ViewShow>, shown: &ScreenShown<'_>) -> Result<ScreenStack, String> {
    let Stage { content, renderer, gpu, .. } = stage;
    let assets = content
        .load_screens(&[shown.id], renderer, gpu)
        .map_err(|error| format!("{error}: a screen shows once it's cooked (`cargo xtask content`)"))?;
    let setup = StackSetup { locale: shown.language.to_owned(), pseudo: false, scale: 1.0 };
    let mut stack = ScreenStack::new(assets.tables, assets.fonts, Atlas::paint(), setup)?;
    let start = Timestamp::from_nanos(0);
    let cooked = assets.documents.first().ok_or("the cook has no such screen")?;
    stack.push(cooked, model_of(shown.values), start, Vec::new())?;
    stack.push_plain_under(start)?;
    stack.replace_document(shown.document, shown.model)?;
    stack.set_now(Timestamp::from_nanos(SCREEN_CLOCK_NANOS));
    Ok(stack)
}

/// Where a screen's clock stands: what the sandbox's does with no `--ui-time`.
const SCREEN_CLOCK_NANOS: u64 = 60_000_000_000;

/// What a viewport shows of a zone being edited.
#[derive(Clone, Copy)]
pub struct ZoneShown<'a> {
    /// The zone's ID, which names its shapes' meshes.
    pub id: AssetId,
    /// The zone source's file stem, which names its pack.
    pub stem: &'a str,
    /// As it last read.
    pub document: &'a ZoneDocument,
    pub gizmo: Option<Gizmo>,
}

impl ZoneShown<'_> {
    /// Each placement, as its shape's mesh at the rest of its transform.
    #[must_use]
    pub fn laid_out(&self) -> Vec<LaidOut> {
        let shape = |placement: &sapphire_zone::Placement| LaidOut {
            mesh: self.id.sub_asset(&placement.shape_name()),
            transform: placement.shape_transform(),
        };
        self.document.placements.iter().map(shape).collect()
    }
}

/// What a viewport plays of a timeline being edited.
#[derive(Clone)]
pub struct TimelineShown {
    /// Its document.
    pub id: AssetId,
    /// As its document last cooked.
    pub timeline: Arc<Timeline>,
    /// The playhead, in frames of [`crate::app::RATE`].
    pub frame: u32,
}

/// Plays `timeline` on `stage`'s fight from its start, as the game plays it: what a frame at a playhead plays on
/// from with [`play_frames`].
pub fn play_timeline(stage: &mut Stage<ViewShow>, timeline: Arc<Timeline>) {
    stage.show.outside = None;
    stage.show.fight.show.play_timeline(timeline, Origin::default());
}

/// `frames` of [`crate::app::RATE`] of what `stage` plays, framed for a target of `size`, with nothing drawn: what
/// a game's frames do between two a capture keeps. Each is [`Stage::skip_frame`]'s, with the target's aspect.
pub fn play_frames(stage: &mut Stage<ViewShow>, frames: u32, size: (u32, u32)) {
    let dt = 1.0 / crate::app::RATE;
    let Stage { show, scene, content, renderer, gpu, .. } = stage;
    for _ in 0..frames {
        show.fight.show.drive_camera(dt, Vec2::ZERO, 0.0);
        content.update(renderer, gpu);
        scene.follow_content(content, show);
        scene.animate(content, 1.0, show);
        let packet = scene.frame_packet(1.0, Some(size), show);
        renderer.prepare(gpu, packet);
        scene.end_frame();
    }
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
    /// The button's been pressed, and the editor hasn't taken its snapshot yet.
    asked_to_play: bool,
    /// A run for the engine half's next frame.
    play: Option<PlayRequest>,
    /// A run's going on.
    playing: bool,
    /// What the run is, for a person.
    status: Option<String>,
    /// The button's been pressed.
    stop: bool,
    /// The camera the last frame drew with.
    camera: Option<ViewCamera>,
    /// The frame's size, when it's a screen's rather than the pane's.
    fixed: Option<(u32, u32)>,
}

impl ViewportPane {
    /// The pane, with `pointer` taking the pointer first, given the camera the last frame drew with and where the
    /// frame shows. When it says it took it, the view doesn't turn.
    pub(crate) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        pointer: impl FnOnce(&egui::Response, Option<&ViewCamera>, egui::Rect) -> bool,
    ) {
        ui.horizontal(|ui| {
            if self.playing {
                self.stop |= ui.button("Stop playing").clicked();
                // Anything with the keyboard would take the fight's keys.
                ui.ctx().memory_mut(egui::Memory::stop_text_input);
            } else {
                self.asked_to_play |=
                    ui.add_enabled(self.texture.is_some(), egui::Button::new("Play in viewport")).clicked();
            }
            if let Some(status) = &self.status {
                ui.label(status);
            }
        });
        let (rect, size) = on_pixels(ui.available_rect_before_wrap(), ui.pixels_per_point());
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Viewport"));
        self.rect = Some(rect);
        self.wanted = (size.0 > 0 && size.1 > 0).then_some(size);
        let taken = self.texture.is_some() && pointer(&response, self.camera.as_ref(), rect);
        if response.dragged() && !taken {
            self.dragged += response.drag_delta();
        }
        if response.hovered() {
            self.scrolled += ui.input(|input| input.smooth_scroll_delta.y);
        }
        self.back_to_game |= response.double_clicked() && !taken;
        if let Some(texture) = self.texture {
            let whole = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            let shown = self.fixed.map_or(rect, |size| fit(rect, size));
            if shown != rect {
                ui.painter().rect_filled(rect, 0.0, egui::Color32::from_gray(24));
            }
            ui.painter().image(texture, shown, whole, egui::Color32::WHITE);
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

    pub(crate) fn take_asked_to_play(&mut self) -> bool {
        std::mem::take(&mut self.asked_to_play)
    }

    pub(crate) fn ask(&mut self, request: PlayRequest) {
        self.play = Some(request);
        self.note = None;
    }

    pub(crate) fn refuse(&mut self, why: String) {
        self.note = Some(why);
    }

    /// The run asked for and not started yet.
    #[must_use]
    pub fn asked_for(&self) -> Option<&PlayRequest> {
        self.play.as_ref()
    }

    /// Whether a run's going on.
    #[must_use]
    pub const fn is_playing(&self) -> bool {
        self.playing
    }

    /// Ends the run at the next frame.
    pub fn stop_playing(&mut self) {
        self.stop = true;
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

    /// Where a frame of `size` pixels shows whole in the pane, if it's a screen's: centred, as large as fits.
    #[must_use]
    pub fn frame_rect(&self) -> Option<egui::Rect> {
        let rect = self.rect?;
        Some(self.fixed.map_or(rect, |size| fit(rect, size)))
    }

    /// The camera the last frame drew with, if there's been one.
    #[must_use]
    pub const fn camera(&self) -> Option<&ViewCamera> {
        self.camera.as_ref()
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
        self.play = None;
        self.playing = false;
        self.stop = false;
        self.status = None;
        self.camera = None;
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
    /// The zone being edited, if the current document is one: it's what shows, rather than the fight.
    pub zone: Option<ZoneShown<'a>>,
    /// The timeline being edited, if the current document is one that cooks: the fight plays it.
    pub timeline: Option<TimelineShown>,
    /// The screen being edited, if the current document is one with a model that reads.
    pub screen: Option<ScreenShown<'a>>,
    /// The time of the pass, in seconds.
    pub now: f64,
}

/// A run going on, on the stage that's up.
struct Run {
    driver: FixedStepDriver,
    snapshot: Snapshot,
    /// When the last frame was, on the pass's clock.
    last: Option<f64>,
    /// How far this frame is between steps.
    alpha: f32,
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
    run: Option<Run>,
    /// The stage stood is a zone's, with no fight.
    zoned: bool,
    /// The zone laid out on it, as it was laid out.
    zone: Option<(AssetId, Vec<LaidOut>)>,
    /// The timeline the stage plays, and the frame it's played to.
    sequenced: Option<(Arc<Timeline>, u32)>,
    /// The stage stood is a screen's, with no fight.
    screened: bool,
    /// The screen drawn over it, as it was shown.
    screen: Option<ScreenState>,
    /// The stage's character stands in an inset, for a screen that has one.
    showcase: bool,
}

/// What a screen on the stage shows.
#[derive(Debug, PartialEq)]
struct ScreenState {
    id: AssetId,
    language: String,
    document: UiDocument,
    values: Vec<(String, StandIn)>,
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

    /// What the run going on started from.
    #[must_use]
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.run.as_ref().map(|run| &run.snapshot)
    }

    /// A platform event, which the bindings of a run take: only the keyboard's and a pad's, since a mouse over the
    /// editor is the editor's. A window that loses focus pauses it, as the game does.
    pub fn handle_event(&mut self, event: &Event) {
        let Some(controls) = self.controls() else { return };
        match event {
            Event::Key(_) | Event::Gamepad { .. } => controls.handle_event(event),
            Event::Window { event: WindowEvent::FocusLost, .. } => controls.set_paused(true),
            _ => {}
        }
    }

    /// Where the bindings stand at `now`, on the clock of events. Call it once a frame, before [`Self::draw`].
    pub fn read_input(&mut self, now: Timestamp) {
        if let Some(controls) = self.controls() {
            controls.update(now);
        }
    }

    fn controls(&mut self) -> Option<&mut CombatControls> {
        self.run.as_ref()?;
        self.stage.as_mut()?.show.controls.as_mut()
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
        let ViewportFrame { pane, open, content, moves, zone, timeline, screen, now } = frame;
        if !open {
            self.close(painter, pane);
            return;
        }
        let leaving = self.controls().is_some_and(|controls| controls.wants_to_leave());
        if std::mem::take(&mut pane.stop) || leaving {
            self.end_run(pane);
        }
        if let Some(request) = pane.play.take() {
            self.start_run(gpu, content, request, pane);
        }
        let Some(wanted) = pane.wanted.take() else {
            // A run behind another tab holds the keys, with its Stop out of reach.
            self.end_run(pane);
            pane.settling = false;
            return;
        };
        // A run has the pane's size; a screen, its aspect's.
        let screen = screen.filter(|_| self.run.is_none());
        let size = screen.map_or(wanted, |screen| screen.aspect.size());
        pane.fixed = screen.map(|screen| screen.aspect.size());
        let stood = if screen.is_some() {
            Stood::Screen
        } else if zone.is_some() {
            Stood::Zone
        } else {
            Stood::Fight
        };
        let changed = self.zoned != (stood == Stood::Zone)
            || self.screened != (stood == Stood::Screen)
            || self.sequenced.is_some() != timeline.is_some();
        if self.run.is_none() && self.stage.is_some() && changed {
            self.stage = None;
            self.sequenced = None;
        }
        if self.stage.is_none() && !self.stood(gpu, content, stood, pane) {
            return;
        }
        if self.run.is_none() {
            match (screen, zone, timeline) {
                (Some(screen), _, _) => self.follow_screen(&screen, pane),
                (None, Some(zone), _) => self.follow_zone(zone, pane),
                (None, None, Some(timeline)) => self.follow_timeline(gpu, content, timeline, size, pane),
                (None, None, None) => self.follow(moves, pane),
            }
        }
        self.look(pane, size);
        self.advance_run(now);
        self.draw_at(gpu, painter, pane, size);
    }

    /// Stands a stage of its own for `request`, with what was up gone first: nothing of it goes on into the run.
    fn start_run(
        &mut self,
        gpu: &Arc<GpuContext>,
        content: Option<&Path>,
        request: PlayRequest,
        pane: &mut ViewportPane,
    ) {
        self.end_run(pane);
        self.stage = None;
        self.moves = None;
        self.zone = None;
        self.zoned = false;
        self.screened = false;
        self.screen = None;
        self.sequenced = None;
        let Some(content) = content else {
            pane.note = Some("No cook to play: `cargo xtask content` makes one".to_owned());
            return;
        };
        let PlayRequest { snapshot, script } = request;
        let option = CombatOption::Fight { script, party: false };
        let host = Host::with_content_at(content);
        let documents = snapshot.fight().clone();
        let started = controls_of(&documents).and_then(|controls| {
            let mut stage = stand_fight(&host, gpu, &option, documents, snapshot.moves().clone())
                .map_err(|error| error.to_string())?;
            stage.show.controls = Some(controls);
            Ok(stage)
        });
        match started {
            Ok(stage) => {
                self.stage = Some(stage);
                pane.note = None;
                pane.playing = true;
                pane.status = Some(if snapshot.not_played().is_empty() {
                    "Playing the documents as they were at Play. Edits wait for the next.".to_owned()
                } else {
                    format!(
                        "Playing, without edits to {}: only moves reach a run.",
                        snapshot.not_played().join(", ")
                    )
                });
                let driver = FixedStepDriver::new(FixedStepConfig::default());
                self.run = Some(Run { driver, snapshot, last: None, alpha: 1.0 });
            }
            Err(why) => pane.note = Some(format!("Can't play: {why}")),
        }
    }

    /// Ends a run, if there's one, and the stage it had: the next frame stands the viewport again.
    fn end_run(&mut self, pane: &mut ViewportPane) {
        pane.playing = false;
        pane.status = None;
        if self.run.take().is_none() {
            return;
        }
        if let Some(mut stage) = self.stage.take()
            && let Err(error) = stage.show.fight.finish()
        {
            tracing::warn!(%error, "the run didn't end well");
        }
    }

    /// A frame of a run, as the game's: the bindings' steps, then the view's share on the real clock.
    fn advance_run(&mut self, now: f64) {
        let (Some(run), Some(stage)) = (&mut self.run, &mut self.stage) else { return };
        let since = now - run.last.replace(now).unwrap_or(now);
        let frame_time = Duration::from_secs_f64(since.max(0.0));
        let show = &mut stage.show;
        show.camera_yaw = show.fight.show.camera_heading(&stage.scene);
        let Some(controls) = &show.controls else { return };
        let steps = controls.advance(&mut run.driver, frame_time);
        for _ in 0..steps.count {
            stage.fixed_step();
        }
        let show = &mut stage.show;
        let Some(controls) = &show.controls else { return };
        let dt = frame_time.min(MAX_PRESENTATION_DT).as_secs_f32();
        let view_dt = if controls.is_paused() { 0.0 } else { dt };
        let (look, zoom) = (controls.look(), controls.camera_zoom());
        show.fight.show.drive_camera(view_dt, look, zoom);
        show.fight.show.end_cue_frame(&mut stage.scene, dt, view_dt, steps.alpha, &mut ());
        run.alpha = steps.alpha;
    }

    /// Makes the stage, a zone's if `zoned`, if that's not failed since a person last asked. Says whether there is
    /// one.
    fn stood(
        &mut self,
        gpu: &Arc<GpuContext>,
        content: Option<&Path>,
        what: Stood,
        pane: &mut ViewportPane,
    ) -> bool {
        if self.failed && !std::mem::take(&mut pane.try_again) {
            return false;
        }
        let Some(content) = content else {
            pane.note = Some("No cook to draw: `cargo xtask content` makes one".to_owned());
            return false;
        };
        let host = Host::with_content_at(content);
        let stood = match what {
            Stood::Fight => stand(&host, gpu),
            Stood::Zone => stand_zone_stage(&host, gpu),
            Stood::Screen => stand_screen_stage(&host, gpu),
        };
        match stood {
            Ok(stage) => {
                self.stage = Some(stage);
                self.moves = None;
                self.zone = None;
                self.sequenced = None;
                self.screen = None;
                self.zoned = what == Stood::Zone;
                self.screened = what == Stood::Screen;
                self.showcase = false;
                self.failed = false;
                self.settling = SETTLING_FRAMES;
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
        let mut live = stage.show.fight.documents.live();
        if let Some((name, set)) = moves {
            live.remember(name, set.clone());
        }
        let Stage { show, scene, content, .. } = stage;
        let started = show.fight.start_on(&the_fight(), live, scene, content);
        pane.note = started.err();
        self.moves = moves.map(|(name, set)| (name.to_owned(), set.clone()));
        self.settling = SETTLING_FRAMES;
    }

    /// Lays the zone out again where it's changed, and puts the gizmo where it's asked for. A drag is one
    /// placement's new transform: one instance moved, with nothing loaded or built.
    fn follow_zone(&mut self, zone: ZoneShown<'_>, pane: &mut ViewportPane) {
        let Some(stage) = &mut self.stage else { return };
        if stage.show.gizmo != zone.gizmo {
            stage.show.gizmo = zone.gizmo;
            self.settling = SETTLING_FRAMES;
        }
        let laid = zone.laid_out();
        let first = self.zone.as_ref().is_none_or(|(id, _)| *id != zone.id);
        if !first && self.zone.as_ref().is_some_and(|(_, was)| *was == laid) {
            return;
        }
        let pack = sapphire_zone::pack_name(zone.stem);
        let Stage { scene, content, renderer, gpu, show, .. } = stage;
        match scene.lay_out_zone(content, &pack, &laid, renderer, gpu) {
            Ok(layout) if layout.missing.is_empty() => pane.note = None,
            Ok(layout) if zone.document.has_split() => {
                pane.note = Some(format!(
                    "{} placements aren't drawn: a zone with a split placement has no shapes",
                    layout.missing.len()
                ));
            }
            Ok(layout) => {
                pane.note = Some(format!(
                    "{} placements aren't drawn: `cargo xtask content --cook editor` cooks their shapes",
                    layout.missing.len()
                ));
            }
            Err(error) => pane.note = Some(format!("The zone can't be laid out: {error}")),
        }
        if first {
            show.outside = Some(OutsideView::of_zone(zone.document));
        }
        self.zone = Some((zone.id, laid));
        self.settling = SETTLING_FRAMES;
    }

    /// Plays the timeline to the playhead: on from where it was, or from its start on a fresh stage when the
    /// playhead's gone back or the timeline's another, as an edit makes it.
    fn follow_timeline(
        &mut self,
        gpu: &Arc<GpuContext>,
        content: Option<&Path>,
        shown: TimelineShown,
        size: (u32, u32),
        pane: &mut ViewportPane,
    ) {
        let TimelineShown { timeline, frame, .. } = shown;
        let on = self.sequenced.as_ref().is_some_and(|(was, at)| Arc::ptr_eq(was, &timeline) && *at <= frame);
        if !on {
            if self.sequenced.is_some() {
                self.stage = None;
                if !self.stood(gpu, content, Stood::Fight, pane) {
                    return;
                }
            }
            let Some(stage) = &mut self.stage else { return };
            play_timeline(stage, Arc::clone(&timeline));
            self.sequenced = Some((timeline, 0));
            self.settling = SETTLING_FRAMES;
        }
        let (Some(stage), Some((_, at))) = (&mut self.stage, &mut self.sequenced) else { return };
        if *at < frame {
            play_frames(stage, frame - *at, size);
            *at = frame;
            self.settling = SETTLING_FRAMES;
        }
    }

    /// Shows `shown` over the stage: loaded from the cook when it's a new screen or language, and as its document
    /// and values read now when they've changed. What doesn't show says why, and what showed stays.
    fn follow_screen(&mut self, shown: &ScreenShown<'_>, pane: &mut ViewportPane) {
        let Some(stage) = &mut self.stage else { return };
        let new =
            self.screen.as_ref().is_none_or(|state| state.id != shown.id || state.language != shown.language);
        if new {
            match screen_stack(stage, shown) {
                Ok(stack) => {
                    if stack.holds_insets() && !std::mem::replace(&mut self.showcase, true) {
                        let character = stage.scene.character();
                        stage.scene.spawn_showcase(character, 0);
                    }
                    stage.show.screen = Some(stack);
                }
                Err(why) => {
                    pane.note = Some(why);
                    stage.show.screen = None;
                    self.screen = None;
                    return;
                }
            }
            pane.note = None;
            self.screen = Some(ScreenState {
                id: shown.id,
                language: shown.language.to_owned(),
                document: shown.document.clone(),
                values: shown.values.to_vec(),
            });
            self.settling = SETTLING_FRAMES;
            return;
        }
        let (Some(state), Some(stack)) = (&mut self.screen, &mut stage.show.screen) else { return };
        if state.document != *shown.document {
            pane.note = stack.replace_document(shown.document, shown.model).err();
            state.document = shown.document.clone();
            self.settling = SETTLING_FRAMES;
        }
        if state.values != shown.values {
            stack.set_model(model_of(shown.values));
            state.values = shown.values.to_vec();
            self.settling = SETTLING_FRAMES;
        }
    }

    /// Takes what a person did to the view.
    fn look(&mut self, pane: &mut ViewportPane, (width, height): (u32, u32)) {
        let Some(stage) = &mut self.stage else { return };
        let (dragged, scrolled) = (std::mem::take(&mut pane.dragged), std::mem::take(&mut pane.scrolled));
        if self.sequenced.is_some() || self.screened {
            return;
        }
        if std::mem::take(&mut pane.back_to_game) && !self.zoned {
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
        let playing = self.run.as_ref().map(|run| run.alpha);
        if (self.settling > 0 || playing.is_some())
            && let Some(target) = &self.target
        {
            self.settling = self.settling.saturating_sub(1);
            self.frames += 1;
            if let Err(error) = stage.render(&target.render_view, playing.unwrap_or(1.0), |_, _| {}) {
                pane.note = Some(error.to_string());
                self.settling = 0;
            }
            stage.scene.end_frame();
            let aspect = size.0 as f32 / size.1 as f32;
            pane.camera = Some(ViewCamera::of(&stage.scene.camera().camera_data(aspect)));
        }
        // A pass that laid out no image has this from the next, which `settling` asks for.
        pane.texture = self.texture;
        pane.settling = self.settling > 0 || playing.is_some();
    }

    fn close(&mut self, painter: &mut egui_wgpu::Renderer, pane: &mut ViewportPane) {
        if let Some(id) = self.texture.take() {
            painter.free_texture(&id);
        }
        *self = Self::default();
        pane.texture = None;
        pane.wanted = None;
        pane.settling = false;
        pane.play = None;
        pane.playing = false;
        pane.status = None;
        pane.camera = None;
        pane.fixed = None;
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
