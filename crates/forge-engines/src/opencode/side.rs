//! Disposable OpenCode forks. Native compaction preserves the session's
//! agent, tools and history prefix; only the fork's model can change.
use super::*;
use crate::{SideReply, SideUsage};
use artisan_api::{ItemKind, Phase, Status};
use tokio_util::sync::CancellationToken;

const LIMIT: Duration = Duration::from_secs(180);

pub(super) async fn compact(turn: Turn, cancel: CancellationToken) -> Result<SideReply> {
    let password = password()?;
    let config_dir = write_agents(&turn.home)?;
    let mut server = spawn(&turn, &password, &config_dir)?;
    let mut api = None;
    let mut fork = None;
    let answer = tokio::select! {
        biased;
        () = cancel.cancelled() => Err(anyhow!("compaction cancelled")),
        () = tokio::time::sleep(LIMIT) => Err(anyhow!("OpenCode compaction timed out")),
        answer = async {
            let base = tokio::time::timeout(READY_TIMEOUT, ready(&mut server)).await
                .context("starting OpenCode for compaction timed out")??;
            api = Some(Api::new(base, password)?);
            let api = api.as_ref().context("no OpenCode API")?;
            let mut events = subscribe(api).await?;
            let native = turn.session_id.is_some();
            let created = if let Some(session) = &turn.session_id {
                api.post(&format!("/api/session/{session}/fork"), Some(&json!({}))).await?
            } else {
                api.post("/api/session", Some(&json!({
                    "agent": "artisan-summary",
                    "location": { "directory": turn.cwd },
                }))).await?
            };
            let session = created["data"]["id"].as_str().context("OpenCode fork has no id")?.to_owned();
            fork = Some(session.clone());
            if let Some(model) = model_ref(turn.config.model.as_deref(), turn.config.variant.as_deref()) {
                api.post(&format!("/api/session/{session}/model"), Some(&json!({ "model": model }))).await?;
            }
            let mut decoder = Decoder::new(Some(session.clone()));
            if native {
                // OpenCode owns the prompt, validation/nudge, overflow handling
                // and accounting. No assistant tools run during this operation.
                api.post(&format!("/api/session/{session}/compact"), Some(&json!({}))).await?;
            } else {
                let message = send_prompt(api, &session, &turn.input).await?;
                decoder.prompt_sent(&message, None);
            }
            let mut reply = SideReply::default();
            loop {
                let data = events.recv().await.context("OpenCode compaction event stream closed")?;
                if native {
                    if let Some(result) = compact_event(&data, &session)? { return Ok(result); }
                } else {
                    for event in decoder.feed(&data)? {
                        match event {
                            EngineEvent::Item { status: Status::Done, kind: ItemKind::AssistantMessage { text, phase: Phase::Final }, .. } => {
                                text.inline().unwrap_or_default().clone_into(&mut reply.text);
                            }
                            EngineEvent::Usage(usage) => {
                                reply.usage = SideUsage { input: usage.input + usage.cache_read + usage.cache_write.unwrap_or(0), cached_input: usage.cache_read, output: usage.output + usage.reasoning };
                            }
                            EngineEvent::Session { .. }
                        | EngineEvent::Model { .. }
                        | EngineEvent::Title { .. }
                        | EngineEvent::Item { .. }
                        | EngineEvent::Image { .. }
                        | EngineEvent::Status { .. }
                        | EngineEvent::Live { .. }
                        | EngineEvent::Commands(_)
                        | EngineEvent::InputAccepted { .. } => {}
                        }
                    }
                    if decoder.idle() {
                        if let Some(error) = decoder.error { bail!("OpenCode compaction failed: {error}"); }
                        return Ok(reply);
                    }
                }
            }
        } => answer,
    };
    // Interrupt even after cancellation/timeout; delete only the fork, never
    // the source. Killing the per-query server is the final cleanup bound.
    if let (Some(api), Some(session)) = (api, fork) {
        tokio::time::timeout(Duration::from_secs(5), async {
            api.post(&format!("/api/session/{session}/interrupt"), None)
                .await
                .ok();
            api.delete(&format!("/api/session/{session}")).await
        })
        .await
        .ok();
    }
    server.kill().await;
    server.wait().await;
    answer
}

