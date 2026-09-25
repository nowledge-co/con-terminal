//! Subscription-specific authentication and model capabilities, separate from the API catalog.
mod catalog;
#[cfg(test)]
mod tests;

pub use catalog::{Catalog, Model, ReasoningEffort, fallback_models, retired_model_replacement};

use anyhow::{Context, Result, anyhow};
use rig::providers::chatgpt;
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

use crate::provider::{
    ProviderConfig, ProviderKind, oauth_token_dir, stable_fingerprint, sync_codex_chatgpt_auth,
};

pub const DEFAULT_MODEL: &str = "gpt-5.6-sol";
pub const API_BASE: &str = "https://chatgpt.com/backend-api/codex";
// Verified with the 2026-09-21 subscription catalog fixture; 0.144.0 hides Astra.
pub const CATALOG_CLIENT_VERSION: &str = "0.155.0";

#[derive(Deserialize)]
struct Credentials {
    access_token: String,
    account_id: Option<String>,
}

/// Deliberately not Debug: the access token must never be logged.
pub struct AuthContext {
    pub access_token: String,
    pub account_id: Option<String>,
    pub scope: String,
}

fn token_override(config: &ProviderConfig) -> Option<String> {
    crate::provider::configured_provider_api_key_value(Some(config)).or_else(|| {
        std::env::var("CHATGPT_ACCESS_TOKEN")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn auth_file() -> Result<PathBuf> {
    oauth_token_dir(&ProviderKind::ChatGPT)
        .map(|dir| dir.join("auth.json"))
        .context("ChatGPT credential storage is unavailable")
}

fn read_context(path: &Path) -> Result<AuthContext> {
    let record: Credentials = serde_json::from_slice(&std::fs::read(path)?)?;
    if record.access_token.trim().is_empty() {
        return Err(anyhow!("Reconnect ChatGPT in Settings"));
    }
    let identity = record
        .account_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or(&record.access_token);
    Ok(AuthContext {
        scope: stable_fingerprint(identity.as_bytes()),
        access_token: record.access_token,
        account_id: record.account_id,
    })
}

fn cached_scope(config: &ProviderConfig) -> Option<String> {
    match token_override(config) {
        Some(token) => Some(format!("override-{}", stable_fingerprint(token.as_bytes()))),
        None => scope_for_file(&auth_file().ok()?),
    }
}

#[derive(PartialEq, Eq)]
struct CredentialStamp {
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    len: u64,
}

// Cache only the derived namespace, never credentials. Atomic token-file
// replacement invalidates it, including when an external Codex process writes.
type Scopes = HashMap<PathBuf, (CredentialStamp, Option<String>)>;
static SCOPES: OnceLock<Mutex<Scopes>> = OnceLock::new();

fn scope_for_file(path: &Path) -> Option<String> {
    let mut scopes = SCOPES.get_or_init(Default::default).lock().unwrap();
    let Ok(metadata) = path.metadata() else {
        scopes.remove(path);
        return None;
    };
    let stamp = CredentialStamp {
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        len: metadata.len(),
    };
    if let Some((cached_stamp, scope)) = scopes.get(path)
        && *cached_stamp == stamp
    {
        return scope.clone();
    }
    let scope = read_context(path).ok().map(|auth| auth.scope);
    scopes.insert(path.into(), (stamp, scope.clone()));
    scope
}

type Clients = HashMap<(PathBuf, String), chatgpt::Client>;
static CLIENTS: OnceLock<Mutex<Clients>> = OnceLock::new();

/// Clones share Rig's OAuth mutex, so chat and model discovery cannot rotate
/// the same refresh token concurrently for this credential file and endpoint.
fn oauth_client(path: &Path, base_url: Option<&str>) -> Result<chatgpt::Client> {
    let endpoint = base_url
        .unwrap_or(API_BASE)
        .trim_end_matches('/')
        .to_owned();
    let mut clients = CLIENTS.get_or_init(Default::default).lock().unwrap();
    let key = (path.to_path_buf(), endpoint.clone());
    if let Some(client) = clients.get(&key) {
        return Ok(client.clone());
    }
    let client = chatgpt::Client::builder()
        .oauth()
        .auth_file(path)
        .allow_device_flow(false)
        .base_url(endpoint)
        .build()?;
    clients.insert(key, client.clone());
    Ok(client)
}

pub(crate) fn client(config: &ProviderConfig) -> Result<chatgpt::Client> {
    if let Some(token) = token_override(config) {
        return Ok(chatgpt::Client::builder()
            .api_key(token)
            .base_url(config.base_url.as_deref().unwrap_or(API_BASE))
            .build()?);
    }
    let path = auth_file()?;
    if let Err(err) = sync_codex_chatgpt_auth(&path) {
        log::warn!("Failed to sync Codex ChatGPT credentials: {err}");
    }
    oauth_client(&path, config.base_url.as_deref())
}

pub async fn authorize_for_discovery(config: &ProviderConfig) -> Result<AuthContext> {
    if let Some(access_token) = token_override(config) {
        return Ok(AuthContext {
            scope: format!("override-{}", stable_fingerprint(access_token.as_bytes())),
            access_token,
            account_id: None,
        });
    }
    let client = client(config)?;
    client.authorize().await.context("Could not refresh ChatGPT credentials; reconnect ChatGPT in Settings if sign-in has expired")?;
    read_context(&auth_file()?)
}

pub fn request_parameters(
    config: &ProviderConfig,
    model: &str,
) -> Result<Option<serde_json::Value>> {
    if let Some(replacement) = retired_model_replacement(model, chrono::Utc::now().date_naive()) {
        anyhow::bail!(
            "ChatGPT subscription model {model} has retired. Select {replacement} in Settings."
        );
    }
    let Some(effort) = config.reasoning_effort else {
        return Ok(None);
    };
    let catalog = Catalog::load(config);
    let supported = catalog
        .as_ref()
        .and_then(|c| c.models.iter().find(|m| m.id == model))
        .or_else(|| fallback_models().iter().find(|m| m.id == model));
    if let Some(model) = supported {
        anyhow::ensure!(
            model.reasoning_efforts.contains(&effort),
            "Reasoning effort {} is not supported for {}. Choose Provider default or a supported effort in Settings.",
            effort.as_str(),
            model.id
        );
    }
    Ok(Some(serde_json::json!({"reasoning": {"effort": effort}})))
}
