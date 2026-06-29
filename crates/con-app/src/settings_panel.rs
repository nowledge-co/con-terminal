use con_agent::provider::{AgentPurpose, ProviderTransport};
use con_agent::{
    OAuthDevicePrompt, ProviderConfig, ProviderKind, SuggestionModelConfig,
    authorize_oauth_provider, oauth_token_dir, read_synced_chatgpt_oauth_access_token,
};
use con_core::{
    Config,
    config::{
        AppearanceConfig, DEFAULT_TERMINAL_FONT_FAMILY, MAX_UI_FONT_SIZE, MIN_UI_FONT_SIZE,
        TabsOrientation, is_gpui_pseudo_font_family, sanitize_terminal_font_family,
    },
};
use futures::{FutureExt, StreamExt};
use gpui::*;

use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::clipboard::Clipboard;
use gpui_component::input::InputState;
use gpui_component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use gpui_component::switch::Switch;
use gpui_component::{ActiveTheme, Disableable, Icon, IndexPath, Sizable as _, input::Input};

use crate::model_registry::ModelRegistry;
use crate::motion::{MotionValue, vertical_reveal_offset};
use crate::ui_scale::ui_density_scale;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

actions!(settings, [ToggleSettings, SaveSettings, DismissSettings]);

/// Emitted when the user selects a different terminal theme for live preview.
pub struct ThemePreview(pub String);

/// Emitted for lightweight appearance changes that should be visible
/// immediately but should not persist/rebuild the full agent config.
pub struct AppearancePreview;

#[derive(Debug, Clone, Default)]
struct ProviderOAuthState {
    in_progress: bool,
    connected: bool,
    prompt: Option<OAuthDevicePrompt>,
    status_message: Option<String>,
    error_message: Option<String>,
}

enum ProviderModelFetchRequest {
    OpenAICompatible {
        base_url: String,
        api_key: Option<String>,
    },
    ChatGPT {
        access_token: String,
        base_url: Option<String>,
    },
}

impl ProviderModelFetchRequest {
    async fn fetch(self) -> anyhow::Result<ProviderModelFetchResult> {
        match self {
            Self::OpenAICompatible { base_url, api_key } => {
                let models =
                    ModelRegistry::fetch_openai_compatible_models(&base_url, api_key.as_deref())
                        .await?;
                Ok(ProviderModelFetchResult::OpenAICompatible { base_url, models })
            }
            Self::ChatGPT {
                access_token,
                base_url,
            } => {
                let models = ModelRegistry::fetch_chatgpt_subscription_models(
                    &access_token,
                    base_url.as_deref(),
                )
                .await?;
                Ok(ProviderModelFetchResult::ChatGPT { models })
            }
        }
    }
}

enum ProviderModelFetchResult {
    OpenAICompatible {
        base_url: String,
        models: Vec<String>,
    },
    ChatGPT {
        models: Vec<String>,
    },
}

impl ProviderModelFetchResult {
    fn models(&self) -> &[String] {
        match self {
            Self::OpenAICompatible { models, .. } | Self::ChatGPT { models } => models,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SettingsSection {
    General,
    Appearance,
    Ai,
    Providers,
    Keys,
}

impl SettingsSection {
    fn label(&self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Ai => "AI",
            Self::Providers => "Providers",
            Self::Keys => "Keys",
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Self::General => "phosphor/sliders.svg",
            Self::Appearance => "phosphor/sun.svg",
            Self::Ai => "phosphor/robot.svg",
            Self::Providers => "phosphor/plugs-connected.svg",
            Self::Keys => "phosphor/keyboard.svg",
        }
    }
}

const ALL_SECTIONS: &[SettingsSection] = &[
    SettingsSection::General,
    SettingsSection::Appearance,
    SettingsSection::Ai,
    SettingsSection::Providers,
    SettingsSection::Keys,
];

pub struct SettingsPanel {
    visible: bool,
    standalone: bool,
    config: Config,
    preview_snapshot: Option<Config>,
    registry: ModelRegistry,
    oauth_runtime: Arc<tokio::runtime::Runtime>,
    focus_handle: FocusHandle,
    active_section: SettingsSection,
    overlay_motion: MotionValue,

    selected_provider: ProviderKind,
    active_provider_select: Entity<SelectState<SearchableVec<String>>>,
    active_model_select: Entity<SelectState<SearchableVec<String>>>,
    model_input: Entity<InputState>,
    model_select: Entity<SelectState<SearchableVec<String>>>,
    endpoint_preset_select: Entity<SelectState<Vec<String>>>,
    api_key_input: Entity<InputState>,
    base_url_input: Entity<InputState>,
    max_tokens_input: Entity<InputState>,
    max_turns_input: Entity<InputState>,
    temperature_input: Entity<InputState>,
    auto_approve: bool,
    ai_purpose_select: Entity<SelectState<Vec<String>>>,

    suggestion_enabled: bool,
    suggestion_provider_select: Entity<SelectState<SearchableVec<String>>>,
    suggestion_model_select: Entity<SelectState<SearchableVec<String>>>,
    oauth_states: HashMap<ProviderKind, ProviderOAuthState>,
    provider_model_fetching: bool,
    provider_model_status: Option<String>,
    provider_model_status_error: bool,

    terminal_font_select: Entity<SelectState<SearchableVec<String>>>,
    ui_font_select: Entity<SelectState<SearchableVec<String>>>,
    cursor_style_select: Entity<SelectState<Vec<String>>>,
    font_size_input: Entity<InputState>,
    ui_font_size_input: Entity<InputState>,
    terminal_opacity_slider: Entity<SliderState>,
    terminal_blur: bool,
    ui_opacity_slider: Entity<SliderState>,
    tab_accent_inactive_alpha_slider: Entity<SliderState>,
    tab_accent_inactive_hover_alpha_slider: Entity<SliderState>,
    background_image_input: Entity<InputState>,
    background_image_opacity_slider: Entity<SliderState>,
    background_image_position_select: Entity<SelectState<Vec<String>>>,
    background_image_fit_select: Entity<SelectState<Vec<String>>>,
    background_image_repeat: bool,
    hide_pane_title_bar: bool,
    save_error: Option<String>,
    save_error_kind: Option<SettingsSaveErrorKind>,
    last_saved_at: Option<std::time::SystemTime>,
    close_confirmation_visible: bool,

    // Theme import
    custom_theme_name_input: Entity<InputState>,
    custom_theme_preview: Option<con_terminal::TerminalTheme>,
    custom_theme_status: Option<String>,

    // Keybindings — which binding is being recorded (field name, e.g. "new_tab")
    recording_key: Option<String>,
    #[cfg(target_os = "macos")]
    recording_resume_keybindings: Option<con_core::config::KeybindingConfig>,

    // Network / proxy
    http_proxy_input: Entity<InputState>,
    https_proxy_input: Entity<InputState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSaveErrorKind {
    KeybindingConflict,
    Other,
}

const SIDEBAR_PROVIDERS: &[ProviderKind] = &[
    ProviderKind::Anthropic,
    ProviderKind::OpenAI,
    ProviderKind::ChatGPT,
    ProviderKind::GitHubCopilot,
    ProviderKind::OpenAICompatible,
    ProviderKind::MiniMax,
    ProviderKind::Moonshot,
    ProviderKind::ZAI,
    ProviderKind::DeepSeek,
    ProviderKind::Groq,
    ProviderKind::Gemini,
    ProviderKind::Ollama,
    ProviderKind::OpenRouter,
    ProviderKind::Mistral,
    ProviderKind::Together,
    ProviderKind::Cohere,
    ProviderKind::Perplexity,
    ProviderKind::XAI,
];

const BACKGROUND_IMAGE_POSITIONS: &[&str] = &[
    "top-left",
    "top-center",
    "top-right",
    "center-left",
    "center",
    "center-right",
    "bottom-left",
    "bottom-center",
    "bottom-right",
];

const BACKGROUND_IMAGE_FITS: &[&str] = &["contain", "cover", "stretch", "none"];
const ENDPOINT_DEFAULT_LABEL: &str = "Provider Default";
const ENDPOINT_CUSTOM_LABEL: &str = "Custom";
const KIMI_CODING_ENDPOINT: EndpointPreset = EndpointPreset {
    label: "Kimi Coding",
    base_url: "https://api.kimi.com/coding/v1",
};

#[derive(Clone, Copy)]
struct EndpointPreset {
    label: &'static str,
    base_url: &'static str,
}

impl SettingsPanel {
    fn clamp_ui_font_size(value: f32) -> f32 {
        value.clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE)
    }

    fn clamp_terminal_opacity(value: f32) -> f32 {
        value.clamp(0.25, 1.0)
    }

    fn clamp_ui_opacity(value: f32) -> f32 {
        value.clamp(0.35, 1.0)
    }

    fn terminal_blur_supported() -> bool {
        !cfg!(target_os = "linux")
    }

    fn effective_terminal_blur(value: bool) -> bool {
        value && Self::terminal_blur_supported()
    }

    fn terminal_opacity_value(&self) -> f32 {
        Self::clamp_terminal_opacity(self.config.appearance.terminal_opacity)
    }

    fn ui_opacity_value(&self) -> f32 {
        Self::clamp_ui_opacity(self.config.appearance.ui_opacity)
    }

    fn clamp_tab_accent_inactive_alpha(value: f32) -> f32 {
        if value.is_finite() {
            value.clamp(
                AppearanceConfig::MIN_TAB_ACCENT_ALPHA,
                AppearanceConfig::MAX_TAB_ACCENT_INACTIVE_ALPHA,
            )
        } else {
            crate::tab_colors::TAB_ACCENT_INACTIVE_ALPHA
        }
    }

    fn clamp_tab_accent_inactive_hover_alpha(value: f32, inactive: f32) -> f32 {
        let value = if value.is_finite() {
            value.clamp(
                AppearanceConfig::MIN_TAB_ACCENT_ALPHA,
                AppearanceConfig::MAX_TAB_ACCENT_INACTIVE_HOVER_ALPHA,
            )
        } else {
            crate::tab_colors::TAB_ACCENT_INACTIVE_HOVER_ALPHA
        };
        value.max(inactive)
    }

    fn tab_accent_inactive_alpha_value(&self) -> f32 {
        Self::clamp_tab_accent_inactive_alpha(self.config.appearance.tab_accent_inactive_alpha)
    }

    fn tab_accent_inactive_hover_alpha_value(&self) -> f32 {
        Self::clamp_tab_accent_inactive_hover_alpha(
            self.config.appearance.tab_accent_inactive_hover_alpha,
            self.tab_accent_inactive_alpha_value(),
        )
    }

    fn clamp_background_image_opacity(value: f32) -> f32 {
        value.clamp(0.0, 1.0)
    }

    fn background_image_opacity_value(&self) -> f32 {
        Self::clamp_background_image_opacity(self.config.appearance.background_image_opacity)
    }

    fn make_string_select(
        options: &[&str],
        current_value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SelectState<Vec<String>>> {
        let items: Vec<String> = options.iter().map(|value| (*value).to_string()).collect();
        let selected_index = items
            .iter()
            .position(|item| item == current_value)
            .map(IndexPath::new);
        cx.new(|cx| SelectState::new(items, selected_index, window, cx))
    }

    fn cursor_style_label(value: &str) -> &'static str {
        match value.trim().to_ascii_lowercase().as_str() {
            "block" => "Block",
            "underline" => "Underline",
            "block_hollow" | "block-hollow" | "hollow" => "Hollow Block",
            _ => "Bar",
        }
    }

    fn cursor_style_from_label(label: &str) -> &'static str {
        match label {
            "Block" => "block",
            "Underline" => "underline",
            "Hollow Block" => "block_hollow",
            _ => "bar",
        }
    }

    fn prepare_terminal_font_families(
        config: &Config,
        mut font_families: Vec<String>,
    ) -> Vec<String> {
        font_families.sort_by_key(|name| name.to_lowercase());
        font_families.dedup();

        let mut preferred = Vec::new();
        let sanitized_terminal_family = sanitize_terminal_font_family(&config.terminal.font_family);
        for family in [
            DEFAULT_TERMINAL_FONT_FAMILY,
            sanitized_terminal_family.as_str(),
        ] {
            if !family.is_empty() && !preferred.iter().any(|existing| existing == family) {
                preferred.push(family.to_string());
            }
        }

        for family in font_families {
            if !is_gpui_pseudo_font_family(&family)
                && !preferred.iter().any(|existing| existing == &family)
            {
                preferred.push(family);
            }
        }
        preferred
    }

    fn prepare_ui_font_families(config: &Config, mut font_families: Vec<String>) -> Vec<String> {
        font_families.sort_by_key(|name| name.to_lowercase());
        font_families.dedup();

        let mut preferred = Vec::new();
        for family in [
            ".SystemUIFont",
            config.appearance.ui_font_family.as_str(),
            DEFAULT_TERMINAL_FONT_FAMILY,
        ] {
            if !family.is_empty() && !preferred.iter().any(|existing| existing == family) {
                preferred.push(family.to_string());
            }
        }

        for family in font_families {
            if !preferred.iter().any(|existing| existing == &family) {
                preferred.push(family);
            }
        }
        preferred
    }

    fn provider_api_key_placeholder(provider: &ProviderKind) -> &'static str {
        match provider {
            ProviderKind::ChatGPT | ProviderKind::GitHubCopilot => "Override for advanced setups",
            ProviderKind::OpenAICompatible => "Optional, or OPENAI_API_KEY",
            _ => "sk-.. or OPENAI_API_KEY",
        }
    }

