//! Durable references to native agent conversations. Neither terminal screen
//! replay nor OSC activity reports establish one of these references.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
        if !matches!(self.agent.as_str(), "claude" | "codex") {
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
        match self.agent.as_str() {
            "claude" => "Claude Code",
            "codex" => "Codex",
            _ => "Agent",
        }
    }

    pub fn config_env(&self) -> Result<&'static str, String> {
        self.validate()?;
        Ok(if self.agent == "codex" {
            "CODEX_HOME"
        } else {
            "CLAUDE_CONFIG_DIR"
        })
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
            vec!["claude".into(), "--resume".into(), self.session_id.clone()]
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
