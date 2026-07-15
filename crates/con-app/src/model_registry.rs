//! Live model registry fetched from models.dev.
//!
//! Provides up-to-date model lists per provider by fetching from the
//! models.dev community API. Falls back to hardcoded defaults when
//! the network is unavailable.

use anyhow::{Context as _, anyhow};
use con_agent::ProviderKind;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const API_URL: &str = "https://models.dev/api.json";
const CHATGPT_CODEX_API_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const CACHE_TTL: Duration = Duration::from_secs(3600); // 1 hour

// ── Fallback model lists (used when API is unreachable) ──────────────

fn fallback_models(provider: &ProviderKind) -> &'static [&'static str] {
    match provider {
        ProviderKind::Anthropic => &[
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
        ProviderKind::ChatGPT => &[
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.4-nano",
            "gpt-5.3-codex-spark",
            "chat-latest",
        ],
        ProviderKind::GitHubCopilot => &[
            "gpt-5.4",
            "gpt-5.3-codex",
            "gpt-5.2",
            "claude-sonnet-4.5",
            "gpt-4o",
        ],
        ProviderKind::MiniMax | ProviderKind::MiniMaxAnthropic => {
            &["MiniMax-M2", "MiniMax-M2.1", "MiniMax-M2.5", "MiniMax-M2.7"]
        }
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
        "minimax" => &[ProviderKind::MiniMax, ProviderKind::MiniMaxAnthropic],
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

// ── Registry ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct CacheEntry {
    models: HashMap<ProviderKind, Vec<String>>,
    fetched_at: Instant,
}

/// Shared, thread-safe model registry with background fetching.
#[derive(Clone)]
pub struct ModelRegistry {
    inner: Arc<Mutex<Option<CacheEntry>>>,
    custom: Arc<Mutex<HashMap<(ProviderKind, String), Vec<String>>>>,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
            custom: Arc::new(Mutex::new(HashMap::new())),
        }
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
        let canonical = canonical_models_provider(provider);
        {
            let custom = self.custom.lock().unwrap();
            if let Some(endpoint) = base_url.and_then(Self::custom_models_scope) {
                if let Some(models) = custom.get(&(canonical.clone(), endpoint)) {
                    if !models.is_empty() {
                        let mut models = models.clone();
                        append_missing_models(&mut models, pinned_models(provider));
                        return models;
                    }
                }
            }
            if let Some(models) = custom.get(&(canonical.clone(), String::new())) {
                if !models.is_empty() {
                    let mut models = models.clone();
                    append_missing_models(&mut models, pinned_models(provider));
                    return models;
                }
            }
        }
        let guard = self.inner.lock().unwrap();
        if let Some(entry) = guard.as_ref() {
            if let Some(models) = entry.models.get(&canonical) {
                if !models.is_empty() {
                    let mut models = models.clone();
                    append_missing_models(&mut models, pinned_models(provider));
                    return models;
                }
            }
        }
        // Fallback
        let mut models: Vec<String> = fallback_models(provider)
            .iter()
            .map(|s| s.to_string())
            .collect();
        append_missing_models(&mut models, pinned_models(provider));
        models
    }

    /// Stores models discovered from a provider-managed endpoint.
    pub fn set_provider_models(&self, provider: ProviderKind, models: Vec<String>) {
        let canonical = canonical_models_provider(&provider);
        self.custom
            .lock()
            .unwrap()
            .insert((canonical, String::new()), models);
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
        log::info!("Fetching model registry from {API_URL}");

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;

        let resp: HashMap<String, ApiProvider> = client.get(API_URL).send().await?.json().await?;

        let mut merged_models: HashMap<ProviderKind, HashMap<String, (bool, u64)>> = HashMap::new();

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
                            existing.1 = existing.1.max(ctx);
                        })
                        .or_insert((model.tool_call, ctx));
                }
            }
        }

        let models_map: HashMap<ProviderKind, Vec<String>> = merged_models
            .into_iter()
            .map(|(kind, models)| {
                let mut model_list: Vec<(String, bool, u64)> = models
                    .into_iter()
                    .map(|(id, (tool_call, ctx))| (id, tool_call, ctx))
                    .collect();
                model_list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
                let ids = model_list.into_iter().map(|(id, _, _)| id).collect();
                (kind, ids)
            })
            .collect();

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

        let mut request = client.get(&endpoint);
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
        access_token: &str,
        base_url: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        let endpoint = Self::chatgpt_subscription_models_url(base_url)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;

        let resp = client
            .get(&endpoint)
            .bearer_auth(access_token)
            .send()
            .await?;
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
        let mut models = extract_model_ids(&raw);
        models.sort();
        models.dedup();
        Ok(models)
    }
}

fn extract_model_ids(raw: &serde_json::Value) -> Vec<String> {
    let entries = raw
        .get("data")
        .and_then(serde_json::Value::as_array)
        .or_else(|| raw.get("models").and_then(serde_json::Value::as_array))
        .into_iter()
        .flatten();

    entries
        .filter_map(|entry| match entry {
            serde_json::Value::String(value) => Some(value.as_str()),
            serde_json::Value::Object(object) => object
                .get("id")
                .or_else(|| object.get("slug"))
                .or_else(|| object.get("name"))
                .and_then(serde_json::Value::as_str),
            _ => None,
        })
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use con_agent::ProviderKind;

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
                "gpt-5.5".to_string(),
                "gpt-5.4".to_string(),
                "gpt-5.4-mini".to_string(),
                "gpt-5.4-nano".to_string(),
                "gpt-5.3-codex-spark".to_string(),
                "chat-latest".to_string(),
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
            "https://chatgpt.com/backend-api/codex/models"
        );
        assert_eq!(
            ModelRegistry::chatgpt_subscription_models_url(Some(
                "https://chatgpt.com/backend-api/codex"
            ))
            .unwrap(),
            "https://chatgpt.com/backend-api/codex/models"
        );
        assert_eq!(
            ModelRegistry::chatgpt_subscription_models_url(Some(
                "https://chatgpt.com/backend-api/codex/models"
            ))
            .unwrap(),
            "https://chatgpt.com/backend-api/codex/models"
        );
    }

    #[test]
    fn extract_model_ids_accepts_data_and_models_shapes() {
        assert_eq!(
            extract_model_ids(&serde_json::json!({
                "data": [
                    {"id": "gpt-5.5"},
                    {"slug": "gpt-5.4"},
                    {"name": "chat-latest"},
                    {"id": " "}
                ]
            })),
            vec![
                "gpt-5.5".to_string(),
                "gpt-5.4".to_string(),
                "chat-latest".to_string(),
            ]
        );
        assert_eq!(
            extract_model_ids(&serde_json::json!({
                "models": ["gpt-5.5", {"id": "gpt-5.4-mini"}]
            })),
            vec!["gpt-5.5".to_string(), "gpt-5.4-mini".to_string()]
        );
    }

    #[test]
    fn custom_models_override_fallback_for_openai_compatible() {
        let registry = ModelRegistry::new();
        registry.set_provider_models(
            ProviderKind::OpenAICompatible,
            vec!["custom-a".to_string(), "custom-b".to_string()],
        );
        assert_eq!(
            registry.models_for(&ProviderKind::OpenAICompatible),
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
