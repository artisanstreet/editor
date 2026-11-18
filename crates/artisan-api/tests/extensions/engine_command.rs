use super::EngineCommand;
#[test]
fn command_arguments_are_required_and_unrelated_arguments_are_rejected() {
    assert!(serde_json::from_str::<EngineCommand>(r#"{"command":"use"}"#).is_err());
    assert!(serde_json::from_str::<EngineCommand>(r#"{"command":"login-code"}"#).is_err());
    assert!(
        serde_json::from_str::<EngineCommand>(r#"{"command":"install","version":"1"}"#).is_err()
    );
    let command = EngineCommand::Use {
        version: "1.2.3".into(),
    };
    assert_eq!(
        serde_json::from_str::<EngineCommand>(&serde_json::to_string(&command).unwrap()).unwrap(),
        command
    );
}
