use artisan_api::{
    EngineConfig, EngineId,
    tls::{Identity, fingerprint},
};
use editor_core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use std::time::{Duration, Instant};

#[test]
fn a_model_choice_forge_did_not_save_stays_chosen_with_a_retry() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .expect("core opens");
    let identity = Identity::generate("picker-test").expect("identity");
    let pin = fingerprint(identity.certificate_der().expect("certificate").as_ref());
    let config = HostConfig {
        id: HostId("unreachable-forge".into()),
        name: "Unreachable".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: pin.clone(),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(&config.address, &pin, &identity).expect("client");
    core.add_test_host(&config, client);
    let chosen: EngineConfig = serde_json::from_value(serde_json::json!({
        "engine": "claude",
        "model": "chosen-model",
        "effort": null,
    }))
    .expect("engine config");
    core.host_mut(&config.id)
        .expect("host")
        .remember_model_config(chosen.clone());

    let deadline = Instant::now() + Duration::from_secs(20);
    while core
        .host(&config.id)
        .expect("host")
        .preference_failure()
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "the failed save never reached the composer"
        );
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
    let host = core.host(&config.id).expect("host");
    assert_eq!(
        host.pending_harness_config(EngineId::Claude),
        Some(&chosen),
        "the choice disappeared after Forge failed to save it"
    );
    core.host_mut(&config.id).expect("host").retry_preferences();
    assert!(
        core.host(&config.id)
            .expect("host")
            .preference_failure()
            .is_none()
    );
}
