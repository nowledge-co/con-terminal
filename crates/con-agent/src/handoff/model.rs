//! Read-only candidate-model discovery for native handoff targets.
//!
//! Only agents with a confirmed side-effect-free list command are probed
//! (today: Cursor and Kimi). Codex reads only its local models cache. Other agents return an empty candidate
//! list so the UI degrades to exact-ID entry. A probe failure, timeout, or
//! missing authentication is never an error — it yields empty candidates.
//! Probes never start an interactive session, never write configuration,
//! and never dump raw provider configuration into the UI or logs. Kimi output is reduced to model keys only.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use tokio::{io::AsyncReadExt, process::Command};

use super::{AgentKind, TargetCapabilities};

#[path = "model/codex_cache.rs"]
mod codex_cache;

/// Mirrors con_core's `MAX_TARGET_MODEL_LEN`; candidates longer than this
/// could never be launched anyway.
const MAX_MODEL_ID_LEN: usize = 128;
/// Bound on candidates returned to the UI.
const MAX_CANDIDATES: usize = 64;
/// Bound on probe stdout; larger listings are treated as probe failures.
const MAX_OUTPUT_BYTES: u64 = 64 * 1024;
/// Cached candidates stay fresh for ten minutes, keyed by the exact target
/// and working directory.
const CACHE_TTL: Duration = Duration::from_secs(600);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

struct ProbeSpec {
    args: &'static [&'static str],
    timeout: Duration,
    parse: fn(&str) -> Vec<String>,
}

/// Only list commands verified to be read-only and side-effect-free qualify.
/// Kimi provider JSON stays private; only validated model keys leave the probe.
fn probe_spec(agent: AgentKind) -> Option<ProbeSpec> {
    match agent {
        AgentKind::Cursor => Some(ProbeSpec {
            args: &["models"],
            timeout: PROBE_TIMEOUT,
            parse: parse_cursor_models,
        }),
        AgentKind::Kimi => Some(ProbeSpec {
            args: &["provider", "list", "--json"],
            timeout: PROBE_TIMEOUT,
            parse: parse_kimi_model_aliases,
        }),
        _ => None,
    }
}

#[derive(Hash, PartialEq, Eq)]
struct CacheKey {
    agent: AgentKind,
    executable: PathBuf,
    version: String,
    cwd: PathBuf,
    codex_cache: Option<PathBuf>,
}

struct CacheEntry {
    at: Instant,
    models: Vec<String>,
}

fn cache() -> &'static Mutex<HashMap<CacheKey, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_get(key: &CacheKey) -> Option<Vec<String>> {
    let mut cache = cache().lock().ok()?;
    match cache.get(key) {
        Some(entry) if entry.at.elapsed() < CACHE_TTL => Some(entry.models.clone()),
        Some(_) => {
            cache.remove(key);
            None
        }
        None => None,
    }
}

fn cache_put(key: CacheKey, models: Vec<String>) {
    if let Ok(mut cache) = cache().lock() {
        cache.insert(
            key,
            CacheEntry {
                at: Instant::now(),
                models,
            },
        );
    }
}

/// Candidate model identifiers accepted by the target's `--model` flag on
/// this machine. Empty when the agent has no confirmed side-effect-free
/// list command or the probe fails, times out, or needs authentication —
/// callers must degrade to exact-ID entry without surfacing an error.
/// Background-callable: the probe is bounded by a short timeout, an output
/// byte cap, and a candidate count cap.
pub async fn candidate_models(target: &TargetCapabilities, cwd: &Path) -> Vec<String> {
    let key = CacheKey {
        agent: target.agent,
        executable: target.executable.clone(),
        version: target.version.clone(),
        cwd: cwd.to_owned(),
        codex_cache: (target.agent == AgentKind::Codex)
            .then(codex_cache::path)
            .flatten(),
    };
    if let Some(models) = cache_get(&key) {
        return models;
    }
    let models = if target.agent == AgentKind::Codex {
        match key.codex_cache.as_deref() {
            Some(path) => codex_cache::read(path).await,
            None => Vec::new(),
        }
    } else if let Some(spec) = probe_spec(target.agent) {
        run_probe(&spec, target, cwd).await.unwrap_or_default()
    } else {
        return Vec::new();
    };
    cache_put(key, models.clone());
    models
}

