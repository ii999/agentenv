//! The SPEC-005 path grammar and path resolution over a profile's entries.
//!
//! One segment grammar serves every path-shaped input. Segments are
//! dot-separated; an unquoted segment is one or more characters other than
//! `.`, `"`, and whitespace, and a fully quoted segment (`"..."`) may contain
//! dots and spaces. Empty segments, empty quoted segments, partial quoting,
//! and keys containing a double quote are grammar errors (SPEC-AS-010 /
//! SPEC-AS-024); there are no escape sequences. `parse(render(path)) ==
//! path` holds for every parsed path, and every rendered path is accepted by
//! `get` verbatim.

use toml::Value;

use crate::config::Profile;
use crate::error::AppError;

/// A parsed path: one or more segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segments {
    segments: Vec<String>,
}

impl Segments {
    /// Parses `input` against the segment grammar. Grammar errors are usage
    /// errors (exit 1, SPEC-018) naming the path and the reason.
    pub fn parse(input: &str) -> Result<Self, AppError> {
        let mut segments = Vec::new();
        let mut rest = input;
        loop {
            if rest.is_empty() {
                if segments.is_empty() {
                    return Err(invalid_path(
                        input,
                        "the path is empty; a path needs at least one segment",
                    ));
                }
                break;
            }
            let (segment, remainder) = parse_segment(input, rest)?;
            segments.push(segment.to_owned());
            rest = remainder;
            if rest.is_empty() {
                break;
            }
            // The remainder starts with '.', the separator; it must be
            // followed by another segment.
            rest = &rest[1..];
            if rest.is_empty() {
                return Err(invalid_path(input, "trailing '.' (empty segment)"));
            }
        }
        Ok(Self { segments })
    }

    /// Renders the path in the segment grammar: segments containing `.`,
    /// `"`, or whitespace are quoted. `parse` accepts the result verbatim.
    pub fn render(&self) -> String {
        render_segments(self.segments.iter().map(String::as_str))
    }

    /// The segments, in order.
    pub fn as_slice(&self) -> &[String] {
        &self.segments
    }

    /// The number of segments.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Always `false`: parsing never produces an empty path.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

/// Parses one segment from the front of `rest`, returning the segment text
/// and the remainder (empty, or starting with the `.` separator).
fn parse_segment<'a>(input: &str, rest: &'a str) -> Result<(&'a str, &'a str), AppError> {
    if let Some(quoted) = rest.strip_prefix('"') {
        let Some(end) = quoted.find('"') else {
            return Err(invalid_path(input, "unterminated quoted segment"));
        };
        let segment = &quoted[..end];
        if segment.is_empty() {
            return Err(invalid_path(input, "empty quoted segment"));
        }
        let remainder = &quoted[end + 1..];
        if !(remainder.is_empty() || remainder.starts_with('.')) {
            return Err(invalid_path(
                input,
                "characters after the closing quote of a quoted segment; a quoted segment must \
                 occupy the whole segment",
            ));
        }
        return Ok((segment, remainder));
    }
    let mut end = rest.len();
    for (offset, char) in rest.char_indices() {
        if char == '.' {
            end = offset;
            break;
        }
        if char == '"' {
            return Err(invalid_path(
                input,
                "a segment containing '\"' must be fully quoted; keys containing a double \
                 quote are not addressable in v1",
            ));
        }
        if char.is_whitespace() {
            return Err(invalid_path(
                input,
                "unquoted segments cannot contain whitespace; quote the segment, e.g. \
                 'entry.\"two words\"'",
            ));
        }
    }
    if end == 0 {
        return Err(invalid_path(input, "empty segment"));
    }
    Ok((&rest[..end], &rest[end..]))
}

fn invalid_path(input: &str, reason: &str) -> AppError {
    AppError::Usage(format!("invalid path '{input}': {reason}"))
}

