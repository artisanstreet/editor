use super::*;
use crate::{Launch, UserInput};
use artisan_api::{EngineConfig, EngineId, RunStatus, ThreadId};
use std::os::unix::fs::PermissionsExt as _;
use tokio_util::sync::CancellationToken;

const SERVER: &str = r#"#!/usr/bin/env python3
import http.server,json,os,pathlib,threading,time
root=pathlib.Path.cwd()
(root/'pid').write_text(str(os.getpid()))
stopped=threading.Event()
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self,*args): pass
    def reply(self,value):
        data=json.dumps(value).encode()
        self.send_response(200)
        self.send_header('Content-Length',str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        if self.path=='/api/event':
            self.send_response(200)
            self.send_header('Content-Type','text/event-stream')
            self.end_headers()
            self.wfile.write(b'data: {"type":"server.connected"}\n\n')
            self.wfile.flush()
            stopped.wait(30)
        else: self.reply([])
    def do_POST(self):
        self.rfile.read(int(self.headers.get('Content-Length',0)))
        if self.path=='/api/session': self.reply({'data':{'id':'root'}})
        elif self.path=='/api/rpc/opencode.goal/get': self.reply({'output':None})
        elif self.path=='/api/session/root/prompt':
            (root/'prompt').touch()
            if os.environ['HANG_PROMPT']=='true': stopped.wait(30)
            self.reply({'data':{'id':'prompt'}})
        elif self.path=='/api/session/root/interrupt':
            time.sleep(0.03)
            (root/'interrupted').touch()
            self.reply(None)
            stopped.set()
        else: raise AssertionError(self.path)
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
print('opencode server listening on http://127.0.0.1:'+str(server.server_port),flush=True)
server.serve_forever()
"#;

#[tokio::test]
async fn cancellation_interrupts_the_session_and_reaps_the_server_even_during_a_prompt() {
    for hang_prompt in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("opencode");
        std::fs::write(&binary, SERVER).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let turn = Turn {
            thread_id: ThreadId::new(),
            engine: EngineId::OpenCode,
            binary,
            home: dir.path().join("home"),
            cwd: dir.path().to_owned(),
            config: EngineConfig::new(EngineId::OpenCode),
            session_id: None,
            input: UserInput {
                text: "work".to_owned(),
                ..Default::default()
            },
            launch: Launch {
                env: vec![("HANG_PROMPT".to_owned(), hang_prompt.to_string())],
                ..Default::default()
            },
            trace_dir: None,
        };
        let (_inputs, inbox) = mpsc::channel(8);
        let (events, mut received) = mpsc::channel(256);
        let cancel = CancellationToken::new();
        let running = tokio::spawn(OpenCodeHarness.run(
            turn,
            RunIo {
                inbox,
                events,
                cancel: cancel.clone(),
            },
        ));
        let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dir.path().join("prompt").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        let outcome = tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, RunStatus::Cancelled, "{:?}", outcome.error);
        assert!(dir.path().join("interrupted").exists());
        let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success(),
            "cancelled CLI process must have been reaped"
        );
        drain.await.unwrap();
    }
}
