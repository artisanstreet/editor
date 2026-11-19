use super::McpServer;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

pub(super) async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = match socket.read(&mut buffer).await {
            Ok(0) => return None,
            Err(error) => panic!("the fake MCP server could not read its request: {error}"),
            Ok(read) => read,
        };
        data.extend_from_slice(&buffer[..read]);
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..end]).into_owned();
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::trim)
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                let body = String::from_utf8_lossy(&data[end + 4..end + 4 + length]).into_owned();
                return Some((head, body));
            }
        }
    }
}

pub(super) fn fake_reply(value: &Value) -> Value {
    let result = match value["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "serverInfo": { "name": "fake", "version": "1" }
        }),
        Some("tools/list") => json!({
            "tools": [{
                "name": "search",
                "description": "Find things",
                "inputSchema": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } }
                }
            }]
        }),
        Some("tools/call") => json!({
            "content": [{ "type": "text", "text": "found it" }],
            "isError": false
        }),
        _ => json!({}),
    };
    json!({ "jsonrpc": "2.0", "id": value["id"], "result": result })
}

pub(in crate::claudeai) async fn fake_server(listener: tokio::net::TcpListener) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            break;
        };
        let Some((_, body)) = read_request(&mut socket).await else {
            break;
        };
        let value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        if value.get("id").is_none() {
            socket
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await
                .expect("the fake MCP server answers a notification");
            continue;
        }
        let body = fake_reply(&value).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("the fake MCP server answers a call");
    }
}

pub(in crate::claudeai) fn fake_stdio(name: &str, env: &[(&str, &str)]) -> McpServer {
    let mut vars: BTreeMap<String, String> = env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    vars.insert("ARTISAN_FAKE_MCP".to_owned(), "1".to_owned());
    McpServer {
        name: name.to_owned(),
        command: std::env::current_exe()
            .expect("the test binary")
            .to_string_lossy()
            .into_owned(),
        args: [
            "--exact",
            "claudeai::mcp::tests::fake_stdio_server",
            "--nocapture",
            "--test-threads=1",
            "--quiet",
        ]
        .map(str::to_owned)
        .to_vec(),
        env: vars,
        ..McpServer::default()
    }
}
