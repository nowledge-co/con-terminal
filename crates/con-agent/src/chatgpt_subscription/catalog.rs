use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use super::{API_BASE, cached_scope};
use crate::provider::{ProviderConfig, stable_fingerprint, write_auth_record};

/// Wire values understood by the current Rig Responses adapter. Unknown future
/// catalog values are ignored rather than offered as unsupported settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(value.to_owned())).ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub display_name: Option<String>,
    pub reasoning_efforts: Vec<ReasoningEffort>,
    pub default_reasoning_effort: Option<ReasoningEffort>,
    pub context_window: Option<u64>,
    #[serde(default)]
    pub input_modalities: Vec<String>,
}

pub fn fallback_models() -> &'static [Model] {
    static MODELS: OnceLock<Vec<Model>> = OnceLock::new();
    MODELS.get_or_init(|| {
        [
            "gpt-6-astra",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
        ]
        .into_iter()
        .map(|id| Model {
            id: id.into(),
            display_name: None,
            reasoning_efforts: vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::Xhigh,
            ],
            default_reasoning_effort: None,
            context_window: None,
            input_modalities: Vec::new(),
        })
        .collect()
    })
}

pub fn retired_model_replacement(model: &str, today: NaiveDate) -> Option<&'static str> {
    match model {
        "gpt-5.4" if today >= NaiveDate::from_ymd_opt(2026, 8, 31).unwrap() => {
            Some("gpt-5.6-terra")
        }
        "gpt-5.4-mini" if today >= NaiveDate::from_ymd_opt(2026, 8, 31).unwrap() => {
            Some("gpt-5.6-luna")
        }
        "gpt-5.5" if today >= NaiveDate::from_ymd_opt(2026, 10, 14).unwrap() => Some("gpt-5.6-sol"),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    version: u8,
    scope: String,
    endpoint: String,
    pub fetched_at: i64,
    pub models: Vec<Model>,
}

// Model pickers render frequently. Read each account/endpoint cache once, and
// publish refreshed catalogs directly to memory after the atomic disk write.
static CATALOGS: OnceLock<Mutex<HashMap<PathBuf, Option<Catalog>>>> = OnceLock::new();

fn endpoint(base_url: Option<&str>) -> String {
    base_url
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(API_BASE)
        .trim_end_matches('/')
        .into()
}

impl Catalog {
    pub fn parse(raw: &serde_json::Value, scope: String, base_url: Option<&str>) -> Result<Self> {
        let entries = raw
            .get("models")
            .or_else(|| raw.get("data"))
            .and_then(serde_json::Value::as_array)
            .context("Response has no subscription model catalog")?;
        let mut models = Vec::new();
        for entry in entries {
            if entry.get("visibility").and_then(|v| v.as_str()) == Some("hide") {
                continue;
            }
            let Some(id) = entry.as_str().or_else(|| {
                ["slug", "id", "name"]
                    .iter()
                    .find_map(|key| entry.get(key).and_then(|v| v.as_str()))
            }) else {
                continue;
            };
            let id = id.trim();
            if id.is_empty() || models.iter().any(|m: &Model| m.id == id) {
                continue;
            }
            let reasoning_efforts = entry
                .get("supported_reasoning_levels")
                .and_then(|v| v.as_array())
                .map(|levels| {
                    levels
                        .iter()
                        .filter_map(|level| {
                            level
                                .as_str()
                                .or_else(|| level.get("effort").and_then(|v| v.as_str()))
                                .and_then(ReasoningEffort::parse)
                        })
                        .collect()
                })
                .unwrap_or_default();
            models.push(Model {
                id: id.into(),
                display_name: entry
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                reasoning_efforts,
                default_reasoning_effort: entry
                    .get("default_reasoning_level")
                    .and_then(|v| v.as_str())
                    .and_then(ReasoningEffort::parse),
                context_window: entry.get("context_window").and_then(|v| v.as_u64()),
                input_modalities: entry
                    .get("input_modalities")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default(),
            });
        }
        ensure!(
            !models.is_empty(),
            "Subscription catalog returned no visible models; keeping the last known catalog"
        );
        Ok(Self {
            version: 1,
            scope,
            endpoint: endpoint(base_url),
            fetched_at: chrono::Utc::now().timestamp(),
            models,
        })
    }

    pub fn model_ids(&self) -> Vec<String> {
        self.models.iter().map(|m| m.id.clone()).collect()
    }

    fn filename(scope: &str, endpoint: &str) -> String {
        let key = serde_json::to_vec(&(scope, endpoint)).unwrap();
        format!("{}.json", stable_fingerprint(&key))
    }

    pub fn save(&self) -> Result<()> {
        let dir = con_paths::app_cache_dir().join("chatgpt-models");
        self.save_in(&dir)?;
        let path = dir.join(Self::filename(&self.scope, &self.endpoint));
        CATALOGS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(path, Some(self.clone()));
        Ok(())
    }

    pub(super) fn save_in(&self, dir: &Path) -> Result<()> {
        let path = dir.join(Self::filename(&self.scope, &self.endpoint));
        write_auth_record(&path, &serde_json::to_vec(self)?)
    }

    pub fn load(config: &ProviderConfig) -> Option<Self> {
        let dir = con_paths::app_cache_dir().join("chatgpt-models");
        let scope = cached_scope(config)?;
        let path = dir.join(Self::filename(
            &scope,
            &endpoint(config.base_url.as_deref()),
        ));
        CATALOGS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .entry(path)
            .or_insert_with(|| Self::load_in(&dir, &scope, config.base_url.as_deref()))
            .clone()
    }

    pub(super) fn load_in(dir: &Path, scope: &str, base_url: Option<&str>) -> Option<Self> {
        let endpoint = endpoint(base_url);
        let path = dir.join(Self::filename(scope, &endpoint));
        let catalog: Self = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        (catalog.version == 1
            && catalog.scope == scope
            && catalog.endpoint == endpoint
            && !catalog.models.is_empty())
        .then_some(catalog)
    }
}
