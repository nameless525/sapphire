//! The editor's arguments, and its ways of running with no window: `--capture` draws a frame, and `--run` runs a
//! script of commands ([`crate::commands`]) and may draw one after it.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::app::{Editor, Options};
use crate::capture::{self, Textures};
use crate::commands::{SCRIPT_SUFFIX, Script};

/// What `sapphire_editor` does when run with no arguments, for the rest see the crate's docs.
pub const USAGE: &str = "\
usage: sapphire_editor [<document>] [--link <address:port>] [--content <cook folder>] [--record <script>]
       sapphire_editor <document> --capture <png> [--size <w>x<h>] [--move <name>] [--tick <n>]
                       [--content <cook folder>]
       sapphire_editor [<document>] --run <script> [--capture <png> [--size <w>x<h>]]
                       [--link <address:port>] [--content <cook folder>]

--capture draws one frame to a PNG with no window and no GPU. --run runs a script of commands
(a .commands.ron) with no window, then draws its last frame if --capture asks. --record keeps
every command a window's session runs in a script, which --run plays back.";

/// What a capture draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureArgs {
    pub png: PathBuf,
    pub width: u32,
    pub height: u32,
    pub mv: Option<String>,
    pub tick: u32,
}

/// A script to run, and what to draw after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunArgs {
    pub script: PathBuf,
    /// The frame to draw when it's done: its move and tick are the script's to choose.
    pub capture: Option<CaptureArgs>,
}

/// What was asked for.
#[derive(Clone, Debug)]
pub enum Launch {
    /// A window, keeping the commands its session runs in a script if `record` names one.
    Window {
        options: Options,
        record: Option<PathBuf>,
    },
    Capture(Options, CaptureArgs),
    Run(Options, RunArgs),
}

/// Reads a command line, program name first, as `std::env::args` and `Platform::args` give it.
pub fn parse_command_line(line: &[String]) -> Result<Launch, String> {
    parse(line.get(1..).unwrap_or_default())
}

/// Reads the arguments after the program's name.
pub fn parse(args: &[String]) -> Result<Launch, String> {
    let mut options = Options::default();
    let mut png = None;
    let mut size = (1400, 900);
    let mut mv = None;
    let mut tick = None;
    let mut script = None;
    let mut record = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value =
            |what: &str| args.next().cloned().ok_or_else(|| format!("{what} needs a value\n\n{USAGE}"));
        match arg.as_str() {
            "--link" => {
                let text = value("--link")?;
                let address: SocketAddr =
                    text.parse().map_err(|_| format!("`{text}` isn't an address and port"))?;
                options.link = Some(address);
            }
            "--content" => options.content = Some(PathBuf::from(value("--content")?)),
            "--capture" => png = Some(PathBuf::from(value("--capture")?)),
            "--move" => mv = Some(value("--move")?),
            "--tick" => {
                let text = value("--tick")?;
                tick = Some(text.parse().map_err(|_| format!("`{text}` isn't a tick"))?);
            }
            "--size" => {
                let text = value("--size")?;
                let parsed = text.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                size = parsed.ok_or_else(|| format!("`{text}` isn't a size like 1400x900"))?;
            }
            "--run" => script = Some(script_path(value("--run")?)?),
            "--record" => record = Some(script_path(value("--record")?)?),
            "--help" | "-h" => return Err(USAGE.to_owned()),
            flag if flag.starts_with('-') => return Err(format!("unknown argument `{flag}`\n\n{USAGE}")),
            path if options.file.is_none() => options.file = Some(PathBuf::from(path)),
            extra => return Err(format!("a second file `{extra}`\n\n{USAGE}")),
        }
    }
    let tick_given = tick.is_some();
    let capture =
        png.map(|png| CaptureArgs { png, width: size.0, height: size.1, mv, tick: tick.unwrap_or(0) });
    match (script, record) {
        (_, Some(_)) if capture.is_some() => Err(format!("--record keeps a window's session\n\n{USAGE}")),
        (Some(_), Some(_)) => Err(format!("--record keeps a window's session, not a script's\n\n{USAGE}")),
        (Some(script), None) => {
            if capture.as_ref().is_some_and(|capture| capture.mv.is_some()) || tick_given {
                return Err(format!("with --run, the script says which move and tick\n\n{USAGE}"));
            }
            Ok(Launch::Run(options, RunArgs { script, capture }))
        }
        (None, record) => Ok(match capture {
            Some(capture) => Launch::Capture(options, capture),
            None => Launch::Window { options, record },
        }),
    }
}

/// A script's path, which says what it is by its suffix.
fn script_path(text: String) -> Result<PathBuf, String> {
    if text.ends_with(SCRIPT_SUFFIX) {
        Ok(PathBuf::from(text))
    } else {
        Err(format!("`{text}` isn't a script: its name ends with {SCRIPT_SUFFIX}"))
    }
}

/// Runs a script of commands on an editor started as `options`, with no window, then draws what it left if it's
/// asked to. A refused command stops it, naming the command and why.
pub fn run_script(options: Options, args: &RunArgs) -> Result<(), String> {
    if let Some(capture) = &args.capture {
        capture::validate_size(capture.width, capture.height)?;
    }
    let script = Script::read(&args.script)?;
    let mut editor = Editor::new(options);
    if let Some(notice) = editor.notice().filter(|notice| notice.trouble) {
        return Err(notice.text.clone());
    }
    editor.run_script(&script).map_err(|error| format!("{}: {error}", args.script.display()))?;
    match &args.capture {
        Some(capture) => write_frame(&mut editor, capture.width, capture.height, &capture.png),
        None => Ok(()),
    }
}

