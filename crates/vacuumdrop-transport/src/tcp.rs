//! TCP transport.
//!
//! Charter 14: TCP first. QUIC is evaluated later and only if measurement shows
//! a requirement it meets ([ADR-0003](../../docs/adr/0003-tcp-first.md)).
//!
//! # Timeouts
//!
//! Charter 65 forbids network operations that wait forever. Every blocking
//! operation here takes an explicit deadline. There is no "no timeout" path:
//! omitting a timeout is a bug, so the parameter is required rather than
//! defaulted.
//!
//! # Buffer sizing
//!
//! `receive` fills a caller-supplied buffer. The transport never allocates from
//! anything the peer said, and never holds an unbounded queue.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use vacuumdrop_core::TransportError;

use crate::{BoxFuture, Connector, Transport, TransportListener};

/// A connected TCP stream.
pub struct TcpTransport {
    stream: Arc<Mutex<TcpStream>>,
    peer: Option<SocketAddr>,
    read_timeout: Duration,
    write_timeout: Duration,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl std::fmt::Debug for TcpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpTransport")
            .field("peer", &self.peer)
            .field("read_timeout", &self.read_timeout)
            .field("write_timeout", &self.write_timeout)
            .field(
                "closed",
                &self.closed.load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish()
    }
}

