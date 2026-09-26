use std::{ffi::OsString, path::Path};

use anyhow::{Context, Result, bail, ensure};
use uuid::Uuid;

use super::{AgentKind, TargetCapabilities, protocol};

/// Obtain an ID before native launch where the product exposes a safe way to do
/// so. None means a native fresh start with an as-yet-unobservable session ID.
/// The coordinator must persist this result before launching, and never replay.
pub async fn create_target(target: &TargetCapabilities, cwd: &Path) -> Result<Option<String>> {
    match target.agent {
        AgentKind::Unknown => bail!("Unsupported target Agent"),
        AgentKind::Cursor => {
            let bytes = protocol::capture(
                &target.executable,
                &["create-chat".into()],
                Some(cwd),
                4096,
                Some(AgentKind::Cursor),
            )
            .await?;
            let id = String::from_utf8(bytes).context("Cursor returned a non-text chat ID")?;
            let id = id.trim();
            validate_uuid(id)?;
            Ok(Some(id.to_owned()))
        }
        AgentKind::Codex | AgentKind::Kimi => Ok(None),
    }
}

/// Build native interactive argv with no model override. The caller must
/// also set current_dir(cwd). This is exactly `target_args_with_model` with
/// `None`, so the historical argv stays byte-identical for existing callers.
pub fn target_args(
    target: &TargetCapabilities,
    cwd: &Path,
    session_id: Option<&str>,
    prompt: Option<&str>,
) -> Result<Vec<OsString>> {
    target_args_with_model(target, cwd, session_id, prompt, None)
}

/// Build native interactive argv. The caller must also set current_dir(cwd).
/// No argument grants permissions, forks worktrees, or resumes a recent
/// session. A model override is added only when the user explicitly selected
/// a model for this launch (see the handoff contract's model section): it
/// travels as its own `--model <value>` argv pair placed before any prompt
/// argument, never inside a shell command, and never rewrites the user's
/// global configuration. When `model` is `None` the argv is identical to the
/// historical no-model shape. Login, trust, and permission prompts
/// stay exactly as the target product presents them.
/// Kimi receives its first message through Con PTY injection after TUI readiness.
pub fn target_args_with_model(
    target: &TargetCapabilities,
    cwd: &Path,
    session_id: Option<&str>,
    prompt: Option<&str>,
    model: Option<&str>,
) -> Result<Vec<OsString>> {
    use AgentKind::*;
    ensure!(
        cwd.is_absolute(),
        "Native handoff requires an absolute working directory"
    );
    let mut args = Vec::new();
    match target.agent {
        Unknown => bail!("Unsupported target Agent"),
        Cursor => {
            let id = session_id.context("This target requires a newly allocated session ID")?;
            validate_uuid(id)?;
            args.extend([
                "--workspace".into(),
                cwd.as_os_str().to_owned(),
                "--resume".into(),
                id.into(),
            ]);
        }
        Codex => {
            ensure!(
                session_id.is_none(),
                "{} must start a fresh native session",
                target.agent.label()
            );
            args.extend(["--cd".into(), cwd.as_os_str().to_owned()]);
        }
        // No undocumented --cwd, session-id or prompt flags.
        Kimi => {
            ensure!(
                session_id.is_none(),
                "{} must start a fresh native session",
                target.agent.label()
            );
        }
    }
    if let Some(model) = model {
        // Defense in depth on top of con_core's validate_target_model: the
        // value rides as its own argv element, but an empty or option-shaped
        // value would still corrupt the target CLI's parsing. Placed before
        // any prompt so `--`-terminated or trailing-positional prompt shapes
        // keep working.
        ensure!(
            !model.is_empty() && !model.starts_with('-') && !model.chars().any(char::is_control),
            "Invalid target model"
        );
        args.extend(["--model".into(), model.into()]);
    }
    if let Some(prompt) = prompt {
        ensure!(
            !prompt.is_empty() && !prompt.contains('\0'),
            "Invalid handoff instruction"
        );
        match target.agent {
            Unknown => bail!("Unsupported target Agent"),
            Kimi => bail!(
                "{} requires PTY prompt delivery in its native TUI",
                target.agent.label()
            ),
            Cursor => {
                // Preserve the exact argv shape used by the verified Cursor path.
                ensure!(
                    !prompt.starts_with('-'),
                    "Cursor instruction cannot be an option"
                );
                args.push(prompt.into());
            }
            Codex => args.extend(["--".into(), prompt.into()]),
        }
    }
    Ok(args)
}

