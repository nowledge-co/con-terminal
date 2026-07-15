use anyhow::Result;
use crossbeam_channel::Sender;
use futures::StreamExt;
use http::{HeaderMap, HeaderValue};
use rig::agent::{MultiTurnStreamItem, StreamingResult};
use rig::client::CompletionClient;
use rig::client::Nothing;
use rig::http_client::{
    self, HttpClientExt, LazyBody, MultipartForm, Request, Response, StreamingResponse,
};
use rig::providers::{
    anthropic, chatgpt, cohere, copilot, deepseek, gemini, groq, minimax, mistral, moonshot,
    ollama, openai, openrouter, perplexity, together, xai, zai,
};
use rig::streaming::{StreamedAssistantContent, StreamingPrompt};
use rig::wasm_compat::WasmCompatSend;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use url::{Host, Url};

use crate::context::TerminalContext;
use crate::conversation::{AgentStep, Conversation, Message};
use crate::hook::{ConHook, ToolApprovalDecision};
use crate::tools::{
    AgentCliTurnTool, BatchExecTool, CreatePaneTool, EditFileTool, EnsureLocalAgentTargetTool,
    EnsureLocalCodingWorkspaceTool, EnsureLocalShellTargetTool, EnsureRemoteShellTargetTool,
    EnsureRemoteTmuxShellTargetTool, EnsureRemoteTmuxWorkspaceTool, FileReadTool, FileWriteTool,
    ListFilesTool, ListPanesTool, ListTabWorkspacesTool, PaneRequest, ProbeShellContextTool,
    ReadPaneTool, RemoteExecTool, ResolveWorkTargetTool, SearchPanesTool, SearchTool, SendKeysTool,
    ShellExecTool, TerminalExecRequest, TerminalExecTool, TmuxCaptureTool,
    TmuxEnsureAgentTargetTool, TmuxEnsureShellTargetTool, TmuxFindTargetsTool, TmuxInspectTool,
    TmuxListTool, TmuxRunCommandTool, TmuxSendKeysTool, TmuxShellTurnTool, WaitForTool,
};

const KIMI_CODING_BASE_URL: &str = "https://api.kimi.com/coding/v1";
const KIMI_CODING_USER_AGENT: &str = "KimiCLI/1.35.0";

#[derive(Clone, Debug)]
struct OpenAICompatibleHttpClient {
    inner: http_client::ReqwestClient,
    strip_authorization: bool,
}

impl OpenAICompatibleHttpClient {
    fn new(strip_authorization: bool) -> Self {
        Self {
            inner: http_client::ReqwestClient::default(),
            strip_authorization,
        }
    }

    fn strip_authorization<T>(&self, req: &mut Request<T>) {
        if self.strip_authorization {
            req.headers_mut().remove(http::header::AUTHORIZATION);
        }
    }
}

impl Default for OpenAICompatibleHttpClient {
    fn default() -> Self {
        Self::new(false)
    }
}

impl HttpClientExt for OpenAICompatibleHttpClient {
    fn send<T, U>(
        &self,
        mut req: Request<T>,
    ) -> impl Future<Output = http_client::Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        T: Into<bytes::Bytes>,
        T: WasmCompatSend,
        U: From<bytes::Bytes>,
        U: WasmCompatSend + 'static,
    {
        self.strip_authorization(&mut req);
        <http_client::ReqwestClient as HttpClientExt>::send::<T, U>(&self.inner, req)
    }

    fn send_multipart<U>(
        &self,
        mut req: Request<MultipartForm>,
    ) -> impl Future<Output = http_client::Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        U: From<bytes::Bytes>,
        U: WasmCompatSend + 'static,
    {
        self.strip_authorization(&mut req);
        <http_client::ReqwestClient as HttpClientExt>::send_multipart::<U>(&self.inner, req)
    }

    fn send_streaming<T>(
        &self,
        mut req: Request<T>,
    ) -> impl Future<Output = http_client::Result<StreamingResponse>> + WasmCompatSend
    where
        T: Into<bytes::Bytes> + WasmCompatSend,
    {
        self.strip_authorization(&mut req);
        <http_client::ReqwestClient as HttpClientExt>::send_streaming::<T>(&self.inner, req)
    }
}

fn is_local_openai_compatible_base_url(base_url: &str) -> bool {
    let Ok(url) = Url::parse(base_url) else {
        return false;
    };

    match url.host() {
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            matches!(domain.as_str(), "localhost" | "host.docker.internal")
                || domain.ends_with(".local")
        }
        Some(Host::Ipv4(addr)) => addr.is_loopback() || addr.is_private() || addr.is_link_local(),
        Some(Host::Ipv6(addr)) => {
            addr.is_loopback() || addr.is_unique_local() || addr.is_unicast_link_local()
        }
        None => false,
    }
}

// ── Provider enum ───────────────────────────────────────────────────

/// Supported LLM providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    OpenAI,
    #[serde(rename = "chatgpt")]
    ChatGPT,
    #[serde(rename = "github-copilot", alias = "githubcopilot")]
    GitHubCopilot,
    #[serde(alias = "openai-compatible")]
    OpenAICompatible,
    #[serde(rename = "minimax")]
    MiniMax,
    #[serde(rename = "minimax-anthropic")]
    MiniMaxAnthropic,
    #[serde(rename = "moonshot")]
    Moonshot,
    #[serde(rename = "moonshot-anthropic")]
    MoonshotAnthropic,
    #[serde(rename = "z-ai", alias = "zai")]
    ZAI,
    #[serde(rename = "z-ai-anthropic", alias = "zai-anthropic")]
    ZAIAnthropic,
    DeepSeek,
    Groq,
    Cohere,
    Gemini,
    Ollama,
    OpenRouter,
    Perplexity,
    Mistral,
    Together,
    XAI,
}

impl Default for ProviderKind {
    fn default() -> Self {
        Self::Anthropic
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anthropic => write!(f, "anthropic"),
            Self::OpenAI => write!(f, "openai"),
            Self::ChatGPT => write!(f, "chatgpt"),
            Self::GitHubCopilot => write!(f, "github-copilot"),
            Self::OpenAICompatible => write!(f, "openai-compatible"),
            Self::MiniMax => write!(f, "minimax"),
            Self::MiniMaxAnthropic => write!(f, "minimax-anthropic"),
            Self::Moonshot => write!(f, "moonshot"),
            Self::MoonshotAnthropic => write!(f, "moonshot-anthropic"),
            Self::ZAI => write!(f, "z-ai"),
            Self::ZAIAnthropic => write!(f, "z-ai-anthropic"),
            Self::DeepSeek => write!(f, "deepseek"),
            Self::Groq => write!(f, "groq"),
            Self::Cohere => write!(f, "cohere"),
            Self::Gemini => write!(f, "gemini"),
            Self::Ollama => write!(f, "ollama"),
            Self::OpenRouter => write!(f, "openrouter"),
            Self::Perplexity => write!(f, "perplexity"),
            Self::Mistral => write!(f, "mistral"),
            Self::Together => write!(f, "together"),
            Self::XAI => write!(f, "xai"),
        }
    }
}

fn display_provider_model(kind: &ProviderKind, model: &str) -> String {
    let provider = match kind {
        ProviderKind::ChatGPT => "ChatGPT",
        ProviderKind::OpenAI => "OpenAI",
        ProviderKind::GitHubCopilot => "GitHub Copilot",
        ProviderKind::OpenAICompatible => "OpenAI-compatible",
        ProviderKind::MiniMax => "MiniMax",
        ProviderKind::MiniMaxAnthropic => "MiniMax Anthropic",
        ProviderKind::Moonshot => "Moonshot",
        ProviderKind::MoonshotAnthropic => "Moonshot Anthropic",
        ProviderKind::ZAI => "Z-AI",
        ProviderKind::ZAIAnthropic => "Z-AI Anthropic",
        ProviderKind::DeepSeek => "DeepSeek",
        ProviderKind::Groq => "Groq",
        ProviderKind::Cohere => "Cohere",
        ProviderKind::Gemini => "Gemini",
        ProviderKind::Ollama => "Ollama",
        ProviderKind::OpenRouter => "OpenRouter",
        ProviderKind::Perplexity => "Perplexity",
        ProviderKind::Mistral => "Mistral",
        ProviderKind::Together => "Together",
        ProviderKind::XAI => "xAI",
        ProviderKind::Anthropic => "Anthropic",
    };
    format!("{provider} · {model}")
}

fn truncate_utf8_for_log(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }

    let end = text
        .char_indices()
        .map(|(idx, _)| idx)
        .take_while(|idx| *idx <= max_bytes)
        .last()
        .unwrap_or(0);
    format!("{}...", &text[..end])
}

const THINK_OPEN_TAG: &str = "<think>";
const THINK_CLOSE_TAG: &str = "</think>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkParseMode {
    Normal,
    InThink,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ThinkParseOutput {
    visible: String,
    reasoning: String,
}

#[derive(Debug, Default)]
struct ThinkTagStreamParser {
    pending: String,
    mode: ThinkParseMode,
}

impl Default for ThinkParseMode {
    fn default() -> Self {
        Self::Normal
    }
}

impl ThinkTagStreamParser {
    fn push(&mut self, chunk: &str) -> ThinkParseOutput {
        self.pending.push_str(chunk);
        self.drain(false)
    }

    fn finish(&mut self) -> ThinkParseOutput {
        self.drain(true)
    }

    fn drain(&mut self, flush_all: bool) -> ThinkParseOutput {
        let mut out = ThinkParseOutput::default();

        loop {
            match self.mode {
                ThinkParseMode::Normal => {
                    if let Some(idx) = self.pending.find(THINK_OPEN_TAG) {
                        out.visible.push_str(&self.pending[..idx]);
                        self.pending.drain(..idx + THINK_OPEN_TAG.len());
                        self.mode = ThinkParseMode::InThink;
                        continue;
                    }

                    let keep = if flush_all {
                        0
                    } else {
                        partial_suffix_len(&self.pending, THINK_OPEN_TAG)
                    };
                    let flush_len = self.pending.len().saturating_sub(keep);
                    if flush_len > 0 {
                        out.visible.push_str(&self.pending[..flush_len]);
                        self.pending.drain(..flush_len);
                    }
                    break;
                }
                ThinkParseMode::InThink => {
                    if let Some(idx) = self.pending.find(THINK_CLOSE_TAG) {
                        out.reasoning.push_str(&self.pending[..idx]);
                        self.pending.drain(..idx + THINK_CLOSE_TAG.len());
                        self.mode = ThinkParseMode::Normal;
                        continue;
                    }

                    let keep = if flush_all {
                        0
                    } else {
                        partial_suffix_len(&self.pending, THINK_CLOSE_TAG)
                    };
                    let flush_len = self.pending.len().saturating_sub(keep);
                    if flush_len > 0 {
                        out.reasoning.push_str(&self.pending[..flush_len]);
                        self.pending.drain(..flush_len);
                    }
                    break;
                }
            }
        }

        out
    }
}

fn partial_suffix_len(text: &str, marker: &str) -> usize {
    let max_len = text.len().min(marker.len().saturating_sub(1));
    for len in (1..=max_len).rev() {
        if text.ends_with(&marker[..len]) {
            return len;
        }
    }
    0
}

