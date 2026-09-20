//! Lossless-enough editor for Con's Ghostty-compatible line configuration.
//!
//! Unknown native entries, comments, blank lines and their order are retained.
//! `config-file` is deliberately not expanded here: native backends receive it via
//! [`NativeEntry`], while Con settings are resolved only from this file.

use super::Config;
use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEntry {
    pub key: String,
    pub value: String,
    pub line: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SourceState {
    pub path: Option<PathBuf>,
    pub source: String,
    pub authored: Option<Value>,
    pub native: Vec<NativeEntry>,
}

const NATIVE: &[(&str, &str)] = &[
    ("command", "terminal.shell"),
    ("font-size", "terminal.font_size"),
    ("theme", "terminal.theme"),
    ("cursor-style", "terminal.cursor_style"),
    ("clipboard-write", "terminal.clipboard_write"),
    ("background-opacity", "appearance.terminal_opacity"),
    ("background-blur", "appearance.terminal_blur"),
    ("background-image", "appearance.background_image"),
    (
        "background-image-opacity",
        "appearance.background_image_opacity",
    ),
    (
        "background-image-position",
        "appearance.background_image_position",
    ),
    ("background-image-fit", "appearance.background_image_fit"),
    (
        "background-image-repeat",
        "appearance.background_image_repeat",
    ),
];

fn parse_line(line: &str) -> Option<(&str, &str)> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    let (key, value) = t.split_once('=')?;
    Some((key.trim(), value.trim()))
}

fn decode_string(raw: &str, key: &str) -> Result<String> {
    if raw.contains(['\n', '\r', '\0']) {
        bail!("{key}: control characters are not supported")
    }
    if !raw.starts_with('"') {
        return Ok(raw.to_owned());
    }
    if raw.len() < 2 || !raw.ends_with('"') {
        bail!("{key}: malformed quoted value")
    }
    // Ghostty strips the outer quotes; it does not unescape their contents.
    Ok(raw[1..raw.len() - 1].to_owned())
}

fn path_for_key(key: &str) -> Option<String> {
    if key == "con.version" {
        return None;
    }
    if let Some(path) = NATIVE.iter().find_map(|(k, p)| (*k == key).then_some(*p)) {
        return Some(path.into());
    }
    let con = key.strip_prefix("con.")?;
    (!NATIVE.iter().any(|(_, path)| *path == con)
        && !matches!(con, "terminal.font_family" | "terminal.font_fallback"))
    .then(|| con.to_owned())
}

fn schema_value<'a>(schema: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(schema, |v, part| v.get(part))
}

#[derive(Clone, Copy)]
enum ScalarKind {
    String,
    Bool,
    Float,
    Integer,
}

fn scalar_kind(path: &str, schema: &Value) -> ScalarKind {
    if path == "agent.temperature" {
        return ScalarKind::Float;
    }
    if path == "agent.max_turns"
        || (path.starts_with("agent.providers.") && path.ends_with(".max_tokens"))
    {
        return ScalarKind::Integer;
    }
    match schema {
        Value::Bool(_) => ScalarKind::Bool,
        Value::Number(n) if n.is_u64() || n.is_i64() => ScalarKind::Integer,
        Value::Number(_) => ScalarKind::Float,
        _ => ScalarKind::String,
    }
}

fn parse_scalar(raw: &str, kind: ScalarKind, key: &str) -> Result<Value> {
    if matches!(kind, ScalarKind::String) {
        return decode_string(raw, key).map(Value::String);
    }
    let decoded = decode_string(raw, key)?;
    let raw = decoded.as_str();
    match kind {
        ScalarKind::Bool => raw
            .parse::<bool>()
            .map(Value::Bool)
            .map_err(|_| anyhow!("{key}: expected boolean")),
        ScalarKind::Float => raw
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| anyhow!("{key}: expected finite number")),
        ScalarKind::Integer => raw
            .parse::<u64>()
            .map(serde_json::Number::from)
            .map(Value::Number)
            .map_err(|_| anyhow!("{key}: expected non-negative integer")),
        ScalarKind::String => unreachable!(),
    }
}

