use super::*;
#[test]
fn install_delta_wire_does_not_repeat_catalog_or_account_fields() {
    let event = Event::EngineInstall {
        engine: crate::EngineId::Codex,
        install: crate::InstallState::Downloading {
            version: "1.2.3".into(),
            received: 1024,
            total: Some(2048),
        },
    };
    let wire = serde_json::to_value(&event).unwrap();
    assert_eq!(wire["type"], "engine_install");
    assert_eq!(wire.as_object().unwrap().len(), 3);
    assert!(wire.get("models").is_none() && wire.get("status").is_none());
    let replay: Event = serde_json::from_value(wire).unwrap();
    assert_eq!(event, replay);
}
