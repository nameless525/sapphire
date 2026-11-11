//! The golden scenes' document, `tests/golden/scenes.ron` (M4 R1): each scene's capture, its gates and its CI
//! times, beside its goldens. The document's own comments say what each field means to someone adding a scene.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::Deserialize;

use super::{SIZE, Scene};
use crate::golden::Thresholds;

/// The document's file name, in the goldens' directory.
pub const FILE_NAME: &str = "scenes.ron";

/// Arguments the runner gives every scene from the document's fields, so `args` and `bases` can't.
const RUNNER_FLAGS: [&str; 3] = ["--capture", "--capture-ticks", "--size"];

/// The document as written.
#[derive(Debug, Deserialize, TypePath)]
#[type_path = "sapphire::testkit"]
#[serde(deny_unknown_fields)]
pub struct GoldenScenes {
    /// Arguments several scenes share, by name: a scene's `base`.
    #[serde(default)]
    bases: BTreeMap<String, Vec<String>>,
    /// In the order they were added.
    scenes: Vec<Row>,
}

impl Versioned for GoldenScenes {
    /// 1: M4 R1.
    const VERSION: u32 = 1;
}

/// One scene as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    name: String,
    /// `<width>x<height>`; [`SIZE`] if absent.
    #[serde(default)]
    size: Option<String>,
    /// The tick the capture is taken at.
    ticks: u32,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    gates: Gates,
    #[serde(default)]
    ci_seconds: Option<CiSeconds>,
}

/// A scene's own FLIP gates, each the default's where absent.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Gates {
    mean: Option<f32>,
    percentile: Option<f32>,
    percentile_max: Option<f32>,
    worst_tile: Option<f32>,
}

/// A scene's median wall time in seconds in CI's golden jobs, on each family `--shard` splits.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiSeconds {
    pub lavapipe: f32,
    pub warp: f32,
}

/// A document that can't be read, or that names a scene the runner can't capture.
#[derive(Debug, thiserror::Error)]
pub enum SceneListError {
    #[error("failed to read {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: {source}")]
    Document { path: PathBuf, source: Box<DocumentError> },
    #[error("{path}: {what}: {problem}")]
    Invalid { path: PathBuf, what: String, problem: String },
}

/// The scenes of the document in `goldens`, in its order.
pub fn read(goldens: &Path) -> Result<Vec<Scene>, SceneListError> {
    let path = goldens.join(FILE_NAME);
    let text =
        std::fs::read_to_string(&path).map_err(|source| SceneListError::Io { path: path.clone(), source })?;
    let document: GoldenScenes = sapphire_serialize::from_ron(&text)
        .map_err(|source| SceneListError::Document { path: path.clone(), source: Box::new(source) })?;
    document.resolve().map_err(|(what, problem)| SceneListError::Invalid { path, what, problem })
}

impl GoldenScenes {
    /// Each row as the runner captures it, or the first thing wrong: what, and why.
    fn resolve(self) -> Result<Vec<Scene>, (String, String)> {
        let mut used = BTreeSet::new();
        for (name, args) in &self.bases {
            runner_flag(args).map_err(|problem| (format!("base {name:?}"), problem))?;
        }
        if self.scenes.is_empty() {
            return Err(("scenes".to_owned(), "lists none".to_owned()));
        }
        let mut names = BTreeSet::new();
        let mut scenes = Vec::with_capacity(self.scenes.len());
        for row in self.scenes {
            let what = format!("scene {:?}", row.name);
            let scene = row.resolve(&self.bases, &mut used).map_err(|problem| (what.clone(), problem))?;
            if !names.insert(scene.name.clone()) {
                return Err((what, "is named twice".to_owned()));
            }
            scenes.push(scene);
        }
        if let Some(unused) = self.bases.keys().find(|name| !used.contains(name.as_str())) {
            return Err((format!("base {unused:?}"), "no scene uses it".to_owned()));
        }
        Ok(scenes)
    }
}

impl Row {
    fn resolve<'b>(
        self,
        bases: &'b BTreeMap<String, Vec<String>>,
        used: &mut BTreeSet<&'b str>,
    ) -> Result<Scene, String> {
        if self.name.is_empty() || !self.name.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
            return Err("a name is lowercase letters and hyphens: it's the goldens' file stem".to_owned());
        }
        let size = self.size.as_deref().unwrap_or(SIZE);
        let parsed = size
            .split_once('x')
            .and_then(|(width, height)| Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?)));
        if !parsed.is_some_and(|(width, height)| width > 0 && height > 0) {
            return Err(format!("size {size:?} isn't <width>x<height> in pixels"));
        }
        runner_flag(&self.args)?;
        let base = match &self.base {
            Some(name) => {
                let (name, args) =
                    bases.get_key_value(name).ok_or_else(|| format!("its base {name:?} isn't in `bases`"))?;
                used.insert(name);
                args.as_slice()
            }
            None => &[],
        };
        let thresholds = self.gates.thresholds()?;
        if let Some(seconds) = self.ci_seconds
            && !(positive(seconds.lavapipe) && positive(seconds.warp))
        {
            return Err("`ci_seconds` are seconds above zero".to_owned());
        }
        let mut args = Vec::with_capacity(4 + base.len() + self.args.len());
        args.extend([
            "--size".to_owned(),
            size.to_owned(),
            "--capture-ticks".to_owned(),
            self.ticks.to_string(),
        ]);
        args.extend(base.iter().cloned());
        args.extend(self.args);
        Ok(Scene { name: self.name, args, thresholds, ci_seconds: self.ci_seconds })
    }
}

