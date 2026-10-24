//! Live model registry fetched from models.dev.
//!
//! Provides up-to-date model lists per provider by fetching from the
//! models.dev community API. Falls back to hardcoded defaults when
//! the network is unavailable.

use anyhow::{Context as _, anyhow};
use con_agent::chatgpt_subscription::{self, Catalog};
use con_agent::{ProviderConfig, ProviderKind};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const API_URL: &str = "https://models.dev/api.json";
const CHATGPT_CODEX_API_BASE_URL: &str = chatgpt_subscription::API_BASE;
// The Codex model catalog filters entries by client compatibility. Keep this
// explicit and validate schema/visibility against representative subscription data.
const CHATGPT_CODEX_MODELS_CLIENT_VERSION: &str = chatgpt_subscription::CATALOG_CLIENT_VERSION;
const CACHE_TTL: Duration = Duration::from_secs(3600); // 1 hour

// ── Fallback model lists (used when API is unreachable) ──────────────

fn fallback_models(provider: &ProviderKind) -> &'static [&'static str] {
    match provider {
        ProviderKind::Anthropic => &[
            "claude-sonnet-5-5",
            "claude-opus-5-5",
            "claude-haiku-5-5",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-opus-4-5",
            "claude-sonnet-4-5",
            "claude-haiku-4-5",
        ],
        ProviderKind::OpenAI => &[
            "gpt-5.5",
            "gpt-5.5-pro",
            "gpt-5.4",
            "gpt-5.4-pro",
            "gpt-5.4-mini",
            "gpt-5.4-nano",
            "o3",
            "o3-pro",
            "gpt-4.1",
            "gpt-4.1-mini",
        ],
        ProviderKind::ChatGPT => &[],
        ProviderKind::GitHubCopilot => &[
            "gpt-5.4",
            "gpt-5.3-codex",
            "gpt-5.2",
            "claude-sonnet-4.5",
            "gpt-4o",
        ],
        ProviderKind::MiniMax | ProviderKind::MiniMaxAnthropic => &[
            "MiniMax-M3.1-Flash",
            "MiniMax-M3",
            "MiniMax-M2.7",
            "MiniMax-M2.5",
        ],
        ProviderKind::Moonshot => &[
            "kimi-for-coding",
            "kimi-k2.5",
            "kimi-k2",
            "moonshot-v1-128k",
        ],
        ProviderKind::MoonshotAnthropic => &["kimi-k2.5", "kimi-k2", "moonshot-v1-128k"],
        ProviderKind::ZAI | ProviderKind::ZAIAnthropic => {
            &["glm-4.6", "glm-4.6-air", "glm-4.5", "glm-4.5v"]
        }
        ProviderKind::DeepSeek => &[
            "deepseek-v4-flash",
            "deepseek-v4-pro",
            "deepseek-chat",
            "deepseek-reasoner",
        ],
        ProviderKind::Groq => &[
            "llama-3.3-70b-versatile",
            "llama-3.1-8b-instant",
            "deepseek-r1-distill-llama-70b",
            "qwen-qwq-32b",
        ],
        ProviderKind::Gemini => &[
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "gemini-2.5-flash-lite",
            "gemini-2.0-flash",
        ],
        ProviderKind::Mistral => &[
            "mistral-large-latest",
            "mistral-medium-latest",
            "mistral-small-latest",
            "codestral-latest",
        ],
        ProviderKind::Cohere => &[
            "command-a-03-2025",
            "command-r-plus-08-2024",
            "command-r-08-2024",
        ],
        ProviderKind::Perplexity => &["sonar-pro", "sonar-reasoning-pro", "sonar"],
        ProviderKind::XAI => &["grok-4", "grok-3", "grok-3-mini", "grok-2"],
        ProviderKind::Ollama => &[
            "llama3.2",
            "llama3.1",
            "qwen2.5-coder",
            "deepseek-v3",
            "mistral",
            "gemma3",
        ],
        _ => &[],
    }
}

