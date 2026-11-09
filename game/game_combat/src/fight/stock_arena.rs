//! Checked immutable inputs for one ordinary two-fighter arena.

use std::time::Duration;

use sapphire_assets::{AssetId, CpuResident, mesh::Mesh, rig::CharacterRig};
use sapphire_gameplay::{
    ActorId, Simulation,
    body::{BodyAssets, BodyGraphAssets},
};
use sapphire_moves::Enemy;
use sapphire_stage::{Content, lookdev};
use sapphire_time::Stopwatch;

use super::{HERO_HEAD, PLAYER_EYES, overhead_profile};
use crate::enemy;

pub(super) struct StockArenaAssets {
    heroine_mesh: CpuResident<Mesh>,
    heroine: BodyAssets,
    warrior: BodyGraphAssets,
    weapon: CpuResident<Mesh>,
    /// What draws the warrior, from its cooked kind (M3 B3).
    look: enemy::Look,
}

impl StockArenaAssets {
    pub(super) fn load(content: &mut Content) -> Result<Self, String> {
        let stopwatch = Stopwatch::start();
        let remaining = || Duration::from_secs(15).saturating_sub(stopwatch.elapsed());
        let kind =
            content.preload_cpu::<Enemy>(enemy::KIND, &remaining).map_err(|error| error.to_string())?;
        let look = enemy::Look::of(&kind.view().look());
        let blade = look.held.as_ref().map(|held| held.mesh).ok_or("the stock warrior holds no blade")?;
        // The first renderer upload may drain this preload before the arena reads the warrior skin.
        content.keep_mesh_copy(look.mesh);
        let heroine_mesh =
            content.preload_cpu::<Mesh>(lookdev::CHARACTER, &remaining).map_err(|error| error.to_string())?;
        let warrior_mesh =
            content.preload_cpu::<Mesh>(look.mesh, &remaining).map_err(|error| error.to_string())?;
        let hero_skeleton = skin_skeleton(&heroine_mesh, "the heroine")?;
        let warrior_skeleton = skin_skeleton(&warrior_mesh, "the warrior")?;
        let heroine_graph = content
            .preload_body_graph(lookdev::GRAPH, hero_skeleton, &remaining)
            .map_err(|error| error.to_string())?;
        let warrior = content
            .preload_body_graph(look.graph, warrior_skeleton, &remaining)
            .map_err(|error| error.to_string())?;
        let rig = content
            .preload_cpu::<CharacterRig>(lookdev::RIG, &remaining)
            .map_err(|error| error.to_string())?;
        let weapon = content.preload_cpu::<Mesh>(blade, &remaining).map_err(|error| error.to_string())?;
        Ok(Self { heroine_mesh, heroine: BodyAssets { graph: heroine_graph, rig }, warrior, weapon, look })
    }

    /// Installs both owners and the private contact route before recording or checking a table hash.
    /// A failed setup discards this new simulation.
    pub(super) fn install(&self, simulation: &mut Simulation, cast: &[ActorId]) -> Result<(), String> {
        let &[hero, warrior] = cast else { return Err("the stock overhead needs exactly two actors".into()) };
        if simulation.tick() != 0 || simulation.party().is_formed() || hero == warrior {
            return Err("the stock overhead requires a fresh two-fighter arena".into());
        }
        let heroine = lookdev::arena_body_profile(
            self.heroine.clone(),
            self.heroine_mesh.view(),
            self.look.eyes,
            HERO_HEAD,
        )?;
        let enemy = enemy::simulation_profile(self.warrior.clone(), &self.look, PLAYER_EYES)?;
        let socket = self
            .warrior
            .skeleton
            .view()
            .find(self.look.held.as_ref().map_or("", |held| held.joint.as_str()))
            .and_then(|joint| u16::try_from(joint).ok())
            .ok_or("the cooked warrior has no supported hand socket")?;
        let (shape, meshes) = overhead_profile::bind(socket, [&self.heroine_mesh, &self.weapon])?;
        simulation.attach_body(hero, Some(warrior), heroine).map_err(|error| error.to_string())?;
        simulation.attach_body(warrior, Some(hero), enemy).map_err(|error| error.to_string())?;
        if !simulation.attach_overhead_application(warrior, hero, shape, meshes) {
            return Err(
                "the stock overhead could not bind its authored attack or the protected bodies".into()
            );
        }
        Ok(())
    }
}

fn skin_skeleton(mesh: &CpuResident<Mesh>, actor: &str) -> Result<AssetId, String> {
    mesh.view()
        .skin
        .as_ref()
        .and_then(|skin| skin.skeleton.as_ref().copied())
        .ok_or_else(|| format!("{actor} needs a cooked skin skeleton for overhead binding"))
}
