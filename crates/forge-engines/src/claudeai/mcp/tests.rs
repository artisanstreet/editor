use std::io::{BufRead as _, Write as _};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use super::*;

/// The bytes the fake server's `image` tool returns.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake screenshot";

/// The stdio server [`fake_stdio`] starts: this test, run in a child
/// process with `ARTISAN_FAKE_MCP` set. Elsewhere it passes at once.
///
/// - `FAKE_TOOLS`: the tools it lists (`echo` by default).
/// - `FAKE_TAG`: what `echo` puts before its text.
/// - `FAKE_DELAY_MS`: how long `initialize` takes.
/// - `FAKE_LOG`: a file it appends `start` and `cancelled <id>` to.
/// - `FAKE_EXIT`: it writes this to stderr and exits 1 instead.
/// - `FAKE_BIG`: how large the line `big` answers with is.
///
/// Tools: `echo` (its `text`), `image` (a PNG), `slow` (progress every
/// 100 ms, the answer after 3 s), `ping` (pings the client with the call's
/// own id first) and `big`.
#[test]
#[expect(
    clippy::too_many_lines,
    clippy::significant_drop_tightening,
    reason = "the whole fake server, one tool per arm, reading stdin to the end"
)]
fn fake_stdio_server() {
    if std::env::var_os("ARTISAN_FAKE_MCP").is_none() {
        return;
    }
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let log = |line: &str| {
        let path = var("FAKE_LOG");
        if !path.is_empty() {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("the log");
            writeln!(file, "{line}").expect("a log line");
        }
    };
    log("start");
    if !var("FAKE_EXIT").is_empty() {
        eprintln!("{}", var("FAKE_EXIT"));
        std::process::exit(1);
    }
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let say = {
        let out = Arc::clone(&out);
        move |line: &str| {
            let mut out = lock(&out);
            writeln!(out, "{line}").expect("stdout");
            out.flush().expect("stdout");
        }
    };
    let tools: Vec<String> = Some(var("FAKE_TOOLS"))
        .filter(|tools| !tools.is_empty())
        .unwrap_or_else(|| "echo".to_owned())
        .split(',')
        .map(str::to_owned)
        .collect();
    let result = |id: &Value, result: Value| {
        json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
    };
    let text = |text: &str| json!({ "content": [{ "type": "text", "text": text }] });
    let mut lines = std::io::stdin().lock().lines();
    while let Some(Ok(line)) = lines.next() {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = message["id"].clone();
        match message["method"].as_str() {
            Some("initialize") => {
                let delay = var("FAKE_DELAY_MS").parse().unwrap_or(0);
                std::thread::sleep(Duration::from_millis(delay));
                say(&result(
                    &id,
                    json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "serverInfo": { "name": "fake", "version": "1" } }),
                ));
            }
            Some("notifications/cancelled") => {
                log(&format!("cancelled {}", message["params"]["requestId"]));
            }
            Some("tools/list") => {
                let listed: Vec<Value> = tools
                    .iter()
                    .map(|name| json!({ "name": name, "inputSchema": { "type": "object" } }))
                    .collect();
                say(&result(&id, json!({ "tools": listed })));
            }
            Some("tools/call") => match message["params"]["name"].as_str() {
                Some("image") => say(&result(
                    &id,
                    json!({ "content": [
                        { "type": "text", "text": "the screen" },
                        { "type": "image", "mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(PNG) }
                    ] }),
                )),
                Some("slow") => {
                    let say = say.clone();
                    let answer = result(&id, text("slow done"));
                    let token = id.clone();
                    std::thread::spawn(move || {
                        for step in 0..30 {
                            say(&json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": { "progressToken": token, "progress": step } }).to_string());
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        say(&answer);
                    });
                }
                Some("ping") => {
                    say(&json!({ "jsonrpc": "2.0", "id": id, "method": "ping" }).to_string());
                    let reply: Value = lines
                        .next()
                        .and_then(Result::ok)
                        .and_then(|line| serde_json::from_str(&line).ok())
                        .unwrap_or_default();
                    let answered = reply["id"] == id
                        && reply.get("method").is_none()
                        && reply["result"] == json!({});
                    say(&result(
                        &id,
                        text(if answered {
                            "ping answered"
                        } else {
                            "ping mishandled"
                        }),
                    ));
                }
                Some("big") => {
                    let size = var("FAKE_BIG").parse().unwrap_or(1024);
                    say(&result(&id, text(&"x".repeat(size))));
                }
                _ => say(&result(
                    &id,
                    text(&format!(
                        "{}{}",
                        var("FAKE_TAG"),
                        message["params"]["arguments"]["text"]
                            .as_str()
                            .unwrap_or_default()
                    )),
                )),
            },
            _ if !id.is_null() => say(&result(&id, json!({}))),
            _ => {}
        }
    }
}

fn log_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Waits up to two seconds for the log to hold `line`.
async fn logged(path: &Path, line: &str) -> bool {
    for _ in 0..40 {
        if log_lines(path)
            .iter()
            .any(|logged| logged.starts_with(line))
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[test]
fn tool_names_are_prefixed_and_sanitized() {
    assert_eq!(
        prefixed_name("docs.web", "search/all"),
        "mcp__docs_web__search_all"
    );
    assert_eq!(
        prefixed_name("artisan", "thread_read"),
        "mcp__artisan__thread_read"
    );
    assert_eq!(prefixed_name("", ""), "mcp__server__server");
}

#[test]
fn tool_names_are_unique_short_and_stable() {
    let long = "a".repeat(60);
    let tools = [
        ("a.b", "search"),
        ("a_b", "search"),
        ("x__y", "z"),
        ("x", "y__z"),
        (long.as_str(), "first"),
        (long.as_str(), "second"),
        ("docs", "search"),
    ];
    let names = tool_names(&tools);
    let grammar = regex::Regex::new("^[a-zA-Z0-9_-]{1,64}$").expect("the grammar");
    for name in &names {
        assert!(grammar.is_match(name), "{name} is not a valid tool name");
    }
    let unique: HashSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "names collide: {names:?}");
    assert_eq!(names[6], "mcp__docs__search", "a plain name stays plain");
    assert_ne!(names[0], names[1]);
    assert_ne!(names[2], names[3]);
    assert_eq!(
        names,
        tool_names(&tools),
        "the same servers get the same names"
    );
}

#[tokio::test]
async fn clashing_names_reach_their_own_server() {
    let servers = Servers::connect(
        &[
            fake_stdio("a.b", &[("FAKE_TAG", "one:")]),
            fake_stdio("a_b", &[("FAKE_TAG", "two:")]),
        ],
        Path::new("."),
    )
    .await;
    assert!(servers.failures().is_empty(), "{:?}", servers.failures());
    let names: Vec<String> = servers
        .definitions()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(names.len(), 2);
    assert_ne!(names[0], names[1], "both servers' echo share a name");
    for name in &names {
        let (server, tool) = servers.lookup(name).expect("the tool");
        assert_eq!(tool, "echo");
        let (client, tool) = servers.find(name).expect("the tool");
        let said = servers
            .call(client, tool, &json!({ "text": "hi" }))
            .await
            .expect("the call");
        let tag = if server == "a.b" { "one:hi" } else { "two:hi" };
        assert_eq!(said, tag, "{name} reached the wrong server");
    }
}

#[test]
fn a_result_renders_text_resources_and_links_and_takes_images_out() {
    let result = json!({ "content": [
        { "type": "text", "text": "one" },
        { "type": "image", "mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(PNG) },
        { "type": "resource", "resource": { "uri": "file:///a", "mimeType": "application/pdf", "blob": "AAAA" } },
        { "type": "resource", "resource": { "uri": "file:///b", "text": "two" } },
        { "type": "resource_link", "uri": "file:///c", "name": "notes", "description": "the notes" }
    ] });
    let rendered = render(&result);
    assert_eq!(
        rendered.text,
        "one\n[image (image/png): attached to this message]\n[resource file:///a (application/pdf): binary, not shown]\n<resource uri=\"file:///b\">\ntwo\n</resource>\n[resource link: notes <file:///c> — the notes]"
    );
    assert_eq!(
        rendered.images,
        vec![Image {
            mime: "image/png".to_owned(),
            data: PNG.to_vec()
        }]
    );
}

#[tokio::test]
async fn an_image_waits_for_the_message_once() {
    let servers = Servers::connect(
        &[fake_stdio("shots", &[("FAKE_TOOLS", "image")])],
        Path::new("."),
    )
    .await;
    let (client, tool) = servers.find("mcp__shots__image").expect("the tool");
    let said = servers
        .call(client, tool, &json!({}))
        .await
        .expect("the call");
    assert!(
        said.contains("the screen") && said.contains("attached"),
        "{said}"
    );
    assert_eq!(
        servers.take_images().len(),
        1,
        "the image waits for the message"
    );
    assert!(servers.take_images().is_empty(), "an image goes once");
}

#[tokio::test]
async fn progress_does_not_extend_a_call_and_a_dropped_call_is_cancelled() {
    let dir = tempfile::tempdir().expect("a dir");
    let log = dir.path().join("log");
    let servers = Servers::connect(
        &[fake_stdio(
            "slow",
            &[("FAKE_TOOLS", "slow"), ("FAKE_LOG", &log.to_string_lossy())],
        )],
        Path::new("."),
    )
    .await;
    assert!(servers.failures().is_empty(), "{:?}", servers.failures());
    let transport = &servers.clients[0].transport;
    let started = std::time::Instant::now();
    let error = transport
        .request(
            "tools/call",
            json!({ "name": "slow", "arguments": {} }),
            Instant::now() + Duration::from_millis(600),
        )
        .await
        .expect_err("the call outlives its deadline");
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "progress kept the call open for {:?}",
        started.elapsed()
    );
    assert!(error.to_string().contains("did not answer"), "{error}");
    assert!(
        logged(&log, "cancelled").await,
        "the server was not told: {:?}",
        log_lines(&log)
    );

    // Stopping the turn drops the call, which cancels it on the server.
    let cancel = tokio_util::sync::CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        stopper.cancel();
    });
    let (client, tool) = servers.find("mcp__slow__slow").expect("the tool");
    let input = json!({});
    tokio::select! {
        _ = servers.call(client, tool, &input) => panic!("the call finished before the stop"),
        () = cancel.cancelled() => {}
    }
    for _ in 0..40 {
        if log_lines(&log)
            .iter()
            .filter(|line| line.starts_with("cancelled"))
            .count()
            == 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        log_lines(&log)
            .iter()
            .filter(|line| line.starts_with("cancelled"))
            .count(),
        2,
        "the stopped call was not cancelled: {:?}",
        log_lines(&log)
    );
}