fn pinned_models(provider: &ProviderKind) -> &'static [&'static str] {
    match provider {
        ProviderKind::OpenAI => &[
            "gpt-5.5",
            "gpt-5.5-pro",
            "gpt-5.4",
            "gpt-5.4-pro",
            "gpt-5.4-mini",
            "gpt-5.4-nano",
            "o3",
            "o3-pro",
            "gpt-4.1",
            "gpt-4.1-mini",
        ],
        ProviderKind::Moonshot => &["kimi-for-coding"],
        ProviderKind::DeepSeek => &["deepseek-v4-flash", "deepseek-v4-pro"],
        _ => &[],
    }
}

fn append_missing_models(models: &mut Vec<String>, extra: &[&str]) {
    for model in extra {
        if !models.iter().any(|existing| existing == model) {
            models.push((*model).to_string());
        }
    }
}

/// Maps settings/runtime provider variants onto the canonical models.dev family.
fn canonical_models_provider(provider: &ProviderKind) -> ProviderKind {
    match provider {
        ProviderKind::MiniMaxAnthropic => ProviderKind::MiniMax,
        ProviderKind::MoonshotAnthropic => ProviderKind::Moonshot,
        ProviderKind::ZAIAnthropic => ProviderKind::ZAI,
        _ => provider.clone(),
    }
}

/// Maps models.dev provider IDs onto one or more Con provider families.
fn models_dev_id_to_providers(id: &str) -> &'static [ProviderKind] {
    match id {
        "anthropic" => &[ProviderKind::Anthropic],
        "openai" => &[ProviderKind::OpenAI],
        "chatgpt" => &[ProviderKind::ChatGPT],
        "github-copilot" => &[ProviderKind::GitHubCopilot],
        "minimax" | "minimax-cn" | "minimax-coding-plan" | "minimax-cn-coding-plan" => {
            &[ProviderKind::MiniMax, ProviderKind::MiniMaxAnthropic]
        }
        "moonshot" | "moonshotai" | "moonshotai-cn" => {
            &[ProviderKind::Moonshot, ProviderKind::MoonshotAnthropic]
        }
        "kimi-for-coding" => &[ProviderKind::Moonshot],
        "z-ai" | "zai" | "zai-coding-plan" => &[ProviderKind::ZAI, ProviderKind::ZAIAnthropic],
        "deepseek" => &[ProviderKind::DeepSeek],
        "groq" => &[ProviderKind::Groq],
        "google" => &[ProviderKind::Gemini],
        "cohere" => &[ProviderKind::Cohere],
        "ollama" | "ollama-cloud" => &[ProviderKind::Ollama],
        "openrouter" => &[ProviderKind::OpenRouter],
        "perplexity" => &[ProviderKind::Perplexity],
        "mistral" => &[ProviderKind::Mistral],
        "togetherai" => &[ProviderKind::Together],
        "xai" => &[ProviderKind::XAI],
        _ => &[],
    }
}

// ── API response types ───────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ApiProvider {
    models: Option<HashMap<String, ApiModel>>,
}

#[derive(serde::Deserialize)]
struct ApiModel {
    id: String,
    #[serde(default)]
    tool_call: bool,
    release_date: Option<String>,
    limit: Option<ApiLimit>,
}

#[derive(serde::Deserialize)]
struct ApiLimit {
    context: Option<u64>,
}

#[derive(serde::Deserialize)]
struct OpenAICompatibleModelsResponse {
    data: Vec<OpenAICompatibleModel>,
}

#[derive(serde::Deserialize)]
struct OpenAICompatibleModel {
    id: String,
}

fn catalog_models(resp: HashMap<String, ApiProvider>) -> HashMap<ProviderKind, Vec<String>> {
    let mut merged_models: HashMap<ProviderKind, HashMap<String, (bool, String, u64)>> =
        HashMap::new();

    for (provider_id, provider_data) in &resp {
        let targets = models_dev_id_to_providers(provider_id);
        if targets.is_empty() {
            continue;
        }

        let Some(models) = &provider_data.models else {
            continue;
        };

        for model in models.values() {
            let ctx = model
                .limit
                .as_ref()
                .and_then(|limit| limit.context)
                .unwrap_or(0);
            for target in targets {
                let entry = merged_models.entry(target.clone()).or_default();
                entry
                    .entry(model.id.clone())
                    .and_modify(|existing| {
                        existing.0 |= model.tool_call;
                        existing.1 = existing
                            .1
                            .clone()
                            .max(model.release_date.clone().unwrap_or_default());
                        existing.2 = existing.2.max(ctx);
                    })
                    .or_insert((
                        model.tool_call,
                        model.release_date.clone().unwrap_or_default(),
                        ctx,
                    ));
            }
        }
    }

    let models_map: HashMap<ProviderKind, Vec<String>> = merged_models
        .into_iter()
        .map(|(kind, models)| {
            let mut model_list: Vec<(String, bool, String, u64)> = models
                .into_iter()
                .map(|(id, (tool_call, date, ctx))| (id, tool_call, date, ctx))
                .collect();
            model_list.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| b.2.cmp(&a.2))
                    .then_with(|| b.3.cmp(&a.3))
                    .then_with(|| a.0.cmp(&b.0))
            });
            let ids = model_list.into_iter().map(|(id, _, _, _)| id).collect();
            (kind, ids)
        })
        .collect();

    models_map
}

