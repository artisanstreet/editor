use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::Instant;

#[derive(Debug, thiserror::Error)]
enum RelayError {
    #[error("connect the latency proxy to {target}")]
    Connect {
        target: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("relay bytes through the latency proxy to {target}")]
    Pipe {
        target: SocketAddr,
        #[source]
        source: std::io::Error,
    },
}

pub(crate) async fn proxy(target: SocketAddr, delay: Duration) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((client, _)) => {
                    tokio::spawn(async move {
                        if let Err(error) = relay(client, target, delay).await {
                            tracing::warn!(
                                error = %diagnostics::chain(&error),
                                "a latency proxy connection failed"
                            );
                        }
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, %address, "the latency proxy stopped accepting");
                    return;
                }
            }
        }
    });
    Ok(address)
}

async fn relay(client: TcpStream, target: SocketAddr, delay: Duration) -> Result<(), RelayError> {
    let server = TcpStream::connect(target)
        .await
        .map_err(|source| RelayError::Connect { target, source })?;
    let (client_read, client_write) = client.into_split();
    let (server_read, server_write) = server.into_split();
    let (upstream, downstream) = tokio::join!(
        pipe(client_read, server_write, delay),
        pipe(server_read, client_write, delay)
    );
    upstream
        .and(downstream)
        .map_err(|source| RelayError::Pipe { target, source })
}

async fn pipe(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    delay: Duration,
) -> std::io::Result<()> {
    let (sender, mut receiver) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    let read = async move {
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let count = from.read(&mut buffer).await?;
            let chunk = buffer
                .get(..count)
                .expect("invariant: a read fills at most the whole buffer")
                .to_vec();
            if count == 0 || sender.send((Instant::now() + delay, chunk)).is_err() {
                return Ok::<(), std::io::Error>(());
            }
        }
    };
    let write = async move {
        while let Some((due, chunk)) = receiver.recv().await {
            tokio::time::sleep_until(due).await;
            to.write_all(&chunk).await?;
        }
        to.shutdown().await
    };
    let (read, write) = tokio::join!(read, write);
    read.and(write)
}
