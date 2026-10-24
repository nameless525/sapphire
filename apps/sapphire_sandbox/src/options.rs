//! Command-line options.

use std::path::PathBuf;

use sapphire_render::Msaa;

/// The flag that selects the offscreen capture mode; see [`CaptureRequest`].
pub(crate) const CAPTURE_FLAG: &str = "--capture";

/// Options parsed from the process arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    /// Exit after this many presented frames.
    pub frame_limit: Option<u64>,
    pub msaa: Msaa,
    /// Render offscreen and write a PNG instead of opening a window.
    pub capture: Option<CaptureRequest>,
}

/// `--capture <png> [--capture-ticks N] [--capture-size WxH]`: simulate `ticks` fixed steps,
/// render one frame offscreen and write it to `path`, without creating a window or taking
/// focus. Desktop only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaptureRequest {
    pub path: PathBuf,
    pub ticks: u32,
    pub width: u32,
    pub height: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { frame_limit: None, msaa: Msaa::X4, capture: None }
    }
}

impl Options {
    /// Parses `args`, which includes the program name first. Unknown arguments are returned
    /// so the caller can warn about them; SDL and the OS may pass their own.
    pub fn parse(args: &[String]) -> Result<(Self, Vec<String>), String> {
        let mut options = Self::default();
        let mut unknown = Vec::new();
        let (mut capture_ticks, mut capture_size) = (None, None);
        let mut args = args.iter().skip(1);
        while let Some(arg) = args.next() {
            let mut value = |name: &str| args.next().cloned().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--frames" => options.frame_limit = Some(number(&value("--frames")?, "--frames")?),
                "--msaa" => {
                    options.msaa = match value("--msaa")?.as_str() {
                        "off" | "1" => Msaa::Off,
                        "2" => Msaa::X2,
                        "4" => Msaa::X4,
                        "8" => Msaa::X8,
                        other => return Err(format!("--msaa expects off, 2, 4 or 8, got {other:?}")),
                    };
                }
                CAPTURE_FLAG => {
                    let path = value(CAPTURE_FLAG)?.into();
                    options.capture = Some(CaptureRequest { path, ticks: 90, width: 1920, height: 1080 });
                }
                "--capture-ticks" => {
                    capture_ticks = Some(number(&value("--capture-ticks")?, "--capture-ticks")?)
                }
                "--capture-size" => capture_size = Some(size(&value("--capture-size")?)?),
                _ => unknown.push(arg.clone()),
            }
        }
        match &mut options.capture {
            Some(capture) => {
                capture.ticks = capture_ticks.unwrap_or(capture.ticks);
                (capture.width, capture.height) = capture_size.unwrap_or((capture.width, capture.height));
            }
            None if capture_ticks.is_some() || capture_size.is_some() => {
                return Err(format!("--capture-ticks and --capture-size need {CAPTURE_FLAG}"));
            }
            None => {}
        }
        Ok((options, unknown))
    }
}

fn number<T: std::str::FromStr>(raw: &str, name: &str) -> Result<T, String> {
    raw.parse().map_err(|_| format!("{name} expects a number, got {raw:?}"))
}

fn size(raw: &str) -> Result<(u32, u32), String> {
    let invalid = || format!("--capture-size expects WIDTHxHEIGHT, got {raw:?}");
    let (width, height) = raw.split_once('x').ok_or_else(invalid)?;
    let (width, height) = (width.parse().map_err(|_| invalid())?, height.parse().map_err(|_| invalid())?);
    if width == 0 || height == 0 {
        return Err(invalid());
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<(Options, Vec<String>), String> {
        let args: Vec<String> =
            std::iter::once("sandbox").chain(args.iter().copied()).map(String::from).collect();
        Options::parse(&args)
    }

    #[test]
    fn parses_known_options() {
        let (options, unknown) = parse(&["--frames", "120", "--msaa", "off"]).unwrap();
        assert_eq!(options, Options { frame_limit: Some(120), msaa: Msaa::Off, capture: None });
        assert!(unknown.is_empty());
    }

    #[test]
    fn parses_capture_requests_in_any_order() {
        let (options, _) =
            parse(&["--capture-size", "640x360", "--capture", "out.png", "--capture-ticks", "5"]).unwrap();
        assert_eq!(
            options.capture,
            Some(CaptureRequest { path: "out.png".into(), ticks: 5, width: 640, height: 360 })
        );
        let (options, _) = parse(&["--capture", "a.png"]).unwrap();
        assert_eq!(options.capture.map(|c| (c.ticks, c.width, c.height)), Some((90, 1920, 1080)));
    }

    #[test]
    fn keeps_unknown_arguments() {
        let (options, unknown) = parse(&["-NSDocumentRevisionsDebugMode", "YES"]).unwrap();
        assert_eq!(options, Options::default());
        assert_eq!(unknown, ["-NSDocumentRevisionsDebugMode", "YES"]);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(parse(&["--frames"]).is_err());
        assert!(parse(&["--frames", "many"]).is_err());
        assert!(parse(&["--msaa", "3"]).is_err());
        assert!(parse(&["--capture-ticks", "4"]).is_err(), "needs --capture");
        assert!(parse(&["--capture", "a.png", "--capture-size", "0x10"]).is_err());
        assert!(parse(&["--capture", "a.png", "--capture-size", "wide"]).is_err());
    }
}