impl TcpTransport {
    /// Wraps an established stream.
    ///
    /// `TCP_NODELAY` is set deliberately. Nagle's algorithm batches small writes
    /// and would make measured latency a study of the delayed-ACK timer rather
    /// than of the protocol.
    pub fn new(stream: TcpStream, read_timeout: Duration, write_timeout: Duration) -> Self {
        // A failure here only means the platform declined the option; the
        // connection still works. Not treating it as an error keeps this usable
        // on stacks that reject it.
        let _ = stream.set_nodelay(true);
        let peer = stream.peer_addr().ok();
        Self {
            stream: Arc::new(Mutex::new(stream)),
            peer,
            read_timeout,
            write_timeout,
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// The local address of the underlying socket, when still reachable.
    ///
    /// `None` once the stream has been shut down: the socket is gone, and
    /// reporting a stale address would be a lie.
    pub async fn local_addr(&self) -> Option<SocketAddr> {
        if self.closed.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        self.stream.lock().await.local_addr().ok()
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(std::sync::atomic::Ordering::Relaxed) {
            Err(TransportError::Closed)
        } else {
            Ok(())
        }
    }
}

impl Transport for TcpTransport {
    fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }

    fn send<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<usize, TransportError>> {
        Box::pin(async move {
            self.check_open()?;
            let mut stream = self.stream.lock().await;
            match tokio::time::timeout(self.write_timeout, stream.write(bytes)).await {
                Ok(result) => result.map_err(TransportError::Io),
                Err(_) => Err(TransportError::ConnectTimeout(self.write_timeout)),
            }
        })
    }

    fn receive<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, Result<usize, TransportError>> {
        Box::pin(async move {
            self.check_open()?;
            let mut stream = self.stream.lock().await;
            match tokio::time::timeout(self.read_timeout, stream.read(buf)).await {
                Ok(result) => result.map_err(TransportError::Io),
                Err(_) => Err(TransportError::ConnectTimeout(self.read_timeout)),
            }
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, Result<(), TransportError>> {
        Box::pin(async move {
            // Set the flag first so concurrent operations fail fast rather than
            // racing on a socket that is being torn down.
            self.closed
                .store(true, std::sync::atomic::Ordering::Relaxed);
            let mut stream = self.stream.lock().await;
            match stream.shutdown().await {
                Ok(()) => Ok(()),
                Err(e) => match e.kind() {
                    // Shutting down an already-closed socket is not a failure.
                    // Callers treat shutdown as idempotent, and on a peer that
                    // hung up first the error here says nothing useful.
                    std::io::ErrorKind::NotConnected => Ok(()),
                    _ => Err(TransportError::Io(e)),
                },
            }
        })
    }

    fn describe(&self) -> String {
        match self.peer {
            Some(addr) => format!("tcp://{addr}"),
            None => "tcp://unconnected".to_owned(),
        }
    }
}

/// A listening TCP socket.
#[derive(Debug)]
pub struct TcpTransportListener {
    listener: TcpListener,
    read_timeout: Duration,
    write_timeout: Duration,
    accept_timeout: Duration,
}

impl TcpTransportListener {
    /// Binds a listener.
    pub async fn bind(
        addr: SocketAddr,
        read_timeout: Duration,
        write_timeout: Duration,
        accept_timeout: Duration,
    ) -> Result<Self, TransportError> {
        let listener = TcpListener::bind(addr).await.map_err(TransportError::Io)?;
        Ok(Self {
            listener,
            read_timeout,
            write_timeout,
            accept_timeout,
        })
    }
}

impl TransportListener for TcpTransportListener {
    fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.listener.local_addr().map_err(TransportError::Io)
    }

    fn accept(&self) -> BoxFuture<'_, Result<Box<dyn Transport>, TransportError>> {
        Box::pin(async move {
            let stream =
                match tokio::time::timeout(self.accept_timeout, self.listener.accept()).await {
                    Ok(Ok((stream, _))) => stream,
                    Ok(Err(e)) => return Err(TransportError::Io(e)),
                    Err(_) => return Err(TransportError::ConnectTimeout(self.accept_timeout)),
                };
            Ok(Box::new(TcpTransport::new(
                stream,
                self.read_timeout,
                self.write_timeout,
            )) as Box<dyn Transport>)
        })
    }
}

/// Connects to TCP endpoints.
#[derive(Debug, Clone, Copy)]
pub struct TcpConnector {
    connect_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
}

impl TcpConnector {
    /// Builds a connector with explicit deadlines.
    pub fn new(connect_timeout: Duration, read_timeout: Duration, write_timeout: Duration) -> Self {
        Self {
            connect_timeout,
            read_timeout,
            write_timeout,
        }
    }
}

impl Connector for TcpConnector {
    fn connect(
        &self,
        addr: SocketAddr,
    ) -> BoxFuture<'_, Result<Box<dyn Transport>, TransportError>> {
        Box::pin(async move {
            let stream =
                match tokio::time::timeout(self.connect_timeout, TcpStream::connect(addr)).await {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => return Err(TransportError::Io(e)),
                    Err(_) => return Err(TransportError::ConnectTimeout(self.connect_timeout)),
                };
            Ok(Box::new(TcpTransport::new(
                stream,
                self.read_timeout,
                self.write_timeout,
            )) as Box<dyn Transport>)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn loopback() -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
    }

    const FAST: Duration = Duration::from_secs(5);

    async fn listener() -> TcpTransportListener {
        TcpTransportListener::bind(loopback(), FAST, FAST, FAST)
            .await
            .unwrap()
    }

    fn connector() -> TcpConnector {
        TcpConnector::new(FAST, FAST, FAST)
    }

    #[tokio::test]
    async fn send_and_receive_round_trip() {
        let listener = listener().await;
        let addr = listener.local_addr().unwrap();

        let accept = tokio::spawn(async move { listener.accept().await });
        let client = connector().connect(addr).await.unwrap();
        let server = accept.await.unwrap().unwrap();

        assert_eq!(client.send(b"hello").await.unwrap(), 5);

        let mut buf = [0u8; 5];
        let n = server.receive(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"hello");
    }

    #[tokio::test]
    async fn peer_address_is_reported() {
        let listener = listener().await;
        let listen_addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await });
        let client = connector().connect(listen_addr).await.unwrap();
        let server = accept.await.unwrap().unwrap();

        // Each side sees the other: the client dials the listener, so its peer is
        // the listening address. The accepted socket's peer is the client's
        // ephemeral port, which is necessarily different.
        assert_eq!(client.peer_addr(), Some(listen_addr));
        let server_peer = server.peer_addr().unwrap();
        assert!(server_peer.ip().is_loopback());
        assert_ne!(server_peer, listen_addr);
        assert!(client.describe().starts_with("tcp://"));
    }

    #[tokio::test]
    async fn binding_to_port_zero_reports_the_chosen_port() {
        // The reason local_addr() is separate from the bind request: tests must
        // not hard-code a port.
        let listener = listener().await;
        assert_ne!(listener.local_addr().unwrap().port(), 0);
    }

    #[tokio::test]
    async fn clean_close_reports_zero_length_read() {
        // A return of 0 is a clean end of stream. Every other condition is an
        // error, so the caller never inspects an error code to detect EOF.
        let listener = listener().await;
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await });
        let client = connector().connect(addr).await.unwrap();
        let server = accept.await.unwrap().unwrap();

