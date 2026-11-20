use artisan_api::{TUNNEL_BODY_MAX_BYTES, ThreadId};
use axum::extract::Query;
use axum::routing::get;
use editor_core::client::{CallFailure, ReadBodyError};
use editor_core::runtime::runtime;
use test_support::editor::forge::TlsServer;

#[derive(serde::Deserialize)]
struct Asked {
    path: String,
}

#[test]
fn a_file_the_relay_cannot_carry_does_not_open_over_a_direct_connection_either() {
    let router = axum::Router::new().route(
        "/threads/{id}/resource",
        get(|Query(asked): Query<Asked>| async move {
            let bytes = match asked.path.as_str() {
                "fits.bin" => TUNNEL_BODY_MAX_BYTES,
                _ => TUNNEL_BODY_MAX_BYTES + 1,
            };
            vec![b'a'; bytes]
        }),
    );
    let server = runtime().block_on(TlsServer::serve(router));
    let client = server.client();
    let thread = ThreadId::from("thread");
    let fits = runtime()
        .block_on(client.resource(&thread, "fits.bin", false, None))
        .expect("a file of exactly the Relay limit opens");
    assert_eq!(fits.len(), TUNNEL_BODY_MAX_BYTES);
    let over = runtime()
        .block_on(client.resource(&thread, "over.bin", false, None))
        .expect_err("a file one byte over the Relay limit is refused on every transport");
    assert!(
        matches!(
            *over.failure,
            CallFailure::Read(ReadBodyError::TooLarge { limit }) if limit == TUNNEL_BODY_MAX_BYTES
        ),
        "{over:?}"
    );
}