fn complete_schema() -> Result<Value> {
    let mut schema = serde_json::to_value(Config::default())?;
    let providers = [
        "anthropic",
        "openai",
        "chatgpt",
        "github-copilot",
        "openaicompatible",
        "minimax",
        "minimax-anthropic",
        "moonshot",
        "moonshot-anthropic",
        "z-ai",
        "z-ai-anthropic",
        "deepseek",
        "groq",
        "cohere",
        "gemini",
        "ollama",
        "openrouter",
        "perplexity",
        "mistral",
        "together",
        "xai",
    ];
    for provider in providers {
        insert(
            &mut schema,
            &format!("agent.providers.{provider}.model"),
            Value::Null,
            false,
        );
        for field in ["api_key", "api_key_env", "base_url"] {
            insert(
                &mut schema,
                &format!("agent.providers.{provider}.{field}"),
                Value::Null,
                false,
            );
        }
        insert(
            &mut schema,
            &format!("agent.providers.{provider}.max_tokens"),
            Value::Number(0u64.into()),
            false,
        );
    }
    Ok(schema)
}

fn insert(root: &mut Value, path: &str, value: Value, list: bool) {
    let mut at = root;
    let parts: Vec<_> = path.split('.').collect();
    for part in &parts[..parts.len() - 1] {
        at = at
            .as_object_mut()
            .unwrap()
            .entry((*part).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let map = at.as_object_mut().unwrap();
    let leaf = parts[parts.len() - 1];
    if list {
        map.entry(leaf)
            .or_insert_with(|| Value::Array(vec![]))
            .as_array_mut()
            .unwrap()
            .push(value);
    } else {
        map.insert(leaf.to_owned(), value);
    }
}

pub(crate) fn parse(source: &str, path: Option<PathBuf>) -> Result<Config> {
    let defaults = complete_schema()?;
    let mut document = serde_json::to_value(Config::default())?;
    let mut native = Vec::new();
    let mut fonts = Vec::new();
    let mut seen_lists = std::collections::HashSet::new();
    for (index, line) in source.lines().enumerate() {
        let Some((key, raw)) = parse_line(line) else {
            let trimmed = line.trim();
            if trimmed.starts_with("con.") && !trimmed.starts_with('#') {
                bail!("malformed Con setting at line {}", index + 1);
            }
            continue;
        };
        if key.is_empty() {
            bail!("malformed Con setting at line {}", index + 1);
        }
        if key == "con.version" {
            if raw.parse::<u32>().ok() != Some(SCHEMA_VERSION) {
                bail!(
                    "unsupported con configuration version at line {}",
                    index + 1
                );
            }
            continue;
        }
        if !key.starts_with("con.") {
            native.push(NativeEntry {
                key: key.into(),
                value: raw.into(),
                line: index + 1,
            });
        }
        if key == "font-family" {
            let font = decode_string(raw, key)?;
            if !font.is_empty() && super::is_gpui_pseudo_font_family(&font) {
                bail!("font-family: GPUI aliases are not valid terminal fonts")
            }
            fonts.push(font);
            continue;
        }
        let Some(field) = path_for_key(key) else {
            if key.starts_with("con.") {
                bail!("unknown Con key `{key}` at line {}", index + 1);
            }
            continue;
        };
        if key.starts_with("con.") && NATIVE.iter().any(|(_, path)| *path == field) {
            bail!(
                "native-owned field must use its native key at line {}",
                index + 1
            );
        }
        let Some(shape) = schema_value(&defaults, &field) else {
            if key.starts_with("con.") {
                bail!("unknown Con key `{key}` at line {}", index + 1);
            }
            continue;
        };
        if shape.is_object() {
            bail!(
                "Con key `{key}` names a section, not a setting at line {}",
                index + 1
            );
        }
        if !key.starts_with("con.") && raw.is_empty() {
            insert(&mut document, &field, shape.clone(), false);
            continue;
        }
        let is_list = shape.is_array();
        if key.starts_with("con.") && raw.is_empty() && !is_list {
            bail!("malformed Con setting at line {}", index + 1);
        }
        if is_list && seen_lists.insert(field.clone()) {
            insert(&mut document, &field, Value::Array(Vec::new()), false);
        }
        if is_list && raw.is_empty() {
            insert(&mut document, &field, Value::Array(Vec::new()), false);
            continue;
        }
        let scalar_shape = shape.as_array().and_then(|a| a.first()).unwrap_or(shape);
        let kind = scalar_kind(&field, scalar_shape);
        // These native settings have a richer Ghostty type than Con's portable
        // representation. Retain valid native values without forcing them into Con.
        if key == "clipboard-write" {
            let value = match decode_string(raw, key)?.as_str() {
                "allow" => Value::Bool(true),
                "deny" => Value::Bool(false),
                _ => continue,
            };
            insert(&mut document, &field, value, false);
            continue;
        }
        if key == "background-blur" && decode_string(raw, key)?.parse::<bool>().is_err() {
            continue;
        }
        insert(
            &mut document,
            &field,
            parse_scalar(raw, kind, key)?,
            is_list,
        );
    }
    if schema_value(&document, "terminal.font_size")
        .and_then(Value::as_f64)
        .is_some_and(|size| !size.is_finite() || size <= 0.0)
    {
        bail!("font-size: expected a positive finite number")
    }
    if schema_value(&document, "terminal.font_family")
        .and_then(Value::as_str)
        .is_some_and(super::is_gpui_pseudo_font_family)
        || schema_value(&document, "terminal.font_fallback")
            .and_then(Value::as_array)
            .is_some_and(|fonts| {
                fonts
                    .iter()
                    .filter_map(Value::as_str)
                    .any(super::is_gpui_pseudo_font_family)
            })
    {
        bail!("terminal fonts cannot use GPUI aliases")
    }
    if !fonts.is_empty() {
        // Ghostty's empty value resets earlier repeated font-family entries.
        if let Some(last_reset) = fonts.iter().rposition(String::is_empty) {
            fonts.drain(..=last_reset);
        }
        if let Some(primary) = fonts.first() {
            insert(
                &mut document,
                "terminal.font_family",
                Value::String(primary.clone()),
                false,
            );
        }
        insert(
            &mut document,
            "terminal.font_fallback",
            Value::Array(fonts.into_iter().skip(1).map(Value::String).collect()),
            false,
        );
    }
    let mut config: Config = serde_json::from_value(document)
        .map_err(|_| anyhow!("invalid Con configuration value type"))?;
    let has_key = |expected| {
        source
            .lines()
            .filter_map(parse_line)
            .any(|(key, _)| key == expected)
    };
    if has_key("con.agent.provider") && !has_key("con.agent.provider_is_explicit") {
        config.agent.provider_is_explicit = true;
    }
    config.agent.migrate_legacy();
    config.normalize();
    // Snapshot resolved state, not defaults or pre-normalized input. This keeps
    // an unrelated settings edit from materializing implicit values.
    let authored = serde_json::to_value(&config)?;
    config.source = Mutex::new(SourceState {
        path,
        source: source.into(),
        authored: Some(authored),
        native,
    });
    Ok(config)
}

fn scalar_text(value: &Value, key: &str) -> Result<String> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::Null => String::new(),
        _ => bail!("{key}: nested payload is not representable"),
    };
    if text.contains(['\n', '\r', '\0']) {
        bail!("{key}: value contains an unsupported control character")
    }
    if matches!(value, Value::String(_)) {
        Ok(format!("\"{text}\""))
    } else {
        Ok(text)
    }
}

