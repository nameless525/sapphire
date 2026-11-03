//! [`SceneShow`]: what a caller plays on a scene that the scene knows nothing of.
//!
//! A scene turns entities and clocks into a frame's packet. It doesn't know what moves them: a game owns its
//! fight, its interface and whatever else, and plays them on the scene through this trait, at the moments of a
//! frame only the scene can offer. What it places, it places in the scene's world
//! ([`Scene::world_mut`]) as ordinary entities with the components the scene extracts, so nothing below this
//! line changes for a new kind of game.
//!
//! `()` plays nothing, and costs nothing.

use sapphire_render::{PipelineInventory, UiDrawList, VfxFrame};

use crate::content::Content;
use crate::frame::FrameLaps;
use crate::scene::Scene;

/// What the show has to hear of what a frame's loading changed ([`Scene::follow_content`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentChanges {
    /// A new version of the character's mesh, rig or skeleton.
    pub character: bool,
    /// A new version of a graph or of a clip one names.
    pub graphs: bool,
}

/// What a caller plays on a scene. Every method defaults to nothing, and each frame calls those after the first in this order.
pub trait SceneShow {
    /// Adds what it may draw through that no pack's table can say, for [`Scene::prewarm`]: asked when a renderer's
    /// made, not each frame, so it answers for what it may do, with what it can do now.
    fn declare_pipelines(&self, _inventory: &mut PipelineInventory) {}

    /// After each of the scene's fixed steps, as a [`crate::Stage`] runs them: a step of whatever it steps, with
    /// `laps` to say where that went. An app with a loop of its own calls what it needs itself.
    fn step(&mut self, _scene: &mut Scene, _laps: &mut FrameLaps) {}

    /// A frame's steps are over, `dt` seconds after the last frame's, as a [`crate::Stage`] draws them: at the
    /// latest step, with no device to tell.
    fn end_steps(&mut self, _scene: &mut Scene, _dt: f32) {}

    /// Takes in what this frame's loading changed, from inside [`Scene::follow_content`].
    fn follow_content(&mut self, _scene: &mut Scene, _content: &mut Content, _changes: ContentChanges) {}

    /// Poses what it plays for a frame `alpha` of the way from the previous fixed step to the latest: last in
    /// [`Scene::animate`].
    fn animate(&mut self, _scene: &mut Scene, _content: &Content, _alpha: f32) {}

    /// Places what it plays and aims the camera, at the head of [`Scene::frame_packet`]. With no `aspect` this
    /// frame draws nothing.
    fn present(&mut self, _scene: &mut Scene, _alpha: f32, _aspect: Option<f32>) {}

    /// Writes its effects into the frame's packet, before extraction.
    fn write_effects(&mut self, _frame: &mut VfxFrame, _alpha: f32) {}

    /// Draws its interface over a frame of `size`, once the frame's characters are listed.
    fn draw_interface(&mut self, _size: (u32, u32), _list: &mut UiDrawList) {}

    /// Its debug draws, last in the frame, which go out with the packet.
    fn draw_debug(&mut self) {}
}

impl SceneShow for () {}
