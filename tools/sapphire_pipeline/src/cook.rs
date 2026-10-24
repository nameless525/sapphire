//! Cooking: import, bake and validate source assets for a target cook, then write the
//! runtime format.
//!
//! M0 cooks meshes only and recooks everything on every run; the content-hash cache and
//! packing arrive in M1 (spec §5.27).

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sapphire_assets::mesh::{MESH_EXTENSION, MeshAsset, MeshFormatError};

use crate::bake::{self, OutlineSettings};
use crate::import::gltf::{self, GltfImportError};

/// A target cook (spec §2): `desktop` for macOS, Windows and Linux, `mobile` for iOS and
/// Android.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cook {
    Desktop,
    Mobile,
}

impl Cook {
    /// Every cook, in a stable order.
    pub const ALL: [Self; 2] = [Self::Desktop, Self::Mobile];

    /// The name used on command lines and in output directories.
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

/// Everything that shapes cooked output. In M1 these join the content-hash cache key and
/// come from each asset's `.meta` import settings (spec §5.27).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CookSettings {
    pub cook: Cook,
    pub outline: OutlineSettings,
}

impl CookSettings {
    /// `cook` with default bake settings.
    pub fn new(cook: Cook) -> Self {
        Self { cook, outline: OutlineSettings::default() }
    }
}

/// Returned when parsing an unknown cook name.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown cook {0:?}; expected `desktop` or `mobile`")]
pub struct UnknownCook(pub String);

impl FromStr for Cook {
    type Err = UnknownCook;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|cook| cook.name() == s).ok_or_else(|| UnknownCook(s.to_owned()))
    }
}

/// Errors from cooking. Each names the file it concerns.
#[derive(Debug, thiserror::Error)]
pub enum CookError {
    #[error("{} is not a supported mesh source (expected .gltf or .glb)", path.display())]
    UnsupportedSource { path: PathBuf },
    #[error("failed to import {}", path.display())]
    Import {
        path: PathBuf,
        #[source]
        source: GltfImportError,
    },
    #[error("cooked mesh from {} is invalid", path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: MeshFormatError,
    },
    #[error("{} and {} would both cook to {}", first.display(), second.display(), output.display())]
    OutputCollision { first: PathBuf, second: PathBuf, output: PathBuf },
    #[error("failed to read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// One source file and the cooked file it produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookedFile {
    pub source: PathBuf,
    pub output: PathBuf,
}

/// Whether `path` is a mesh source this pipeline imports.
pub fn is_mesh_source(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gltf") || ext.eq_ignore_ascii_case("glb"))
}

/// Imports, bakes and validates one source mesh.
pub fn cook_mesh(source: &Path, settings: &CookSettings) -> Result<MeshAsset, CookError> {
    if !is_mesh_source(source) {
        return Err(CookError::UnsupportedSource { path: source.to_owned() });
    }
    let mut mesh =
        gltf::import(source).map_err(|e| CookError::Import { path: source.to_owned(), source: e })?;
    bake::outline_normals(&mut mesh, &settings.outline);
    match settings.cook {
        // Identical until the mobile cook gets its lower-detail LOD0 and fewer LODs
        // (spec §5.27, M1).
        Cook::Desktop | Cook::Mobile => {}
    }
    mesh.validate().map_err(|e| CookError::Invalid { path: source.to_owned(), source: e })?;
    Ok(mesh)
}

/// Cooks one source mesh to `output` (a `.smesh` file), creating parent directories.
pub fn cook_mesh_file(source: &Path, output: &Path, settings: &CookSettings) -> Result<(), CookError> {
    let mesh = cook_mesh(source, settings)?;
    write_atomically(output, &mesh.to_bytes())
}

/// Cooks every `.gltf`/`.glb` under `source_root` into `out_root`, mirroring relative
/// paths with the `.smesh` extension. Hidden files and directories are skipped. Returns
/// the cooked files in path order.
pub fn cook_dir(
    source_root: &Path,
    out_root: &Path,
    settings: &CookSettings,
) -> Result<Vec<CookedFile>, CookError> {
    let mut sources = Vec::new();
    collect_sources(source_root, &mut sources)?;

    let mut outputs: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut plan = Vec::with_capacity(sources.len());
    for source in sources {
        let relative = source.strip_prefix(source_root).expect("collected under the source root");
        let output = out_root.join(relative).with_extension(MESH_EXTENSION);
        if let Some(first) = outputs.insert(output.clone(), source.clone()) {
            return Err(CookError::OutputCollision { first, second: source, output });
        }
        plan.push(CookedFile { source, output });
    }
    for file in &plan {
        cook_mesh_file(&file.source, &file.output, settings)?;
    }
    Ok(plan)
}

