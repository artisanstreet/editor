#![cfg(test)]

use artisan_account::network::{CoreSessionClient, SessionToken};
use std::{
    io::{BufRead as _, BufReader, Write as _},
    net::TcpListener,
};

async fn read_profile(
    details_status: u16,
    details: &str,
) -> artisan_account::network::AccountProfile {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let details = details.to_owned();
    let server = std::thread::spawn(move || {
        for (path, status, body) in [
            ("/v1/me", 200, serde_json::json!({"account_id":"account", "email":"test@example.invalid", "display_name":"Sander", "totp_enrolled":false}).to_string()),
            ("/v1/account/profile", details_status, details),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with(&format!("GET {path} ")));
            let mut authorized = false;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" { break; }
                authorized |= line.trim().eq_ignore_ascii_case("authorization: Bearer session");
            }
            assert!(authorized);
            write!(stream, "HTTP/1.1 {status} Response\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let profile = CoreSessionClient::new(&url)
        .unwrap()
        .me(&SessionToken::new("session".into()))
        .await
        .unwrap();
    server.join().unwrap();
    profile
}

#[tokio::test]
async fn account_reads_the_username_from_the_profile_endpoint() {
    let profile = read_profile(
        200,
        r#"{"username":"ss","display_name":"Sander","avatar_url":null,"phone":null}"#,
    )
    .await;
    assert_eq!(profile.username.as_deref(), Some("ss"));
    let cached = serde_json::to_value(&profile).unwrap();
    assert_eq!(cached["username"], "ss");
}

#[tokio::test]
async fn an_unavailable_profile_does_not_discard_a_valid_account() {
    let profile = read_profile(404, "{}").await;
    assert_eq!(profile.account_id, "account");
    assert_eq!(profile.username, None);
}