fn apply_stream_text_chunk(
    parser: &mut ThinkTagStreamParser,
    response_text: &mut String,
    event_tx: &Sender<AgentEvent>,
    chunk: &str,
    allow_thinking_emit: bool,
) -> bool {
    let parsed = parser.push(chunk);
    if !parsed.visible.is_empty() {
        response_text.push_str(&parsed.visible);
    }
    if allow_thinking_emit && !parsed.reasoning.is_empty() {
        let _ = event_tx.send(AgentEvent::ThinkingDelta(parsed.reasoning));
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rig_budget_preserves_the_previous_model_call_ceiling() {
        assert_eq!(rig_model_call_budget(0), 2);
        assert_eq!(rig_model_call_budget(1), 3);
        assert_eq!(rig_model_call_budget(30), 32);
        assert_eq!(rig_model_call_budget(usize::MAX), usize::MAX);
    }

    #[test]
    fn codex_chatgpt_auth_converter_flattens_codex_token_shape() {
        let auth = serde_json::from_value::<CodexAuthFile>(serde_json::json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": "access",
                "refresh_token": "refresh",
                "id_token": "id",
                "account_id": "account"
            }
        }))
        .expect("codex auth shape should parse");

        assert_eq!(
            convert_codex_chatgpt_auth(auth),
            Some(RigChatGptAuthRecord {
                access_token: "access".into(),
                refresh_token: Some("refresh".into()),
                id_token: Some("id".into()),
                account_id: Some("account".into()),
            })
        );
    }

    #[test]
    fn chatgpt_oauth_cache_accepts_unexpired_access_token() {
        let record = RigChatGptAuthTokenView {
            access_token: Some("access".into()),
            refresh_token: None,
            expires_at: Some(1_061),
        };

        assert!(chatgpt_oauth_record_ready_at(record, 1_000));
    }

    #[test]
    fn chatgpt_oauth_cache_accepts_refreshable_record() {
        let record = RigChatGptAuthTokenView {
            access_token: None,
            refresh_token: Some("refresh".into()),
            expires_at: None,
        };

        assert!(chatgpt_oauth_record_ready_at(record, 1_000));
    }

    #[test]
    fn chatgpt_oauth_cache_rejects_access_token_without_valid_expiry() {
        let missing_expiry = RigChatGptAuthTokenView {
            access_token: Some("access".into()),
            refresh_token: None,
            expires_at: None,
        };
        let inside_expiry_skew = RigChatGptAuthTokenView {
            access_token: Some("access".into()),
            refresh_token: None,
            expires_at: Some(1_060),
        };

        assert!(!chatgpt_oauth_record_ready_at(missing_expiry, 1_000));
        assert!(!chatgpt_oauth_record_ready_at(inside_expiry_skew, 1_000));
    }

    #[test]
    fn chatgpt_oauth_cache_rejects_empty_credentials() {
        let record = RigChatGptAuthTokenView {
            access_token: Some("  ".into()),
            refresh_token: Some(String::new()),
            expires_at: Some(i64::MAX),
        };

        assert!(!chatgpt_oauth_record_ready_at(record, 1_000));
    }

    #[test]
    fn preferred_default_keeps_explicitly_selected_provider() {
        let config = AgentConfig {
            provider: ProviderKind::OpenAI,
            ..AgentConfig::default()
        };
        assert_eq!(
            preferred_default_provider_for_state(&config, true, false, true),
            None
        );
    }

    #[test]
    fn preferred_default_keeps_explicit_anthropic_without_credentials() {
        let config = AgentConfig::default();

        assert_eq!(
            preferred_default_provider_for_state(&config, true, false, true),
            None
        );
    }

    #[test]
    fn preferred_default_uses_chatgpt_for_implicit_unconfigured_anthropic() {
        let config = AgentConfig::default();

        assert_eq!(
            preferred_default_provider_for_state(&config, false, false, true),
            Some(ProviderKind::ChatGPT)
        );
        assert_eq!(
            preferred_default_provider_for_state(&config, false, false, false),
            None
        );
    }

    #[test]
    fn preferred_default_recomputes_automatic_chatgpt_selection() {
        let config = AgentConfig {
            provider: ProviderKind::ChatGPT,
            ..AgentConfig::default()
        };

        assert_eq!(
            preferred_default_provider_for_state(&config, false, true, true),
            Some(ProviderKind::Anthropic)
        );
        assert_eq!(
            preferred_default_provider_for_state(&config, false, false, false),
            Some(ProviderKind::Anthropic)
        );
    }

    #[test]
    fn preferred_default_keeps_anthropic_when_inline_key_configured() {
        // Anthropic with a usable inline key is a real setup — don't redirect to
        // ChatGPT even if a ChatGPT cache happens to exist.
        let mut config = AgentConfig {
            provider: ProviderKind::Anthropic,
            ..AgentConfig::default()
        };
        config.providers.set(
            &ProviderKind::Anthropic,
            ProviderConfig {
                model: None,
                api_key: Some("sk-ant-test".into()),
                api_key_env: None,
                base_url: None,
                max_tokens: None,
            },
        );
        assert!(anthropic_credentials_available(&config));
        assert_eq!(
            preferred_default_provider_for_state(&config, false, true, true),
            None
        );
    }

    #[test]
    fn preferred_default_keeps_anthropic_when_legacy_api_key_env_contains_key() {
        let mut config = AgentConfig {
            provider: ProviderKind::Anthropic,
            ..AgentConfig::default()
        };
        config.providers.set(
            &ProviderKind::Anthropic,
            ProviderConfig {
                model: None,
                api_key: None,
                api_key_env: Some("sk-ant-legacy-direct-key".into()),
                base_url: None,
                max_tokens: None,
            },
        );
        assert!(anthropic_credentials_available(&config));
        assert_eq!(
            preferred_default_provider_for_state(&config, false, true, true),
            None
        );
    }

    #[test]
    fn preferred_default_keeps_automatic_chatgpt_with_key_override() {
        let mut config = AgentConfig {
            provider: ProviderKind::ChatGPT,
            ..AgentConfig::default()
        };
        config.providers.set(
            &ProviderKind::ChatGPT,
            ProviderConfig {
                api_key: Some("chatgpt-access-token".into()),
                ..ProviderConfig::default()
            },
        );

        assert!(provider_credentials_available(
            &config,
            &ProviderKind::ChatGPT
        ));
        assert_eq!(
            preferred_default_provider_for_state(&config, false, false, true),
            None
        );
    }

    #[test]
    fn codex_chatgpt_auth_converter_ignores_non_chatgpt_auth() {
        let auth = serde_json::from_value::<CodexAuthFile>(serde_json::json!({
            "auth_mode": "api-key",
            "tokens": {
                "access_token": "access",
                "refresh_token": "refresh"
            }
        }))
        .expect("codex auth shape should parse");

        assert_eq!(convert_codex_chatgpt_auth(auth), None);
    }

    #[test]
    fn codex_chatgpt_auth_converter_requires_refresh_token() {
        let auth = serde_json::from_value::<CodexAuthFile>(serde_json::json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": "access",
                "id_token": "id",
                "account_id": "account"
            }
        }))
        .expect("codex auth shape should parse");

        assert_eq!(convert_codex_chatgpt_auth(auth), None);
    }

    #[test]
    fn codex_chatgpt_auth_sync_writes_rig_cache_without_clobbering_seen_source() {
        let root = std::env::temp_dir().join(format!(
            "con-agent-codex-auth-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root should be created");
        let source = root.join("codex-auth.json");
        let target = root.join("con").join("auth.json");
        std::fs::write(
            &source,
            serde_json::to_vec(&serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "access",
                    "refresh_token": "refresh",
                    "id_token": "id",
                    "account_id": "account"
                }
            }))
            .expect("source json should serialize"),
        )
        .expect("source auth should be written");

        assert!(
            sync_codex_chatgpt_auth_from_file(&source, &target)
                .expect("codex auth import should succeed")
        );

        let imported = std::fs::read_to_string(&target).expect("target auth should exist");
        let imported: serde_json::Value =
            serde_json::from_str(&imported).expect("target auth should be valid json");
        assert_eq!(imported["access_token"], "access");
        assert_eq!(imported["refresh_token"], "refresh");
        assert_eq!(imported["id_token"], "id");
        assert_eq!(imported["account_id"], "account");

        std::fs::write(&target, r#"{"access_token":"existing"}"#)
            .expect("target auth should be overwritten by test setup");
        assert!(
            !sync_codex_chatgpt_auth_from_file(&source, &target)
                .expect("existing target should be left alone")
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("target auth should still exist"),
            r#"{"access_token":"existing"}"#
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_chatgpt_auth_sync_updates_when_codex_source_fingerprint_changes() {
        let root = std::env::temp_dir().join(format!(
            "con-agent-codex-auth-update-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root should be created");
        let source = root.join("codex-auth.json");
        let target = root.join("con").join("auth.json");

        std::fs::write(
            &source,
            serde_json::to_vec(&serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "codex-v1",
                    "refresh_token": "refresh-v1"
                }
            }))
            .expect("source json should serialize"),
        )
        .expect("source auth should be written");

        assert!(
            sync_codex_chatgpt_auth_from_file(&source, &target)
                .expect("initial codex auth sync should succeed")
        );

        std::fs::write(
            &source,
            serde_json::to_vec(&serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "codex-v2",
                    "refresh_token": "refresh-v2"
                }
            }))
            .expect("updated source json should serialize"),
        )
        .expect("source auth should be updated");

        assert!(
            sync_codex_chatgpt_auth_from_file(&source, &target)
                .expect("managed codex auth should update")
        );
        let imported = std::fs::read_to_string(&target).expect("target auth should exist");
        let imported: serde_json::Value =
            serde_json::from_str(&imported).expect("target auth should be valid json");
        assert_eq!(imported["access_token"], "codex-v2");
        assert_eq!(imported["refresh_token"], "refresh-v2");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_chatgpt_auth_sync_preserves_preexisting_unmanaged_auth() {
        let root = std::env::temp_dir().join(format!(
            "con-agent-codex-auth-unmanaged-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root should be created");
        let source = root.join("codex-auth.json");
        let target = root.join("con").join("auth.json");
        std::fs::create_dir_all(target.parent().expect("target should have a parent"))
            .expect("target parent should be created");
        std::fs::write(&target, r#"{"access_token":"manual"}"#)
            .expect("manual auth should be written");
        std::fs::write(
            &source,
            serde_json::to_vec(&serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "codex",
                    "refresh_token": "refresh"
                }
            }))
            .expect("source json should serialize"),
        )
        .expect("source auth should be written");

        assert!(
            !sync_codex_chatgpt_auth_from_file(&source, &target)
                .expect("unmanaged auth should be preserved")
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("target auth should still exist"),
            r#"{"access_token":"manual"}"#
        );
        assert!(!codex_auth_sync_state_file(&target).exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_chatgpt_auth_sync_retires_ownership_after_local_auth_changes() {
        let root = std::env::temp_dir().join(format!(
            "con-agent-codex-auth-local-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root should be created");
        let source = root.join("codex-auth.json");
        let target = root.join("con").join("auth.json");

        std::fs::write(
            &source,
            serde_json::to_vec(&serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "codex-v1",
                    "refresh_token": "refresh-v1"
                }
            }))
            .expect("source json should serialize"),
        )
        .expect("source auth should be written");

        assert!(
            sync_codex_chatgpt_auth_from_file(&source, &target)
                .expect("initial codex auth sync should succeed")
        );

        std::fs::write(&target, r#"{"access_token":"manual"}"#)
            .expect("manual auth should be written");
        assert!(
            !sync_codex_chatgpt_auth_from_file(&source, &target)
                .expect("same codex source should remain seen")
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("target auth should still exist"),
            r#"{"access_token":"manual"}"#
        );
        assert!(!codex_auth_sync_state_file(&target).exists());

        std::fs::write(
            &source,
            serde_json::to_vec(&serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "codex-v2",
                    "refresh_token": "refresh-v2"
                }
            }))
            .expect("source json should serialize"),
        )
        .expect("source auth should be updated");

        assert!(
            !sync_codex_chatgpt_auth_from_file(&source, &target)
                .expect("updated codex auth should not replace local auth")
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("target auth should still exist"),
            r#"{"access_token":"manual"}"#
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migrate_legacy_infers_anthropic_transport_preferences() {
        let mut config = AgentConfig {
            provider: ProviderKind::Anthropic,
            ..AgentConfig::default()
        };
        config.providers.set(
            &ProviderKind::MiniMaxAnthropic,
            ProviderConfig {
                model: Some("MiniMax-M2.7".into()),
                api_key: None,
                api_key_env: None,
                base_url: Some("https://api.minimaxi.com/anthropic".into()),
                max_tokens: None,
            },
        );
        config.providers.set(
            &ProviderKind::ZAIAnthropic,
            ProviderConfig {
                model: Some("glm-4.6".into()),
                api_key: None,
                api_key_env: None,
                base_url: Some("https://api.z.ai/api/anthropic".into()),
                max_tokens: None,
            },
        );

        config.migrate_legacy();

        assert_eq!(
            config.provider_transport_for(&ProviderKind::MiniMax),
            Some(ProviderTransport::Anthropic)
        );
        assert_eq!(
            config.provider_transport_for(&ProviderKind::ZAI),
            Some(ProviderTransport::Anthropic)
        );
    }

    #[test]
    fn openai_compatible_allows_keyless_local_provider() {
        let mut config = AgentConfig {
            provider: ProviderKind::OpenAICompatible,
            ..AgentConfig::default()
        };
        config.providers.set(
            &ProviderKind::OpenAICompatible,
            ProviderConfig {
                base_url: Some("http://127.0.0.1:11434/v1".into()),
                model: Some("local-model".into()),
                ..ProviderConfig::default()
            },
        );

        let provider = AgentProvider::new(config);

        assert_eq!(
            provider
                .resolve_configured_api_key(&ProviderKind::OpenAICompatible)
                .expect("api key resolution"),
            None
        );
        provider
            .build_openai_compatible_client()
            .expect("keyless OpenAI-compatible client");
    }

    #[test]
    fn openai_compatible_preserves_default_env_for_hosted_provider() {
        let mut config = AgentConfig {
            provider: ProviderKind::OpenAICompatible,
            ..AgentConfig::default()
        };
        config.providers.set(
            &ProviderKind::OpenAICompatible,
            ProviderConfig {
                base_url: Some("https://api.example.com/v1".into()),
                model: Some("hosted-model".into()),
                ..ProviderConfig::default()
            },
        );

        let provider = AgentProvider::new(config);

        assert!(!provider.should_skip_default_env_api_key(&ProviderKind::OpenAICompatible));
    }

    #[test]
    fn local_openai_compatible_base_url_detection_covers_common_local_hosts() {
        assert!(is_local_openai_compatible_base_url(
            "http://127.0.0.1:11434/v1"
        ));
        assert!(is_local_openai_compatible_base_url(
            "http://localhost:8080/v1"
        ));
        assert!(is_local_openai_compatible_base_url(
            "http://192.168.1.10:8080/v1"
        ));
        assert!(is_local_openai_compatible_base_url(
            "http://host.docker.internal:8080/v1"
        ));
        assert!(!is_local_openai_compatible_base_url(
            "https://api.example.com/v1"
        ));
    }

    #[test]
    fn kimi_coding_url_detection_is_scoped_to_coding_endpoint() {
        assert!(is_kimi_coding_base_url("https://api.kimi.com/coding/v1"));
        assert!(is_kimi_coding_base_url("https://api.kimi.com/coding/v1/"));
        assert!(!is_kimi_coding_base_url("https://api.moonshot.ai/v1"));
        assert!(!is_kimi_coding_base_url(
            "https://api.moonshot.ai/anthropic"
        ));
    }

    #[test]
    fn kimi_coding_headers_match_kimi_cli_api_key_path() {
        let headers = kimi_coding_headers();

        assert_eq!(
            headers
                .get(http::header::USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(KIMI_CODING_USER_AGENT)
        );
        assert!(headers.get("X-Msh-Platform").is_none());
    }

    #[test]
    fn moonshot_client_adds_kimi_headers_only_for_coding_endpoint() {
        let mut coding_config = AgentConfig::default();
        coding_config.providers.set(
            &ProviderKind::Moonshot,
            ProviderConfig {
                api_key: Some("test-key".into()),
                base_url: Some(KIMI_CODING_BASE_URL.into()),
                ..Default::default()
            },
        );

        let coding_client = AgentProvider::new(coding_config)
            .build_moonshot_client()
            .expect("moonshot coding client");

        assert_eq!(
            coding_client
                .headers()
                .get(http::header::USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(KIMI_CODING_USER_AGENT)
        );

        let mut standard_config = AgentConfig::default();
        standard_config.providers.set(
            &ProviderKind::Moonshot,
            ProviderConfig {
                api_key: Some("test-key".into()),
                base_url: Some("https://api.moonshot.ai/v1".into()),
                ..Default::default()
            },
        );

        let standard_client = AgentProvider::new(standard_config)
            .build_moonshot_client()
            .expect("moonshot standard client");

        assert!(
            standard_client
                .headers()
                .get(http::header::USER_AGENT)
                .is_none()
        );
    }

    #[test]
    fn think_parser_extracts_inline_reasoning() {
        let mut parser = ThinkTagStreamParser::default();
        let out = parser.push("before<think>reasoning</think>after");
        let tail = parser.finish();

        assert_eq!(out.visible, "beforeafter");
        assert_eq!(out.reasoning, "reasoning");
        assert_eq!(tail, ThinkParseOutput::default());
    }

    #[test]
    fn think_parser_handles_split_tags_across_chunks() {
        let mut parser = ThinkTagStreamParser::default();

        let a = parser.push("brew<th");
        let b = parser.push("ink>install");
        let c = parser.push(" the tool</th");
        let d = parser.push("ink> now");
        let tail = parser.finish();

        assert_eq!(a.visible, "brew");
        assert_eq!(a.reasoning, "");
        assert_eq!(b.visible, "");
        assert_eq!(b.reasoning, "install");
        assert_eq!(c.visible, "");
        assert_eq!(c.reasoning, " the tool");
        assert_eq!(d.visible, " now");
        assert_eq!(d.reasoning, "");
        assert_eq!(tail, ThinkParseOutput::default());
    }

    #[test]
    fn think_parser_keeps_plain_text_unchanged() {
        let mut parser = ThinkTagStreamParser::default();
        let out = parser.push("plain output only");
        let tail = parser.finish();

        assert_eq!(out.visible, "plain output only");
        assert_eq!(out.reasoning, "");
        assert_eq!(tail, ThinkParseOutput::default());
    }
}

fn is_kimi_coding_base_url(url: &str) -> bool {
    url.trim_end_matches('/') == KIMI_CODING_BASE_URL
}

fn kimi_coding_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    // Kimi CLI sends this header for API-key based LLM calls. Its X-Msh device
    // headers belong to the OAuth path, so Con does not fabricate them here.
    headers.insert(
        http::header::USER_AGENT,
        HeaderValue::from_static(KIMI_CODING_USER_AGENT),
    );
    headers
}

impl ProviderKind {
    pub fn default_api_key_env(&self) -> &str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::ChatGPT => "CHATGPT_ACCESS_TOKEN",
            Self::GitHubCopilot => "GITHUB_COPILOT_API_KEY",
            Self::OpenAICompatible => "OPENAI_API_KEY",
            Self::OpenAI => "OPENAI_API_KEY",
            Self::MiniMax | Self::MiniMaxAnthropic => "MINIMAX_API_KEY",
            Self::Moonshot | Self::MoonshotAnthropic => "MOONSHOT_API_KEY",
            Self::ZAI | Self::ZAIAnthropic => "ZAI_API_KEY",
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::Groq => "GROQ_API_KEY",
            Self::Cohere => "COHERE_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Ollama => "OLLAMA_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::Perplexity => "PERPLEXITY_API_KEY",
            Self::Mistral => "MISTRAL_API_KEY",
            Self::Together => "TOGETHER_API_KEY",
            Self::XAI => "XAI_API_KEY",
        }
    }

    pub fn default_model(&self) -> &str {
        match self {
            Self::Anthropic => "claude-sonnet-4-6",
            Self::ChatGPT => "gpt-5.5",
            Self::GitHubCopilot => "gpt-4o",
            Self::OpenAICompatible => "gpt-4o",
            Self::OpenAI => "gpt-4o",
            Self::MiniMax => "MiniMax-M2",
            Self::MiniMaxAnthropic => "MiniMax-M2",
            Self::Moonshot => "kimi-k2.5",
            Self::MoonshotAnthropic => "kimi-k2.5",
            Self::ZAI => "glm-4.6",
            Self::ZAIAnthropic => "glm-4.6",
            Self::DeepSeek => "deepseek-v4-flash",
            Self::Groq => "llama-3.3-70b-versatile",
            Self::Cohere => "command-a-03-2025",
            Self::Gemini => "gemini-2.5-flash",
            Self::Ollama => "llama3.2",
            Self::OpenRouter => "anthropic/claude-sonnet-4-6",
            Self::Perplexity => "sonar-pro",
            Self::Mistral => "mistral-large-latest",
            Self::Together => "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            Self::XAI => "grok-3",
        }
    }
}

