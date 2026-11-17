//! The voice-over pipeline (M6 U11, spec §5.15): WAV masters to committed Ogg Vorbis, each language's
//! `VoiceLines` written from its folder, a recording script a language and an import report.
//!
//! Voice over lives in `content/vo/`:
//!
//! - **`script.ron`**, written by hand: every voiced line, by key, with the Fluent message that is its
//!   text, the one that names its speaker (none for a fight's line, said by whoever is in play), the scene
//!   it's said in, the direction an actor reads it with, and the languages whose takes are approved; the
//!   source language and every language voiced. A conversation's `vo` keys must be the script's, with its
//!   speaker and line, or `vo` fails naming both.
//! - **`<locale>/<key>.wav`,** a line's master, as a studio delivers it: mono, 16 or 24-bit or 32-bit float
//!   ([`wav`]). Generated lines' masters are `sapphire_soundgen`'s, from `tools/sounds/`.
//!
//! `sapphire_pipeline vo --content content` imports every master and writes, beside it, what the cook and
//! a studio read; none of these is edited by hand, and each is committed:
//!
//! - **`<key>.ogg`,** the master encoded by libvorbis ([`encode::encode_line`], quality
//!   [`encode::QUALITY`], as `sapphire_soundgen` encodes), with a sidecar asking for its mouth curve, and **`<key>.cue.ron`**, the
//!   line's cue on the voice bus, with its own sidecar. The encoder is never in a cook: libvorbis's floating
//!   point isn't the same on every machine, so the Ogg is committed and cooks copy it, the same bytes
//!   everywhere. A master is encoded when its Ogg is missing or the import report says it came from other
//!   bytes; a cue that exists is kept, so its level is the mix's to tune.
//! - **`<locale>.voice.ron`,** the language's [`VoiceLines`], every recorded line of the script in the
//!   script's order, by its cue's ID. A table's ID is its sidecar's and never changes, so what names it
//!   (the cue table's `voices`) holds through every import.
//! - **`<locale>/script.csv`,** the recording script ([`SCRIPT_COLUMNS`]): each line's key, speaker, source
//!   text, text in the language, context, direction, length in the source language and in this one, and
//!   status (`missing`, `untranslated`, `recorded` or `approved`). UTF-8 with a byte-order mark, which a
//!   spreadsheet needs to read Japanese.
//! - **`<locale>/import.csv`,** the import report ([`REPORT_COLUMNS`]): each line's master format, rate,
//!   length against the source language's, peak and RMS level, the Ogg's size, and the master's and the
//!   Ogg's BLAKE3, which is how an import knows a master changed and `--check` that nothing is stale.
//!
//! A master with no line in the script fails, naming it; a line with no master is listed by key as
//! [`Imported::missing`], and the command fails after writing everything else. `--check` writes nothing
//! and fails on any file an import would change, which the tests run on the committed tree.
//!
//! The cook packs each language's lines, their cues and their sounds as `vo_<locale>` (`crate::pack`), which
//! an app mounts for the languages it speaks.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use fluent_syntax::ast;
use fluent_syntax::parser::parse;
use sapphire_assets::voice::{VoiceLine, VoiceLines, is_line_key, is_locale_tag};
use sapphire_dialogue::DialogueDocument;
use sapphire_math::det;
use sapphire_reflect::Versioned;
use sapphire_serialize::{DocumentError, from_ron};
use serde::{Deserialize, Serialize};

use crate::audio::{CueSettings, SoundSettings, VoiceLinesSettings};
use crate::loc::FluentSettings;
use crate::meta::{self, AssetMeta, ImportSettings, MetaError};

pub mod encode;
pub mod wav;

use wav::{Master, WavError};

/// The folder under the content directory voice over lives in.
pub const FOLDER: &str = "vo";
/// The script's file name in [`FOLDER`].
pub const SCRIPT: &str = "script.ron";
/// The recording script's file name in each language's folder.
pub const SCRIPT_CSV: &str = "script.csv";
/// The import report's file name in each language's folder.
pub const REPORT_CSV: &str = "import.csv";

/// The recording script's columns.
pub const SCRIPT_COLUMNS: [&str; 9] =
    ["key", "speaker", "source text", "text", "context", "direction", "source seconds", "seconds", "status"];
