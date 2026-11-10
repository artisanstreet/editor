use std::sync::OnceLock;
use tokio::runtime::{Builder, Runtime};

#[expect(
    clippy::expect_used,
    reason = "the Editor cannot reach Forge without this runtime"
)]
pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("forge-net")
            .enable_all()
            .build()
            .expect("start the network runtime")
    })
}

pub async fn run_http<T: Send + 'static>(
    future: impl std::future::Future<Output = Result<T, artisan_account::network::NetworkError>>
    + Send
    + 'static,
) -> Result<T, artisan_account::network::NetworkError> {
    match tokio_util::task::AbortOnDropHandle::new(runtime().spawn(future)).await {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!("Core request task did not finish: {error}");
            Err(artisan_account::network::NetworkError::Transport)
        }
    }
}
