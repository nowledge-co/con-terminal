use super::*;
use serde_json::json;

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("con-subscription-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn current_catalog_fixture_exposes_astra_and_only_selectable_wire_capabilities() {
    let raw = serde_json::from_str(include_str!("fixtures/catalog-0.155.0.json")).unwrap();
    let catalog = Catalog::parse(&raw, "fixture-account".into(), None).unwrap();
    assert_eq!(
        catalog.model_ids(),
        [
            "gpt-6-astra",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5"
        ]
    );
    let astra = &catalog.models[0];
    assert_eq!(
        astra.reasoning_efforts,
        [
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Xhigh
        ]
    );
    assert_eq!(astra.input_modalities, ["text", "image"]);
}

#[test]
fn catalog_preserves_capabilities_and_filters_hidden_and_unknown_efforts() {
    let raw = json!({"models": [
        {"slug":"gpt-6-astra", "display_name":"Astra", "context_window":272000,
         "input_modalities":["text", "image"], "default_reasoning_level":"high",
         "supported_reasoning_levels":[{"effort":"high"}, {"effort":"future-mode"}, "xhigh"]},
        {"slug":"hidden", "visibility":"hide"}, {"slug":"gpt-6-astra"}, {"slug":""}
    ]});
    let catalog = Catalog::parse(&raw, "account-a".into(), None).unwrap();
    assert_eq!(catalog.model_ids(), ["gpt-6-astra"]);
    let model = &catalog.models[0];
    assert_eq!(
        model.reasoning_efforts,
        [ReasoningEffort::High, ReasoningEffort::Xhigh]
    );
    assert_eq!(model.default_reasoning_effort, Some(ReasoningEffort::High));
    assert_eq!(model.context_window, Some(272000));
    assert_eq!(model.input_modalities, ["text", "image"]);
}

#[test]
fn catalog_persists_and_is_scoped_to_account_and_endpoint() {
    let dir = TempDir::new();
    let catalog = Catalog::parse(
        &json!({"models":[{"slug":"account-model"}]}),
        "account-a".into(),
        None,
    )
    .unwrap();
    catalog.save_in(&dir.0).unwrap();
    assert_eq!(
        Catalog::load_in(&dir.0, "account-a", Some(API_BASE))
            .unwrap()
            .model_ids(),
        ["account-model"]
    );
    assert!(Catalog::load_in(&dir.0, "account-b", None).is_none());
    assert!(Catalog::load_in(&dir.0, "account-a", Some("https://other.example/codex")).is_none());
    assert!(Catalog::parse(&json!({"models":[]}), "account-a".into(), None).is_err());
    assert!(Catalog::parse(&json!({"error":"unauthorized"}), "account-a".into(), None).is_err());
    assert_eq!(
        Catalog::load_in(&dir.0, "account-a", None)
            .unwrap()
            .model_ids(),
        ["account-model"]
    );
}