/// The import report's columns.
pub const REPORT_COLUMNS: [&str; 11] = [
    "key",
    "status",
    "format",
    "rate",
    "seconds",
    "of source",
    "peak dBFS",
    "RMS dBFS",
    "ogg bytes",
    "master blake3",
    "ogg blake3",
];

/// What a fight's line's speaker reads as in the recording script: whoever is in play.
const IN_PLAY: &str = "(whoever is in play)";
/// A new line's cue's level, as the generated lines' cues have it.
const LINE_VOLUME_DB: f32 = -4.0;
/// What a spreadsheet takes for UTF-8.
const BYTE_ORDER_MARK: &str = "\u{feff}";

/// The script: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct VoiceScript {
    /// The language the lines are written in, whose takes the others are timed against.
    pub source: String,
    /// Every voiced language, each a folder beside the script: the source first.
    pub languages: Vec<String>,
    pub lines: Vec<ScriptLine>,
}

impl Versioned for VoiceScript {
    const VERSION: u32 = 1;
}

/// One voiced line of the [`VoiceScript`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptLine {
    /// What a game asks for: the master's file name, and the line's key in each `VoiceLines`.
    pub key: String,
    /// The Fluent message it says, in every language.
    pub text: String,
    /// The Fluent message naming who says it, or none for whoever is in play.
    #[serde(default)]
    pub speaker: Option<String>,
    /// Where it's said, for an actor and a director.
    pub context: String,
    /// How it's read.
    pub direction: String,
    /// The languages whose takes are approved.
    #[serde(default)]
    pub approved: Vec<String>,
}

/// Whether `vo` writes or only looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Import every master and write what's out of date.
    Write,
    /// Write nothing; list what an import would change as [`Imported::stale`].
    Check,
}

/// What an import did, or a check found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Imported {
    /// Files written ([`Mode::Write`]) or out of date ([`Mode::Check`]).
    pub stale: Vec<PathBuf>,
    /// Masters encoded ([`Mode::Write`]) or to encode ([`Mode::Check`]).
    pub encoded: Vec<PathBuf>,
    /// Each line with no master, as (language, key).
    pub missing: Vec<(String, String)>,
    /// Every master, as (language, key, seconds).
    pub recorded: Vec<(String, String, f64)>,
}

/// Voice over that can't be imported. Each names its file.
#[derive(Debug, thiserror::Error)]
pub enum VoError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{}: {source}", path.display())]
    Document {
        path: PathBuf,
        #[source]
        source: Box<DocumentError>,
    },
    #[error("{}: {problem}", path.display())]
    Problem { path: PathBuf, problem: String },
    #[error("{}: {source}", path.display())]
    Master {
        path: PathBuf,
        #[source]
        source: WavError,
    },
    #[error("{}: encoding: {source:#}", path.display())]
    Encode { path: PathBuf, source: anyhow::Error },
    #[error(transparent)]
    Meta(#[from] MetaError),
}

fn problem(path: &Path, problem: impl Into<String>) -> VoError {
    VoError::Problem { path: path.to_owned(), problem: problem.into() }
}

fn read(path: &Path) -> Result<Vec<u8>, VoError> {
    fs::read(path).map_err(|source| VoError::Io { path: path.to_owned(), source })
}

fn read_text(path: &Path) -> Result<String, VoError> {
    String::from_utf8(read(path)?).map_err(|_| problem(path, "isn't UTF-8"))
}

/// Reads the script at `path` and checks every value.
pub fn read_script(path: &Path) -> Result<VoiceScript, VoError> {
    let script: VoiceScript = from_ron(&read_text(path)?)
        .map_err(|source| VoError::Document { path: path.to_owned(), source: Box::new(source) })?;
    let field = |field: &str, why: String| problem(path, format!("`{field}`: {why}"));
    if script.languages.first() != Some(&script.source) {
        return Err(field("languages", format!("the source, {:?}, comes first", script.source)));
    }
    for (index, language) in script.languages.iter().enumerate() {
        if !is_locale_tag(language) || script.languages[..index].contains(language) {
            return Err(field("languages", format!("{language:?} isn't a language tag, or is there twice")));
        }
    }
    for (index, line) in script.lines.iter().enumerate() {
        let at = format!("lines[{index}]");
        if !is_line_key(&line.key) {
            return Err(field(&at, format!("{:?} isn't a line's key", line.key)));
        }
        if script.lines[..index].iter().any(|earlier| earlier.key == line.key) {
            return Err(field(&at, format!("`{}` is there twice", line.key)));
        }
        if let Some(language) = line.approved.iter().find(|language| !script.languages.contains(language)) {
            return Err(field(
                &at,
                format!("`{}` is approved in {language:?}, which isn't voiced", line.key),
            ));
        }
    }
    Ok(script)
}

