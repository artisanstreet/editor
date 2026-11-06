use super::{HostCompletion, HostData, HostEvent, HostMessage};
use crate::core::hosts::{self, HostKind};
use tokio_util::task::AbortOnDropHandle;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::core) enum WslTarget {
    #[default]
    None,
    Distro(String),
    Default,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslStatus {
    pub distro: String,
    pub running: bool,
    pub starting: bool,
    pub error: Option<String>,
}

impl HostData {
    pub fn set_pairing(&mut self, kind: &HostKind, address: &str) {
        self.wsl_target = match kind {
            HostKind::Wsl { distro } => WslTarget::Distro(distro.clone()),
            HostKind::Local | HostKind::Remote if cfg!(windows) && is_loopback(address) => {
                WslTarget::Default
            }
            HostKind::Local | HostKind::Remote | HostKind::Account { .. } => WslTarget::None,
        };
    }
    pub(super) fn learn_wsl_distro(&mut self, distro: Option<&str>) {
        let Some(distro) = distro else {
            return;
        };
        if self.wsl_target == WslTarget::None
            || self.wsl_target == WslTarget::Distro(distro.to_owned())
        {
            return;
        }
        self.wsl_target = WslTarget::Distro(distro.to_owned());
        self.pending_events.push(HostEvent::WslDistro {
            distro: distro.to_owned(),
        });
    }
    pub(super) fn wsl_connection_changed(&mut self) {
        if self.wsl_target == WslTarget::None {
            return;
        }
        if self.is_live() {
            self.wsl = None;
            self.wsl_probe = None;
            self.keep_wsl_awake();
        } else if matches!(self.status, crate::core::ConnectionStatus::Offline { .. }) {
            self.probe_wsl();
        }
    }
    fn keep_wsl_awake(&mut self) {
        let WslTarget::Distro(distro) = &self.wsl_target else {
            return;
        };
        if let Some(child) = &mut self.wsl_awake
            && matches!(child.try_wait(), Ok(None))
        {
            return;
        }
        self.wsl_awake = hosts::keep_wsl_awake(distro)
            .inspect_err(|error| tracing::warn!("couldn't keep {distro} awake: {error:#}"))
            .ok();
    }
    fn probe_wsl(&mut self) {
        if self.wsl_probe.is_some() {
            return;
        }
        self.wsl_generation = self.wsl_generation.wrapping_add(1);
        let generation = self.wsl_generation;
        let target = self.wsl_target.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = hosts::wsl_state().await;
            let _ = sender
                .send(HostCompletion::WslProbe(generation, target, result))
                .await;
        });
        self.wsl_probe = Some(AbortOnDropHandle::new(task));
    }
    pub(super) fn finish_wsl_probe(
        &mut self,
        generation: u64,
        target: WslTarget,
        result: anyhow::Result<Vec<hosts::WslDistro>>,
    ) -> bool {
        if generation != self.wsl_generation {
            return false;
        }
        self.wsl_probe = None;
        if self.is_live() {
            return false;
        }
        let distros = match result {
            Ok(distros) => distros,
            Err(error) => {
                tracing::debug!("couldn't read WSL's state: {error:#}");
                return false;
            }
        };
        let found = distros.into_iter().find(|distro| match &target {
            WslTarget::Distro(name) => &distro.name == name,
            WslTarget::Default => distro.default,
            WslTarget::None => false,
        });
        let status = found.map(|distro| WslStatus {
            distro: distro.name,
            running: distro.running,
            starting: self.wsl.as_ref().is_some_and(|wsl| wsl.starting),
            error: self.wsl.as_ref().and_then(|wsl| wsl.error.clone()),
        });
        if self.wsl == status {
            return false;
        }
        self.wsl = status;
        true
    }
    pub fn status_line(&self) -> Option<&'static str> {
        match &self.status {
            crate::core::ConnectionStatus::Live => None,
            crate::core::ConnectionStatus::Connecting => Some("Connecting…"),
            crate::core::ConnectionStatus::Offline { .. } => Some(match &self.wsl {
                Some(wsl) if !wsl.running => "WSL stopped",
                Some(_) => "Forge stopped",
                None => "Offline",
            }),
        }
    }
    pub fn start_wsl(&mut self) {
        let Some(wsl) = &mut self.wsl else {
            return;
        };
        if wsl.starting {
            return;
        }
        wsl.starting = true;
        wsl.error = None;
        let distro = wsl.distro.clone();
        self.pending_events.push(HostEvent::Changed);
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = hosts::start_wsl_forge(distro.clone()).await;
            let _ = sender
                .send(HostCompletion::WslStarted(distro, result))
                .await;
        });
        self.wsl_start = Some(AbortOnDropHandle::new(task));
    }
    pub(super) fn finish_wsl_start(&mut self, distro: String, result: anyhow::Result<()>) {
        self.wsl_start = None;
        if let Some(wsl) = &mut self.wsl {
            wsl.starting = false;
            wsl.error = result.as_ref().err().map(|error| format!("{error:#}"));
        }
        if let Err(error) = &result {
            tracing::warn!(distro, "couldn't start Forge in WSL: {error:#}");
        }
        if self.wsl_target == WslTarget::Default {
            self.wsl_target = WslTarget::Distro(distro);
        }
        self.retry_now();
        self.probe_wsl();
    }
}
fn is_loopback(address: &str) -> bool {
    let host = address
        .rsplit_once(':')
        .map_or(address, |(host, _)| host)
        .trim_matches(['[', ']']);
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}
