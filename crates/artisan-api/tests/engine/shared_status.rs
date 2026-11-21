use super::*;
#[test]
fn shared_catalog_wire_stays_an_array_and_empty_versions_stay_omitted() {
    let status: EngineStatus = serde_json::from_value(serde_json::json!({
        "engine":"claude","install":{"state":"absent"},"sign_in":{"state":"unknown"},
        "models":[],"default_model":null,"usage":null
    }))
    .unwrap();
    let copy = status.clone();
    assert!(std::sync::Arc::ptr_eq(&copy.models, &status.models));
    assert!(std::sync::Arc::ptr_eq(&copy.versions, &status.versions));
    let wire = serde_json::to_value(status).unwrap();
    assert_eq!(wire["models"], serde_json::json!([]));
    assert!(wire.get("versions").is_none());
}