/// Imports the voice over under `content`'s [`FOLDER`] (see the [module docs](self)).
pub fn import(content: &Path, mode: Mode) -> Result<Imported, VoError> {
    let folder = content.join(FOLDER);
    let script_path = folder.join(SCRIPT);
    let script = read_script(&script_path)?;
    let strings = strings(content, &script.languages)?;
    check_dialogues(content, &script)?;
    let mut imported = Imported::default();
    let mut writer = Writer { mode, imported: &mut imported };

    // Each language's masters, read and checked against the script, before anything is written.
    let mut takes: Vec<BTreeMap<String, Take>> = Vec::with_capacity(script.languages.len());
    for language in &script.languages {
        let dir = folder.join(language);
        let mut found = BTreeMap::new();
        for path in files(&dir, ".wav")? {
            let key = stem(&path, ".wav");
            if !script.lines.iter().any(|line| line.key == key) {
                return Err(problem(&path, format!("`{key}` isn't a line of {}", script_path.display())));
            }
            let bytes = read(&path)?;
            let master =
                wav::read(&bytes).map_err(|source| VoError::Master { path: path.clone(), source })?;
            found.insert(key, Take { hash: blake3::hash(&bytes).to_hex().to_string(), master });
        }
        for path in files(&dir, ".ogg")? {
            if !found.contains_key(&stem(&path, ".ogg")) {
                return Err(problem(
                    &path,
                    "has no master beside it: an Ogg here is only ever a master's encode",
                ));
            }
        }
        takes.push(found);
    }

    let source_seconds = |key: &str| takes[0].get(key).map(Take::seconds);
    for ((language, found), strings_of) in script.languages.iter().zip(&takes).zip(&strings) {
        let dir = folder.join(language);
        let earlier = read_report(&dir.join(REPORT_CSV))?;
        let mut lines = Vec::new();
        let mut report = csv_row(&REPORT_COLUMNS);
        let mut recording = format!("{BYTE_ORDER_MARK}{}", csv_row(&SCRIPT_COLUMNS));
        for line in &script.lines {
            let take = found.get(&line.key);
            let text = strings_of.get(&line.text);
            if !strings[0].contains_key(&line.text) {
                return Err(problem(
                    &script_path,
                    format!("`{}`'s text `{}` has no {} string", line.key, line.text, script.source),
                ));
            }
            let status = match (take, text) {
                (Some(_), _) if line.approved.contains(language) => "approved",
                (Some(_), _) => "recorded",
                (None, None) => "untranslated",
                (None, Some(_)) => "missing",
            };
            if take.is_none() && line.approved.contains(language) {
                return Err(problem(
                    &script_path,
                    format!("`{}` is approved in {language} with no master", line.key),
                ));
            }
            let speaker = match &line.speaker {
                Some(key) => strings_of
                    .get(key)
                    .or_else(|| strings[0].get(key))
                    .ok_or_else(|| {
                        problem(
                            &script_path,
                            format!("`{}`'s speaker `{key}` has no {} string", line.key, script.source),
                        )
                    })?
                    .clone(),
                None => IN_PLAY.to_owned(),
            };
            let seconds = take.map(Take::seconds);
            recording.push_str(&csv_row(&[
                line.key.as_str(),
                &speaker,
                &strings[0][&line.text],
                text.map_or("", String::as_str),
                &line.context,
                &line.direction,
                &seconds_cell(source_seconds(&line.key)),
                &seconds_cell(seconds),
                status,
            ]));
            let Some(take) = take else {
                writer.imported.missing.push((language.clone(), line.key.clone()));
                report.push_str(&csv_row(&[line.key.as_str(), status, "", "", "", "", "", "", "", "", ""]));
                continue;
            };
            writer.imported.recorded.push((language.clone(), line.key.clone(), take.seconds()));
            let ogg_path = dir.join(format!("{}.ogg", line.key));
            // Kept only if the report says it's this master's encode, as it was.
            let ogg = match (fs::read(&ogg_path), earlier.get(&line.key)) {
                (Ok(ogg), Some(row))
                    if row.master == take.hash && row.ogg == blake3::hash(&ogg).to_hex().as_str() =>
                {
                    ogg
                }
                _ => writer.encode(&ogg_path, &format!("{FOLDER}/{language}/{}.ogg", line.key), take)?,
            };
            let sound = writer.sidecar(&ogg_path, ImportSettings::Sound(SoundSettings { mouth: true }))?;
            let cue_path = dir.join(format!("{}.cue.ron", line.key));
            if !cue_path.exists() {
                writer.file(&cue_path, cue_document(&line.key, language, sound).as_bytes())?;
            }
            let cue = writer.sidecar(&cue_path, ImportSettings::Cue(CueSettings::default()))?;
            lines.push(VoiceLine { key: line.key.clone(), cue });
            let of_source = match source_seconds(&line.key) {
                Some(source) if source > 0.0 => format!("{:.2}", take.seconds() / source),
                _ => String::new(),
            };
            let master = &take.master;
            report.push_str(&csv_row(&[
                line.key.as_str(),
                status,
                master.format.name(),
                &master.sample_rate.to_string(),
                &seconds_cell(Some(take.seconds())),
                &of_source,
                &decibels(peak(&master.samples)),
                &decibels(mean_square(&master.samples).sqrt() as f32),
                &ogg.len().to_string(),
                &take.hash,
                blake3::hash(&ogg).to_hex().as_ref(),
            ]));
        }
        writer.file(&dir.join(SCRIPT_CSV), recording.as_bytes())?;
        writer.file(&dir.join(REPORT_CSV), report.as_bytes())?;
        let table_path = folder.join(format!("{language}.voice.ron"));
        if lines.is_empty() {
            if table_path.exists() {
                return Err(problem(
                    &table_path,
                    format!("names lines, but {} holds no master", dir.display()),
                ));
            }
            continue;
        }
        let table = VoiceLines { locale: language.clone(), lines };
        table.check().map_err(|why| problem(&table_path, why.to_string()))?;
        writer.file(&table_path, voice_document(&table).as_bytes())?;
        writer.sidecar(&table_path, ImportSettings::VoiceLines(VoiceLinesSettings::default()))?;
    }
    Ok(imported)
}

