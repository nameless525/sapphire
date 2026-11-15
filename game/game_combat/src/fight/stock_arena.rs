//! Checked immutable inputs for one ordinary two-fighter arena.
//!
//! **The player's body is the arena's, not a kind's (M5 Z3).** The simulation binds the heroine's body by
//! numbers compiled in here, beside [`HERO_HEAD`] and the overhead's geometry: her mesh, rig and graph
//! ([`BODY_MESH`], [`BODY_RIG`], [`BODY_GRAPH`]), her graph's seed, the joints her body touches by and the face
//! it rests on ([`BODY`]). Her playable kind's document (`heroine.playable.ron`) draws her, and a patch may
//! change it, so it supplies none of them: a stand only checks that the kind it draws the player as names these
//! assets, so what's drawn is the body the simulation solved. The assets' own bytes still come from the packs
//! (M4's binding, which the release tool counts as presentation).

use std::time::Duration;

use sapphire_assets::rig::ExpressionPreset;
use sapphire_assets::{AssetId, CpuResident, mesh::Mesh, rig::CharacterRig};
use sapphire_gameplay::{
    ActorId, Simulation,
    body::{BodyAssets, BodyGraphAssets},
};
use sapphire_moves::Enemy;
use sapphire_stage::lookdev::{self, ArenaBody};
use sapphire_stage::{Content, PlayableLook};
use sapphire_time::Stopwatch;

use super::{HERO_HEAD, PLAYER_EYES, overhead_profile};
use crate::enemy;

/// The heroine's model, `content/lookdev/character/character.vrm`, which the simulation binds her body on.
const BODY_MESH: AssetId = AssetId::from_text("6f8a81b6-29c4-485f-b161-847b001d561f");
/// Its rig, which the cook derives from it.
const BODY_RIG: AssetId = AssetId::from_text("4b4073fd-44c5-8437-a712-1cccad486da3");
/// Her graph, `content/lookdev/character/character.animgraph.ron`, whose clips her body plays.
const BODY_GRAPH: AssetId = AssetId::from_text("b7f2c9a4-6e13-4d8a-a5c0-1e94d7b23f68");
/// The rest of what her body binds by: the numbers every committed run was recorded with.
const BODY: ArenaBody<'static> = ArenaBody {
    seed: 1,
    contacts: &[
        "J_Bip_C_Hips",
        "J_Bip_C_Spine",
        "J_Bip_C_UpperChest",
        "J_Bip_C_Neck",
        "J_Bip_C_Head",
        "J_Bip_L_UpperLeg",
        "J_Bip_L_LowerLeg",
        "J_Bip_L_Foot",
        "J_Bip_L_ToeBase",
        "J_Bip_R_UpperLeg",
        "J_Bip_R_LowerLeg",
        "J_Bip_R_Foot",
        "J_Bip_R_ToeBase",
        "J_Bip_L_UpperArm",
        "J_Bip_L_LowerArm",
        "J_Bip_L_Hand",
        "J_Bip_R_UpperArm",
        "J_Bip_R_LowerArm",
        "J_Bip_R_Hand",
    ],
    rest: Some((ExpressionPreset::Relaxed, 0.35)),
};

pub(super) struct StockArenaAssets {
    heroine_mesh: CpuResident<Mesh>,
    heroine: BodyAssets,
    warrior: BodyGraphAssets,
    weapon: CpuResident<Mesh>,
    /// What draws the warrior, from its cooked kind (M3 B3).
    look: enemy::Look,
}

impl StockArenaAssets {
    /// Loads what the simulation binds, for a player drawn as `drawn`, which must name the body's assets:
    /// nothing else of it is read (see the [module docs](self)).
    pub(super) fn load(content: &mut Content, drawn: &PlayableLook) -> Result<Self, String> {
        if (drawn.mesh, drawn.rig, drawn.graph) != (BODY_MESH, BODY_RIG, BODY_GRAPH) {
            return Err(format!(
                "the stock arena's simulation binds the heroine's body, and its player draws as playable kind {}, \
                 on another mesh, rig or graph",
                drawn.kind
            ));
        }
        let stopwatch = Stopwatch::start();
        let remaining = || Duration::from_secs(15).saturating_sub(stopwatch.elapsed());
        let kind =
            content.preload_cpu::<Enemy>(enemy::KIND, &remaining).map_err(|error| error.to_string())?;
        let look = enemy::Look::of(&kind.view().look());
        let blade = look.held.as_ref().map(|held| held.mesh).ok_or("the stock warrior holds no blade")?;
        // The first renderer upload may drain this preload before the arena reads the warrior skin.
        content.keep_mesh_copy(look.mesh);
        let heroine_mesh =
            content.preload_cpu::<Mesh>(BODY_MESH, &remaining).map_err(|error| error.to_string())?;
        let warrior_mesh =
            content.preload_cpu::<Mesh>(look.mesh, &remaining).map_err(|error| error.to_string())?;
        let hero_skeleton = skin_skeleton(&heroine_mesh, "the heroine")?;
        let warrior_skeleton = skin_skeleton(&warrior_mesh, "the warrior")?;
        let heroine_graph = content
            .preload_body_graph(BODY_GRAPH, hero_skeleton, &remaining)
            .map_err(|error| error.to_string())?;
        let warrior = content
            .preload_body_graph(look.graph, warrior_skeleton, &remaining)
            .map_err(|error| error.to_string())?;
        let rig =
            content.preload_cpu::<CharacterRig>(BODY_RIG, &remaining).map_err(|error| error.to_string())?;
        let weapon = content.preload_cpu::<Mesh>(blade, &remaining).map_err(|error| error.to_string())?;
        let heroine = BodyAssets { graph: heroine_graph, rig };
        Ok(Self { heroine_mesh, heroine, warrior, weapon, look })
    }

    /// Installs both owners and the private contact route before recording or checking a table hash.
    /// A failed setup discards this new simulation.
    pub(super) fn install(&self, simulation: &mut Simulation, cast: &[ActorId]) -> Result<(), String> {
        let &[hero, warrior] = cast else { return Err("the stock overhead needs exactly two actors".into()) };
        if simulation.tick() != 0 || simulation.party().is_formed() || hero == warrior {
            return Err("the stock overhead requires a fresh two-fighter arena".into());
        }
        // Both bind on the simulation's table, which a game's `body` document set when the fight started.
        let tuning = simulation.body_tuning();
        let heroine = lookdev::arena_body_profile(
            self.heroine.clone(),
            self.heroine_mesh.view(),
            BODY,
            self.look.eyes,
            HERO_HEAD,
            tuning,
        )?;
        let enemy = enemy::simulation_profile(self.warrior.clone(), &self.look, PLAYER_EYES, tuning)?;
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