// ── Registry ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct CacheEntry {
    models: HashMap<ProviderKind, Vec<String>>,
    fetched_at: Instant,
}

/// Per-provider custom model IDs keyed by `(provider, model id)`.
type CustomModelMap = HashMap<(ProviderKind, String), Vec<String>>;

/// Shared, thread-safe model registry with background fetching.
#[derive(Clone)]
pub struct ModelRegistry {
    inner: Arc<Mutex<Option<CacheEntry>>>,
    custom: Arc<Mutex<CustomModelMap>>,
    codex_local: Arc<Mutex<Vec<String>>>,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
            custom: Arc::new(Mutex::new(HashMap::new())),
            codex_local: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn models_for_config(
        &self,
        provider: &ProviderKind,
        config: &ProviderConfig,
    ) -> Vec<String> {
        if *provider == ProviderKind::ChatGPT {
            return Catalog::load(config)
                .map(|c| c.model_ids())
                .unwrap_or_else(|| {
                    chatgpt_subscription::fallback_models()
                        .iter()
                        .map(|m| m.id.clone())
                        .collect()
                });
        }
        self.models_for_base_url(provider, config.base_url.as_deref())
    }

    /// Returns the cached model list for a provider.
    /// Falls back to hardcoded defaults if the cache is empty.
    #[cfg(test)]
    pub fn models_for(&self, provider: &ProviderKind) -> Vec<String> {
        self.models_for_base_url(provider, None)
    }

    /// Returns the cached model list for a provider scoped to a concrete
    /// endpoint when one is available.
    pub fn models_for_base_url(
        &self,
        provider: &ProviderKind,
        base_url: Option<&str>,
    ) -> Vec<String> {
        if *provider == ProviderKind::CodexLocal {
            return self.codex_local.lock().unwrap().clone();
        }
        // Subscription availability and capabilities come from its dedicated
        // catalog. Keep this compatibility path on the same fallback source so
        // older callers cannot drift from `models_for_config`.
        if *provider == ProviderKind::ChatGPT {
            return chatgpt_subscription::fallback_models()
                .iter()
                .map(|model| model.id.clone())
                .collect();
        }
        let base_url = base_url
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .or_else(|| provider.default_base_url());
        let canonical = canonical_models_provider(provider);
        {
            let custom = self.custom.lock().unwrap();
            if let Some(endpoint) = base_url.and_then(Self::custom_models_scope)
                && let Some(models) = custom.get(&(canonical.clone(), endpoint))
                && !models.is_empty()
            {
                let mut models = models.clone();
                append_missing_models(&mut models, pinned_models(provider));
                return models;
            }
        }
        let guard = self.inner.lock().unwrap();
        if let Some(entry) = guard.as_ref()
            && let Some(models) = entry.models.get(&canonical)
            && !models.is_empty()
        {
            let mut models = models.clone();
            append_missing_models(&mut models, pinned_models(provider));
            return models;
        }
        // Fallback
        let mut models: Vec<String> = fallback_models(provider)
            .iter()
            .map(|s| s.to_string())
            .collect();
        append_missing_models(&mut models, pinned_models(provider));
        models
    }

    /// Local Codex availability is independent of the public API catalog.
    pub fn set_codex_local_models(&self, models: Vec<String>) {
        *self.codex_local.lock().unwrap() = models;
    }