#[derive(Debug, Clone)]
pub struct OAuthDevicePrompt {
    pub verification_uri: String,
    pub user_code: String,
}

pub fn oauth_token_dir(kind: &ProviderKind) -> Option<PathBuf> {
    let provider_dir = match kind {
        ProviderKind::ChatGPT => "chatgpt-subscription",
        ProviderKind::GitHubCopilot => "github-copilot",
        _ => return None,
    };

    Some(con_paths::app_config_dir().join("auth").join(provider_dir))
}

#[derive(Debug, Deserialize)]
struct CodexAuthFile {
    auth_mode: Option<String>,
    tokens: Option<CodexAuthTokens>,
}

#[derive(Debug, Deserialize)]
struct CodexAuthTokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct RigChatGptAuthRecord {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RigChatGptAuthTokenView {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
}

const CHATGPT_TOKEN_EXPIRY_SKEW_SECONDS: i64 = 60;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct CodexAuthSyncState {
    source_fingerprint: String,
}

fn convert_codex_chatgpt_auth(auth: CodexAuthFile) -> Option<RigChatGptAuthRecord> {
    if auth.auth_mode.as_deref() != Some("chatgpt") {
        return None;
    }

    let tokens = auth.tokens?;
    let refresh_token = tokens.refresh_token?;
    Some(RigChatGptAuthRecord {
        access_token: tokens.access_token?,
        refresh_token: Some(refresh_token),
        id_token: tokens.id_token,
        account_id: tokens.account_id,
    })
}

fn codex_auth_file() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".codex").join("auth.json"))
}

pub fn read_chatgpt_oauth_access_token(auth_file: &std::path::Path) -> Result<Option<String>> {
    Ok(read_chatgpt_oauth_record(auth_file)?
        .and_then(|record| non_empty_token(record.access_token)))
}

