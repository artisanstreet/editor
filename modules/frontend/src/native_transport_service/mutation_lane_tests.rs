//! Navigation is answered while a draft write is still unanswered, and one
//! scope's writes still go out one at a time, in order.
//!
//! A scripted Forge on its own thread serves one connection. It holds the
//! first draft save unanswered until a navigation request has arrived and
//! been answered, which a serial command loop can never do: the navigation
//! would wait behind the save forever.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use artisan_domain::{AuthoredText, Command, ComposerDraftScope, RequestId, SaveComposerDraft};
use artisan_protocol::{ClientRequest, WireEnvelope, WireEnvelopeBody};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

use super::push_stall_tests::{
    WATCHDOG, connected_runtime, identity, refuse, thread, welcome_editor,
};
use super::{
    ComposerDraftCommand, ComposerDraftEvent, FrameFactory, NativeTransportCommand,
    NativeTransportEvent, PreviewLane, QueuedCommand, command_loop_with_delivery,
};

/// One request the scripted Forge accepted.
struct Accepted {
    send: quinn::SendStream,
    request: WireEnvelope,
}

impl Accepted {
    /// The draft text of a save, `None` for anything else.
    fn saved_text(&self) -> Option<String> {
        match &self.request.body {
            WireEnvelopeBody::Request(ClientRequest::Command(Command::SaveComposerDraft(save))) => {
                Some(save.text().as_str().to_owned())
            }
            _ => None,
        }
    }
}

async fn accept(connection: &quinn::Connection) -> Result<Accepted, String> {
    let (send, mut receive) = connection.accept_bi().await.map_err(|e| e.to_string())?;
    let request = artisan_transport::receive_envelope(&mut receive)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Accepted { send, request })
}

/// What the Forge saw, in the order it answered: `nav` or a save's text.
async fn serve(connection: &quinn::Connection) -> Result<Vec<String>, String> {
    welcome_editor(connection).await?;
    // The first save and the navigation are both in flight; the second
    // save of the same scope must not be.
    let (first, second) = (accept(connection).await?, accept(connection).await?);
    let (save, navigation) = match (first.saved_text(), second.saved_text()) {
        (Some(_), None) => (first, second),
        (None, Some(_)) => (second, first),
        _ => return Err("expected one save and one navigation request".into()),
    };
    let mut answered = Vec::new();
    refuse(navigation.send, &navigation.request, "navigation").await?;
    answered.push("nav".to_owned());
    let text = save.saved_text().unwrap_or_default();
    refuse(save.send, &save.request, "first-save").await?;
    answered.push(text);
    let next = accept(connection).await?;
    let text = next
        .saved_text()
        .ok_or("expected the scope's second save")?;
    refuse(next.send, &next.request, "second-save").await?;
    answered.push(text);
    Ok(answered)
}

fn start_forge(
    certificate: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
) -> (
    SocketAddr,
    Receiver<Result<Vec<String>, String>>,
    JoinHandle<()>,
) {
    let (address_tx, address_rx) = sync_channel(1);
    let (outcome_tx, outcome_rx) = sync_channel(1);
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("forge runtime");
        runtime.block_on(async move {
            let config = artisan_transport::server_config(vec![certificate], key).expect("config");
            let server = artisan_transport::bind_loopback_server(config).expect("bind");
            address_tx
                .send(server.local_addr().expect("address"))
                .expect("address");
            let Ok(Some(incoming)) = tokio::time::timeout(WATCHDOG, server.accept()).await else {
                let _ = outcome_tx.send(Err("no connection".into()));
                return;
            };
            let Ok(connection) = incoming.await else {
                let _ = outcome_tx.send(Err("handshake failed".into()));
                return;
            };
            let outcome = tokio::time::timeout(WATCHDOG, serve(&connection))
                .await
                .unwrap_or_else(|_| Err("the Forge's script timed out".into()));
            let _ = outcome_tx.send(outcome);
            let _ = tokio::time::timeout(WATCHDOG, connection.closed()).await;
            server.close(0u32.into(), b"done");
            server.wait_idle().await;
        });
    });
    let address = address_rx.recv_timeout(WATCHDOG).expect("forge address");
    (address, outcome_rx, thread)
}

fn save(sequence: u64, text: &str) -> QueuedCommand {
    let command = SaveComposerDraft::new(
        RequestId::mint("draft-save").expect("request id"),
        ComposerDraftScope::Thread(thread()),
        AuthoredText::parse(text.to_owned()).expect("text"),
        Vec::new(),
    )
    .expect("save");
    NativeTransportCommand::ComposerDraft(ComposerDraftCommand::Save {
        sequence,
        command: Box::new(command),
    })
    .into()
}

#[test]
fn navigation_is_answered_while_a_draft_save_is_unanswered() {
    let (certificate, key) = identity();
    let (address, outcome, forge) = start_forge(certificate.clone(), key);
    let (command_tx, mut command_rx) = tokio::sync::mpsc::channel::<QueuedCommand>(8);
    let (event_tx, event_rx) = sync_channel::<NativeTransportEvent>(64);

    // The application: records the answers in arrival order, then stops the
    // service.
    let shutdown = command_tx.clone();
    let application = std::thread::spawn(move || {
        let deadline = Instant::now() + WATCHDOG;
        let mut seen = Vec::new();
        while Instant::now() < deadline && seen.len() < 3 {
            match event_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(NativeTransportEvent::RegisteredProfilesFailed(_)) => {
                    seen.push("nav".to_owned())
                }
                Ok(NativeTransportEvent::ComposerDraft(ComposerDraftEvent::SaveFailed {
                    sequence,
                    ..
                })) => seen.push(format!("save-{sequence}")),
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = shutdown.blocking_send(NativeTransportCommand::Shutdown.into());
        while event_rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
        seen
    });

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("editor runtime");
    runtime.block_on(async {
        // Two saves of one scope, then a navigation request queued behind
        // them, exactly as a thread switch follows typing.
        for command in [
            save(1, "first"),
            save(2, "second"),
            NativeTransportCommand::ListRegisteredProfiles.into(),
        ] {
            command_tx.send(command).await.expect("queued");
        }
        let mut service = connected_runtime(address, certificate, event_tx.clone()).await;
        let mut frames = FrameFactory::new();
        let result = tokio::time::timeout(
            WATCHDOG,
            command_loop_with_delivery(
                &mut command_rx,
                &PreviewLane::default(),
                &mut service,
                &mut frames,
                &event_tx,
            ),
        )
        .await;
        if !matches!(result, Ok(Ok(()))) {
            panic!(
                "the service loop ends cleanly: {result:?}, forge: {:?}",
                outcome.recv_timeout(WATCHDOG)
            );
        }
        assert!(service.cleanup().await.is_ok(), "clean teardown");
    });
    drop(event_tx);
    let forge_outcome = outcome.recv_timeout(WATCHDOG).expect("forge outcome");
    let seen = application.join().expect("application thread");
    forge.join().expect("forge thread");
    assert_eq!(
        forge_outcome,
        Ok(vec![
            "nav".to_owned(),
            "first".to_owned(),
            "second".to_owned()
        ]),
        "the navigation is answered while the first save waits, and the \
         scope's second save follows its first"
    );
    // The navigation and the first save are answered back to back on
    // separate lanes, so their events may arrive in either order; the
    // scope's own results keep theirs.
    assert!(
        seen.iter().any(|event| event == "nav"),
        "navigation answered: {seen:?}"
    );
    let saves: Vec<_> = seen.iter().filter(|event| *event != "nav").collect();
    assert_eq!(saves, ["save-1", "save-2"]);
}