    fn provider_api_key_label(provider: &ProviderKind) -> &'static str {
        match provider {
            ProviderKind::ChatGPT => "Access Token Override",
            ProviderKind::GitHubCopilot => "API Key Override",
            _ => "API Key",
        }
    }

    fn provider_api_key_hint(provider: &ProviderKind) -> &'static str {
        match provider {
            ProviderKind::ChatGPT => "Leave blank for ChatGPT OAuth.",
            ProviderKind::GitHubCopilot => "Leave blank for GitHub OAuth.",
            ProviderKind::OpenAICompatible => {
                "Optional for local endpoints; use a key or env var when required."
            }
            _ => "Key or env var name",
        }
    }

    fn provider_has_oauth(provider: &ProviderKind) -> bool {
        matches!(
            provider,
            ProviderKind::ChatGPT | ProviderKind::GitHubCopilot
        )
    }

    fn provider_oauth_label(provider: &ProviderKind) -> Option<&'static str> {
        match provider {
            ProviderKind::ChatGPT => Some("ChatGPT Subscription"),
            ProviderKind::GitHubCopilot => Some("GitHub Copilot"),
            _ => None,
        }
    }

    fn provider_oauth_button_label(provider: &ProviderKind) -> Option<&'static str> {
        match provider {
            ProviderKind::ChatGPT => Some("Auth ChatGPT"),
            ProviderKind::GitHubCopilot => Some("Auth GitHub"),
            _ => None,
        }
    }

    fn sidebar_provider_kind(provider: &ProviderKind) -> ProviderKind {
        match provider {
            ProviderKind::MiniMaxAnthropic => ProviderKind::MiniMax,
            ProviderKind::MoonshotAnthropic => ProviderKind::Moonshot,
            ProviderKind::ZAIAnthropic => ProviderKind::ZAI,
            _ => provider.clone(),
        }
    }

    fn protocol_pair(provider: &ProviderKind) -> Option<(ProviderKind, ProviderKind)> {
        match Self::sidebar_provider_kind(provider) {
            ProviderKind::MiniMax => Some((ProviderKind::MiniMax, ProviderKind::MiniMaxAnthropic)),
            ProviderKind::Moonshot => {
                Some((ProviderKind::Moonshot, ProviderKind::MoonshotAnthropic))
            }
            ProviderKind::ZAI => Some((ProviderKind::ZAI, ProviderKind::ZAIAnthropic)),
            _ => None,
        }
    }

    fn uses_anthropic_protocol(provider: &ProviderKind) -> bool {
        matches!(
            provider,
            ProviderKind::MiniMaxAnthropic
                | ProviderKind::MoonshotAnthropic
                | ProviderKind::ZAIAnthropic
        )
    }

    fn protocol_switch_label(provider: &ProviderKind) -> Option<&'static str> {
        Self::protocol_pair(provider).map(|_| "Anthropic API")
    }

    fn protocol_switch_hint(provider: &ProviderKind) -> Option<&'static str> {
        Self::protocol_pair(provider).map(|_| "OpenAI- or Anthropic-compatible API")
    }

    fn provider_icon_path(provider: &ProviderKind) -> &'static str {
        match Self::sidebar_provider_kind(provider) {
            ProviderKind::Anthropic => "providers/anthropic.svg",
            ProviderKind::OpenAI => "providers/openai.svg",
            ProviderKind::ChatGPT => "providers/openai.svg",
            ProviderKind::GitHubCopilot => "providers/githubcopilot.svg",
            ProviderKind::OpenAICompatible => "phosphor/plugs-connected.svg",
            ProviderKind::MiniMax => "providers/minimax.svg",
            ProviderKind::Moonshot => "providers/moonshot.svg",
            ProviderKind::ZAI => "providers/zai.svg",
            ProviderKind::DeepSeek => "providers/deepseek.svg",
            ProviderKind::Groq => "providers/groq.svg",
            ProviderKind::Gemini => "providers/gemini.svg",
            ProviderKind::Ollama => "providers/ollama.svg",
            ProviderKind::OpenRouter => "providers/openrouter.svg",
            ProviderKind::Mistral => "providers/mistral.svg",
            ProviderKind::Together => "providers/together.svg",
            ProviderKind::Cohere => "providers/cohere.svg",
            ProviderKind::Perplexity => "providers/perplexity.svg",
            ProviderKind::XAI => "providers/xai.svg",
            _ => "phosphor/plugs-connected.svg",
        }
    }

    fn provider_config_has_auth_signal(provider: &ProviderKind, config: &ProviderConfig) -> bool {
        let has_api_key = config
            .api_key
            .as_ref()
            .is_some_and(|v| !v.trim().is_empty())
            || config
                .api_key_env
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty());
        if has_api_key {
            return true;
        }

        *provider == ProviderKind::OpenAICompatible
            && config
                .base_url
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
    }

    fn provider_is_configured(&self, provider: &ProviderKind, cx: &App) -> bool {
        let sidebar_provider = Self::sidebar_provider_kind(provider);
        if Self::provider_oauth_cache_present(&sidebar_provider) {
            return true;
        }
        if self
            .oauth_state(&sidebar_provider)
            .is_some_and(|state| state.connected || state.in_progress)
        {
            return true;
        }

        let has_config = |kind: &ProviderKind| {
            if *kind == self.selected_provider {
                let current = self.read_provider_inputs(cx);
                return Self::provider_config_has_auth_signal(kind, &current);
            }
            self.config
                .agent
                .providers
                .get(kind)
                .is_some_and(|config| Self::provider_config_has_auth_signal(kind, config))
        };

        has_config(&sidebar_provider)
            || match sidebar_provider {
                ProviderKind::MiniMax => has_config(&ProviderKind::MiniMaxAnthropic),
                ProviderKind::Moonshot => has_config(&ProviderKind::MoonshotAnthropic),
                ProviderKind::ZAI => has_config(&ProviderKind::ZAIAnthropic),
                _ => false,
            }
    }

    fn provider_oauth_cache_present(provider: &ProviderKind) -> bool {
        let Some(dir) = oauth_token_dir(provider) else {
            return false;
        };
        match provider {
            ProviderKind::ChatGPT => dir.join("auth.json").is_file(),
            ProviderKind::GitHubCopilot => {
                dir.join("access-token").is_file() || dir.join("api-key.json").is_file()
            }
            _ => false,
        }
    }

    fn provider_oauth_ready(&self, provider: &ProviderKind) -> bool {
        let sidebar_provider = Self::sidebar_provider_kind(provider);
        Self::provider_oauth_cache_present(&sidebar_provider)
            || self
                .oauth_state(&sidebar_provider)
                .is_some_and(|state| state.connected || state.in_progress)
    }

    fn sidebar_selection_target(
        clicked_provider: &ProviderKind,
        current_selected: &ProviderKind,
    ) -> ProviderKind {
        if Self::sidebar_provider_kind(current_selected) == *clicked_provider
            && Self::uses_anthropic_protocol(current_selected)
        {
            current_selected.clone()
        } else {
            clicked_provider.clone()
        }
    }

    fn protocol_toggled_provider(
        provider: &ProviderKind,
        use_anthropic: bool,
    ) -> Option<ProviderKind> {
        Self::protocol_pair(provider).map(|(openai_kind, anthropic_kind)| {
            if use_anthropic {
                anthropic_kind
            } else {
                openai_kind
            }
        })
    }

    fn provider_for_transport(
        provider: &ProviderKind,
        transport: ProviderTransport,
    ) -> Option<ProviderKind> {
        Self::protocol_pair(provider).map(|(openai_kind, anthropic_kind)| match transport {
            ProviderTransport::OpenAI => openai_kind,
            ProviderTransport::Anthropic => anthropic_kind,
        })
    }

    fn preferred_sidebar_provider(
        config: &Config,
        clicked_provider: &ProviderKind,
    ) -> ProviderKind {
        let sidebar_provider = Self::sidebar_provider_kind(clicked_provider);
        let transport = config.agent.provider_transport_for(&sidebar_provider);
        Self::provider_for_transport(
            &sidebar_provider,
            transport.unwrap_or(ProviderTransport::OpenAI),
        )
        .unwrap_or(sidebar_provider)
    }

    fn provider_for_saved_transport(config: &Config, provider: &ProviderKind) -> ProviderKind {
        let sidebar_provider = Self::sidebar_provider_kind(provider);
        let Some(transport) = config.agent.provider_transport_for(&sidebar_provider) else {
            return provider.clone();
        };

        Self::provider_for_transport(&sidebar_provider, transport).unwrap_or(provider.clone())
    }

    fn normalize_active_provider_for_saved_transport(&mut self) {
        self.config.agent.provider =
            Self::provider_for_saved_transport(&self.config, &self.config.agent.provider);
    }

    fn set_active_provider_if_tracking(
        &mut self,
        source_provider: &ProviderKind,
        target_provider: &ProviderKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if Self::sidebar_provider_kind(&self.config.agent.provider)
            != Self::sidebar_provider_kind(source_provider)
        {
            return;
        }

        self.config.agent.provider = target_provider.clone();
        self.active_model_select =
            Self::make_active_model_select(&self.config, &self.registry, window, cx);
    }

    fn oauth_state(&self, provider: &ProviderKind) -> Option<&ProviderOAuthState> {
        self.oauth_states.get(provider)
    }

    fn oauth_state_mut(&mut self, provider: &ProviderKind) -> &mut ProviderOAuthState {
        self.oauth_states.entry(provider.clone()).or_default()
    }

    fn start_provider_oauth(
        &mut self,
        provider: ProviderKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.oauth_state_mut(&provider);
        state.in_progress = true;
        state.connected = false;
        state.prompt = None;
        state.status_message = Some("Waiting for device authorization…".to_string());
        state.error_message = None;
        cx.notify();

        let runtime = self.oauth_runtime.clone();
        cx.spawn_in(window, async move |this, window| {
            let (prompt_tx, prompt_rx) = futures::channel::mpsc::unbounded::<OAuthDevicePrompt>();
            let (result_tx, result_rx) = futures::channel::oneshot::channel::<Result<(), String>>();

            runtime.spawn({
                let provider = provider.clone();
                async move {
                    let result = authorize_oauth_provider(provider, move |prompt| {
                        let _ = prompt_tx.unbounded_send(prompt);
                    })
                    .await
                    .map_err(|err| err.to_string());

                    let _ = result_tx.send(result);
                }
            });

            let mut prompt_rx = prompt_rx.fuse();
            let mut result_rx = result_rx.fuse();

            loop {
                futures::select! {
                    maybe_prompt = prompt_rx.next() => {
                        let Some(prompt) = maybe_prompt else {
                            continue;
                        };
                        let verification_uri = prompt.verification_uri.clone();
                        let _ = window.update(|window, cx| {
                            let _ = this.update(cx, |panel, cx| {
                                let state = panel.oauth_state_mut(&provider);
                                state.prompt = Some(prompt);
                                state.status_message = Some("Finish sign-in in your browser. Con will continue automatically.".to_string());
                                state.error_message = None;
                                cx.open_url(&verification_uri);
                                cx.notify();
                            });
                            let _ = window;
                        });
                    }
                    result = result_rx => {
                        let _ = window.update(|window, cx| {
                            let _ = this.update(cx, |panel, cx| {
                                let state = panel.oauth_state_mut(&provider);
                                state.in_progress = false;
                                match result {
                                    Ok(Ok(())) => {
                                        state.connected = true;
                                        state.prompt = None;
                                        state.status_message = Some("Authorized and stored in Con’s auth cache.".to_string());
                                        state.error_message = None;
                                    }
                                    Ok(Err(err)) => {
                                        state.connected = false;
                                        state.error_message = Some(err);
                                        state.status_message = None;
                                    }
                                    Err(_) => {
                                        state.connected = false;
                                        state.error_message = Some("OAuth flow ended unexpectedly.".to_string());
                                        state.status_message = None;
                                    }
                                }
                                cx.notify();
                            });
                            let _ = window;
                        });
                        break;
                    }
                }
            }
        })
        .detach();
    }

    fn provider_base_url_hint(provider: &ProviderKind) -> &'static str {
        if Self::provider_endpoint_presets(provider).is_empty() {
            "Blank for the default"
        } else {
            "Blank for provider default"
        }
    }

    fn provider_endpoint_presets(provider: &ProviderKind) -> &'static [EndpointPreset] {
        match provider {
            ProviderKind::MiniMax => &[
                EndpointPreset {
                    label: "Global",
                    base_url: "https://api.minimax.io/v1",
                },
                EndpointPreset {
                    label: "China",
                    base_url: "https://api.minimaxi.com/v1",
                },
            ],
            ProviderKind::MiniMaxAnthropic => &[
                EndpointPreset {
                    label: "Global",
                    base_url: "https://api.minimax.io/anthropic",
                },
                EndpointPreset {
                    label: "China",
                    base_url: "https://api.minimaxi.com/anthropic",
                },
            ],
            ProviderKind::Moonshot => &[
                EndpointPreset {
                    label: "Global",
                    base_url: "https://api.moonshot.ai/v1",
                },
                EndpointPreset {
                    label: "China",
                    base_url: "https://api.moonshot.cn/v1",
                },
                KIMI_CODING_ENDPOINT,
            ],
            ProviderKind::MoonshotAnthropic => &[EndpointPreset {
                label: "Global",
                base_url: "https://api.moonshot.ai/anthropic",
            }],
            ProviderKind::ZAI => &[
                EndpointPreset {
                    label: "General",
                    base_url: "https://api.z.ai/api/paas/v4",
                },
                EndpointPreset {
                    label: "Coding",
                    base_url: "https://api.z.ai/api/coding/paas/v4",
                },
            ],
            ProviderKind::ZAIAnthropic => &[EndpointPreset {
                label: "Anthropic",
                base_url: "https://api.z.ai/api/anthropic",
            }],
            _ => &[],
        }
    }

    fn extra_endpoint_presets(provider: &ProviderKind) -> &'static [EndpointPreset] {
        match provider {
            // Kimi Coding is OpenAI-compatible only. Show it while the Moonshot
            // card is in Anthropic mode and switch back to OpenAI when selected.
            ProviderKind::MoonshotAnthropic => &[KIMI_CODING_ENDPOINT],
            _ => &[],
        }
    }

    fn endpoint_options(provider: &ProviderKind) -> Vec<String> {
        let presets = Self::provider_endpoint_presets(provider);
        let extra_presets = Self::extra_endpoint_presets(provider);
        if presets.is_empty() && extra_presets.is_empty() {
            return vec![ENDPOINT_DEFAULT_LABEL.to_string()];
        }

        let mut options = Vec::with_capacity(presets.len() + extra_presets.len() + 2);
        options.push(ENDPOINT_DEFAULT_LABEL.to_string());
        options.extend(presets.iter().map(|preset| preset.label.to_string()));
        for preset in extra_presets {
            if !options.iter().any(|option| option == preset.label) {
                options.push(preset.label.to_string());
            }
        }
        options.push(ENDPOINT_CUSTOM_LABEL.to_string());
        options
    }

    fn endpoint_preset_for_label(
        provider: &ProviderKind,
        label: &str,
    ) -> Option<(ProviderKind, EndpointPreset)> {
        if let Some(preset) = Self::provider_endpoint_presets(provider)
            .iter()
            .copied()
            .find(|preset| preset.label == label)
        {
            return Some((provider.clone(), preset));
        }

        Self::extra_endpoint_presets(provider)
            .iter()
            .copied()
            .find(|preset| preset.label == label)
            .and_then(|preset| match provider {
                ProviderKind::MoonshotAnthropic => Some((ProviderKind::Moonshot, preset)),
                _ => None,
            })
    }

    fn endpoint_label_for_base_url(
        provider: &ProviderKind,
        base_url: Option<&str>,
    ) -> &'static str {
        let Some(base_url) = base_url.map(str::trim).filter(|value| !value.is_empty()) else {
            return ENDPOINT_DEFAULT_LABEL;
        };

        for preset in Self::provider_endpoint_presets(provider) {
            if preset.base_url == base_url {
                return preset.label;
            }
        }
        for preset in Self::extra_endpoint_presets(provider) {
            if preset.base_url == base_url {
                return preset.label;
            }
        }

        ENDPOINT_CUSTOM_LABEL
    }

    fn mapped_protocol_base_url(
        source_provider: &ProviderKind,
        target_provider: &ProviderKind,
        source_base_url: Option<&str>,
    ) -> Option<String> {
        let source_label = Self::endpoint_label_for_base_url(source_provider, source_base_url);
        if matches!(source_label, ENDPOINT_DEFAULT_LABEL | ENDPOINT_CUSTOM_LABEL) {
            let target_presets = Self::provider_endpoint_presets(target_provider);
            return (target_presets.len() == 1).then(|| target_presets[0].base_url.to_string());
        }

        if let Some(preset) = Self::provider_endpoint_presets(target_provider)
            .iter()
            .find(|preset| preset.label == source_label)
        {
            return Some(preset.base_url.to_string());
        }

        let target_presets = Self::provider_endpoint_presets(target_provider);
        (target_presets.len() == 1).then(|| target_presets[0].base_url.to_string())
    }

    fn source_endpoint_is_named_preset(
        source_provider: &ProviderKind,
        source_base_url: Option<&str>,
    ) -> bool {
        !matches!(
            Self::endpoint_label_for_base_url(source_provider, source_base_url),
            ENDPOINT_DEFAULT_LABEL | ENDPOINT_CUSTOM_LABEL
        )
    }

    fn seed_protocol_variant_config(
        source_provider: &ProviderKind,
        target_provider: &ProviderKind,
        source: &ProviderConfig,
        target: &ProviderConfig,
    ) -> ProviderConfig {
        let mut seeded = target.clone();

        if seeded.model.is_none() {
            seeded.model = source.model.clone();
        }
        if seeded.api_key.is_none() {
            seeded.api_key = source.api_key.clone();
        }
        if seeded.api_key_env.is_none() {
            seeded.api_key_env = source.api_key_env.clone();
        }
        if seeded.max_tokens.is_none() {
            seeded.max_tokens = source.max_tokens;
        }
        let target_base_url_is_empty = seeded
            .base_url
            .as_deref()
            .is_none_or(|value| value.trim().is_empty());
        let mapped_base_url = Self::mapped_protocol_base_url(
            source_provider,
            target_provider,
            source.base_url.as_deref(),
        );
        if let Some(mapped_base_url) = mapped_base_url
            && (target_base_url_is_empty
                || Self::source_endpoint_is_named_preset(
                    source_provider,
                    source.base_url.as_deref(),
                ))
        {
            seeded.base_url = Some(mapped_base_url);
        }

        seeded
    }

    fn make_endpoint_preset_select(
        provider: &ProviderKind,
        current_base_url: &Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SelectState<Vec<String>>> {
        let options = Self::endpoint_options(provider);
        let selected_index = options
            .iter()
            .position(|item| {
                item == Self::endpoint_label_for_base_url(provider, current_base_url.as_deref())
            })
            .map(IndexPath::new);
        let entity = cx.new(|cx| SelectState::new(options, selected_index, window, cx));
        cx.subscribe_in(
            &entity,
            window,
            |this, _, ev: &SelectEvent<Vec<String>>, window, cx| {
                let SelectEvent::Confirm(Some(value)) = ev else {
                    return;
                };

                match value.as_str() {
                    ENDPOINT_DEFAULT_LABEL => {
                        this.base_url_input
                            .update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    ENDPOINT_CUSTOM_LABEL => {}
                    _ => {
                        if let Some((target_provider, preset)) =
                            Self::endpoint_preset_for_label(&this.selected_provider, value.as_str())
                        {
                            if target_provider != this.selected_provider {
                                let source_provider = this.selected_provider.clone();
                                let source_config = this.read_provider_inputs(cx);
                                this.config
                                    .agent
                                    .providers
                                    .set(&source_provider, source_config.clone());

                                let target_config =
                                    this.config.agent.providers.get_or_default(&target_provider);
                                let mut seeded_target = Self::seed_protocol_variant_config(
                                    &source_provider,
                                    &target_provider,
                                    &source_config,
                                    &target_config,
                                );
                                seeded_target.base_url = Some(preset.base_url.to_string());
                                this.config
                                    .agent
                                    .providers
                                    .set(&target_provider, seeded_target);
                                this.config.agent.set_provider_transport(
                                    &target_provider,
                                    Some(ProviderTransport::OpenAI),
                                );
                                this.set_active_provider_if_tracking(
                                    &source_provider,
                                    &target_provider,
                                    window,
                                    cx,
                                );
                                this.transition_provider(target_provider, window, cx);
                                return;
                            }

                            this.base_url_input.update(cx, |input, cx| {
                                input.set_value(preset.base_url, window, cx);
                            });
                        }
                    }
                }

                cx.notify();
            },
        )
        .detach();
        entity
    }

    fn sync_provider_placeholders(
        &self,
        provider: &ProviderKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.api_key_input.update(cx, |input, cx| {
            input.set_placeholder(Self::provider_api_key_placeholder(provider), window, cx);
        });
        self.base_url_input.update(cx, |input, cx| {
            input.set_placeholder("Default endpoint", window, cx);
        });
    }

    fn make_searchable_string_select(
        options: &[String],
        current_value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SelectState<SearchableVec<String>>> {
        let items = SearchableVec::new(options.to_vec());
        let selected_index = options
            .iter()
            .position(|item| item == current_value)
            .map(IndexPath::new);
        cx.new(|cx| SelectState::new(items, selected_index, window, cx).searchable(true))
    }

    fn suggestion_provider_options() -> Vec<String> {
        let mut options = vec!["Same as active provider".to_string()];
        options.extend(
            SIDEBAR_PROVIDERS
                .iter()
                .map(|provider| provider_label(provider).to_string()),
        );
        options
    }

    fn suggestion_provider_label(provider: Option<&ProviderKind>) -> String {
        provider
            .map(|provider| provider_label(&Self::sidebar_provider_kind(provider)))
            .unwrap_or("Same as active provider")
            .to_string()
    }

    fn suggestion_provider_from_label(label: &str) -> Option<ProviderKind> {
        if label == "Same as active provider" {
            return None;
        }

        SIDEBAR_PROVIDERS
            .iter()
            .find(|provider| provider_label(provider) == label)
            .cloned()
    }

    fn effective_suggestion_provider(config: &Config) -> ProviderKind {
        let provider = config
            .agent
            .suggestion_model
            .provider
            .clone()
            .unwrap_or_else(|| config.agent.provider.clone());
        Self::provider_for_saved_transport(config, &provider)
    }

    fn provider_options() -> Vec<String> {
        SIDEBAR_PROVIDERS
            .iter()
            .map(|provider| provider_label(provider).to_string())
            .collect()
    }

    fn ai_purpose_options() -> &'static [&'static str] {
        &["Build", "Explain", "Operate"]
    }

    fn ai_purpose_label(purpose: AgentPurpose) -> &'static str {
        match purpose {
            AgentPurpose::Build => "Build",
            AgentPurpose::Explain => "Explain",
            AgentPurpose::Operate => "Operate",
        }
    }

    fn ai_purpose_from_label(label: &str) -> AgentPurpose {
        match label {
            "Explain" => AgentPurpose::Explain,
            "Operate" => AgentPurpose::Operate,
            _ => AgentPurpose::Build,
        }
    }

    fn card_opacity(&self) -> f32 {
        0.74
    }

    pub fn new(
        config: &Config,
        registry: ModelRegistry,
        oauth_runtime: Arc<tokio::runtime::Runtime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut config = config.clone();
        config.appearance.terminal_blur =
            Self::effective_terminal_blur(config.appearance.terminal_blur);
        let active_provider = Self::provider_for_saved_transport(&config, &config.agent.provider);
        config.agent.provider = active_provider;
        let agent = &config.agent;
        let selected_provider = Self::provider_for_saved_transport(&config, &agent.provider);
        let pc = agent.providers.get_or_default(&selected_provider);
        let active_provider_select = Self::make_searchable_string_select(
            &Self::provider_options(),
            provider_label(&Self::sidebar_provider_kind(&agent.provider)),
            window,
            cx,
        );
        let active_model_select = Self::make_active_model_select(&config, &registry, window, cx);

        let model_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_placeholder("Provider default or custom model ID", window, cx);
            s.set_value(&pc.model.clone().unwrap_or_default(), window, cx);
            s
        });
        let model_select = Self::make_model_select(
            &selected_provider,
            &pc.model,
            pc.base_url.as_deref(),
            &registry,
            window,
            cx,
        );
        let endpoint_preset_select =
            Self::make_endpoint_preset_select(&selected_provider, &pc.base_url, window, cx);
        let api_key_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_placeholder(
                Self::provider_api_key_placeholder(&selected_provider),
                window,
                cx,
            );
            let val = pc
                .api_key
                .clone()
                .or_else(|| pc.api_key_env.clone())
                .unwrap_or_default();
            s.set_value(&val, window, cx);
            s
        });
        let base_url_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_placeholder("Default endpoint", window, cx);
            s.set_value(&pc.base_url.clone().unwrap_or_default(), window, cx);
            s
        });
        let max_tokens_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_placeholder("Provider default", window, cx);
            s.set_value(
                &pc.max_tokens.map(|t| t.to_string()).unwrap_or_default(),
                window,
                cx,
            );
            s
        });
        let max_turns_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_placeholder("10", window, cx);
            s.set_value(&agent.max_turns.to_string(), window, cx);
            s
        });
        let temperature_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_placeholder("Provider default", window, cx);
            s.set_value(
                &agent.temperature.map(|t| t.to_string()).unwrap_or_default(),
                window,
                cx,
            );
            s
        });
        let ai_purpose_select = Self::make_string_select(
            Self::ai_purpose_options(),
            Self::ai_purpose_label(agent.purpose),
            window,
            cx,
        );
        let suggestion_provider_select = Self::make_searchable_string_select(
            &Self::suggestion_provider_options(),
            &Self::suggestion_provider_label(agent.suggestion_model.provider.as_ref()),
            window,
            cx,
        );
        let suggestion_model_select =
            Self::make_suggestion_model_select(&config, &registry, window, cx);
        let all_font_families = cx.text_system().all_font_names();
        let terminal_font_families =
            Self::prepare_terminal_font_families(&config, all_font_families.clone());
        let ui_font_families = Self::prepare_ui_font_families(&config, all_font_families);
        let terminal_font_family = sanitize_terminal_font_family(&config.terminal.font_family);
        let terminal_font_select = Self::make_searchable_string_select(
            &terminal_font_families,
            &terminal_font_family,
            window,
            cx,
        );
        let ui_font_select = Self::make_searchable_string_select(
            &ui_font_families,
            &config.appearance.ui_font_family,
            window,
            cx,
        );
        let cursor_style_select = Self::make_string_select(
            &["Bar", "Block", "Underline", "Hollow Block"],
            Self::cursor_style_label(&config.terminal.cursor_style),
            window,
            cx,
        );
        let font_size_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_placeholder("14.0", window, cx);
            s.set_value(&config.terminal.font_size.to_string(), window, cx);
            s
        });
        let ui_font_size_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_placeholder("16.0", window, cx);
            s.set_value(
                &Self::clamp_ui_font_size(config.appearance.ui_font_size).to_string(),
                window,
                cx,
            );
            s
        });
        let terminal_opacity_slider = cx.new(|_| {
            SliderState::new()
                .min(0.25)
                .max(1.0)
                .step(0.01)
                .default_value(Self::clamp_terminal_opacity(
                    config.appearance.terminal_opacity,
                ))
        });
        let ui_opacity_slider = cx.new(|_| {
            SliderState::new()
                .min(0.35)
                .max(1.0)
                .step(0.01)
                .default_value(Self::clamp_ui_opacity(config.appearance.ui_opacity))
        });
        let tab_accent_inactive_alpha_slider = cx.new(|_| {
            SliderState::new()
                .min(AppearanceConfig::MIN_TAB_ACCENT_ALPHA)
                .max(AppearanceConfig::MAX_TAB_ACCENT_INACTIVE_ALPHA)
                .step(0.01)
                .default_value(Self::clamp_tab_accent_inactive_alpha(
                    config.appearance.tab_accent_inactive_alpha,
                ))
        });
        let tab_accent_inactive_hover_alpha_slider = cx.new(|_| {
            let inactive =
                Self::clamp_tab_accent_inactive_alpha(config.appearance.tab_accent_inactive_alpha);
            SliderState::new()
                .min(AppearanceConfig::MIN_TAB_ACCENT_ALPHA)
                .max(AppearanceConfig::MAX_TAB_ACCENT_INACTIVE_HOVER_ALPHA)
                .step(0.01)
                .default_value(Self::clamp_tab_accent_inactive_hover_alpha(
                    config.appearance.tab_accent_inactive_hover_alpha,
                    inactive,
                ))
        });
        let background_image_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_placeholder("~/Pictures/wallpaper.jpg", window, cx);
            s.set_value(
                &config
                    .appearance
                    .background_image
                    .clone()
                    .unwrap_or_default(),
                window,
                cx,
            );
            s
        });
        let background_image_opacity_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(1.0)
                .step(0.01)
                .default_value(Self::clamp_background_image_opacity(
                    config.appearance.background_image_opacity,
                ))
        });
        let background_image_position_select = Self::make_string_select(
            BACKGROUND_IMAGE_POSITIONS,
            &config.appearance.background_image_position,
            window,
            cx,
        );
        let background_image_fit_select = Self::make_string_select(
            BACKGROUND_IMAGE_FITS,
            &config.appearance.background_image_fit,
            window,
            cx,
        );
        let custom_theme_name_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_placeholder("Save as, e.g. flexoki-amber", window, cx);
            s
        });
        let http_proxy_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            let val = config.network.http_proxy.clone().unwrap_or_default();
            s.set_value(val, window, cx);
            s.set_placeholder("http://127.0.0.1:1086", window, cx);
            s
        });
        let https_proxy_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            let val = config.network.https_proxy.clone().unwrap_or_default();
            s.set_value(val, window, cx);
            s.set_placeholder("http://127.0.0.1:1086", window, cx);
            s
        });

        cx.subscribe(
            &terminal_opacity_slider,
            |this, _, event: &SliderEvent, cx| match event {
                SliderEvent::Change(value) => {
                    this.config.appearance.terminal_opacity =
                        Self::clamp_terminal_opacity(value.end());
                    cx.emit(AppearancePreview);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe(
            &ui_opacity_slider,
            |this, _, event: &SliderEvent, cx| match event {
                SliderEvent::Change(value) => {
                    this.config.appearance.ui_opacity = Self::clamp_ui_opacity(value.end());
                    cx.emit(AppearancePreview);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &tab_accent_inactive_alpha_slider,
            window,
            |this, _, event: &SliderEvent, window, cx| match event {
                SliderEvent::Change(value) => {
                    let inactive = Self::clamp_tab_accent_inactive_alpha(value.end());
                    this.config.appearance.tab_accent_inactive_alpha = inactive;
                    let hover = Self::clamp_tab_accent_inactive_hover_alpha(
                        this.config.appearance.tab_accent_inactive_hover_alpha,
                        inactive,
                    );
                    this.config.appearance.tab_accent_inactive_hover_alpha = hover;
                    this.tab_accent_inactive_hover_alpha_slider
                        .update(cx, |slider, cx| slider.set_value(hover, window, cx));
                    cx.emit(AppearancePreview);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &tab_accent_inactive_hover_alpha_slider,
            window,
            |this, _, event: &SliderEvent, window, cx| match event {
                SliderEvent::Change(value) => {
                    let inactive = Self::clamp_tab_accent_inactive_alpha(
                        this.config.appearance.tab_accent_inactive_alpha,
                    );
                    let hover = Self::clamp_tab_accent_inactive_hover_alpha(value.end(), inactive);
                    this.config.appearance.tab_accent_inactive_hover_alpha = hover;
                    this.tab_accent_inactive_hover_alpha_slider
                        .update(cx, |slider, cx| slider.set_value(hover, window, cx));
                    cx.emit(AppearancePreview);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe(
            &background_image_opacity_slider,
            |this, _, event: &SliderEvent, cx| match event {
                SliderEvent::Change(value) => {
                    this.config.appearance.background_image_opacity =
                        Self::clamp_background_image_opacity(value.end());
                    cx.emit(AppearancePreview);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &active_provider_select,
            window,
            |this, _, ev: &SelectEvent<SearchableVec<String>>, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    if let Some(provider) = Self::suggestion_provider_from_label(value) {
                        this.config.agent.provider =
                            Self::provider_for_saved_transport(&this.config, &provider);
                        this.active_model_select = Self::make_active_model_select(
                            &this.config,
                            &this.registry,
                            window,
                            cx,
                        );
                        cx.notify();
                    }
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &suggestion_provider_select,
            window,
            |this, _, ev: &SelectEvent<SearchableVec<String>>, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    this.config.agent.suggestion_model.provider =
                        Self::suggestion_provider_from_label(value);
                    this.suggestion_model_select = Self::make_suggestion_model_select(
                        &this.config,
                        &this.registry,
                        window,
                        cx,
                    );
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &ai_purpose_select,
            window,
            |this, _, ev: &SelectEvent<Vec<String>>, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    this.config.agent.purpose = Self::ai_purpose_from_label(value);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &terminal_font_select,
            window,
            |this, _, ev: &SelectEvent<SearchableVec<String>>, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    this.config.terminal.font_family = sanitize_terminal_font_family(value);
                    cx.emit(AppearancePreview);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &ui_font_select,
            window,
            |this, _, ev: &SelectEvent<SearchableVec<String>>, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    this.config.appearance.ui_font_family = value.clone();
                    cx.emit(AppearancePreview);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &cursor_style_select,
            window,
            |this, _, ev: &SelectEvent<Vec<String>>, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    this.config.terminal.cursor_style =
                        Self::cursor_style_from_label(value).to_string();
                    cx.emit(AppearancePreview);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &background_image_position_select,
            window,
            |this, _, ev: &SelectEvent<Vec<String>>, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    this.config.appearance.background_image_position = value.clone();
                    cx.emit(AppearancePreview);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &background_image_fit_select,
            window,
            |this, _, ev: &SelectEvent<Vec<String>>, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    this.config.appearance.background_image_fit = value.clone();
                    cx.emit(AppearancePreview);
                    cx.notify();
                }
            },
        )
        .detach();

        Self {
            visible: false,
            standalone: false,
            config: config.clone(),
            preview_snapshot: None,
            registry,
            oauth_runtime,
            focus_handle: cx.focus_handle(),
            active_section: SettingsSection::General,
            overlay_motion: MotionValue::new(0.0),
            selected_provider,
            active_provider_select,
            active_model_select,
            model_input,
            model_select,
            endpoint_preset_select,
            api_key_input,
            base_url_input,
            max_tokens_input,
            max_turns_input,
            temperature_input,
            auto_approve: config.agent.auto_approve_tools,
            ai_purpose_select,
            suggestion_enabled: config.agent.suggestion_model.enabled,
            suggestion_provider_select,
            suggestion_model_select,
            oauth_states: HashMap::new(),
            provider_model_fetching: false,
            provider_model_status: None,
            provider_model_status_error: false,
            terminal_font_select,
            ui_font_select,
            cursor_style_select,
            font_size_input,
            ui_font_size_input,
            terminal_opacity_slider,
            terminal_blur: Self::effective_terminal_blur(config.appearance.terminal_blur),
            ui_opacity_slider,
            tab_accent_inactive_alpha_slider,
            tab_accent_inactive_hover_alpha_slider,
            background_image_input,
            background_image_opacity_slider,
            background_image_position_select,
            background_image_fit_select,
            background_image_repeat: config.appearance.background_image_repeat,
            hide_pane_title_bar: config.appearance.hide_pane_title_bar,
            save_error: None,
            save_error_kind: None,
            last_saved_at: std::fs::metadata(Config::config_path())
                .and_then(|m| m.modified())
                .ok(),
            close_confirmation_visible: false,
            custom_theme_name_input,
            custom_theme_preview: None,
            custom_theme_status: None,
            recording_key: None,
            #[cfg(target_os = "macos")]
            recording_resume_keybindings: None,
            http_proxy_input,
            https_proxy_input,
        }
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.standalone = false;
        self.visible = !self.visible;
        self.overlay_motion.set_target(
            if self.visible { 1.0 } else { 0.0 },
            std::time::Duration::from_millis(if self.visible { 220 } else { 180 }),
        );
        if self.visible {
            self.refresh_controls_from_config(window, cx);
        } else {
            // Ensure hotkeys are always re-enabled when the panel closes,
            // even if recording was active when the user dismissed it.
            self.set_recording_key(None);
        }
        cx.notify();
    }

    pub fn open_standalone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.standalone = true;
        self.visible = true;
        self.preview_snapshot = Some(self.config.clone());
        self.close_confirmation_visible = false;
        self.overlay_motion
            .set_target(1.0, std::time::Duration::ZERO);
        self.refresh_controls_from_config(window, cx);
        cx.notify();
    }

    pub fn revert_standalone_preview(&mut self, cx: &mut Context<Self>) {
        if !self.standalone {
            return;
        }
        self.close_confirmation_visible = false;
        self.set_recording_key(None);
        if let Some(snapshot) = self.preview_snapshot.take() {
            self.config = snapshot;
            cx.emit(AppearancePreview);
            cx.notify();
        }
    }

    fn refresh_controls_from_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.normalize_active_provider_for_saved_transport();
        self.selected_provider =
            Self::provider_for_saved_transport(&self.config, &self.selected_provider);
        let agent = &self.config.agent;
        let pc = agent.providers.get_or_default(&self.selected_provider);
        self.load_provider_inputs(&pc, window, cx);
        self.sync_provider_placeholders(&self.selected_provider, window, cx);
        self.max_turns_input.update(cx, |s, cx| {
            s.set_value(&agent.max_turns.to_string(), window, cx)
        });
        self.temperature_input.update(cx, |s, cx| {
            s.set_value(
                &agent.temperature.map(|t| t.to_string()).unwrap_or_default(),
                window,
                cx,
            )
        });
        self.active_provider_select.update(cx, |select, cx| {
            select.set_selected_value(
                &provider_label(&Self::sidebar_provider_kind(&agent.provider)).to_string(),
                window,
                cx,
            );
        });
        self.active_model_select =
            Self::make_active_model_select(&self.config, &self.registry, window, cx);
        self.ai_purpose_select.update(cx, |select, cx| {
            select.set_selected_value(
                &Self::ai_purpose_label(agent.purpose).to_string(),
                window,
                cx,
            );
        });
        self.suggestion_enabled = agent.suggestion_model.enabled;
        self.suggestion_provider_select.update(cx, |select, cx| {
            select.set_selected_value(
                &Self::suggestion_provider_label(agent.suggestion_model.provider.as_ref()),
                window,
                cx,
            );
        });
        self.suggestion_model_select =
            Self::make_suggestion_model_select(&self.config, &self.registry, window, cx);
        self.auto_approve = agent.auto_approve_tools;
        self.model_select = Self::make_model_select(
            &self.selected_provider,
            &pc.model,
            pc.base_url.as_deref(),
            &self.registry,
            window,
            cx,
        );
        self.endpoint_preset_select =
            Self::make_endpoint_preset_select(&self.selected_provider, &pc.base_url, window, cx);
        self.terminal_font_select.update(cx, |select, cx| {
            select.set_selected_value(
                &sanitize_terminal_font_family(&self.config.terminal.font_family),
                window,
                cx,
            );
        });
        self.ui_font_select.update(cx, |select, cx| {
            select.set_selected_value(&self.config.appearance.ui_font_family, window, cx);
        });
        self.cursor_style_select.update(cx, |select, cx| {
            select.set_selected_value(
                &Self::cursor_style_label(&self.config.terminal.cursor_style).to_string(),
                window,
                cx,
            );
        });
        self.font_size_input.update(cx, |s, cx| {
            s.set_value(&self.config.terminal.font_size.to_string(), window, cx)
        });
        self.ui_font_size_input.update(cx, |s, cx| {
            s.set_value(
                &Self::clamp_ui_font_size(self.config.appearance.ui_font_size).to_string(),
                window,
                cx,
            )
        });
        self.terminal_opacity_slider.update(cx, |slider, cx| {
            slider.set_value(
                Self::clamp_terminal_opacity(self.config.appearance.terminal_opacity),
                window,
                cx,
            );
        });
        self.terminal_blur = Self::effective_terminal_blur(self.config.appearance.terminal_blur);
        self.config.appearance.terminal_blur = self.terminal_blur;
        self.ui_opacity_slider.update(cx, |slider, cx| {
            slider.set_value(
                Self::clamp_ui_opacity(self.config.appearance.ui_opacity),
                window,
                cx,
            );
        });
        self.tab_accent_inactive_alpha_slider
            .update(cx, |slider, cx| {
                slider.set_value(
                    Self::clamp_tab_accent_inactive_alpha(
                        self.config.appearance.tab_accent_inactive_alpha,
                    ),
                    window,
                    cx,
                );
            });
        self.tab_accent_inactive_hover_alpha_slider
            .update(cx, |slider, cx| {
                slider.set_value(
                    Self::clamp_tab_accent_inactive_hover_alpha(
                        self.config.appearance.tab_accent_inactive_hover_alpha,
                        Self::clamp_tab_accent_inactive_alpha(
                            self.config.appearance.tab_accent_inactive_alpha,
                        ),
                    ),
                    window,
                    cx,
                );
            });
        self.background_image_input.update(cx, |s, cx| {
            s.set_value(
                &self
                    .config
                    .appearance
                    .background_image
                    .clone()
                    .unwrap_or_default(),
                window,
                cx,
            );
        });
        self.background_image_opacity_slider
            .update(cx, |slider, cx| {
                slider.set_value(
                    Self::clamp_background_image_opacity(
                        self.config.appearance.background_image_opacity,
                    ),
                    window,
                    cx,
                );
            });
        self.background_image_position_select
            .update(cx, |select, cx| {
                select.set_selected_value(
                    &self.config.appearance.background_image_position,
                    window,
                    cx,
                );
            });
        self.background_image_fit_select.update(cx, |select, cx| {
            select.set_selected_value(&self.config.appearance.background_image_fit, window, cx);
        });
        self.background_image_repeat = self.config.appearance.background_image_repeat;
        self.hide_pane_title_bar = self.config.appearance.hide_pane_title_bar;
        self.provider_model_fetching = false;
        self.provider_model_status = None;
        self.provider_model_status_error = false;
        self.set_recording_key(None);
        // Network / proxy — repopulate so reopening the panel shows current values.
        self.http_proxy_input.update(cx, |s, cx| {
            s.set_value(
                &self.config.network.http_proxy.clone().unwrap_or_default(),
                window,
                cx,
            )
        });
        self.https_proxy_input.update(cx, |s, cx| {
            s.set_value(
                &self.config.network.https_proxy.clone().unwrap_or_default(),
                window,
                cx,
            )
        });
        self.focus_handle.focus(window, cx);
    }

    /// Load a provider's config into the per-provider input fields.
    fn load_provider_inputs(
        &self,
        pc: &ProviderConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model_input.update(cx, |s, cx| {
            s.set_value(&pc.model.clone().unwrap_or_default(), window, cx)
        });
        let key_val = pc
            .api_key
            .clone()
            .or_else(|| pc.api_key_env.clone())
            .unwrap_or_default();
        self.api_key_input
            .update(cx, |s, cx| s.set_value(&key_val, window, cx));
        self.base_url_input.update(cx, |s, cx| {
            s.set_value(&pc.base_url.clone().unwrap_or_default(), window, cx)
        });
        let endpoint_label =
            Self::endpoint_label_for_base_url(&self.selected_provider, pc.base_url.as_deref())
                .to_string();
        self.endpoint_preset_select.update(cx, |select, cx| {
            select.set_selected_value(&endpoint_label, window, cx);
        });
        self.max_tokens_input.update(cx, |s, cx| {
            s.set_value(
                &pc.max_tokens.map(|t| t.to_string()).unwrap_or_default(),
                window,
                cx,
            )
        });
    }

    /// Build a model select entity for the given provider.
    fn provider_base_url<'a>(config: &'a Config, provider: &ProviderKind) -> Option<&'a str> {
        config
            .agent
            .providers
            .get(provider)
            .and_then(|pc| pc.base_url.as_deref())
    }

    fn make_model_select_state(
        provider: &ProviderKind,
        current_model: &Option<String>,
        base_url: Option<&str>,
        registry: &ModelRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SelectState<SearchableVec<String>>> {
        let mut models: Vec<String> = registry.models_for_base_url(provider, base_url);
        if let Some(model) = current_model
            .as_ref()
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
        {
            if !models.iter().any(|item| item == model) {
                models.insert(0, model.to_string());
            }
        }
        let selected_index = current_model
            .as_ref()
            .and_then(|m| models.iter().position(|item| item == m).map(IndexPath::new));
        let entity = cx.new(|cx| {
            SelectState::new(SearchableVec::new(models), selected_index, window, cx)
                .searchable(true)
        });
        entity
    }

    fn make_model_select(
        provider: &ProviderKind,
        current_model: &Option<String>,
        base_url: Option<&str>,
        registry: &ModelRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SelectState<SearchableVec<String>>> {
        let entity =
            Self::make_model_select_state(provider, current_model, base_url, registry, window, cx);
        cx.subscribe_in(
            &entity,
            window,
            |this, _, ev: &SelectEvent<SearchableVec<String>>, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    this.model_input.update(cx, |s, cx| {
                        s.set_value(value, window, cx);
                    });
                    cx.notify();
                }
            },
        )
        .detach();
        entity
    }

    fn make_active_model_select(
        config: &Config,
        registry: &ModelRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SelectState<SearchableVec<String>>> {
        let provider = config.agent.provider.clone();
        let current_model = config
            .agent
            .providers
            .get(&provider)
            .and_then(|pc| pc.model.clone());
        let entity = Self::make_model_select_state(
            &provider,
            &current_model,
            Self::provider_base_url(config, &provider),
            registry,
            window,
            cx,
        );
        cx.subscribe_in(
            &entity,
            window,
            |this, _, ev: &SelectEvent<SearchableVec<String>>, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    let provider = this.config.agent.provider.clone();
                    let mut pc = this.config.agent.providers.get_or_default(&provider);
                    pc.model = Some(value.clone());
                    this.config.agent.providers.set(&provider, pc);
                    cx.notify();
                }
            },
        )
        .detach();
        entity
    }

    fn make_suggestion_model_select(
        config: &Config,
        registry: &ModelRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SelectState<SearchableVec<String>>> {
        let provider = Self::effective_suggestion_provider(config);
        let current_model = config.agent.suggestion_model.model.clone();
        let entity = Self::make_model_select_state(
            &provider,
            &current_model,
            Self::provider_base_url(config, &provider),
            registry,
            window,
            cx,
        );
        cx.subscribe_in(
            &entity,
            window,
            |this, _, ev: &SelectEvent<SearchableVec<String>>, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = ev {
                    this.config.agent.suggestion_model.model = Some(value.clone());
                    cx.notify();
                }
            },
        )
        .detach();
        entity
    }

    /// Parse a ghostty config from clipboard and show a live preview.
    fn paste_theme_from_clipboard(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let text = match cx
            .read_from_clipboard()
            .and_then(|c| c.text().map(|s| s.to_string()))
        {
            Some(t) if !t.trim().is_empty() => t,
            _ => {
                self.custom_theme_status =
                    Some("Error: clipboard is empty. Copy a Ghostty theme first.".into());
                cx.notify();
                return;
            }
        };

        // Get theme name from input, fallback to "custom"
        let name_raw = self.custom_theme_name_input.read(cx).value().to_string();
        let name = if name_raw.trim().is_empty() {
            "custom".to_string()
        } else {
            name_raw.trim().to_lowercase().replace(' ', "-")
        };

        match con_terminal::TerminalTheme::from_ghostty_format(&name, &text) {
            Some(theme) => {
                self.custom_theme_status = Some(format!(
                    "Loaded \"{}\" from the clipboard. {} ANSI colors are ready to preview.",
                    display_theme_name(&name),
                    theme.ansi.len()
                ));
                self.custom_theme_preview = Some(theme);
            }
            None => {
                self.custom_theme_status = Some(
                    "Error: couldn't read a Ghostty theme. Include background, foreground, and palette entries.".into()
                );
                self.custom_theme_preview = None;
            }
        }
        cx.notify();
    }

    fn browse_background_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a background image".into()),
        });

        let input = self.background_image_input.clone();
        cx.spawn_in(window, async move |this, window| {
            let path = paths.await.ok()?.ok()??.into_iter().next()?;
            let path_text = path.to_string_lossy().to_string();

            window
                .update(|window, cx| {
                    _ = input.update(cx, |state, cx| {
                        state.set_value(&path_text, window, cx);
                    });
                    _ = this.update(cx, |panel, cx| {
                        panel.config.appearance.background_image = Some(path_text.clone());
                        cx.emit(AppearancePreview);
                        cx.notify();
                    });
                })
                .ok()?;

            Some(())
        })
        .detach();
    }

    /// Save the custom theme to the user themes directory and apply it.
    fn apply_custom_theme(&mut self, cx: &mut Context<Self>) {
        let preview = match &self.custom_theme_preview {
            Some(t) => t.clone(),
            None => return,
        };

        let dir = con_terminal::TerminalTheme::user_themes_dir();

        // Create directory if needed
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.custom_theme_status = Some(format!("Error: {}", e));
            cx.notify();
            return;
        }

        // Read the config text from clipboard again for saving
        let text = match cx
            .read_from_clipboard()
            .and_then(|c| c.text().map(|s| s.to_string()))
        {
            Some(t) if !t.trim().is_empty() => t,
            _ => {
                self.custom_theme_status =
                    Some("Error: the clipboard changed before save. Load the theme again.".into());
                cx.notify();
                return;
            }
        };

        let file_path = dir.join(&preview.name);
        if let Err(e) = std::fs::write(&file_path, &text) {
            self.custom_theme_status = Some(format!("Error: {}", e));
            cx.notify();
            return;
        }

        // Apply the theme
        self.config.terminal.theme = preview.name.clone();
        cx.emit(ThemePreview(preview.name.clone()));
        self.custom_theme_status = Some(format!("Saved and applied: {}", file_path.display()));
        self.custom_theme_preview = None;
        cx.notify();
    }

    /// Read current per-provider input fields into a ProviderConfig.
    fn read_provider_inputs(&self, cx: &App) -> ProviderConfig {
        let key_text = self.api_key_input.read(cx).value().to_string();
        let is_env_var = !key_text.is_empty()
            && key_text
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit());

        let (api_key, api_key_env) = if key_text.is_empty() {
            (None, None)
        } else if is_env_var {
            (None, Some(key_text))
        } else {
            (Some(key_text), None)
        };

        let model_text = self.model_input.read(cx).value().to_string();
        let base_url_text = self.base_url_input.read(cx).value().to_string();
        let max_tokens_text = self.max_tokens_input.read(cx).value().to_string();

        ProviderConfig {
            model: if model_text.is_empty() {
                None
            } else {
                Some(model_text)
            },
            api_key,
            api_key_env,
            base_url: if base_url_text.is_empty() {
                None
            } else {
                Some(base_url_text)
            },
            max_tokens: if max_tokens_text.is_empty() {
                None
            } else {
                max_tokens_text.parse().ok()
            },
        }
    }

    fn resolve_provider_api_key(config: &ProviderConfig) -> Result<Option<String>, String> {
        if let Some(key) = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(Some(key.to_string()));
        }

        if let Some(env_name) = config
            .api_key_env
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return std::env::var(env_name)
                .map(|value| value.trim().to_string())
                .map_err(|_| format!("Environment variable {env_name} is not set."))
                .and_then(|value| {
                    if value.is_empty() {
                        Err(format!("Environment variable {env_name} is empty."))
                    } else {
                        Ok(Some(value))
                    }
                });
        }

        Ok(None)
    }

    fn fetch_selected_provider_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(
            self.selected_provider,
            ProviderKind::OpenAICompatible | ProviderKind::ChatGPT
        ) || self.provider_model_fetching
        {
            return;
        }

        let provider = self.selected_provider.clone();
        let provider_config = self.read_provider_inputs(cx);
        self.config
            .agent
            .providers
            .set(&provider, provider_config.clone());

        let fetch_request = match provider {
            ProviderKind::OpenAICompatible => {
                let Some(base_url) = provider_config
                    .base_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                else {
                    self.provider_model_status =
                        Some("Enter the provider Base URL, usually ending in /v1.".to_string());
                    self.provider_model_status_error = true;
                    cx.notify();
                    return;
                };

                let api_key = match Self::resolve_provider_api_key(&provider_config) {
                    Ok(api_key) => api_key,
                    Err(message) => {
                        self.provider_model_status = Some(message);
                        self.provider_model_status_error = true;
                        cx.notify();
                        return;
                    }
                };
                ProviderModelFetchRequest::OpenAICompatible { base_url, api_key }
            }
            ProviderKind::ChatGPT => {
                let Some(auth_file) =
                    oauth_token_dir(&ProviderKind::ChatGPT).map(|dir| dir.join("auth.json"))
                else {
                    self.provider_model_status =
                        Some("ChatGPT OAuth token storage is unavailable.".to_string());
                    self.provider_model_status_error = true;
                    cx.notify();
                    return;
                };
                let access_token = match read_synced_chatgpt_oauth_access_token(&auth_file) {
                    Ok(Some(token)) => token,
                    Ok(None) => {
                        self.provider_model_status =
                            Some("Sign in with ChatGPT OAuth before fetching models.".to_string());
                        self.provider_model_status_error = true;
                        cx.notify();
                        return;
                    }
                    Err(err) => {
                        self.provider_model_status =
                            Some(format!("Could not refresh ChatGPT OAuth cache: {err}"));
                        self.provider_model_status_error = true;
                        cx.notify();
                        return;
                    }
                };
                let base_url = provider_config
                    .base_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                ProviderModelFetchRequest::ChatGPT {
                    access_token,
                    base_url,
                }
            }
            _ => return,
        };

        self.provider_model_fetching = true;
        self.provider_model_status = Some("Fetching models from the provider…".to_string());
        self.provider_model_status_error = false;
        cx.notify();

        let registry = self.registry.clone();
        let runtime = self.oauth_runtime.clone();
        cx.spawn_in(window, async move |this, window| {
            let result = runtime
                .spawn(async move { fetch_request.fetch().await })
                .await
                .map_err(|err| anyhow::anyhow!("Model fetch task failed: {err}"))
                .and_then(|result| result);

            let _ = window.update(|window, cx| {
                let _ = this.update(cx, |panel, cx| {
                    panel.provider_model_fetching = false;
                    match result {
                        Ok(result) if result.models().is_empty() => {
                            panel.provider_model_status = Some(
                                "The endpoint responded, but returned no models. Type the model ID manually."
                                    .to_string(),
                            );
                            panel.provider_model_status_error = true;
                        }
                        Ok(result) => {
                            let count = result.models().len();
                            match result {
                                ProviderModelFetchResult::OpenAICompatible { base_url, models } => {
                                    if let Err(err) = registry.set_provider_models_for_base_url(
                                        ProviderKind::OpenAICompatible,
                                        &base_url,
                                        models,
                                    ) {
                                        panel.provider_model_status = Some(err.to_string());
                                        panel.provider_model_status_error = true;
                                        cx.notify();
                                        return;
                                    }
                                }
                                ProviderModelFetchResult::ChatGPT { models } => {
                                    registry.set_provider_models(ProviderKind::ChatGPT, models);
                                }
                            }
                            if panel.selected_provider == provider {
                                let current_model =
                                    panel.model_input.read(cx).value().trim().to_string();
                                let current_model =
                                    (!current_model.is_empty()).then_some(current_model);
                                panel.model_select = Self::make_model_select(
                                    &provider,
                                    &current_model,
                                    Self::provider_base_url(&panel.config, &provider),
                                    &registry,
                                    window,
                                    cx,
                                );
                            }
                            if panel.config.agent.provider == provider {
                                panel.active_model_select = Self::make_active_model_select(
                                    &panel.config,
                                    &registry,
                                    window,
                                    cx,
                                );
                            }
                            if Self::effective_suggestion_provider(&panel.config) == provider {
                                panel.suggestion_model_select = Self::make_suggestion_model_select(
                                    &panel.config,
                                    &registry,
                                    window,
                                    cx,
                                );
                            }
                            panel.provider_model_status = Some(format!(
                                "Fetched {count} model{}.",
                                if count == 1 { "" } else { "s" }
                            ));
                            panel.provider_model_status_error = false;
                        }
                        Err(err) => {
                            panel.provider_model_status = Some(format!(
                                "Could not fetch models: {err}. Type the model ID manually."
                            ));
                            panel.provider_model_status_error = true;
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn is_visible(&self) -> bool {
        self.visible || self.overlay_motion.is_animating()
    }

    pub fn is_overlay_visible(&self) -> bool {
        !self.standalone && self.is_visible()
    }

    fn sync_config_from_controls(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let max_turns_text = self.max_turns_input.read(cx).value().to_string();
        let temperature_text = self.temperature_input.read(cx).value().to_string();
        let suggestion_provider_label = self
            .suggestion_provider_select
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_else(|| "Same as active provider".to_string());
        let suggestion_model_text = self
            .suggestion_model_select
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default();
        let font_size_text = self.font_size_input.read(cx).value().to_string();
        let ui_font_size_text = self.ui_font_size_input.read(cx).value().trim().to_string();

        // Save current provider's per-provider fields into the map
        let pc = self.read_provider_inputs(cx);
        self.config.agent.providers.set(&self.selected_provider, pc);
        self.normalize_active_provider_for_saved_transport();

        // Update global fields
        self.config.agent.max_turns = max_turns_text.parse().unwrap_or(10);
        self.config.agent.temperature = if temperature_text.is_empty() {
            None
        } else {
            temperature_text.parse().ok()
        };
        self.config.agent.auto_approve_tools = self.auto_approve;
        let suggestion_provider = Self::suggestion_provider_from_label(&suggestion_provider_label)
            .map(|provider| Self::provider_for_saved_transport(&self.config, &provider));
        self.config.agent.suggestion_model = SuggestionModelConfig {
            enabled: self.suggestion_enabled,
            provider: suggestion_provider,
            model: if suggestion_model_text.is_empty() {
                None
            } else {
                Some(suggestion_model_text)
            },
        };
        self.config.terminal.font_family =
            sanitize_terminal_font_family(&self.config.terminal.font_family);
        self.config.terminal.font_size = font_size_text.parse().unwrap_or(14.0);
        let parsed_ui_font_size = if ui_font_size_text.is_empty() {
            Some(self.config.appearance.ui_font_size)
        } else {
            ui_font_size_text.parse::<f32>().ok()
        };
        let Some(parsed_ui_font_size) = parsed_ui_font_size else {
            return Err(format!(
                "UI Size must be a number between {:.1} and {:.1}.",
                MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE
            ));
        };
        self.config.appearance.ui_font_size = Self::clamp_ui_font_size(parsed_ui_font_size);
        self.config.appearance.terminal_opacity =
            Self::clamp_terminal_opacity(self.terminal_opacity_slider.read(cx).value().end());
        self.config.appearance.terminal_blur = Self::effective_terminal_blur(self.terminal_blur);
        self.terminal_blur = self.config.appearance.terminal_blur;
        self.config.appearance.ui_opacity =
            Self::clamp_ui_opacity(self.ui_opacity_slider.read(cx).value().end());
        self.config.appearance.tab_accent_inactive_alpha = Self::clamp_tab_accent_inactive_alpha(
            self.tab_accent_inactive_alpha_slider.read(cx).value().end(),
        );
        self.config.appearance.tab_accent_inactive_hover_alpha =
            Self::clamp_tab_accent_inactive_hover_alpha(
                self.tab_accent_inactive_hover_alpha_slider
                    .read(cx)
                    .value()
                    .end(),
                self.config.appearance.tab_accent_inactive_alpha,
            );
        let background_image_text = self
            .background_image_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        self.config.appearance.background_image = if background_image_text.is_empty() {
            None
        } else {
            Some(background_image_text)
        };
        self.config.appearance.background_image_opacity = Self::clamp_background_image_opacity(
            self.background_image_opacity_slider.read(cx).value().end(),
        );
        self.config.appearance.background_image_repeat = self.background_image_repeat;

        // Network / proxy
        // Blank field → None (leave inherited env untouched).
        // Non-empty   → Some(value) (override or clear on next startup).
        let http_proxy_text = self.http_proxy_input.read(cx).value().trim().to_string();
        let https_proxy_text = self.https_proxy_input.read(cx).value().trim().to_string();
        self.config.network.http_proxy = if http_proxy_text.is_empty() {
            None
        } else {
            Some(http_proxy_text)
        };
        self.config.network.https_proxy = if https_proxy_text.is_empty() {
            None
        } else {
            Some(https_proxy_text)
        };

        Ok(())
    }

    fn config_matches(a: &Config, b: &Config) -> bool {
        match (toml::to_string_pretty(a), toml::to_string_pretty(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }

    fn has_unsaved_changes(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(snapshot) = self.preview_snapshot.clone() else {
            return false;
        };

        match self.sync_config_from_controls(cx) {
            Ok(()) => !Self::config_matches(&snapshot, &self.config),
            Err(_) => true,
        }
    }

    pub fn request_standalone_close(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.standalone {
            return true;
        }

        if !self.has_unsaved_changes(cx) {
            self.close_confirmation_visible = false;
            self.revert_standalone_preview(cx);
            return true;
        }

        self.close_confirmation_visible = true;
        cx.notify();
        false
    }

    fn keep_editing_after_close_prompt(&mut self, cx: &mut Context<Self>) {
        self.close_confirmation_visible = false;
        cx.notify();
    }

    fn save_and_close_standalone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save(window, cx);
        if self.save_error.is_none() {
            self.close_confirmation_visible = false;
            window.remove_window();
        } else {
            self.close_confirmation_visible = true;
            cx.notify();
        }
    }

    fn save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_recording_key(None);

        if let Err(message) = self.sync_config_from_controls(cx) {
            self.save_error = Some(message);
            self.save_error_kind = Some(SettingsSaveErrorKind::Other);
            cx.notify();
            return;
        }

        // Keybindings are updated directly via record_keystroke — no reading needed
        if let Some(message) = keybinding_conflict_message(&self.config.keybindings) {
            self.save_error = Some(message);
            self.save_error_kind = Some(SettingsSaveErrorKind::KeybindingConflict);
            cx.notify();
            return;
        }

        match self.persist_config() {
            Ok(()) => {
                self.save_error = None;
                self.save_error_kind = None;
                self.last_saved_at = Some(std::time::SystemTime::now());
                self.preview_snapshot = Some(self.config.clone());
                if !self.standalone {
                    self.visible = false;
                    self.overlay_motion
                        .set_target(0.0, std::time::Duration::from_millis(180));
                }
                cx.emit(SaveSettings);
            }
            Err(e) => {
                log::error!("Failed to save config: {}", e);
                self.save_error = Some(e.to_string());
                self.save_error_kind = Some(SettingsSaveErrorKind::Other);
            }
        }
        cx.notify();
    }

    /// Record a keystroke for the binding currently being recorded.

    fn set_recording_key(&mut self, key: Option<String>) {
        #[cfg(target_os = "macos")]
        let was_recording = self.recording_key.is_some();
        #[cfg(target_os = "macos")]
        let will_record = key.is_some();
        self.recording_key = key;

        #[cfg(target_os = "macos")]
        match (was_recording, will_record) {
            (false, true) => {
                let keybindings = con_core::Config::load()
                    .map(|config| config.keybindings)
                    .unwrap_or_else(|err| {
                        log::warn!(
                            "settings: failed to load persisted config before hotkey recording: {err}"
                        );
                        self.config.keybindings.clone()
                    });
                self.recording_resume_keybindings = Some(keybindings.clone());
                crate::global_hotkey::suspend_global_hotkeys(&keybindings);
            }
            (true, false) => {
                if let Some(keybindings) = self.recording_resume_keybindings.take() {
                    crate::global_hotkey::resume_global_hotkeys(&keybindings);
                } else {
                    log::warn!("settings: hotkey recording ended without saved resume keybindings");
                }
            }
            _ => {}
        }
    }

    fn record_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let field = match &self.recording_key {
            Some(f) => f.clone(),
            None => return,
        };

        // Don't record bare modifier keys or escape (used to cancel)
        let key = &keystroke.key;
        if matches!(
            key.as_str(),
            "shift" | "control" | "alt" | "meta" | "fn" | "escape"
        ) {
            if key == "escape" {
                self.set_recording_key(None);
                cx.notify();
            }
            return;
        }

        // Build GPUI binding format: cmd-shift-k
        let binding = keystroke_to_binding(keystroke);

        // Write directly into config
        match field.as_str() {
            "global_summon" => self.config.keybindings.global_summon = binding,
            "quick_terminal" => self.config.keybindings.quick_terminal = binding,
            "new_window" => self.config.keybindings.new_window = binding,
            "new_tab" => self.config.keybindings.new_tab = binding,
            "close_tab" => self.config.keybindings.close_tab = binding,
            "close_pane" => self.config.keybindings.close_pane = binding,
            "toggle_pane_zoom" => self.config.keybindings.toggle_pane_zoom = binding,
            "next_tab" => self.config.keybindings.next_tab = binding,
            "previous_tab" => self.config.keybindings.previous_tab = binding,
            "settings" => self.config.keybindings.settings = binding,
            "command_palette" => self.config.keybindings.command_palette = binding,
            "toggle_agent" => self.config.keybindings.toggle_agent = binding,
            "toggle_input_bar" => self.config.keybindings.toggle_input_bar = binding,
            "focus_input" => self.config.keybindings.focus_input = binding,
            "cycle_input_mode" => self.config.keybindings.cycle_input_mode = binding,
            "split_right" => self.config.keybindings.split_right = binding,
            "split_down" => self.config.keybindings.split_down = binding,
            "toggle_pane_scope" => self.config.keybindings.toggle_pane_scope = binding,
            "toggle_left_panel" => self.config.keybindings.toggle_left_panel = binding,
            "focus_files" => self.config.keybindings.focus_files = binding,
            "search_files" => self.config.keybindings.search_files = binding,
            "collapse_sidebar" => self.config.keybindings.collapse_sidebar = binding,
            "new_surface" => self.config.keybindings.new_surface = binding,
            "new_surface_split_right" => self.config.keybindings.new_surface_split_right = binding,
            "new_surface_split_down" => self.config.keybindings.new_surface_split_down = binding,
            "next_surface" => self.config.keybindings.next_surface = binding,
            "previous_surface" => self.config.keybindings.previous_surface = binding,
            "rename_surface" => self.config.keybindings.rename_surface = binding,
            "close_surface" => self.config.keybindings.close_surface = binding,
            "quit" => self.config.keybindings.quit = binding,
            _ => {}
        }
        self.set_recording_key(None);
        sync_keybinding_conflict_error(
            &mut self.save_error,
            &mut self.save_error_kind,
            &self.config.keybindings,
        );
        cx.notify();
    }

    /// Get the current value of a keybinding by field name.
    fn binding_value(&self, field: &str) -> &str {
        match field {
            "global_summon" => &self.config.keybindings.global_summon,
            "quick_terminal" => &self.config.keybindings.quick_terminal,
            "new_window" => &self.config.keybindings.new_window,
            "new_tab" => &self.config.keybindings.new_tab,
            "close_tab" => &self.config.keybindings.close_tab,
            "close_pane" => &self.config.keybindings.close_pane,
            "toggle_pane_zoom" => &self.config.keybindings.toggle_pane_zoom,
            "next_tab" => &self.config.keybindings.next_tab,
            "previous_tab" => &self.config.keybindings.previous_tab,
            "settings" => &self.config.keybindings.settings,
            "command_palette" => &self.config.keybindings.command_palette,
            "toggle_agent" => &self.config.keybindings.toggle_agent,
            "toggle_input_bar" => &self.config.keybindings.toggle_input_bar,
            "focus_input" => &self.config.keybindings.focus_input,
            "cycle_input_mode" => &self.config.keybindings.cycle_input_mode,
            "split_right" => &self.config.keybindings.split_right,
            "split_down" => &self.config.keybindings.split_down,
            "toggle_pane_scope" => &self.config.keybindings.toggle_pane_scope,
            "toggle_left_panel" => &self.config.keybindings.toggle_left_panel,
            "focus_files" => &self.config.keybindings.focus_files,
            "search_files" => &self.config.keybindings.search_files,
            "collapse_sidebar" => &self.config.keybindings.collapse_sidebar,
            "new_surface" => &self.config.keybindings.new_surface,
            "new_surface_split_right" => &self.config.keybindings.new_surface_split_right,
            "new_surface_split_down" => &self.config.keybindings.new_surface_split_down,
            "next_surface" => &self.config.keybindings.next_surface,
            "previous_surface" => &self.config.keybindings.previous_surface,
            "rename_surface" => &self.config.keybindings.rename_surface,
            "close_surface" => &self.config.keybindings.close_surface,
            "quit" => &self.config.keybindings.quit,
            _ => "",
        }
    }

    /// Updates the settings draft only. Normal Settings edits persist through
    /// the Save button; callers that already persisted this value should use
    /// `set_persisted_restore_terminal_text` to keep close/revert semantics
    /// aligned with disk.
    pub fn set_restore_terminal_text(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.preview_snapshot.is_none() {
            self.preview_snapshot = Some(self.config.clone());
        }
        self.config.appearance.restore_terminal_text = enabled;
        cx.notify();
    }

    pub fn set_persisted_restore_terminal_text(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.config.appearance.restore_terminal_text = enabled;
        if let Some(snapshot) = &mut self.preview_snapshot {
            snapshot.appearance.restore_terminal_text = enabled;
        } else {
            self.preview_snapshot = Some(self.config.clone());
        }
        cx.notify();
    }

    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn terminal_config(&self) -> &con_core::config::TerminalConfig {
        &self.config.terminal
    }
    pub fn appearance_config(&self) -> &con_core::config::AppearanceConfig {
        &self.config.appearance
    }
    fn persist_config(&self) -> anyhow::Result<()> {
        self.config.save()
    }

    fn select_provider(
        &mut self,
        provider: ProviderKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let provider = if Self::protocol_pair(&provider).is_some() {
            if Self::sidebar_provider_kind(&self.selected_provider)
                == Self::sidebar_provider_kind(&provider)
            {
                self.selected_provider.clone()
            } else {
                Self::preferred_sidebar_provider(&self.config, &provider)
            }
        } else {
            Self::sidebar_selection_target(&provider, &self.selected_provider)
        };
        self.transition_provider(provider, window, cx);
    }

    fn transition_provider(
        &mut self,
        provider: ProviderKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_pc = self.read_provider_inputs(cx);
        self.config
            .agent
            .providers
            .set(&self.selected_provider, current_pc);

        if provider == self.selected_provider {
            cx.notify();
            return;
        }

        self.selected_provider = provider.clone();
        self.provider_model_fetching = false;
        self.provider_model_status = None;
        self.provider_model_status_error = false;

        let pc = self.config.agent.providers.get_or_default(&provider);
        self.load_provider_inputs(&pc, window, cx);
        self.sync_provider_placeholders(&provider, window, cx);

        self.model_select = Self::make_model_select(
            &provider,
            &pc.model,
            pc.base_url.as_deref(),
            &self.registry,
            window,
            cx,
        );
        self.endpoint_preset_select =
            Self::make_endpoint_preset_select(&provider, &pc.base_url, window, cx);
        cx.notify();
    }

    fn toggle_selected_provider_protocol(
        &mut self,
        use_anthropic: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source_provider = self.selected_provider.clone();
        let source_config = self.read_provider_inputs(cx);
        self.config
            .agent
            .providers
            .set(&source_provider, source_config.clone());

        let Some(provider) =
            Self::protocol_toggled_provider(&self.selected_provider, use_anthropic)
        else {
            return;
        };
        self.config.agent.set_provider_transport(
            &self.selected_provider,
            Some(if use_anthropic {
                ProviderTransport::Anthropic
            } else {
                ProviderTransport::OpenAI
            }),
        );
        let target_config = self.config.agent.providers.get_or_default(&provider);
        let seeded_target = Self::seed_protocol_variant_config(
            &source_provider,
            &provider,
            &source_config,
            &target_config,
        );
        self.config.agent.providers.set(&provider, seeded_target);
        self.set_active_provider_if_tracking(&source_provider, &provider, window, cx);
        self.transition_provider(provider, window, cx);
    }

    // ── Section content ──────────────────────────────────────────

    fn render_general(&mut self, cx: &mut Context<Self>) -> Div {
        let card_opacity = self.card_opacity();

        // --- Skills path chips (must render before borrowing theme) ---
        let project_paths = self.config.skills.project_paths.clone();
        let global_paths = self.config.skills.global_paths.clone();

        let project_chips = self.render_path_chips("project", &project_paths, cx);
        let global_chips = self.render_path_chips("global", &global_paths, cx);

        let project_presets = self.render_path_presets(
            "project",
            &project_paths,
            &[
                ("skills", "con"),
                (".con/skills", "con local"),
                (".agents/skills", "Agents"),
            ],
            cx,
        );
        let con_global_skills = con_paths::default_global_skills_path();
        let global_presets = self.render_path_presets(
            "global",
            &global_paths,
            &[
                (con_global_skills.as_str(), "con"),
                ("~/.agents/skills", "Agents"),
            ],
            cx,
        );

        let theme = cx.theme();

        // Build the Updates card (only shown for channels that poll).
        let channel = con_core::release_channel::current();
        // On any target outside macOS / Windows / Linux we have no
        // update backend, so skip the card even if the channel
        // otherwise would poll.
        #[cfg_attr(
            all(
                not(target_os = "macos"),
                not(target_os = "windows"),
                not(target_os = "linux")
            ),
            allow(unused_variables)
        )]
        let show_updates = channel.polls_for_updates();

        #[cfg_attr(
            all(
                not(target_os = "macos"),
                not(target_os = "windows"),
                not(target_os = "linux")
            ),
            allow(unused_mut)
        )]
        let mut container = section_content(
            "General",
            "Terminal defaults and shared app behavior.",
            theme,
        );

        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        if show_updates {
            let updater_status = crate::updater::status();
            let latest_state = crate::updater::latest_check();

            container = container.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(group_label("Updates", &theme))
                    .child(
                        card(theme, card_opacity)
                            .child(
                                div()
                                    .flex()
                                    .px(px(16.0))
                                    .py(px(14.0))
                                    .flex_col()
                                    .gap(px(12.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_start()
                                            .justify_between()
                                            .gap(px(16.0))
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(8.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(10.0))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(
                                                                theme
                                                                    .muted_foreground
                                                                    .opacity(0.5),
                                                            )
                                                            .child("Channel"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(13.0))
                                                            .line_height(px(18.0))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .child(channel.display_name()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .items_end()
                                                    .gap(px(8.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(10.0))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(
                                                                theme
                                                                    .muted_foreground
                                                                    .opacity(0.5),
                                                            )
                                                            .child("Version"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .line_height(px(18.0))
                                                            .font_family(
                                                                theme.mono_font_family.clone(),
                                                            )
                                                            .text_color(
                                                                theme
                                                                    .muted_foreground
                                                                    .opacity(0.82),
                                                            )
                                                            .child(format!(
                                                                "{} ({})",
                                                                crate::app_display_version(),
                                                                crate::app_build_number()
                                                            )),
                                                    ),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .mx(px(16.0))
                                    .h(px(1.0))
                                    .bg(theme.muted.opacity(0.10)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_end()
                                    .justify_between()
                                    .gap(px(16.0))
                                    .px(px(16.0))
                                    .pb(px(14.0))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(3.0))
                                            .max_w(px(420.0))
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(
                                                        theme
                                                            .muted_foreground
                                                            .opacity(0.5),
                                                    )
                                                    .child("Status"),
                                            )
                                            .child({
                                                let (summary, detail) =
                                                    update_summary_and_detail(
                                                        &latest_state,
                                                        updater_status,
                                                    );
                                                let mut col = div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(3.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.5))
                                                            .line_height(px(17.0))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(
                                                                theme.foreground.opacity(0.88),
                                                            )
                                                            .child(summary),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(10.5))
                                                            .line_height(px(15.0))
                                                            .text_color(
                                                                theme
                                                                    .muted_foreground
                                                                    .opacity(0.62),
                                                            )
                                                            .child(detail),
                                                    );
                                                if let Some(url) =
                                                    update_download_url(&latest_state)
                                                {
                                                    let label = match &latest_state {
                                                        crate::updater::CheckState::UpdateAvailable { version, .. } =>
                                                            format!("Download v{version}"),
                                                        _ => "Download release".to_string(),
                                                    };
                                                    col = col.child(
                                                        div().pt(px(4.0)).child(
                                                            gpui_component::link::Link::new(
                                                                "update-download-link",
                                                            )
                                                            .href(url)
                                                            .text_size(px(11.0))
                                                            .child(label),
                                                        ),
                                                    );
                                                }
                                                col
                                            }),
                                    )
                                    .child({
                                        let actions = div().flex().items_center().gap(px(6.0));

                                        // The notify-only updater
                                        // (Windows + Linux) shows
                                        // "Update now" when the
                                        // latest state has a fresh
                                        // version. macOS uses
                                        // Sparkle's own dialog
                                        // instead.
                                        #[cfg(any(target_os = "windows", target_os = "linux"))]
                                        let actions = if matches!(
                                            &latest_state,
                                            crate::updater::CheckState::UpdateAvailable { .. }
                                        ) {
                                            actions.child(
                                                Button::new("apply-update")
                                                    .small()
                                                    .primary()
                                                    .label("Update now")
                                                    .on_click(cx.listener(
                                                        |_this, _, _window, _cx| {
                                                            crate::updater::apply_update_in_place();
                                                        },
                                                    )),
                                            )
                                        } else {
                                            actions
                                        };

                                        actions.child(
                                            Button::new("check-updates")
                                                .small()
                                                .ghost()
                                                .disabled(!updater_status.can_check_manually())
                                                .label("Check for Updates")
                                                .on_click(cx.listener(
                                                    |_this, _, _window, _cx| {
                                                        crate::updater::check_for_updates();
                                                    },
                                                )),
                                        )
                                    }),
                            ),
                    ),
            );
        }

        container
        // Continuity
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Continuity", &theme))
                .child(card(theme, card_opacity).child(toggle_row(
                    "Restore Terminal Text",
                    "Keep terminal text on restart continuity.",
                    Switch::new("restore-terminal-text-toggle")
                        .checked(self.config.appearance.restore_terminal_text)
                        .small()
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.set_restore_terminal_text(*checked, cx);
                        })),
                    theme,
                ))),
        )
        // Skills paths
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Skills", &theme))
                .child(
                    card(theme, card_opacity)
                        .child(
                            div()
                                .px(px(16.0))
                                .py(px(13.0))
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child("Skill Sources"),
                                )
                                .child(
                                    div()
                                        .max_w(px(500.0))
                                        .whitespace_normal()
                                        .text_size(px(11.5))
                                        .line_height(px(17.0))
                                        .text_color(theme.muted_foreground.opacity(0.65))
                                        .child("Con scans these folders for slash-command skills. Project paths follow the active working directory; global paths are always available."),
                                ),
                        )
                        .child(row_separator(theme))
                        // Project paths row
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .px(px(16.0))
                                .py(px(12.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(div().text_sm().child("Project paths"))
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(theme.muted_foreground.opacity(0.5))
                                                .child("relative to cwd"),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap(px(4.0))
                                        .children(project_chips)
                                        .children(project_presets),
                                ),
                        )
                        .child(row_separator(theme))
                        // Global paths row
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .px(px(16.0))
                                .py(px(12.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(div().text_sm().child("Global paths"))
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(theme.muted_foreground.opacity(0.5))
                                                .child("~ expanded to home"),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap(px(4.0))
                                        .children(global_chips)
                                        .children(global_presets),
                                ),
                        ),
                ),
        )
        // Network / proxy
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Network", &theme))
                .child(
                    card(theme, card_opacity)
                        .child(row_field("HTTP Proxy", &self.http_proxy_input))
                        .child(row_separator(theme))
                        .child(row_field("HTTPS Proxy", &self.https_proxy_input)),
                ),
        )
    }

    /// Render removable path chips for a given path list.
    fn render_path_chips(
        &self,
        kind: &str,
        paths: &[String],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = cx.theme();
        let chip_bg = theme.muted.opacity(0.06);
        let chip_hover_bg = theme.muted.opacity(0.10);
        let fg = theme.foreground;
        let muted = theme.muted_foreground.opacity(0.5);
        let danger = theme.danger;

        paths
            .iter()
            .enumerate()
            .map(|(idx, path)| {
                let kind = kind.to_string();
                let path_display = path.clone();

                div()
                    .id(SharedString::from(format!("skill-{kind}-{idx}")))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .h(px(24.0))
                    .px(px(7.0))
                    .rounded(px(5.0))
                    .bg(chip_bg)
                    .hover(move |s| s.bg(chip_hover_bg))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(fg)
                            .child(path_display),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("skill-rm-{kind}-{idx}")))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(14.0))
                            .rounded(px(3.0))
                            .cursor_pointer()
                            .text_color(muted)
                            .hover(|s| s.bg(danger.opacity(0.10)).text_color(danger))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.remove_skill_path(&kind, idx);
                                    cx.notify();
                                }),
                            )
                            .child(
                                svg()
                                    .path("phosphor/x.svg")
                                    .size(px(10.0))
                                    .text_color(muted),
                            ),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// Render preset quick-add buttons for paths not yet in the list.
    fn render_path_presets(
        &self,
        kind: &str,
        current_paths: &[String],
        presets: &[(&str, &str)],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = cx.theme();
        let muted_fg = theme.muted_foreground.opacity(0.4);
        let hover_bg = theme.muted.opacity(0.08);
        let hover_fg = theme.foreground;

        presets
            .iter()
            .filter(|(path, _)| !current_paths.iter().any(|p| p == path))
            .map(|(path, label)| {
                let kind = kind.to_string();
                let path = path.to_string();
                let label = label.to_string();

                div()
                    .id(SharedString::from(format!("skill-add-{kind}-{label}")))
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .h(px(24.0))
                    .px(px(8.0))
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .text_color(muted_fg)
                    .hover(|s| s.bg(hover_bg).text_color(hover_fg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.add_skill_path(&kind, &path);
                            cx.notify();
                        }),
                    )
                    .child(
                        svg()
                            .path("phosphor/plus.svg")
                            .size(px(10.0))
                            .text_color(muted_fg),
                    )
                    .child(div().text_size(px(10.0)).child(label))
                    .into_any_element()
            })
            .collect()
    }

    fn add_skill_path(&mut self, kind: &str, path: &str) {
        let paths = match kind {
            "project" => &mut self.config.skills.project_paths,
            "global" => &mut self.config.skills.global_paths,
            _ => return,
        };
        if !paths.iter().any(|p| p == path) {
            paths.push(path.to_string());
        }
    }

    fn remove_skill_path(&mut self, kind: &str, idx: usize) {
        let paths = match kind {
            "project" => &mut self.config.skills.project_paths,
            "global" => &mut self.config.skills.global_paths,
            _ => return,
        };
        if idx < paths.len() {
            paths.remove(idx);
        }
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> Div {
        let current_theme = self.config.terminal.theme.clone();
        let terminal_font_select = self.terminal_font_select.clone();
        let ui_font_select = self.ui_font_select.clone();
        let font_size_input = self.font_size_input.clone();
        let ui_font_size_input = self.ui_font_size_input.clone();
        let terminal_opacity_slider = self.terminal_opacity_slider.clone();
        let ui_opacity_slider = self.ui_opacity_slider.clone();
        let tab_accent_inactive_alpha_slider = self.tab_accent_inactive_alpha_slider.clone();
        let tab_accent_inactive_hover_alpha_slider =
            self.tab_accent_inactive_hover_alpha_slider.clone();
        let background_image_input = self.background_image_input.clone();
        let background_image_opacity_slider = self.background_image_opacity_slider.clone();
        let background_image_position_select = self.background_image_position_select.clone();
        let background_image_fit_select = self.background_image_fit_select.clone();
        let terminal_opacity = self.terminal_opacity_value();
        let ui_opacity = self.ui_opacity_value();
        let tab_accent_inactive_alpha = self.tab_accent_inactive_alpha_value();
        let tab_accent_inactive_hover_alpha = self.tab_accent_inactive_hover_alpha_value();
        let background_image_opacity = self.background_image_opacity_value();
        let card_opacity = self.card_opacity();
        let image_repeat_toggle = Switch::new("background-image-repeat")
            .checked(self.background_image_repeat)
            .small()
            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                this.background_image_repeat = *checked;
                this.config.appearance.background_image_repeat = *checked;
                cx.emit(AppearancePreview);
                cx.notify();
            }));
        let all_themes = con_terminal::TerminalTheme::all_available();

        // Split into built-in and user themes
        let builtin_names: Vec<&str> = con_terminal::TerminalTheme::available().to_vec();
        let mut builtin_themes = Vec::new();
        let mut user_themes = Vec::new();
        for t in all_themes.iter() {
            if builtin_names.contains(&t.name.as_str()) {
                builtin_themes.push(t);
            } else {
                user_themes.push(t);
            }
        }
        let has_user_themes = !user_themes.is_empty();
        let total_count = builtin_themes.len() + user_themes.len();

        // Build theme grids first (these need &mut cx for listeners)
        let builtin_grid = self.render_theme_grid(&builtin_themes, &current_theme, cx);
        let user_grid = if has_user_themes {
            Some(self.render_theme_grid(&user_themes, &current_theme, cx))
        } else {
            None
        };

        // Build import section
        let custom_theme_name_input = self.custom_theme_name_input.clone();
        let paste_btn = Button::new("paste-theme-btn")
            .label("Load from Clipboard")
            .icon(Icon::default().path("phosphor/clipboard-text.svg"))
            .small()
            .ghost()
            .on_click(cx.listener(|this, _, window, cx| {
                this.paste_theme_from_clipboard(window, cx);
            }));
        let browse_background_image_btn = Button::new("browse-background-image")
            .label("Browse…")
            .icon(Icon::default().path("phosphor/folder-open.svg"))
            .small()
            .ghost()
            .on_click(cx.listener(|this, _, window, cx| {
                this.browse_background_image(window, cx);
            }));
        let open_catalog_btn = Button::new("theme-catalog-link")
            .label("Browse Themes")
            .icon(Icon::default().path("phosphor/arrow-square-out.svg"))
            .small()
            .ghost()
            .on_click(cx.listener(|_, _, _, cx| {
                cx.open_url("https://ghostty-style.vercel.app/");
            }));
        let preview_card = self
            .custom_theme_preview
            .as_ref()
            .map(|preview| self.render_single_theme_card(preview, false, cx));

        let preview_actions: Option<AnyElement> = if let Some(card) = preview_card {
            let apply_btn = Button::new("apply-custom-theme")
                .label("Save & Apply")
                .icon(Icon::default().path("phosphor/check.svg"))
                .small()
                .primary()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.apply_custom_theme(cx);
                }));
            let preview_btn = Button::new("preview-custom-theme")
                .label("Preview")
                .icon(Icon::default().path("phosphor/eye.svg"))
                .small()
                .ghost()
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(ref preview) = this.custom_theme_preview {
                        cx.emit(ThemePreview(preview.name.clone()));
                    }
                }));
            Some(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.0))
                    .child(card)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(apply_btn)
                            .child(preview_btn),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };

        // Now all mutable borrows are done — get theme for pure layout
        let theme = cx.theme();

        let mut import_section = div()
            .px(px(18.0))
            .py(px(16.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Import Theme"),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .line_height(px(18.0))
                    .text_color(theme.muted_foreground.opacity(0.6))
                    .child("Browse community Ghostty themes, copy, and paste here."),
            )
            // Name input
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground.opacity(0.5))
                            .child("Theme name"),
                    )
                    .child(Input::new(&custom_theme_name_input)),
            )
            // Action buttons — compact row
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(paste_btn)
                    .child(open_catalog_btn),
            );

        // Preview card with save/preview actions
        if let Some(preview) = preview_actions {
            import_section = import_section.child(div().pt(px(4.0)).child(preview));
        }

        if let Some(ref status) = self.custom_theme_status {
            import_section = import_section.child(
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(6.0))
                    .bg(if status.starts_with("Error") {
                        theme.danger.opacity(0.08)
                    } else {
                        theme.success.opacity(0.08)
                    })
                    .text_size(px(11.0))
                    .line_height(px(17.0))
                    .text_color(if status.starts_with("Error") {
                        theme.danger
                    } else {
                        theme.success
                    })
                    .child(status.clone()),
            );
        }

        let mut content = section_content(
            "Appearance",
            "Tweak the Con's textures, tastes and feels.",
            theme,
        );

        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Fonts", &theme))
                .child(
                    card(theme, card_opacity)
                        .child(searchable_select_row(
                            "Terminal Font",
                            "Terminal and mono UI like code blocks.",
                            &terminal_font_select,
                            "Search fonts…",
                            theme,
                        ))
                        .child(row_separator(theme))
                        .child(searchable_select_row(
                            "UI Font",
                            "Settings, prose, and other UI.",
                            &ui_font_select,
                            "Search fonts…",
                            theme,
                        ))
                        .child(row_separator(theme))
                        .child(row_field("UI Size", &ui_font_size_input))
                        .child(row_separator(theme))
                        .child(row_field("Terminal Size", &font_size_input)),
                ),
        );

        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Cursor", &theme))
                .child(
                    card(theme, card_opacity).child(div().px(px(16.0)).child(select_row(
                        "Cursor Style",
                        "Choose how the terminal insertion point is drawn.",
                        &self.cursor_style_select,
                        theme,
                    ))),
                ),
        );

        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Transparency", &theme))
                .child(
                    card(theme, card_opacity)
                        .child(slider_row(
                            "Terminal Glass",
                            "How much of the desktop shows through the terminal.",
                            &terminal_opacity_slider,
                            terminal_opacity,
                            theme,
                        ))
                        .child(row_separator(theme))
                        .child(toggle_row(
                            "Terminal Blur",
                            if cfg!(target_os = "linux") {
                                "Disabled on Linux until rounded compositor blur regions are available."
                            } else {
                                "Blur the desktop behind transparent terminal surfaces."
                            },
                            {
                                let terminal_blur_supported = Self::terminal_blur_supported();
                                let mut toggle = Switch::new("terminal-blur-toggle")
                                    .checked(Self::effective_terminal_blur(self.terminal_blur))
                                    .small()
                                    .disabled(!terminal_blur_supported);

                                if terminal_blur_supported {
                                    toggle = toggle.on_click(cx.listener(
                                        |this, checked: &bool, _, cx| {
                                            this.terminal_blur = *checked;
                                            this.config.appearance.terminal_blur = *checked;
                                            cx.emit(AppearancePreview);
                                            cx.notify();
                                        },
                                    ));
                                }

                                toggle
                            },
                            theme,
                        ))
                        .child(row_separator(theme))
                        .child(slider_row(
                            "Window Chrome",
                            "Opacity for tabs, panels, and window controls.",
                            &ui_opacity_slider,
                            ui_opacity,
                            theme,
                        )),
                ),
        );

        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Pane", &theme))
                .child(
                    card(theme, card_opacity)
                        .child(toggle_row(
                            "Vertical Tabs",
                            "Use the left sidebar for workspace tabs.",
                            Switch::new("vertical-tabs-toggle")
                                .checked(
                                    self.config.appearance.tabs_orientation
                                        == TabsOrientation::Vertical,
                                )
                                .small()
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.config.appearance.tabs_orientation = if *checked {
                                        TabsOrientation::Vertical
                                    } else {
                                        TabsOrientation::Horizontal
                                    };
                                    cx.emit(AppearancePreview);
                                    cx.notify();
                                })),
                            theme,
                        ))
                        .child(row_separator(theme))
                        .child(toggle_row(
                            "Hide Pane Title Bar",
                            "Hide the title bar on split panes.",
                            Switch::new("hide-pane-title-bar-toggle")
                                .checked(self.hide_pane_title_bar)
                                .small()
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    let previous = this.config.appearance.hide_pane_title_bar;
                                    this.hide_pane_title_bar = *checked;
                                    this.config.appearance.hide_pane_title_bar = *checked;
                                    if let Err(err) = this.config.save() {
                                        this.hide_pane_title_bar = previous;
                                        this.config.appearance.hide_pane_title_bar = previous;
                                        log::warn!(
                                            "settings: persist hide_pane_title_bar failed: {err}"
                                        );
                                        this.save_error = Some(err.to_string());
                                        this.save_error_kind = Some(SettingsSaveErrorKind::Other);
                                        cx.notify();
                                        return;
                                    }
                                    if let Some(snapshot) = &mut this.preview_snapshot {
                                        snapshot.appearance.hide_pane_title_bar = *checked;
                                    }
                                    this.save_error = None;
                                    this.save_error_kind = None;
                                    cx.emit(AppearancePreview);
                                    cx.notify();
                                })),
                            theme,
                        ))
                        .child(row_separator(theme))
                        .child(slider_row(
                            "Inactive Accent",
                            "Accent strength for inactive tabs and unfocused pane titles.",
                            &tab_accent_inactive_alpha_slider,
                            tab_accent_inactive_alpha,
                            theme,
                        ))
                        .child(row_separator(theme))
                        .child(slider_row(
                            "Hover Accent",
                            "Accent strength when hovering inactive tabs.",
                            &tab_accent_inactive_hover_alpha_slider,
                            tab_accent_inactive_hover_alpha,
                            theme,
                        )),
                ),
        );

        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Background Image", &theme))
                .child(
                    card(theme, card_opacity)
                        .child(
                            div()
                                .px(px(16.0))
                                .pt(px(12.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(2.0))
                                                .child(
                                                    div()
                                                        .text_size(px(11.5))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child("Image Path"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.5))
                                                        .line_height(px(16.0))
                                                        .text_color(
                                                            theme.muted_foreground.opacity(0.65),
                                                        )
                                                        .child(
                                                            "Choose a PNG or JPEG. The image is applied per terminal.",
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .child(Input::new(&background_image_input)),
                                                )
                                                .child(browse_background_image_btn),
                                        ),
                                ),
                        )
                        .child(row_separator(theme))
                        .child(
                            div()
                                .px(px(16.0))
                                .child(
                                    select_row(
                                        "Fit",
                                        "Choose how the image fills the terminal.",
                                        &background_image_fit_select,
                                        theme,
                                    ),
                                ),
                        )
                        .child(row_separator(theme))
                        .child(
                            div()
                                .px(px(16.0))
                                .child(
                                    select_row(
                                        "Position",
                                        "Anchor if not filling the full surface.",
                                        &background_image_position_select,
                                        theme,
                                    ),
                                ),
                        )
                        .child(row_separator(theme))
                        .child(
                            toggle_row(
                                "Repeat",
                                "Tile if the fit leaves empty space around it.",
                                image_repeat_toggle,
                                theme,
                            ),
                        )
                        .child(row_separator(theme))
                        .child(slider_row(
                            "Image Strength",
                            "Blend more softly or let come forward.",
                            &background_image_opacity_slider,
                            background_image_opacity,
                            theme,
                        ))
                        .child(row_separator(theme))
                        .child(
                            div()
                                .px(px(16.0))
                                .pb(px(12.0))
                                .text_size(px(11.0))
                                .line_height(px(16.0))
                                .text_color(theme.muted_foreground.opacity(0.65))
                                .child(
                                    "Ghostty renders the image per terminal.",
                                ),
                        ),
                ),
        );

        // ── Built-in themes ──
        let mut theme_card_inner = div()
            .px(px(16.0))
            .py(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(12.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Terminal Theme"),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme.muted_foreground.opacity(0.5))
                            .child(format!("{total_count} themes")),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.muted_foreground.opacity(0.4))
                    .mb(px(10.0))
                    .child("You can also import community-maintained Ghostty styles."),
            )
            .child(builtin_grid);

        // User-installed themes
        if let Some(user_grid) = user_grid {
            theme_card_inner = theme_card_inner.child(
                div()
                    .mt(px(16.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .mb(px(10.0))
                            .child(
                                svg()
                                    .path("phosphor/folder.svg")
                                    .size(px(12.0))
                                    .text_color(theme.muted_foreground.opacity(0.5)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme.muted_foreground.opacity(0.6))
                                    .child("Installed"),
                            ),
                    )
                    .child(user_grid),
            );
        }
        content = content.child(card(theme, card_opacity).child(theme_card_inner));
        content = content.child(card(theme, card_opacity).child(import_section));

        content
    }

    /// Render a grid of theme preview cards.
    fn render_theme_grid(
        &self,
        themes: &[&con_terminal::TerminalTheme],
        current_theme: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut grid = div().flex().flex_wrap().gap(px(10.0));
        for term_theme in themes.iter() {
            let is_sel = term_theme.name.as_str() == current_theme;
            grid = grid.child(self.render_single_theme_card(term_theme, is_sel, cx));
        }
        grid
    }

    /// Render a single theme preview card.
    fn render_single_theme_card(
        &self,
        term_theme: &con_terminal::TerminalTheme,
        is_sel: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let name = term_theme.name.clone();
        let theme_name = name.clone();
        let bg = term_theme.background;
        let fg = term_theme.foreground;
        let bg_gpui = gpui::rgb(bg.to_u32());
        let fg_gpui = gpui::rgb(fg.to_u32());
        let green = gpui::rgb(term_theme.ansi[2].to_u32());
        let cyan = gpui::rgb(term_theme.ansi[6].to_u32());
        let blue = gpui::rgb(term_theme.ansi[4].to_u32());
        let yellow = gpui::rgb(term_theme.ansi[3].to_u32());
        let red = gpui::rgb(term_theme.ansi[1].to_u32());
        let magenta = gpui::rgb(term_theme.ansi[5].to_u32());

        let terminal_preview = div()
            .flex()
            .flex_col()
            .bg(bg_gpui)
            .rounded_t(px(8.0))
            .px(px(8.0))
            .pt(px(6.0))
            .pb(px(6.0))
            .gap(px(1.0))
            .font_family(theme.mono_font_family.clone())
            .text_size(px(8.0))
            .line_height(px(12.0))
            .child(
                div()
                    .flex()
                    .gap(px(3.0))
                    .pb(px(4.0))
                    .child(div().size(px(5.0)).rounded_full().bg(red))
                    .child(div().size(px(5.0)).rounded_full().bg(yellow))
                    .child(div().size(px(5.0)).rounded_full().bg(green)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(3.0))
                    .child(div().text_color(green).child("$"))
                    .child(div().text_color(cyan).child("git"))
                    .child(div().text_color(fg_gpui).child("log --oneline")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(3.0))
                    .child(div().text_color(yellow).child("a1b2c3d"))
                    .child(div().text_color(fg_gpui).child("feat: init")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(3.0))
                    .child(div().text_color(yellow).child("e4f5g6h"))
                    .child(div().text_color(fg_gpui).child("fix: theme")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(3.0))
                    .child(div().text_color(green).child("$"))
                    .child(div().text_color(blue).child("ls"))
                    .child(div().text_color(fg_gpui).child("src/")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(4.0))
                    .child(div().text_color(blue).child("lib/"))
                    .child(div().text_color(magenta).child("main.rs"))
                    .child(div().text_color(fg_gpui).child("README")),
            );

        let mut palette_strip = div().flex().h(px(4.0));
        for idx in 0..16 {
            let c = term_theme.ansi[idx];
            palette_strip = palette_strip.child(div().flex_1().h_full().bg(gpui::rgb(c.to_u32())));
        }

        let display_name = display_theme_name(&name);

        div()
            .id(SharedString::from(format!("term-theme-{name}")))
            .cursor_pointer()
            .w(px(150.0))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .overflow_hidden()
            .bg(if is_sel {
                theme.primary.opacity(0.10)
            } else {
                theme.muted.opacity(0.04)
            })
            .hover(|s| s.bg(theme.primary.opacity(0.06)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.config.terminal.theme = theme_name.clone();
                    cx.emit(ThemePreview(theme_name.clone()));
                    cx.notify();
                }),
            )
            .child(terminal_preview)
            .child(palette_strip)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(4.0))
                    .h(px(26.0))
                    .text_size(px(10.5))
                    .font_weight(if is_sel {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if is_sel {
                        theme.primary
                    } else {
                        theme.muted_foreground
                    })
                    .children(if is_sel {
                        Some(
                            svg()
                                .path("phosphor/check.svg")
                                .size(px(10.0))
                                .text_color(theme.primary),
                        )
                    } else {
                        None
                    })
                    .child(display_name),
            )
    }

    fn render_ai(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let card_opacity = self.card_opacity();
        let max_turns_input = self.max_turns_input.clone();
        let temperature_input = self.temperature_input.clone();
        let active_provider_select = self.active_provider_select.clone();
        let active_model_select = self.active_model_select.clone();
        let suggestion_provider_select = self.suggestion_provider_select.clone();
        let suggestion_model_select = self.suggestion_model_select.clone();
        let routing_card = card(theme, card_opacity)
            .child(
                div()
                    .px(px(16.0))
                    .py(px(13.0))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Routing"),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .line_height(px(17.0))
                            .text_color(theme.muted_foreground.opacity(0.65))
                            .child("Choose a default model for agent and the fast path for inline command suggestions."),
                    ),
            )
            .child(row_separator(theme))
            .child(searchable_select_row(
                    "Active Provider",
                    "Default provider for the agent panel, command palette actions, and AI fallback suggestions.",
                    &active_provider_select,
                    "Select a provider…",
                    theme,
                ))
            .child(searchable_select_row(
                    "Active Model",
                    "Model override for the currently active provider.",
                    &active_model_select,
                    "Select a model…",
                    theme,
                ))
            .child(toggle_row(
                    "Auto-Approve Tools",
                    "Allow the agent to run tools without per-action approval.",
                    Switch::new("auto-approve-toggle")
                        .checked(self.auto_approve)
                        .small()
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.auto_approve = *checked;
                            cx.notify();
                        })),
                    theme,
                ))
            .child(toggle_row(
                    "AI Command Suggestions",
                    "Use the suggestion provider only when local command history has no strong match.",
                    Switch::new("ai-suggestion-toggle")
                        .checked(self.suggestion_enabled)
                        .small()
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.suggestion_enabled = *checked;
                            cx.notify();
                        })),
                    theme,
                ))
            .child(
                    div()
                        .opacity(if self.suggestion_enabled { 1.0 } else { 0.55 })
                        .child(searchable_select_row(
                            "Suggestions Provider",
                            "Route inline command completion to the active provider or a faster secondary host.",
                            &suggestion_provider_select,
                            "Select a provider…",
                            theme,
                        )),
                )
            .child(
                    div()
                        .opacity(if self.suggestion_enabled { 1.0 } else { 0.55 })
                        .child(searchable_select_row(
                            "Suggestions Model",
                            "Choose a faster or cheaper model for command completion when history has no strong match.",
                            &suggestion_model_select,
                            "Select a suggestion model…",
                            theme,
                        )),
                );

        let behavior_card = card(theme, card_opacity).child(
            div()
                .px(px(14.0))
                .py(px(12.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Behavior"),
                )
                .child(stacked_input_field(
                    "Max Turns",
                    "Tool-use turns before the session is forced to stop.",
                    &max_turns_input,
                    theme,
                ))
                .child(stacked_input_field(
                    "Temperature",
                    "Blank for provider default.",
                    &temperature_input,
                    theme,
                )),
        );

        let ai_layout = div()
            .flex()
            .flex_col()
            .flex_1()
            .gap(px(12.0))
            .child(routing_card)
            .child(behavior_card);

        section_content("AI", "Model selection and AI harness configuration.", theme)
            .child(ai_layout)
    }

    fn render_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let card_opacity = self.card_opacity();
        let viewport_w = window.viewport_size().width.as_f32();
        let compact = viewport_w < 980.0;
        let narrow = viewport_w < 840.0;
        let settings_sidebar_w = if narrow {
            48.0
        } else if compact {
            144.0
        } else {
            160.0
        };
        let settings_content_pad = if narrow {
            14.0
        } else if compact {
            18.0
        } else {
            24.0
        };
        let settings_surface_w = if self.standalone {
            viewport_w
        } else {
            ((viewport_w * 0.76).clamp(680.0, 980.0)).min((viewport_w - 32.0).max(320.0))
        };
        let provider_content_w =
            (settings_surface_w - settings_sidebar_w - settings_content_pad * 2.0).max(0.0);
        let provider_sidebar_w = if provider_content_w < 600.0 {
            px(148.0)
        } else {
            px(180.0)
        };
        let model_input = self.model_input.clone();
        let api_key_input = self.api_key_input.clone();
        let base_url_input = self.base_url_input.clone();
        let max_tokens_input = self.max_tokens_input.clone();
        let models = self.registry.models_for_base_url(
            &self.selected_provider,
            Self::provider_base_url(&self.config, &self.selected_provider),
        );
        let model_select = self.model_select.clone();
        let endpoint_preset_select = self.endpoint_preset_select.clone();
        let endpoint_presets = Self::provider_endpoint_presets(&self.selected_provider);
        let can_fetch_models = matches!(
            self.selected_provider,
            ProviderKind::OpenAICompatible | ProviderKind::ChatGPT
        );
        let protocol_switch_label = Self::protocol_switch_label(&self.selected_provider);
        let protocol_switch_hint = Self::protocol_switch_hint(&self.selected_provider);
        let anthropic_protocol_enabled = Self::uses_anthropic_protocol(&self.selected_provider);

        let mut provider_list = div().flex().flex_col();
        let active_sidebar_provider = Self::sidebar_provider_kind(&self.selected_provider);
        for provider in SIDEBAR_PROVIDERS.iter() {
            let is_selected = *provider == active_sidebar_provider;
            let is_configured = self.provider_is_configured(provider, cx);
            let label = provider_label(provider);
            let icon_path = Self::provider_icon_path(provider);
            let icon_color = if is_selected {
                theme.primary
            } else if is_configured {
                theme.foreground.opacity(0.76)
            } else {
                theme.muted_foreground.opacity(0.38)
            };
            let label_color = if is_selected {
                theme.foreground
            } else if is_configured {
                theme.foreground.opacity(0.74)
            } else {
                theme.muted_foreground.opacity(0.52)
            };
            let status_color = if is_configured {
                theme.success.opacity(if is_selected { 0.92 } else { 0.74 })
            } else {
                theme.muted_foreground.opacity(0.18)
            };
            let provider_clone = provider.clone();

            provider_list = provider_list.child(
                div()
                    .id(SharedString::from(format!("prov-{label}")))
                    .h(px(34.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .bg(if is_selected {
                        theme.primary.opacity(0.08)
                    } else {
                        theme.transparent
                    })
                    .hover(|s| {
                        if is_selected {
                            s
                        } else {
                            s.bg(theme.muted.opacity(0.06))
                        }
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.select_provider(provider_clone.clone(), window, cx);
                        }),
                    )
                    .child(
                        div()
                            .w(px(2.0))
                            .h(px(16.0))
                            .rounded(px(1.0))
                            .bg(if is_selected {
                                theme.primary
                            } else {
                                theme.transparent
                            }),
                    )
                    .child(
                        div()
                            .size(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .bg(if is_selected {
                                theme.primary.opacity(0.11)
                            } else if is_configured {
                                theme.muted.opacity(0.08)
                            } else {
                                theme.transparent
                            })
                            .child(svg().path(icon_path).size(px(13.0)).text_color(icon_color)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.0))
                            .line_height(px(14.0))
                            .font_weight(if is_selected {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(label_color)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(label),
                    )
                    .child(div().size(px(6.0)).rounded_full().bg(status_color)),
            );
        }

        let oauth_state = self.oauth_state(&self.selected_provider).cloned();
        let has_key_override = !self.api_key_input.read(cx).value().is_empty();
        let oauth_cache_present = Self::provider_oauth_cache_present(&Self::sidebar_provider_kind(
            &self.selected_provider,
        ));
        let connection_ready = if Self::provider_has_oauth(&self.selected_provider) {
            self.provider_oauth_ready(&self.selected_provider)
        } else {
            has_key_override
        };
        let connection_label = if Self::provider_has_oauth(&self.selected_provider) {
            match oauth_state.as_ref() {
                Some(state) if state.in_progress => "Signing In",
                Some(state) if state.connected => "Signed In",
                _ if oauth_cache_present => "Signed In",
                _ => "OAuth",
            }
        } else if has_key_override {
            "Ready"
        } else {
            "No key"
        };

        let model_card_content = card(theme, card_opacity).child(
            div()
                .px(px(14.0))
                .py(px(12.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Provider Default Model"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(div().size(px(6.0)).rounded_full().bg(if connection_ready {
                                    theme.success
                                } else {
                                    theme.muted_foreground.opacity(0.2)
                                }))
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(theme.muted_foreground.opacity(0.45))
                                        .child(connection_label),
                                ),
                        ),
                )
                .child(if models.is_empty() {
                    div()
                        .w_full()
                        .min_w_0()
                        .child(Input::new(&model_input).small())
                } else {
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div().w_full().min_w_0().child(
                                Select::new(&model_select)
                                    .placeholder("Select a known model…")
                                    .small(),
                            ),
                        )
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .child(Input::new(&model_input).small()),
                        )
                })
                .children(can_fetch_models.then(|| {
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .pt(px(2.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(220.0))
                                .text_size(px(11.0))
                                .line_height(px(16.0))
                                .text_color(if self.provider_model_status_error {
                                    theme.danger
                                } else {
                                    theme.muted_foreground.opacity(0.62)
                                })
                                .child(
                                    self.provider_model_status.clone().unwrap_or_else(|| {
                                        if self.selected_provider == ProviderKind::ChatGPT {
                                            "Fetch the current ChatGPT Subscription model list after OAuth sign-in. Or enter the model ID.".to_string()
                                        } else {
                                            "Fetch /models when the provider exposes a model list. Or enter the model ID.".to_string()
                                        }
                                    }),
                                ),
                        )
                        .child(
                            div().flex_shrink_0().child(
                                Button::new("fetch-openai-compatible-models")
                                    .small()
                                    .ghost()
                                    .label("Fetch Models")
                                    .loading(self.provider_model_fetching)
                                    .disabled(self.provider_model_fetching)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.fetch_selected_provider_models(window, cx);
                                    })),
                            ),
                        )
                })),
        );

        let right_col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(12.0))
            .child(model_card_content)
            .child(
                card(theme, card_opacity).child(
                    div()
                        .px(px(14.0))
                        .py(px(12.0))
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Connection"),
                        )
                        .children(protocol_switch_label.map(|switch_label| {
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(12.0))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(2.0))
                                                .child(div().text_sm().child("Protocol"))
                                                .child(
                                                    div()
                                                        .text_size(px(11.0))
                                                        .text_color(theme.muted_foreground)
                                                        .child(protocol_switch_hint.unwrap_or(
                                                            "Choose the provider transport.",
                                                        )),
                                                ),
                                        )
                                        .child(
                                            Switch::new(format!(
                                                "provider-protocol-{}",
                                                provider_label(&Self::sidebar_provider_kind(
                                                    &self.selected_provider
                                                ))
                                            ))
                                            .checked(anthropic_protocol_enabled)
                                            .label(switch_label)
                                            .on_click(cx.listener(
                                                |this, checked: &bool, window, cx| {
                                                    this.toggle_selected_provider_protocol(
                                                        *checked, window, cx,
                                                    );
                                                },
                                            )),
                                        ),
                                )
                                .into_any_element()
                        }))
                        .children(if protocol_switch_label.is_some() {
                            Some(div().child(row_separator(theme)))
                        } else {
                            None
                        })
                        .children(Self::provider_oauth_label(&self.selected_provider).map(
                            |provider_name| {
                                let oauth = oauth_state.clone().unwrap_or_default();
                                let oauth_ready =
                                    self.provider_oauth_ready(&self.selected_provider);
                                let provider_for_click = self.selected_provider.clone();
                                let prompt = oauth.prompt.clone();
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(12.0))
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(2.0))
                                                    .child(
                                                        div().text_sm().child(format!(
                                                            "{provider_name} OAuth"
                                                        )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(11.0))
                                                            .text_color(theme.muted_foreground)
                                                            .child("Device login"),
                                                    ),
                                            )
                                            .child(
                                                Button::new(format!(
                                                    "oauth-connect-{}",
                                                    provider_label(&self.selected_provider)
                                                ))
                                                .label(if oauth_ready && !oauth.in_progress {
                                                    "Reconnect"
                                                } else {
                                                    Self::provider_oauth_button_label(
                                                        &self.selected_provider,
                                                    )
                                                    .unwrap_or("Sign In")
                                                })
                                                .small()
                                                .primary()
                                                .loading(oauth.in_progress)
                                                .disabled(oauth.in_progress)
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.start_provider_oauth(
                                                            provider_for_click.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                            ),
                                    )
                                    .children(oauth.status_message.as_ref().map(|message| {
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(theme.muted_foreground)
                                            .child(message.clone())
                                    }))
                                    .children(oauth.error_message.as_ref().map(|message| {
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(theme.danger)
                                            .child(message.clone())
                                    }))
                                    .children(prompt.map(|prompt| {
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .p(px(10.0))
                                            .rounded(px(8.0))
                                            .bg(theme.muted.opacity(0.05))
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(theme.muted_foreground)
                                                    .child("Browser authorization"),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap(px(8.0))
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_col()
                                                            .gap(px(2.0))
                                                            .child(
                                                                div()
                                                                    .text_size(px(11.0))
                                                                    .text_color(
                                                                        theme.muted_foreground,
                                                                    )
                                                                    .child("Code"),
                                                            )
                                                            .child(
                                                                div()
                                                                    .font_weight(
                                                                        FontWeight::SEMIBOLD,
                                                                    )
                                                                    .child(
                                                                        prompt.user_code.clone(),
                                                                    ),
                                                            ),
                                                    )
                                                    .child(
                                                        Clipboard::new(format!(
                                                            "oauth-code-{}",
                                                            provider_label(&self.selected_provider)
                                                        ))
                                                        .value(SharedString::from(
                                                            prompt.user_code.clone(),
                                                        )),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap(px(8.0))
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .text_size(px(11.0))
                                                            .text_color(theme.muted_foreground)
                                                            .child(prompt.verification_uri.clone()),
                                                    )
                                                    .child(
                                                        Button::new(format!(
                                                            "oauth-open-{}",
                                                            provider_label(&self.selected_provider)
                                                        ))
                                                        .label("Open Browser")
                                                        .small()
                                                        .ghost()
                                                        .on_click(move |_, _, cx| {
                                                            cx.open_url(&prompt.verification_uri);
                                                        }),
                                                    ),
                                            )
                                    }))
                                    .into_any_element()
                            },
                        ))
                        .children(if Self::provider_has_oauth(&self.selected_provider) {
                            Some(div().child(row_separator(theme)))
                        } else {
                            None
                        })
                        .child(stacked_input_field(
                            Self::provider_api_key_label(&self.selected_provider),
                            Self::provider_api_key_hint(&self.selected_provider),
                            &api_key_input,
                            theme,
                        ))
                        .child(stacked_input_field(
                            "Base URL",
                            Self::provider_base_url_hint(&self.selected_provider),
                            &base_url_input,
                            theme,
                        ))
                        .children(if endpoint_presets.is_empty() {
                            None
                        } else {
                            Some(div().child(select_row(
                                "Endpoint Preset",
                                "Switch region or protocol",
                                &endpoint_preset_select,
                                theme,
                            )))
                        }),
                ),
            )
            .child(
                card(theme, card_opacity).child(
                    div()
                        .px(px(14.0))
                        .py(px(12.0))
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Provider Limits"),
                        )
                        .child(stacked_input_field(
                            "Max Tokens",
                            "Per-provider token ceiling.",
                            &max_tokens_input,
                            theme,
                        )),
                ),
            );

        let provider_column = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .w(provider_sidebar_w)
            .flex_shrink_0()
            .child(
                card(theme, card_opacity).child(div().px(px(4.0)).py(px(4.0)).child(provider_list)),
            );

        section_content(
            "Providers",
            "Manage credentials, endpoints, and provider-specific defaults independently from the app-wide AI behavior.",
            theme,
        )
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .gap(px(16.0))
                .child(provider_column)
                .child(right_col),
        )
    }

    fn render_keys(&mut self, cx: &mut Context<Self>) -> Div {
        let recording = self.recording_key.clone();
        let card_opacity = self.card_opacity();

        // Editable keybinding definitions: (label, field_name)
        let general_keys: &[(&str, &str)] = &[
            ("New Window", "new_window"),
            ("New Tab", "new_tab"),
            ("Next Tab", "next_tab"),
            ("Previous Tab", "previous_tab"),
            ("Close Tab", "close_tab"),
            ("Settings", "settings"),
            ("Command Palette", "command_palette"),
            ("Toggle Agent", "toggle_agent"),
            ("Toggle Input Bar", "toggle_input_bar"),
            ("Toggle Input / Terminal", "focus_input"),
            ("Cycle Input Mode", "cycle_input_mode"),
            ("Toggle Pane Scope", "toggle_pane_scope"),
            ("Toggle Left Sidebar", "toggle_left_panel"),
            ("Focus Files", "focus_files"),
            ("Search Files", "search_files"),
            ("Hide Left Sidebar", "collapse_sidebar"),
            ("Quit", "quit"),
        ];

        let pane_keys: &[(&str, &str)] = &[
            ("Split Right", "split_right"),
            ("Split Down", "split_down"),
            ("Toggle Pane Zoom", "toggle_pane_zoom"),
            ("Close Pane", "close_pane"),
        ];

        let surface_keys: &[(&str, &str)] = &[
            ("New Surface Tab", "new_surface"),
            ("New Surface Pane Right", "new_surface_split_right"),
            ("New Surface Pane Down", "new_surface_split_down"),
            ("Next Surface Tab", "next_surface"),
            ("Previous Surface Tab", "previous_surface"),
            ("Rename Surface", "rename_surface"),
            ("Close Surface", "close_surface"),
        ];

        let build_card = |keys: &[(&str, &str)],
                          recording: &Option<String>,
                          this: &mut Self,
                          cx: &mut Context<Self>|
         -> Div {
            let theme = cx.theme();
            let mut c = card(theme, card_opacity);
            for (i, (label, field)) in keys.iter().enumerate() {
                if i > 0 {
                    c = c.child(row_separator(theme));
                }
                let value = this.binding_value(field).to_string();
                let is_recording = recording.as_deref() == Some(*field);
                let badge = if is_recording {
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Press shortcut…")
                        .into_any_element()
                } else {
                    crate::keycaps::keycaps_for_binding(&value, theme)
                };
                let field_str = field.to_string();
                c = c.child(
                    div()
                        .id(SharedString::from(format!("key-{field}")))
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(16.0))
                        .h(px(34.0))
                        .hover(|s| s.bg(theme.muted.opacity(0.025)))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.foreground.opacity(0.86))
                                .child(label.to_string()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("key-badge-{field}")))
                                .min_h(px(23.0))
                                .px(px(4.0))
                                .flex()
                                .items_center()
                                .rounded(px(5.0))
                                .cursor_pointer()
                                .bg(if is_recording {
                                    theme.primary.opacity(0.12)
                                } else {
                                    theme.transparent
                                })
                                .text_color(if is_recording {
                                    theme.primary
                                } else {
                                    theme.muted_foreground
                                })
                                .hover(|s| s.bg(theme.muted.opacity(0.055)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.set_recording_key(Some(field_str.clone()));
                                        cx.notify();
                                    }),
                                )
                                .child(badge),
                        ),
                );
            }
            c
        };

        let general_card = build_card(general_keys, &recording, self, cx);
        let pane_card_keys = pane_keys;
        let pane_card = build_card(pane_card_keys, &recording, self, cx);
        let surface_card = build_card(surface_keys, &recording, self, cx);
        let global_summon_enabled = self.config.keybindings.global_summon_enabled;
        let global_summon_value = self.config.keybindings.global_summon.clone();
        let global_summon_recording = recording.as_deref() == Some("global_summon");
        #[cfg(target_os = "macos")]
        let quick_terminal_enabled = self.config.keybindings.quick_terminal_enabled;
        #[cfg(target_os = "macos")]
        let quick_terminal_value = self.config.keybindings.quick_terminal.clone();
        #[cfg(target_os = "macos")]
        let quick_terminal_recording = recording.as_deref() == Some("quick_terminal");
        let theme = cx.theme();

        let fixed_tab_card = card(theme, card_opacity).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                .px(px(16.0))
                .h(px(34.0))
                .hover(|s| s.bg(theme.muted.opacity(0.025)))
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.foreground.opacity(0.86))
                        .child("Select Tab by Number"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(crate::keycaps::keycaps_for_binding("secondary-1", theme))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted_foreground.opacity(0.55))
                                .child("…"),
                        )
                        .child(crate::keycaps::keycaps_for_binding("secondary-9", theme)),
                ),
        );
        #[cfg(target_os = "macos")]
        let fixed_tab_card = fixed_tab_card
            .child(row_separator(theme))
            .child(key_row("Minimize Window", "cmd-m", theme))
            .child(row_separator(theme))
            .child(key_row("Next Window", "cmd-`", theme))
            .child(row_separator(theme))
            .child(key_row("Previous Window", "cmd-shift-`", theme));

        let global_summon_badge = if global_summon_recording {
            div()
                .min_h(px(28.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded(px(8.0))
                .bg(theme.primary.opacity(0.10))
                .text_color(theme.primary)
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .child("Press shortcut…")
                .into_any_element()
        } else if !global_summon_value.trim().is_empty() {
            crate::keycaps::keycaps_for_binding(&global_summon_value, theme)
        } else {
            div()
                .min_h(px(28.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded(px(8.0))
                .bg(theme.muted.opacity(0.08))
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child("Not set")
                .into_any_element()
        };

        let global_summon_card = card(theme, card_opacity).child(
            div()
                .px(px(16.0))
                .py(px(13.0))
                .flex()
                .items_start()
                .justify_between()
                .gap(px(16.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .flex_1()
                        .max_w(px(430.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Global Hotkey"),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .line_height(px(17.0))
                                .text_color(theme.muted_foreground.opacity(0.68))
                                .child(
                                    "Show Con from anywhere in macOS. Press it again while Con is frontmost to hide the app.",
                                ),
                        ),
                )
                .child(
                    div().pt(px(1.0)).child(
                        Switch::new("global-summon-enabled")
                            .checked(global_summon_enabled)
                            .small()
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.config.keybindings.global_summon_enabled = *checked;
                                if *checked
                                    && this.config.keybindings.global_summon.trim().is_empty()
                                {
                                    this.config.keybindings.global_summon =
                                        "alt-space".to_string();
                                }
                                sync_keybinding_conflict_error(
                                    &mut this.save_error,
                                    &mut this.save_error_kind,
                                    &this.config.keybindings,
                                );
                                cx.notify();
                            })),
                    ),
                )
        )
        .child(row_separator(theme))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                .px(px(16.0))
                .py(px(11.0))
                .hover(|s| s.bg(theme.muted.opacity(0.035)))
                .text_color(if global_summon_enabled {
                    theme.foreground
                } else {
                    theme.muted_foreground
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .child("Shortcut"),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .line_height(px(15.0))
                                .text_color(theme.muted_foreground.opacity(0.62))
                                .child(if global_summon_enabled {
                                    "Use a low-conflict system shortcut. Option-Space is familiar, but may collide with launchers."
                                } else {
                                    "Off by default to avoid conflicts with other global shortcuts."
                                }),
                        ),
                )
                .child(
                    div()
                        .id("key-badge-global-summon")
                        .min_w(px(112.0))
                        .flex()
                        .justify_end()
                        .opacity(if global_summon_enabled { 1.0 } else { 0.45 })
                        .cursor_pointer()
                        .rounded(px(7.0))
                        .px(px(4.0))
                        .py(px(3.0))
                        .hover(|s| s.bg(theme.muted.opacity(0.08)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if this.config.keybindings.global_summon_enabled {
                                    this.set_recording_key(Some("global_summon".to_string()));
                                    cx.notify();
                                }
                            }),
                        )
                        .child(global_summon_badge),
                ),
        );

        #[cfg(target_os = "macos")]
        let quick_terminal_badge = if quick_terminal_recording {
            div()
                .min_h(px(28.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded(px(8.0))
                .bg(theme.primary.opacity(0.10))
                .text_color(theme.primary)
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .child("Press shortcut…")
                .into_any_element()
        } else if !quick_terminal_value.trim().is_empty() {
            crate::keycaps::keycaps_for_binding(&quick_terminal_value, theme)
        } else {
            div()
                .min_h(px(28.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded(px(8.0))
                .bg(theme.muted.opacity(0.08))
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child("Not set")
                .into_any_element()
        };

        #[cfg(target_os = "macos")]
        let quick_terminal_card = card(theme, card_opacity)
            .child(
                div()
                    .px(px(16.0))
                    .py(px(13.0))
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .flex_1()
                            .max_w(px(430.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Quick Terminal"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .line_height(px(17.0))
                                    .text_color(theme.muted_foreground.opacity(0.68))
                                    .child("Show a dedicated floating Con window that slides down from the top of the screen."),
                            ),
                    )
                    .child(
                        div().pt(px(1.0)).child(
                            Switch::new("hotkey-window-enabled")
                                .checked(quick_terminal_enabled)
                                .small()
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.config.keybindings.quick_terminal_enabled = *checked;
                                    if *checked
                                        && this.config.keybindings.quick_terminal.trim().is_empty()
                                    {
                                        this.config.keybindings.quick_terminal = "cmd-\\".to_string();
                                    }
                                    sync_keybinding_conflict_error(
                                        &mut this.save_error,
                                        &mut this.save_error_kind,
                                        &this.config.keybindings,
                                    );
                                    cx.notify();
                                })),
                        ),
                    ),
            )
            .child(row_separator(theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0))
                    .px(px(16.0))
                    .py(px(11.0))
                    .text_color(if quick_terminal_enabled {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Shortcut"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .line_height(px(15.0))
                                    .text_color(theme.muted_foreground.opacity(0.62))
                                    .child("Use a low-conflict macOS shortcut. Cmd-Backslash matches the requested default."),
                            ),
                    )
                    .child(
                        div()
                            .id("key-badge-hotkey-window")
                            .min_w(px(112.0))
                            .flex()
                            .justify_end()
                            .opacity(if quick_terminal_enabled { 1.0 } else { 0.45 })
                            .cursor_pointer()
                            .rounded(px(7.0))
                            .px(px(4.0))
                            .py(px(3.0))
                            .hover(|s| s.bg(theme.muted.opacity(0.08)))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                if this.config.keybindings.quick_terminal_enabled {
                                    this.set_recording_key(Some("quick_terminal".to_string()));
                                    cx.notify();
                                }
                            }))
                            .child(quick_terminal_badge),
                    ),
            );

        let shortcut_groups = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(group_label("Global", &theme))
            .child(global_summon_card);
        #[cfg(target_os = "macos")]
        let shortcut_groups = shortcut_groups.child(quick_terminal_card);
        let shortcut_groups = shortcut_groups
            .child(div().h(px(8.0)))
            .child(group_label("General", &theme))
            .child(general_card);

        section_content(
            "Keyboard Shortcuts",
            "Click a shortcut to record a new key combination.",
            theme,
        )
        .child(shortcut_groups)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Fixed Shortcuts", &theme))
                .child(fixed_tab_card),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Panes", &theme))
                .child(pane_card),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Surfaces", &theme))
                .child(surface_card),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(group_label("Terminal", &theme))
                .child(
                    card(theme, card_opacity)
                        // Terminal clipboard uses ⌘C/V on macOS and the
                        // Windows-Terminal-standard Ctrl+Shift+C/V on
                        // Windows (plain Ctrl+C would raise SIGINT in
                        // the shell). `secondary-` would collapse to
                        // Ctrl-only on Windows, so we branch explicitly.
                        .child(key_row(
                            "Copy",
                            if cfg!(target_os = "macos") {
                                "cmd-c"
                            } else {
                                "ctrl-shift-c"
                            },
                            theme,
                        ))
                        .child(row_separator(theme))
                        .child(key_row(
                            "Paste",
                            if cfg!(target_os = "macos") {
                                "cmd-v"
                            } else {
                                "ctrl-shift-v"
                            },
                            theme,
                        ))
                        .child(row_separator(theme))
                        .child(key_row("Select All", "secondary-a", theme)),
                ),
        )
    }
}

