#![cfg(test)]

use artisan_account::browser::{
    Arrival, Handoff, Problem, Provider, arrival, bind, done_location, receive, request_target,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const RFC_7636_VERIFIER: [u8; 32] = [
    116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37, 77,
    105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121,
];

fn handoff() -> Handoff {
    Handoff::from_secrets(RFC_7636_VERIFIER, [7; 32])
}

#[test]
fn the_challenge_is_the_pkce_s256_of_the_verifier() {
    let handoff = handoff();
    assert_eq!(handoff.verifier(), "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    assert_eq!(handoff.challenge(), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    assert_eq!(handoff.state().len(), 43);
}

#[test]
fn the_browser_starts_on_the_desktop_page_without_the_verifier() {
    let handoff = handoff();
    let url = handoff.start_url("https://auth.artisan.st/", Provider::GitHub, 51234);
    assert_eq!(
        url,
        format!(
            "https://auth.artisan.st/desktop?provider=github&port=51234&challenge={}&state={}",
            handoff.challenge(),
            handoff.state()
        )
    );
    assert!(!url.contains(handoff.verifier()));
}

#[test]
fn only_callbacks_carrying_our_state_are_answers() {
    let state = handoff().state().to_owned();
    assert_eq!(
        arrival(&format!("/callback?code=abc&state={state}"), &state),
        Arrival::Code("abc".to_owned())
    );
    assert_eq!(
        arrival(&format!("/callback?state={state}&error=email-taken"), &state),
        Arrival::Problem(Problem::EmailTaken)
    );
    assert_eq!(
        arrival(&format!("/callback?state={state}&error=whatever"), &state),
        Arrival::Problem(Problem::Unfinished)
    );
    assert_eq!(
        arrival(&format!("/callback?state={state}&code="), &state),
        Arrival::Problem(Problem::Unfinished)
    );
    assert_eq!(arrival("/callback?code=abc&state=forged", &state), Arrival::Unrelated);
    assert_eq!(arrival("/favicon.ico", &state), Arrival::Unrelated);
}

#[test]
fn the_browser_is_sent_back_to_a_done_page_that_names_the_problem() {
    let origin = "https://auth.artisan.st";
    assert_eq!(
        done_location(origin, &Arrival::Code("abc".to_owned())),
        "https://auth.artisan.st/desktop/done"
    );
    assert_eq!(
        done_location(origin, &Arrival::Problem(Problem::Denied)),
        "https://auth.artisan.st/desktop/done?problem=denied"
    );
    assert_eq!(request_target("GET /callback?x=1 HTTP/1.1\r\nHost: a\r\n\r\n"), Some("/callback?x=1"));
    assert_eq!(request_target("POST /callback HTTP/1.1\r\n\r\n"), None);
}

async fn get(port: u16, target: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream
        .write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn the_loopback_ignores_strays_and_answers_the_real_callback() {
    let handoff = handoff();
    let (listener, port) = bind().unwrap();
    let state = handoff.state().to_owned();
    let waiting = tokio::spawn(async move { receive(listener, &state, "https://auth.artisan.st").await });
    let stray = get(port, "/favicon.ico").await;
    assert!(stray.starts_with("HTTP/1.1 404"));
    let forged = get(port, "/callback?code=stolen&state=forged").await;
    assert!(forged.starts_with("HTTP/1.1 404"));
    let answer = get(port, &format!("/callback?code=granted&state={}", handoff.state())).await;
    assert!(answer.starts_with("HTTP/1.1 303"));
    assert!(answer.contains("Location: https://auth.artisan.st/desktop/done\r\n"));
    assert_eq!(waiting.await.unwrap().unwrap(), Arrival::Code("granted".to_owned()));
}
