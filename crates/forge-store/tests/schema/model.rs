use super::*;
use serde_json::{Value, json};

#[test]
fn remembered_model_configs_keep_full_access() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate_to(&mut conn, 23).unwrap();
    let configs = json!({
        "claude-opus-5-5": {"engine":"claude","model":"claude-opus-5-5","effort":"medium",
            "access":"full","approvals":"never","options":{"permission_mode":"bypassPermissions"}},
        "claude-sonnet-5-5": {"engine":"claude","model":"claude-sonnet-5-5","effort":"medium",
            "access":"full","approvals":"on_request","options":{"permission_mode":"auto"}},
    });
    conn.execute(
        "INSERT INTO kv VALUES ('model_engine_configs',7,?1)",
        [configs.to_string()],
    )
    .unwrap();
    migrate(&mut conn).unwrap();
    migrate(&mut conn).unwrap();
    let (seq, value): (i64, String) = conn
        .query_row(
            "SELECT seq,value FROM kv WHERE key='model_engine_configs'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let value: Value = serde_json::from_str(&value).unwrap();
    let config = |model: &str| {
        artisan_api::saved_model_config(&value, artisan_api::EngineId::Claude, model).unwrap()
    };
    assert_eq!(
        config("claude-opus-5-5").permissions,
        artisan_api::Permissions::Full
    );
    assert_eq!(
        config("claude-sonnet-5-5").permissions,
        artisan_api::Permissions::Auto
    );
    assert!(config("claude-opus-5-5").options.is_empty());
    assert_eq!(seq, 7);
}
