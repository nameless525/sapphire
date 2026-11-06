//! Filesystem path identity for callers that may create an output later.
//!
//! Owns existing-prefix resolution. Callers own containment/overlap policy and publication.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Resolves existing prefixes after [`std::path::absolute`]'s platform normalization,
/// retaining a missing suffix. Ordinary Windows paths normalize parent components before
/// filesystem queries; drive-relative and root-relative forms follow that API's directory rules.
/// On Unix, parent components survive that step and are applied after resolving preceding symlinks.
/// Any parent components remaining after the initial normalization are handled in prefix order.
/// Dangling symlinks and other resolution errors are returned, not treated as new paths.
/// This reads path identity at one instant; it is not an atomic filesystem sandbox and does not
/// detect hard-link identity or prevent another process replacing a path afterward.
pub fn resolve_existing_prefix(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                match fs::canonicalize(&resolved) {
                    Ok(path) => resolved = path,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        // A dangling symlink is not a missing directory we can safely create.
                        if fs::symlink_metadata(&resolved).is_ok() {
                            return Err(error);
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_existing_roots_and_missing_suffixes() {
        let root = std::env::temp_dir().join(format!("sapphire_core_paths_suffix_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let canonical = fs::canonicalize(&root).unwrap();
        assert_eq!(resolve_existing_prefix(&root).unwrap(), canonical);
        assert_eq!(resolve_existing_prefix(&root.join("new/child")).unwrap(), canonical.join("new/child"));
        assert_eq!(resolve_existing_prefix(&root.join("new/../sibling")).unwrap(), canonical.join("sibling"));
        #[cfg(unix)]
        {
            // Intermediate-component error classification is platform-specific.
            fs::write(root.join("file"), "held").unwrap();
            assert!(resolve_existing_prefix(&root.join("file/child")).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_prefixes_resolve_before_parent_components_and_dangling_links_fail() {
        let root = std::env::temp_dir().join(format!("sapphire_core_paths_symlink_{}", std::process::id()));
        fs::create_dir_all(root.join("source/child")).unwrap();
        let source = fs::canonicalize(root.join("source")).unwrap();
        std::os::unix::fs::symlink(root.join("source/child"), root.join("alias")).unwrap();
        assert_eq!(resolve_existing_prefix(&root.join("alias")).unwrap(), source.join("child"));
        assert_eq!(resolve_existing_prefix(&root.join("alias/../new")).unwrap(), source.join("new"));
        fs::write(root.join("source/reference.txt"), "held").unwrap();
        std::os::unix::fs::symlink(root.join("source/reference.txt"), root.join("current.txt")).unwrap();
        assert_eq!(resolve_existing_prefix(&root.join("current.txt")).unwrap(), source.join("reference.txt"));
        std::os::unix::fs::symlink(root.join("absent"), root.join("dangling")).unwrap();
        assert_eq!(
            resolve_existing_prefix(&root.join("dangling")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs::remove_dir_all(root).unwrap();
    }
}
