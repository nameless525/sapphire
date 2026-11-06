//! The editor's arguments, and its one way of running with no window: `--capture`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::app::{Editor, Options};
use crate::capture::{self, Textures};

/// What `sapphire_editor` does when run with no arguments, for the rest see the crate's docs.
pub const USAGE: &str = "\
usage: sapphire_editor [<file.moves.ron>] [--link <address:port>] [--content <cook folder>]
       sapphire_editor <file.moves.ron> --capture <png> [--size <w>x<h>] [--move <name>] [--tick <n>]
                       [--content <cook folder>]

--capture draws one frame to a PNG with no window and no GPU.";

/// What a capture draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureArgs {
    pub png: PathBuf,
    pub width: u32,
    pub height: u32,
    pub mv: Option<String>,
    pub tick: u32,
}

/// What was asked for.
#[derive(Clone, Debug)]
pub enum Command {
    Window(Options),
    Capture(Options, CaptureArgs),
}

/// Reads a command line, program name first, as `std::env::args` and `Platform::args` give it.
pub fn parse_command_line(line: &[String]) -> Result<Command, String> {
    parse(line.get(1..).unwrap_or_default())
}

/// Reads the arguments after the program's name.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut options = Options::default();
    let mut png = None;
    let mut size = (1400, 900);
    let mut mv = None;
    let mut tick = 0;
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
                tick = text.parse().map_err(|_| format!("`{text}` isn't a tick"))?;
            }
            "--size" => {
                let text = value("--size")?;
                let parsed = text.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                size = parsed.ok_or_else(|| format!("`{text}` isn't a size like 1400x900"))?;
            }
            "--help" | "-h" => return Err(USAGE.to_owned()),
            flag if flag.starts_with('-') => return Err(format!("unknown argument `{flag}`\n\n{USAGE}")),
            path if options.file.is_none() => options.file = Some(PathBuf::from(path)),
            extra => return Err(format!("a second file `{extra}`\n\n{USAGE}")),
        }
    }
    Ok(match png {
        Some(png) => Command::Capture(options, CaptureArgs { png, width: size.0, height: size.1, mv, tick }),
        None => Command::Window(options),
    })
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
        let Ok(Command::Window(options)) = parse(&args("a.moves.ron --link 127.0.0.1:7900 --content cook"))
        else {
            panic!("a window")
        };
        assert_eq!(options.file, Some(PathBuf::from("a.moves.ron")));
        assert_eq!(options.link, Some("127.0.0.1:7900".parse().expect("an address")));
        assert_eq!(options.content, Some(PathBuf::from("cook")));

        let Ok(Command::Capture(_, capture)) =
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
        let Ok(Command::Window(options)) = parse_command_line(&args("sapphire_editor")) else {
            panic!("a window")
        };
        assert_eq!(options.file, None);
        let Ok(Command::Window(options)) = parse_command_line(&args("sapphire_editor a.moves.ron")) else {
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
}
