use super::fixture::VerifiedBootstrap;
use crate::core::{Core, EditorFiles, HostId, Screen, hosts};
use anyhow::{Context, Result};
use std::path::Path;

pub struct Bootstrap {
    verified: VerifiedBootstrap,
    files: EditorFiles,
    host: HostId,
    thread: artisan_api::ThreadId,
}

impl Bootstrap {
    pub fn load(path: &Path) -> Result<Self> {
        let runtime = crate::core::runtime::runtime();
        let verified = runtime.block_on(VerifiedBootstrap::load(path))?;
        let pairing = runtime.block_on(verified.server.pairing_input())?;
        let files = EditorFiles {
            dir: verified.editor_dir.clone(),
        };
        let identity = files.identity()?;
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
            .context("pair qualification Editor with Forge")?;
        let host = HostId(verified.request.host_id.clone());
        config.id = host.clone();
        files.save_hosts(&hosts::HostsFile {
            hosts: vec![config],
            last: Some(host.clone()),
        })?;
        verified.verify_inputs()?;
        let thread = artisan_api::ThreadId::from(verified.request.thread_id.clone());
        Ok(Self {
            verified,
            files,
            host,
            thread,
        })
    }

    pub fn core(&self) -> Result<Core> {
        let mut core = Core::for_tests(self.files.clone())?;
        core.navigate(Screen::Thread {
            host: self.host.clone(),
            thread: self.thread.clone(),
        });
        Ok(core)
    }

    pub fn dimensions(&self) -> (f64, f64) {
        (
            f64::from(self.verified.request.width),
            f64::from(self.verified.request.height),
        )
    }

    pub fn editor_directory(&self) -> &Path {
        &self.files.dir
    }
}
