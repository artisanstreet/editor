//! Forge in WSL: when it is unreachable, whether its distribution runs and a
//! way to start it; while it is live, a session that keeps WSL from shutting
//! the distribution (and Forge) down once no terminal is open.

use super::{HostData, HostEvent};
use crate::hosts::{self, HostKind};
use gpui::Context;

/// Which WSL distribution a host's Forge runs in, as far as the Editor knows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WslTarget {
    /// Not in WSL, or not on Windows.
    #[default]
    None,
    /// Paired with this distribution, or Forge said it runs there.
    Distro(String),
    /// A loopback address on Windows: most likely the default distribution.
    Default,
}

/// What WSL looks like while a WSL host's Forge is unreachable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslStatus {
    pub distro: String,
    /// Whether the distribution runs; when it does, Forge in it doesn't.
    pub running: bool,
    /// Starting the distribution and its Forge.
    pub starting: bool,
    /// Why the last start failed.
    pub error: Option<String>,
}

impl HostData {
    /// Where the host's Forge runs, from its pairing: a WSL pairing names its
    /// distribution; a loopback address on Windows is probably WSL's.
    pub fn set_pairing(&mut self, kind: &HostKind, address: &str) {
        self.wsl_target = match kind {
            HostKind::Wsl { distro } => WslTarget::Distro(distro.clone()),
            HostKind::Local | HostKind::Remote if cfg!(windows) && is_loopback(address) => {
                WslTarget::Default
            }
            HostKind::Local | HostKind::Remote => WslTarget::None,
        };
    }

    /// Forge's own word on its distribution; emitted so the pairing is saved.
    pub(super) fn learn_wsl_distro(&mut self, distro: Option<&str>, cx: &mut Context<Self>) {
        let Some(distro) = distro else {
            return;
        };
        if self.wsl_target == WslTarget::None
            || self.wsl_target == WslTarget::Distro(distro.to_owned())
        {
            return;
        }
        self.wsl_target = WslTarget::Distro(distro.to_owned());
        cx.emit(HostEvent::WslDistro {
            distro: distro.to_owned(),
        });
    }

    /// Follows the connection: checks WSL while offline, keeps it awake while live.
    pub(super) fn wsl_connection_changed(&mut self, cx: &Context<Self>) {
        if self.wsl_target == WslTarget::None {
            return;
        }
        if self.is_live() {
            self.wsl = None;
            self.wsl_probe = None;
            self.keep_wsl_awake();
        } else if matches!(self.status, crate::ConnectionStatus::Offline { .. }) {
            self.probe_wsl(cx);
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

    /// Reads whether the distribution runs; one read at a time.
    fn probe_wsl(&mut self, cx: &Context<Self>) {
        if self.wsl_probe.is_some() {
            return;
        }
        let target = self.wsl_target.clone();
        self.wsl_probe = Some(cx.spawn(async move |this, cx| {
            let state = hosts::wsl_state().await;
            this.update(cx, |host, cx| {
                host.wsl_probe = None;
                if host.is_live() {
                    return;
                }
                let distros = match state {
                    Ok(distros) => distros,
                    Err(error) => {
                        tracing::debug!("couldn't read WSL's state: {error:#}");
                        return;
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
                    starting: host.wsl.as_ref().is_some_and(|wsl| wsl.starting),
                    error: host.wsl.as_ref().and_then(|wsl| wsl.error.clone()),
                });
                if host.wsl != status {
                    host.wsl = status;
                    cx.emit(HostEvent::Changed);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// The connection in a few words, for host rows; `None` while live.
    #[must_use]
    pub const fn status_line(&self) -> Option<&'static str> {
        match &self.status {
            crate::ConnectionStatus::Live => None,
            crate::ConnectionStatus::Connecting => Some("Connecting…"),
            crate::ConnectionStatus::Offline { .. } => Some(match &self.wsl {
                Some(wsl) if !wsl.running => "WSL stopped",
                Some(_) => "Forge stopped",
                None => "Offline",
            }),
        }
    }

    /// Starts the distribution and its Forge, then reconnects at once.
    pub fn start_wsl(&mut self, cx: &mut Context<Self>) {
        let Some(wsl) = &mut self.wsl else {
            return;
        };
        if wsl.starting {
            return;
        }
        wsl.starting = true;
        wsl.error = None;
        let distro = wsl.distro.clone();
        cx.emit(HostEvent::Changed);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = hosts::start_wsl_forge(distro.clone()).await;
            this.update(cx, |host, cx| {
                if let Some(wsl) = &mut host.wsl {
                    wsl.starting = false;
                    wsl.error = result.as_ref().err().map(|error| format!("{error:#}"));
                }
                if let Err(error) = &result {
                    tracing::warn!(distro, "couldn't start Forge in WSL: {error:#}");
                }
                // A Forge found by address is in this distribution.
                if host.wsl_target == WslTarget::Default {
                    host.wsl_target = WslTarget::Distro(distro.clone());
                }
                host.retry_now();
                host.probe_wsl(cx);
                cx.emit(HostEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_addresses_are_recognised_with_their_ports() {
        for address in ["127.0.0.1:47901", "localhost:47900", "[::1]:47900"] {
            assert!(is_loopback(address), "{address}");
        }
        for address in ["192.168.1.4:47900", "forge.example:47900"] {
            assert!(!is_loopback(address), "{address}");
        }
    }

    #[test]
    fn wsl_listing_marks_the_default_and_running_distributions() {
        let text = "  NAME            STATE           VERSION\r\n\
                    * Ubuntu          Stopped         2\r\n  \
                    Debian          Running         2\r\n";
        assert_eq!(
            hosts::parse_wsl_verbose(text),
            [
                hosts::WslDistro {
                    name: "Ubuntu".into(),
                    default: true,
                    running: false,
                },
                hosts::WslDistro {
                    name: "Debian".into(),
                    default: false,
                    running: true,
                },
            ]
        );
    }
}
