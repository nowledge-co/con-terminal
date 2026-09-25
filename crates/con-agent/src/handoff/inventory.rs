use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

use futures::future::join_all;
use tokio::sync::Mutex;

use super::{AgentAvailability, AgentKind, protocol, target::probe_executable};

/// Installed CLIs change rarely; probing every product spawns two subprocesses
/// per agent. Background/CLI callers reuse it; opening the panel refreshes it.
const INVENTORY_TTL: Duration = Duration::from_secs(300);

static INVENTORY_CACHE: OnceLock<Mutex<(Instant, Vec<AgentAvailability>)>> = OnceLock::new();

/// Return every known product, including missing or incompatible installations.
/// No source history, authentication, model, or target-creation request is made.
pub async fn installed_agents() -> Vec<AgentAvailability> {
    inventory(false).await
}

/// Refresh on panel open so newly installed or upgraded CLIs appear immediately.
pub async fn refresh_installed_agents() -> Vec<AgentAvailability> {
    inventory(true).await
}

async fn inventory(refresh: bool) -> Vec<AgentAvailability> {
    let cache = INVENTORY_CACHE.get_or_init(|| Mutex::new((Instant::now(), Vec::new())));
    cached_inventory(cache, refresh, || async {
        join_all(AgentKind::ALL.into_iter().map(availability)).await
    })
    .await
}

async fn cached_inventory<F, Fut>(
    cache: &Mutex<(Instant, Vec<AgentAvailability>)>,
    refresh: bool,
    probe: F,
) -> Vec<AgentAvailability>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Vec<AgentAvailability>>,
{
    let mut guard = cache.lock().await;
    if !refresh && !guard.1.is_empty() && guard.0.elapsed() < INVENTORY_TTL {
        return guard.1.clone();
    }
    let agents = probe().await;
    *guard = (Instant::now(), agents.clone());
    agents
}

async fn availability(agent: AgentKind) -> AgentAvailability {
    let executable = match protocol::executable(agent.executable_name()) {
        Ok(executable) => executable,
        Err(error) => {
            return AgentAvailability {
                agent,
                executable: None,
                version: None,
                source_supported: false,
                target_supported: false,
                diagnostic: Some(error.to_string()),
            };
        }
    };
    match probe_executable(agent, &executable).await {
        Ok(capabilities) => {
            // This describes source-format support, not service side effects:
            // Cursor's session reader can initialize its native services.
            let source_supported = true;
            let diagnostic = if capabilities.automatic_delivery {
                None
            } else if agent == AgentKind::Kimi {
                Some("Automatic PTY delivery when the interactive screen is ready; clipboard fallback on uncertainty.".into())
            } else {
                Some("Native startup supported; paste the handoff instruction manually. Automatic delivery awaits validation.".into())
            };
            AgentAvailability {
                agent,
                executable: Some(executable),
                version: Some(capabilities.version),
                source_supported,
                target_supported: true,
                diagnostic,
            }
        }
        Err(error) => AgentAvailability {
            agent,
            executable: Some(executable),
            version: None,
            source_supported: false,
            target_supported: false,
            diagnostic: Some(error.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn panel_refresh_replaces_recent_inventory_and_cached_reads_reuse_it() {
        let missing = AgentAvailability {
            agent: AgentKind::Codex,
            executable: None,
            version: None,
            source_supported: false,
            target_supported: false,
            diagnostic: None,
        };
        let cache = Mutex::new((Instant::now(), vec![missing.clone()]));
        let cached = cached_inventory(&cache, false, || async { panic!("cache missed") }).await;
        assert!(!cached[0].target_supported);
        let mut installed = missing;
        installed.target_supported = true;
        installed.executable = Some("/new/codex".into());
        let fresh = cached_inventory(&cache, true, || async { vec![installed] }).await;
        assert!(fresh[0].target_supported);
        let cached =
            cached_inventory(&cache, false, || async { panic!("refresh not cached") }).await;
        assert_eq!(cached[0].executable, fresh[0].executable);
    }
}