impl EventEmitter<SaveSettings> for SettingsPanel {}
impl EventEmitter<ThemePreview> for SettingsPanel {}
impl EventEmitter<AppearancePreview> for SettingsPanel {}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay_progress = self.overlay_motion.value(window);
        if overlay_progress <= 0.001 && !self.visible {
            return div().id("settings-overlay");
        }

        let active = self.active_section;

        // Render content first (AI needs &mut self)
        let content = match active {
            SettingsSection::General => self.render_general(cx),
            SettingsSection::Appearance => self.render_appearance(cx),
            SettingsSection::Ai => self.render_ai(cx),
            SettingsSection::Providers => self.render_providers(window, cx),
            SettingsSection::Keys => self.render_keys(cx),
        };

        let has_unsaved_changes = self.standalone && self.has_unsaved_changes(cx);
        let theme = cx.theme();
        let viewport = window.viewport_size();
        let viewport_w = viewport.width.as_f32();
        let viewport_h = viewport.height.as_f32();
        let compact = viewport_w < 980.0;
        let narrow = viewport_w < 840.0;
        let sidebar_w = if narrow {
            px(48.0)
        } else if compact {
            px(144.0)
        } else {
            px(160.0)
        };
        let content_pad = if narrow {
            px(14.0)
        } else if compact {
            px(18.0)
        } else {
            px(24.0)
        };
        // Uniform width for all sections — prevents position jumping when switching tabs
        let card_width = px(((viewport_w * 0.76).clamp(680.0, 980.0)).min(viewport_w - 32.0));
        let card_height = {
            let target = match active {
                SettingsSection::Appearance => (viewport_h * 0.82).clamp(440.0, 780.0),
                SettingsSection::Providers => (viewport_h * 0.80).clamp(440.0, 760.0),
                _ => (viewport_h * 0.76).clamp(420.0, 720.0),
            };
            px(target.min(viewport_h - 32.0))
        };

        // Sidebar
        let mut sidebar = div()
            .flex()
            .flex_col()
            .w(sidebar_w)
            .pt(px(8.0))
            .pb(px(12.0))
            .px(if narrow { px(4.0) } else { px(8.0) })
            .gap(px(2.0))
            .flex_shrink_0();

        for section in ALL_SECTIONS {
            let is_active = *section == active;
            let section_val = *section;
            let mut nav_item = div()
                .id(SharedString::from(format!("nav-{}", section.label())))
                .flex()
                .items_center()
                .h(px(32.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .bg(if is_active {
                    theme.muted.opacity(0.15)
                } else {
                    theme.transparent
                })
                .text_color(if is_active {
                    theme.foreground
                } else {
                    theme.muted_foreground
                })
                .font_weight(if is_active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .hover(|s| {
                    if is_active {
                        s
                    } else {
                        s.bg(theme.muted.opacity(0.08))
                    }
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.active_section = section_val;
                        cx.notify();
                    }),
                );

            if narrow {
                // Icon-only mode: centered icon, no label
                nav_item = nav_item.justify_center().size(px(36.0)).mx_auto().child(
                    svg()
                        .path(section.icon())
                        .size(px(16.0))
                        .text_color(if is_active {
                            theme.foreground
                        } else {
                            theme.muted_foreground
                        }),
                );
            } else {
                nav_item = nav_item
                    .gap(px(8.0))
                    .px(px(10.0))
                    .text_size(px(13.0))
                    .child(
                        svg()
                            .path(section.icon())
                            .size(px(15.0))
                            .text_color(if is_active {
                                theme.foreground
                            } else {
                                theme.muted_foreground
                            }),
                    )
                    .child(section.label());
            }

            sidebar = sidebar.child(nav_item);
        }

        let content_scroll = div()
            .id("settings-content-scroll")
            .flex_1()
            .overflow_y_scroll()
            .p(content_pad)
            .child(content);
        let save_button_tint = theme.foreground.opacity(if has_unsaved_changes {
            if theme.is_dark() { 0.88 } else { 0.82 }
        } else {
            if theme.is_dark() { 0.62 } else { 0.52 }
        });
        let config_button_tone =
            theme
                .foreground
                .opacity(if theme.is_dark() { 0.66 } else { 0.54 });
        let save_button_style = ButtonCustomVariant::new(cx)
            .color(save_button_tint.opacity(if has_unsaved_changes { 0.11 } else { 0.04 }))
            .foreground(save_button_tint)
            .hover(save_button_tint.opacity(if has_unsaved_changes { 0.16 } else { 0.04 }))
            .active(save_button_tint.opacity(if has_unsaved_changes { 0.20 } else { 0.04 }));
        let header_density = ui_density_scale(theme);
        let surface_rounding = if self.standalone { px(0.0) } else { px(12.0) };
        let header_left_padding = if self.standalone && cfg!(target_os = "macos") {
            px(78.0)
        } else {
            px(20.0)
        };
        let mut header_title_area = div()
            .id("settings-titlebar-drag-area")
            .flex()
            .items_center()
            .h_full()
            .flex_1()
            .min_w_0()
            .pl(header_left_padding)
            .pr(px(12.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .child("Settings"),
            );
        if self.standalone && cfg!(target_os = "macos") {
            header_title_area = header_title_area
                .window_control_area(WindowControlArea::Drag)
                .on_click(|event, window, _cx| {
                    if event.click_count() == 2 {
                        window.titlebar_double_click();
                    }
                });
        }
        let surface = div()
            .id("settings-card")
            .w(if self.standalone {
                px(viewport_w)
            } else {
                card_width
            })
            .h(if self.standalone {
                px(viewport_h)
            } else {
                card_height
            })
            .rounded(surface_rounding)
            .bg(theme.title_bar)
            .overflow_hidden()
            .flex()
            .flex_col()
            .occlude()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // If recording a keybinding, capture the keystroke.
                if this.recording_key.is_some() {
                    this.record_keystroke(&event.keystroke, cx);
                    return;
                }
                match event.keystroke.key.as_str() {
                    "escape" => {
                        if this.standalone {
                            if this.request_standalone_close(window, cx) {
                                window.remove_window();
                            }
                        } else {
                            this.save(window, cx);
                        }
                    }
                    "enter" if event.keystroke.modifiers.platform => {
                        this.save(window, cx);
                    }
                    "s" if event.keystroke.modifiers.platform => {
                        this.save(window, cx);
                    }
                    "w" if event.keystroke.modifiers.platform => {
                        if this.standalone {
                            if this.request_standalone_close(window, cx) {
                                window.remove_window();
                            }
                        } else {
                            this.save(window, cx);
                        }
                    }
                    _ => {}
                }
            }))
            // Header
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(44.0))
                            .child(header_title_area)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .flex_shrink_0()
                                    .pr(px(20.0))
                                    .children(self.standalone.then(|| {
                                        let (icon, label, tone) = if has_unsaved_changes {
                                            (
                                                "phosphor/warning.svg",
                                                "Unsaved",
                                                theme
                                                    .warning
                                                    .opacity(if theme.is_dark() { 0.96 } else { 0.92 }),
                                            )
                                        } else {
                                            (
                                                "phosphor/check-circle-fill.svg",
                                                "Saved",
                                                theme
                                                    .foreground
                                                    .opacity(if theme.is_dark() { 0.52 } else { 0.42 }),
                                            )
                                        };
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(5.0))
                                            .min_w(px(64.0))
                                            .justify_end()
                                            .px(px(7.0))
                                            .h(px(24.0))
                                            .rounded(px(6.0))
                                            .bg(if has_unsaved_changes {
                                                theme.warning.opacity(if theme.is_dark() {
                                                    0.095
                                                } else {
                                                    0.070
                                                })
                                            } else {
                                                theme.transparent
                                            })
                                            .child(
                                                svg()
                                                    .path(icon)
                                                    .size(px(12.0))
                                                    .text_color(tone),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .line_height(px(14.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(tone)
                                                    .whitespace_nowrap()
                                                    .child(label),
                                            )
                                    }))
                                    .child(
                                        Button::new("settings-open-config")
                                            .ghost()
                                            .small()
                                            .compact()
                                            .h(px(28.0 * header_density))
                                            .w(px(28.0 * header_density))
                                            .rounded(px(7.0 * header_density))
                                            .tooltip("Open config.toml")
                                            .child(
                                                svg()
                                                    .path("phosphor/file-text.svg")
                                                    .size(px(15.0 * header_density))
                                                    .text_color(config_button_tone),
                                            )
                                            .on_click(|_, _, cx| {
                                                let path = Config::config_path();
                                                // Ensure the file exists so the editor has something to open.
                                                if !path.exists() {
                                                    if let Some(parent) = path.parent() {
                                                        let _ = std::fs::create_dir_all(parent);
                                                    }
                                                    let _ = std::fs::write(&path, "");
                                                }
                                                match Url::from_file_path(&path) {
                                                    Ok(url) => cx.open_url(url.as_str()),
                                                    Err(()) => {
                                                        log::warn!(
                                                            "settings: failed to build file URL for {}",
                                                            path.display()
                                                        );
                                                    }
                                                }
                                            }),
                                    )
                                    .children(self.standalone.then(|| {
                                        Button::new("settings-apply")
                                            .small()
                                            .compact()
                                            .custom(save_button_style)
                                            .disabled(!has_unsaved_changes)
                                            .h(px(28.0 * header_density))
                                            .px(px(10.0 * header_density))
                                            .rounded(px(8.0 * header_density))
                                            .gap(px(5.0 * header_density))
                                            .tooltip("Save settings")
                                            .child(
                                                svg()
                                                    .path("phosphor/check.svg")
                                                    .size(px(12.0 * header_density))
                                                    .text_color(save_button_tint),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.0 * header_density))
                                                    .line_height(px(15.0 * header_density))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(save_button_tint)
                                                    .whitespace_nowrap()
                                                    .child("Save"),
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.save(window, cx);
                                            }))
                                    })),
                            ),
                    )
                    .child(div().h(px(1.0)).bg(theme.muted.opacity(0.10))),
            )
            .children(
                (self.standalone && self.close_confirmation_visible && has_unsaved_changes).then(
                    || {
                        div()
                            .id("settings-close-confirmation")
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .min_h(px(42.0))
                            .px(px(20.0))
                            .bg(theme.warning.opacity(if theme.is_dark() {
                                0.075
                            } else {
                                0.055
                            }))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .min_w_0()
                                    .child(
                                        svg()
                                            .path("phosphor/warning.svg")
                                            .size(px(14.0))
                                            .text_color(theme.warning.opacity(if theme.is_dark() {
                                                0.96
                                            } else {
                                                0.90
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .line_height(px(16.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.foreground.opacity(if theme.is_dark() {
                                                0.84
                                            } else {
                                                0.76
                                            }))
                                            .whitespace_nowrap()
                                            .child("Save changes before closing?"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        Button::new("settings-close-prompt-keep-editing")
                                            .ghost()
                                            .small()
                                            .compact()
                                            .h(px(26.0 * header_density))
                                            .px(px(8.0 * header_density))
                                            .rounded(px(7.0 * header_density))
                                            .child(
                                                div()
                                                    .text_size(px(12.0 * header_density))
                                                    .line_height(px(15.0 * header_density))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(
                                                        theme.muted_foreground.opacity(0.74),
                                                    )
                                                    .whitespace_nowrap()
                                                    .child("Keep Editing"),
                                            )
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                this.keep_editing_after_close_prompt(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("settings-close-prompt-save")
                                            .small()
                                            .compact()
                                            .custom(save_button_style)
                                            .h(px(26.0 * header_density))
                                            .px(px(9.0 * header_density))
                                            .rounded(px(7.0 * header_density))
                                            .gap(px(5.0 * header_density))
                                            .child(
                                                svg()
                                                    .path("phosphor/check.svg")
                                                    .size(px(12.0 * header_density))
                                                    .text_color(save_button_tint),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.0 * header_density))
                                                    .line_height(px(15.0 * header_density))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(save_button_tint)
                                                    .whitespace_nowrap()
                                                    .child("Save and Close"),
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.save_and_close_standalone(window, cx);
                                            })),
                                    ),
                            )
                    },
                ),
            )
            // Error banner
            .children(self.save_error.as_ref().map(|err| {
                let message = if self.save_error_kind == Some(SettingsSaveErrorKind::KeybindingConflict) {
                    err.to_string()
                } else {
                    format!("Save failed: {err}")
                };
                div()
                    .px_4()
                    .py_2()
                    .mx_4()
                    .mt_2()
                    .rounded_md()
                    .bg(theme.danger)
                    .text_color(theme.danger_foreground)
                    .text_xs()
                    .child(message)
            }))
            // Body
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar)
                    .child(content_scroll),
            );

        if self.standalone {
            return div()
                .id("settings-window")
                .size_full()
                .font_family(theme.font_family.clone())
                .bg(theme.background)
                .child(surface);
        }

        let backdrop = div()
            .id("settings-backdrop")
            .occlude()
            .absolute()
            .size_full()
            .bg(theme.background.opacity(0.6 * overlay_progress))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.save(window, cx);
                }),
            );

        let card = div()
            .id("settings-card-shell")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .opacity(overlay_progress)
            .child(
                div()
                    .pt(vertical_reveal_offset(overlay_progress, 18.0))
                    .opacity(overlay_progress)
                    .child(surface),
            );

        div()
            .id("settings-overlay")
            .absolute()
            .size_full()
            .font_family(theme.font_family.clone())
            .child(backdrop)
            .child(card)
    }
}

