//! Explicit, reversible native hook installation. Never called by app startup.
use super::*;

const MARKER: &str = "native-session hook --agent";

fn is_ours(value: &Value) -> bool {
    value
        .get("command")
        .or_else(|| value.get("bash"))
        .and_then(Value::as_str)
        .is_some_and(|command| {
            ["claude", "codex", "cursor", "qwen", "copilot"]
                .iter()
                .any(|agent| {
                    command
                        .strip_suffix(&format!(" {MARKER} {agent}"))
                        .is_some_and(|executable| {
                            executable.starts_with('\'') && executable.ends_with("/con-cli'")
                        })
                })
        })
}

fn edit_hooks(mut root: Value, agent: &str, command: &str, install: bool) -> Result<Value> {
    if !install && root.get("hooks").is_none() {
        return Ok(root);
    }
    let object = root
        .as_object_mut()
        .context("Native settings must be a JSON object")?;
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("Native hooks must be an object")?;
    let event = if agent == "cursor" {
        "sessionStart"
    } else {
        "SessionStart"
    };
    let groups = hooks
        .entry(event)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("SessionStart hooks must be an array")?;
    if matches!(agent, "cursor" | "copilot") {
        ensure!(
            groups.iter().all(Value::is_object),
            "Native hook entries must be objects"
        );
        groups.retain(|entry| !is_ours(entry));
        if install {
            groups.push(if agent == "copilot" {
                json!({"type":"command", "bash":command, "timeoutSec":5})
            } else {
                json!({"command":command})
            });
        }
    } else {
        // Validate first, then remove only groups made empty by our own removal.
        for group in groups.iter() {
            ensure!(
                group.get("hooks").and_then(Value::as_array).is_some(),
                "SessionStart hook groups must contain a hooks array"
            );
        }
        groups.retain_mut(|group| {
            let nested = group
                .get_mut("hooks")
                .and_then(Value::as_array_mut)
                .expect("hook groups were validated");
            let had_ours = nested.iter().any(is_ours);
            nested.retain(|hook| !is_ours(hook));
            !had_ours || !nested.is_empty()
        });
        if install {
            let mut group = json!({"hooks":[{"type":"command", "command":command, "timeout":5}]});
            if agent == "qwen" {
                group["matcher"] = json!("*");
            }
            groups.push(group);
        }
    }
    if groups.is_empty() {
        hooks.remove(event);
    }
    Ok(root)
}