fn key_for_path(path: &str) -> String {
    NATIVE
        .iter()
        .find_map(|(k, p)| (*p == path).then_some((*k).to_owned()))
        .unwrap_or_else(|| format!("con.{path}"))
}

fn flatten(value: &Value, prefix: &str, out: &mut Vec<(String, Value)>) {
    if let Value::Object(map) = value {
        for (k, v) in map {
            let p = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            if v.is_object() {
                flatten(v, &p, out)
            } else {
                out.push((p, v.clone()))
            }
        }
    }
}

fn changes(current: &Value, baseline: &Value) -> Vec<(String, Value)> {
    let mut fields = Vec::new();
    flatten(current, "", &mut fields);
    let mut old = Vec::new();
    flatten(baseline, "", &mut old);
    for (path, _) in old {
        if schema_value(current, &path).is_none() {
            fields.push((path, Value::Null));
        }
    }
    fields.retain(|(path, value)| schema_value(baseline, path) != Some(value));
    fields
}

pub(crate) fn render(config: &Config) -> Result<String> {
    let current = serde_json::to_value(config)?;
    let source = config.source.lock().expect("config source mutex poisoned");
    let baseline = source.authored.as_ref().unwrap_or(&Value::Null);
    let changed = changes(&current, baseline);
    let mut changed_keys: std::collections::HashSet<_> =
        changed.iter().map(|(p, _)| key_for_path(p)).collect();
    let font_changed = schema_value(&current, "terminal.font_family")
        != schema_value(baseline, "terminal.font_family")
        || schema_value(&current, "terminal.font_fallback")
            != schema_value(baseline, "terminal.font_fallback");
    if font_changed {
        changed_keys.insert("font-family".to_owned());
    }
    if source.native.iter().any(|entry| entry.key == "config-file")
        && (font_changed
            || changed
                .iter()
                .any(|(path, _)| NATIVE.iter().any(|(_, native)| native == path)))
    {
        bail!(
            "cannot save native terminal setting while config-file includes are present; edit the included Ghostty file or remove config-file first"
        )
    }
    let mut out = String::new();
    for line in source.source.lines() {
        if parse_line(line).is_some_and(|(k, _)| changed_keys.contains(k)) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if source.source.is_empty() {
        out.push_str("# Con configuration\ncon.version = 1\n");
    }
    for (path, value) in changed {
        let key = key_for_path(&path);
        if path == "terminal.font_family" || path == "terminal.font_fallback" {
            continue;
        }
        match value {
            Value::Array(values) => {
                if values.is_empty() {
                    out.push_str(&format!("{key} = \n"));
                }
                for value in values {
                    out.push_str(&format!("{key} = {}\n", scalar_text(&value, &key)?));
                }
            }
            Value::Null => {}
            Value::Bool(value) if key == "clipboard-write" => out.push_str(&format!(
                "{key} = {}\n",
                if value { "allow" } else { "deny" }
            )),
            value => out.push_str(&format!("{key} = {}\n", scalar_text(&value, &key)?)),
        }
    }
    if font_changed {
        out.push_str("font-family = \n");
        for path in ["terminal.font_family", "terminal.font_fallback"] {
            if let Some(v) = schema_value(&current, path) {
                match v {
                    Value::Array(a) => {
                        for x in a {
                            out.push_str(&format!(
                                "font-family = {}\n",
                                scalar_text(x, "font-family")?
                            ));
                        }
                    }
                    x => out.push_str(&format!(
                        "font-family = {}\n",
                        scalar_text(x, "font-family")?
                    )),
                }
            }
        }
    }
    Ok(out)
}

pub(crate) fn native_text_from_str(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        if parse_line(line).is_some_and(|(key, _)| !key.starts_with("con."))
            || parse_line(line).is_none()
        {
            out.push_str(line);
        }
        // Keep source line numbers stable after removing Con-only settings.
        out.push('\n');
    }
    out
}