// ── Update status helpers ─────────────────────────────────────────

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn update_summary_and_detail(
    state: &crate::updater::CheckState,
    status: crate::updater::UpdaterStatus,
) -> (String, String) {
    use crate::updater::CheckState;
    match state {
        CheckState::Checking => (
            "Checking for updates…".to_string(),
            "Fetching the release feed.".to_string(),
        ),
        CheckState::UpdateAvailable { version, .. } => (
            format!("Update available: {version}"),
            "A newer build has been published.".to_string(),
        ),
        CheckState::UpToDate => (
            "Up to date".to_string(),
            format!(
                "Running {} — latest published build.",
                crate::app_display_version()
            ),
        ),
        CheckState::Error(e) => ("Update check failed".to_string(), e.clone()),
        CheckState::Idle => (status.summary().to_string(), status.detail().to_string()),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn update_download_url(state: &crate::updater::CheckState) -> Option<String> {
    match state {
        crate::updater::CheckState::UpdateAvailable { url, .. } => Some(url.clone()),
        _ => None,
    }
}

// ── Reusable building blocks ──────────────────────────────────────

fn section_content(title: &str, subtitle: &str, theme: &gpui_component::Theme) -> Div {
    div().flex().flex_col().gap(px(20.0)).child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(19.0))
                    .line_height(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .max_w(px(520.0))
                    .text_size(px(12.0))
                    .line_height(px(19.0))
                    .text_color(theme.muted_foreground.opacity(0.68))
                    .child(subtitle.to_string()),
            ),
    )
}

