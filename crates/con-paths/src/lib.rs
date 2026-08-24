//! Shared per-user app paths.
//!
//! Windows cannot safely use a bare `con` path segment because `CON` is
//! a reserved DOS device name. Keep this policy centralized so config,
//! session, auth, themes, and future storage paths do not drift.

use std::path::PathBuf;

#[cfg(target_os = "windows")]
pub const APP_DIR_NAME: &str = "con-terminal";
#[cfg(not(target_os = "windows"))]
pub const APP_DIR_NAME: &str = "con";

pub fn app_config_dir() -> PathBuf {
    dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR_NAME)
}

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR_NAME)
}

pub fn app_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR_NAME)
}

pub fn config_file() -> PathBuf {
    app_config_dir().join("config.toml")
}

pub fn default_global_skills_path() -> String {
    format!("~/.config/{APP_DIR_NAME}/skills")
}

pub fn user_themes_dir() -> PathBuf {
    app_config_dir().join("themes")
}

pub fn runtime_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
            let path = PathBuf::from(dir);
            if path.is_dir() {
                return path;
            }
        }
        std::env::temp_dir()
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir()
    }
}

/// Returns true if running inside a Flatpak container.
pub fn is_flatpak() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/.flatpak-info").exists()
            || std::env::var("FLATPAK_ID").is_ok()
            || std::env::var("container").is_ok_and(|v| v == "flatpak")
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Constructs a `std::process::Command` that executes on the host system.
/// If running inside a Flatpak container, wraps the command with `flatpak-spawn --host`.
pub fn host_command(program: &str) -> std::process::Command {
    if is_flatpak() {
        let mut cmd = std::process::Command::new("flatpak-spawn");
        cmd.arg("--host");
        cmd.arg("--unset-env=FLATPAK_ID");
        cmd.arg("--unset-env=container");
        cmd.arg(program);
        cmd
    } else {
        std::process::Command::new(program)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        APP_DIR_NAME, app_config_dir, app_data_dir, config_file, default_global_skills_path,
        host_command, is_flatpak, user_themes_dir,
    };

    #[test]
    fn app_paths_use_platform_safe_dir_name() {
        assert!(app_config_dir().ends_with(APP_DIR_NAME));
        assert!(app_data_dir().ends_with(APP_DIR_NAME));
        assert!(config_file().ends_with(std::path::Path::new(APP_DIR_NAME).join("config.toml")));
        assert!(user_themes_dir().ends_with(std::path::Path::new(APP_DIR_NAME).join("themes")));
    }

    #[test]
    fn default_skill_path_uses_platform_safe_dir_name() {
        assert_eq!(
            default_global_skills_path(),
            if cfg!(target_os = "windows") {
                "~/.config/con-terminal/skills"
            } else {
                "~/.config/con/skills"
            }
        );
    }

    #[test]
    fn host_command_returns_command() {
        let cmd = host_command("git");
        if is_flatpak() {
            assert_eq!(cmd.get_program(), "flatpak-spawn");
        } else {
            assert_eq!(cmd.get_program(), "git");
        }
    }
}