fn read_chatgpt_oauth_record(
    auth_file: &std::path::Path,
) -> Result<Option<RigChatGptAuthTokenView>> {
    match std::fs::read_to_string(auth_file) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn non_empty_token(token: Option<String>) -> Option<String> {
    token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

fn chatgpt_oauth_record_ready_at(record: RigChatGptAuthTokenView, now: i64) -> bool {
    let access_token_ready = non_empty_token(record.access_token).is_some()
        && record.expires_at.is_some_and(|expires_at| {
            now < expires_at.saturating_sub(CHATGPT_TOKEN_EXPIRY_SKEW_SECONDS)
        });
    let refresh_token_ready = non_empty_token(record.refresh_token).is_some();

    access_token_ready || refresh_token_ready
}

pub fn read_synced_chatgpt_oauth_access_token(
    auth_file: &std::path::Path,
) -> Result<Option<String>> {
    sync_codex_chatgpt_auth(auth_file)?;
    read_chatgpt_oauth_access_token(auth_file)
}

fn codex_auth_sync_state_file(target_auth_file: &std::path::Path) -> PathBuf {
    target_auth_file.with_file_name("codex-auth-sync.json")
}

fn sync_codex_chatgpt_auth_from_file(
    source_auth_file: &std::path::Path,
    target_auth_file: &std::path::Path,
) -> Result<bool> {
    if !source_auth_file.is_file() {
        return Ok(false);
    }

    let source = std::fs::read_to_string(source_auth_file)?;
    let codex_auth: CodexAuthFile = serde_json::from_str(&source)?;
    let Some(record) = convert_codex_chatgpt_auth(codex_auth) else {
        return Ok(false);
    };

    let bytes = serde_json::to_vec_pretty(&record)?;
    let source_fingerprint = stable_fingerprint(&bytes);
    let sync_state_file = codex_auth_sync_state_file(target_auth_file);
    let target_bytes = target_auth_file
        .is_file()
        .then(|| std::fs::read(target_auth_file))
        .transpose()?;

    // An identical cache is safe to adopt as Codex-managed, including caches
    // imported by an older Con build before the provenance sidecar existed.
    if target_bytes.as_deref() == Some(bytes.as_slice()) {
        write_codex_auth_sync_state(&sync_state_file, &source_fingerprint)?;
        return Ok(false);
    }

    if let Some(target_bytes) = target_bytes.as_deref() {
        let Some(sync_state) = read_codex_auth_sync_state(&sync_state_file)? else {
            // A pre-existing cache without provenance belongs to the user (or
            // another auth flow), not to the automatic Codex import path.
            return Ok(false);
        };

        if stable_fingerprint(target_bytes) != sync_state.source_fingerprint {
            // The cache changed after Con imported it. Rig may have refreshed
            // it, or the user may have signed into another account. Either way,
            // retire our ownership marker rather than overwrite local auth.
            clear_auth_record_if_present(&sync_state_file)?;
            return Ok(false);
        }
    }

    write_auth_record(target_auth_file, &bytes)?;
    write_codex_auth_sync_state(&sync_state_file, &source_fingerprint)?;

    log::info!(
        "[provider] Synced ChatGPT OAuth cache from Codex auth into {}",
        target_auth_file.display()
    );
    Ok(true)
}

fn stable_fingerprint(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn read_codex_auth_sync_state(path: &std::path::Path) -> Result<Option<CodexAuthSyncState>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn write_codex_auth_sync_state(path: &std::path::Path, source_fingerprint: &str) -> Result<()> {
    let state = CodexAuthSyncState {
        source_fingerprint: source_fingerprint.to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&state)?;
    write_auth_record(path, &bytes)
}

fn clear_auth_record_if_present(path: &std::path::Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn clear_codex_auth_sync_state(target_auth_file: &std::path::Path) -> Result<bool> {
    clear_auth_record_if_present(&codex_auth_sync_state_file(target_auth_file))
}

fn write_auth_record(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp_path = {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        path.with_extension(format!("tmp.{}.{}", std::process::id(), unique))
    };

    let write_result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let mut file = options.open(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err.into());
    }

    Ok(())
}

fn sync_codex_chatgpt_auth(target_auth_file: &std::path::Path) -> Result<bool> {
    let Some(source_auth_file) = codex_auth_file() else {
        return Ok(false);
    };
    sync_codex_chatgpt_auth_from_file(&source_auth_file, target_auth_file)
}

/// Best-effort, zero-touch sync of the ChatGPT Subscription OAuth cache from
/// Codex's `~/.codex/auth.json` into Con's token directory.
///
/// Call this once at startup so a user who is already signed in to Codex is
/// picked up automatically — no manual device login and no need to ever open
/// the Providers settings. UI sign-in indicators key off provider-specific
/// cache files (for ChatGPT, `auth.json`), so writing the cache before the first
/// render is what makes the experience feel seamless. Existing local auth is
/// never replaced unless its provenance marker proves that Con previously
/// imported it from Codex and it has not changed since.
///
/// Returns `Ok(true)` when fresh credentials were written, `Ok(false)` when
/// there was nothing to do (no Codex auth, not a ChatGPT OAuth login, or the
/// cache is current or independently owned).
pub fn ensure_chatgpt_oauth_synced_from_codex() -> Result<bool> {
    let Some(dir) = oauth_token_dir(&ProviderKind::ChatGPT) else {
        return Ok(false);
    };
    sync_codex_chatgpt_auth(&dir.join("auth.json"))
}

fn anthropic_credentials_available(config: &AgentConfig) -> bool {
    provider_credentials_available(config, &ProviderKind::Anthropic)
}

/// Whether a provider has a usable API credential through the same paths used
/// by live requests: inline config, a referenced env var, a legacy direct key
/// in `api_key_env`, or the provider's default environment variable.
fn provider_credentials_available(config: &AgentConfig, kind: &ProviderKind) -> bool {
    if configured_api_key_value(config, kind).is_some() {
        return true;
    }
    std::env::var(kind.default_api_key_env()).is_ok_and(|value| !value.trim().is_empty())
}

fn configured_api_key_value(config: &AgentConfig, kind: &ProviderKind) -> Option<String> {
    let pc = config.providers.get(kind);

    if let Some(key) = pc
        .and_then(|p| p.api_key.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return Some(key.to_string());
    }

    if let Some(key_or_env) = pc
        .and_then(|p| p.api_key_env.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        if provider_api_key_env_name_like(key_or_env) {
            if let Ok(val) = std::env::var(key_or_env) {
                let val = val.trim().to_string();
                if !val.is_empty() {
                    return Some(val);
                }
            }
        } else {
            return Some(key_or_env.to_string());
        }
    }

    None
}

fn provider_api_key_env_name_like(value: &str) -> bool {
    value
        .chars()
        .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
}

/// Whether Rig can use the ChatGPT Subscription OAuth cache without prompting.
///
/// This mirrors Rig's native authenticator: it can proceed with either an
/// unexpired access token (including Rig's 60-second expiry skew) or a refresh
/// token. An access token without expiry metadata is not enough for zero-touch
/// startup because Rig treats it as expired and opens device authorization.
fn chatgpt_oauth_cache_ready() -> bool {
    let Some(auth_file) = oauth_token_dir(&ProviderKind::ChatGPT).map(|dir| dir.join("auth.json"))
    else {
        return false;
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();

    read_chatgpt_oauth_record(&auth_file)
        .ok()
        .flatten()
        .is_some_and(|record| chatgpt_oauth_record_ready_at(record, now))
}

/// Choose the effective default provider for a freshly loaded config.
///
/// Zero-touch sign-in: the hard-coded default provider is Anthropic, but when
/// the user has *not* configured any Anthropic credentials and a ChatGPT
/// credential is ready (an API-key override or a Subscription OAuth cache,
/// typically synced from Codex), prefer ChatGPT so the first run — and every
/// new window — works without manually switching providers.
///
/// Deliberately conservative: explicit choices are always preserved. An
/// automatic choice is recomputed from current credential readiness, preferring
/// configured Anthropic and otherwise using ChatGPT only while one of its
/// credential paths is usable. Returns the provider to switch to, or `None`
/// when it is unchanged.
pub fn preferred_default_provider(config: &AgentConfig) -> Option<ProviderKind> {
    preferred_default_provider_for_state(
        config,
        config.provider_is_explicit,
        anthropic_credentials_available(config),
        provider_credentials_available(config, &ProviderKind::ChatGPT)
            || chatgpt_oauth_cache_ready(),
    )
}

fn preferred_default_provider_for_state(
    config: &AgentConfig,
    provider_is_explicit: bool,
    anthropic_credentials_ready: bool,
    chatgpt_credentials_ready: bool,
) -> Option<ProviderKind> {
    if provider_is_explicit {
        return None;
    }

    let provider = if anthropic_credentials_ready || !chatgpt_credentials_ready {
        ProviderKind::Anthropic
    } else {
        ProviderKind::ChatGPT
    };
    (config.provider != provider).then_some(provider)
}

fn mark_codex_chatgpt_auth_source_seen(target_auth_file: &std::path::Path) -> Result<bool> {
    let Some(source_auth_file) = codex_auth_file() else {
        return Ok(false);
    };
    if !source_auth_file.is_file() {
        return Ok(false);
    }

    let source = std::fs::read_to_string(source_auth_file)?;
    let codex_auth: CodexAuthFile = serde_json::from_str(&source)?;
    let Some(record) = convert_codex_chatgpt_auth(codex_auth) else {
        return Ok(false);
    };

    let bytes = serde_json::to_vec_pretty(&record)?;
    let source_fingerprint = stable_fingerprint(&bytes);
    write_codex_auth_sync_state(
        &codex_auth_sync_state_file(target_auth_file),
        &source_fingerprint,
    )?;
    Ok(true)
}

pub async fn authorize_oauth_provider<F>(kind: ProviderKind, prompt_handler: F) -> Result<()>
where
    F: Fn(OAuthDevicePrompt) + Send + Sync + 'static,
{
    let prompt_handler = Arc::new(prompt_handler);

    match kind {
        ProviderKind::ChatGPT => {
            let prompt_handler = prompt_handler.clone();
            let mut builder = chatgpt::Client::builder()
                .oauth()
                .on_device_code(move |prompt| {
                    prompt_handler(OAuthDevicePrompt {
                        verification_uri: prompt.verification_uri,
                        user_code: prompt.user_code,
                    });
                });
            if let Some(dir) = oauth_token_dir(&kind) {
                let auth_file = dir.join("auth.json");
                if clear_auth_record_if_present(&auth_file)? {
                    log::info!(
                        "[provider] Cleared existing ChatGPT OAuth cache before manual authorization"
                    );
                }
                clear_codex_auth_sync_state(&auth_file)?;
                builder = builder.token_dir(dir);
            }
            let client = builder
                .build()
                .map_err(|e| anyhow::anyhow!("ChatGPT client error: {e}"))?;
            client
                .authorize()
                .await
                .map_err(|e| anyhow::anyhow!("ChatGPT OAuth error: {e}"))?;
            if let Some(dir) = oauth_token_dir(&kind) {
                let auth_file = dir.join("auth.json");
                if let Err(err) = mark_codex_chatgpt_auth_source_seen(&auth_file) {
                    log::warn!("[provider] Failed to mark Codex ChatGPT auth source seen: {err}");
                }
            }
            Ok(())
        }
        ProviderKind::GitHubCopilot => {
            let prompt_handler = prompt_handler.clone();
            let mut builder = copilot::Client::builder()
                .oauth()
                .on_device_code(move |prompt| {
                    prompt_handler(OAuthDevicePrompt {
                        verification_uri: prompt.verification_uri,
                        user_code: prompt.user_code,
                    });
                });
            if let Some(dir) = oauth_token_dir(&kind) {
                builder = builder.token_dir(dir);
            }
            let client = builder
                .build()
                .map_err(|e| anyhow::anyhow!("GitHub Copilot client error: {e}"))?;
            client
                .authorize()
                .await
                .map_err(|e| anyhow::anyhow!("GitHub Copilot OAuth error: {e}"))
        }
        _ => anyhow::bail!("{kind} does not support OAuth device login"),
    }
}

// ── Per-provider config ─────────────────────────────────────────────

/// Settings specific to a single provider — model, credentials, endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub model: Option<String>,
    /// Direct API key value.
    pub api_key: Option<String>,
    /// Environment variable name containing the API key.
    pub api_key_env: Option<String>,
    /// Custom base URL override (most providers have sensible defaults in Rig).
    pub base_url: Option<String>,
    /// Max output tokens (provider-specific limits apply).
    pub max_tokens: Option<u64>,
}

/// Map of per-provider configurations.
/// Explicit fields (not HashMap) for clean TOML serialization.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderMap {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chatgpt: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "github-copilot")]
    pub github_copilot: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none", alias = "openai-compatible")]
    pub openaicompatible: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimax: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "minimax-anthropic")]
    pub minimax_anthropic: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moonshot: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "moonshot-anthropic")]
    pub moonshot_anthropic: Option<ProviderConfig>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "z-ai",
        alias = "zai"
    )]
    pub zai: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "z-ai-anthropic")]
    pub zai_anthropic: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepseek: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groq: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cohere: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gemini: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openrouter: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub perplexity: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mistral: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub together: Option<ProviderConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xai: Option<ProviderConfig>,
}

