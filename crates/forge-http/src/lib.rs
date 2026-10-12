//! Forge's network API: HTTPS (HTTP/2) with mutual TLS, JSON routes and one
//! server-sent event stream. Nothing is tied to a connection; a client's
//! only sync state is the last seq it saw.

mod events;
mod extensions;
mod links;
mod repository;
mod routes;
pub mod tls;

use anyhow::{Context, Result};
use artisan_api::EngineId;
use artisan_api::Timestamp;
use forge_engines::Engines;
use forge_runs::Hub;
use forge_store::Store;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tower::Service;

pub use tls::Identity;

/// Default port for `ae forge serve`.
pub const DEFAULT_PORT: u16 = 47_900;
const IDENTITY_KEY: &str = "forge_identity";
const PAIR_CODE_LIFETIME_MS: Timestamp = 10 * 60 * 1000;

#[derive(Clone, Debug)]
pub struct ServeConfig {
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
}

/// What a new Editor needs to pair: Forge's certificate (to pin) and a
/// one-time code (to prove the user allowed it).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairInvite {
    pub code: String,
    pub forge_certificate: String,
    pub expires_at: Timestamp,
}

/// Opens the store and engines under `data_dir` and serves until the
/// listener fails. Holds an exclusive lock on the data directory.
pub async fn serve(config: ServeConfig) -> Result<()> {
    start(config).await?.run().await
}

/// A bound, not yet serving Forge.
pub struct Server {
    listener: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    app: axum::Router,
    store: Store,
    links: Arc<links::LinkTitles>,
    _lock: std::fs::File,
}

pub async fn start(config: ServeConfig) -> Result<Server> {
    start_with_harness(config, std::sync::Arc::new(forge_engines::harness)).await
}

/// [`start`] with the engine adapters swapped, so clients can be tested
/// against a scripted engine.
pub async fn start_with_harness(
    config: ServeConfig,
    harness_for: forge_runs::HarnessFor,
) -> Result<Server> {
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("create {}", config.data_dir.display()))?;
    let lock = lock_data_dir(&config.data_dir)?;
    let store = Store::open(&config.data_dir).context("open the store")?;
    let engines = Engines::new(&config.data_dir);
    let hub = Hub::with_harness(store.clone(), engines, config.data_dir.clone(), harness_for);
    hub.resume_pending()
        .await
        .context("resume undelivered messages")?;
    let engines = hub.engines().clone();
    tokio::spawn(async move {
        for engine in EngineId::ALL {
            engines.refresh(engine).await;
        }
    });
    let identity = identity(&store).await?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls::server_config(&identity)?));
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("listen on {}", config.listen))?;
    tracing::info!(address = %listener.local_addr()?, data = %config.data_dir.display(), "Forge is serving");
    let links = Arc::new(links::LinkTitles::new()?);
    let app = routes::router(routes::App {
        hub,
        forge_version: env!("CARGO_PKG_VERSION").to_owned(),
        links: Arc::clone(&links),
    });
    Ok(Server {
        listener,
        acceptor,
        app,
        store,
        links,
        _lock: lock,
    })
}

impl Server {
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Lets link titles come from loopback and private addresses, which
    /// Forge otherwise refuses. For tests that serve pages locally.
    pub fn reach_private_links(&self) {
        self.links.reach_private();
    }

    pub async fn run(self) -> Result<()> {
        loop {
            let (socket, peer) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!("accept failed: {error}");
                    continue;
                }
            };
            let acceptor = self.acceptor.clone();
            let app = self.app.clone();
            tokio::spawn(async move {
                let stream = match acceptor.accept(socket).await {
                    Ok(stream) => stream,
                    Err(error) => {
                        tracing::debug!(%peer, "TLS handshake failed: {error}");
                        return;
                    }
                };
                let Some(certificate) = stream
                    .get_ref()
                    .1
                    .peer_certificates()
                    .and_then(|certs| certs.first())
                else {
                    return;
                };
                let client = routes::Client {
                    certificate: tls::certificate_id(certificate),
                };
                let service = hyper::service::service_fn(
                    move |mut request: hyper::Request<hyper::body::Incoming>| {
                        request.extensions_mut().insert(client.clone());
                        app.clone().call(request)
                    },
                );
                if let Err(error) =
                    hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                        .serve_connection_with_upgrades(TokioIo::new(stream), service)
                        .await
                {
                    tracing::debug!(%peer, "connection ended: {error}");
                }
            });
        }
    }
}

/// Forge's certificate, made on first use and kept in the store.
pub async fn identity(store: &Store) -> Result<Identity> {
    let fresh = serde_json::to_string(&Identity::generate("artisan-forge")?)?;
    let stored = store.secret_or_insert(IDENTITY_KEY, fresh).await?;
    serde_json::from_str(&stored).context("read Forge's identity")
}

/// Makes a one-time pairing code, valid for ten minutes.
pub async fn invite(store: &Store) -> Result<PairInvite> {
    let identity = identity(store).await?;
    let (code, expires_at) = new_code()?;
    store.add_pair_code(hash_code(&code), expires_at).await?;
    Ok(PairInvite {
        code,
        forge_certificate: identity.certificate,
        expires_at,
    })
}

/// [`invite`] without opening the store, so it works while Forge runs on
/// `data_dir` (opening a `Store` would recover, i.e. cancel, its live runs).
pub fn invite_offline(data_dir: &Path) -> Result<PairInvite> {
    let mut admin = forge_store::admin::Admin::open(data_dir)?;
    let fresh = serde_json::to_string(&Identity::generate("artisan-forge")?)?;
    let identity: Identity = serde_json::from_str(&admin.secret_or_insert(IDENTITY_KEY, &fresh)?)
        .context("read Forge's identity")?;
    let (code, expires_at) = new_code()?;
    admin.add_pair_code(&hash_code(&code), expires_at)?;
    Ok(PairInvite {
        code,
        forge_certificate: identity.certificate,
        expires_at,
    })
}

fn new_code() -> Result<(String, Timestamp)> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("random code: {error}"))?;
    // 32 symbols, so `% 32` is unbiased; no 0/O or 1/I to misread.
    let alphabet = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let code = bytes
        .iter()
        .map(|byte| alphabet[usize::from(byte % 32)] as char)
        .collect();
    Ok((code, artisan_api::now() + PAIR_CODE_LIFETIME_MS))
}

pub(crate) fn hash_code(code: &str) -> String {
    let normalised: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    Sha256::digest(normalised.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// One Forge per data directory; anything else that opens the `Store`
/// (like the legacy import) takes the same lock.
pub fn lock_data_dir(dir: &Path) -> Result<std::fs::File> {
    use fs2::FileExt;
    let path = dir.join("forge.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.try_lock_exclusive()
        .with_context(|| format!("another Forge is using {}", dir.display()))?;
    Ok(file)
}

#[cfg(test)]
mod tests;
