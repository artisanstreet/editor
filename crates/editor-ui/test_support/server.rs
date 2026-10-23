//! Owned temporary Forge server for native Client tests.
pub struct Server {
    pub store: forge_store::Store,
    pub runtime: tokio::runtime::Runtime,
    pub address: String,
    pub directory: tempfile::TempDir,
}

/// A fixture owns its runtime, temporary database and paired HTTP server.
/// Dropping it releases the server and every task spawned on that runtime.
pub fn start() -> Server {
    let directory = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let server = runtime
        .block_on(forge_http::start(forge_http::ServeConfig {
            data_dir: directory.path().join("forge"),
            listen: "127.0.0.1:0".parse().unwrap(),
        }))
        .unwrap();
    let address = server.local_addr().unwrap().to_string();
    let store = server.store().clone();
    runtime.spawn(async move {
        server.run().await.ok();
    });
    Server {
        store,
        runtime,
        address,
        directory,
    }
}
