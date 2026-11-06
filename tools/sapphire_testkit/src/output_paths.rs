//! Require this tool's output tree to be disjoint from its inputs.

use std::path::Path;

use sapphire_core::path::resolve_existing_prefix;

/// Require output and inputs to have disjoint trees. No filesystem writes happen here.
pub(crate) fn disjoint(out: &Path, inputs: &[&Path]) -> Result<(), String> {
    let output =
        resolve_existing_prefix(out).map_err(|error| format!("resolving {}: {error}", out.display()))?;
    for input in inputs {
        let source = resolve_existing_prefix(input)
            .map_err(|error| format!("resolving {}: {error}", input.display()))?;
        if output.starts_with(&source) || source.starts_with(&output) {
            return Err(format!(
                "output {} overlaps input {}; choose disjoint directories",
                out.display(),
                input.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_equal_ancestor_descendant_and_lexical_aliases_without_writing() {
        let root = std::env::current_dir().unwrap();
        let input = root.join("source");
        for output in [input.clone(), root.clone(), input.join("nested"), root.join("new/../source")] {
            assert!(disjoint(&output, &[&input]).is_err(), "{}", output.display());
        }
        assert!(disjoint(&root.join("output/new"), &[&input]).is_ok());
    }
}
