//! The normalized boundary types this crate exposes.
//!
//! Charter 47: the FFI boundary exchanges **simple concepts**, never platform
//! objects. This module is the entire public vocabulary of the adapter.
//!
//! Every type here is plain Rust — no `windows::*`, no COM, no WinRT. That is
//! not a style preference; it is the mechanism by which
//! [ADR-0004](../../docs/adr/0004-transport-abstraction.md) is enforced rather
//! than merely intended. If a WinRT type ever needed to reach the core, it would
//! have to appear in one of these signatures, and that is where review would
//! catch it.

use std::fmt;
use std::net::IpAddr;

/// Why a connection attempt failed.
///
/// `PartialEq` is derived so tests can assert on a specific variant without
/// matching every field. Deliberately not `Eq`: `Windows` carries a free-form
/// message, and message text is not a value identity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    /// The machine cannot do Wi-Fi Direct at all.
    #[error("Wi-Fi Direct is unavailable: {0}")]
    Unavailable(String),

    /// Wi-Fi Direct exists but is administratively off.
    #[error("Wi-Fi Direct is disabled: {0}")]
    Disabled(String),

    /// Discovery found nothing within the deadline.
    #[error("discovery found no peers within {0:?}")]
    NoPeersFound(std::time::Duration),

    /// A named peer could not be reached or connected.
    #[error("could not connect to `{peer}`: {reason}")]
    ConnectFailed {
        /// The peer identifier as we know it.
        peer: String,
        /// Why it failed.
        reason: String,
    },

    /// The link formed but the OS gave us no usable address.
    ///
    /// Distinct from `ConnectFailed`: the radio link succeeded, and the failure
    /// is in address assignment. That means retrying the *link* would be
    /// pointless.
    #[error("the Wi-Fi Direct group formed but no IP address was assigned")]
    NoAddressAssigned,

    /// The adapter was used after being shut down.
    #[error("the adapter has been shut down")]
    ShutDown,

    /// The OS reported a failure HRESULT.
    #[error("Windows API failure: {message} ({code:#010X})")]
    Windows {
        /// The HRESULT.
        code: i32,
        /// The OS's own message, which is more useful than the numeric code.
        message: String,
    },
}

/// A peer found by discovery.
///
/// `device_id` is an opaque WinRT device identifier, kept as a `String` so no
/// WinRT type escapes. The core treats it as opaque: it is not an address, not a
/// MAC, and carries no identity meaning (Charter 16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    /// Opaque platform identifier, used to connect.
    pub device_id: String,
    /// Peer-declared name. **Untrusted** — a hostile peer controls this string,
    /// so it must pass the core's validator before display.
    pub display_name: String,
}

impl DiscoveredPeer {
    /// Builds a peer record.
    pub fn new(device_id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            display_name: display_name.into(),
        }
    }
}

impl fmt::Display for DiscoveredPeer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The name is untrusted and may contain terminal control characters. The
        // core rejects those, but this type is constructed by the adapter before
        // validation, so Display must not be the thing that trusts it.
        write!(f, "{}", sanitise_for_display(&self.display_name))
    }
}

/// Escapes control characters for display.
///
/// Deliberately duplicated from the core's rule rather than imported: this crate
/// must not depend on `vacuumdrop-core` for a formatting concern, and the
/// dependency direction is enforced to flow core <- platform, not the reverse.
pub fn sanitise_for_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() {
            out.push('\u{FFFD}');
        } else {
            out.push(c);
        }
    }
    out
}

/// Where a formed Wi-Fi Direct group can be reached.
///
/// This is the crate's single most important type. It is what the OS hands back
/// after group formation, and it is deliberately **just an IP address and a
/// port** — nothing about the radio that produced it.
///
/// Charter 47 lists `ConnectionInfo` as one of the six concepts permitted across
/// the boundary. Everything Wi-Fi-Direct-specific stays on this side of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// Address the group owner assigned to this peer.
    pub ip: IpAddr,
    /// TCP port the VacuumDrop protocol listens on.
    ///
    /// Fixed by our own protocol, not by Windows. Windows hands us an address;
    /// everything above it is ours.
    pub port: u16,
}

impl ConnectionInfo {
    /// Builds connection info.
    pub fn new(ip: IpAddr, port: u16) -> Self {
        Self { ip, port }
    }

    /// The address as something a socket can be opened to.
    ///
    /// Lives here rather than at each call site so the pairing of `ip` with
    /// `port` cannot be got wrong by a caller, and so the returned value is a
    /// plain `std` type with no WinRT in it.
    pub fn socket_addr(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::new(self.ip, self.port)
    }
}

