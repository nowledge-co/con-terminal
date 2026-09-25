use super::*;
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum HandoffRpc {
    Open,
    Agents,
    Sources {
        cwd: PathBuf,
        #[serde(default)]
        agent: con_agent::handoff::AgentKind,
    },
    Prepare {
        #[serde(flatten)]
        request: PrepareRequest,
    },
    List {
        cwd: PathBuf,
    },
    Get {
        job_id: String,
    },
    Start {
        job_id: String,
        expected_revision: u64,
        tab_index: Option<usize>,
        #[serde(default)]
        source: crate::SurfaceTarget,
    },
    Respond {
        job_id: String,
        expected_revision: u64,
        outcome: HandoffResponse,
    },
    Cancel {
        job_id: String,
        expected_revision: u64,
    },
}

impl HandoffRpc {
    pub fn method(&self) -> &'static str {
        match self {
            Self::Open => "handoffs.open",
            Self::Agents => "handoffs.agents",
            Self::Sources { .. } => "handoffs.sources",
            Self::Prepare { .. } => "handoffs.prepare",
            Self::List { .. } => "handoffs.list",
            Self::Get { .. } => "handoffs.get",
            Self::Start { .. } => "handoffs.start",
            Self::Respond { .. } => "handoffs.respond",
            Self::Cancel { .. } => "handoffs.cancel",
        }
    }
    pub fn params(&self) -> Value {
        let mut value = serde_json::to_value(self).expect("Handoff RPC is serializable");
        value.as_object_mut().unwrap().remove("operation");
        value
    }
    pub fn parse(method: &str, mut params: Value) -> Result<Self> {
        let operation = method
            .strip_prefix("handoffs.")
            .ok_or_else(|| anyhow::anyhow!("Unknown handoff method"))?;
        ensure!(
            [
                "open", "agents", "sources", "prepare", "list", "get", "start", "respond", "cancel"
            ]
            .contains(&operation),
            "Unknown handoff method"
        );
        let object = params
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("Handoff parameters must be an object"))?;
        object.insert("operation".into(), json!(operation));
        Ok(serde_json::from_value(params)?)
    }
    /// Start additionally needs a live Con window and is handled by con-app.
    pub async fn execute(self) -> Result<Value> {
        ensure!(
            cfg!(target_os = "macos"),
            "Agent Handoff currently supports macOS only"
        );
        if let Self::Agents = self {
            return Ok(serde_json::to_value(
                con_agent::handoff::installed_agents().await,
            )?);
        }
        if let Self::Sources { cwd, agent } = self {
            return Ok(serde_json::to_value(
                con_agent::handoff::discover_sessions(agent, &cwd).await?,
            )?);
        }
        let service = tokio::task::spawn_blocking(HandoffService::new).await??;
        if let Self::Prepare { request } = self {
            let job = service.prepare(request).await?;
            return tokio::task::spawn_blocking(move || {
                Ok(json!({"job":job,"preview":service.bundle(&job.id)?.context}))
            })
            .await?;
        }
        tokio::task::spawn_blocking(move || match self {
            Self::List {cwd} => Ok(serde_json::to_value(service.list(&cwd)?)?),
            Self::Get {job_id} => Ok(json!({"job":service.get(&job_id)?,"preview":service.bundle(&job_id)?.context,"instruction":instruction(&job_id)})),
            Self::Respond {job_id,expected_revision,outcome} => Ok(serde_json::to_value(service.respond(&job_id,expected_revision,outcome)?)?),
            Self::Cancel {job_id,expected_revision} => Ok(serde_json::to_value(service.cancel(&job_id,expected_revision)?)?),
            _ => bail!("This operation requires a live Con surface"),
        }).await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rpc_requires_revision_and_keeps_outcomes() {
        // The source-stop confirmation was removed (2026-09-24): prepare no
        // longer carries `source_stopped` and parses without it.
        let params = json!({"request_id":"id","cwd":"/tmp","source_session_id":"source"});
        let cmd = HandoffRpc::parse("handoffs.prepare", params).unwrap();
        assert_eq!(cmd.method(), "handoffs.prepare");
        assert_eq!(cmd.params()["request_id"], json!("id"));
        assert!(HandoffRpc::parse("handoffs.start", json!({"job_id":"id"})).is_err());
        let params = json!({"job_id":"id","expected_revision":4,"outcome":"confirm_stopped"});
        let cmd = HandoffRpc::parse("handoffs.respond", params.clone()).unwrap();
        assert_eq!(cmd.params(), params);
        assert_eq!(cmd.method(), "handoffs.respond");
        assert!(HandoffRpc::parse("handoffs.run", json!({})).is_err());
    }
}