fn group_label(text: &str, theme: &gpui_component::Theme) -> Div {
    div()
        .text_size(px(10.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.muted_foreground.opacity(0.5))
        .px(px(2.0))
        .pb(px(2.0))
        .child(text.to_string())
}

fn card(theme: &gpui_component::Theme, opacity: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(12.0))
        .overflow_hidden()
        .bg(theme.background.opacity(opacity.clamp(0.35, 0.98)))
}

fn row_separator(_theme: &gpui_component::Theme) -> Div {
    div().h(px(6.0))
}

fn row_field(label: &str, input: &Entity<InputState>) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .px(px(16.0))
        .h(px(46.0))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .flex_shrink_0()
                .child(label.to_string()),
        )
        .child(div().flex_1().min_w(px(160.0)).child(Input::new(input)))
}

fn slider_row(
    label: &str,
    hint: &str,
    slider: &Entity<SliderState>,
    value: f32,
    theme: &gpui_component::Theme,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(18.0))
        .px(px(16.0))
        .py(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .flex_1()
                .min_w_0()
                .max_w(px(380.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(17.0))
                        .text_color(theme.muted_foreground.opacity(0.65))
                        .child(hint.to_string()),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .w(px(260.0))
                .flex_shrink_0()
                .child(
                    div().flex().justify_end().child(
                        div()
                            .min_w(px(58.0))
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(999.0))
                            .bg(theme.muted.opacity(0.10))
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_align(TextAlign::Center)
                            .text_color(theme.foreground)
                            .child(format!("{:.0}%", value * 100.0)),
                    ),
                )
                .child(div().w_full().child(Slider::new(slider).w_full())),
        )
}

