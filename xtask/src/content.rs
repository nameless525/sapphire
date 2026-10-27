//! `cargo xtask content`: makes sure the sample sources exist, then cooks and packs `content/`
//! with the pipeline binary (spec §5.27, §7.4).
//!
//! Each cook writes loose cooked files to `target/content/loose/<cook>/` and packs them into
//! `target/content/<cook>/`, which then holds packs only (`shared.spack`): what apps ship and
//! desktop runs read (M1 A4).
//!
//! Every run runs the pipeline, whose content cache (`target/content/cache/`, shared by both
//! cooks) makes it incremental: an asset is cooked only when an input its cache key covers
//! changed (its source bytes, sidecar settings, importer and cooker versions, the cook or
//! a file it references), an asset is compressed for a pack only when its cook changed, an
//! output or pack is written only when its bytes change, and cooked files whose source is gone
//! are removed. Rebuilding the pipeline recooks nothing, so a change to an importer, cooker or
//! the packer must bump that stage's version (see `sapphire_pipeline::cache`).
//!
//! With `--locked` (CI), a source without a committed `.meta` sidecar fails the cook instead
//! of getting a fresh ID that would be thrown away. A lock file per cook keeps concurrent
//! xtask runs from changing content another run is still copying or running against.
//!
//! Generating missing sources comes first, under a lock of its own
//! (`target/content/generate.lock`), taken and released before the cook's: concurrent runs (a
//! desktop and a mobile cook in one checkout) never generate at once, and none cooks while
//! another is generating. The generators write only missing files, so generation never
//! overwrites a hand edit.

use std::fmt;
use std::fs::{self, File, TryLockError};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use anyhow::{Context as _, anyhow};

use crate::workspace::{self, CargoBuild};

/// Source assets live here, relative to the workspace root.
pub const SOURCE_DIR: &str = "content";

/// Generated sample sources, relative to [`SOURCE_DIR`]. `gen-samples` writes them, and the face
/// map's threshold masks (`samples/face/masks/`), which its document names: a cook missing one
/// fails, naming it. It writes only the files that are missing, keeping any that differ from its
/// output (a hand edit), unless run with `--force`.
const SAMPLE_SOURCES: &[&str] = &[
    "samples/mannequin.glb",
    "samples/swatch.ramp.ron",
    "samples/swatch.ilm.ron",
    "samples/swatch_ids.png",
    "samples/swatch_bias.png",
    "samples/swatch_ramp.material.ron",
    "samples/swatch_subsurface.material.ron",
    "samples/swatch_bands.material.ron",
    "samples/face/face_head.glb",
    "samples/face/face.facesdf.ron",
    "samples/face/face_base.png",
    "samples/face/face_mask.png",
    "samples/face/face_expression.png",
    "samples/face/face.material.ron",
    "samples/face/hair.material.ron",
    "samples/face/bangs.material.ron",
    "samples/face/brow.material.ron",
    "samples/face/hair_mask.png",
    "samples/face/eye_white.material.ron",
    "samples/face/eye_shadow.material.ron",
    "samples/face/iris.material.ron",
    "samples/face/eye_highlight.material.ron",
    "samples/face/lash.material.ron",
    "samples/face/lid.material.ron",
    "samples/face/eye_shadow.png",
    "samples/face/iris.png",
    "samples/face/eye_highlight.png",
    "samples/sky_clouds.png",
    "samples/sky_cirrus.png",
    "samples/sky_moon.png",
    "samples/sky_stars.png",
    "samples/sky_milky_way.png",
    "samples/sky.material.ron",
    "samples/environment/courtyard.glb",
    "samples/environment/ground.material.ron",
    "samples/environment/rock.material.ron",
    "samples/environment/stone.material.ron",
    "samples/environment/wall.material.ron",
    "samples/environment/lantern.material.ron",
    "samples/environment/terminator_band.png",
    "samples/environment/ground.png",
    "samples/environment/mist_sheet.glb",
    "samples/environment/haze_card.glb",
    "samples/environment/mist.material.ron",
    "samples/environment/haze.material.ron",
    "samples/environment/mist.png",
    "samples/environment/arcade.glb",
    "samples/environment/courtyard.probes.ron",
    "samples/hard_edges.glb",
    "samples/foliage/tree.glb",
    "samples/foliage/bush.glb",
    "samples/foliage/grass.glb",
    "samples/foliage/bark.material.ron",
    "samples/foliage/leaves.material.ron",
    "samples/foliage/bush.material.ron",
    "samples/foliage/grass.material.ron",
    "samples/foliage/leaves.png",
    "samples/foliage/grass.png",
    "samples/materials/matcap.png",
    "samples/materials/layers.png",
    "samples/materials/lacquer.material.ron",
    "samples/materials/gold.material.ron",
    "samples/materials/leather.material.ron",
    "samples/materials/stockings.material.ron",
    "samples/materials/glow.material.ron",
];

