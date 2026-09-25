/// Parser for .test files.
///
/// File format:
///
/// ```text
/// # This is a comment
///
/// con-cli <arguments...>   # preferred
/// # Legacy: old tests may still use `cmd ...`
/// ---- <mode>              # mode is optional, default: contains
/// expected output here
/// ```
///
/// The `----` line optionally carries the match mode:
///   ----              → contains (default)
///   ---- exact        → full string equality
///   ---- contains     → actual output contains the expected string
///   ---- json-subset  → every key/value in expected JSON exists in actual JSON
///   ---- ok           → only checks exit code == 0; expected block ignored
///   ---- error        → checks exit code != 0; expected matched against stderr
///   ---- regex        → expected is a regex pattern (not yet implemented)
///
/// Blank lines and lines starting with `#` outside a step block are ignored.
///
/// Both `con-cli ...` (preferred) and legacy `cmd ...` step directives are
/// accepted. Legacy `match <mode>` line between the step directive and `----`
/// is also accepted for backwards compatibility but the inline `---- <mode>`
/// form is preferred.
use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq, Default)]
pub enum MatchMode {
    /// Full string equality (trailing whitespace trimmed per line)
    Exact,
    /// Actual output contains the expected string (default)
    #[default]
    Contains,
    /// Expected is valid JSON; every key present in expected must match in actual
    JsonSubset,
    /// Expected is a regex matched against actual
    Regex,
    /// Only check exit code == 0; ignore expected block
    Ok,
    /// Check exit code != 0; match expected against stderr
    Error,
}

impl MatchMode {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim() {
            "" => Ok(MatchMode::Contains),
            "exact" => Ok(MatchMode::Exact),
            "contains" => Ok(MatchMode::Contains),
            "json-subset" => Ok(MatchMode::JsonSubset),
            "regex" => Ok(MatchMode::Regex),
            "ok" => Ok(MatchMode::Ok),
            "error" => Ok(MatchMode::Error),
            other => bail!(
                "unknown match mode {:?}; valid: exact, contains, json-subset, ok, error",
                other
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Step {
    /// Human-readable label (from inline comment on cmd line, or auto-generated)
    pub label: String,
    /// Arguments to pass to con-cli (already split on whitespace, respecting quotes)
    pub args: Vec<String>,
    pub match_mode: MatchMode,
    /// Expected output (empty for `ok` / `error` with no expected block)
    pub expected: String,
    /// 1-based line number of the `cmd` directive in the source file
    pub line: usize,
}

pub fn parse_file(path: &std::path::Path) -> Result<Vec<Step>> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    parse_source(&source, path.display().to_string())
}

pub fn parse_source(source: &str, origin: String) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        // Skip blank lines and standalone comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }

        if let Some(rest) = step_args(trimmed) {
            let cmd_line = i + 1; // 1-based

            // Split inline label comment: `con-cli foo bar  # my label`
            let (args_str, label) = split_inline_comment(rest);
            let args = shell_split(args_str.trim()).with_context(|| {
                format!("{origin}:{cmd_line}: failed to parse con-cli arguments")
            })?;

            let label = if label.is_empty() {
                format!("line {cmd_line}: {}", args_str.trim())
            } else {
                label.to_string()
            };

            i += 1;

            // Legacy: optional `match <mode>` line before `----`
            let mut legacy_mode: Option<MatchMode> = None;
            if i < lines.len() {
                let next = lines[i].trim();
                if let Some(mode_str) = next.strip_prefix("match ") {
                    legacy_mode = Some(
                        MatchMode::parse(mode_str)
                            .with_context(|| format!("{origin}:{}: invalid match mode", i + 1))?,
                    );
                    i += 1;
                }
            }

            // Expect `----` separator, optionally followed by match mode
            if i >= lines.len() {
                bail!(
                    "{origin}:{}: expected `----` after con-cli/cmd block, got <eof>",
                    i + 1
                );
            }
            let sep_line = lines[i].trim();
            let inline_mode_str = if sep_line == "----" {
                ""
            } else if let Some(mode) = sep_line.strip_prefix("---- ") {
                mode.trim()
            } else {
                bail!(
                    "{origin}:{}: expected `----` after con-cli/cmd block, got {:?}",
                    i + 1,
                    sep_line
                );
            };
            let match_mode = if let Some(legacy) = legacy_mode {
                // Legacy `match` line takes precedence if both are present
                legacy
            } else {
                MatchMode::parse(inline_mode_str)
                    .with_context(|| format!("{origin}:{}: invalid match mode", i + 1))?
            };
            i += 1; // skip ----

            // Collect expected output until blank line, next step directive, or EOF
            let mut expected_lines: Vec<&str> = Vec::new();
            while i < lines.len() {
                let line = lines[i];
                let t = line.trim();
                if step_args(t).is_some() {
                    break;
                }
                // A blank line ends the expected block (only after content has started)
                if t.is_empty() && !expected_lines.is_empty() {
                    break;
                }
                expected_lines.push(line);
                i += 1;
            }

            // Trim trailing blank lines from expected block
            while expected_lines
                .last()
                .map(|l: &&str| l.trim().is_empty())
                .unwrap_or(false)
            {
                expected_lines.pop();
            }

            let expected = expected_lines.join("\n");

            steps.push(Step {
                label,
                args,
                match_mode,
                expected,
                line: cmd_line,
            });
        } else {
            bail!(
                "{origin}:{}: unexpected line {:?} (expected `con-cli`, legacy `cmd`, `#`, or blank)",
                i + 1,
                trimmed
            );
        }
    }

