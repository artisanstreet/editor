use crate::core::runtime::runtime;
use futures::future::BoxFuture;
use std::{sync::Arc, time::Duration};
use tokio_util::task::AbortOnDropHandle;

const MAX_BODY_BYTES: usize = 128 * 1024 * 1024;

pub struct MediaResponse {
    pub status: reqwest::StatusCode,
    pub body: Vec<u8>,
}
pub struct MediaHttpClient {
    redirects: reqwest::Client,
    direct: reqwest::Client,
}

pub fn client() -> Arc<MediaHttpClient> {
    let build = |redirects| {
        reqwest::Client::builder()
            .redirect(redirects)
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(None)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("build public media HTTP client")
    };
    Arc::new(MediaHttpClient {
        redirects: build(reqwest::redirect::Policy::limited(5)),
        direct: build(reqwest::redirect::Policy::none()),
    })
}

impl MediaHttpClient {
    pub fn get(
        &self,
        url: &str,
        follow_redirects: bool,
    ) -> BoxFuture<'static, anyhow::Result<MediaResponse>> {
        let request_client = if follow_redirects {
            &self.redirects
        } else {
            &self.direct
        }
        .clone();
        let request = request_client.get(url).build();
        Box::pin(async move {
            let request = request?;
            anyhow::ensure!(
                matches!(request.url().scheme(), "http" | "https"),
                "Media requires HTTP or HTTPS"
            );
            let task = AbortOnDropHandle::new(runtime().spawn(async move {
                let response = request_client.execute(request).await?;
                let status = response.status();
                let body = crate::core::client::read_body(response, MAX_BODY_BYTES).await?;
                Ok::<_, anyhow::Error>(MediaResponse { status, body })
            }));
            task.await?
        })
    }
}
