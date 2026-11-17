use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

#[derive(Debug)]
pub struct ClosingListener {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ClosingListener {
    pub fn bind() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback listener");
        let address = listener
            .local_addr()
            .expect("read the loopback listener's address");
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let worker = std::thread::spawn(move || {
            for connection in listener.incoming() {
                drop(connection);
                if stop.load(Ordering::SeqCst) {
                    return;
                }
            }
        });
        Self {
            address,
            stopping,
            worker: Some(worker),
        }
    }

    pub const fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for ClosingListener {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        let wake = TcpStream::connect(self.address)
            .expect("invariant: the closing listener accepts its own wake-up connection");
        drop(wake);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .expect("invariant: the closing listener's worker exits cleanly");
        }
    }
}

#[cfg(unix)]
#[derive(Debug)]
pub struct PortReservation {
    _socket: socket2::Socket,
    address: SocketAddr,
}

#[cfg(unix)]
impl PortReservation {
    pub fn loopback() -> Self {
        use socket2::{Domain, Socket, Type};
        let socket =
            Socket::new(Domain::IPV4, Type::STREAM, None).expect("open a port reservation socket");
        socket
            .set_reuse_address(true)
            .expect("let the reserved port's owner bind beside the reservation");
        socket
            .bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())
            .expect("reserve a loopback port");
        let address = socket
            .local_addr()
            .expect("read the reserved address")
            .as_socket()
            .expect("invariant: an IPv4 reservation has a socket address");
        Self {
            _socket: socket,
            address,
        }
    }

    pub const fn address(&self) -> SocketAddr {
        self.address
    }
}
