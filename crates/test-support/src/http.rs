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

    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for StubServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
