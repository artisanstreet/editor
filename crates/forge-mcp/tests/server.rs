use artisan_api::ThreadId;
use forge_engines::Engines;
use forge_mcp::{ArtisanMcp, server};
use forge_runs::Hub;
use forge_store::Store;
use serde_json::{Value, json};

async fn replies(messages: Value) -> Option<Value> {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let store = Store::open(&directory.path().join("store")).expect("a fresh store opens");
    let hub = Hub::new(
        store,
        Engines::new(directory.path()),
        directory.path().to_path_buf(),
    );
    let reply = server::handle(&ArtisanMcp::new(hub.clone()), &ThreadId::new(), messages).await;
    hub.shutdown();
    reply
}

#[tokio::test]
async fn malformed_requests_with_an_id_are_answered_as_invalid() {
    for (message, id) in [
        (json!({"jsonrpc":"2.0","id":7}), json!(7)),
        (
            json!({"jsonrpc":"2.0","id":"call-8","method":12}),
            json!("call-8"),
        ),
        (
            json!({"jsonrpc":"2.0","id":9,"method":"ping","params":"loose"}),
            json!(9),
        ),
        (json!({"jsonrpc":"1.0","id":10,"method":"ping"}), json!(10)),
        (
            json!({"jsonrpc":"2.0","id":{"nested":true},"method":"ping"}),
            Value::Null,
        ),
        (json!("not a message"), Value::Null),
    ] {
        let reply = replies(message.clone())
            .await
            .unwrap_or_else(|| panic!("no reply to {message}"));
        assert_eq!(reply["error"]["code"], -32600, "{message}: {reply}");
        assert_eq!(reply["id"], id, "{message}: {reply}");
    }
}

#[tokio::test]
async fn a_batch_answers_its_malformed_requests_and_not_its_notifications() {
    let reply = replies(json!([
        {"jsonrpc":"2.0","method":"notifications/initialized"},
        {"jsonrpc":"2.0","id":1},
        {"jsonrpc":"2.0","id":2,"method":"ping"},
    ]))
    .await
    .unwrap();
    assert_eq!(
        reply,
        json!([
            {"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"A JSON-RPC request has a method name."}},
            {"jsonrpc":"2.0","id":2,"result":{}},
        ])
    );
    let empty = replies(json!([])).await.unwrap();
    assert_eq!(empty["error"]["code"], -32600, "{empty}");
    assert!(
        replies(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await
            .is_none()
    );
}
