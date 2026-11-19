use artisan_api::{Permissions, UnsandboxedReason};
use forge_isolation::{Support, Unavailable};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Host {
    Enforced,
    Unavailable(Unavailable),
}

impl From<Support> for Host {
    fn from(support: Support) -> Self {
        match support {
            Support::Enforced(_) => Self::Enforced,
            Support::Unavailable(unavailable) => Self::Unavailable(unavailable),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Coverage {
    Watching {
        support: Host,
        launched: Permissions,
    },
    Announced,
}

impl Coverage {
    pub(crate) const fn new(support: Host, launched: Permissions) -> Self {
        Self::Watching { support, launched }
    }

    pub(crate) fn observe(&mut self, now: Permissions) -> Option<UnsandboxedReason> {
        match *self {
            Self::Announced => None,
            Self::Watching { support, launched } => {
                let reason = gap(support, launched, now)?;
                *self = Self::Announced;
                Some(reason)
            }
        }
    }
}

const fn gap(support: Host, launched: Permissions, now: Permissions) -> Option<UnsandboxedReason> {
    match (now, support, launched) {
        (Permissions::Full, _, _) | (Permissions::Auto, Host::Enforced, Permissions::Auto) => None,
        (Permissions::Auto, Host::Unavailable(unavailable), _) => Some(missing(unavailable)),
        (Permissions::Auto, Host::Enforced, Permissions::Full) => {
            Some(UnsandboxedReason::StartedWithFullAccess)
        }
    }
}

const fn missing(unavailable: Unavailable) -> UnsandboxedReason {
    match unavailable {
        Unavailable::KernelWithoutLandlock => UnsandboxedReason::KernelWithoutLandlock,
        Unavailable::Windows => UnsandboxedReason::Windows,
        Unavailable::MacOs => UnsandboxedReason::MacOs,
        Unavailable::OtherOs => UnsandboxedReason::OtherOs,
    }
}

#[must_use]
pub fn auto_notice_on_this_host() -> Option<&'static str> {
    match forge_isolation::support() {
        Support::Enforced(_) => None,
        Support::Unavailable(unavailable) => Some(notice(missing(unavailable))),
    }
}

pub(crate) const fn notice(reason: UnsandboxedReason) -> &'static str {
    match reason {
        UnsandboxedReason::KernelWithoutLandlock => {
            "This run isn't sandboxed: this Linux kernel has no Landlock, so the engine can write anywhere you can."
        }
        UnsandboxedReason::Windows => {
            "This run isn't sandboxed: Forge has no sandbox on Windows, so the engine can write anywhere you can."
        }
        UnsandboxedReason::MacOs => {
            "This run isn't sandboxed: Forge has no sandbox on macOS, so the engine can write anywhere you can."
        }
        UnsandboxedReason::OtherOs => {
            "This run isn't sandboxed: Forge has no sandbox on this system, so the engine can write anywhere you can."
        }
        UnsandboxedReason::StartedWithFullAccess => {
            "This run started with Full access, so it finishes outside Forge's sandbox. The next run is sandboxed."
        }
    }
}