async fn run_probe(
    spec: &ProbeSpec,
    target: &TargetCapabilities,
    cwd: &Path,
) -> Result<Vec<String>> {
    let mut child = Command::new(&target.executable)
        .args(spec.args)
        .env_clear()
        .envs(super::environment::reader_environment(target.agent))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Start model listing")?;
    let mut stdout = child
        .stdout
        .take()
        .context("Missing model listing stdout")?
        .take(MAX_OUTPUT_BYTES + 1);
    let mut bytes = Vec::new();
    tokio::time::timeout(spec.timeout, async {
        stdout.read_to_end(&mut bytes).await?;
        ensure!(bytes.len() as u64 <= MAX_OUTPUT_BYTES, "Listing too large");
        ensure!(child.wait().await?.success(), "Model listing failed");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Model listing timed out")??;
    let text = String::from_utf8(bytes).context("Model listing is not text")?;
    Ok((spec.parse)(&text))
}

/// Keep only tokens that could be passed back verbatim as a `--model` value.
fn push_candidate(candidates: &mut Vec<String>, token: &str) {
    if token.is_empty()
        || token.len() > MAX_MODEL_ID_LEN
        || token.starts_with('-')
        || token.chars().any(|c| c.is_whitespace() || c.is_control())
        || candidates.iter().any(|seen| seen == token)
    {
        return;
    }
    if candidates.len() < MAX_CANDIDATES {
        candidates.push(token.to_owned());
    }
}

fn parse_kimi_model_aliases(text: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return candidates;
    };
    if let Some(models) = value.get("models").and_then(serde_json::Value::as_object) {
        for key in models.keys() {
            push_candidate(&mut candidates, key);
        }
    }
    candidates
}

/// Cursor `models` lists `id - Display Name` lines after a header.
fn parse_cursor_models(text: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some((id, _)) = line.split_once(" - ") {
            push_candidate(&mut candidates, id.trim());
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_candidates_expire_after_ten_minutes() {
        let key = CacheKey {
            agent: AgentKind::Codex,
            executable: "/unused-test".into(),
            version: "test".into(),
            cwd: "/test".into(),
            codex_cache: Some("/test/models_cache.json".into()),
        };
        cache().lock().unwrap().insert(
            key,
            CacheEntry {
                at: Instant::now(),
                models: vec!["model".into()],
            },
        );
        let mut cache = cache().lock().unwrap();
        let (key, entry) = cache
            .iter_mut()
            .find(|(key, _)| key.version == "test")
            .unwrap();
        let key = CacheKey {
            agent: key.agent,
            executable: key.executable.clone(),
            version: key.version.clone(),
            cwd: key.cwd.clone(),
            codex_cache: key.codex_cache.clone(),
        };
        entry.at = Instant::now() - CACHE_TTL;
        drop(cache);
        assert!(cache_get(&key).is_none());
    }

    #[test]
    fn kimi_listing_exposes_only_model_keys() {
        let raw = r#"{"providers":{"p":{"apiKey":"SECRET_KEY","oauth":"SECRET_OAUTH","endpoint":"SECRET_ENDPOINT"}},"models":{"kimi-code/k3":{"apiKey":"SECRET_MODEL"},"-bad":{},"has space":{}}}"#;
        let models = parse_kimi_model_aliases(raw);
        assert_eq!(models, ["kimi-code/k3"]);
        assert!(!format!("{models:?}").contains("SECRET"));
        assert!(parse_kimi_model_aliases("SECRET_INVALID_JSON").is_empty());
        // Neither successful output nor parse failures may introduce logging.
        let production = include_str!("model.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!production.contains("log::"));
        assert!(!production.contains("println!"));
    }

    #[test]
    fn cursor_listing_yields_verbatim_ids() {
        // Shape recorded from cursor-agent 2026.09.18 `models` output.
        let text = "Available models\n\nauto - Auto (default)\ngpt-5.3-codex-low - Codex 5.3 Low\ncomposer-2.5 - Composer 2.5\n";
        assert_eq!(
            parse_cursor_models(text),
            ["auto", "gpt-5.3-codex-low", "composer-2.5"]
        );
        assert!(parse_cursor_models("Login required").is_empty());
    }

    #[test]
    fn candidates_must_be_replayable_model_values() {
        let mut candidates = Vec::new();
        for token in [
            "",
            "-rf",
            "has space",
            "ctrl\u{7}",
            &"x".repeat(MAX_MODEL_ID_LEN + 1),
            "fine-model",
            "fine-model",
        ] {
            push_candidate(&mut candidates, token);
        }
        assert_eq!(candidates, ["fine-model"]);
    }

    #[test]
    fn only_confirmed_side_effect_free_agents_are_probed() {
        assert!(
            probe_spec(AgentKind::Codex).is_none(),
            "Codex must never spawn a model probe"
        );
        for agent in AgentKind::ALL {
            let probed = probe_spec(agent).is_some();
            assert_eq!(
                probed,
                matches!(agent, AgentKind::Cursor | AgentKind::Kimi),
                "{agent} probe support changed"
            );
        }
    }
}
