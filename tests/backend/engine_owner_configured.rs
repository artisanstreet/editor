//! Focused composition checks for the settings-carrying owner lane.

use std::num::NonZeroUsize;

use super::{EngineOwner, EngineOwnerHealth, EngineOwnerShutdown};

#[tokio::test(flavor = "current_thread")]
async fn configured_owner_starts_active_and_drains_without_child() {
    let mut owner = EngineOwner::start_configured(
        NonZeroUsize::new(1).expect("one slot is nonzero"),
        &tokio::runtime::Handle::current(),
    );

    assert_eq!(owner.health(), EngineOwnerHealth::Active);
    assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
}
