use std::ffi::{OsStr, OsString};

#[derive(Debug, Clone, PartialEq, Eq)]
enum EnvUpdate {
    Set(OsString),
    Remove,
    Unchanged,
}

pub fn sanitize_inherited_terminal_environment() {
    // SAFETY: startup is single-threaded here. A nested con must never
    // accidentally advertise the enclosing terminal's resume capability.
    unsafe {
        std::env::remove_var(con_terminal::program_resume::TOKEN_ENV);
        std::env::remove_var(con_terminal::program_resume::TERMINAL_CONTEXT_ENV);
    };

    let update = conductor_zdotdir_update(
        std::env::var_os("ZDOTDIR").as_deref(),
        std::env::var_os("CONDUCTOR_INTEGRATION_ZDOTDIR").as_deref(),
        std::env::var_os("CONDUCTOR_USER_ZDOTDIR").as_deref(),
        std::env::var_os("CONDUCTOR_ORIGINAL_ZDOTDIR").as_deref(),
    );

    match update {
        EnvUpdate::Set(value) => {
            // SAFETY: callers run this during app startup, before GPUI and
            // terminal worker threads are created.
            unsafe { std::env::set_var("ZDOTDIR", value) };
        }
        EnvUpdate::Remove => {
            // SAFETY: callers run this during app startup, before GPUI and
            // terminal worker threads are created.
            unsafe { std::env::remove_var("ZDOTDIR") };
        }
        EnvUpdate::Unchanged => {}
    }
}

fn conductor_zdotdir_update(
    zdotdir: Option<&OsStr>,
    conductor_integration_zdotdir: Option<&OsStr>,
    conductor_user_zdotdir: Option<&OsStr>,
    conductor_original_zdotdir: Option<&OsStr>,
) -> EnvUpdate {
    let Some(current) = zdotdir else {
        return EnvUpdate::Unchanged;
    };
    let Some(integration) = conductor_integration_zdotdir else {
        return EnvUpdate::Unchanged;
    };
    if current != integration {
        return EnvUpdate::Unchanged;
    }

    conductor_user_zdotdir
        .filter(|value| !value.is_empty())
        .or_else(|| conductor_original_zdotdir.filter(|value| !value.is_empty()))
        .map(|value| {
            if value == current {
                EnvUpdate::Unchanged
            } else {
                EnvUpdate::Set(value.to_os_string())
            }
        })
        .unwrap_or(EnvUpdate::Remove)
}

#[cfg(test)]
mod tests {
    use super::{EnvUpdate, conductor_zdotdir_update};
    use std::ffi::OsStr;

    #[test]
    fn leaves_absent_or_non_conductor_zdotdir_unchanged() {
        assert_eq!(
            conductor_zdotdir_update(None, Some(OsStr::new("/tmp/integration")), None, None),
            EnvUpdate::Unchanged
        );
        assert_eq!(
            conductor_zdotdir_update(
                Some(OsStr::new("/custom/zsh")),
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("/Users/example")),
                None
            ),
            EnvUpdate::Unchanged
        );
    }

    #[test]
    fn restores_user_zdotdir_when_conductor_integration_leaks_in() {
        assert_eq!(
            conductor_zdotdir_update(
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("/Users/example")),
                Some(OsStr::new("/fallback"))
            ),
            EnvUpdate::Set("/Users/example".into())
        );
    }

    #[test]
    fn falls_back_to_original_zdotdir_or_removes_when_no_user_value_exists() {
        assert_eq!(
            conductor_zdotdir_update(
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("/tmp/integration")),
                None,
                Some(OsStr::new("/original"))
            ),
            EnvUpdate::Set("/original".into())
        );
        assert_eq!(
            conductor_zdotdir_update(
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("")),
                Some(OsStr::new("/original"))
            ),
            EnvUpdate::Set("/original".into())
        );
        assert_eq!(
            conductor_zdotdir_update(
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("/tmp/integration")),
                None,
                None
            ),
            EnvUpdate::Remove
        );
    }

    #[test]
    fn leaves_matching_conductor_fallback_unchanged() {
        assert_eq!(
            conductor_zdotdir_update(
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("/original"))
            ),
            EnvUpdate::Unchanged
        );
        assert_eq!(
            conductor_zdotdir_update(
                Some(OsStr::new("/tmp/integration")),
                Some(OsStr::new("/tmp/integration")),
                None,
                Some(OsStr::new("/tmp/integration"))
            ),
            EnvUpdate::Unchanged
        );
    }
}
