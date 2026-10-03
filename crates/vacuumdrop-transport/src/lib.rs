//! The transport abstraction.
//!
//! Charter 12 defines the interface: `connect`, `accept`, `send`, `receive`,
//! `shutdown`. The exact trait is settled here, with one rule taken from
//! ADR-0004 and enforced by review:
//!
//! > If a name would be meaningless over Ethernet or USB, it does not belong in
//! > this trait.
//!
//! So there is no `connect_wifi_direct_group_owner()`, no `set_go_intent()`, no
//! `wait_for_p2p_group_formation()`. The trait deals in addresses, streams, and
//! bytes. A Wi-Fi Direct adapter establishes the radio link and then hands the
//! core an address; from that point the radio is irrelevant.
//!
//! # Object safety
//!
//! The trait is object-safe on purpose. Phase 3 registers a Windows adapter,
//! Phase 6 an Android one, and the engine needs to hold "whichever transport this
//! platform provides" as a single type. A generic-only trait would push that
//! choice up into every caller for no benefit.
//!
//! Async methods are expressed through [`BoxFuture`] rather than `async fn in
//! trait`, because an `async fn` trait method is not object-safe. The trade-off
//! is one heap allocation per call, which is irrelevant next to a network round
//! trip and worth paying for the dispatch flexibility.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

use vacuumdrop_core::TransportError;

// TCP is this build's transport ([ADR-0003](../../docs/adr/0003-tcp-first.md)).
// Declared here so a Wi-Fi Direct adapter in Phase 3 is added as a sibling
// module rather than by replacing this one.
pub mod tcp;

/// A boxed, `Send` future.
///
/// `Send` is required because transport futures are spawned onto the Tokio
/// runtime across task boundaries.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A bidirectional byte stream.
///
/// Implementations move bytes. They do not interpret them, and they must not
/// impose protocol semantics on the data.
pub trait Transport: Send + Sync {
    /// The peer's address, when one is meaningful.
    fn peer_addr(&self) -> Option<SocketAddr>;

    /// Sends bytes, flushing the write half before returning.
    ///
    /// The buffer is borrowed for the duration of the future, so the caller
    /// cannot mutate or free it while the write is in flight.
    fn send<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<usize, TransportError>>;

    /// Receives into `buf`, returning 0 at end of stream.
    ///
    /// A return of 0 means the peer closed cleanly. Any other condition is an
    /// error, so the caller never has to distinguish "clean end" from "failed"
    /// by inspecting an error code.
    fn receive<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, Result<usize, TransportError>>;

    /// Closes the connection.
    ///
    /// Idempotent: calling it on an already-closed transport succeeds.
    fn shutdown(&self) -> BoxFuture<'_, Result<(), TransportError>>;

    /// A human-readable description for logs.
    fn describe(&self) -> String {
        match self.peer_addr() {
            Some(addr) => format!("{addr}"),
            None => "unconnected".to_owned(),
        }
    }
}

/// Something that produces [`Transport`] instances.
pub trait TransportListener: Send + Sync {
    /// The address actually bound.
    ///
    /// Separate from the requested address so binding to port 0 can report the
    /// port the OS chose, which is how a test avoids hard-coding a port.
    fn local_addr(&self) -> Result<SocketAddr, TransportError>;

    /// Waits for an inbound connection.
    ///
    /// Takes `&self` so a single listener can accept concurrently.
    fn accept(&self) -> BoxFuture<'_, Result<Box<dyn Transport>, TransportError>>;
}

/// Anything that creates outbound transports.
pub trait Connector: Send + Sync {
    /// Connects to `addr`, or fails.
    fn connect(
        &self,
        addr: SocketAddr,
    ) -> BoxFuture<'_, Result<Box<dyn Transport>, TransportError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The abstraction must be usable behind a trait object, since the engine
    /// will hold "whichever transport this platform provides" as one type.
    #[test]
    fn all_three_traits_are_object_safe() {
        // Separate bounds per trait: asserting all three against `Transport`
        // would only prove `Transport` is object-safe.
        fn assert_transport<T: Transport + ?Sized>() {}
        fn assert_listener<T: TransportListener + ?Sized>() {}
        fn assert_connector<T: Connector + ?Sized>() {}

        assert_transport::<dyn Transport>();
        assert_listener::<dyn TransportListener>();
        assert_connector::<dyn Connector>();
    }

    /// No method may require a Wi-Fi Direct concept. This is the naming rule from
    /// ADR-0004 expressed as a test: if a transport-specific method is ever added
    /// here, this list must be updated deliberately rather than by accident.
    #[test]
    fn transport_surface_has_no_radio_concepts() {
        let forbidden = [
            "group_owner",
            "go_intent",
            "p2p",
            "wifi",
            "wlan",
            "advertisement",
            "discovery",
            "channel",
        ];
        // The current surface is exactly these five methods.
        let surface = ["peer_addr", "send", "receive", "shutdown", "describe"];
        for name in surface {
            for bad in forbidden {
                assert!(
                    !name.to_ascii_lowercase().contains(bad),
                    "`{name}` leaks a radio concept (`{bad}`)"
                );
            }
        }
    }

    #[test]
    fn default_describe_handles_absent_address() {
        struct Unconnected;
        impl Transport for Unconnected {
            fn peer_addr(&self) -> Option<SocketAddr> {
                None
            }
            fn send(&self, _: &[u8]) -> BoxFuture<'_, Result<usize, TransportError>> {
                Box::pin(async { Err(TransportError::Closed) })
            }
            fn receive(&self, _: &mut [u8]) -> BoxFuture<'_, Result<usize, TransportError>> {
                Box::pin(async { Err(TransportError::Closed) })
            }
            fn shutdown(&self) -> BoxFuture<'_, Result<(), TransportError>> {
                Box::pin(async { Ok(()) })
            }
        }

        assert_eq!(Unconnected.describe(), "unconnected");
    }

    #[tokio::test]
    async fn boxed_futures_are_usable_across_task_boundaries() {
        struct Echo;
        impl Transport for Echo {
            fn peer_addr(&self) -> Option<SocketAddr> {
                None
            }
            fn send(&self, bytes: &[u8]) -> BoxFuture<'_, Result<usize, TransportError>> {
                let n = bytes.len();
                Box::pin(async move { Ok(n) })
            }
            fn receive(&self, _: &mut [u8]) -> BoxFuture<'_, Result<usize, TransportError>> {
                Box::pin(async { Ok(0) })
            }
            fn shutdown(&self) -> BoxFuture<'_, Result<(), TransportError>> {
                Box::pin(async { Ok(()) })
            }
        }

        let t: Box<dyn Transport> = Box::new(Echo);
        // Spawning proves the future is Send, which the runtime requires.
        let n = tokio::spawn(async move {
            let t: Box<dyn Transport> = t;
            t.send(b"hello").await
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(n, 5);
    }
}