pub(crate) fn changed_keys(config: &Config) -> Result<Vec<String>> {
    let current = serde_json::to_value(config)?;
    let source = config.source.lock().expect("config source mutex poisoned");
    let baseline = source.authored.as_ref().unwrap_or(&Value::Null);
    let mut keys: Vec<_> = changes(&current, baseline)
        .into_iter()
        .filter(|(path, value)| schema_value(baseline, path) != Some(value))
        .map(|(path, _)| key_for_path(&path))
        .collect();
    keys.sort();
    keys.dedup();
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "con-config-test-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn native_and_namespaced_fields_parse_with_ordered_lists() {
        let source = "# keep\nfont-family = Fira Code\nfont-family = C:\\Fonts\\Emoji\nfont-size = 15.5\ncon.skills.project_paths = one\ncon.skills.project_paths = two\nunknown-native = yes\n";
        let config = parse(source, None).unwrap();
        assert_eq!(config.terminal.font_family, "Fira Code");
        assert_eq!(config.terminal.font_fallback, ["C:\\Fonts\\Emoji"]);
        assert_eq!(config.skills.project_paths, ["one", "two"]);
        assert!(
            config
                .native_entries()
                .iter()
                .any(|e| e.key == "unknown-native")
        );
    }

    #[test]
    fn unrelated_edit_retains_native_source_without_solidifying_defaults() {
        let mut config = parse("# authored\nunknown = value\ntheme = custom\n", None).unwrap();
        config.terminal.font_size = 18.0;
        let output = render(&config).unwrap();
        assert!(output.contains("# authored\nunknown = value\ntheme = custom\n"));
        assert!(output.contains("font-size = 18"));
        assert!(!output.contains("con.skills"));
    }

    #[test]
    fn validation_errors_do_not_echo_values() {
        assert!(
            parse("con.no_such_field = secret", None)
                .unwrap_err()
                .to_string()
                .contains("unknown Con key")
        );
        let error = parse("con.appearance.ui_opacity = definitely-secret", None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("expected finite number"));
        assert!(!error.contains("definitely-secret"));
        let error = parse("con.agent.provider = definitely-secret", None).unwrap_err();
        assert!(!format!("{error:#}").contains("definitely-secret"));
    }

    #[test]
    fn authored_provider_is_explicit_unless_marked_automatic() {
        assert!(
            parse("con.agent.provider = anthropic\n", None)
                .unwrap()
                .agent
                .provider_is_explicit
        );
        assert!(
            !parse(
                "con.agent.provider = anthropic\ncon.agent.provider_is_explicit = false\n",
                None
            )
            .unwrap()
            .agent
            .provider_is_explicit
        );
    }

    #[test]
    fn quoted_windows_paths_are_raw_ghostty_strings() {
        let config = parse(r#"command = "C:\tools\con.exe""#, None).unwrap();
        assert_eq!(config.terminal.shell.as_deref(), Some(r"C:\tools\con.exe"));
    }

    #[test]
    fn native_clipboard_policy_uses_allow_and_deny() {
        let mut config = parse("clipboard-write = deny\n", None).unwrap();
        assert!(!config.terminal.clipboard_write);
        config.terminal.clipboard_write = true;
        let text = render(&config).unwrap();
        assert!(text.contains("clipboard-write = allow"));
        assert!(!text.contains("clipboard-write = true"));
    }

    #[test]
    fn aliases_and_invalid_native_font_sizes_are_rejected() {
        assert!(parse("font-family = .SystemUIFont\n", None).is_err());
        assert!(parse("font-size = 0\n", None).is_err());
        assert!(parse("con.terminal.font_family = .ZedMono\n", None).is_err());
        assert!(parse("con.terminal.font_family = Courier\n", None).is_err());
        assert!(parse("font-size = 18\nfont-size =\n", None).is_ok());
    }

    #[test]
    fn default_render_parse_round_trip_is_equal() {
        let config = Config::default();
        let decoded = parse(&render(&config).unwrap(), None).unwrap();
        assert_eq!(
            serde_json::to_value(config).unwrap(),
            serde_json::to_value(decoded).unwrap()
        );
    }

    #[test]
    fn reset_empty_lists_round_trips() {
        let mut config = parse("con.skills.project_paths = old\n", None).unwrap();
        config.skills.project_paths.clear();
        let text = render(&config).unwrap();
        assert!(text.contains("con.skills.project_paths = \n"));
        assert!(parse(&text, None).unwrap().skills.project_paths.is_empty());
    }

    #[test]
    fn include_blocks_native_edits_and_preserves_file() {
        let path = temp_file("config");
        let fixture = "config-file = child\nfont-size = 12\n";
        fs::write(&path, fixture).unwrap();
        let mut config = Config::load_from_path(&path).unwrap();
        config.terminal.font_size = 13.0;
        assert!(
            config
                .save_to_path(&path)
                .unwrap_err()
                .to_string()
                .contains("config-file")
        );
        assert_eq!(fs::read_to_string(path).unwrap(), fixture);
    }

    #[test]
    fn malformed_render_is_detected_before_write() {
        let path = temp_file("config");
        fs::write(&path, "# original\n").unwrap();
        let mut config = Config::load_from_path(&path).unwrap();
        config.appearance.ui_font_family = "invalid\nvalue".into();
        assert!(config.save_to_path(&path).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "# original\n");
    }

    #[test]
    fn repeated_saves_use_the_latest_snapshot() {
        let path = temp_file("config");
        let mut config = Config::load_from_path(&path).unwrap();
        config.appearance.ui_opacity = 0.7;
        config.save_to_path(&path).unwrap();
        config.appearance.ui_opacity = 0.6;
        config.save_to_path(&path).unwrap();
        assert_eq!(
            Config::load_from_path(path).unwrap().appearance.ui_opacity,
            0.6
        );
    }

    #[test]
    fn cloned_drafts_have_independent_snapshots_and_conflict() {
        let path = temp_file("config");
        fs::write(&path, "con.appearance.ui_opacity = 0.8\n").unwrap();
        let original = Config::load_from_path(&path).unwrap();
        let mut first = original.clone();
        let mut second = original.clone();
        first.appearance.ui_opacity = 0.7;
        first.save_to_path(&path).unwrap();
        second.appearance.ui_opacity = 0.6;
        assert!(
            second
                .save_to_path(&path)
                .unwrap_err()
                .to_string()
                .contains("changed on disk")
        );
        assert_eq!(
            Config::load_from_path(path).unwrap().appearance.ui_opacity,
            0.7
        );
    }

    #[test]
    fn external_mutation_and_removal_are_not_overwritten() {
        let path = temp_file("config");
        fs::write(&path, "theme = one\n").unwrap();
        let mut changed = Config::load_from_path(&path).unwrap();
        changed.terminal.theme = "two".into();
        fs::write(&path, "theme = external\n").unwrap();
        assert!(changed.save_to_path(&path).is_err());
        let removed_path = temp_file("removed");
        fs::write(&removed_path, "theme = one\n").unwrap();
        let mut removed = Config::load_from_path(&removed_path).unwrap();
        removed.terminal.theme = "two".into();
        fs::remove_file(&removed_path).unwrap();
        assert!(removed.save_to_path(&removed_path).is_err());
    }

    #[test]
    fn native_text_reflects_unsaved_draft() {
        let mut config = parse("font-size = 12\ncon.appearance.ui_opacity = 0.8\n", None).unwrap();
        config.terminal.font_size = 19.0;
        let text = config.native_config_text().unwrap();
        assert!(text.contains("font-size = 19"));
        assert!(!text.contains("con.appearance"));
    }

    #[test]
    fn section_assignments_are_rejected_before_leaf_insertion() {
        for section in [
            "agent",
            "skills",
            "agent.providers",
            "agent.providers.anthropic",
        ] {
            let source = format!("con.{section} = invalid\ncon.agent.provider = anthropic\n");
            assert!(
                parse(&source, None)
                    .unwrap_err()
                    .to_string()
                    .contains("names a section")
            );
        }
    }

    #[test]
    fn quoted_numbers_list_resets_and_included_font_edits() {
        let config = parse("font-size = \"17\"\ncon.skills.project_paths = old\ncon.skills.project_paths =\ncon.skills.project_paths = new\n", None).unwrap();
        assert_eq!(config.terminal.font_size, 17.0);
        assert_eq!(config.skills.project_paths, ["new"]);
        let mut included = parse("config-file = child\n", None).unwrap();
        included.terminal.font_family = "Other Font".into();
        assert!(
            render(&included)
                .unwrap_err()
                .to_string()
                .contains("config-file")
        );
    }
}
