//! `eider.toml`, read with a deliberately small TOML subset.
//!
//! eider's only dependency is its SQL parser, so it carries its own TOML
//! reader, and the subset is only what the project file needs: `#` comments,
//! `[section]` headers, and bare `key = ["string", ...]` arrays (which may
//! span lines).
//! Anything outside that subset, and any unknown section or key, is an
//! error rather than ignored: a typo in a contract file should fail loudly,
//! not silently drop a declaration.
//!
//! ```toml
//! [sources]
//! bronze   = ["fieldbook__sightings", "fieldbook__species"]
//! reference = ["birds__taxonomy"]
//! ```
//!
//! `[sources]` declares the catalogs eider reads but never writes, and the
//! relations in each. A model may read only declared relations, which is
//! what makes the dependency graph complete: every edge ends at either a
//! model or a declared source.

use std::collections::{BTreeMap, BTreeSet};

use crate::project::LAYERS;

#[derive(Debug, Default)]
pub struct Config {
    /// Source catalog → declared relation names, all lowercase.
    pub sources: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug)]
pub struct ConfigError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "eider.toml:{}: {}", self.line, self.message)
    }
}

pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let mut config = Config::default();
    let mut section: Option<String> = None;
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line_no = i + 1;
        let line = strip_comment(lines[i]).trim().to_string();
        i += 1;
        if line.is_empty() {
            continue;
        }
        let err = |message: String| ConfigError {
            line: line_no,
            message,
        };
        if let Some(header) = line.strip_prefix('[') {
            let name = header
                .strip_suffix(']')
                .ok_or_else(|| err(format!("malformed section header `{line}`")))?
                .trim();
            if name != "sources" {
                return Err(err(format!("unknown section `[{name}]`")));
            }
            section = Some(name.to_string());
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| err(format!("expected `key = value`, got `{line}`")))?;
        let key = key.trim().to_ascii_lowercase();
        if !is_bare_key(&key) {
            return Err(err(format!(
                "expected a bare key (letters, digits, `_`, `-`), got `{key}`"
            )));
        }
        let mut value = value.trim().to_string();
        // An array may span lines: keep reading until its brackets close.
        while value.starts_with('[') && !brackets_closed(&value) {
            let next = lines
                .get(i)
                .ok_or_else(|| err(format!("unterminated array for `{key}`")))?;
            value.push(' ');
            value.push_str(strip_comment(next).trim());
            i += 1;
        }
        match section.as_deref() {
            Some("sources") => {
                // eider writes the layer catalogs, and a source is read-only
                // input; one catalog cannot be both.
                if LAYERS.contains(&key.as_str()) {
                    return Err(err(format!(
                        "`{key}` is a layer eider builds, so it cannot be a source"
                    )));
                }
                let names = parse_string_array(&value).map_err(err)?;
                let set = names.into_iter().map(|n| n.to_ascii_lowercase()).collect();
                if config.sources.insert(key.clone(), set).is_some() {
                    return Err(err(format!("source `{key}` declared twice")));
                }
            }
            _ => return Err(err(format!("key `{key}` outside a known section"))),
        }
    }
    Ok(config)
}

/// Drops a `#` comment, ignoring `#` inside a double-quoted string.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if in_string {
            if c == '"' && !escaped {
                in_string = false;
            }
            escaped = c == '\\' && !escaped;
        } else if c == '"' {
            in_string = true;
        } else if c == '#' {
            return &line[..i];
        }
    }
    line
}

/// A bare TOML key. Quoted and dotted keys are outside the subset.
fn is_bare_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Whether every `[` outside a string has its `]`, so a multi-line array
/// knows when to stop reading lines.
fn brackets_closed(value: &str) -> bool {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for c in value.chars() {
        if in_string {
            if c == '"' && !escaped {
                in_string = false;
            }
            escaped = c == '\\' && !escaped;
            continue;
        }
        match c {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

fn parse_string_array(value: &str) -> Result<Vec<String>, String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .ok_or_else(|| format!("expected an array of strings, got `{value}`"))?;
    let mut out = Vec::new();
    let mut chars = inner.chars().peekable();
    let skip_whitespace = |chars: &mut std::iter::Peekable<std::str::Chars>| {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
    };
    loop {
        skip_whitespace(&mut chars);
        match chars.next() {
            None => break,
            Some('"') => {
                let mut s = String::new();
                loop {
                    match chars.next() {
                        None => return Err("unterminated string".into()),
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => s.push(c),
                            Some(c) => return Err(format!("unsupported escape `\\{c}`")),
                            None => return Err("unterminated string".into()),
                        },
                        Some(c) => s.push(c),
                    }
                }
                out.push(s);
                // Elements are comma-separated; a trailing comma is allowed.
                skip_whitespace(&mut chars);
                match chars.next() {
                    None | Some(',') => {}
                    Some(c) => return Err(format!("expected `,` between strings, found `{c}`")),
                }
            }
            Some(c) => return Err(format!("expected a quoted string, found `{c}`")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sources_with_comments_and_multiline_arrays() {
        let text = "# project\n[sources]\nbronze = [\n  \"FieldBook_Sightings\", # raw\n  \"fieldbook__species\",\n]\nreference = [\"x#y\"]\n";
        let c = parse(text).unwrap();
        assert_eq!(
            c.sources["bronze"].iter().collect::<Vec<_>>(),
            ["fieldbook__species", "fieldbook_sightings"]
        );
        assert!(c.sources["reference"].contains("x#y"));
    }

    #[test]
    fn rejects_unknown_section_and_layer_as_source() {
        assert!(parse("[models]\n").is_err());
        let e = parse("[sources]\nsilver = []\n").unwrap_err();
        assert_eq!(e.line, 2);
    }

    #[test]
    fn rejects_key_outside_section_and_duplicates() {
        assert!(parse("bronze = []\n").is_err());
        assert!(parse("[sources]\nbronze = []\nbronze = []\n").is_err());
    }

    #[test]
    fn rejects_non_string_values() {
        assert!(parse("[sources]\nbronze = [1]\n").is_err());
        assert!(parse("[sources]\nbronze = \"x\"\n").is_err());
    }

    #[test]
    fn escaped_quote_does_not_hide_closing_bracket() {
        let c = parse("[sources]\nbronze = [\"a\\\"b\"]\n").unwrap();
        assert!(c.sources["bronze"].contains("a\"b"));
    }

    #[test]
    fn rejects_keys_outside_the_bare_subset() {
        for key in ["", "\"bronze\"", "bron ze", "bronze.raw"] {
            let e = parse(&format!("[sources]\n{key} = [\"a\"]\n")).unwrap_err();
            assert_eq!(e.line, 2, "{key}");
        }
    }

    #[test]
    fn array_elements_need_commas() {
        assert!(parse("[sources]\nbronze = [\"a\" \"b\"]\n").is_err());
        assert!(parse("[sources]\nbronze = [,\"a\"]\n").is_err());
        assert!(parse("[sources]\nbronze = [\"a\",,\"b\"]\n").is_err());
        let c = parse("[sources]\nbronze = [\"a\", \"b\",]\n").unwrap();
        assert_eq!(c.sources["bronze"].len(), 2);
    }

    #[test]
    fn unsupported_escape_names_the_character() {
        let e = parse("[sources]\nbronze = [\"a\\n\"]\n").unwrap_err();
        assert_eq!(e.message, "unsupported escape `\\n`");
    }
}