fn collect_sources(dir: &Path, sources: &mut Vec<PathBuf>) -> Result<(), CookError> {
    let read_error = |source| CookError::Read { path: dir.to_owned(), source };
    let mut entries = std::fs::read_dir(dir)
        .map_err(read_error)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(read_error)?;
    entries.sort();
    for path in entries {
        let hidden = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            collect_sources(&path, sources)?;
        } else if is_mesh_source(&path) {
            sources.push(path);
        }
    }
    Ok(())
}

/// Writes via a temporary sibling and a rename, so a watcher (hot reload, M1) never sees
/// a half-written file.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), CookError> {
    let write_error = |source| CookError::Write { path: path.to_owned(), source };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(write_error)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    std::fs::write(&temp, bytes).map_err(write_error)?;
    std::fs::rename(&temp, path).map_err(write_error)
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::samples;
    use crate::test_util::TempDir;

    #[test]
    fn parses_cook_names() {
        for cook in Cook::ALL {
            assert_eq!(cook.name().parse(), Ok(cook));
            assert_eq!(cook.to_string(), cook.name());
        }
        assert_eq!("console".parse::<Cook>(), Err(UnknownCook("console".into())));
    }

    #[test]
    fn cooks_a_tree_into_mirrored_paths() {
        let source = TempDir::new("cook_source");
        let out = TempDir::new("cook_out");
        let nested = source.path().join("characters").join("mannequin");
        samples::write_all(&nested).unwrap();
        samples::write_all(&source.path().join(".hidden")).unwrap();
        std::fs::write(source.path().join("notes.txt"), "not an asset").unwrap();

        for cook in Cook::ALL {
            let out_root = out.path().join(cook.name());
            let cooked = cook_dir(source.path(), &out_root, &CookSettings::new(cook)).unwrap();
            let expected = out_root.join("characters/mannequin/mannequin.smesh");
            assert_eq!(
                cooked,
                [CookedFile { source: nested.join(samples::MANNEQUIN_FILE_NAME), output: expected.clone() }]
            );

            let mesh = MeshAsset::from_bytes(&std::fs::read(&expected).unwrap()).unwrap();
            // Outline normals are baked: at a pedestal corner they differ from the split
            // shading normals.
            let corner = mesh
                .vertices
                .iter()
                .find(|v| v.position == [0.55, 0.0, 0.55])
                .expect("pedestal bottom corner");
            assert_ne!(corner.outline_normal, corner.normal);
            assert!(
                Vec3::from(corner.outline_normal).abs_diff_eq(Vec3::new(1.0, -1.0, 1.0).normalize(), 1e-5)
            );
        }
        assert!(!out.path().join("desktop/.hidden").exists());
    }

    #[test]
    fn settings_reach_the_bake() {
        let source = TempDir::new("cook_settings");
        let path = samples::write_all(source.path()).unwrap().remove(0);
        let default = cook_mesh(&path, &CookSettings::new(Cook::Desktop)).unwrap();
        let coarse = CookSettings {
            cook: Cook::Desktop,
            outline: OutlineSettings::with_weld_tolerance(0.05).unwrap(),
        };
        let welded = cook_mesh(&path, &coarse).unwrap();
        assert_eq!(default.vertices.len(), welded.vertices.len());
        assert_ne!(default.vertices, welded.vertices, "a 5 cm weld must change some outline normals");
    }

    #[test]
    fn reports_bad_inputs_with_their_paths() {
        let source = TempDir::new("cook_errors");
        let desktop = CookSettings::new(Cook::Desktop);
        std::fs::write(source.path().join("model.gltf"), "{}").unwrap();
        std::fs::write(source.path().join("model.glb"), samples::mannequin_glb()).unwrap();
        let err = cook_dir(source.path(), &source.path().join("out"), &desktop).unwrap_err();
        assert!(matches!(err, CookError::OutputCollision { .. }), "{err:?}");

        std::fs::remove_file(source.path().join("model.glb")).unwrap();
        let err = cook_dir(source.path(), &source.path().join("out"), &desktop).unwrap_err();
        assert!(matches!(&err, CookError::Import { path, .. } if path.ends_with("model.gltf")), "{err:?}");

        let err = cook_mesh(Path::new("model.fbx"), &desktop).unwrap_err();
        assert!(matches!(err, CookError::UnsupportedSource { .. }));
    }
}