/// A master, read.
struct Take {
    hash: String,
    master: Master,
}

impl Take {
    fn seconds(&self) -> f64 {
        self.master.samples.len() as f64 / f64::from(self.master.sample_rate)
    }
}

/// Writes, or in [`Mode::Check`] notes, what an import changes.
struct Writer<'a> {
    mode: Mode,
    imported: &'a mut Imported,
}

impl Writer<'_> {
    /// `path` holds `bytes`, or is stale.
    fn file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), VoError> {
        if fs::read(path).is_ok_and(|existing| existing == bytes) {
            return Ok(());
        }
        self.imported.stale.push(path.to_owned());
        if self.mode == Mode::Write {
            meta::replace_file(path, bytes)
                .map_err(|source| VoError::Io { path: path.to_owned(), source })?;
        }
        Ok(())
    }

    /// The master `take` encoded as the Ogg at `path`, whose stream serial is a hash of `relative`, its path
    /// under the content directory.
    fn encode(&mut self, path: &Path, relative: &str, take: &Take) -> Result<Vec<u8>, VoError> {
        let failed = |source| VoError::Encode { path: path.to_owned(), source };
        let serial = encode::serial_of(relative);
        let bytes =
            encode::encode_line(&take.master.samples, take.master.sample_rate, serial).map_err(failed)?;
        self.imported.encoded.push(path.to_owned());
        self.file(path, &bytes)?;
        Ok(bytes)
    }

    /// The ID of `source`'s sidecar, which is created with `importer` if it's missing. One that's there
    /// keeps its ID and settings: the cook says if they're wrong.
    fn sidecar(
        &mut self,
        source: &Path,
        importer: ImportSettings,
    ) -> Result<sapphire_assets::AssetId, VoError> {
        let path = meta::sidecar_path(source);
        if path.exists() {
            return Ok(meta::read(&path)?.id);
        }
        self.imported.stale.push(path.clone());
        let id = meta::new_id().map_err(|error| problem(&path, format!("no random ID: {error}")))?;
        if self.mode == Mode::Write {
            meta::create(&path, &AssetMeta::new(id, importer))?;
        }
        Ok(id)
    }
}