fn validate_uuid(id: &str) -> Result<()> {
    ensure!(
        Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id),
        "Target returned an invalid session UUID; inspect the native agent before retrying"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_cwd() -> PathBuf {
        std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join("中文 ' $(echo nope)")
    }

    fn target(agent: AgentKind) -> TargetCapabilities {
        TargetCapabilities {
            agent,
            executable: agent.executable_name().into(),
            version: "fixture".into(),
            automatic_delivery: false,
        }
    }

    fn session_id_for(agent: AgentKind, id: &str) -> Option<&str> {
        (agent == AgentKind::Cursor).then_some(id)
    }

    #[test]
    fn codex_automatic_prompt_is_one_literal_argument() {
        let prompt = "Read .con/handoffs/id/context.md; $(echo not-a-shell)";
        let cwd = test_cwd();
        let args = target_args_with_model(
            &target(AgentKind::Codex),
            &cwd,
            None,
            Some(prompt),
            Some("model/id"),
        )
        .unwrap();
        assert_eq!(
            args,
            [
                OsString::from("--cd"),
                cwd.into_os_string(),
                OsString::from("--model"),
                OsString::from("model/id"),
                OsString::from("--"),
                OsString::from(prompt),
            ]
        );
    }

    #[test]
    fn native_argv_preserves_paths_and_permission_policy() {
        let cwd = test_cwd();
        let id = "020755ba-3c6b-491b-8cdf-4e3c1ab0a6e8";
        for agent in AgentKind::ALL {
            let session_id = session_id_for(agent, id);
            let args = target_args(&target(agent), &cwd, session_id, None).unwrap();
            // An unset model must not change the historical argv one bit.
            assert_eq!(
                args,
                target_args_with_model(&target(agent), &cwd, session_id, None, None).unwrap()
            );
            assert!(!args.iter().any(|arg| {
                [
                    "--force",
                    "--trust",
                    "--yolo",
                    "--continue",
                    "--worktree",
                    "--model",
                    "--provider",
                ]
                .iter()
                .any(|flag| arg.as_os_str() == std::ffi::OsStr::new(flag))
            }));
            if matches!(agent, AgentKind::Codex | AgentKind::Cursor) {
                assert!(args.iter().any(|arg| arg == cwd.as_os_str()));
            }
        }
    }

    #[test]
    fn explicit_model_is_a_standalone_argv_pair() {
        let cwd = test_cwd();
        let id = "020755ba-3c6b-491b-8cdf-4e3c1ab0a6e8";
        let model = "provider/some-model 7";
        for agent in AgentKind::ALL {
            let session_id = session_id_for(agent, id);
            let with_model =
                target_args_with_model(&target(agent), &cwd, session_id, None, Some(model));
            let with_model = with_model.unwrap();
            let without_model = target_args(&target(agent), &cwd, session_id, None).unwrap();
            // The override is exactly one inserted `--model <value>` pair;
            // every other element stays in place.
            let stripped: Vec<_> = with_model
                .iter()
                .filter(|arg| arg.as_os_str() != "--model" && arg.as_os_str() != model)
                .cloned()
                .collect();
            assert_eq!(stripped, without_model);
            let flag = with_model
                .iter()
                .position(|arg| arg.as_os_str() == "--model")
                .unwrap();
            assert_eq!(with_model[flag + 1].as_os_str(), model);
            assert_eq!(with_model.len(), without_model.len() + 2);
        }
    }

    #[test]
    fn malformed_or_option_like_models_are_rejected() {
        let cwd = test_cwd();
        for model in ["", "--continue", "-m", "bad\u{7}model"] {
            assert!(
                target_args_with_model(
                    &target(AgentKind::Cursor),
                    &cwd,
                    Some("020755ba-3c6b-491b-8cdf-4e3c1ab0a6e8"),
                    None,
                    Some(model)
                )
                .is_err()
            );
        }
    }

    #[test]
    fn model_override_precedes_prompt_delivery() {
        let id = Some("020755ba-3c6b-491b-8cdf-4e3c1ab0a6e8");
        let cwd = test_cwd();
        // Cursor's prompt is a trailing positional; the model pair must not
        // land after it.
        let args = target_args_with_model(
            &target(AgentKind::Cursor),
            &cwd,
            id,
            Some("Read context"),
            Some("composer-2.5"),
        )
        .unwrap();
        let flag = args.iter().position(|arg| arg == "--model").unwrap();
        let prompt = args.iter().position(|arg| arg == "Read context").unwrap();
        assert!(flag + 2 == prompt);
    }

    #[test]
    fn noninteractive_flags_cannot_replace_native_prompt_delivery() {
        let cwd = test_cwd();
        {
            let agent = AgentKind::Kimi;
            assert!(target_args(&target(agent), &cwd, None, Some("Read context")).is_err());
        }
    }

    #[test]
    fn stale_or_malformed_target_ids_are_rejected() {
        let cwd = test_cwd();
        assert!(target_args(&target(AgentKind::Cursor), &cwd, Some("--continue"), None).is_err());
        assert!(target_args(&target(AgentKind::Codex), &cwd, Some("old-session"), None).is_err());
    }
}

/// Paste literally into Kimi's interactive editor, then submit the turn.
pub fn kimi_delivery_payload(instruction: &str) -> Vec<u8> {
    format!("\x1b[200~{instruction}\x1b[201~\n").into_bytes()
}

#[cfg(test)]
mod kimi_payload_tests {
    #[test]
    fn bracketed_paste_has_one_submit_after_literal_utf8() {
        assert_eq!(
            super::kimi_delivery_payload("Read 中文\ncontext"),
            b"\x1b[200~Read \xe4\xb8\xad\xe6\x96\x87\ncontext\x1b[201~\n"
        );
    }
}
