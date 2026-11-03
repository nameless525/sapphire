//! `cargo xtask script-api`: makes the script API's Rust wrappers and its `.d.luau` from the one declared
//! table, `crates/sapphire_script/api.toml` (spec §5.24, M3 L2).
//!
//! Generated because a binding has rules a hand-written one can get wrong without anything failing (see
//! `sapphire_script::bindings`): each wrapper here goes through `with_frame`, reads only numbers and pushes
//! only numbers, on every path, since there's only the one path to write. `--check` and a test here hold
//! what's checked in to the table.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail, ensure};
use serde::Deserialize;

use crate::workspace;

const TABLE: &str = "crates/sapphire_script/api.toml";
const RUST: &str = "crates/sapphire_script/src/bindings/generated.rs";
const DEFINITIONS: &str = "crates/sapphire_script/sapphire.d.luau";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    function: Vec<Function>,
    hook: Vec<Hook>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Query,
    Command,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Function {
    kind: Kind,
    name: String,
    args: Vec<String>,
    returns: Vec<String>,
    doc: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hook {
    name: String,
    args: Vec<String>,
    doc: String,
}

/// A name that's a Luau identifier and a Rust one alike, by the rule the table is held to.
fn is_identifier(name: &str) -> bool {
    let mut letters = name.chars();
    letters.next().is_some_and(|first| first.is_ascii_lowercase())
        && letters.all(|letter| letter.is_ascii_lowercase() || letter.is_ascii_digit() || letter == '_')
}

fn parse(text: &str) -> anyhow::Result<Table> {
    let table: Table = toml::from_str(text).context("reading the script API's table")?;
    let mut seen = BTreeSet::new();
    for function in &table.function {
        ensure!(is_identifier(&function.name), "`{}` isn't a function's name", function.name);
        ensure!(seen.insert(&function.name), "`{}` is declared twice", function.name);
        ensure!(!function.doc.contains('\n'), "`{}`'s doc is more than a line", function.name);
        for name in function.args.iter().chain(&function.returns) {
            ensure!(is_identifier(name), "`{name}`, in `{}`, isn't a name", function.name);
        }
        ensure!(
            function.kind == Kind::Query || function.returns.is_empty(),
            "`{}` is a command, which returns nothing: what it did isn't known until the hook's over",
            function.name
        );
        ensure!(function.returns.len() <= 3, "`{}` returns more than a C function's sure of", function.name);
    }
    let mut hooks = BTreeSet::new();
    for hook in &table.hook {
        ensure!(
            hook.name.starts_with("on_") && is_identifier(&hook.name),
            "`{}` isn't a hook's name",
            hook.name
        );
        ensure!(hooks.insert(&hook.name), "`{}` is declared twice", hook.name);
        ensure!(hook.args.len() <= 3, "`{}` takes more than a hook is handed", hook.name);
        for arg in &hook.args {
            ensure!(is_identifier(arg), "`{arg}`, in `{}`, isn't a name", hook.name);
        }
    }
    Ok(table)
}

fn wrapper(out: &mut String, function: &Function) {
    let Function { name, args, returns, .. } = function;
    writeln!(out, "unsafe extern \"C-unwind\" fn {name}(state: *mut lua_State) -> c_int {{").unwrap();
    let call_args: String = args.iter().map(|arg| format!(", {arg}")).collect();
    let bind = if returns.is_empty() { "" } else { "let result = " };
    writeln!(out, "    {bind}with_frame(|frame| {{").unwrap();
    for (place, arg) in args.iter().enumerate() {
        writeln!(out, "        // SAFETY: Luau passes a live state, and reading a number can't raise.")
            .unwrap();
        writeln!(out, "        let {arg} = unsafe {{ number(state, {}, frame) }};", place + 1).unwrap();
    }
    writeln!(out, "        api::{name}(frame{call_args})").unwrap();
    writeln!(out, "    }});").unwrap();
    match returns.len() {
        0 => {}
        1 => {
            writeln!(out, "    // SAFETY: Luau passes a live state and a slot for each result.").unwrap();
            writeln!(out, "    unsafe {{ ffi::lua_pushnumber(state, result.unwrap_or_default()) }};")
                .unwrap();
        }
        _ => {
            let names: Vec<&str> = returns.iter().map(String::as_str).collect();
            writeln!(out, "    let ({}) = result.unwrap_or_default();", names.join(", ")).unwrap();
            writeln!(out, "    // SAFETY: Luau passes a live state and a slot for each result.").unwrap();
            writeln!(out, "    unsafe {{").unwrap();
            for name in names {
                writeln!(out, "        ffi::lua_pushnumber(state, {name});").unwrap();
            }
            writeln!(out, "    }}").unwrap();
        }
    }
    writeln!(out, "    {}", returns.len()).unwrap();
    writeln!(out, "}}\n").unwrap();
}

fn list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| format!("{item:?}")).collect();
    quoted.join(", ")
}

