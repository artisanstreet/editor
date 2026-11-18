use super::*;
use axum::{
    Router,
    body::{Body, Bytes},
    routing::get,
};
use std::time::Duration;
use test_support::{http::StubServer, wait::within};

#[tokio::test]
async fn an_endless_archive_stops_at_the_limit() {
    let stub = StubServer::serve(Router::new().route(
        "/archive",
        get(|| async {
            Body::from_stream(futures::stream::repeat_with(|| {
                Ok::<_, std::io::Error>(Bytes::from(vec![0_u8; 64 * 1024]))
            }))
        }),
    ))
    .await;
    let response = reqwest::get(format!("{}/archive", stub.url()))
        .await
        .expect("reach the stub");
    let downloaded = within(
        "an endless archive",
        Duration::from_secs(10),
        download(response, 1024 * 1024, "owner/repo"),
    )
    .await;
    let error = downloaded.expect_err("an endless archive is refused");
    assert!(
        matches!(&error, FetchSkillError::TooLarge { repository } if repository == "owner/repo"),
        "{error}"
    );
}