#[test]
fn credential_scope_cache_invalidates_on_account_change_and_logout() {
    let dir = TempDir::new();
    let path = dir.0.join("auth.json");
    std::fs::write(
        &path,
        json!({"access_token":"token-a","account_id":"account-a"}).to_string(),
    )
    .unwrap();
    let first = scope_for_file(&path).unwrap();
    assert_eq!(scope_for_file(&path).as_deref(), Some(first.as_str()));
    crate::provider::write_auth_record(
        &path,
        json!({"access_token":"token-b","account_id":"another-account"})
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    let second = scope_for_file(&path).unwrap();
    assert_ne!(first, second);
    // Token rotation in the same account keeps its last-known model catalog.
    crate::provider::write_auth_record(
        &path,
        json!({"access_token":"rotated-token","account_id":"another-account"})
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(scope_for_file(&path).as_deref(), Some(second.as_str()));
    std::fs::remove_file(&path).unwrap();
    assert!(scope_for_file(&path).is_none());
}

#[test]
fn subscription_retirement_is_date_scoped_and_does_not_rewrite_config() {
    let before = chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap();
    let retirement = chrono::NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
    assert_eq!(retired_model_replacement("gpt-5.5", before), None);
    assert_eq!(
        retired_model_replacement("gpt-5.5", retirement),
        Some("gpt-5.6-sol")
    );
    assert_eq!(
        retired_model_replacement("gpt-5.4", before),
        Some("gpt-5.6-terra")
    );
    assert_eq!(retired_model_replacement("custom", retirement), None);
    let mut agent = crate::AgentConfig::default();
    for provider in [ProviderKind::ChatGPT, ProviderKind::OpenAI] {
        agent.providers.set(
            &provider,
            ProviderConfig {
                model: Some("gpt-5.4".into()),
                ..Default::default()
            },
        );
        assert_eq!(agent.effective_model(&provider), "gpt-5.4");
    }
}

#[test]
fn reasoning_parameters_are_valid_for_rig_and_default_is_omitted() {
    let config = ProviderConfig {
        api_key: Some("test-token".into()),
        reasoning_effort: Some(ReasoningEffort::High),
        ..Default::default()
    };
    let params = request_parameters(&config, DEFAULT_MODEL).unwrap().unwrap();
    assert_eq!(params, json!({"reasoning":{"effort":"high"}}));
    let _: rig::providers::openai::responses_api::AdditionalParameters =
        serde_json::from_value(params).unwrap();
    let mut config = config;
    config.reasoning_effort = Some(ReasoningEffort::None);
    assert!(request_parameters(&config, DEFAULT_MODEL).is_err());
    config.reasoning_effort = None;
    assert!(
        request_parameters(&config, DEFAULT_MODEL)
            .unwrap()
            .is_none()
    );
}

#[test]
fn discovery_and_chat_keep_legacy_direct_token_overrides() {
    let config = ProviderConfig {
        api_key_env: Some("legacy-direct-token".into()),
        ..Default::default()
    };
    assert_eq!(
        token_override(&config).as_deref(),
        Some("legacy-direct-token")
    );
    let config = ProviderConfig {
        api_key: Some(" explicit-token ".into()),
        ..config
    };
    assert_eq!(token_override(&config).as_deref(), Some("explicit-token"));
}

#[tokio::test]
async fn missing_credentials_require_settings_instead_of_starting_device_login() {
    let dir = TempDir::new();
    let client = oauth_client(&dir.0.join("auth.json"), None).unwrap();
    let error = client.authorize().await.unwrap_err();
    assert!(error.to_string().contains("Reconnect ChatGPT in Settings"));
}

#[tokio::test]
async fn cached_unexpired_credentials_do_not_require_network() {
    let dir = TempDir::new();
    let path = dir.0.join("auth.json");
    let record = json!({"access_token":"test-access-token", "expires_at":chrono::Utc::now().timestamp()+3600, "account_id":"account-a"});
    std::fs::write(&path, record.to_string()).unwrap();
    let client = oauth_client(&path, Some("http://127.0.0.1:1")).unwrap();
    client.authorize().await.unwrap();
    let auth = read_context(&path).unwrap();
    assert_eq!(auth.access_token, "test-access-token");
    assert_eq!(auth.account_id.as_deref(), Some("account-a"));
}

#[tokio::test]
async fn expired_unrefreshable_token_is_not_accepted_for_discovery() {
    let dir = TempDir::new();
    let path = dir.0.join("auth.json");
    std::fs::write(
        &path,
        json!({"access_token":"expired-token", "expires_at":1}).to_string(),
    )
    .unwrap();
    let client = oauth_client(&path, Some("http://127.0.0.1:1")).unwrap();
    let error = client.authorize().await.unwrap_err().to_string();
    assert!(error.contains("Reconnect ChatGPT in Settings"));
}

#[test]
fn catalog_accepts_legacy_shapes_without_inventing_capabilities() {
    let catalog = Catalog::parse(
        &json!({"data":[{"id":"a"},{"name":"b"}, "c"]}),
        "account".into(),
        None,
    )
    .unwrap();
    assert_eq!(catalog.model_ids(), ["a", "b", "c"]);
    assert!(
        catalog
            .models
            .iter()
            .all(|model| model.reasoning_efforts.is_empty())
    );
}

#[tokio::test]
async fn reasoning_reaches_responses_wire_request_with_subscription_invariants() {
    use rig::{
        OneOrMany,
        client::CompletionClient,
        completion::{CompletionModel, CompletionRequest, Message},
    };
    let http = rig::test_utils::RecordingHttpClient::with_error_response(
        http::StatusCode::BAD_REQUEST,
        "intentional test response",
    );
    let client = chatgpt::Client::builder()
        .api_key(chatgpt::ChatGPTAuth::AccessToken {
            access_token: "test-token".into(),
            account_id: Some("test-account".into()),
        })
        .http_client(http.clone())
        .build()
        .unwrap();
    let config = ProviderConfig {
        api_key: Some("test-token".into()),
        reasoning_effort: Some(ReasoningEffort::High),
        ..Default::default()
    };
    let model = client.completion_model(DEFAULT_MODEL);
    let result = model
        .completion(CompletionRequest {
            model: Some(DEFAULT_MODEL.into()),
            preamble: Some("Help with code".into()),
            chat_history: OneOrMany::one(Message::user("Hello")),
            documents: vec![],
            tools: vec![],
            temperature: Some(0.7),
            max_tokens: Some(100),
            tool_choice: None,
            additional_params: request_parameters(&config, DEFAULT_MODEL).unwrap(),
            output_schema: None,
        })
        .await;
    assert!(result.is_err());
    let requests = http.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.uri, format!("{API_BASE}/responses"));
    assert_eq!(request.headers["ChatGPT-Account-Id"], "test-account");
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["model"], DEFAULT_MODEL);
    assert_eq!(body["reasoning"]["effort"], "high");
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert!(
        body["include"]
            .as_array()
            .unwrap()
            .contains(&json!("reasoning.encrypted_content"))
    );
    assert!(body.get("temperature").is_none());
    assert!(body.get("max_output_tokens").is_none());
}