fn rust(table: &Table) -> String {
    let mut out = String::from(
        "//! Generated by `cargo xtask script-api` from `api.toml`. Don't edit: change the table.\n\n\
         use std::ffi::c_int;\n\n\
         use mlua::{ffi, lua_State};\n\n\
         use super::{Binding, Declared, number, with_frame};\n\
         use crate::api;\n\n",
    );
    for function in &table.function {
        wrapper(&mut out, function);
    }
    writeln!(out, "/// Every function a script can call in `sapphire`, as the table declares it.").unwrap();
    writeln!(out, "pub const API: &[Declared] = &[").unwrap();
    for function in &table.function {
        let Function { name, args, returns, .. } = function;
        writeln!(
            out,
            "    Declared {{ binding: Binding {{ name: {name:?}, function: {name} }}, args: &[{}], returns: &[{}] }},",
            list(args),
            list(returns)
        )
        .unwrap();
    }
    writeln!(out, "];\n").unwrap();
    writeln!(out, "/// Hooks a chunk may hold, by the name it returns them under, with what each is handed.")
        .unwrap();
    writeln!(out, "pub const HOOKS: &[(&str, &[&str])] = &[").unwrap();
    for hook in &table.hook {
        writeln!(out, "    ({:?}, &[{}]),", hook.name, list(&hook.args)).unwrap();
    }
    writeln!(out, "];\n").unwrap();
    writeln!(out, "/// How many hooks there are.").unwrap();
    writeln!(out, "pub const HOOK_COUNT: usize = {};\n", table.hook.len()).unwrap();
    for (place, hook) in table.hook.iter().enumerate() {
        writeln!(out, "pub const {}: usize = {place};", hook.name.to_ascii_uppercase()).unwrap();
    }
    out
}

fn signature(args: &[String]) -> String {
    let args: Vec<String> = args.iter().map(|arg| format!("{arg}: number")).collect();
    args.join(", ")
}

fn definitions(table: &Table) -> String {
    let mut out = String::from(
        "-- Generated by `cargo xtask script-api` from crates/sapphire_script/api.toml. Don't edit: change the table.\n\
         -- For an editor's autocompletion: nothing cooks this.\n\n\
         -- What a chunk returns. `names` is what a hook and a command mean by a name's number: a place in\n\
         -- this list, counting from 1.\n\
         export type Hooks = {\n    names: { string }?,\n",
    );
    for hook in &table.hook {
        writeln!(out, "    -- {}", hook.doc).unwrap();
        writeln!(out, "    {}: (({}) -> ())?,", hook.name, signature(&hook.args)).unwrap();
    }
    out.push_str("}\n\ndeclare sapphire: {\n");
    for function in &table.function {
        writeln!(out, "    -- {}", function.doc).unwrap();
        let returns = match function.returns.len() {
            0 => "()".to_owned(),
            1 => "number".to_owned(),
            count => format!("({})", vec!["number"; count].join(", ")),
        };
        writeln!(out, "    {}: ({}) -> {returns},", function.name, signature(&function.args)).unwrap();
    }
    out.push_str("}\n");
    out
}

/// What the table makes, by path from the workspace's root.
fn outputs(root: &Path) -> anyhow::Result<[(PathBuf, String); 2]> {
    let path = root.join(TABLE);
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let table = parse(&text)?;
    Ok([(root.join(RUST), rust(&table)), (root.join(DEFINITIONS), definitions(&table))])
}

/// Writes what the table makes, or with `check`, fails if what's checked in isn't that.
pub fn run(check: bool) -> anyhow::Result<()> {
    let root = workspace::root();
    for (path, text) in outputs(&root)? {
        if check {
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
            if on_disk != text {
                bail!("{} isn't what {TABLE} makes: run `cargo xtask script-api`", path.display());
            }
        } else {
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = r#"
        [[function]]
        kind = "query"
        name = "two"
        args = ["actor"]
        returns = ["x", "y"]
        doc = "Two numbers."

        [[function]]
        kind = "command"
        name = "ask"
        args = []
        returns = []
        doc = "Asks."

        [[hook]]
        name = "on_it"
        args = ["actor"]
        doc = "It."
    "#;

    /// The checked-in Rust and `.d.luau` are what the table makes: change one and not the table, or the
    /// table and not `cargo xtask script-api`, and this fails.
    #[test]
    fn whats_checked_in_is_what_the_table_makes() {
        for (path, text) in outputs(&workspace::root()).unwrap() {
            let on_disk = std::fs::read_to_string(&path).unwrap();
            assert!(
                on_disk == text,
                "{} isn't what the table makes: run `cargo xtask script-api`",
                path.display()
            );
        }
    }

    #[test]
    fn a_wrapper_goes_through_the_frame_and_pushes_what_it_declares() {
        let table = parse(ONE).unwrap();
        let rust = rust(&table);
        assert!(rust.contains("unsafe extern \"C-unwind\" fn two("));
        assert!(rust.contains("with_frame(|frame| {"));
        assert!(rust.contains("let actor = unsafe { number(state, 1, frame) };"));
        assert!(rust.contains("api::two(frame, actor)"));
        assert!(rust.contains("let (x, y) = result.unwrap_or_default();"));
        assert_eq!(rust.matches("ffi::lua_pushnumber").count(), 2, "{rust}");
        assert!(rust.contains("pub const ON_IT: usize = 0;"));
        let declared = definitions(&table);
        assert!(declared.contains("two: (actor: number) -> (number, number),"), "{declared}");
        assert!(declared.contains("ask: () -> (),"), "{declared}");
        assert!(declared.contains("on_it: ((actor: number) -> ())?,"), "{declared}");
    }

    #[test]
    fn a_bad_table_is_refused() {
        let twice = format!("{ONE}\n[[hook]]\nname = \"on_it\"\nargs = []\ndoc = \"Again.\"\n");
        assert!(parse(&twice).unwrap_err().to_string().contains("twice"));
        let returns = ONE.replace("kind = \"query\"", "kind = \"command\"");
        assert!(parse(&returns).unwrap_err().to_string().contains("command"));
        let name = ONE.replace("name = \"two\"", "name = \"Two\"");
        assert!(parse(&name).unwrap_err().to_string().contains("name"));
        let unknown = ONE.replace("doc = \"It.\"", "doc = \"It.\"\nplace = 1");
        assert!(parse(&unknown).is_err(), "the table denies a field it doesn't know");
    }
}