/// A new line's cue: its sound on the voice bus at [`LINE_VOLUME_DB`].
fn cue_document(key: &str, language: &str, sound: sapphire_assets::AssetId) -> String {
    format!(
        "// `{key}` in `{language}`: its master's take, as `sapphire_pipeline vo` first wrote it (M6 U11). Its level and\n\
         // pitch are the mix's to tune, and an import keeps them.\n\
         (\n    type: \"sapphire::assets::Cue\",\n    version: 1,\n    data: (\n        sounds: [\"{sound}\"],\n        \
         volume_db: ({LINE_VOLUME_DB:?}, {LINE_VOLUME_DB:?}),\n        pitch: (1.0, 1.0),\n        bus: Voice,\n    ),\n)\n"
    )
}

/// A language's voice-lines document, as an import writes it.
fn voice_document(table: &VoiceLines) -> String {
    let mut text = format!(
        "// `{}`'s voice lines, written by `sapphire_pipeline vo` from `{SCRIPT}` and the masters in `{}/` (M6 U11):\n\
         // every recorded line, in the script's order. Edit those and import again, never this.\n\
         (\n    type: \"sapphire::assets::VoiceLines\",\n    version: 1,\n    data: (\n        locale: {:?},\n        lines: [\n",
        table.locale, table.locale, table.locale
    );
    for line in &table.lines {
        let _ = writeln!(text, "            (key: {:?}, cue: \"{}\"),", line.key, line.cue);
    }
    text.push_str("        ],\n    ),\n)\n");
    text
}

/// One row of an earlier import's report: the hashes it recorded.
struct ReportRow {
    master: String,
    ogg: String,
}

/// The rows of the report at `path`, by key: none if there's no report yet.
fn read_report(path: &Path) -> Result<BTreeMap<String, ReportRow>, VoError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(source) => return Err(VoError::Io { path: path.to_owned(), source }),
    };
    let column = |name| REPORT_COLUMNS.iter().position(|&column| column == name).expect("a column");
    let (master, ogg) = (column("master blake3"), column("ogg blake3"));
    let mut rows = BTreeMap::new();
    // The columns this reads back hold keys and hashes, which a CSV never quotes.
    for line in text.lines().skip(1) {
        let cells: Vec<&str> = line.split(',').collect();
        if cells.len() != REPORT_COLUMNS.len() {
            return Err(problem(
                path,
                format!("`{line}` isn't a row of the report: import again after deleting it"),
            ));
        }
        rows.insert(
            cells[0].to_owned(),
            ReportRow { master: cells[master].to_owned(), ogg: cells[ogg].to_owned() },
        );
    }
    Ok(rows)
}

/// Each of `languages`' strings, by key, from every `.ftl` under `content` whose sidecar (or, with none,
/// whose folder) names it: each message's value as its source has it, a placeable as `{ $name }`.
fn strings(content: &Path, languages: &[String]) -> Result<Vec<BTreeMap<String, String>>, VoError> {
    let mut strings = vec![BTreeMap::new(); languages.len()];
    for path in walk(content, ".ftl")? {
        let sidecar = meta::sidecar_path(&path);
        let locale = if sidecar.exists() {
            match meta::read(&sidecar)?.importer {
                ImportSettings::Fluent(settings) => settings.locale,
                _ => continue,
            }
        } else {
            FluentSettings::for_source(&path).locale
        };
        let Some(index) = languages.iter().position(|language| *language == locale) else { continue };
        let text = read_text(&path)?;
        let resource =
            parse(text.as_str()).map_err(|_| problem(&path, "doesn't parse: cook it to see why"))?;
        for entry in &resource.body {
            if let ast::Entry::Message(message) = entry
                && let Some(value) = &message.value
            {
                strings[index].insert(message.id.name.to_owned(), pattern_text(value));
            }
        }
    }
    Ok(strings)
}

