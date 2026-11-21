use tokio::task::JoinHandle;

#[derive(Debug)]
pub struct StubServer {
    url: String,
    task: JoinHandle<()>,
}

impl StubServer {
    pub async fn serve(router: axum::Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback stub server");
        let address = listener
            .local_addr()
            .expect("read the stub server's address");
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("stub server stopped serving");
        });
        Self {
            url: format!("http://{address}"),
            task,
        }
    }

    pub async fn json(path: &str, body: serde_json::Value) -> Self {
        let router = axum::Router::new().route(
            path,
            axum::routing::get(move || std::future::ready(axum::Json(body))),
        );
        Self::serve(router).await
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for StubServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamEnd {
    Close,
    Hang,
}

pub fn event_stream(
    data: impl IntoIterator<Item = String>,
    end: StreamEnd,
) -> axum::response::Response {
    use futures::StreamExt as _;
    let frames: Vec<Result<String, std::convert::Infallible>> = data
        .into_iter()
        .map(|data| Ok(format!("data: {data}\n\n")))
        .collect();
    let frames = futures::stream::iter(frames);
    let body = match end {
        StreamEnd::Close => axum::body::Body::from_stream(frames),
        StreamEnd::Hang => axum::body::Body::from_stream(frames.chain(futures::stream::pending())),
    };
    axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
        .body(body)
        .expect("invariant: a fixed event stream response is well formed")
}