/// Generated look-dev sources (M1 X1), relative to [`SOURCE_DIR`]: the town's painted maps, its
/// plants (W6's foliage samples, one file per look-dev material), the night sky's Milky Way,
/// the character's hair mask, irises, face mask, expression map and neck shadow, and the far
/// mountains' sectors and palette (X1 part 2d). `gen-lookdev` writes those that are missing, as
/// `gen-samples` does.
const LOOKDEV_SOURCES: &[&str] = &[
    "lookdev/foliage/tree_green.glb",
    "lookdev/foliage/tree_blossom.glb",
    "lookdev/foliage/tree_autumn.glb",
    "lookdev/foliage/bush_green.glb",
    "lookdev/foliage/bush_lavender.glb",
    "lookdev/foliage/grass.glb",
    "lookdev/sky/milky_way.png",
    "lookdev/sky/cumulus.png",
    "lookdev/sky/nebula.png",
    "lookdev/town/textures/stone.png",
    "lookdev/town/textures/timber.png",
    "lookdev/town/textures/grass.png",
    "lookdev/town/textures/foliage.png",
    "lookdev/town/textures/band.png",
    "lookdev/town/textures/whitewash.png",
    "lookdev/town/textures/granite.png",
    "lookdev/town/textures/tiles.png",
    "lookdev/town/textures/tiles_mask.png",
    "lookdev/town/textures/rafters.png",
    "lookdev/town/textures/lantern.png",
    "lookdev/town/textures/flagstones.png",
    "lookdev/character/hair_mask.png",
    "lookdev/character/eye_iris.png",
    "lookdev/character/face/face_mask.png",
    "lookdev/character/face/face_expression.png",
    "lookdev/character/neck_shadow.png",
    "lookdev/mountains/near_0.glb",
    "lookdev/mountains/near_1.glb",
    "lookdev/mountains/near_2.glb",
    "lookdev/mountains/near_3.glb",
    "lookdev/mountains/near_4.glb",
    "lookdev/mountains/near_5.glb",
    "lookdev/mountains/near_6.glb",
    "lookdev/mountains/near_7.glb",
    "lookdev/mountains/near_8.glb",
    "lookdev/mountains/near_9.glb",
    "lookdev/mountains/near_10.glb",
    "lookdev/mountains/near_11.glb",
    "lookdev/mountains/near_12.glb",
    "lookdev/mountains/near_13.glb",
    "lookdev/mountains/near_14.glb",
    "lookdev/mountains/near_15.glb",
    "lookdev/mountains/near_16.glb",
    "lookdev/mountains/near_17.glb",
    "lookdev/mountains/near_18.glb",
    "lookdev/mountains/near_19.glb",
    "lookdev/mountains/middle_0.glb",
    "lookdev/mountains/middle_1.glb",
    "lookdev/mountains/middle_2.glb",
    "lookdev/mountains/middle_3.glb",
    "lookdev/mountains/middle_4.glb",
    "lookdev/mountains/middle_5.glb",
    "lookdev/mountains/middle_6.glb",
    "lookdev/mountains/middle_7.glb",
    "lookdev/mountains/middle_8.glb",
    "lookdev/mountains/middle_9.glb",
    "lookdev/mountains/middle_10.glb",
    "lookdev/mountains/middle_11.glb",
    "lookdev/mountains/middle_12.glb",
    "lookdev/mountains/middle_13.glb",
    "lookdev/mountains/middle_14.glb",
    "lookdev/mountains/middle_15.glb",
    "lookdev/mountains/middle_16.glb",
    "lookdev/mountains/middle_17.glb",
    "lookdev/mountains/middle_18.glb",
    "lookdev/mountains/middle_19.glb",
    "lookdev/mountains/far_0.glb",
    "lookdev/mountains/far_1.glb",
    "lookdev/mountains/far_2.glb",
    "lookdev/mountains/far_3.glb",
    "lookdev/mountains/far_4.glb",
    "lookdev/mountains/far_5.glb",
    "lookdev/mountains/far_6.glb",
    "lookdev/mountains/far_7.glb",
    "lookdev/mountains/mountains.png",
];

