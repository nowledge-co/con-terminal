//! Shared provider brand marks for settings and the agent panel.
use con_agent::ProviderKind;

pub(crate) fn provider_icon_path(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Anthropic => "providers/anthropic.svg",
        ProviderKind::OpenAI | ProviderKind::ChatGPT => "providers/openai.svg",
        ProviderKind::CodexLocal => "providers/codex.svg",
        ProviderKind::GitHubCopilot => "providers/githubcopilot.svg",
        ProviderKind::OpenAICompatible => "phosphor/plugs-connected.svg",
        ProviderKind::Magpie => "providers/magpie.svg",
        ProviderKind::MiniMax | ProviderKind::MiniMaxAnthropic => "providers/minimax.svg",
        ProviderKind::Moonshot | ProviderKind::MoonshotAnthropic => "providers/moonshot.svg",
        ProviderKind::ZAI | ProviderKind::ZAIAnthropic => "providers/zai.svg",
        ProviderKind::DeepSeek => "providers/deepseek.svg",
        ProviderKind::Groq => "providers/groq.svg",
        ProviderKind::Cohere => "providers/cohere.svg",
        ProviderKind::Gemini => "providers/gemini.svg",
        ProviderKind::Ollama => "providers/ollama.svg",
        ProviderKind::OpenRouter => "providers/openrouter.svg",
        ProviderKind::Perplexity => "providers/perplexity.svg",
        ProviderKind::Mistral => "providers/mistral.svg",
        ProviderKind::Together => "providers/together.svg",
        ProviderKind::XAI => "providers/xai.svg",
    }
}
