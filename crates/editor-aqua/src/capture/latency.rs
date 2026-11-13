use anyhow::Result;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::Instant;

pub async fn proxy(target: SocketAddr, delay: Duration) -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            tokio::spawn(relay(client, target, delay));
        }
    });
    Ok(address)
}

async fn relay(client: TcpStream, target: SocketAddr, delay: Duration) {
    let Ok(server) = TcpStream::connect(target).await else {
        return;
    };
    let (client_read, client_write) = client.into_split();
    let (server_read, server_write) = server.into_split();
    tokio::join!(
        pipe(client_read, server_write, delay),
        pipe(server_read, client_write, delay)
    );
}

async fn pipe(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, delay: Duration) {
    let (sender, mut receiver) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    let read = async move {
        let mut buffer = vec![0; 64 * 1024];
        while let Ok(count) = from.read(&mut buffer).await
            && count > 0
            && sender
                .send((Instant::now() + delay, buffer[..count].to_vec()))
                .is_ok()
        {}
    };
    let write = async move {
        while let Some((due, chunk)) = receiver.recv().await {
            tokio::time::sleep_until(due).await;
            if to.write_all(&chunk).await.is_err() {
                return;
            }
        }
        let _ = to.shutdown().await;
    };
    tokio::join!(read, write);
}
