use super::*;
use serde_json::{Value, json};
#[test]
fn persisted_settings_and_old_question_answers_upgrade_once_without_new_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    migrate_to(&mut conn, 9).unwrap();
    let codex = json!({"engine":"codex","model":null,"effort":null,"options":{"service_tier":"priority","unrelated":42}});
    let cursor = json!({"engine":"cursor","model":null,"effort":null,"options":{"fast":true}});
    let opencode =
        json!({"engine":"opencode","model":null,"effort":"high","variant":"custom","options":{}});
    conn.execute(
        include_str!("insert_thread_t.sql"),
        [json!({"config":codex}).to_string()],
    )
    .unwrap();
    conn.execute(
        include_str!("insert_default_engine_config.sql"),
        [cursor.to_string()],
    )
    .unwrap();
    conn.execute(
        include_str!("insert_model_engine_configs.sql"),
        [json!({"selected":opencode}).to_string()],
    )
    .unwrap();
    let question = json!({"id":"q","thread_id":"t","position":1,"seq":20,"run_id":null,"parent_id":null,"status":"done","created_at":1,"updated_at":1,"kind":{"type":"question","blocking":false,"questions":[{"id":"q","header":null,"text":"Pick","multi_select":false,"options":[{"label":"Same","description":null},{"label":"Same","description":null}]}],"answers":[["Same"]]}});
    conn.execute(include_str!("insert_question.sql"), [question.to_string()])
        .unwrap();
    migrate(&mut conn).unwrap();
    drop(conn);
    let mut conn = Connection::open(&path).unwrap();
    migrate(&mut conn).unwrap();
    let body: String = conn
        .query_row(include_str!("thread_t_body.sql"), [], |row| row.get(0))
        .unwrap();
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["config"]["speed"], "priority");
    assert_eq!(body["config"]["options"], json!({"unrelated":42}));
    let settings: Vec<(String, i64, String)> = conn
        .prepare(include_str!("kv_rows.sql"))
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(settings[0].1, 18);
    assert_eq!(settings[1].1, 19);
    let default: Value = serde_json::from_str(&settings[0].2).unwrap();
    assert_eq!(default["speed"], "fast");
    assert!(default["options"].get("fast").is_none());
    let models: Value = serde_json::from_str(&settings[1].2).unwrap();
    assert_eq!(models["selected"]["variant"], "custom");
    assert!(models["selected"].get("effort").is_none());
    let (seq, body): (i64, String) = conn
        .query_row(include_str!("question_row.sql"), [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(seq, 20);
    let body: Value = serde_json::from_str(&body).unwrap();
    let kind: artisan_api::ItemKind = serde_json::from_value(body["kind"].clone()).unwrap();
    let artisan_api::ItemKind::Question {
        questions, answers, ..
    } = kind
    else {
        panic!("question changed type")
    };
    let options = questions.inline().unwrap()[0].options.inline().unwrap();
    assert_ne!(options[0].id, options[1].id);
    assert_eq!(
        answers,
        Some(
            vec![Some(artisan_api::QuestionAnswer::Choices {
                ids: vec!["option-0".into()].into()
            })]
            .into()
        )
    );
    assert_eq!(
        conn.query_row(include_str!("thread_t_seq.sql"), [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        17
    );
}
