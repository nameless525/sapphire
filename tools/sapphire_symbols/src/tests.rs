//! Each test links a small program the way `cargo xtask package` links the game on the host it runs on:
//! Mach-O with a dSYM on macOS, ELF with a build ID and a stripped copy on Linux, PE with a PDB on Windows.
//! CI's test job runs them on all three, so each format is read from a real linker's output.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;

/// What a link left: the binary a package ships, and its symbols.
struct Linked {
    shipped: PathBuf,
    symbols: PathBuf,
}

/// A directory of its own for a test, emptied first.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sapphire_symbols-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(command: &mut Command) {
    let output = command.output().unwrap_or_else(|e| panic!("{command:?} didn't start: {e}"));
    assert!(output.status.success(), "{command:?} failed: {}", String::from_utf8_lossy(&output.stderr));
}

/// Links `source` as `name` in `dir`, as a shipping package does on this host, stripped unless `strip`
/// is false.
fn link(dir: &Path, name: &str, source: &str, strip: bool) -> Linked {
    let source_path = dir.join(format!("{name}.rs"));
    fs::create_dir_all(dir).unwrap();
    fs::write(&source_path, source).unwrap();
    let mut rustc = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
    rustc.arg(&source_path).args(["--edition", "2024", "-C", "debuginfo=2", "--crate-name", name]);
    // The workspace's cargo `[env]` sets it for every process cargo runs, tests too.
    rustc.env_remove("IPHONEOS_DEPLOYMENT_TARGET");
    match std::env::consts::OS {
        "macos" => {
            rustc.args(["-C", "split-debuginfo=packed"]);
            if strip {
                rustc.args(["-C", "strip=symbols"]);
            }
            run(rustc.arg("-o").arg(dir.join(name)));
            Linked { shipped: dir.join(name), symbols: dir.join(format!("{name}.dSYM")) }
        }
        "windows" => {
            run(rustc.arg("--out-dir").arg(dir));
            Linked { shipped: dir.join(format!("{name}.exe")), symbols: dir.join(format!("{name}.pdb")) }
        }
        _ => {
            let symbols = dir.join("symbols").join(name);
            fs::create_dir_all(symbols.parent().unwrap()).unwrap();
            rustc.args(["-C", "split-debuginfo=off", "-C", "link-arg=-Wl,--build-id=sha1"]);
            run(rustc.arg("-o").arg(&symbols));
            let shipped = dir.join(name);
            fs::copy(&symbols, &shipped).unwrap();
            if strip {
                run(Command::new("strip").arg("--strip-all").arg(&shipped));
            }
            Linked { shipped, symbols }
        }
    }
}

const HELLO: &str = "fn main() { println!(\"{}\", std::env::args().count()); }\n";
const OTHER: &str = "fn main() { println!(\"{:?}\", std::env::args().collect::<Vec<_>>()); }\n";

#[test]
fn ids_print_and_file_as_breakpad_does() {
    let raw: [u8; 16] = std::array::from_fn(|index| index as u8);
    assert_eq!(DebugId::from_le_guid(raw, 0).to_string(), "030201000504070608090A0B0C0D0E0F0");
    assert_eq!(DebugId::from_le_guid(raw, 0x1a).to_string(), "030201000504070608090A0B0C0D0E0F1A");
    assert_eq!(DebugId { guid: raw, age: 0 }.to_string(), "000102030405060708090A0B0C0D0E0F0");

    let mut module = Module {
        format: Format::Pe,
        arch: "x86_64",
        debug_file: "game_app.pdb".into(),
        debug_id: DebugId { guid: raw, age: 2 },
        code_id: String::new(),
    };
    assert_eq!(module.store_path(), Path::new("game_app.pdb/000102030405060708090A0B0C0D0E0F2/game_app.sym"));
    module.debug_file = "libgame_app.so".into();
    assert_eq!(
        module.store_path(),
        Path::new("libgame_app.so/000102030405060708090A0B0C0D0E0F2/libgame_app.so.sym")
    );
}

#[test]
fn a_shipping_link_checks_out() {
    let dir = scratch("link");
    let linked = link(&dir, "hello", HELLO, true);
    let module = check(&linked.shipped, &linked.symbols).unwrap();
    let format = match std::env::consts::OS {
        "macos" => Format::MachO,
        "windows" => Format::Pe,
        _ => Format::Elf,
    };
    assert_eq!(module.format, format);
    let expected_file = if format == Format::Pe { "hello.pdb" } else { "hello" };
    assert_eq!(module.debug_file, expected_file);
    assert_eq!(module.arch, if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" });
    assert_ne!(module.debug_id.guid, [0; 16]);
    match format {
        // Mach-O's and ELF's age is always 0; the ELF code ID is the whole 20-byte SHA-1 build ID.
        Format::MachO => assert_eq!(module.code_id, module.debug_id.to_string()[..32]),
        Format::Elf => {
            assert_eq!(module.debug_id.age, 0);
            assert_eq!(module.code_id.len(), 40);
        }
        Format::Pe => assert!(module.debug_id.age >= 1, "{}", module.debug_id),
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn symbols_from_another_link_are_refused() {
    let dir = scratch("another");
    let hello = link(&dir.join("hello"), "hello", HELLO, true);
    let other = link(&dir.join("other"), "hello", OTHER, true);
    let error = check(&hello.shipped, &other.symbols).unwrap_err();
    assert!(format!("{error:#}").contains("the symbols come from another link"), "{error:#}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_unstripped_binary_is_refused() {
    if std::env::consts::OS == "windows" {
        // MSVC's linker writes debug information only to the PDB: there's nothing to strip.
        return;
    }
    let dir = scratch("unstripped");
    let linked = link(&dir, "hello", HELLO, false);
    let error = check(&linked.shipped, &linked.symbols).unwrap_err();
    assert!(format!("{error:#}").contains("isn't stripped"), "{error:#}");
    fs::remove_dir_all(dir).unwrap();
}

/// A package's whole store, as the nightly packages keep it: every module its `package.json` lists gets
/// its `.sym` where Breakpad looks, and filling it again writes nothing new.
#[test]
fn a_package_fills_its_store() {
    let dir = scratch("store");
    let linked = link(&dir, "hello", HELLO, true);
    let module = check(&linked.shipped, &linked.symbols).unwrap();
    let mut listed = module.to_json();
    let symbols = linked.symbols.strip_prefix(&dir).unwrap();
    let symbols: Vec<&str> = symbols.iter().map(|part| part.to_str().unwrap()).collect();
    listed["symbols"] = symbols.join("/").into();
    fs::write(dir.join("package.json"), serde_json::json!({ "modules": [listed] }).to_string()).unwrap();

    let packaged = store::packaged(std::slice::from_ref(&dir)).unwrap();
    let store = store::Store { root: dir.join("store") };
    let written = store.fill(&packaged).unwrap();
    assert_eq!(written, [store.root.join(module.store_path())]);
    let sym = fs::read_to_string(&written[0]).unwrap();
    let first = sym.lines().next().unwrap();
    assert!(
        first.starts_with("MODULE ")
            && first.ends_with(&format!("{} {}", module.debug_id, module.debug_file))
    );
    assert!(sym.lines().any(|line| line.starts_with("FUNC ")), "no functions in {}", written[0].display());
    let modified = fs::metadata(&written[0]).unwrap().modified().unwrap();
    assert_eq!(store.fill(&packaged).unwrap(), written);
    assert_eq!(fs::metadata(&written[0]).unwrap().modified().unwrap(), modified);
    fs::remove_dir_all(dir).unwrap();
}
