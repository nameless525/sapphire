//! Command-line options.

use sapphire_render::Msaa;

/// Options parsed from the process arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    /// Exit after this many frames.
    pub frame_limit: Option<u64>,
    pub msaa: Msaa,
}

impl Default for Options {
    fn default() -> Self {
        Self { frame_limit: None, msaa: Msaa::X4 }
    }
}

impl Options {
    /// Parses `args`, which includes the program name first. Unknown arguments are returned
    /// so the caller can warn about them; SDL and the OS may pass their own.
    pub fn parse(args: &[String]) -> Result<(Self, Vec<String>), String> {
        let mut options = Self::default();
        let mut unknown = Vec::new();
        let mut args = args.iter().skip(1);
        while let Some(arg) = args.next() {
            let mut value = |name: &str| args.next().cloned().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--frames" => {
                    let raw = value("--frames")?;
                    let frames =
                        raw.parse().map_err(|_| format!("--frames expects a number, got {raw:?}"))?;
                    options.frame_limit = Some(frames);
                }
                "--msaa" => {
                    options.msaa = match value("--msaa")?.as_str() {
                        "off" | "1" => Msaa::Off,
                        "4" => Msaa::X4,
                        other => return Err(format!("--msaa expects `off` or `4`, got {other:?}")),
                    };
                }
                _ => unknown.push(arg.clone()),
            }
        }
        Ok((options, unknown))
    }
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
        assert_eq!(options, Options { frame_limit: Some(120), msaa: Msaa::Off });
        assert!(unknown.is_empty());
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
        assert!(parse(&["--msaa", "8"]).is_err());
    }
}