/// A pattern as its source writes it, but for the inside of a placeable that isn't a variable.
fn pattern_text(pattern: &ast::Pattern<&str>) -> String {
    let mut text = String::new();
    for element in &pattern.elements {
        match element {
            ast::PatternElement::TextElement { value } => text.push_str(value),
            ast::PatternElement::Placeable {
                expression: ast::Expression::Inline(ast::InlineExpression::VariableReference { id }),
            } => {
                let _ = write!(text, "{{ ${} }}", id.name);
            }
            ast::PatternElement::Placeable { .. } => text.push_str("{ … }"),
        }
    }
    text
}

/// Refuses a conversation's voiced line the script doesn't list, or lists with another speaker or text.
fn check_dialogues(content: &Path, script: &VoiceScript) -> Result<(), VoError> {
    for path in walk(content, ".dialogue.ron")? {
        let document = DialogueDocument::from_ron(&read_text(&path)?)
            .map_err(|error| problem(&path, format!("doesn't read: {error}")))?;
        for node in &document.nodes {
            let Some(say) = &node.say else { continue };
            let Some(key) = &say.vo else { continue };
            let Some(line) = script.lines.iter().find(|line| &line.key == key) else {
                return Err(problem(
                    &path,
                    format!("node `{}` voices `{key}`, which the script doesn't list", node.name),
                ));
            };
            if line.text != say.line || line.speaker.as_deref() != Some(say.speaker.as_str()) {
                return Err(problem(
                    &path,
                    format!(
                        "node `{}` has `{}` say `{}`, and the script's `{key}` has {:?} say `{}`",
                        node.name, say.speaker, say.line, line.speaker, line.text
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The files in `dir` (not below it) whose names end with `suffix`, sorted: none if `dir` is missing.
fn files(dir: &Path, suffix: &str) -> Result<Vec<PathBuf>, VoError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(VoError::Io { path: dir.to_owned(), source }),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry.map_err(|source| VoError::Io { path: dir.to_owned(), source })?.path();
        if path.is_file() && name(&path).is_some_and(|name| !name.starts_with('.') && name.ends_with(suffix))
        {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// The files under `dir`, at any depth, whose names end with `suffix`, sorted, skipping hidden entries as
/// the cook does.
fn walk(dir: &Path, suffix: &str) -> Result<Vec<PathBuf>, VoError> {
    let mut found = files(dir, suffix)?;
    let entries = fs::read_dir(dir).map_err(|source| VoError::Io { path: dir.to_owned(), source })?;
    let mut dirs = Vec::new();
    for entry in entries {
        let path = entry.map_err(|source| VoError::Io { path: dir.to_owned(), source })?.path();
        if path.is_dir() && name(&path).is_some_and(|name| !name.starts_with('.')) {
            dirs.push(path);
        }
    }
    dirs.sort();
    for dir in dirs {
        found.extend(walk(&dir, suffix)?);
    }
    Ok(found)
}

fn name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

/// A file's name without `suffix`.
fn stem(path: &Path, suffix: &str) -> String {
    let name = name(path).unwrap_or_default();
    name[..name.len() - suffix.len()].to_owned()
}

/// A row of a CSV file (RFC 4180): a cell holding a comma, a quote or a line break is quoted, its quotes
/// doubled.
fn csv_row(cells: &[&str]) -> String {
    let mut row = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            row.push(',');
        }
        if cell.contains([',', '"', '\n', '\r']) {
            let _ = write!(row, "\"{}\"", cell.replace('"', "\"\""));
        } else {
            row.push_str(cell);
        }
    }
    row.push('\n');
    row
}

fn seconds_cell(seconds: Option<f64>) -> String {
    seconds.map(|seconds| format!("{seconds:.3}")).unwrap_or_default()
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

fn mean_square(samples: &[f32]) -> f64 {
    samples.iter().map(|&sample| f64::from(sample) * f64::from(sample)).sum::<f64>() / samples.len() as f64
}

/// A level against full scale, in decibels to a tenth, by the deterministic logarithm, so a report reads the
/// same on every machine.
fn decibels(level: f32) -> String {
    if level <= 0.0 {
        return "-inf".to_owned();
    }
    format!("{:.1}", 20.0 * det::ln(level) / det::ln(10.0))
}

#[cfg(test)]
mod tests;
