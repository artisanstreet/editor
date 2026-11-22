use super::{Artifact, Engines, InstallError, Resolved, install};
use artisan_api::EngineId;

impl Engines {
    pub(crate) async fn install_resolved(
        &self,
        engine: EngineId,
        artifact: Artifact,
        resolved: Resolved,
    ) -> Result<String, InstallError> {
        let _lock = install::lock(&self.inner.layout, engine)
            .await
            .map_err(|source| InstallError::Lock { engine, source })?;
        let result = self
            .fetch(engine, artifact, &resolved)
            .await
            .map(|()| resolved.version.clone())
            .map_err(InstallError::Fetch);
        self.finish(engine, result).await
    }
}