/// A pipeline command that generates sources, the directory it writes (relative to
/// [`SOURCE_DIR`]) and the files it writes there (relative to [`SOURCE_DIR`] too).
#[derive(Clone, Copy, Debug)]
struct Generator {
    command: &'static str,
    dir: &'static str,
    sources: &'static [&'static str],
}

/// The generators `cook` runs when one of their files is missing.
const GENERATORS: [Generator; 2] = [
    Generator { command: "gen-samples", dir: "samples", sources: SAMPLE_SOURCES },
    Generator { command: "gen-lookdev", dir: "lookdev", sources: LOOKDEV_SOURCES },
];

/// A target cook (spec §2): `desktop` for macOS, Windows and Linux, `mobile` for iOS and
/// Android. Mirrors `sapphire_pipeline::cook::Cook`; xtask runs the pipeline as a
/// process rather than linking it, since nothing may depend on a tool crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cook {
    Desktop,
    Mobile,
}

impl Cook {
    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Mobile => "mobile",
        }
    }
}

impl fmt::Display for Cook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Cook {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        [Self::Desktop, Self::Mobile]
            .into_iter()
            .find(|cook| cook.name() == s)
            .ok_or_else(|| anyhow!("unknown cook `{s}`; expected `desktop` or `mobile`"))
    }
}

/// Where a cook's packs go: `target/content/<cook>/`, the content root apps ship.
pub fn cooked_dir(cook: Cook) -> PathBuf {
    workspace::output_dir().join("content").join(cook.name())
}

/// Where a cook's loose cooked files go: `target/content/loose/<cook>/`, which tools read and
/// which never ships.
fn loose_dir(cook: Cook) -> PathBuf {
    workspace::output_dir().join("content").join("loose").join(cook.name())
}

/// The content cache both cooks share: `target/content/cache/`. Keys include the cook.
fn cache_dir() -> PathBuf {
    workspace::output_dir().join("content").join("cache")
}

/// A cooked content directory, held under a shared lock: no other xtask run recooks it
/// until this is dropped.
#[derive(Debug)]
pub struct Cooked {
    pub dir: PathBuf,
    _lock: File,
}

/// Generates missing sample and look-dev sources, then cooks all of `content/` for `cook`. Keep the
/// result alive while copying from or running against its directory.
pub fn cook(cook: Cook) -> anyhow::Result<Cooked> {
    // Release: cooking is the slow part, and the pipeline itself rarely changes.
    let pipeline = workspace::build_binary(
        &CargoBuild { package: "sapphire_pipeline", target: None, release: true, features: &[], env: &[] },
        "sapphire_pipeline",
    )?;

    let source = workspace::root().join(SOURCE_DIR);
    fs::create_dir_all(workspace::output_dir().join("content"))?;
    generate_missing(&generate_lock(), &source, &GENERATORS, &mut |generator| {
        workspace::run(
            Command::new(&pipeline).arg(generator.command).arg("--out").arg(source.join(generator.dir)),
        )
    })?;

    let out = cooked_dir(cook);
    let lock_path = out.with_extension("lock");
    let lock =
        File::create(&lock_path).with_context(|| format!("failed to create {}", lock_path.display()))?;
    wait_for(&lock, File::try_lock, File::lock, &format!("to finish with the {cook} content"))?;
    let mut command = Command::new(&pipeline);
    command
        .arg("cook")
        .arg("--source")
        .arg(&source)
        .arg("--out")
        .arg(loose_dir(cook))
        .arg("--packs")
        .arg(&out)
        .args(["--cook", cook.name()])
        .arg("--cache")
        .arg(cache_dir());
    if workspace::locked() {
        command.arg("--locked");
    }
    workspace::run(&mut command)?;
    // Another run may recook in the gap before the shared lock; its output is the same.
    lock.unlock()?;
    wait_for(&lock, File::try_lock_shared, File::lock_shared, &format!("to finish with the {cook} content"))?;
    Ok(Cooked { dir: out, _lock: lock })
}

/// The lock generation runs under: `target/content/generate.lock`.
fn generate_lock() -> PathBuf {
    workspace::output_dir().join("content").join("generate.lock")
}