impl ProviderMap {
    pub fn get(&self, kind: &ProviderKind) -> Option<&ProviderConfig> {
        match kind {
            ProviderKind::Anthropic => self.anthropic.as_ref(),
            ProviderKind::OpenAI => self.openai.as_ref(),
            ProviderKind::ChatGPT => self.chatgpt.as_ref(),
            ProviderKind::GitHubCopilot => self.github_copilot.as_ref(),
            ProviderKind::OpenAICompatible => self.openaicompatible.as_ref(),
            ProviderKind::MiniMax => self.minimax.as_ref(),
            ProviderKind::MiniMaxAnthropic => self.minimax_anthropic.as_ref(),
            ProviderKind::Moonshot => self.moonshot.as_ref(),
            ProviderKind::MoonshotAnthropic => self.moonshot_anthropic.as_ref(),
            ProviderKind::ZAI => self.zai.as_ref(),
            ProviderKind::ZAIAnthropic => self.zai_anthropic.as_ref(),
            ProviderKind::DeepSeek => self.deepseek.as_ref(),
            ProviderKind::Groq => self.groq.as_ref(),
            ProviderKind::Cohere => self.cohere.as_ref(),
            ProviderKind::Gemini => self.gemini.as_ref(),
            ProviderKind::Ollama => self.ollama.as_ref(),
            ProviderKind::OpenRouter => self.openrouter.as_ref(),
            ProviderKind::Perplexity => self.perplexity.as_ref(),
            ProviderKind::Mistral => self.mistral.as_ref(),
            ProviderKind::Together => self.together.as_ref(),
            ProviderKind::XAI => self.xai.as_ref(),
        }
    }

    pub fn get_or_default(&self, kind: &ProviderKind) -> ProviderConfig {
        self.get(kind).cloned().unwrap_or_default()
    }

    pub fn set(&mut self, kind: &ProviderKind, config: ProviderConfig) {
        let slot = match kind {
            ProviderKind::Anthropic => &mut self.anthropic,
            ProviderKind::OpenAI => &mut self.openai,
            ProviderKind::ChatGPT => &mut self.chatgpt,
            ProviderKind::GitHubCopilot => &mut self.github_copilot,
            ProviderKind::OpenAICompatible => &mut self.openaicompatible,
            ProviderKind::MiniMax => &mut self.minimax,
            ProviderKind::MiniMaxAnthropic => &mut self.minimax_anthropic,
            ProviderKind::Moonshot => &mut self.moonshot,
            ProviderKind::MoonshotAnthropic => &mut self.moonshot_anthropic,
            ProviderKind::ZAI => &mut self.zai,
            ProviderKind::ZAIAnthropic => &mut self.zai_anthropic,
            ProviderKind::DeepSeek => &mut self.deepseek,
            ProviderKind::Groq => &mut self.groq,
            ProviderKind::Cohere => &mut self.cohere,
            ProviderKind::Gemini => &mut self.gemini,
            ProviderKind::Ollama => &mut self.ollama,
            ProviderKind::OpenRouter => &mut self.openrouter,
            ProviderKind::Perplexity => &mut self.perplexity,
            ProviderKind::Mistral => &mut self.mistral,
            ProviderKind::Together => &mut self.together,
            ProviderKind::XAI => &mut self.xai,
        };
        *slot = Some(config);
    }
}

// ── Config ──────────────────────────────────────────────────────────

/// Optional overrides for the inline suggestion model.
/// API key and base_url are inherited from the provider's entry in `providers`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SuggestionModelConfig {
    pub enabled: bool,
    pub provider: Option<ProviderKind>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentPurpose {
    Build,
    Explain,
    Operate,
}

impl Default for AgentPurpose {
    fn default() -> Self {
        Self::Build
    }
}

impl AgentPurpose {
    pub fn system_prompt_note(self) -> &'static str {
        match self {
            Self::Build => {
                "Default operating mode: Build.\n\
                 Prefer carrying tasks through to implementation, verification, and concrete outcomes.\n\
                 Use tools proactively when they reduce ambiguity or unblock execution."
            }
            Self::Explain => {
                "Default operating mode: Explain.\n\
                 Prefer investigation, explanation, and read-first analysis.\n\
                 Do not make edits or run write-capable actions unless the user asks for them."
            }
            Self::Operate => {
                "Default operating mode: Operate.\n\
                 Prefer terminal-first workflows, shell verification, and concise command-driven execution.\n\
                 Bias toward precise operational status, command hygiene, and low-ceremony responses."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderTransport {
    OpenAI,
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderProtocolPreferences {
    pub minimax: Option<ProviderTransport>,
    pub moonshot: Option<ProviderTransport>,
    pub zai: Option<ProviderTransport>,
}

/// Agent configuration from config.toml
///
/// ```toml
/// [agent]
/// provider = "anthropic"
/// max_turns = 10
/// temperature = 0.7
///
/// [agent.providers.anthropic]
/// model = "claude-sonnet-4-6"
/// api_key = "sk-ant-..."
/// max_tokens = 8192
///
/// [agent.providers.groq]
/// model = "llama-3.3-70b-versatile"
/// api_key_env = "GROQ_API_KEY"
/// max_tokens = 16384
///
/// [agent.suggestion_model]
/// provider = "groq"
/// model = "llama-3.1-8b-instant"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Active provider selection.
    pub provider: ProviderKind,
    /// Whether the active provider was explicitly selected by the user.
    ///
    /// Older Con versions always serialized `provider`, including its default,
    /// so presence alone cannot distinguish a choice from generated config.
    #[serde(default)]
    pub provider_is_explicit: bool,
    /// High-level operating stance for the built-in agent.
    pub purpose: AgentPurpose,
    /// Per-provider settings (model, key, endpoint, max_tokens).
    pub providers: ProviderMap,
    /// Remember provider-specific transport choice for providers that expose both OpenAI and Anthropic-compatible APIs.
    pub provider_protocols: ProviderProtocolPreferences,
    /// Global: max agent turns per request.
    pub max_turns: usize,
    /// Global: sampling temperature (applied to all providers).
    pub temperature: Option<f64>,
    pub auto_context: bool,
    pub auto_approve_tools: bool,
    pub suggestion_model: SuggestionModelConfig,

    // Legacy flat fields — deserialize from old configs, never written back.
    #[serde(default, skip_serializing)]
    model: Option<String>,
    #[serde(default, skip_serializing)]
    api_key: Option<String>,
    #[serde(default, skip_serializing)]
    api_key_env: Option<String>,
    #[serde(default, skip_serializing)]
    base_url: Option<String>,
    #[serde(default, skip_serializing)]
    max_tokens: Option<u64>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            provider: ProviderKind::default(),
            provider_is_explicit: false,
            purpose: AgentPurpose::default(),
            providers: ProviderMap::default(),
            provider_protocols: ProviderProtocolPreferences::default(),
            max_turns: 30,
            temperature: None,
            auto_context: true,
            auto_approve_tools: false,
            suggestion_model: SuggestionModelConfig::default(),
            // Legacy
            model: None,
            api_key: None,
            api_key_env: None,
            base_url: None,
            max_tokens: None,
        }
    }
}

impl Default for SuggestionModelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: None,
            model: None,
        }
    }
}

impl AgentConfig {
    pub fn select_provider(&mut self, provider: ProviderKind) {
        self.provider = provider;
        self.provider_is_explicit = true;
    }

    pub fn provider_transport_for(&self, provider: &ProviderKind) -> Option<ProviderTransport> {
        match provider {
            ProviderKind::MiniMax | ProviderKind::MiniMaxAnthropic => {
                self.provider_protocols.minimax
            }
            ProviderKind::Moonshot | ProviderKind::MoonshotAnthropic => {
                self.provider_protocols.moonshot
            }
            ProviderKind::ZAI | ProviderKind::ZAIAnthropic => self.provider_protocols.zai,
            _ => None,
        }
    }

    pub fn set_provider_transport(
        &mut self,
        provider: &ProviderKind,
        transport: Option<ProviderTransport>,
    ) {
        match provider {
            ProviderKind::MiniMax | ProviderKind::MiniMaxAnthropic => {
                self.provider_protocols.minimax = transport;
            }
            ProviderKind::Moonshot | ProviderKind::MoonshotAnthropic => {
                self.provider_protocols.moonshot = transport;
            }
            ProviderKind::ZAI | ProviderKind::ZAIAnthropic => {
                self.provider_protocols.zai = transport;
            }
            _ => {}
        }
    }

    /// Migrate legacy flat fields into the per-provider map.
    /// Called once after deserialization. Safe to call multiple times.
    pub fn migrate_legacy(&mut self) {
        let has_legacy = self.model.is_some()
            || self.api_key.is_some()
            || self.api_key_env.is_some()
            || self.base_url.is_some()
            || self.max_tokens.is_some();

        if has_legacy && self.providers.get(&self.provider).is_none() {
            self.providers.set(
                &self.provider,
                ProviderConfig {
                    model: self.model.take(),
                    api_key: self.api_key.take(),
                    api_key_env: self.api_key_env.take(),
                    base_url: self.base_url.take(),
                    max_tokens: self.max_tokens.take(),
                },
            );
        }

        let infer_transport = |openai_kind: ProviderKind,
                               anthropic_kind: ProviderKind,
                               active_provider: &ProviderKind,
                               providers: &ProviderMap|
         -> Option<ProviderTransport> {
            if active_provider == &anthropic_kind {
                Some(ProviderTransport::Anthropic)
            } else if active_provider == &openai_kind {
                Some(ProviderTransport::OpenAI)
            } else if providers.get(&anthropic_kind).is_some()
                && providers.get(&openai_kind).is_none()
            {
                Some(ProviderTransport::Anthropic)
            } else if providers.get(&openai_kind).is_some()
                && providers.get(&anthropic_kind).is_none()
            {
                Some(ProviderTransport::OpenAI)
            } else {
                None
            }
        };

        if self.provider_protocols.minimax.is_none() {
            self.provider_protocols.minimax = infer_transport(
                ProviderKind::MiniMax,
                ProviderKind::MiniMaxAnthropic,
                &self.provider,
                &self.providers,
            );
        }
        if self.provider_protocols.moonshot.is_none() {
            self.provider_protocols.moonshot = infer_transport(
                ProviderKind::Moonshot,
                ProviderKind::MoonshotAnthropic,
                &self.provider,
                &self.providers,
            );
        }
        if self.provider_protocols.zai.is_none() {
            self.provider_protocols.zai = infer_transport(
                ProviderKind::ZAI,
                ProviderKind::ZAIAnthropic,
                &self.provider,
                &self.providers,
            );
        }
    }