#[tokio::test]
async fn a_server_ping_is_answered_and_not_taken_for_the_reply() {
    let servers = Servers::connect(
        &[fake_stdio("pinger", &[("FAKE_TOOLS", "ping")])],
        Path::new("."),
    )
    .await;
    let (client, tool) = servers.find("mcp__pinger__ping").expect("the tool");
    let said = servers
        .call(client, tool, &json!({}))
        .await
        .expect("the call");
    assert_eq!(said, "ping answered");
}

#[tokio::test]
async fn an_oversized_line_fails_its_call_and_the_server_carries_on() {
    let server = fake_stdio("big", &[("FAKE_TOOLS", "big,echo"), ("FAKE_BIG", "200000")]);
    let resolved = resolve(&server, &|_| None).expect("the config");
    let stream = Stream::stdio(&resolved, Path::new("."), 64 * 1024).expect("the server");
    let transport = Transport::Stream(stream);
    let deadline = || Instant::now() + Duration::from_secs(20);
    transport
        .initialize(deadline())
        .await
        .expect("the handshake");
    let started = std::time::Instant::now();
    let error = transport
        .request(
            "tools/call",
            json!({ "name": "big", "arguments": {} }),
            deadline(),
        )
        .await
        .expect_err("the answer is over the limit");
    assert!(error.to_string().contains("larger than 64 KB"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the call waited for its deadline"
    );
    let echoed = transport
        .request(
            "tools/call",
            json!({ "name": "echo", "arguments": { "text": "still here" } }),
            deadline(),
        )
        .await
        .expect("the next call");
    assert_eq!(render(&echoed).text, "still here");
    assert!(transport.alive());
}

#[test]
fn the_id_of_a_skipped_line_is_found_first_or_last() {
    let scan = |text: &str| {
        let mut scan = IdScan::default();
        for piece in text.as_bytes().chunks(3) {
            scan.feed(piece);
        }
        scan.id
    };
    assert_eq!(
        scan(r#"{"jsonrpc":"2.0","id":42,"result":{"id":7}}"#),
        Some(42)
    );
    assert_eq!(
        scan(r#"{"result":{"id":7,"t":"\"id\":9"},"id": 12}"#),
        Some(12)
    );
    assert_eq!(scan(r#"{"result":{"id":7}}"#), None);
}

#[test]
fn variables_expand_as_claude_code_expands_them() {
    let server = McpServer {
        name: "docs".to_owned(),
        command: "${BIN}/server".to_owned(),
        args: vec!["--root=${ROOT:-/srv}".to_owned(), "$HOME ${".to_owned()],
        env: BTreeMap::from([("TOKEN".to_owned(), "${TOKEN}".to_owned())]),
        ..McpServer::default()
    };
    let lookup = |name: &str| match name {
        "BIN" => Some("/opt/docs".to_owned()),
        "TOKEN" => Some("secret-token".to_owned()),
        _ => None,
    };
    let resolved = resolve(&server, &lookup).expect("the config");
    assert_eq!(resolved.command, "/opt/docs/server");
    assert_eq!(resolved.args, vec!["--root=/srv", "$HOME ${"]);
    assert_eq!(resolved.env["TOKEN"], "secret-token");
    assert_eq!(
        redact("auth failed for secret-token", &resolved.secrets),
        "auth failed for [redacted]"
    );
    let missing = resolve(
        &McpServer {
            command: "${NOPE}".to_owned(),
            ..server
        },
        &lookup,
    )
    .err();
    assert_eq!(
        missing.as_deref(),
        Some("the environment variable NOPE is not set")
    );
}

#[tokio::test]
async fn a_failed_start_tells_the_model_why_without_the_servers_output() {
    let servers = Servers::connect(
        &[fake_stdio(
            "broken",
            &[
                ("FAKE_EXIT", "invalid key sk-live-12345"),
                ("API_KEY", "sk-live-12345"),
            ],
        )],
        Path::new("."),
    )
    .await;
    assert_eq!(
        servers.failures(),
        [(
            "broken".to_owned(),
            "failed to start: exited with status 1".to_owned()
        )]
    );
}

#[tokio::test]
async fn servers_connect_at_once() {
    let started = std::time::Instant::now();
    let servers = Servers::connect(
        &[
            fake_stdio("one", &[("FAKE_DELAY_MS", "1000")]),
            fake_stdio("two", &[("FAKE_DELAY_MS", "1000")]),
        ],
        Path::new("."),
    )
    .await;
    assert!(servers.failures().is_empty(), "{:?}", servers.failures());
    assert_eq!(servers.offered(), vec![("one", 1), ("two", 1)]);
    assert!(
        started.elapsed() < Duration::from_millis(1900),
        "two one-second starts took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_thread_keeps_its_servers_between_turns() {
    let dir = tempfile::tempdir().expect("a dir");
    let log = dir.path().join("log");
    let config = vec![fake_stdio("warm", &[("FAKE_LOG", &log.to_string_lossy())])];
    let starts = || {
        log_lines(&log)
            .iter()
            .filter(|line| *line == "start")
            .count()
    };
    for _ in 0..2 {
        let servers = for_thread("thread-warm", Servers::connect(&config, Path::new("."))).await;
        let (client, tool) = servers.find("mcp__warm__echo").expect("the tool");
        servers
            .call(client, tool, &json!({ "text": "x" }))
            .await
            .expect("the call");
    }
    assert_eq!(starts(), 1, "the second turn started the server again");

    let mut changed = config.clone();
    changed[0]
        .env
        .insert("FAKE_TAG".to_owned(), "changed:".to_owned());
    drop(for_thread("thread-warm", Servers::connect(&changed, Path::new("."))).await);
    assert_eq!(starts(), 2, "a changed config reuses the old server");

    drop(for_thread("thread-other", Servers::connect(&changed, Path::new("."))).await);
    assert_eq!(starts(), 3, "another thread shares this thread's server");
}

/// A Streamable HTTP server for tests that can forget sessions and answer
/// over a stream it keeps open.
#[derive(Default)]
struct FakeHttp {
    /// Answer over SSE and keep the stream open five seconds after.
    stream: bool,
    /// Forget the session after each call.
    forget: bool,
    session: Mutex<Option<String>>,
    initialized: AtomicUsize,
    authorization: Mutex<Vec<String>>,
}

async fn serve_http(listener: tokio::net::TcpListener, fake: Arc<FakeHttp>) {
    while let Ok((mut socket, _)) = listener.accept().await {
        let fake = Arc::clone(&fake);
        tokio::spawn(async move {
            let Some((head, body)) = read_request(&mut socket).await else {
                return;
            };
            let header = |name: &str| {
                head.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.trim()
                        .eq_ignore_ascii_case(name)
                        .then(|| value.trim().to_owned())
                })
            };
            if let Some(value) = header("authorization") {
                lock(&fake.authorization).push(value);
            }
            let value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let method = value["method"].as_str().unwrap_or_default();
            if method != "initialize" && header("mcp-session-id") != *lock(&fake.session) {
                socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await
                    .ok();
                return;
            }
            if value.get("id").is_none() {
                socket
                    .write_all(
                        b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await
                    .ok();
                return;
            }
            let mut extra = String::new();
            if method == "initialize" {
                let count = fake.initialized.fetch_add(1, Ordering::SeqCst) + 1;
                let session = format!("s{count}");
                extra = format!("mcp-session-id: {session}\r\n");
                *lock(&fake.session) = Some(session);
            }
            if method == "tools/call" && fake.forget {
                *lock(&fake.session) = Some("forgotten".to_owned());
            }
            let reply = fake_reply(&value).to_string();
            if fake.stream {
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n{extra}connection: close\r\n\r\n"
                );
                socket.write_all(head.as_bytes()).await.ok();
                let ping =
                    json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} });
                socket
                    .write_all(
                        format!("data: {ping}\n\nevent: message\ndata: {reply}\n\n").as_bytes(),
                    )
                    .await
                    .ok();
                socket.flush().await.ok();
                tokio::time::sleep(Duration::from_secs(5)).await;
            } else {
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                socket.write_all(response.as_bytes()).await.ok();
            }
        });
    }
}

async fn http_server(fake: FakeHttp) -> (String, Arc<FakeHttp>, AbortOnDropHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a listener");
    let addr = listener.local_addr().expect("an address");
    let fake = Arc::new(fake);
    let task = AbortOnDropHandle::new(tokio::spawn(serve_http(listener, Arc::clone(&fake))));
    (format!("http://{addr}/mcp"), fake, task)
}

#[tokio::test]
async fn headers_are_expanded_and_sent_and_a_lost_session_starts_again() {
    let (url, fake, _task) = http_server(FakeHttp {
        forget: true,
        ..FakeHttp::default()
    })
    .await;
    let servers = Servers::connect(
        &[McpServer {
            name: "docs".to_owned(),
            url: Some(url),
            headers: BTreeMap::from([(
                "Authorization".to_owned(),
                "Bearer ${ARTISAN_TEST_NEVER_SET:-tok123}".to_owned(),
            )]),
            ..McpServer::default()
        }],
        Path::new("."),
    )
    .await;
    assert!(servers.failures().is_empty(), "{:?}", servers.failures());
    let (client, tool) = servers.find("mcp__docs__search").expect("the tool");
    for _ in 0..2 {
        assert_eq!(
            servers
                .call(client, tool, &json!({ "query": "x" }))
                .await
                .expect("the call"),
            "found it"
        );
    }
    assert_eq!(
        fake.initialized.load(Ordering::SeqCst),
        2,
        "the forgotten session was not started again"
    );
    let sent = lock(&fake.authorization).clone();
    assert!(!sent.is_empty());
    assert!(
        sent.iter().all(|value| value == "Bearer tok123"),
        "{sent:?}"
    );
}

#[tokio::test]
async fn a_streamed_answer_returns_before_the_stream_closes() {
    let (url, _fake, _task) = http_server(FakeHttp {
        stream: true,
        ..FakeHttp::default()
    })
    .await;
    let started = std::time::Instant::now();
    let servers = Servers::connect(
        &[McpServer {
            name: "docs".to_owned(),
            url: Some(url),
            ..McpServer::default()
        }],
        Path::new("."),
    )
    .await;
    assert!(servers.failures().is_empty(), "{:?}", servers.failures());
    let (client, tool) = servers.find("mcp__docs__search").expect("the tool");
    let said = servers
        .call(client, tool, &json!({}))
        .await
        .expect("the call");
    assert_eq!(said, "found it");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the client waited for the streams to close: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn an_http_server_that_refuses_is_named_with_its_status_only() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a listener");
    let addr = listener.local_addr().expect("an address");
    let _task = AbortOnDropHandle::new(tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            read_request(&mut socket).await;
            let body = "invalid token sk-live-12345";
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.ok();
        }
    }));
    let servers = Servers::connect(
        &[McpServer {
            name: "docs".to_owned(),
            url: Some(format!("http://{addr}/mcp")),
            ..McpServer::default()
        }],
        Path::new("."),
    )
    .await;
    assert_eq!(
        servers.failures(),
        [("docs".to_owned(), "initialize failed: HTTP 401".to_owned())]
    );
}