pub(super) fn atomic_replace(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("Missing native configuration parent")?;
    std::fs::create_dir_all(parent)?;
    // Keep an immutable, private copy before changing an existing file.
    if path.exists() {
        let backup = parent.join(format!(
            "{}.con-backup-{}",
            path.file_name().unwrap().to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        std::fs::copy(path, &backup)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(backup, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    let temporary = parent.join(format!(".con-native-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

pub(super) fn contains_own_hook(root: &Value, agent: &str) -> bool {
    root.get("hooks")
        .and_then(|hooks| hooks.get("SessionStart"))
        .and_then(Value::as_array)
        .is_some_and(|groups| {
            groups.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| {
                        hooks.iter().any(|hook| {
                            is_ours(hook)
                                && hook.get("command").and_then(Value::as_str).is_some_and(
                                    |command| command.ends_with(&format!(" {MARKER} {agent}")),
                                )
                        })
                    })
            })
        })
}

pub fn update(
    agent: &str,
    directory: Option<PathBuf>,
    shell_rc: Option<PathBuf>,
    install: bool,
    json_output: bool,
) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "Native session hooks currently support macOS only"
    );
    let directory = match directory {
        Some(directory) => directory,
        None => super::config_dir(agent)?,
    }
    .canonicalize()?;
    ensure!(
        native_agent_adapter(agent).is_some(),
        "Unsupported native agent"
    );
    let path = directory.join(if matches!(agent, "codex" | "cursor") {
        "hooks.json"
    } else {
        "settings.json"
    });
    ensure!(
        !path.is_symlink(),
        "Refusing to replace a symlinked native settings file"
    );
    let path_existed = path.exists();
    let old = if path_existed {
        std::fs::read(&path)?
    } else {
        b"{}".to_vec()
    };
    let mut root: Value =
        serde_json::from_slice(&old).context("Native settings are malformed; left unchanged")?;
    let original_root = root.clone();
    if agent == "cursor" {
        ensure!(
            root.get("version")
                .is_none_or(|version| version.as_u64() == Some(1)),
            "Unsupported Cursor hooks version; left unchanged"
        );
        if install && root.get("version").is_none() {
            root.as_object_mut()
                .context("Native settings must be a JSON object")?
                .insert("version".into(), json!(1));
        }
    }
    let executable = std::env::current_exe()?.canonicalize()?;
    let executable = executable
        .to_str()
        .context("Companion CLI path is not UTF-8")?;
    ensure!(
        !executable.chars().any(char::is_control),
        "Invalid companion CLI path"
    );
    let command = format!("'{}' {MARKER} {agent}", executable.replace('\'', "'\\''"));
    let updated = edit_hooks(root.clone(), agent, &command, install)?;
    // Parse every input before mutating either file. Preserve TOML comments,
    // profile tables, provider configuration, and unrelated feature flags.
    let config_path = directory.join("config.toml");
    let config_update = if agent == "codex" && install {
        ensure!(
            !config_path.is_symlink(),
            "Refusing to replace a symlinked native config file"
        );
        let text = if config_path.exists() {
            std::fs::read_to_string(&config_path)?
        } else {
            String::new()
        };
        let mut document = text
            .parse::<toml_edit::DocumentMut>()
            .context("Native config is malformed; left unchanged")?;
        ensure!(
            document
                .get("features")
                .is_none_or(|features| features.as_table_like().is_some()),
            "Native features must be a TOML table; left unchanged"
        );
        document["features"]["hooks"] = toml_edit::value(true);
        let new = document.to_string();
        (new != text).then_some(new)
    } else {
        None
    };
    // Preflight shell changes before touching any native configuration.
    let shell = if agent == "codex" {
        Some(launch::prepare(install, shell_rc, &directory)?)
    } else {
        None
    };
    // Enabling an unused native feature is harmless if the process stops
    // between writes. Installing a hook while its native feature is disabled
    // would misleadingly report success, so write the feature first.
    let old_config = config_update
        .as_ref()
        .map(|_| std::fs::read(&config_path))
        .transpose();
    let old_config = match old_config {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if let Some(config) = &config_update {
        atomic_replace(&config_path, config.as_bytes())?;
    }
    if updated != original_root
        && let Err(error) = atomic_replace(&path, &serde_json::to_vec_pretty(&updated)?)
    {
        if config_update.is_some() {
            let rollback = if let Some(old) = old_config {
                atomic_replace(&config_path, &old)
            } else {
                std::fs::remove_file(&config_path).map_err(Into::into)
            };
            rollback.context("Hook installation failed and native feature rollback also failed; inspect the private backups")?;
        }
        return Err(
            error.context("Hook installation failed; native feature change was rolled back")
        );
    }
    if let Some(shell) = &shell
        && let Err(error) = shell.apply()
    {
        if updated != original_root {
            if path_existed {
                atomic_replace(&path, &old)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        if config_update.is_some() {
            if let Some(old) = old_config {
                atomic_replace(&config_path, &old)?;
            } else {
                std::fs::remove_file(&config_path)?;
            }
        }
        return Err(error.context("Shell integration failed; native settings were rolled back"));
    }
    let shell_path = shell.map(|shell| shell.rc);
    // Uninstall does not disable native hooks: other integrations may use them.
    let result = json!({"agent":agent,"installed":install,"hooks_path":path,"shell_rc":shell_path,
        "note":"Only sessions started in a current con terminal can link; restart the native agent after installing."});
    if json_output {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        if agent == "codex" && install {
            println!(
                "Codex integration installed. Open a new con terminal and run codex as usual."
            );
            if let Some(path) = &shell_path {
                println!("Shell integration: {}", path.display());
            }
            return Ok(());
        }
        println!(
            "{} native session hooks {}",
            agent,
            if install { "installed" } else { "removed" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn install_is_idempotent_and_uninstall_keeps_other_hooks_and_settings() {
        let original = json!({"model":"keep","hooks":{"SessionStart":[{"matcher":"resume", "hooks":[{"type":"command","command":"other-tool"}]},{"matcher":"startup","hooks":[]}],"Stop":[{"hooks":[]} ]}});
        let installed = edit_hooks(
            original.clone(),
            "codex",
            "'/path/con-cli' native-session hook --agent codex",
            true,
        )
        .unwrap();
        assert_eq!(
            edit_hooks(
                installed.clone(),
                "codex",
                "'/path/con-cli' native-session hook --agent codex",
                true
            )
            .unwrap(),
            installed
        );
        let removed = edit_hooks(installed, "codex", "", false).unwrap();
        assert_eq!(removed, original);
    }
    #[test]
    fn native_hook_schemas_remain_distinct_and_removable() {
        for (agent, event, field) in [
            ("cursor", "sessionStart", "command"),
            ("copilot", "SessionStart", "bash"),
        ] {
            let original =
                json!({"version":1,"model":"keep", "hooks":{event:[{field:"another hook"}]}});
            let command = format!("'/path/con-cli' native-session hook --agent {agent}");
            let installed = edit_hooks(original.clone(), agent, &command, true).unwrap();
            assert_eq!(installed["hooks"][event][1][field], command);
            assert_eq!(
                edit_hooks(installed.clone(), agent, &command, true).unwrap(),
                installed
            );
            assert_eq!(edit_hooks(installed, agent, "", false).unwrap(), original);
        }
        let installed = edit_hooks(
            json!({}),
            "qwen",
            "'/path/con-cli' native-session hook --agent qwen",
            true,
        )
        .unwrap();
        assert_eq!(installed["hooks"]["SessionStart"][0]["matcher"], "*");
        assert!(installed["hooks"]["SessionStart"][0]["hooks"].is_array());
    }
    #[test]
    fn malformed_hook_groups_are_rejected_without_partial_edits() {
        assert!(
            edit_hooks(
                json!({"hooks":{"SessionStart":"bad"}}),
                "codex",
                "command",
                true
            )
            .is_err()
        );
        assert!(
            edit_hooks(
                json!({"hooks":{"SessionStart":[{"hooks":"bad"}]}}),
                "codex",
                "command",
                true
            )
            .is_err()
        );
    }
}
