use editor_core::mcp_sign_in::{CallbackError, wait_for_code};
use editor_core::{EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use std::time::Duration;
use test_support::wait::{wait_for, within};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot::error::TryRecvError;
use tokio_util::sync::CancellationToken;

const LIMIT: Duration = Duration::from_secs(10);
const ABANDON_LIMIT: Duration = Duration::from_secs(5);
const STATE: &str = "st&te/1";
const ENCODED_STATE: &str = "st%26te%2F1";

async fn listening() -> (TcpListener, u16) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind a loopback callback");
    let port = listener.local_addr().expect("callback address").port();
    (listener, port)
}

async fn send(port: u16, request: &[u8]) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("reach the callback");
    stream.write_all(request).await.expect("send the request");
    stream
}

async fn answer(port: u16, request: &str) -> String {
    let mut stream = send(port, request.as_bytes()).await;
    let mut response = String::new();
    within(
        "the callback's answer",
        LIMIT,
        stream.read_to_string(&mut response),
    )
    .await
    .expect("read the answer");
    response
}

#[tokio::test]
async fn only_this_attempts_callback_brings_back_a_code() {
    let (listener, port) = listening().await;
    let waiting = tokio::spawn(wait_for_code(
        listener,
        STATE.to_owned(),
        CancellationToken::new(),
    ));
    for request in [
        "GET /favicon.ico HTTP/1.1\r\n\r\n".to_owned(),
        "GET /callback?code=stolen HTTP/1.1\r\n\r\n".to_owned(),
        "GET /callback?state=earlier-attempt&code=stolen HTTP/1.1\r\n\r\n".to_owned(),
        "GET /callback?state=st&te/1&code=stolen HTTP/1.1\r\n\r\n".to_owned(),
        format!("GET /callback?state={ENCODED_STATE}&code= HTTP/1.1\r\n\r\n"),
        format!("GET /elsewhere?state={ENCODED_STATE}&code=stolen HTTP/1.1\r\n\r\n"),
        format!("POST /callback?state={ENCODED_STATE}&code=stolen HTTP/1.1\r\n\r\n"),
        "GET /callback?state=earlier-attempt&error=access_denied HTTP/1.1\r\n\r\n".to_owned(),
        "\u{1}\u{2} not http\r\n\r\n".to_owned(),
    ] {
        let response = answer(port, &request).await;
        assert!(
            response.starts_with("HTTP/1.1 404"),
            "{request:?} was answered with {response:?}"
        );
        assert!(!waiting.is_finished(), "{request:?} ended the wait");
    }
    let mut oversized = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("reach the callback");
    if let Err(error) = oversized.write_all(&vec![b'a'; 32 * 1024]).await {
        assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
            ),
            "sending an oversized request failed with {error}"
        );
    }
    drop(oversized);
    let _browser = send(
        port,
        format!(
            "GET /callback?state={ENCODED_STATE}&code=a%2Fb%20c HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        )
        .as_bytes(),
    )
    .await;
    let (code, _stream) = within("the code", LIMIT, waiting)
        .await
        .expect("the wait runs to the end")
        .expect("the wait succeeds")
        .expect("the wait was not cancelled");
    assert_eq!(code, "a/b c");
}

#[tokio::test]
async fn a_denied_sign_in_shows_the_oauth_code_and_never_the_providers_description() {
    let (listener, port) = listening().await;
    let waiting = tokio::spawn(wait_for_code(
        listener,
        STATE.to_owned(),
        CancellationToken::new(),
    ));
    let response = answer(
        port,
        &format!(
            "GET /callback?state={ENCODED_STATE}&error=access_denied&error_description=token%20sk-secret%20rejected HTTP/1.1\r\n\r\n"
        ),
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response:?}");
    assert!(response.contains("access_denied"), "{response:?}");
    assert!(
        !response.contains("sk-secret"),
        "the browser page doesn't repeat the provider's description: {response:?}"
    );
    let ended = within("the refusal", LIMIT, waiting)
        .await
        .expect("the wait runs to the end");
    let Err(denied @ CallbackError::Denied { .. }) = ended else {
        panic!("a refusal ended as {ended:?}");
    };
    assert_eq!(
        denied.to_string(),
        "The sign-in was refused (access_denied).",
        "the Editor shows the code, not the provider's description"
    );
}

async fn refused_with(error: &str) -> (String, String) {
    let (listener, port) = listening().await;
    let waiting = tokio::spawn(wait_for_code(
        listener,
        STATE.to_owned(),
        CancellationToken::new(),
    ));
    let response = answer(
        port,
        &format!("GET /callback?state={ENCODED_STATE}&error={error} HTTP/1.1\r\n\r\n"),
    )
    .await;
    let ended = within("the refusal", LIMIT, waiting)
        .await
        .expect("the wait runs to the end");
    let Err(denied @ CallbackError::Denied { .. }) = ended else {
        panic!("a refusal ended as {ended:?}");
    };
    (response, denied.to_string())
}

#[tokio::test]
async fn a_refusal_with_a_code_artisan_doesnt_know_still_shows_the_code() {
    let (page, told) = refused_with("login_required").await;
    assert!(page.contains("login_required"), "{page:?}");
    assert_eq!(told, "The sign-in was refused (login_required).");
}

#[tokio::test]
async fn a_refusal_code_that_isnt_a_plain_word_is_never_repeated() {
    let (page, told) = refused_with("%3Cimg%20src%3Dx%3E").await;
    assert!(!page.contains("img"), "{page:?}");
    assert_eq!(
        told,
        "The sign-in was refused (an OAuth error code Artisan can't show)."
    );
}

#[tokio::test]
async fn cancelling_stops_waiting_for_the_browser() {
    let (listener, port) = listening().await;
    let cancel = CancellationToken::new();
    let waiting = tokio::spawn(wait_for_code(listener, STATE.to_owned(), cancel.clone()));
    cancel.cancel();
    let ended = within("the cancelled wait", LIMIT, waiting)
        .await
        .expect("the wait runs to the end");
    assert!(
        matches!(ended, Ok(None)),
        "a cancelled wait ended as {ended:?}"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", port)).await.is_err(),
        "the callback still listens after cancelling"
    );
}

#[test]
fn removing_a_host_abandons_its_sign_ins() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let identity = files.identity().expect("identity");
    let silent_forge = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind a silent Forge");
    let config = HostConfig {
        id: HostId("forge".to_owned()),
        name: "Forge".to_owned(),
        address: silent_forge
            .local_addr()
            .expect("Forge address")
            .to_string(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client =
        || ForgeClient::new(&config.address, &config.fingerprint, &identity).expect("client");
    let mut core = test_support::editor::core::test_core(&files.dir);
    test_support::editor::core::connect_client(&mut core, &config, client());
    let mut started = core.start_mcp_sign_in(&config.id, "linear", false);
    assert!(core.mcp_sign_in(&config.id, "linear").is_some());

    core.remove_host(&config.id).expect("remove the host");
    wait_for(
        "the sign-in to be abandoned",
        ABANDON_LIMIT,
        || match started.try_recv() {
            Err(TryRecvError::Closed) => Some(()),
            Err(TryRecvError::Empty) => None,
            Ok(answer) => panic!("the removed host's sign-in answered {answer:?}"),
        },
    );

    test_support::editor::core::connect_client(&mut core, &config, client());
    assert!(
        core.mcp_sign_in(&config.id, "linear").is_none(),
        "a new host inherited the removed host's sign-in"
    );
    drop(silent_forge);
}
