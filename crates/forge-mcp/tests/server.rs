use artisan_api::ThreadId;
use forge_engines::Engines;
use forge_mcp::{ArtisanMcp, Content, Tool, ToolSet, server};
use forge_runs::{Acting, GatewayCaller, Hub};
use forge_store::Store;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

async fn replies(messages: Value) -> Option<Value> {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let store = Store::open(&directory.path().join("store")).expect("a fresh store opens");
    let hub = Hub::new(
        store,
        Engines::new(directory.path()),
        directory.path().to_path_buf(),
    );
    let reply = server::handle(
        &ArtisanMcp::new(hub.clone()),
        &GatewayCaller {
            thread: ThreadId::new(),
            ended: CancellationToken::new(),
            acting: Acting::Run {
                stopped: CancellationToken::new(),
            },
        },
        messages,
    )
    .await;
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

#[derive(Debug, thiserror::Error)]
#[error("the gated tool set never fails")]
struct NeverFails;

impl forge_mcp::ToolError for NeverFails {
    fn failure(&self) -> forge_mcp::Failure<'_> {
        forge_mcp::Failure::Broken
    }
}

#[derive(Clone, Default)]
struct Gated {
    waiting: Arc<tokio::sync::Notify>,
    run_ended: Arc<tokio::sync::Notify>,
    created: Arc<AtomicUsize>,
}

fn no_properties() -> Value {
    json!({})
}

impl ToolSet for Gated {
    type Error = NeverFails;

    fn name(&self) -> &'static str {
        "gated"
    }

    fn title(&self) -> &'static str {
        "Gated"
    }

    fn instructions(&self) -> &'static str {
        ""
    }

    fn tools(&self) -> Vec<Tool> {
        ["wait_for_agents", "create_thread"]
            .into_iter()
            .map(|name| Tool {
                name,
                description: name,
                properties: no_properties,
                required: &[],
                output_schema: None,
                read_only: false,
                destructive: false,
            })
            .collect()
    }

    async fn call(
        &self,
        _caller: &ThreadId,
        name: &str,
        _arguments: Value,
    ) -> Result<Vec<Content>, NeverFails> {
        if name == "wait_for_agents" {
            self.waiting.notify_one();
            self.run_ended.notified().await;
        }
        self.created.fetch_add(1, Ordering::SeqCst);
        Ok(vec![Content::Text("created".to_owned())])
    }
}

fn tool_call(id: u64, name: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":{}}})
}

#[tokio::test]
async fn a_batch_admitted_before_its_run_ended_does_nothing_once_the_run_ends() {
    for stopping_the_run in [false, true] {
        let set = Gated::default();
        let caller = GatewayCaller {
            thread: ThreadId::new(),
            ended: CancellationToken::new(),
            acting: Acting::Run {
                stopped: CancellationToken::new(),
            },
        };
        let handled = tokio::spawn({
            let (set, caller) = (set.clone(), caller.clone());
            async move {
                server::handle(
                    &set,
                    &caller,
                    json!([
                        tool_call(1, "wait_for_agents"),
                        tool_call(2, "create_thread")
                    ]),
                )
                .await
            }
        });
        set.waiting.notified().await;

        match (stopping_the_run, &caller.acting) {
            (true, Acting::Run { stopped }) => stopped.cancel(),
            (false, _) | (true, Acting::Between) => caller.ended.cancel(),
        }
        set.run_ended.notify_one();
        let reply = tokio::time::timeout(Duration::from_secs(10), handled)
            .await
            .expect("the batch kept running after its run ended")
            .unwrap()
            .unwrap();

        assert_eq!(
            set.created.load(Ordering::SeqCst),
            0,
            "a call after the run ended still acted for its thread (stopped: {stopping_the_run})"
        );
        let ended = |id: u64| json!({"jsonrpc":"2.0","id":id,"error":{"code":-32001,"message":"This MCP address is no longer valid: the run it was issued to has ended."}});
        assert_eq!(
            reply,
            json!([ended(1), ended(2)]),
            "stopped: {stopping_the_run}"
        );
    }
}

#[tokio::test]
async fn a_batch_runs_every_call_while_its_run_lasts() {
    let set = Gated::default();
    let caller = GatewayCaller {
        thread: ThreadId::new(),
        ended: CancellationToken::new(),
        acting: Acting::Run {
            stopped: CancellationToken::new(),
        },
    };

    let reply = server::handle(
        &set,
        &caller,
        json!([tool_call(1, "create_thread"), tool_call(2, "create_thread")]),
    )
    .await
    .unwrap();

    assert_eq!(set.created.load(Ordering::SeqCst), 2, "{reply}");
}

#[tokio::test]
async fn between_runs_the_address_lists_its_tools_but_refuses_to_call_them() {
    let set = Gated::default();
    let thread = ThreadId::new();
    let caller = GatewayCaller {
        thread: thread.clone(),
        ended: CancellationToken::new(),
        acting: Acting::Between,
    };

    let reply = server::handle(
        &set,
        &caller,
        json!([
            {"jsonrpc":"2.0","id":1,"method":"initialize","params":{}},
            {"jsonrpc":"2.0","id":2,"method":"tools/list"},
            {"jsonrpc":"2.0","id":3,"method":"ping"},
            tool_call(4, "create_thread"),
        ]),
    )
    .await
    .unwrap();

    assert_eq!(set.created.load(Ordering::SeqCst), 0, "{reply}");
    for answered in 0..3 {
        assert!(reply[answered].get("result").is_some(), "{reply}");
    }
    assert_eq!(
        reply[3],
        json!({"jsonrpc":"2.0","id":4,"error":{"code":-32001,"message":format!("Artisan refused this call: thread {thread} has no live run, so its Artisan tools are unavailable until its next turn.")}})
    );
}
