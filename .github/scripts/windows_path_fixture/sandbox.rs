//! Deterministic capture producer for path/IO checks only. No renderer or window.
use std::{env, fs, path::PathBuf};

fn main() {
    let args: Vec<_> = env::args_os().collect();
    let at = args.iter().position(|arg| arg == "--capture").expect("--capture");
    let capture = PathBuf::from(args.get(at + 1).expect("capture path"));
    let content = PathBuf::from(env::var_os("SAPPHIRE_CONTENT_DIR").expect("content"));
    assert_eq!(fs::read(content.join("canary.txt")).unwrap(), b"protected content\n");
    fs::copy(content.join("capture.png"), &capture).expect("write capture");
    fs::write(capture.with_extension("adapter.txt"), "backend=fixture\nadapter=path-only\n")
        .expect("write fixture identity");
}
