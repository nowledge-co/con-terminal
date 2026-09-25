//! Post-prepare validation: the workspace snapshot and the exported source
//! history must not have drifted between prepare and delivery.
use anyhow::{Result, ensure};
use con_agent::handoff::export_session;

use super::{HandoffService, snapshot};

impl HandoffService {
    pub fn validate_workspace(&self, id: &str) -> Result<()> {
        snapshot::unchanged(&self.bundle(id)?.workspace)
    }

    pub async fn validate_source(&self, id: &str) -> Result<()> {
        let service = self.clone();
        let id = id.to_owned();
        let bundle = tokio::task::spawn_blocking(move || service.bundle(&id)).await??;
        let latest = export_session(
            bundle.history.source.agent,
            &bundle.workspace.cwd,
            &bundle.history.source.id,
        )
        .await?;
        ensure!(
            latest.source.agent == bundle.history.source.agent
                && latest.source.store_identity == bundle.history.source.store_identity
                && latest.digest == bundle.history.digest
                && latest.last_turn_id == bundle.history.last_turn_id,
            "Source history changed; send a new handoff"
        );
        Ok(())
    }
}
