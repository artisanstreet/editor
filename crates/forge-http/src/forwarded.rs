use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

const LEN: usize = 19;
const V4: u8 = 4;
const V6: u8 = 6;
const DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadPeerError {
    #[error("read the address of the peer the gateway forwards")]
    Read(#[source] std::io::Error),
    #[error("the gateway did not name the peer it forwards within {DEADLINE:?}")]
    Late(#[source] tokio::time::error::Elapsed),
    #[error("the gateway named a peer of unknown address family {family}")]
    Family { family: u8 },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ForwardError {
    #[error("connect to the backend at {backend}")]
    Connect {
        backend: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("name peer {peer} to the backend at {backend}")]
    Announce {
        peer: SocketAddr,
        backend: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("forward peer {peer}'s connection to the backend at {backend}")]
    Copy {
        peer: SocketAddr,
        backend: SocketAddr,
        #[source]
        source: std::io::Error,
    },
}

impl ForwardError {
    pub(crate) const fn reached_backend(&self) -> bool {
        !matches!(self, Self::Connect { .. })
    }
}

fn encode(peer: SocketAddr) -> [u8; LEN] {
    let mut prefix = [0; LEN];
    match peer.ip().to_canonical() {
        IpAddr::V4(address) => {
            prefix[0] = V4;
            prefix[1..5].copy_from_slice(&address.octets());
        }
        IpAddr::V6(address) => {
            prefix[0] = V6;
            prefix[1..17].copy_from_slice(&address.octets());
        }
    }
    prefix[17..].copy_from_slice(&peer.port().to_be_bytes());
    prefix
}

fn decode(prefix: &[u8; LEN]) -> Result<SocketAddr, ReadPeerError> {
    let port = u16::from_be_bytes([prefix[17], prefix[18]]);
    let ip = match prefix[0] {
        V4 => IpAddr::V4(Ipv4Addr::new(prefix[1], prefix[2], prefix[3], prefix[4])),
        V6 => {
            let mut octets = [0; 16];
            octets.copy_from_slice(&prefix[1..17]);
            IpAddr::V6(Ipv6Addr::from(octets))
        }
        family => return Err(ReadPeerError::Family { family }),
    };
    Ok(SocketAddr::new(ip, port))
}

pub(crate) async fn read_peer(
    socket: &mut (impl AsyncRead + Unpin),
) -> Result<SocketAddr, ReadPeerError> {
    let mut prefix = [0; LEN];
    tokio::time::timeout(DEADLINE, socket.read_exact(&mut prefix))
        .await
        .map_err(ReadPeerError::Late)?
        .map_err(ReadPeerError::Read)?;
    decode(&prefix)
}

pub(crate) async fn forward(
    mut client: TcpStream,
    peer: SocketAddr,
    backend: SocketAddr,
) -> Result<(), ForwardError> {
    let mut upstream = TcpStream::connect(backend)
        .await
        .map_err(|source| ForwardError::Connect { backend, source })?;
    upstream
        .write_all(&encode(peer))
        .await
        .map_err(|source| ForwardError::Announce {
            peer,
            backend,
            source,
        })?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .map_err(|source| ForwardError::Copy {
            peer,
            backend,
            source,
        })?;
    Ok(())
}