    /// Stores models discovered from a specific user-configured endpoint.
    pub fn set_provider_models_for_base_url(
        &self,
        provider: ProviderKind,
        base_url: &str,
        models: Vec<String>,
    ) -> anyhow::Result<()> {
        let canonical = canonical_models_provider(&provider);
        let endpoint = Self::openai_compatible_models_url(base_url)?;
        self.custom
            .lock()
            .unwrap()
            .insert((canonical, endpoint), models);
        Ok(())
    }

    /// Only entries actually returned by the public catalog, without fallback IDs.
    pub fn public_catalog_models(&self, provider: &ProviderKind) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|entry| entry.models.get(&canonical_models_provider(provider)))
            .cloned()
            .unwrap_or_default()
    }

    /// Whether the cache is stale or empty and needs a refresh.
    pub fn needs_refresh(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        match guard.as_ref() {
            None => true,
            Some(entry) => entry.fetched_at.elapsed() > CACHE_TTL,
        }
    }

    /// Fetch model data from models.dev and update the cache.
    /// Intended to be called from a background task.
    pub async fn fetch(&self) -> anyhow::Result<()> {
        self.fetch_from_url(API_URL).await
    }

    async fn fetch_from_url(&self, endpoint: &str) -> anyhow::Result<()> {
        log::info!("Fetching model registry from {endpoint}");

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;

        let resp: HashMap<String, ApiProvider> = client
            .get(endpoint)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let models_map = catalog_models(resp);
        anyhow::ensure!(
            !models_map.is_empty(),
            "models.dev returned no recognized models; keeping the previous catalog"
        );

        let entry = CacheEntry {
            models: models_map,
            fetched_at: Instant::now(),
        };

        *self.inner.lock().unwrap() = Some(entry);
        log::info!("Model registry updated from models.dev");
        Ok(())
    }

    pub fn openai_compatible_models_url(base_url: &str) -> anyhow::Result<String> {
        let raw = base_url.trim();
        if raw.is_empty() {
            return Err(anyhow!("Base URL is required"));
        }

        let mut parsed = url::Url::parse(raw).context("Base URL must be an absolute URL")?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(anyhow!("Base URL must use http or https"));
        }

        let mut path = parsed.path().trim_end_matches('/').to_string();
        if path.ends_with("/chat/completions") {
            path.truncate(path.len() - "/chat/completions".len());
        }
        if !path.ends_with("/models") {
            if path.is_empty() || path == "/" {
                path = "/models".to_string();
            } else {
                path.push_str("/models");
            }
        }

        parsed.set_path(&path);
        parsed.set_fragment(None);
        Ok(parsed.to_string())
    }

    pub fn chatgpt_subscription_models_url(base_url: Option<&str>) -> anyhow::Result<String> {
        let raw = base_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(CHATGPT_CODEX_API_BASE_URL);
        let mut parsed = url::Url::parse(raw).context("Base URL must be an absolute URL")?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(anyhow!("Base URL must use http or https"));
        }

        let mut path = parsed.path().trim_end_matches('/').to_string();
        if !path.ends_with("/models") {
            if path.is_empty() || path == "/" {
                path = "/models".to_string();
            } else {
                path.push_str("/models");
            }
        }

        parsed.set_path(&path);
        parsed.set_fragment(None);
        let query_pairs = parsed
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        if !query_pairs
            .iter()
            .any(|(key, value)| key == "client_version" && !value.trim().is_empty())
        {
            parsed.set_query(None);
            parsed
                .query_pairs_mut()
                .extend_pairs(
                    query_pairs
                        .iter()
                        .filter(|(key, _)| key != "client_version"),
                )
                .append_pair("client_version", CHATGPT_CODEX_MODELS_CLIENT_VERSION);
        }
        Ok(parsed.to_string())
    }

    fn custom_models_scope(base_url: &str) -> Option<String> {
        Self::openai_compatible_models_url(base_url).ok()
    }

    /// Fetch a model list from an OpenAI-compatible endpoint.
    pub async fn fetch_openai_compatible_models(
        base_url: &str,
        api_key: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        let endpoint = Self::openai_compatible_models_url(base_url)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;

        let mut request = client
            .get(&endpoint)
            .header(reqwest::header::USER_AGENT, con_agent::magpie::USER_AGENT);
        if let Some(api_key) = api_key.map(str::trim).filter(|value| !value.is_empty()) {
            request = request.bearer_auth(api_key);
        }
        let resp = request.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let detail = body.trim();
            let detail = if detail.chars().count() > 180 {
                format!("{}…", detail.chars().take(180).collect::<String>())
            } else {
                detail.to_string()
            };
            return Err(anyhow!(
                "Model list request failed with HTTP {status}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            ));
        }

        let parsed: OpenAICompatibleModelsResponse = resp
            .json()
            .await
            .context("Response was not an OpenAI-compatible model list")?;
        let mut models: Vec<String> = parsed
            .data
            .into_iter()
            .map(|model| model.id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect();
        models.sort();
        models.dedup();
        Ok(models)
    }

    pub async fn fetch_chatgpt_subscription_models(
        config: &ProviderConfig,
    ) -> anyhow::Result<Vec<String>> {
        let auth = chatgpt_subscription::authorize_for_discovery(config).await?;
        let endpoint = Self::chatgpt_subscription_models_url(config.base_url.as_deref())?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;

        let mut request = client
            .get(&endpoint)
            .bearer_auth(&auth.access_token)
            .header("originator", "con")
            .header("User-Agent", concat!("con/", env!("CARGO_PKG_VERSION")));
        if let Some(account_id) = &auth.account_id {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        let resp = request.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let detail = body.trim();
            let detail = if detail.chars().count() > 180 {
                format!("{}…", detail.chars().take(180).collect::<String>())
            } else {
                detail.to_string()
            };
            return Err(anyhow!(
                "Model list request failed with HTTP {status}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            ));
        }

        let raw: serde_json::Value = resp
            .json()
            .await
            .context("Response was not a ChatGPT Subscription model list")?;
        let catalog = Catalog::parse(&raw, auth.scope, config.base_url.as_deref())?;
        catalog.save()?;
        Ok(catalog.model_ids())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use con_agent::ProviderKind;

    #[test]
    fn public_catalog_keeps_new_models_first_and_merges_regional_ids() {
        let response = serde_json::from_value(serde_json::json!({
            "anthropic":{"models":{
                "old":{"id":"claude-sonnet-4-6","tool_call":true,"release_date":"2026-02-17","limit":{"context":2000000}},
                "new":{"id":"claude-sonnet-5-5","tool_call":true,"release_date":"2026-09-28","limit":{"context":1000000}}
            }},
            "minimax-cn":{"models":{
                "new":{"id":"MiniMax-M3.1-Flash","tool_call":true,"release_date":"2026-10-19"}
            }}
        })).unwrap();
        let models = super::catalog_models(response);
        assert_eq!(
            models[&ProviderKind::Anthropic],
            vec!["claude-sonnet-5-5", "claude-sonnet-4-6"]
        );
        assert_eq!(models[&ProviderKind::MiniMax], vec!["MiniMax-M3.1-Flash"]);
        assert_eq!(
            models[&ProviderKind::MiniMaxAnthropic],
            models[&ProviderKind::MiniMax]
        );
    }

    #[test]
    fn codex_models_stay_separate_from_api_models_and_allow_a_default() {
        let registry = ModelRegistry::new();
        assert!(registry.models_for(&ProviderKind::CodexLocal).is_empty());
        let api_models = registry.models_for(&ProviderKind::OpenAI);
        registry.set_codex_local_models(vec!["local-only-model".into()]);
        assert_eq!(
            registry.models_for(&ProviderKind::CodexLocal),
            vec!["local-only-model"]
        );
        assert_eq!(registry.models_for(&ProviderKind::OpenAI), api_models);
        assert_eq!(
            registry.clone().models_for(&ProviderKind::CodexLocal),
            vec!["local-only-model"]
        );
    }

    #[test]
    fn anthropic_variants_share_canonical_models_family() {
        assert_eq!(
            canonical_models_provider(&ProviderKind::MiniMaxAnthropic),
            ProviderKind::MiniMax
        );
        assert_eq!(
            canonical_models_provider(&ProviderKind::MoonshotAnthropic),
            ProviderKind::Moonshot
        );
        assert_eq!(
            canonical_models_provider(&ProviderKind::ZAIAnthropic),
            ProviderKind::ZAI
        );
    }

    #[test]
    fn models_dev_aliases_cover_live_provider_ids() {
        assert_eq!(
            models_dev_id_to_providers("openai"),
            &[ProviderKind::OpenAI]
        );
        assert_eq!(
            models_dev_id_to_providers("chatgpt"),
            &[ProviderKind::ChatGPT]
        );
        assert_eq!(
            models_dev_id_to_providers("minimax"),
            &[ProviderKind::MiniMax, ProviderKind::MiniMaxAnthropic]
        );
        assert_eq!(
            models_dev_id_to_providers("moonshotai"),
            &[ProviderKind::Moonshot, ProviderKind::MoonshotAnthropic]
        );
        assert_eq!(
            models_dev_id_to_providers("kimi-for-coding"),
            &[ProviderKind::Moonshot]
        );
        assert_eq!(
            models_dev_id_to_providers("zai-coding-plan"),
            &[ProviderKind::ZAI, ProviderKind::ZAIAnthropic]
        );
        assert_eq!(
            models_dev_id_to_providers("deepseek"),
            &[ProviderKind::DeepSeek]
        );
    }

    #[test]
    fn chatgpt_subscription_fallback_is_curated_not_openai_api_catalog() {
        let registry = ModelRegistry::new();

        assert_eq!(
            registry.models_for(&ProviderKind::ChatGPT),
            vec![
                "gpt-6-astra".to_string(),
                "gpt-5.6-sol".to_string(),
                "gpt-5.6-terra".to_string(),
                "gpt-5.6-luna".to_string(),
            ]
        );
    }

    #[test]
    fn pinned_models_are_merged_into_live_cache() {
        let registry = ModelRegistry::new();
        let mut models = HashMap::new();
        models.insert(ProviderKind::OpenAI, vec!["gpt-5.5".to_string()]);
        models.insert(ProviderKind::Moonshot, vec!["kimi-k2.5".to_string()]);
        models.insert(ProviderKind::DeepSeek, vec!["deepseek-chat".to_string()]);
        *registry.inner.lock().unwrap() = Some(CacheEntry {
            models,
            fetched_at: Instant::now(),
        });

        assert_eq!(
            registry.models_for(&ProviderKind::OpenAI),
            vec![
                "gpt-5.5".to_string(),
                "gpt-5.5-pro".to_string(),
                "gpt-5.4".to_string(),
                "gpt-5.4-pro".to_string(),
                "gpt-5.4-mini".to_string(),
                "gpt-5.4-nano".to_string(),
                "o3".to_string(),
                "o3-pro".to_string(),
                "gpt-4.1".to_string(),
                "gpt-4.1-mini".to_string(),
            ]
        );
        assert_eq!(
            registry.models_for(&ProviderKind::Moonshot),
            vec!["kimi-k2.5".to_string(), "kimi-for-coding".to_string()]
        );
        assert_eq!(
            registry.models_for(&ProviderKind::DeepSeek),
            vec![
                "deepseek-chat".to_string(),
                "deepseek-v4-flash".to_string(),
                "deepseek-v4-pro".to_string(),
            ]
        );
    }

    #[test]
    fn openai_compatible_models_url_uses_models_endpoint() {
        assert_eq!(
            ModelRegistry::openai_compatible_models_url("https://api.example.com/v1").unwrap(),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            ModelRegistry::openai_compatible_models_url(
                "https://api.example.com/v1/chat/completions"
            )
            .unwrap(),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            ModelRegistry::openai_compatible_models_url("https://api.example.com/v1/models")
                .unwrap(),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            ModelRegistry::openai_compatible_models_url(
                "https://api.example.com/v1?api-version=2026-04-30"
            )
            .unwrap(),
            "https://api.example.com/v1/models?api-version=2026-04-30"
        );
        assert_eq!(
            ModelRegistry::openai_compatible_models_url(
                "https://api.example.com/v1/chat/completions?api-version=2026-04-30"
            )
            .unwrap(),
            "https://api.example.com/v1/models?api-version=2026-04-30"
        );
        assert!(ModelRegistry::openai_compatible_models_url("/chat/completions").is_err());
        assert!(ModelRegistry::openai_compatible_models_url("chat/completions").is_err());
    }

    #[test]
    fn chatgpt_subscription_models_url_uses_codex_models_endpoint() {
        assert_eq!(
            ModelRegistry::chatgpt_subscription_models_url(None).unwrap(),
            "https://chatgpt.com/backend-api/codex/models?client_version=0.155.0"
        );
        assert_eq!(
            ModelRegistry::chatgpt_subscription_models_url(Some(
                "https://chatgpt.com/backend-api/codex"
            ))
            .unwrap(),
            "https://chatgpt.com/backend-api/codex/models?client_version=0.155.0"
        );
        assert_eq!(
            ModelRegistry::chatgpt_subscription_models_url(Some(
                "https://chatgpt.com/backend-api/codex/models"
            ))
            .unwrap(),
            "https://chatgpt.com/backend-api/codex/models?client_version=0.155.0"
        );
        assert_eq!(
            ModelRegistry::chatgpt_subscription_models_url(Some(
                "https://chatgpt.com/backend-api/codex?region=us"
            ))
            .unwrap(),
            "https://chatgpt.com/backend-api/codex/models?region=us&client_version=0.155.0"
        );
        assert_eq!(
            ModelRegistry::chatgpt_subscription_models_url(Some(
                "https://chatgpt.com/backend-api/codex/models?client_version=0.200.0"
            ))
            .unwrap(),
            "https://chatgpt.com/backend-api/codex/models?client_version=0.200.0"
        );
        assert_eq!(
            ModelRegistry::chatgpt_subscription_models_url(Some(
                "https://chatgpt.com/backend-api/codex?region=us&client_version="
            ))
            .unwrap(),
            "https://chatgpt.com/backend-api/codex/models?region=us&client_version=0.155.0"
        );
    }

    #[test]
    fn magpie_catalog_uses_default_gateway_and_stays_isolated() {
        let registry = ModelRegistry::new();
        assert!(registry.models_for(&ProviderKind::Magpie).is_empty());
        registry
            .set_provider_models_for_base_url(
                ProviderKind::Magpie,
                con_agent::magpie::BASE_URL,
                vec!["deepseek/deepseek-chat".into(), "group/coding".into()],
            )
            .unwrap();
        assert_eq!(
            registry.models_for(&ProviderKind::Magpie),
            vec!["deepseek/deepseek-chat", "group/coding"]
        );
        assert!(
            registry
                .models_for_base_url(&ProviderKind::Magpie, Some("http://127.0.0.1:4444/v1"))
                .is_empty()
        );
        assert!(
            registry
                .models_for_base_url(
                    &ProviderKind::OpenAICompatible,
                    Some(con_agent::magpie::BASE_URL)
                )
                .is_empty()
        );
    }

    #[test]
    fn custom_models_override_fallback_for_openai_compatible() {
        let registry = ModelRegistry::new();
        registry
            .set_provider_models_for_base_url(
                ProviderKind::OpenAICompatible,
                "https://api.example.com/v1",
                vec!["custom-a".to_string(), "custom-b".to_string()],
            )
            .unwrap();
        assert_eq!(
            registry.models_for_base_url(
                &ProviderKind::OpenAICompatible,
                Some("https://api.example.com/v1"),
            ),
            vec!["custom-a".to_string(), "custom-b".to_string()]
        );
    }

    #[test]
    fn custom_openai_compatible_models_are_scoped_by_endpoint() {
        let registry = ModelRegistry::new();
        registry
            .set_provider_models_for_base_url(
                ProviderKind::OpenAICompatible,
                "https://one.example.com/v1",
                vec!["one-a".to_string()],
            )
            .unwrap();
        registry
            .set_provider_models_for_base_url(
                ProviderKind::OpenAICompatible,
                "https://two.example.com/v1/chat/completions",
                vec!["two-a".to_string()],
            )
            .unwrap();

        assert_eq!(
            registry.models_for_base_url(
                &ProviderKind::OpenAICompatible,
                Some("https://one.example.com/v1/models")
            ),
            vec!["one-a".to_string()]
        );
        assert_eq!(
            registry.models_for_base_url(
                &ProviderKind::OpenAICompatible,
                Some("https://two.example.com/v1")
            ),
            vec!["two-a".to_string()]
        );
    }

    #[test]
    fn openai_compatible_without_discovered_models_uses_manual_entry() {
        let registry = ModelRegistry::new();

        assert!(
            registry
                .models_for_base_url(
                    &ProviderKind::OpenAICompatible,
                    Some("https://api.example.com/v1")
                )
                .is_empty()
        );
    }
}
