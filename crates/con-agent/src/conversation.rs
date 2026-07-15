use chrono::{DateTime, Utc};
use rig::OneOrMany;
use rig::message::{AssistantContent, Message as RigMessage, Text, UserContent};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Maximum number of messages to keep in a conversation before truncating
const MAX_HISTORY: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

/// A single step in the agent's reasoning/execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStep {
    Thinking(String),
    ToolCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        tool: String,
        input: serde_json::Value,
    },
    ToolResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        tool: String,
        output: String,
        success: bool,
    },
    Text(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub steps: Vec<AgentStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    pub timestamp: DateTime<Utc>,
    /// Model name that generated this response (assistant messages only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Response duration in milliseconds (assistant messages only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: content.into(),
            steps: Vec::new(),
            thinking: None,
            timestamp: Utc::now(),
            model: None,
            duration_ms: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::Assistant,
            content: content.into(),
            steps: Vec::new(),
            thinking: None,
            timestamp: Utc::now(),
            model: None,
            duration_ms: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::System,
            content: content.into(),
            steps: Vec::new(),
            thinking: None,
            timestamp: Utc::now(),
            model: None,
            duration_ms: None,
        }
    }

    /// Builder: attach model name to this message.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Builder: attach duration to this message.
    pub fn with_duration_ms(mut self, ms: u64) -> Self {
        self.duration_ms = Some(ms);
        self
    }

    /// Builder: attach persisted thinking text to this message.
    pub fn with_thinking(mut self, thinking: impl Into<String>) -> Self {
        self.thinking = Some(thinking.into());
        self
    }
}

/// A conversation with bounded history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub messages: Vec<Message>,
    pub created_at: DateTime<Utc>,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            messages: Vec::new(),
            created_at: Utc::now(),
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        // Keep history bounded — drop oldest messages beyond the limit,
        // but always preserve the first system message if present
        if self.messages.len() > MAX_HISTORY {
            let drain_count = self.messages.len() - MAX_HISTORY;
            let start = if self
                .messages
                .first()
                .is_some_and(|m| m.role == MessageRole::System)
            {
                1 // preserve system message at index 0
            } else {
                0
            };
            self.messages.drain(start..start + drain_count);
        }
    }

    /// Truncate conversation to keep only the first `n` messages.
    /// Preserves the system message at index 0 if present.
    pub fn truncate_to(&mut self, n: usize) {
        if n < self.messages.len() {
            self.messages.truncate(n);
        }
    }

    pub fn last_user_message(&self) -> Option<&Message> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::User)
    }

    /// Convert our conversation history to Rig's Vec<Message> for the Chat trait.
    /// Excludes the last user message (which becomes the prompt) and system messages
    /// (which go into the preamble).
    ///
    /// Limits history to the most recent messages to avoid context pollution —
    /// stale assistant responses can override tool definitions and system prompts.
    const MAX_HISTORY_MESSAGES: usize = 20;

    pub fn to_rig_history(&self) -> Vec<RigMessage> {
        let mut history = Vec::new();
        // Skip the last user message — it will be sent as the prompt
        let msgs = if self
            .messages
            .last()
            .is_some_and(|m| m.role == MessageRole::User)
        {
            &self.messages[..self.messages.len() - 1]
        } else {
            &self.messages
        };

        // Take only the most recent messages to keep history manageable
        let start = msgs.len().saturating_sub(Self::MAX_HISTORY_MESSAGES);
        let msgs = &msgs[start..];

        for msg in msgs {
            match msg.role {
                MessageRole::User => {
                    history.push(RigMessage::User {
                        content: OneOrMany::one(UserContent::Text(Text::new(msg.content.clone()))),
                    });
                }
                MessageRole::Assistant => {
                    history.push(RigMessage::Assistant {
                        id: None,
                        content: OneOrMany::one(AssistantContent::Text(Text::new(
                            msg.content.clone(),
                        ))),
                    });
                }
                // System and Tool messages are handled separately
                MessageRole::System | MessageRole::Tool => {}
            }
        }
        history
    }
}

/// Summary of a saved conversation for listing in the UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub message_count: usize,
    /// Model from the first assistant message (if any)
    #[serde(default)]
    pub model: Option<String>,
}

