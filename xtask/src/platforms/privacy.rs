//! The privacy policy's address (M4 Z3b), which the game shows on its privacy page and every store's
//! listing gives. Apple's App Review Guideline 5.1.1(i) and Play's User Data policy want the policy reachable
//! inside the app as well as on the listing (docs/store/apple.md, rule 7; google-play.md, rule 8).
//!
//! The address is a string, `privacy-url`, in each language's [`STRINGS`], so the page needs no code and a
//! patch can move it. Until the maintainer hosts the policy (docs/store/privacy-policy.md) it lies under a
//! domain RFC 2606 reserves, as the placeholder bundle ID does, so nobody can take it for a chosen one. A
//! placeholder never ships silently: [`PrivacyPolicy::load`] refuses one once the game's identity is chosen,
//! and while the identity is a placeholder too, `package` warns of both and `package.json` marks it.

use std::fs;

use anyhow::{Context as _, bail, ensure};

use crate::workspace;

/// Each language's strings that hold [`KEY`], as the game's screens read them.
pub const STRINGS: [(&str, &str); 2] =
    [("en", "content/loc/en/game_ui.ftl"), ("ja", "content/loc/ja/game_ui.ftl")];

/// The string the privacy page shows the address by.
const KEY: &str = "privacy-url";

/// The domains RFC 2606 reserves: a placeholder address lies under one, and a chosen one never does.
const PLACEHOLDER_DOMAINS: [&str; 3] = ["example.com", "example.net", "example.org"];

/// The policy's address in each language, and whether any of them is still a placeholder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivacyPolicy {
    pub urls: Vec<(&'static str, String)>,
    pub placeholder: bool,
}

impl PrivacyPolicy {
    /// Reads the address from each of [`STRINGS`], for a game whose identity is `identity_placeholder`.
    pub fn load(identity_placeholder: bool) -> anyhow::Result<Self> {
        let root = workspace::root();
        let mut texts = Vec::with_capacity(STRINGS.len());
        for (language, file) in STRINGS {
            let path = root.join(file);
            let text =
                fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
            texts.push((language, text));
        }
        let strings: Vec<(&'static str, &str)> =
            texts.iter().map(|(language, text)| (*language, text.as_str())).collect();
        Self::from_strings(&strings, identity_placeholder)
    }

    /// The address in each language's Fluent `strings`. Each must give it once, on one line, as an
    /// `https://` URL. A placeholder fails unless the identity is one too.
    fn from_strings(strings: &[(&'static str, &str)], identity_placeholder: bool) -> anyhow::Result<Self> {
        let mut urls = Vec::with_capacity(strings.len());
        for &(language, text) in strings {
            let found: Vec<&str> = text
                .lines()
                .filter_map(|line| line.strip_prefix(KEY)?.trim_start().strip_prefix('='))
                .map(str::trim)
                .collect();
            let [url] = found[..] else {
                bail!(
                    "{language}'s strings give `{KEY}` {} times, where the privacy page wants it once",
                    found.len()
                )
            };
            let host = url.strip_prefix("https://").and_then(|rest| rest.split(['/', '?', '#']).next());
            let Some(host) = host.filter(|host| !host.is_empty() && !host.contains(char::is_whitespace))
            else {
                bail!("{language}'s `{KEY}` is {url:?}: the privacy policy's address is an https:// URL")
            };
            urls.push((language, url.to_owned(), is_placeholder_host(host)));
        }
        let placeholder = urls.iter().any(|(_, _, placeholder)| *placeholder);
        ensure!(
            !placeholder || identity_placeholder,
            "the game's identity is chosen, but the privacy policy's address is still a placeholder under {}: \
             give where the policy is hosted as `{KEY}` in {}",
            PLACEHOLDER_DOMAINS.join(", "),
            STRINGS.map(|(_, file)| file).join(" and ")
        );
        Ok(Self { urls: urls.into_iter().map(|(language, url, _)| (language, url)).collect(), placeholder })
    }
}

/// Whether `host` is, or lies under, one of [`PLACEHOLDER_DOMAINS`].
fn is_placeholder_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    PLACEHOLDER_DOMAINS
        .iter()
        .any(|domain| host.strip_suffix(domain).is_some_and(|rest| rest.is_empty() || rest.ends_with('.')))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: &str = "privacy-title = Privacy policy\nprivacy-url = https://example.com/game/privacy\n";
    const JA: &str = "privacy-url = https://example.com/game/privacy/ja\n";

    /// The tree's address is a placeholder only while the game's identity is one, which is what `package`
    /// reads: so the two are chosen together, or a package says it's no store's.
    #[test]
    fn the_trees_address_is_a_placeholder_only_while_the_identity_is() {
        let identities = fs::read_to_string(workspace::root().join(super::super::IDENTITY_FILE)).unwrap();
        let identities: toml::Table = toml::from_str(&identities).unwrap();
        let placeholder =
            identities["game_app"].get("placeholder").and_then(toml::Value::as_bool) == Some(true);
        let policy = PrivacyPolicy::load(placeholder).unwrap();
        assert_eq!(policy.urls.len(), STRINGS.len());
        assert!(!policy.placeholder || placeholder);
    }

    /// A placeholder can't ship under a chosen identity, and a chosen address under a placeholder identity
    /// is fine: the policy may be hosted before the game is named.
    #[test]
    fn a_placeholder_address_never_passes_for_a_chosen_one() {
        let strings = [("en", EN), ("ja", JA)];
        let err = PrivacyPolicy::from_strings(&strings, false).unwrap_err();
        assert!(err.to_string().contains("still a placeholder"), "{err}");
        let policy = PrivacyPolicy::from_strings(&strings, true).unwrap();
        assert!(policy.placeholder);
        assert_eq!(policy.urls[1], ("ja", "https://example.com/game/privacy/ja".to_owned()));

        let chosen = [
            ("en", "privacy-url = https://games.example.jp/privacy\n"),
            ("ja", "privacy-url =https://notexample.com/p\n"),
        ];
        let policy = PrivacyPolicy::from_strings(&chosen, false).unwrap();
        assert!(!policy.placeholder);
        // One language's placeholder is enough to hold the rest back.
        let mixed = [("en", "privacy-url = https://games.example.jp/privacy\n"), ("ja", JA)];
        assert!(PrivacyPolicy::from_strings(&mixed, false).is_err());
        assert!(is_placeholder_host("docs.EXAMPLE.org"));
        assert!(!is_placeholder_host("example.company"));
    }

    #[test]
    fn each_language_gives_one_https_address() {
        let refused =
            |en: &str| PrivacyPolicy::from_strings(&[("en", en), ("ja", JA)], true).unwrap_err().to_string();
        assert!(refused("privacy-title = Privacy\n").contains("0 times"));
        assert!(refused(&format!("{EN}{EN}")).contains("2 times"));
        assert!(refused("privacy-url = http://example.com/privacy\n").contains("https://"));
        assert!(refused("privacy-url = https:///privacy\n").contains("https://"));
        assert!(refused("privacy-url =\n").contains("https://"));
    }
}