fn searchable_select_row(
    label: &str,
    hint: &str,
    select: &Entity<SelectState<SearchableVec<String>>>,
    placeholder: &str,
    theme: &gpui_component::Theme,
) -> Div {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(16.0))
        .px(px(16.0))
        .py(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .flex_1()
                .max_w(px(340.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(17.0))
                        .text_color(theme.muted_foreground.opacity(0.65))
                        .child(hint.to_string()),
                ),
        )
        .child(
            div().w(px(236.0)).flex_shrink_0().child(
                Select::new(select)
                    .placeholder(placeholder.to_string())
                    .small(),
            ),
        )
}

fn select_row(
    label: &str,
    hint: &str,
    select: &Entity<SelectState<Vec<String>>>,
    theme: &gpui_component::Theme,
) -> Div {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(16.0))
        .py(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .flex_1()
                .max_w(px(320.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(17.0))
                        .text_color(theme.muted_foreground.opacity(0.65))
                        .child(hint.to_string()),
                ),
        )
        .child(
            div()
                .w(px(188.0))
                .flex_shrink_0()
                .child(Select::new(select).small()),
        )
}

fn toggle_row(label: &str, hint: &str, toggle: Switch, theme: &gpui_component::Theme) -> Div {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(16.0))
        .px(px(16.0))
        .py(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .flex_1()
                .max_w(px(360.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(17.0))
                        .text_color(theme.muted_foreground.opacity(0.65))
                        .child(hint.to_string()),
                ),
        )
        .child(div().pt(px(2.0)).child(toggle))
}

