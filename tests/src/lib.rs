//! Shared helpers for integration tests.
//!
//! Kept in one place so a test needing a working pair of connected sessions does
//! not re-implement the plumbing. Every helper binds to port 0 and reports the
//! address the OS chose, so tests never collide on a fixed port.
//!
//! Limits are shared via [`Arc`] because most tests spawn a server task that
//! needs the same configuration as the client. Passing `Arc<Limits>` rather than
//! `Limits` keeps that free and makes the sharing explicit.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use vacuumdrop_core::{Capability, Limits, SessionId};
use vacuumdrop_protocol::message::GoodbyeReason;
use vacuumdrop_protocol::session::{Session, SessionConfig};
use vacuumdrop_transport::tcp::{TcpConnector, TcpTransportListener};
use vacuumdrop_transport::{Connector, Transport, TransportListener};

/// Short deadlines: a test that is going to fail should fail fast.
pub const FAST: Duration = Duration::from_secs(5);

/// A short handshake deadline.
pub const HANDSHAKE: Duration = Duration::from_secs(3);

/// Shared limits with test-friendly deadlines.
pub fn limits() -> Arc<Limits> {
    Arc::new(Limits::new())
}

/// A listening socket on an ephemeral loopback port.
pub async fn listener(limits: &Arc<Limits>) -> (TcpTransportListener, SocketAddr) {
    let l = TcpTransportListener::bind(
        "127.0.0.1:0".parse().unwrap(),
        limits.idle_timeout,
        limits.idle_timeout,
        limits.idle_timeout,
    )
    .await
    .expect("bind loopback");
    let addr = l.local_addr().expect("bound address");
    (l, addr)
}

/// A connector with explicit deadlines.
pub fn connector(limits: &Arc<Limits>) -> TcpConnector {
    TcpConnector::new(
        limits.connect_timeout,
        limits.idle_timeout,
        limits.idle_timeout,
    )
}

/// Wraps a transport for use by a [`Session`].
pub fn shared(t: Box<dyn Transport>) -> Arc<dyn Transport> {
    Arc::from(t)
}

/// Builds a session over an already-connected transport.
pub fn session(id: u64, transport: Arc<dyn Transport>, limits: &Arc<Limits>) -> Session {
    Session::new(SessionId::new(id), transport, limits.as_ref().clone()).expect("session")
}

/// A config with a given device name.
pub fn config(name: &str, limits: &Arc<Limits>) -> SessionConfig {
    SessionConfig {
        device_name: name.to_owned(),
        capabilities: vec![Capability::SupportsResume],
        max_chunk_size: 1024 * 1024,
        limits: limits.as_ref().clone(),
    }
}

/// Writes every byte, tolerating partial writes.
///
/// A real socket may accept fewer bytes than offered; a test that assumes a
/// single write succeeds would be testing the wrong thing.
pub async fn write_all(t: &Arc<dyn Transport>, bytes: &[u8]) {
    let mut written = 0;
    while written < bytes.len() {
        match t.send(&bytes[written..]).await {
            Ok(0) | Err(_) => return,
            Ok(n) => written += n,
        }
    }
}

/// Runs both sides of a handshake concurrently over a real TCP connection.
///
/// Returns the parameters from each side. This is the workhorse: it exercises
/// the codec, the transport, the state machine, and the validators together,
/// which is the only way to know the layers compose.
pub async fn handshake_pair(
    client_name: &str,
    server_name: &str,
) -> (
    vacuumdrop_protocol::handshake::SessionParameters,
    vacuumdrop_protocol::handshake::SessionParameters,
) {
    let client_name = client_name.to_owned();
    let server_name = server_name.to_owned();
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let transport = listener.accept().await.expect("accept");
            let mut session = session(1, shared(transport), &limits);
            let cfg = config(&server_name, &limits);
            let params = session
                .run_server_handshake(&cfg)
                .await
                .expect("server handshake");
            session.close(GoodbyeReason::Complete).await.ok();
            params
        }
    });

    let t = connector(&limits).connect(addr).await.expect("connect");
    let mut client_session = session(2, shared(t), &limits);
    let cfg = config(&client_name, &limits);
    let client_params = client_session
        .run_client_handshake(&cfg)
        .await
        .expect("client handshake");
    client_session.close(GoodbyeReason::Complete).await.ok();

    let server_params = server_task.await.expect("server task");
    (client_params, server_params)
}
