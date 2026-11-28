use super::*;

#[test]
fn ultra_commands_are_keywords_of_the_turn_they_start() {
    assert_eq!(
        normalize_command(EngineId::Claude, "/ultracode ship the parser").unwrap(),
        Some("ultracode\nship the parser".to_owned())
    );
    assert_eq!(
        normalize_command(EngineId::Claude, "  /ultrathink why?").unwrap(),
        Some("ultrathink\nwhy?".to_owned())
    );
    for text in ["/ultracode", "/ultrathink  "] {
        assert!(normalize_command(EngineId::Claude, text).is_err(), "{text}");
    }
    for (engine, text) in [
        (EngineId::Codex, "/ultracode ship it"),
        (EngineId::Claude, "/goal ship it"),
        (EngineId::Claude, "plain text"),
    ] {
        assert!(normalize_command(engine, text).unwrap().is_none(), "{text}");
    }
}

#[test]
fn compact_command_parses_only_leading_compact_with_optional_follow_up() {
    assert_eq!(compact_argument("/compact"), Some(""));
    assert_eq!(compact_argument("  /compact  "), Some(""));
    assert_eq!(
        compact_argument("/compact\n\nAnd use the real color token values"),
        Some("And use the real color token values")
    );
    assert_eq!(compact_argument("/compacting"), None);
    assert_eq!(compact_argument("please /compact"), None);
}