fn stacked_input_field(
    label: &str,
    hint: &str,
    input: &Entity<InputState>,
    theme: &gpui_component::Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .line_height(px(16.0))
                        .text_color(theme.muted_foreground.opacity(0.65))
                        .child(hint.to_string()),
                ),
        )
        .child(Input::new(input))
}

/// Convert a GPUI Keystroke to the binding format string (e.g. "cmd-shift-d").
fn keystroke_to_binding(ks: &gpui::Keystroke) -> String {
    let mut parts = Vec::new();
    if ks.modifiers.platform {
        parts.push("cmd");
    }
    if ks.modifiers.control {
        parts.push("ctrl");
    }
    if ks.modifiers.alt {
        parts.push("alt");
    }
    if ks.modifiers.shift {
        parts.push("shift");
    }
    parts.push(&ks.key);
    parts.join("-")
}

fn keybinding_conflict_message(kb: &con_core::config::KeybindingConfig) -> Option<String> {
    let conflicts = kb.shortcut_conflicts(&reserved_keybinding_shortcuts());
    let conflict = conflicts.first()?;
    Some(format!(
        "Shortcut conflict: {} is assigned to {}. Pick a different shortcut before saving.",
        conflict.binding,
        human_join(&conflict.actions)
    ))
}

fn sync_keybinding_conflict_error(
    save_error: &mut Option<String>,
    save_error_kind: &mut Option<SettingsSaveErrorKind>,
    kb: &con_core::config::KeybindingConfig,
) {
    match keybinding_conflict_message(kb) {
        Some(message) => {
            *save_error = Some(message);
            *save_error_kind = Some(SettingsSaveErrorKind::KeybindingConflict);
        }
        None if *save_error_kind == Some(SettingsSaveErrorKind::KeybindingConflict) => {
            *save_error = None;
            *save_error_kind = None;
        }
        None => {}
    }
}