#[tokio::test]
async fn a_legacy_sse_server_is_connected_and_called() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a listener");
    let addr = listener.local_addr().expect("an address");
    let stream: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>> = Arc::default();
    let _task = AbortOnDropHandle::new(tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let stream = Arc::clone(&stream);
            tokio::spawn(async move {
                let Some((head, body)) = read_request(&mut socket).await else {
                    return;
                };
                if head.starts_with("GET") {
                    let (sender, mut events) = mpsc::unbounded_channel::<String>();
                    *lock(&stream) = Some(sender);
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\nevent: endpoint\ndata: /messages?session=1\n\n")
                        .await
                        .ok();
                    while let Some(event) = events.recv().await {
                        socket.write_all(event.as_bytes()).await.ok();
                    }
                    return;
                }
                let value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                if value.get("id").is_some() && value.get("method").is_some() {
                    let event = format!("event: message\ndata: {}\n\n", fake_reply(&value));
                    if let Some(sender) = lock(&stream).as_ref() {
                        sender.send(event).ok();
                    }
                }
                socket
                    .write_all(
                        b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await
                    .ok();
            });
        }
    }));
    let servers = Servers::connect(
        &[McpServer {
            name: "legacy".to_owned(),
            url: Some(format!("http://{addr}/sse")),
            sse: true,
            ..McpServer::default()
        }],
        Path::new("."),
    )
    .await;
    assert!(servers.failures().is_empty(), "{:?}", servers.failures());
    let (client, tool) = servers.find("mcp__legacy__search").expect("the tool");
    assert_eq!(
        servers
            .call(client, tool, &json!({}))
            .await
            .expect("the call"),
        "found it"
    );
}