    /// Effective model for the given provider.
    pub fn effective_model<'a>(&'a self, kind: &'a ProviderKind) -> &'a str {
        self.providers
            .get(kind)
            .and_then(|p| p.model.as_deref())
            .unwrap_or_else(move || kind.default_model())
    }

    /// Effective base URL override for the given provider.
    pub fn effective_base_url(&self, kind: &ProviderKind) -> Option<&str> {
        self.providers.get(kind).and_then(|p| p.base_url.as_deref())
    }

    /// Effective max tokens for the given provider.
    pub fn effective_max_tokens(&self, kind: &ProviderKind) -> Option<u64> {
        self.providers.get(kind).and_then(|p| p.max_tokens)
    }

    pub fn system_prompt_prefix(&self) -> &'static str {
        self.purpose.system_prompt_note()
    }

    /// Build a lightweight config for inline suggestions.
    /// Uses the suggestion provider's credentials from the providers map.
    pub fn suggestion_agent_config(&self) -> AgentConfig {
        let suggestion_provider = self
            .suggestion_model
            .provider
            .clone()
            .unwrap_or_else(|| self.provider.clone());

        // Build a minimal providers map with just the suggestion provider's config
        let mut providers = ProviderMap::default();
        if let Some(pc) = self.providers.get(&suggestion_provider) {
            let mut pc = pc.clone();
            // Override model if suggestion_model specifies one
            if let Some(ref model) = self.suggestion_model.model {
                pc.model = Some(model.clone());
            }
            pc.max_tokens = Some(48);
            providers.set(&suggestion_provider, pc);
        } else if let Some(ref model) = self.suggestion_model.model {
            providers.set(
                &suggestion_provider,
                ProviderConfig {
                    model: Some(model.clone()),
                    max_tokens: Some(48),
                    ..Default::default()
                },
            );
        }

        AgentConfig {
            provider: suggestion_provider,
            provider_is_explicit: false,
            purpose: self.purpose,
            providers,
            provider_protocols: ProviderProtocolPreferences::default(),
            max_turns: 1,
            temperature: Some(0.0),
            auto_context: false,
            auto_approve_tools: false,
            suggestion_model: SuggestionModelConfig::default(),
            // No legacy
            model: None,
            api_key: None,
            api_key_env: None,
            base_url: None,
            max_tokens: None,
        }
    }
}

// ── Events ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Thinking,
    /// Incremental extended thinking/reasoning text from the model
    ThinkingDelta(String),
    Token(String),
    Step(AgentStep),
    ToolCallStart {
        call_id: String,
        tool_name: String,
        args: String,
    },
    ToolCallComplete {
        call_id: String,
        tool_name: String,
        result: String,
    },
    Done(Message),
    Error(String),
}

// ── Agent builder macro ─────────────────────────────────────────────

/// Build an agent with all tools, stream the response, and consume it.
///
/// Rig's `CompletionClient` trait has associated types (`CompletionModel`,
/// `StreamingResponse`) whose lifetime bounds prevent a clean generic function.
/// This macro expands identically at each call site, giving the compiler
/// concrete types while keeping tool registration in one place.
macro_rules! build_and_stream {
    ($client:expr, $cfg:expr, $kind:expr, $system_prompt:expr, $prompt:expr,
     $history:expr, $hook:expr, $terminal_exec_tx:expr, $pane_tx:expr,
     $event_tx:expr, $cancelled:expr, $workspace_root:expr) => {{
        let root: std::path::PathBuf = $workspace_root;
        let mut builder = $client
            .agent($cfg.effective_model(&$kind))
            .preamble($system_prompt)
            .tool(TerminalExecTool::new($terminal_exec_tx.clone()))
            .tool(ShellExecTool)
            .tool(FileReadTool::new(root.clone()))
            .tool(FileWriteTool::new(root.clone()))
            .tool(EditFileTool::new(root.clone()))
            .tool(ListFilesTool::new(root.clone()))
            .tool(SearchTool::new(root))
            .tool(ListPanesTool::new($pane_tx.clone()))
            .tool(ListTabWorkspacesTool::new($pane_tx.clone()))
            .tool(TmuxInspectTool::new($pane_tx.clone()))
            .tool(TmuxListTool::new($pane_tx.clone()))
            .tool(TmuxCaptureTool::new($pane_tx.clone()))
            .tool(TmuxFindTargetsTool::new($pane_tx.clone()))
            .tool(ResolveWorkTargetTool::new($pane_tx.clone()))
            .tool(EnsureLocalCodingWorkspaceTool::new($pane_tx.clone()))
            .tool(AgentCliTurnTool::new($pane_tx.clone()))
            .tool(EnsureLocalAgentTargetTool::new($pane_tx.clone()))
            .tool(EnsureLocalShellTargetTool::new($pane_tx.clone()))
            .tool(EnsureRemoteShellTargetTool::new($pane_tx.clone()))
            .tool(EnsureRemoteTmuxShellTargetTool::new(
                $pane_tx.clone(),
                $terminal_exec_tx.clone(),
            ))
            .tool(EnsureRemoteTmuxWorkspaceTool::new(
                $pane_tx.clone(),
                $terminal_exec_tx.clone(),
            ))
            .tool(RemoteExecTool::new(
                $pane_tx.clone(),
                $terminal_exec_tx.clone(),
            ))
            .tool(TmuxEnsureShellTargetTool::new($pane_tx.clone()))
            .tool(TmuxShellTurnTool::new($pane_tx.clone()))
            .tool(TmuxSendKeysTool::new($pane_tx.clone()))
            .tool(TmuxRunCommandTool::new($pane_tx.clone()))
            .tool(TmuxEnsureAgentTargetTool::new($pane_tx.clone()))
            .tool(ProbeShellContextTool::new($pane_tx.clone()))
            .tool(ReadPaneTool::new($pane_tx.clone()))
            .tool(SendKeysTool::new($pane_tx.clone()))
            .tool(SearchPanesTool::new($pane_tx.clone()))
            .tool(CreatePaneTool::new($pane_tx.clone()))
            .tool(WaitForTool::new($pane_tx, $cancelled.clone()))
            .tool(BatchExecTool::new($terminal_exec_tx))
            .default_max_turns(rig_model_call_budget($cfg.max_turns));

        if let Some(max_tokens) = $cfg.effective_max_tokens(&$kind) {
            builder = builder.max_tokens(max_tokens);
        }
        if let Some(temp) = $cfg.temperature {
            builder = builder.temperature(temp);
        }
        builder = builder.add_hook($hook);

        let agent = builder.build();

        // Diagnostic: log registered tools by querying the tool server
        match agent.tool_server_handle.get_tool_defs(None).await {
            Ok(defs) => {
                let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
                log::info!(
                    "[agent] Registered {} tools for {:?}/{}: {:?}",
                    defs.len(),
                    $kind,
                    $cfg.effective_model(&$kind),
                    names,
                );
            }
            Err(e) => {
                log::error!("[agent] Failed to query tool definitions: {}", e);
            }
        }

        let stream = agent.stream_prompt($prompt).history($history).await;

        consume_stream(stream, $event_tx, $cancelled).await
    }};
}

/// Rig 0.36 allowed two model calls beyond an explicit `max_turns` value. Rig
/// 0.40 makes the model-call budget exact, so retain Con's shipped ceiling.
fn rig_model_call_budget(tool_turns: usize) -> usize {
    tool_turns.saturating_add(2)
}

// ── Provider ────────────────────────────────────────────────────────

pub struct AgentProvider {
    config: AgentConfig,
}