fn render_segments<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    parts
        .map(|part| {
            if part
                .chars()
                .any(|c| c == '.' || c == '"' || c.is_whitespace())
            {
                format!("\"{part}\"")
            } else {
                part.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Resolves `segments` against `profile`: the first segment is an entry
/// name, the rest navigate entry tables (arrays are retrieved whole,
/// SPEC-AS-003). Unknown paths are name-resolution failures (exit 3) naming
/// the failing path and a next action.
pub fn resolve<'a>(profile: &'a Profile, segments: &Segments) -> Result<&'a Value, AppError> {
    let parts = segments.as_slice();
    let Some(entry_name) = parts.first() else {
        return Err(AppError::Usage(
            "invalid path: a path needs at least one segment".to_owned(),
        ));
    };
    let Some(entry) = profile.entries.get(entry_name.as_str()) else {
        return Err(AppError::NotFound(format!(
            "unknown path '{}': entry '{}' is not defined in profile '{}'; run \
             'agent-context list' to see the entries of the active profile",
            segments.render(),
            entry_name,
            profile.name
        )));
    };
    let mut current = entry;
    let mut traversed: Vec<&str> = vec![entry_name.as_str()];
    for segment in &parts[1..] {
        let Some(table) = current.as_table() else {
            return Err(AppError::NotFound(format!(
                "unknown path '{}': '{}' is a {} and has no field '{}'; run 'agent-context \
                 get {}' to read it",
                segments.render(),
                render_segments(traversed.iter().copied()),
                current.type_str(),
                segment,
                render_segments(traversed.iter().copied())
            )));
        };
        let Some(next) = table.get(segment.as_str()) else {
            return Err(AppError::NotFound(format!(
                "unknown path '{}': entry '{}' has no field '{}'; run 'agent-context list {}' \
                 to see its fields",
                segments.render(),
                entry_name,
                segment,
                entry_name
            )));
        };
        current = next;
        traversed.push(segment.as_str());
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use toml::Table;

    use super::{resolve, Segments};
    use crate::config::Profile;
    use crate::error::AppError;

    fn segments(input: &str) -> Segments {
        Segments::parse(input).unwrap_or_else(|error| panic!("'{input}' should parse: {error}"))
    }

    fn test_profile(entries_toml: &str) -> Profile {
        Profile {
            name: "work".to_owned(),
            description: "the work profile".to_owned(),
            entries: entries_toml
                .parse::<Table>()
                .unwrap_or_else(|error| panic!("the entry fixture should parse: {error}")),
        }
    }

    #[test]
    fn grammar_table_valid_paths() {
        let cases = [
            ("llm", vec!["llm"]),
            ("llm.endpoint", vec!["llm", "endpoint"]),
            ("a.b.c", vec!["a", "b", "c"]),
            ("\"quoted\"", vec!["quoted"]),
            ("server.\"my.field\"", vec!["server", "my.field"]),
            ("a.\"two words\"", vec!["a", "two words"]),
            ("a.\"b\".c", vec!["a", "b", "c"]),
            ("entry.日本語", vec!["entry", "日本語"]),
            ("x[0]", vec!["x[0]"]),
        ];
        for (input, expected) in cases {
            assert_eq!(
                segments(input).as_slice(),
                expected.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "'{input}' should parse to {expected:?}"
            );
        }
    }

    #[test]
    fn grammar_table_invalid_paths() {
        // AC-005.3 and SPEC-AS-010/-024.
        let cases = [
            "", ".", "a.", "a..b", "..", "\"\"", "\"a", "a\"b", "\"a\"b", "\"a\".", "a b", "a .b",
            " a", "a. b",
        ];
        for input in cases {
            match Segments::parse(input) {
                Err(AppError::Usage(message)) => {
                    assert!(message.contains(input) || input.is_empty(), "{message}");
                }
                other => panic!("expected '{input}' to be a grammar error, got {other:?}"),
            }
        }
    }

    #[test]
    fn render_round_trips() {
        let inputs = [
            "llm",
            "llm.endpoint",
            "server.\"my.field\"",
            "a.\"two words\"",
            "a.\"b\".c",
            "entry.日本語",
        ];
        for input in inputs {
            let parsed = segments(input);
            let rendered = parsed.render();
            assert_eq!(
                Segments::parse(&rendered).expect("a rendered path parses"),
                parsed,
                "parse(render('{input}')) must equal parse('{input}')"
            );
        }
    }

    #[test]
    fn render_quotes_only_when_needed() {
        assert_eq!(segments("llm.endpoint").render(), "llm.endpoint");
        assert_eq!(
            segments("server.\"my.field\"").render(),
            "server.\"my.field\""
        );
        assert_eq!(segments("a.\"plain\"").render(), "a.plain");
    }

    #[test]
    fn resolve_returns_scalars_arrays_and_tables() {
        let profile = test_profile(
            r#"
[llm]
endpoint = "https://llm.example.com/v1"
retries = 3
ratio = 1.5
enabled = true
tags = ["linux", "self-hosted"]

[llm.nested]
key = "value"
"#,
        );
        let assert_type = |path: &str, expected: &str| {
            let value = resolve(&profile, &segments(path))
                .unwrap_or_else(|error| panic!("'{path}' should resolve: {error}"));
            assert_eq!(value.type_str(), expected, "'{path}'");
        };
        assert_type("llm.endpoint", "string");
        assert_type("llm.retries", "integer");
        assert_type("llm.ratio", "float");
        assert_type("llm.enabled", "boolean");
        assert_type("llm.tags", "array");
        assert_type("llm", "table");
        assert_type("llm.nested", "table");
        assert_type("llm.nested.key", "string");
        // A quoted segment reaches keys with dots (AC-005.1, logic level).
        let dotted = test_profile(
            r#"
[server]
"my.field" = "value"
"#,
        );
        let value =
            resolve(&dotted, &segments("server.\"my.field\"")).expect("the dotted key resolves");
        assert_eq!(value.as_str(), Some("value"));
    }

    #[test]
    fn resolve_unknown_entry_is_not_found() {
        let profile = test_profile("[llm]\nendpoint = \"x\"\n");
        match resolve(&profile, &segments("nosuch.field")) {
            Err(AppError::NotFound(message)) => {
                assert!(message.contains("nosuch.field"), "{message}");
                assert!(message.contains("agent-context list"), "{message}");
            }
            other => panic!("expected a not-found error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_unknown_field_is_not_found_with_next_action() {
        // AC-005.2 (logic level).
        let profile = test_profile("[llm]\nendpoint = \"x\"\n");
        match resolve(&profile, &segments("llm.region")) {
            Err(AppError::NotFound(message)) => {
                assert!(message.contains("llm.region"), "{message}");
                assert!(message.contains("list llm"), "{message}");
            }
            other => panic!("expected a not-found error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_into_a_scalar_is_not_found() {
        let profile = test_profile("[llm]\nendpoint = \"x\"\n");
        match resolve(&profile, &segments("llm.endpoint.deep")) {
            Err(AppError::NotFound(message)) => {
                assert!(message.contains("llm.endpoint.deep"), "{message}");
                assert!(message.contains("llm.endpoint"), "{message}");
            }
            other => panic!("expected a not-found error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_into_an_array_is_not_found() {
        // SPEC-AS-003: arrays are retrieved whole; elements are not
        // addressable.
        let profile = test_profile("[ci]\ntags = [\"a\"]\n");
        match resolve(&profile, &segments("ci.tags.0")) {
            Err(AppError::NotFound(message)) => {
                assert!(message.contains("ci.tags.0"), "{message}");
            }
            other => panic!("expected a not-found error, got {other:?}"),
        }
    }
}