/// Runs `generate` for each of `generators` with a file missing under `source`, holding the
/// lock file at `lock` from before the first check until the last generator is done (review
/// BC-007). A run that waited for the lock checks once it has it, so it finds what the other
/// run generated. Returns the generators that ran.
///
/// The lock is released before the cook takes its own, so the two are never nested: a run
/// holding a cook's lock (an app running against its content) never blocks another run's
/// generation, and generation never waits on a cook.
fn generate_missing(
    lock: &Path,
    source: &Path,
    generators: &[Generator],
    generate: &mut dyn FnMut(&Generator) -> anyhow::Result<()>,
) -> anyhow::Result<Vec<&'static str>> {
    let file = File::create(lock).with_context(|| format!("failed to create {}", lock.display()))?;
    wait_for(&file, File::try_lock, File::lock, "to finish generating sources")?;
    let mut ran = Vec::new();
    for generator in generators {
        if generator.sources.iter().any(|file| !source.join(file).exists()) {
            generate(generator)?;
            ran.push(generator.command);
        }
    }
    file.unlock()?;
    Ok(ran)
}

/// Takes a lock with `try_lock`, falling back to the blocking `lock` with a message when
/// another xtask run holds it: "waiting for another xtask run `until`...".
fn wait_for(
    file: &File,
    try_lock: fn(&File) -> Result<(), TryLockError>,
    lock: fn(&File) -> std::io::Result<()>,
    until: &str,
) -> anyhow::Result<()> {
    match try_lock(file) {
        Ok(()) => Ok(()),
        Err(TryLockError::WouldBlock) => {
            eprintln!("waiting for another xtask run {until}...");
            Ok(lock(file)?)
        }
        Err(TryLockError::Error(error)) => Err(error).context("failed to lock content"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn parses_cooks_and_places_their_output() {
        assert_eq!("desktop".parse::<Cook>().unwrap(), Cook::Desktop);
        assert_eq!("mobile".parse::<Cook>().unwrap(), Cook::Mobile);
        assert!("console".parse::<Cook>().is_err());
        assert!(cooked_dir(Cook::Mobile).ends_with("target/content/mobile"));
        assert!(loose_dir(Cook::Mobile).ends_with("target/content/loose/mobile"));
        assert!(cache_dir().ends_with("target/content/cache"));
        assert!(generate_lock().ends_with("target/content/generate.lock"));
    }

    #[test]
    fn concurrent_runs_generate_once() {
        // Review BC-007: two runs that find a source missing must not both generate, nor go on to
        // cook while the other is still generating.
        const SOURCES: &[&str] = &["gen/a.txt", "gen/nested/b.txt"];
        let generators = [Generator { command: "gen-test", dir: "gen", sources: SOURCES }];
        let dir = std::env::temp_dir().join(format!("xtask-generate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("gen")).unwrap();
        fs::write(dir.join(SOURCES[0]), "kept").unwrap();
        let lock = dir.join("generate.lock");
        let runs = AtomicUsize::new(0);
        let start = Barrier::new(2);
        let ran: Vec<Vec<&str>> = thread::scope(|scope| {
            let threads: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        let ran = generate_missing(&lock, &dir, &generators, &mut |generator| {
                            runs.fetch_add(1, Ordering::SeqCst);
                            // A slow generator, which writes only what's missing.
                            thread::sleep(Duration::from_millis(200));
                            for file in generator.sources {
                                let path = dir.join(file);
                                if !path.exists() {
                                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                                    fs::write(path, "generated").unwrap();
                                }
                            }
                            Ok(())
                        })
                        .unwrap();
                        // Whichever run generated, every source is there once either goes on.
                        assert!(SOURCES.iter().all(|file| dir.join(file).exists()));
                        ran
                    })
                })
                .collect();
            threads.into_iter().map(|thread| thread.join().unwrap()).collect()
        });
        assert_eq!(runs.load(Ordering::SeqCst), 1, "one run generates");
        assert_eq!(ran.concat(), ["gen-test"]);
        assert_eq!(fs::read_to_string(dir.join(SOURCES[0])).unwrap(), "kept");

        // Nothing missing: nothing runs.
        let ran = generate_missing(&lock, &dir, &generators, &mut |_| panic!("nothing is missing")).unwrap();
        assert!(ran.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