impl AgentProvider {
    pub fn new(config: AgentConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    pub async fn send(
        &self,
        conversation: &Conversation,
        context: &TerminalContext,
        event_tx: Sender<AgentEvent>,
        approval_rx: crossbeam_channel::Receiver<ToolApprovalDecision>,
        terminal_exec_tx: crossbeam_channel::Sender<TerminalExecRequest>,
        pane_tx: crossbeam_channel::Sender<PaneRequest>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Message> {
        let _ = event_tx.send(AgentEvent::Thinking);
        let kind = &self.config.provider;

        let system_prompt = format!(
            "{}\n\n{}",
            self.config.system_prompt_prefix(),
            context.to_system_prompt()
        );
        let chat_history = conversation.to_rig_history();
        let last_user_msg = conversation
            .last_user_message()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let model = self.config.effective_model(kind);
        let base_url = self.config.effective_base_url(kind);
        log::info!(
            "[agent] Sending to {:?}/{}: system_prompt={} chars, history={} msgs, user_msg={} chars, panes={}, base_url={:?}",
            kind,
            model,
            system_prompt.len(),
            chat_history.len(),
            last_user_msg.len(),
            1 + context.other_panes.len(),
            base_url,
        );

        // Warn if a custom base_url is set but the model is the provider default —
        // the user probably forgot to set [agent.providers.<name>].model and is
        // sending the default model name to a third-party API.
        if base_url.is_some() && model == kind.default_model() {
            log::warn!(
                "[agent] Custom base_url is set for {:?} but model is the default '{}'. \
                 Set [agent.providers.{:?}].model in config.toml to override.",
                kind,
                model,
                kind,
            );
        }

        log::info!(
            target: "con_agent::flow",
            "{{\"event\":\"request_start\",\"provider\":\"{:?}\",\"model\":\"{}\",\"system_chars\":{},\"history_msgs\":{},\"user_chars\":{}}}",
            kind, model, system_prompt.len(), chat_history.len(), last_user_msg.len(),
        );
        let _ = event_tx.send(AgentEvent::Step(AgentStep::Thinking(format!(
            "Using {}",
            display_provider_model(kind, model)
        ))));

        log::info!(
            "[provider] auto_approve_tools = {}",
            self.config.auto_approve_tools
        );
        let hook = ConHook::new(
            event_tx.clone(),
            approval_rx,
            self.config.auto_approve_tools,
            cancelled.clone(),
        );

        // Each provider dispatches to its native Rig client — exhaustive match
        // prevents silent misrouting.
        // Derive workspace root from terminal context cwd, falling back to
        // $HOME, then to the platform's temp dir.
        let workspace_root = context
            .cwd
            .as_ref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| dirs::home_dir())
            .unwrap_or_else(std::env::temp_dir);

        macro_rules! stream_with {
            ($client:expr) => {
                build_and_stream!(
                    $client,
                    self.config,
                    kind,
                    &system_prompt,
                    &last_user_msg,
                    chat_history,
                    hook,
                    terminal_exec_tx,
                    pane_tx,
                    &event_tx,
                    &cancelled,
                    workspace_root.clone()
                )?
            };
        }

        let response = match *kind {
            ProviderKind::Anthropic => stream_with!(self.build_anthropic_client()?),
            ProviderKind::OpenAI => stream_with!(self.build_openai_client()?),
            ProviderKind::ChatGPT => stream_with!(self.build_chatgpt_client()?),
            ProviderKind::GitHubCopilot => stream_with!(self.build_github_copilot_client()?),
            ProviderKind::OpenAICompatible => {
                stream_with!(self.build_openai_compatible_client()?)
            }
            ProviderKind::MiniMax => stream_with!(self.build_minimax_client()?),
            ProviderKind::MiniMaxAnthropic => stream_with!(self.build_minimax_anthropic_client()?),
            ProviderKind::Moonshot => stream_with!(self.build_moonshot_client()?),
            ProviderKind::MoonshotAnthropic => {
                stream_with!(self.build_moonshot_anthropic_client()?)
            }
            ProviderKind::ZAI => stream_with!(self.build_zai_client()?),
            ProviderKind::ZAIAnthropic => stream_with!(self.build_zai_anthropic_client()?),
            ProviderKind::DeepSeek => stream_with!(self.build_deepseek_client()?),
            ProviderKind::Groq => stream_with!(self.build_groq_client()?),
            ProviderKind::Cohere => stream_with!(self.build_cohere_client()?),
            ProviderKind::Gemini => stream_with!(self.build_gemini_client()?),
            ProviderKind::Ollama => stream_with!(self.build_ollama_client()?),
            ProviderKind::OpenRouter => stream_with!(self.build_openrouter_client()?),
            ProviderKind::Perplexity => stream_with!(self.build_perplexity_client()?),
            ProviderKind::Mistral => stream_with!(self.build_mistral_client()?),
            ProviderKind::Together => stream_with!(self.build_together_client()?),
            ProviderKind::XAI => stream_with!(self.build_xai_client()?),
        };

        let model_name = self.config.effective_model(kind).to_string();
        let message = Message::assistant(&response).with_model(model_name);
        let _ = event_tx.send(AgentEvent::Done(message.clone()));

        Ok(message)
    }

    // ── Client builders ──────────────────────────────────────────
    //
    // Each provider uses its native Rig client with per-provider config
    // from the providers map. Tool registration is centralized via the
    // `build_and_stream!` macro above.

    fn build_anthropic_client(&self) -> Result<anthropic::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::Anthropic)?;
        let mut builder = anthropic::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::Anthropic) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Anthropic client error: {e}"))
    }

    fn build_openai_client(&self) -> Result<openai::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::OpenAI)?;
        let mut builder = openai::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::OpenAI) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("OpenAI client error: {e}"))
    }

    /// OpenAI-compatible providers use the Chat Completions API (`/chat/completions`)
    /// rather than the Responses API (`/responses`). Most third-party providers
    /// (MiniMax, Together, etc.) only implement the completions endpoint.
    fn build_openai_compatible_client(
        &self,
    ) -> Result<openai::CompletionsClient<OpenAICompatibleHttpClient>> {
        self.build_openai_compatible_client_for(&ProviderKind::OpenAICompatible)
    }

    fn build_openai_compatible_client_for(
        &self,
        kind: &ProviderKind,
    ) -> Result<openai::CompletionsClient<OpenAICompatibleHttpClient>> {
        let configured_api_key = if self.should_skip_default_env_api_key(kind) {
            self.resolve_configured_api_key(kind)?
        } else {
            self.resolve_optional_api_key(kind)?
        };
        let api_key = configured_api_key.as_deref().unwrap_or("con-local-keyless");
        let mut builder = openai::CompletionsClient::builder()
            .api_key(api_key)
            .http_client(OpenAICompatibleHttpClient::new(
                configured_api_key.is_none(),
            ));
        if let Some(url) = self.config.effective_base_url(kind) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("OpenAI-compatible client error: {e}"))
    }

    fn build_chatgpt_client(&self) -> Result<chatgpt::Client> {
        let mut builder = chatgpt::Client::builder();
        if let Some(url) = self.config.effective_base_url(&ProviderKind::ChatGPT) {
            builder = builder.base_url(url);
        }
        let builder =
            if let Some(api_key) = self.resolve_optional_api_key(&ProviderKind::ChatGPT)? {
                builder.api_key(api_key)
            } else {
                let mut builder = builder.oauth();
                if let Some(dir) = oauth_token_dir(&ProviderKind::ChatGPT) {
                    let auth_file = dir.join("auth.json");
                    if let Err(err) = sync_codex_chatgpt_auth(&auth_file) {
                        log::warn!("[provider] Failed to sync Codex ChatGPT auth cache: {err}");
                    }
                    builder = builder.token_dir(dir);
                }
                builder
            };
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("ChatGPT client error: {e}"))
    }

    fn build_github_copilot_client(&self) -> Result<copilot::Client> {
        let mut builder = copilot::Client::builder();
        if let Some(url) = self.config.effective_base_url(&ProviderKind::GitHubCopilot) {
            builder = builder.base_url(url);
        }
        let mut builder =
            if let Some(api_key) = self.resolve_optional_api_key(&ProviderKind::GitHubCopilot)? {
                builder.api_key(api_key)
            } else {
                builder.oauth()
            };
        if let Some(dir) = oauth_token_dir(&ProviderKind::GitHubCopilot) {
            builder = builder.token_dir(dir);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("GitHub Copilot client error: {e}"))
    }

    fn build_minimax_client(&self) -> Result<minimax::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::MiniMax)?;
        let mut builder = minimax::Client::builder().api_key(api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::MiniMax) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("MiniMax client error: {e}"))
    }

    fn build_minimax_anthropic_client(&self) -> Result<minimax::AnthropicClient> {
        let api_key = self.resolve_api_key(&ProviderKind::MiniMaxAnthropic)?;
        let mut builder = minimax::AnthropicClient::builder().api_key(api_key);
        if let Some(url) = self
            .config
            .effective_base_url(&ProviderKind::MiniMaxAnthropic)
        {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("MiniMax Anthropic client error: {e}"))
    }

    fn build_moonshot_client(&self) -> Result<moonshot::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::Moonshot)?;
        let mut builder = moonshot::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::Moonshot) {
            if is_kimi_coding_base_url(url) {
                builder = builder.http_headers(kimi_coding_headers());
            }
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Moonshot client error: {e}"))
    }

    fn build_moonshot_anthropic_client(&self) -> Result<moonshot::AnthropicClient> {
        let api_key = self.resolve_api_key(&ProviderKind::MoonshotAnthropic)?;
        let mut builder = moonshot::AnthropicClient::builder().api_key(api_key);
        if let Some(url) = self
            .config
            .effective_base_url(&ProviderKind::MoonshotAnthropic)
        {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Moonshot Anthropic client error: {e}"))
    }

    fn build_zai_client(&self) -> Result<zai::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::ZAI)?;
        let mut builder = zai::Client::builder().api_key(api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::ZAI) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Z.AI client error: {e}"))
    }

    fn build_zai_anthropic_client(&self) -> Result<zai::AnthropicClient> {
        let api_key = self.resolve_api_key(&ProviderKind::ZAIAnthropic)?;
        let mut builder = zai::AnthropicClient::builder().api_key(api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::ZAIAnthropic) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Z.AI Anthropic client error: {e}"))
    }

    fn build_deepseek_client(&self) -> Result<deepseek::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::DeepSeek)?;
        let mut builder = deepseek::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::DeepSeek) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("DeepSeek client error: {e}"))
    }

    fn build_groq_client(&self) -> Result<groq::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::Groq)?;
        let mut builder = groq::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::Groq) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Groq client error: {e}"))
    }

    fn build_cohere_client(&self) -> Result<cohere::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::Cohere)?;
        let mut builder = cohere::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::Cohere) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Cohere client error: {e}"))
    }

    fn build_gemini_client(&self) -> Result<gemini::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::Gemini)?;
        let mut builder = gemini::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::Gemini) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Gemini client error: {e}"))
    }

    fn build_ollama_client(&self) -> Result<ollama::Client> {
        let mut builder = ollama::Client::builder().api_key(Nothing);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::Ollama) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Ollama client error: {e}"))
    }

    fn build_openrouter_client(&self) -> Result<openrouter::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::OpenRouter)?;
        let mut builder = openrouter::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::OpenRouter) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("OpenRouter client error: {e}"))
    }

    fn build_perplexity_client(&self) -> Result<perplexity::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::Perplexity)?;
        let mut builder = perplexity::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::Perplexity) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Perplexity client error: {e}"))
    }

    fn build_mistral_client(&self) -> Result<mistral::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::Mistral)?;
        let mut builder = mistral::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::Mistral) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Mistral client error: {e}"))
    }

    fn build_together_client(&self) -> Result<together::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::Together)?;
        let mut builder = together::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::Together) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Together client error: {e}"))
    }

    fn build_xai_client(&self) -> Result<xai::Client> {
        let api_key = self.resolve_api_key(&ProviderKind::XAI)?;
        let mut builder = xai::Client::builder().api_key(&api_key);
        if let Some(url) = self.config.effective_base_url(&ProviderKind::XAI) {
            builder = builder.base_url(url);
        }
        builder
            .build()
            .map_err(|e| anyhow::anyhow!("xAI client error: {e}"))
    }

    /// Lightweight completion — no tools, no history, just a simple prompt→response.
    /// Used for shell-suggestion-style quick completions. Hardcodes a
    /// preamble that tells the model it's a shell completion bot.
    pub async fn complete(&self, prompt: &str) -> Result<String> {
        self.complete_with_options(
            prompt,
            "You are a shell command completion assistant. Be extremely concise.",
            100,
        )
        .await
    }

    /// Same plumbing as `complete`, but lets the caller pick its own
    /// preamble + token budget. Use when the default
    /// shell-completion preamble would mislead the model — e.g.
    /// vertical-tabs uses this to ask for a small JSON summary;
    /// the shell-completion preamble fights that prompt and most
    /// providers respond with empty text.
    pub async fn complete_with_options(
        &self,
        prompt: &str,
        preamble: &str,
        max_tokens: u64,
    ) -> Result<String> {
        use futures::StreamExt;
        use rig::agent::MultiTurnStreamItem;
        use rig::streaming::{StreamedAssistantContent, StreamingPrompt};

        let kind = &self.config.provider;

        // Drive completion via the streaming API, not the
        // non-streaming `Prompt::prompt` path: rig's openai-
        // compatible non-streaming parser only reads
        // `choices[0].message.content` and ignores `reasoning_content`,
        // so reasoning models that emit their answer into
        // `reasoning_content` (Kimi K2.6, DeepSeek-R1, …) come back
        // empty. The streaming path parses both channels.
        macro_rules! do_complete {
            ($client:expr) => {{
                let mut builder = $client.agent(self.config.effective_model(kind));
                builder = builder.preamble(preamble);
                builder = builder.max_tokens(max_tokens);
                // Apply temperature only when the user explicitly
                // set one — there's no defensible cross-provider
                // default to guess.
                if let Some(temp) = self.config.temperature {
                    builder = builder.temperature(temp);
                }
                let agent = builder.build();
                drive_streaming_completion(&agent, prompt).await
            }};
        }

        async fn drive_streaming_completion<M>(
            agent: &rig::agent::Agent<M>,
            prompt: &str,
        ) -> Result<String>
        where
            M: rig::completion::CompletionModel + 'static,
            M::StreamingResponse: rig::completion::GetTokenUsage,
        {
            let mut stream = agent.stream_prompt(prompt.to_owned()).await;
            // Most models emit answers to `content`; some reasoning
            // models (Kimi K2.6, DeepSeek-R1 via openai-compatible)
            // emit everything to `reasoning_content` and leave
            // `content` empty. Prefer `content`; fall back to
            // `reasoning_content` only when it's the only output.
            let mut visible_text = String::new();
            let mut reasoning_text = String::new();
            let mut final_response_text: Option<String> = None;
            while let Some(item) = stream.next().await {
                match item {
                    Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => match content {
                        StreamedAssistantContent::Text(text) => {
                            visible_text.push_str(&text.text);
                        }
                        StreamedAssistantContent::Reasoning(reasoning) => {
                            reasoning_text.push_str(&reasoning.display_text());
                        }
                        StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                            reasoning_text.push_str(&reasoning);
                        }
                        // Tool calls / user items aren't relevant
                        // for single-turn structured prompts.
                        _ => {}
                    },
                    Ok(MultiTurnStreamItem::FinalResponse(fin)) => {
                        let resp = fin.output.as_str();
                        if !resp.is_empty() {
                            final_response_text = Some(resp.to_string());
                        }
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        return Err(anyhow::anyhow!("Completion error: {e}"));
                    }
                }
            }
            if let Some(text) = final_response_text {
                return Ok(text);
            }
            if !visible_text.is_empty() {
                return Ok(visible_text);
            }
            if !reasoning_text.is_empty() {
                // Fallback for reasoning-only models (Kimi K2.6
                // etc.). Caller is expected to validate output —
                // genuine chain-of-thought will fail to parse.
                log::debug!(
                    target: "con_agent::provider",
                    "complete_with_options: using reasoning_content as final answer ({} chars) — content was empty",
                    reasoning_text.len(),
                );
                return Ok(reasoning_text);
            }
            Err(anyhow::anyhow!(
                "Completion error: streaming completion produced no visible text; contained no message or tool call (empty)"
            ))
        }

        match *kind {
            ProviderKind::Anthropic => do_complete!(self.build_anthropic_client()?),
            ProviderKind::OpenAI => do_complete!(self.build_openai_client()?),
            ProviderKind::ChatGPT => do_complete!(self.build_chatgpt_client()?),
            ProviderKind::GitHubCopilot => do_complete!(self.build_github_copilot_client()?),
            ProviderKind::OpenAICompatible => {
                do_complete!(self.build_openai_compatible_client()?)
            }
            ProviderKind::MiniMax => do_complete!(self.build_minimax_client()?),
            ProviderKind::MiniMaxAnthropic => do_complete!(self.build_minimax_anthropic_client()?),
            ProviderKind::Moonshot => do_complete!(self.build_moonshot_client()?),
            ProviderKind::MoonshotAnthropic => {
                do_complete!(self.build_moonshot_anthropic_client()?)
            }
            ProviderKind::ZAI => do_complete!(self.build_zai_client()?),
            ProviderKind::ZAIAnthropic => do_complete!(self.build_zai_anthropic_client()?),
            ProviderKind::DeepSeek => do_complete!(self.build_deepseek_client()?),
            ProviderKind::Groq => do_complete!(self.build_groq_client()?),
            ProviderKind::Cohere => do_complete!(self.build_cohere_client()?),
            ProviderKind::Gemini => do_complete!(self.build_gemini_client()?),
            ProviderKind::Ollama => do_complete!(self.build_ollama_client()?),
            ProviderKind::OpenRouter => do_complete!(self.build_openrouter_client()?),
            ProviderKind::Perplexity => do_complete!(self.build_perplexity_client()?),
            ProviderKind::Mistral => do_complete!(self.build_mistral_client()?),
            ProviderKind::Together => do_complete!(self.build_together_client()?),
            ProviderKind::XAI => do_complete!(self.build_xai_client()?),
        }
    }

    /// Resolve API key for a specific provider from the providers map.
    fn resolve_api_key(&self, kind: &ProviderKind) -> Result<String> {
        if let Some(api_key) = self.resolve_optional_api_key(kind)? {
            return Ok(api_key);
        }

        let default_env = kind.default_api_key_env();
        anyhow::bail!(
            "No API key found for {}. Set {} or configure api_key in settings.",
            kind,
            default_env
        );
    }

    fn resolve_optional_api_key(&self, kind: &ProviderKind) -> Result<Option<String>> {
        if let Some(api_key) = self.resolve_configured_api_key(kind)? {
            return Ok(Some(api_key));
        }

        if self.should_skip_default_env_api_key(kind) {
            return Ok(None);
        }

        // 3. Fall back to provider's default env var
        let default_env = kind.default_api_key_env();
        if *kind == ProviderKind::Ollama {
            return Ok(Some(
                std::env::var(default_env).unwrap_or_else(|_| "ollama".into()),
            ));
        }

        Ok(std::env::var(default_env).ok())
    }

    fn should_skip_default_env_api_key(&self, kind: &ProviderKind) -> bool {
        *kind == ProviderKind::OpenAICompatible
            && self
                .config
                .effective_base_url(kind)
                .as_deref()
                .is_some_and(is_local_openai_compatible_base_url)
    }

    fn resolve_configured_api_key(&self, kind: &ProviderKind) -> Result<Option<String>> {
        Ok(configured_api_key_value(&self.config, kind))
    }
}

