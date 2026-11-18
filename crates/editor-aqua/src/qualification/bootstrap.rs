use super::fixture::{LoadBootstrapError, PairingError, VerifiedBootstrap, VerifyInputsError};
use crate::core::config_file::WriteConfigError;
use crate::core::hosts::PairError;
use crate::core::{Core, EditorFiles, HostId, IdentityError, OpenCoreError, Screen, hosts};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("load the qualification request")]
    Load(#[source] LoadBootstrapError),
    #[error("invite the qualification Editor to Forge")]
    Invite(#[source] PairingError),
    #[error("load the qualification Editor's identity")]
    Identity(#[source] IdentityError),
    #[error("pair the qualification Editor with Forge")]
    Pair(#[source] PairError),
    #[error("save the qualification Editor's host {host}")]
    SaveHosts {
        host: HostId,
        #[source]
        source: WriteConfigError,
    },
    #[error("check that the qualification inputs are unchanged")]
    Inputs(#[source] VerifyInputsError),
}

pub struct Bootstrap {
    verified: VerifiedBootstrap,
    files: EditorFiles,
}

impl Bootstrap {
    pub fn load(path: &Path) -> Result<Self, BootstrapError> {
        let runtime = crate::core::runtime::runtime();
        let verified = runtime
            .block_on(VerifiedBootstrap::load(path))
            .map_err(BootstrapError::Load)?;
        let pairing = runtime
            .block_on(verified.pairing_input())
            .map_err(BootstrapError::Invite)?;
        let files = EditorFiles {
            dir: verified.editor_dir().to_owned(),
        };
        let identity = files.identity().map_err(BootstrapError::Identity)?;
        let mut config = runtime
            .block_on(hosts::pair(
                &pairing.address,
                &pairing.code,
                &pairing.fingerprint,
                &identity,
                "qualification",
                "Forge".to_owned(),
                hosts::HostKind::Local,
            ))
            .map_err(BootstrapError::Pair)?;
        let host = verified.host().clone();
        config.id = host.clone();
        files
            .save_hosts(&hosts::HostsFile {
                hosts: vec![config],
                last: Some(host.clone()),
            })
            .map_err(|source| BootstrapError::SaveHosts { host, source })?;
        verified.verify_inputs().map_err(BootstrapError::Inputs)?;
        Ok(Self { verified, files })
    }

    pub fn core(&self) -> Result<Core, OpenCoreError> {
        let mut core = Core::for_tests(self.files.clone())?;
        core.navigate(Screen::Thread {
            host: self.verified.host().clone(),
            thread: self.verified.thread().clone(),
        });
        Ok(core)
    }

    pub fn dimensions(&self) -> (f64, f64) {
        let (width, height) = self.verified.dimensions();
        (f64::from(width), f64::from(height))
    }

    pub fn editor_directory(&self) -> &Path {
        &self.files.dir
    }
}
