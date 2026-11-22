use super::EngineCommand;
#[test]
fn command_arguments_are_required_and_unrelated_arguments_are_rejected() {
    serde_json::from_str::<EngineCommand>(r#"{"command":"use"}"#).expect_err("use needs a version");
    serde_json::from_str::<EngineCommand>(r#"{"command":"login-code"}"#)
        .expect_err("login-code needs a code");
    serde_json::from_str::<EngineCommand>(r#"{"command":"install","version":"1"}"#)
        .expect_err("install takes no version");
    let command = EngineCommand::Use {
        version: "1.2.3".into(),
    };
    assert_eq!(
        serde_json::from_str::<EngineCommand>(&serde_json::to_string(&command).unwrap()).unwrap(),
        command
    );
}
