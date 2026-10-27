//! Account machines known to this device and how to reach them.

use crate::sync::MachineRecord;

/// The machines of an account plus which are reachable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Discovery {
    pub account: Option<String>,
    pub machines: Vec<MachineRecord>,
}

impl Discovery {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the cached machine list (account authority; Forge data is
    /// never the discovery source).
    pub fn replace(&mut self, account: String, machines: Vec<MachineRecord>) {
        self.account = Some(account);
        self.machines = machines;
    }

    #[must_use]
    pub fn contains(&self, machine: &str) -> bool {
        self.machines.iter().any(|record| record.machine == machine)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn discovery_lists_account_machines() {
        let mut discovery = Discovery::new();
        discovery.replace(
            "alice".into(),
            vec![MachineRecord {
                machine: "forge-01".into(),
                ed25519_pk_b64u: "key".into(),
                noise_static_b64u: "noise".into(),
                fingerprint: "fp".into(),
            }],
        );
        assert!(discovery.contains("forge-01"));
        assert!(!discovery.contains("forge-02"));
    }
}