impl Gates {
    fn thresholds(&self) -> Result<Thresholds, String> {
        let default = Thresholds::default();
        let gate = |value: Option<f32>, default: f32, field: &str| match value {
            Some(value) if !positive(value) => Err(format!("its gate `{field}` is {value}, not above zero")),
            value => Ok(value.unwrap_or(default)),
        };
        let thresholds = Thresholds {
            mean: gate(self.mean, default.mean, "mean")?,
            percentile: gate(self.percentile, default.percentile, "percentile")?,
            percentile_max: gate(self.percentile_max, default.percentile_max, "percentile_max")?,
            tile_max: gate(self.worst_tile, default.tile_max, "worst_tile")?,
        };
        if thresholds.percentile > 100.0 {
            return Err(format!("its gate `percentile` is {}, past 100", thresholds.percentile));
        }
        Ok(thresholds)
    }
}

fn positive(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

/// An error if `args` give a flag the runner sets from the document's fields.
fn runner_flag(args: &[String]) -> Result<(), String> {
    for arg in args {
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        if RUNNER_FLAGS.contains(&flag) {
            return Err(format!(
                "its arguments give {flag}, which the runner gives from `size` and `ticks`, or from the capture's path"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(scenes: &str) -> Result<Vec<Scene>, (String, String)> {
        let text = format!(
            "(type: \"sapphire::testkit::GoldenScenes\", version: 1, data: (bases: {{\"pins\": [\"--ao\", \"off\"]}}, \
             scenes: [{scenes}]))"
        );
        sapphire_serialize::from_ron::<GoldenScenes>(&text)
            .map_err(|e| ("document".to_owned(), e.to_string()))?
            .resolve()
    }

    #[test]
    fn a_row_becomes_the_runner_s_command_line() {
        let scenes = resolve(
            "(name: \"plain\", ticks: 90, base: \"pins\", args: [\"--fog\", \"off\"]), \
             (name: \"wide\", size: \"1280x720\", ticks: 2, base: \"pins\", gates: (mean: 0.005), \
              ci_seconds: (lavapipe: 1.5, warp: 2.0))",
        )
        .unwrap();
        assert_eq!(
            scenes[0].args,
            ["--size", "640x360", "--capture-ticks", "90", "--ao", "off", "--fog", "off"]
        );
        assert_eq!(scenes[0].thresholds, Thresholds::default());
        assert_eq!(scenes[0].ci_seconds, None);
        assert_eq!(scenes[1].args, ["--size", "1280x720", "--capture-ticks", "2", "--ao", "off"]);
        assert_eq!(scenes[1].thresholds, Thresholds { mean: 0.005, ..Thresholds::default() });
        assert_eq!(scenes[1].ci_seconds, Some(CiSeconds { lavapipe: 1.5, warp: 2.0 }));
    }

    #[test]
    fn a_wrong_row_is_an_error_naming_it() {
        let scene = "base: \"pins\"";
        for (row, problem) in [
            (format!("(name: \"a\", ticks: 1, {scene}), (name: \"a\", ticks: 2, {scene})"), "named twice"),
            (format!("(name: \"Upper\", ticks: 1, {scene})"), "lowercase"),
            (format!("(name: \"digit-2\", ticks: 1, {scene})"), "lowercase"),
            (format!("(name: \"a\", ticks: 1, size: \"640\", {scene})"), "<width>x<height>"),
            (format!("(name: \"a\", ticks: 1, size: \"0x360\", {scene})"), "<width>x<height>"),
            (format!("(name: \"a\", ticks: 1, {scene}, args: [\"--size\", \"8x8\"])"), "--size"),
            (format!("(name: \"a\", ticks: 1, {scene}, args: [\"--capture-ticks=3\"])"), "--capture-ticks"),
            (format!("(name: \"a\", ticks: 1, {scene}, args: [\"--capture\", \"x.png\"])"), "--capture"),
            ("(name: \"a\", ticks: 1, base: \"other\")".to_owned(), "isn't in `bases`"),
            ("(name: \"a\", ticks: 1)".to_owned(), "no scene uses it"),
            (format!("(name: \"a\", ticks: 1, {scene}, gates: (mean: 0.0))"), "`mean`"),
            (format!("(name: \"a\", ticks: 1, {scene}, gates: (percentile: 101.0))"), "past 100"),
            (
                format!("(name: \"a\", ticks: 1, {scene}, ci_seconds: (lavapipe: 1.0, warp: -1.0))"),
                "ci_seconds",
            ),
            // Unknown fields are refused, so a typo can't drop a pin or a gate unseen.
            (format!("(name: \"a\", ticks: 1, {scene}, arg: [\"--fog\", \"off\"])"), "arg"),
            (format!("(name: \"a\", ticks: 1, {scene}, gates: (p99: 0.1))"), "p99"),
            ("(name: \"a\", base: \"pins\")".to_owned(), "ticks"),
        ] {
            let error = resolve(&row).expect_err(&row);
            assert!(error.1.contains(problem), "{row}: {error:?}");
        }
        assert!(resolve("").is_err_and(|error| error.1.contains("lists none")));
    }
}