    Ok(steps)
}

fn step_args(line: &str) -> Option<&str> {
    line.strip_prefix("con-cli ")
        .or_else(|| if line == "con-cli" { Some("") } else { None })
        .or_else(|| line.strip_prefix("cmd "))
        .or_else(|| if line == "cmd" { Some("") } else { None })
}

/// Split `foo bar  # comment` into (`foo bar`, `comment`).
fn split_inline_comment(s: &str) -> (&str, &str) {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = s.as_bytes();
    for idx in 0..bytes.len() {
        match bytes[idx] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single && !in_double && (idx == 0 || bytes[idx - 1] == b' ') => {
                let args = s[..idx].trim_end();
                let comment = s[idx + 1..].trim();
                return (args, comment);
            }
            _ => {}
        }
    }
    (s, "")
}

/// Very small shell-word splitter: handles double-quoted strings and backslash escapes.
pub fn shell_split(s: &str) -> Result<Vec<String>> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_double = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => in_double = !in_double,
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ' ' | '\t' if !in_double => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(c),
        }
    }
    if in_double {
        bail!("unterminated double-quoted string in: {s:?}");
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_step_default_contains() {
        let src = "con-cli --json identify\n----\n{\"version\":\n";
        let steps = parse_source(src, "test".into()).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].args, vec!["--json", "identify"]);
        assert_eq!(steps[0].match_mode, MatchMode::Contains);
        assert!(steps[0].expected.contains("{\"version\":"));
    }

    #[test]
    fn parse_legacy_cmd_directive_still_works() {
        let src = "cmd --json identify\n----\n{\"version\":\n";
        let steps = parse_source(src, "test".into()).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].args, vec!["--json", "identify"]);
        assert_eq!(steps[0].match_mode, MatchMode::Contains);
    }

    #[test]
    fn parse_inline_mode_on_separator() {
        let src = "con-cli --json tabs list\n---- exact\n{}\n";
        let steps = parse_source(src, "test".into()).unwrap();
        assert_eq!(steps[0].match_mode, MatchMode::Exact);
    }

    #[test]
    fn parse_inline_mode_json_subset() {
        let src = "con-cli --json tabs list\n---- json-subset\n{\"tabs\":[]}\n";
        let steps = parse_source(src, "test".into()).unwrap();
        assert_eq!(steps[0].match_mode, MatchMode::JsonSubset);
    }

    #[test]
    fn parse_inline_mode_ok_empty_expected() {
        let src = "con-cli tabs new\n---- ok\n";
        let steps = parse_source(src, "test".into()).unwrap();
        assert_eq!(steps[0].match_mode, MatchMode::Ok);
        assert_eq!(steps[0].expected, "");
    }

    #[test]
    fn parse_legacy_match_line_still_works() {
        let src = "con-cli --json tabs list\nmatch json-subset\n----\n{\"tabs\":[]}\n";
        let steps = parse_source(src, "test".into()).unwrap();
        assert_eq!(steps[0].match_mode, MatchMode::JsonSubset);
    }

    #[test]
    fn separator_requires_space_before_mode() {
        let src = "con-cli --json tabs list\n----json-subset\n{\"tabs\":[]}\n";
        assert!(parse_source(src, "test".into()).is_err());
    }

    #[test]
    fn parse_multiple_steps() {
        let src = "con-cli identify\n----\ncon\n\ncon-cli --json tabs list\n---- json-subset\n{\"tabs\":[]}\n";
        let steps = parse_source(src, "test".into()).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1].match_mode, MatchMode::JsonSubset);
    }

    #[test]
    fn inline_label_comment() {
        let src = "con-cli --json identify  # smoke test\n----\ncon\n";
        let steps = parse_source(src, "test".into()).unwrap();
        assert_eq!(steps[0].label, "smoke test");
    }

    #[test]
    fn shell_split_quoted() {
        assert_eq!(
            shell_split(r#"panes exec --tab 1 "echo hello world""#).unwrap(),
            vec!["panes", "exec", "--tab", "1", "echo hello world"]
        );
    }

    #[test]
    fn shell_split_backslash() {
        assert_eq!(
            shell_split(r"panes exec --tab 1 echo\ hello").unwrap(),
            vec!["panes", "exec", "--tab", "1", "echo hello"]
        );
    }
}