#[test]
fn sse_bodies_yield_their_json_messages() {
    let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
    let mut decoder = SseDecoder::with_limit(MAX_EVENT_BYTES);
    let mut buffer = body.as_bytes();
    let Some(Ok(SseEvent::Message(message))) = decoder.next(&mut buffer) else {
        panic!("no event");
    };
    let value: Value = serde_json::from_str(&message.data).expect("JSON");
    assert_eq!(value["id"], 1);
}

#[test]
fn a_json_rpc_error_becomes_the_error() {
    let reply = json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32601, "message": "Unknown method" } });
    let error = response_result(&reply).unwrap_err();
    assert!(error.to_string().contains("Unknown method"));
}

#[test]
fn project_mcp_json_is_read_with_the_nearest_file_winning() {
    let root = tempfile::tempdir().expect("a root");
    std::fs::create_dir_all(root.path().join(".git")).expect("a repository");
    let sub = root.path().join("sub");
    std::fs::create_dir_all(&sub).expect("a subfolder");
    std::fs::write(
        root.path().join(".mcp.json"),
        json!({ "mcpServers": {
            "docs": { "command": "docs-server", "args": ["--root", "."] },
            "shared": { "command": "old" }
        } })
        .to_string(),
    )
    .expect("the root config");
    std::fs::write(
        sub.join(".mcp.json"),
        json!({ "mcpServers": {
            "shared": { "type": "http", "url": "https://mcp.example/shared", "headers": { "Authorization": "Bearer ${TOKEN}" } },
            "legacy": { "type": "sse", "url": "https://mcp.example/sse" },
            "local": { "command": "local", "env": { "TOKEN": "x" } }
        } })
        .to_string(),
    )
    .expect("the sub config");
    let servers = project_servers(&sub);
    assert_eq!(servers.len(), 4);
    let named = |name: &str| {
        servers
            .iter()
            .find(|server| server.name == name)
            .expect("the server")
    };
    let shared = named("shared");
    assert_eq!(shared.url.as_deref(), Some("https://mcp.example/shared"));
    assert_eq!(shared.headers["Authorization"], "Bearer ${TOKEN}");
    assert!(!shared.sse);
    assert!(named("legacy").sse);
    assert_eq!(
        named("local").env.get("TOKEN").map(String::as_str),
        Some("x")
    );
    assert_eq!(
        named("docs").args,
        vec!["--root".to_owned(), ".".to_owned()]
    );
}

#[tokio::test]
async fn an_http_server_is_connected_listed_and_called() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a listener");
    let addr = listener.local_addr().expect("an address");
    let server = tokio::spawn(fake_server(listener));
    let servers = Servers::connect(
        &[McpServer {
            name: "docs".to_owned(),
            url: Some(format!("http://{addr}/mcp")),
            ..McpServer::default()
        }],
        Path::new("."),
    )
    .await;
    assert!(servers.failures().is_empty(), "{:?}", servers.failures());
    let (client, tool) = servers.find("mcp__docs__search").expect("the tool");
    assert_eq!(
        servers
            .call(client, tool, &json!({ "query": "x" }))
            .await
            .expect("the call"),
        "found it"
    );
    server.abort();
}