        client.shutdown().await.unwrap();
        assert_eq!(server.receive(&mut [0u8; 16]).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn shutdown_is_idempotent() {
        let listener = listener().await;
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await });
        let client = connector().connect(addr).await.unwrap();
        let _server = accept.await.unwrap().unwrap();

        client.shutdown().await.unwrap();
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn operations_after_shutdown_are_refused() {
        let listener = listener().await;
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await });
        let client = connector().connect(addr).await.unwrap();
        let _server = accept.await.unwrap().unwrap();

        client.shutdown().await.unwrap();

        assert!(matches!(
            client.send(b"x").await,
            Err(TransportError::Closed)
        ));
        assert!(matches!(
            client.receive(&mut [0u8; 4]).await,
            Err(TransportError::Closed)
        ));
    }

    #[tokio::test]
    async fn connect_to_closed_port_fails_rather_than_hanging() {
        // Bind then drop, so the port is very likely free.
        let addr = {
            let l = listener().await;
            l.local_addr().unwrap()
        };
        let r = connector().connect(addr).await;
        assert!(r.is_err(), "connecting to a closed port succeeded");
    }

    #[tokio::test]
    async fn connect_timeout_is_enforced() {
        // A very short deadline against a non-routable address. Proves the
        // timeout path is real rather than decorative (Charter 65).
        let unreachable: SocketAddr =
            SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 9);
        let c = TcpConnector::new(Duration::from_millis(200), FAST, FAST);
        // Only the error cases are matched: a successful connect here would mean the
        // timeout did not fire, which the panic below reports.
        match c.connect(unreachable).await {
            Err(TransportError::ConnectTimeout(_)) => {}
            Err(TransportError::Io(_)) => {} // refused immediately; also valid
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("connected to a non-routable address"),
        }
    }

    #[tokio::test]
    async fn accept_timeout_is_enforced() {
        // Nothing will connect, so accept must time out rather than wait forever.
        let listener =
            TcpTransportListener::bind(loopback(), FAST, FAST, Duration::from_millis(200))
                .await
                .unwrap();
        assert!(matches!(
            listener.accept().await,
            Err(TransportError::ConnectTimeout(_))
        ));
    }

    #[tokio::test]
    async fn large_payload_moves_in_bounded_pieces() {
        // The transport fills a caller-supplied buffer and never allocates from
        // a peer-declared size. A 1 MiB payload through a 64 KiB buffer must
        // arrive intact across many iterations.
        let listener = listener().await;
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await });
        let client = connector().connect(addr).await.unwrap();
        let server = accept.await.unwrap().unwrap();

        // Shared rather than moved into the sender task, so the expected bytes are
        // still available for comparison afterwards.
        let payload = std::sync::Arc::new(
            (0..1024 * 1024)
                .map(|i| (i % 251) as u8)
                .collect::<Vec<u8>>(),
        );
        let expected = payload.len();

        let sender = tokio::spawn({
            let payload = payload.clone();
            async move {
                let mut written = 0usize;
                while written < payload.len() {
                    written += client.send(&payload[written..]).await.unwrap();
                }
                client.shutdown().await.unwrap();
            }
        });

        let mut received = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = server.receive(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
        }
        sender.await.unwrap();

        assert_eq!(received.len(), expected);
        assert_eq!(received, payload.as_slice());
    }

    #[tokio::test]
    async fn transport_works_behind_a_trait_object() {
        // The engine must hold "whichever transport this platform provides" as
        // one type (ADR-0004).
        let listener = listener().await;
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await });

        let t: Box<dyn Transport> = connector().connect(addr).await.unwrap();
        let _server = accept.await.unwrap().unwrap();

        assert_eq!(t.send(b"abc").await.unwrap(), 3);
        t.shutdown().await.unwrap();
    }
}
