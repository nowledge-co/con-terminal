//! Durable references to native agent conversations. Neither terminal screen
//! replay nor OSC activity reports establish one of these references.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Explicit native contracts. Adding a display icon is not resume support.
#[derive(Clone, Copy, Debug)]
pub struct NativeAgentAdapter {
    pub agent: &'static str,
    pub label: &'static str,
    pub config_env: &'static str,
    pub default_directory: &'static str,
    pub executable: &'static str,
    pub resume_option: &'static str,
}

pub const NATIVE_AGENT_ADAPTERS: &[NativeAgentAdapter] = &[
    NativeAgentAdapter {
        agent: "claude",
        label: "Claude Code",
        config_env: "CLAUDE_CONFIG_DIR",
        default_directory: ".claude",
        executable: "claude",
        resume_option: "--resume",
    },
    NativeAgentAdapter {
        agent: "codex",
        label: "Codex",
        config_env: "CODEX_HOME",
        default_directory: ".codex",
        executable: "codex",
        resume_option: "resume",
    },
    NativeAgentAdapter {
        agent: "cursor",
        label: "Cursor Agent",
        config_env: "CURSOR_CONFIG_DIR",
        default_directory: ".cursor",
        executable: "cursor-agent",
        resume_option: "--resume",
    },
    NativeAgentAdapter {
        agent: "qwen",
        label: "Qwen Code",
        config_env: "QWEN_HOME",
        default_directory: ".qwen",
        executable: "qwen",
        resume_option: "--resume",
    },
    NativeAgentAdapter {
        agent: "copilot",
        label: "GitHub Copilot",
        config_env: "COPILOT_HOME",
        default_directory: ".copilot",
        executable: "copilot",
        resume_option: "--resume",
    },
];

pub fn native_agent_adapter(agent: &str) -> Option<&'static NativeAgentAdapter> {
    NATIVE_AGENT_ADAPTERS
        .iter()
        .find(|adapter| adapter.agent == agent)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NativeAgentSession {
    /// Kept as text so a newer adapter does not make older layout files unreadable.
    pub agent: String,
    pub session_id: String,
    pub cwd: PathBuf,
    /// The native agent's own store/config namespace, never a copied transcript.
    pub config_dir: PathBuf,
    #[serde(default)]
    pub transcript_path: Option<PathBuf>,
    /// Last confirmed terminal pixel size, only for hidden startup before layout.
    #[serde(default)]
    pub viewport_pixels: Option<[u32; 2]>,
    #[serde(default)]
    pub title: Option<String>,
}

impl NativeAgentSession {
    pub fn validate(&self) -> Result<(), String> {
        if native_agent_adapter(&self.agent).is_none() {
            return Err("This native agent is not supported for session resume".into());
        }
        if !uuid::Uuid::parse_str(&self.session_id)
            .is_ok_and(|id| id.to_string() == self.session_id)
        {
            return Err("The saved native session ID is invalid".into());
        }
        for path in [&self.cwd, &self.config_dir]
            .into_iter()
            .chain(self.transcript_path.iter())
        {
            let text = path.to_str().ok_or("Native session paths must be UTF-8")?;
            if !path.is_absolute() || text.len() > 4096 || text.chars().any(char::is_control) {
                return Err(
                    "Native session paths must be absolute and contain no control characters"
                        .into(),
                );
            }
        }
        if self
            .viewport_pixels
            .is_some_and(|size| size.into_iter().any(|value| !(1..=32768).contains(&value)))
        {
            return Err("Invalid saved native viewport size".into());
        }
        if self
            .title
            .as_ref()
            .is_some_and(|title| title.len() > 256 || title.chars().any(char::is_control))
        {
            return Err("Invalid native session title".into());
        }
        Ok(())
    }

    pub fn label(&self) -> &str {
        native_agent_adapter(&self.agent).map_or("Agent", |adapter| adapter.label)
    }