impl fmt::Display for ConnectionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // IPv6 literals MUST be bracketed when combined with a port. Without
        // brackets, `fe80::1:9000` is ambiguous and every consumer that parses it
        // will read the last group as the port. A Wi-Fi Direct group owner hands
        // out a link-local IPv6 address, so this is the common case, not an edge
        // case.
        match self.ip {
            std::net::IpAddr::V4(v4) => write!(f, "{v4}:{}", self.port),
            std::net::IpAddr::V6(v6) => write!(f, "[{v6}]:{}", self.port),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_info_display_is_a_socket_address() {
        let c = ConnectionInfo::new("192.168.137.42".parse().unwrap(), 9000);
        assert_eq!(c.to_string(), "192.168.137.42:9000");
    }

    #[test]
    fn connection_info_display_round_trips_through_the_parser() {
        // The rendered form must be parseable back into the same address and port.
        // This is the property that matters: the string is what a user copies and
        // what a log line is grepped on.
        for ip in [
            "192.168.137.42",
            "127.0.0.1",
            "::1",
            "fe80::1234:5678:9abc:def0",
        ] {
            let c = ConnectionInfo::new(ip.parse().unwrap(), 9000);
            let rendered = c.to_string();
            let back: std::net::SocketAddr = rendered
                .parse()
                .unwrap_or_else(|e| panic!("`{rendered}` did not parse: {e}"));
            assert_eq!(back.ip(), c.ip, "ip changed for `{ip}`");
            assert_eq!(back.port(), c.port, "port changed for `{ip}`");
        }
    }

    #[test]
    fn socket_addr_agrees_with_the_display_form() {
        // `socket_addr()` is what the CLI hands to the connector, so a disagreement
        // between it and `Display` would mean connecting somewhere other than what
        // the user was shown.
        for (ip, port) in [("192.168.137.42", 9000u16), ("::1", 1), ("fe80::1", 65535)] {
            let c = ConnectionInfo::new(ip.parse().unwrap(), port);
            let sock = c.socket_addr();
            assert_eq!(sock.ip(), c.ip);
            assert_eq!(sock.port(), c.port);
            assert_eq!(sock.to_string(), c.to_string());
        }
    }

    #[test]
    fn connection_info_handles_ipv6() {
        let c = ConnectionInfo::new("fe80::1".parse().unwrap(), 9000);
        // Bracketed, because an unbracketed IPv6 address with a port is
        // ambiguous and will be parsed wrong by anything that consumes it.
        assert!(c.to_string().starts_with("[fe80::1]"), "got {}", c);
    }

    #[test]
    fn peer_display_escapes_control_characters() {
        let p = DiscoveredPeer::new("id-1", "Ian\u{1b}[31m's Laptop");
        let shown = p.to_string();
        assert!(
            !shown.chars().any(|c| c == '\u{1b}'),
            "escape sequence leaked into display: {shown:?}"
        );
        assert!(
            shown.contains('\u{FFFD}'),
            "control char was dropped silently"
        );
    }

    #[test]
    fn sanitiser_preserves_ordinary_text() {
        assert_eq!(sanitise_for_display("Büro 123"), "Büro 123");
        assert_eq!(sanitise_for_display(""), "");
    }

    #[test]
    fn sanitiser_replaces_every_control_class() {
        for c in ['\0', '\n', '\r', '\t', '\u{1b}', '\u{7f}'] {
            let s = format!("a{c}b");
            let out = sanitise_for_display(&s);
            assert!(!out.contains(c), "control {c:?} survived sanitisation");
            assert_eq!(out.chars().count(), 3, "length changed for {c:?}");
        }
    }

    #[test]
    fn peer_equality_uses_the_fields_not_display() {
        let a = DiscoveredPeer::new("id", "Name");
        let b = DiscoveredPeer::new("id", "Name");
        assert_eq!(a, b);
        let c = DiscoveredPeer::new("id", "Other");
        assert_ne!(a, c);
    }

    #[test]
    fn adapter_error_names_the_peer_on_connect_failure() {
        // A connect failure without the peer name is unactionable when several
        // peers are in range.
        let e = AdapterError::ConnectFailed {
            peer: "peer-abc".into(),
            reason: "device unreachable".into(),
        };
        let s = e.to_string();
        assert!(s.contains("peer-abc"));
        assert!(s.contains("device unreachable"));
    }

    #[test]
    fn no_address_is_distinct_from_connect_failure() {
        // The radio link succeeded; retrying it would be pointless. The distinct
        // variant is what lets the caller know that.
        assert!(
            AdapterError::NoAddressAssigned
                .to_string()
                .contains("no IP address")
        );
    }
}