impl Conversation {
    /// Save this conversation to disk
    pub fn save(&self) -> anyhow::Result<()> {
        if self.messages.is_empty() {
            return Ok(());
        }
        let path = Self::conversation_path(&self.id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Delete a saved conversation by ID.
    pub fn delete(id: &str) -> anyhow::Result<()> {
        let path = Self::conversation_path(id);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Load a conversation by ID
    pub fn load(id: &str) -> anyhow::Result<Self> {
        let path = Self::conversation_path(id);
        let content = std::fs::read_to_string(&path)?;
        let conv: Conversation = serde_json::from_str(&content)?;
        Ok(conv)
    }

    /// List all saved conversations as summaries, newest first
    pub fn list_all() -> Vec<ConversationSummary> {
        let dir = Self::conversations_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };

        let mut summaries: Vec<ConversationSummary> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|e| {
                let content = std::fs::read_to_string(e.path()).ok()?;
                let conv: Conversation = serde_json::from_str(&content).ok()?;
                let title = conv
                    .messages
                    .iter()
                    .find(|m| m.role == MessageRole::User)
                    .map(|m| truncate_summary_title(&m.content, 60))
                    .unwrap_or_else(|| "Empty conversation".to_string());
                let model = conv
                    .messages
                    .iter()
                    .find(|m| m.role == MessageRole::Assistant)
                    .and_then(|m| m.model.clone());
                Some(ConversationSummary {
                    id: conv.id,
                    title,
                    created_at: conv.created_at,
                    message_count: conv.messages.len(),
                    model,
                })
            })
            .collect();

        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        summaries
    }

    fn conversations_dir() -> PathBuf {
        if let Some(path) = std::env::var_os("CON_CONVERSATIONS_DIR") {
            return PathBuf::from(path);
        }
        con_paths::app_data_dir().join("conversations")
    }

    fn conversation_path(id: &str) -> PathBuf {
        Self::conversations_dir().join(format!("{}.json", id))
    }
}

fn truncate_summary_title(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }

    let truncated: String = content.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{truncated}...")
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_history_titles_on_char_boundaries() {
        let title = "请 ssh 到 sandy 和 cinnamon 检查一下内存、硬盘情况最后给我出一个表格";
        let truncated = truncate_summary_title(title, 20);

        assert!(truncated.ends_with("..."));
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn message_serialization_round_trips_thinking_and_steps() {
        let mut message = Message::assistant("done")
            .with_model("gpt-5.4")
            .with_duration_ms(1234)
            .with_thinking("checking shell context");
        message.steps = vec![
            AgentStep::ToolCall {
                call_id: Some("call-1".to_string()),
                tool: "list_panes".to_string(),
                input: serde_json::json!({"pane_id": 7}),
            },
            AgentStep::ToolResult {
                call_id: Some("call-1".to_string()),
                tool: "list_panes".to_string(),
                output: "{\"ok\":true}".to_string(),
                success: true,
            },
        ];

        let json = serde_json::to_string(&message).expect("serialize message");
        let round_trip: Message = serde_json::from_str(&json).expect("deserialize message");

        assert_eq!(round_trip.content, "done");
        assert_eq!(round_trip.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(round_trip.duration_ms, Some(1234));
        assert_eq!(
            round_trip.thinking.as_deref(),
            Some("checking shell context")
        );
        assert_eq!(round_trip.steps.len(), 2);
        match &round_trip.steps[0] {
            AgentStep::ToolCall {
                call_id,
                tool,
                input,
            } => {
                assert_eq!(call_id.as_deref(), Some("call-1"));
                assert_eq!(tool, "list_panes");
                assert_eq!(input["pane_id"], 7);
            }
            other => panic!("unexpected first step: {other:?}"),
        }
        match &round_trip.steps[1] {
            AgentStep::ToolResult {
                call_id,
                tool,
                output,
                success,
            } => {
                assert_eq!(call_id.as_deref(), Some("call-1"));
                assert_eq!(tool, "list_panes");
                assert_eq!(output, "{\"ok\":true}");
                assert!(*success);
            }
            other => panic!("unexpected second step: {other:?}"),
        }
    }
}