/// Consume a streaming response, accumulating the full text.
/// Hook callbacks (on_text_delta, on_tool_call, on_tool_result) fire
/// as the stream is consumed — this function just collects the result.
///
/// Emits `ThinkingDelta` events for extended thinking/reasoning blocks,
/// allowing the UI to display the model's reasoning process.
///
/// Checks the cancellation flag between stream items. When cancelled,
/// returns the partial response accumulated so far.
///
/// **Important:** We break on `FinalResponse` rather than waiting for
/// the stream to yield `None`. Rig's `async_stream` generator may not
/// terminate promptly after yielding `FinalResponse` (tracing
/// instrumentation, async cleanup), causing an indefinite hang.
async fn consume_stream<R: Send + 'static>(
    mut stream: StreamingResult<R>,
    event_tx: &Sender<AgentEvent>,
    cancelled: &AtomicBool,
) -> Result<String> {
    let stream_start = std::time::Instant::now();
    let mut tool_call_count = 0u32;
    let mut response_text = String::new();
    let mut think_parser = ThinkTagStreamParser::default();
    // Track whether we received streaming reasoning deltas.
    // If so, the final Reasoning block is redundant (it contains the same text).
    let mut had_reasoning_deltas = false;
    let mut had_embedded_think_reasoning = false;

    while let Some(item) = stream.next().await {
        if cancelled.load(Ordering::Relaxed) {
            log::info!("[agent] Stream cancelled by user");
            break;
        }
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => match content {
                StreamedAssistantContent::Text(text) => {
                    had_embedded_think_reasoning |= apply_stream_text_chunk(
                        &mut think_parser,
                        &mut response_text,
                        event_tx,
                        &text.text,
                        !had_reasoning_deltas,
                    );
                }
                StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                    if had_embedded_think_reasoning {
                        continue;
                    }
                    had_reasoning_deltas = true;
                    let _ = event_tx.send(AgentEvent::ThinkingDelta(reasoning));
                }
                StreamedAssistantContent::Reasoning(reasoning) => {
                    if had_embedded_think_reasoning {
                        continue;
                    }
                    // Only emit if we didn't get streaming deltas (avoids duplication).
                    // Some providers send only the full block without deltas.
                    if !had_reasoning_deltas {
                        for part in &reasoning.content {
                            if let rig::completion::message::ReasoningContent::Text {
                                text, ..
                            } = part
                            {
                                let _ = event_tx.send(AgentEvent::ThinkingDelta(text.clone()));
                            }
                        }
                    }
                }
                StreamedAssistantContent::ToolCall { tool_call, .. } => {
                    tool_call_count += 1;
                    if tool_call.function.name.is_empty() || tool_call.id.is_empty() {
                        log::warn!(
                            "[agent] Malformed tool call: name={:?} id={:?} args={:?}",
                            tool_call.function.name,
                            tool_call.id,
                            tool_call.function.arguments,
                        );
                    }
                    log::info!(
                        target: "con_agent::flow",
                        "{{\"event\":\"tool_call\",\"name\":\"{}\",\"call_id\":\"{}\",\"args\":{},\"seq\":{}}}",
                        tool_call.function.name,
                        tool_call.id,
                        tool_call.function.arguments,
                        tool_call_count,
                    );
                }
                _ => {}
            },
            Ok(MultiTurnStreamItem::StreamUserItem(user_item)) => {
                // Log the full tool result content for debugging provider compatibility
                let result_preview = format!("{:?}", user_item);
                let preview = truncate_utf8_for_log(&result_preview, 500);
                log::info!(
                    target: "con_agent::flow",
                    "{{\"event\":\"tool_result\",\"elapsed_ms\":{},\"preview\":\"{}\"}}",
                    stream_start.elapsed().as_millis(),
                    preview.replace('"', "\\\"").replace('\n', "\\n"),
                );
            }
            Ok(MultiTurnStreamItem::FinalResponse(final_resp)) => {
                log::info!(
                    "[agent] Stream: final response ({} chars accumulated, {} chars in FinalResponse)",
                    response_text.len(),
                    final_resp.output.len(),
                );
                // Use FinalResponse text if we somehow missed streaming deltas.
                // FinalResponse contains the last turn's text; response_text has
                // all turns. Prefer response_text when available.
                if response_text.is_empty() && !final_resp.output.is_empty() {
                    let _ = apply_stream_text_chunk(
                        &mut think_parser,
                        &mut response_text,
                        event_tx,
                        &final_resp.output,
                        !had_reasoning_deltas,
                    );
                }
                // FinalResponse is the terminal item — do NOT wait for None.
                // Rig's async_stream generator may not yield None promptly after
                // FinalResponse due to tracing instrumentation and async cleanup,
                // causing the stream to hang indefinitely.
                break;
            }
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();

                // MaxTurnError: graceful degradation instead of hard error.
                // Return whatever the agent has produced so far, appending a
                // notice that the turn limit was reached so the user can continue.
                if msg.contains("MaxTurnError") || msg.contains("max turn") {
                    log::warn!("[agent] Reached max turn limit — returning partial response");
                    let notice = "\n\n---\n*Reached the turn limit for this request. You can send another message to continue where I left off.*";
                    response_text.push_str(notice);
                    break;
                }

                log::error!("[agent] Stream error: {e}");
                // Surface actionable error for models that don't support tool use
                if msg.contains("tool use") || msg.contains("tool_use") {
                    return Err(anyhow::anyhow!(
                        "This model does not support tool use. Choose a model that supports function calling (e.g., Claude, GPT-4o, Llama 3.3)."
                    ));
                }
                return Err(anyhow::anyhow!("Streaming error: {e}"));
            }
        }
    }
    let tail = think_parser.finish();
    if !tail.visible.is_empty() {
        response_text.push_str(&tail.visible);
    }
    if !had_reasoning_deltas && !tail.reasoning.is_empty() {
        let _ = event_tx.send(AgentEvent::ThinkingDelta(tail.reasoning));
    }
    log::info!(
        target: "con_agent::flow",
        "{{\"event\":\"stream_end\",\"chars\":{},\"tool_calls\":{},\"elapsed_ms\":{}}}",
        response_text.len(),
        tool_call_count,
        stream_start.elapsed().as_millis(),
    );
    log::info!(
        "[agent] Stream consumption complete: {} chars",
        response_text.len(),
    );
    Ok(response_text)
}
