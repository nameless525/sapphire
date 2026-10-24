//! `sapphire_pipeline`: command-line front end to the asset pipeline.

use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Args, Parser, Subcommand};
use sapphire_pipeline::bake::{self, OutlineSettings};
use sapphire_pipeline::cook::{self, Cook, CookSettings};
use sapphire_pipeline::samples;

#[derive(Parser)]
#[command(version, about = "Sapphire asset pipeline: generate, import, bake and cook source assets.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write the procedural sample source assets (mannequin.glb) into a directory.
    GenSamples {
        /// Directory to write into; created if missing.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
    /// Cook one .gltf/.glb file into a .smesh file.
    CookMesh {
        /// Source .gltf or .glb file.
        input: PathBuf,
        /// Cooked .smesh file to write.
        output: PathBuf,
        /// Target cook: `desktop` or `mobile`.
        #[arg(long, default_value = "desktop")]
        cook: Cook,
        #[command(flatten)]
        bake: BakeArgs,
    },
    /// Cook every .gltf/.glb under a source tree into mirrored .smesh paths.
    Cook {
        /// Source root to scan recursively (hidden entries are skipped).
        #[arg(long, value_name = "DIR")]
        source: PathBuf,
        /// Output root; relative paths under the source root are mirrored here.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// Target cook: `desktop` or `mobile`.
        #[arg(long)]
        cook: Cook,
        #[command(flatten)]
        bake: BakeArgs,
    },
}

/// Bake options shared by the cook commands.
#[derive(Args)]
struct BakeArgs {
    /// Vertices closer than this many meters share one outline normal.
    #[arg(long, value_name = "METERS", default_value_t = bake::DEFAULT_WELD_TOLERANCE, value_parser = weld_tolerance)]
    weld_tolerance: f32,
}

impl BakeArgs {
    fn settings(&self, cook: Cook) -> CookSettings {
        let outline =
            OutlineSettings::with_weld_tolerance(self.weld_tolerance).expect("validated when parsed");
        CookSettings { cook, outline }
    }
}

fn weld_tolerance(arg: &str) -> Result<f32, String> {
    let meters: f32 = arg.parse().map_err(|e| format!("{e}"))?;
    OutlineSettings::with_weld_tolerance(meters)
        .map(|_| meters)
        .ok_or_else(|| "must be a positive, finite number of meters".to_owned())
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::GenSamples { out } => {
            let written = samples::write_all(&out)
                .with_context(|| format!("failed to write samples to {}", out.display()))?;
            for path in written {
                eprintln!("generated {}", path.display());
            }
        }
        Command::CookMesh { input, output, cook, bake } => {
            cook::cook_mesh_file(&input, &output, &bake.settings(cook))?;
            eprintln!("cooked {} -> {} ({cook})", input.display(), output.display());
        }
        Command::Cook { source, out, cook, bake } => {
            let cooked = cook::cook_dir(&source, &out, &bake.settings(cook))?;
            for file in &cooked {
                eprintln!("cooked {} -> {}", file.source.display(), file.output.display());
            }
            eprintln!("{} asset(s) cooked for {cook} into {}", cooked.len(), out.display());
        }
    }
    Ok(())
}
