# Study: Rig — Rust AI Agent Framework

## Overview

[Rig](https://rig.rs/) is the most mature Rust-native AI agent framework. MIT licensed.
The workspace uses the published `rig-core` 0.40 release from crates.io. Con keeps
provider construction, conversation persistence, terminal context, and approval
policy in its own crates; Rig supplies provider clients and the agent run loop.

## Core Architecture

### Client + Provider

```rust
use rig::providers::anthropic;
use rig::client::CompletionClient;

// From API key
let client = anthropic::Client::new("sk-ant-...").unwrap();

// From environment variable
let client = anthropic::Client::from_env(); // reads ANTHROPIC_API_KEY

// Model constants
anthropic::completion::CLAUDE_4_SONNET   // "claude-sonnet-4-0"
anthropic::completion::CLAUDE_4_OPUS     // "claude-opus-4-0"
anthropic::completion::CLAUDE_3_5_SONNET // "claude-3-5-sonnet-latest"
```

### Agent Builder

```rust
let agent = client
    .agent(anthropic::completion::CLAUDE_4_SONNET)
    .preamble("You are a terminal assistant.")
    .tool(ShellExecTool)
    .tool(FileReadTool)
    .max_tokens(4096)
    .default_max_turns(12) // preserve Con's former max_turns = 10 ceiling
    .build();
```

### Chat (multi-turn)

```rust
use rig::completion::Chat;

let response: String = agent
    .chat("explain this error", chat_history)
    .await?;
```

### Prompt (single turn)

```rust
use rig::completion::Prompt;

let response: String = agent
    .prompt("list files in current dir")
    .await?;
```

## Tool Definition (Rig API)

```rust
use rig::tool::Tool;
pub struct ShellExecTool;

impl Tool for ShellExecTool {
    const NAME: &'static str = "shell_exec";
    type Error = ToolError;        // must impl std::error::Error + Send + Sync
    type Args = ShellExecArgs;     // must impl Deserialize
    type Output = ShellExecOutput; // must impl Serialize

    fn description(&self) -> String {
        "Execute a shell command".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ /* JSON schema */ })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Execute the tool
    }
}
```

## AgentHook Trait (lifecycle callbacks)

```rust
use rig::agent::{AgentHook, Flow, HookContext, StepEvent};

impl<M: CompletionModel> AgentHook<M> for MyHook {
    async fn on_event(&self, _ctx: &HookContext, event: StepEvent<'_, M>) -> Flow {
        match event {
            StepEvent::TextDelta { delta, .. } => stream_to_ui(delta),
            StepEvent::ToolCall { tool_name, .. } => request_approval(tool_name),
            StepEvent::ToolResult { result, .. } => record_result(result),
            _ => {}
        }
        Flow::cont()
    }
}
```

## Message Types

```rust
use rig::message::{Message, UserContent, AssistantContent, Text};
use rig::OneOrMany;

// User message
Message::User {
    content: OneOrMany::one(UserContent::Text(Text::new("hello")))
}

// Assistant message
Message::Assistant {
    id: None,
    content: OneOrMany::one(AssistantContent::Text(Text::new("hi")))
}
```

## Streaming

```rust
use rig::agent::MultiTurnStreamItem;
use rig::streaming::StreamedAssistantContent;

// Streaming responses yield these variants:
MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(...))
MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(...))
MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCall { ... })
MultiTurnStreamItem::StreamUserItem(...)  // completed tool result
MultiTurnStreamItem::FinalResponse(...)   // unified PromptResponse
```

## Integration with con

1. `con-agent/provider.rs` creates `anthropic::Client` from config
2. Builds an `Agent` with Con's terminal, tmux, file, and workspace tools
3. Uses `stream_prompt()` for live multi-turn conversation
4. `con-agent/conversation.rs` converts our Message types to Rig's `Vec<Message>`
5. `con-core/harness.rs` runs agent work on a shared tokio runtime

## Key Differences from Rig 0.36

- `Tool` exposes synchronous `description()` and `parameters()` metadata methods.
- `AgentHook::on_event()` replaces the model-generic `PromptHook` method set.
- `PromptResponse` unifies streaming and non-streaming final output.
- `max_turns` is a total model-call budget, including the initial request.
- Per-delta hook interest is explicit, avoiding dispatch work for unused stream events.

## Workspace Integration Notes

Rig is an ordinary crates.io dependency. `3pp/` remains read-only reference material and is
never part of dependency resolution.