fn reserved_keybinding_shortcuts() -> Vec<(&'static str, &'static str)> {
    crate::fixed_app_keybinding_shortcuts()
}

fn human_join(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let mut text = items[..items.len() - 1].join(", ");
            text.push_str(", and ");
            text.push_str(&items[items.len() - 1]);
            text
        }
    }
}

fn key_row(action: &str, shortcut: &str, theme: &gpui_component::Theme) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(16.0))
        .h(px(34.0))
        .hover(|s| s.bg(theme.muted.opacity(0.025)))
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground.opacity(0.86))
                .child(action.to_string()),
        )
        .child(crate::keycaps::keycaps_for_binding(shortcut, theme))
}

pub(crate) fn provider_label(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Anthropic => "Anthropic",
        ProviderKind::OpenAI => "OpenAI",
        ProviderKind::ChatGPT => "ChatGPT Subscription",
        ProviderKind::GitHubCopilot => "GitHub Copilot",
        ProviderKind::OpenAICompatible => "OpenAI Compatible",
        ProviderKind::MiniMax => "MiniMax",
        ProviderKind::MiniMaxAnthropic => "MiniMax / Anthropic",
        ProviderKind::Moonshot => "Moonshot",
        ProviderKind::MoonshotAnthropic => "Moonshot / Anthropic",
        ProviderKind::ZAI => "Z.AI",
        ProviderKind::ZAIAnthropic => "Z.AI / Anthropic",
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
    }
}

fn display_theme_name(name: &str) -> String {
    match name {
        "flexoki-dark" => "Flexoki Dark".into(),
        "flexoki-light" => "Flexoki Light".into(),
        "catppuccin-mocha" => "Catppuccin".into(),
        "tokyonight" => "Tokyo Night".into(),
        "rose-pine" => "Rose Pine".into(),
        "gruvbox-dark" => "Gruvbox Dark".into(),
        "solarized-dark" => "Solarized Dark".into(),
        "solarized-light" => "Solarized Light".into(),
        "one-half-dark" => "One Half Dark".into(),
        "kanagawa-wave" => "Kanagawa Wave".into(),
        "everforest-dark" => "Everforest Dark".into(),
        "everforest-light" => "Everforest Light".into(),
        "paper-light" => "Paper Light".into(),
        // User themes: convert kebab-case to Title Case
        other => other
            .split('-')
            .map(|word| {
                let mut c = word.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().to_string() + c.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}