    pub fn config_env(&self) -> Result<&'static str, String> {
        self.validate()?;
        Ok(native_agent_adapter(&self.agent)
            .expect("validated adapter")
            .config_env)
    }

    /// A structural key, not concatenated shell text or a guessed recent session.
    pub fn identity(&self) -> (&str, &std::path::Path, &str) {
        (&self.agent, &self.config_dir, &self.session_id)
    }

    pub fn resume_argv(&self) -> Result<Vec<String>, String> {
        self.validate()?;
        Ok(if self.agent == "codex" {
            // Shared daemons do not preserve the originating terminal context
            // for SessionStart hooks. Keep this client tied to this surface.
            vec![
                "codex".into(),
                "--no-daemon".into(),
                "resume".into(),
                self.session_id.clone(),
            ]
        } else {
            let adapter = native_agent_adapter(&self.agent).expect("validated adapter");
            if self.agent == "copilot" {
                vec![
                    adapter.executable.into(),
                    format!("--resume={}", self.session_id),
                ]
            } else {
                vec![
                    adapter.executable.into(),
                    adapter.resume_option.into(),
                    self.session_id.clone(),
                ]
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> NativeAgentSession {
        let root = std::env::temp_dir();
        NativeAgentSession {
            agent: "codex".into(),
            session_id: uuid::Uuid::new_v4().to_string(),
            cwd: root.join("worker ' $(nope)"),
            config_dir: root.join("native-config"),
            title: None,
            transcript_path: None,
            viewport_pixels: None,
        }
    }
    #[test]
    fn resume_is_exact_and_preserves_native_store_without_permission_flags() {
        let mut reference = session();
        let args = reference.resume_argv().unwrap();
        assert_eq!(
            args,
            vec![
                "codex",
                "--no-daemon",
                "resume",
                reference.session_id.as_str()
            ]
        );
        assert_eq!(reference.config_env().unwrap(), "CODEX_HOME");
        reference.agent = "claude".into();
        assert_eq!(
            reference.resume_argv().unwrap(),
            vec!["claude", "--resume", reference.session_id.as_str()]
        );
        assert_eq!(reference.config_env().unwrap(), "CLAUDE_CONFIG_DIR");
    }
    #[test]
    fn adapters_select_exact_native_id_and_store() {
        for (agent, executable, env, expected) in [
            ("cursor", "cursor-agent", "CURSOR_CONFIG_DIR", "--resume"),
            ("qwen", "qwen", "QWEN_HOME", "--resume"),
            ("copilot", "copilot", "COPILOT_HOME", "--resume"),
        ] {
            let mut reference = session();
            reference.agent = agent.into();
            let argv = reference.resume_argv().unwrap();
            let expected = if agent == "copilot" {
                vec![
                    executable.to_string(),
                    format!("{expected}={}", reference.session_id),
                ]
            } else {
                vec![
                    executable.to_string(),
                    expected.to_string(),
                    reference.session_id.clone(),
                ]
            };
            assert_eq!(argv, expected);
            assert_eq!(reference.config_env().unwrap(), env);
        }
    }
    #[test]
    fn malformed_or_unknown_references_survive_loading_but_never_generate_commands() {
        let mut reference = session();
        reference.agent = "future-agent".into();
        let reference: NativeAgentSession =
            serde_json::from_value(serde_json::to_value(reference).unwrap()).unwrap();
        assert!(reference.resume_argv().is_err());
        for id in ["--last", "latest", "$(touch nope)", "bad\ncommand"] {
            let mut reference = session();
            reference.session_id = id.into();
            assert!(reference.resume_argv().is_err());
        }
        let mut reference = session();
        reference.cwd = PathBuf::from("relative");
        assert!(reference.resume_argv().is_err());
        let mut reference = session();
        reference.viewport_pixels = Some([0, 400]);
        assert!(reference.resume_argv().is_err());
        reference.viewport_pixels = Some([800, 600]);
        reference.transcript_path = Some(PathBuf::from("relative-history"));
        assert!(reference.resume_argv().is_err());
    }
}