/// Draws one frame of the editor on `options` to a PNG: no window, no SDL, no GPU.
pub fn run_capture(options: Options, args: &CaptureArgs) -> Result<(), String> {
    if options.file.is_none() {
        return Err(format!("--capture needs a file\n\n{USAGE}"));
    }
    capture::validate_size(args.width, args.height)?;
    let mut editor = Editor::new(options);
    if let Some(notice) = editor.notice() {
        return Err(notice.text.clone());
    }
    if let Some(name) = &args.mv
        && !editor.select_move(name)
    {
        return Err(format!("no move `{name}` in this file"));
    }
    editor.set_tick(args.tick);
    write_frame(&mut editor, args.width, args.height, &args.png)
}

/// Draws the editor as it stands, to a PNG.
pub fn write_frame(editor: &mut Editor, width: u32, height: u32, png: &Path) -> Result<(), String> {
    capture::validate_size(width, height)?;
    if let Ok(output) = png.canonicalize() {
        for id in editor.documents().ids() {
            if editor.documents().path(id).and_then(|path| path.canonicalize().ok()).as_ref() == Some(&output)
            {
                return Err(format!(
                    "{} is an open source document, not a capture destination",
                    png.display()
                ));
            }
        }
    }
    let ctx = egui::Context::default();
    let mut textures = Textures::default();
    // egui's panels and grids measure on their first passes.
    let mut last = None;
    for pass in 0..4 {
        let input = capture::raw_input(width, height, f64::from(pass) * 0.05);
        let output = ctx.run_ui(input, |ui| editor.ui(ui));
        if pass == 3 {
            last = Some(capture::rasterize(&ctx, &mut textures, output, width, height));
        } else {
            capture::settle(&ctx, &mut textures, output);
        }
    }
    let image = last.ok_or_else(|| "no frame".to_owned())?;
    image.write_png(png).map_err(|error| format!("{}: {error}", png.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(ToOwned::to_owned).collect()
    }

    #[test]
    fn a_capture_cannot_replace_an_open_source_document() {
        let dir = std::env::temp_dir().join(format!("sapphire_capture_source_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("test directory");
        let path = dir.join("source.moves.ron");
        let text = crate::edits::test_files::WARRIOR;
        std::fs::write(&path, text).expect("source");
        let mut editor = Editor::new(Options {
            file: Some(path.clone()),
            content: Some(dir.join("no cook")),
            link: Some("127.0.0.1:9".parse().expect("an address")),
        });
        let error = write_frame(&mut editor, 32, 32, &path).expect_err("source alias refused");
        assert!(error.contains("open source document"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).expect("source remains"), text);
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_window_and_a_capture_read_their_arguments() {
        let Ok(Launch::Window { options, record: None }) =
            parse(&args("a.moves.ron --link 127.0.0.1:7900 --content cook"))
        else {
            panic!("a window")
        };
        assert_eq!(options.file, Some(PathBuf::from("a.moves.ron")));
        assert_eq!(options.link, Some("127.0.0.1:7900".parse().expect("an address")));
        assert_eq!(options.content, Some(PathBuf::from("cook")));

        let Ok(Launch::Capture(_, capture)) =
            parse(&args("a.moves.ron --capture out.png --size 800x600 --move light_1 --tick 7"))
        else {
            panic!("a capture")
        };
        assert_eq!(
            capture,
            CaptureArgs {
                png: "out.png".into(),
                width: 800,
                height: 600,
                mv: Some("light_1".to_owned()),
                tick: 7
            }
        );
    }

    #[test]
    fn a_program_name_is_not_a_file() {
        let Ok(Launch::Window { options, .. }) = parse_command_line(&args("sapphire_editor")) else {
            panic!("a window")
        };
        assert_eq!(options.file, None);
        let Ok(Launch::Window { options, .. }) = parse_command_line(&args("sapphire_editor a.moves.ron"))
        else {
            panic!("a window")
        };
        assert_eq!(options.file, Some(PathBuf::from("a.moves.ron")));
        assert!(parse_command_line(&[]).is_ok());
    }

    #[test]
    fn a_misspelled_argument_fails_rather_than_doing_something_else() {
        for bad in ["--capturee x.png", "a b", "--link nowhere", "--tick many", "--size big", "--move"] {
            assert!(parse(&args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_script_runs_and_a_session_records_by_their_own_arguments() {
        let Ok(Launch::Run(_, run)) = parse(&args("--run a.commands.ron --capture out.png --size 800x600"))
        else {
            panic!("a run")
        };
        assert_eq!(run.script, PathBuf::from("a.commands.ron"));
        assert_eq!(run.capture.map(|capture| (capture.width, capture.height)), Some((800, 600)));
        let Ok(Launch::Window { record, .. }) = parse(&args("a.moves.ron --record s.commands.ron")) else {
            panic!("a window that records")
        };
        assert_eq!(record, Some(PathBuf::from("s.commands.ron")));
        for bad in [
            "--run a.ron",
            "--record a.ron",
            "--run a.commands.ron --record b.commands.ron",
            "a.moves.ron --capture x.png --record b.commands.ron",
            "--run a.commands.ron --tick 3",
            "--run a.commands.ron --capture x.png --move light_1",
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad}");
        }
    }
}
