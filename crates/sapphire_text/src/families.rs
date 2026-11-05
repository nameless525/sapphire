//! The family string boundary. Parley's pinned parser handles ordinary CSS lists, but treats
//! backslashes literally even in quoted names. Decode quoted escapes before handing it named
//! families, so punctuation in an imported family never changes its identity.

use std::borrow::Cow;

use parley::style::{FontFamily, FontFamilyName};

/// Family names as a quoted CSS list. Always quote names, including names such as `serif` that
/// would otherwise select a generic family.
pub fn family_list<'a>(families: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for family in families {
        if !out.is_empty() {
            out.push_str(", ");
        }
        let quote = if family.contains('\'') { '"' } else { '\'' };
        out.push(quote);
        for letter in family.chars() {
            if letter == quote || letter == '\\' {
                out.push('\\');
                out.push(letter);
            } else {
                match letter {
                    '\n' => out.push_str("\\a "),
                    '\r' => out.push_str("\\d "),
                    '\u{c}' => out.push_str("\\c "),
                    _ => out.push(letter),
                }
            }
        }
        out.push(quote);
    }
    out
}

pub(crate) fn font_families(source: &str) -> FontFamily<'_> {
    if !source.contains('\\') {
        return FontFamily::Source(source.into());
    }
    FontFamily::List(escaped_list(source).into())
}

/// Keep the pinned parser's generic and unquoted-name behavior and stop at the first malformed
/// entry as it does. Only quoted strings add escape handling.
fn escaped_list(mut source: &str) -> Vec<FontFamilyName<'_>> {
    let mut families = Vec::new();
    loop {
        source = whitespace(source);
        if source.is_empty() {
            break;
        }
        let (family, rest) = if source.starts_with(['\'', '"']) {
            let Some((name, rest)) = quoted(source) else { break };
            (FontFamilyName::Named(name), rest)
        } else {
            let end = source.find(',').unwrap_or(source.len());
            let Some(Ok(family)) = FontFamilyName::parse_css_list(&source[..end]).next() else { break };
            (family, &source[end..])
        };
        let rest = whitespace(rest);
        if !rest.is_empty() && !rest.starts_with(',') {
            break;
        }
        families.push(family);
        let Some(rest) = rest.strip_prefix(',') else { break };
        source = rest;
    }
    families
}

fn whitespace(source: &str) -> &str {
    source.trim_start_matches(|letter: char| letter.is_ascii_whitespace())
}

fn quoted(source: &str) -> Option<(Cow<'_, str>, &str)> {
    let quote = source.chars().next()?;
    let content = &source[1..];
    let mut letters = content.char_indices().peekable();
    let mut decoded = None::<String>;
    while let Some((index, letter)) = letters.next() {
        if letter == quote {
            let name = decoded.map_or_else(|| Cow::Borrowed(&content[..index]), Cow::Owned);
            return Some((name, &content[index + 1..]));
        }
        if letter != '\\' {
            if let Some(decoded) = &mut decoded {
                decoded.push(letter);
            }
            continue;
        }
        let out = decoded.get_or_insert_with(|| content[..index].to_owned());
        let (_, escaped) = letters.next()?;
        if escaped.is_ascii_hexdigit() {
            let mut code = escaped.to_digit(16)?;
            for _ in 1..6 {
                let Some((_, digit)) = letters.peek().copied() else { break };
                let Some(digit) = digit.to_digit(16).filter(|_| digit.is_ascii()) else { break };
                letters.next();
                code = code * 16 + digit;
            }
            if let Some(&(_, letter)) = letters.peek()
                && letter.is_ascii_whitespace()
            {
                letters.next();
                if letter == '\r' && letters.peek().is_some_and(|(_, letter)| *letter == '\n') {
                    letters.next();
                }
            }
            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
        } else if escaped == '\r' {
            if letters.peek().is_some_and(|(_, letter)| *letter == '\n') {
                letters.next();
            }
        } else if escaped != '\n' && escaped != '\u{c}' {
            out.push(escaped);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<FontFamilyName<'_>> {
        match font_families(source) {
            FontFamily::Source(source) => FontFamilyName::parse_css_list(&source)
                .map_while(Result::ok)
                .map(FontFamilyName::into_owned)
                .collect(),
            FontFamily::List(families) => families.into_owned(),
            FontFamily::Single(_) => unreachable!(),
        }
    }

    #[test]
    fn imported_names_round_trip_without_changing_identity_or_fallback_order() {
        let families = [
            "Oxanium",
            "O'Brien",
            "Double \" quote",
            "Both '\", comma",
            "slash\\quote'\"",
            "  padded  ",
            "日本語",
            "serif",
            "line\nbreak",
        ];
        let list = family_list(families);
        let expected: Vec<_> = families.into_iter().map(FontFamilyName::named).collect();
        assert_eq!(names(&list), expected);
        // Ordinary names and the single-apostrophe witness also pass the actual pinned parser.
        let source = family_list(["O'Brien", "M PLUS 1"]);
        assert_eq!(
            FontFamilyName::parse_css_list(&source).collect::<Result<Vec<_>, _>>().unwrap(),
            [FontFamilyName::named("O'Brien"), FontFamilyName::named("M PLUS 1")]
        );
    }

    #[test]
    fn quoted_escapes_and_generic_names_reach_the_structured_boundary() {
        let source = r#"serif, 'O\27 Brien', "a\"b", 'back\\slash', sans-serif"#;
        let expected = [
            FontFamilyName::parse("serif").unwrap(),
            FontFamilyName::named("O'Brien"),
            FontFamilyName::named("a\"b"),
            FontFamilyName::named("back\\slash"),
            FontFamilyName::parse("sans-serif").unwrap(),
        ];
        assert_eq!(names(source), expected);
    }

    #[test]
    fn ordinary_syntax_and_first_error_behavior_stay_with_the_pinned_parser() {
        for source in [
            "serif, 'M PLUS 1', monospace",
            "Arial,",
            ", Arial",
            "Arial,,serif",
            "'bad' serif",
            "Arial, 'unfinished",
            "A\\B, serif",
        ] {
            let expected: Vec<_> = FontFamilyName::parse_css_list(source).map_while(Result::ok).collect();
            assert_eq!(names(source), expected, "{source}");
        }
        assert_eq!(names(r#"Arial, 'bad\' string' serif, monospace"#), [FontFamilyName::named("Arial")]);
    }
}