fn compact_event(data: &str, session: &str) -> Result<Option<SideReply>> {
    let event: Value = serde_json::from_str(data)?;
    let body = &event["data"];
    if body["sessionID"] != session {
        return Ok(None);
    }
    match event["type"].as_str() {
        Some("session.compaction.ended") => {
            let tokens = &body["tokens"];
            let count = |name: &str| tokens[name].as_u64().unwrap_or(0);
            let cached = tokens["cache"]["read"].as_u64().unwrap_or(0);
            let written = tokens["cache"]["write"].as_u64().unwrap_or(0);
            Ok(Some(SideReply {
                text: body["text"].as_str().unwrap_or_default().to_owned(),
                usage: SideUsage {
                    input: count("input") + cached + written,
                    cached_input: cached,
                    output: count("output") + count("reasoning"),
                },
            }))
        }
        Some("session.compaction.failed" | "session.execution.failed") => {
            bail!("OpenCode compaction failed: {}", body["error"])
        }
        Some("session.execution.interrupted") => bail!("OpenCode compaction interrupted"),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_forks_compaction_is_accepted_and_usage_includes_cache() {
        let data = json!({ "type": "session.compaction.ended", "data": {
            "sessionID": "fork", "text": "## Objective\n- Continue",
            "tokens": { "input": 10, "output": 20, "reasoning": 5, "cache": { "read": 100, "write": 30 } },
        }}).to_string();
        assert!(compact_event(&data, "source").unwrap().is_none());
        let result = compact_event(&data, "fork").unwrap().unwrap();
        assert_eq!(
            result.usage,
            SideUsage {
                input: 140,
                cached_input: 100,
                output: 25
            }
        );
        assert!(crate::is_compaction_summary(&result.text));
        assert!(compact_event(&json!({ "type": "session.compaction.failed", "data": { "sessionID": "fork", "error": "overflow" }}).to_string(), "fork").is_err());
    }
}

#[cfg(all(test, unix))]
mod integration_tests {
    use super::*;
    use crate::{Launch, UserInput};
    use artisan_api::{EngineConfig, EngineId, ThreadId};
    use std::os::unix::fs::PermissionsExt as _;

    /// Exercise the real adapter against a fake v2 server, checking that the
    /// source is never prompted, compacted, changed or deleted.
    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one adapter scenario with an inline fake HTTP server"
    )]
    async fn native_compaction_forks_overrides_only_the_fork_and_cleans_it_up() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("opencode");
        std::fs::write(&binary, r#"#!/usr/bin/env python3
import http.server,json,queue,pathlib,threading,os
root=pathlib.Path.cwd()
events=queue.Queue()
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self,*args): pass
    def do_GET(self):
        assert self.path=='/api/event',self.path
        self.send_response(200)
        self.send_header('Content-Type','text/event-stream')
        self.end_headers()
        self.wfile.write(b'data: {"type":"server.connected"}\n\n')
        self.wfile.flush()
        while True:
            event=events.get()
            self.wfile.write(('data: '+json.dumps(event)+'\n\n').encode())
            self.wfile.flush()
    def do_POST(self):
        length=int(self.headers.get('Content-Length',0))
        body=json.loads(self.rfile.read(length)) if length else None
        with (root/'requests').open('a') as f: f.write(json.dumps([self.path,body])+'\n')
        if self.path=='/api/session/source/fork':
            response={'data':{'id':'fork'}}
        elif self.path=='/api/session/fork/model':
            assert body=={'model':{'providerID':'opencode','id':'big-pickle'}},body
            response=None
        elif self.path=='/api/session/fork/compact':
            if os.environ.get('ARTISAN_TEST_COMPACTION_HANG'):
                (root/'hanging').write_text('waiting')
            else:
                events.put({'type':'session.compaction.ended','data':{'sessionID':'source','text':'wrong session'}})
                events.put({'type':'session.compaction.ended','data':{'sessionID':'fork','text':'## Objective\n- Keep building','tokens':{'input':3,'output':20,'reasoning':0,'cache':{'read':2000,'write':0}}}})
            response={'data':{'id':'compact-request'}}
        elif self.path=='/api/session/fork/interrupt': response=None
        else: raise AssertionError(self.path)
        payload=json.dumps(response).encode() if response else b''
        self.send_response(200)
        self.send_header('Content-Length',str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
    def do_DELETE(self):
        assert self.path=='/api/session/fork',self.path
        (root/'deleted-fork').write_text(self.path)
        self.send_response(204)
        self.end_headers()
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
print('opencode server listening on http://127.0.0.1:'+str(server.server_port),flush=True)
server.serve_forever()
"#).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = EngineConfig::new(EngineId::OpenCode);
        config.model = Some("opencode/big-pickle".to_owned());
        let turn = Turn {
            thread_id: ThreadId::new(),
            engine: EngineId::OpenCode,
            binary,
            home: dir.path().join("home"),
            cwd: dir.path().to_owned(),
            config,
            session_id: Some("source".to_owned()),
            input: UserInput {
                text: crate::compaction_prompt(),
                ..Default::default()
            },
            launch: Launch::default(),
            trace_dir: None,
        };
        let reply = tokio::time::timeout(
            Duration::from_secs(15),
            compact(turn.clone(), CancellationToken::new()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(reply.text, "## Objective\n- Keep building");
        assert_eq!(reply.usage.cached_input, 2000);
        assert!(dir.path().join("deleted-fork").exists());
        let requests = std::fs::read_to_string(dir.path().join("requests")).unwrap();
        let paths: Vec<String> = requests
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).unwrap()[0]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            paths,
            [
                "/api/session/source/fork",
                "/api/session/fork/model",
                "/api/session/fork/compact",
                "/api/session/fork/interrupt"
            ]
        );
        let mut hanging = turn;
        hanging
            .launch
            .env
            .push(("ARTISAN_TEST_COMPACTION_HANG".into(), "1".into()));
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { compact(hanging, task_cancel).await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dir.path().join("hanging").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        let error = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }
}
