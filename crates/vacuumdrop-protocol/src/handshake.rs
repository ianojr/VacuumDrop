//! Session handshake.
//!
//! Charter 40 and 66: a session negotiates a protocol version explicitly and
//! fails closed when there is no overlap. A silent downgrade to an older or
//! weaker protocol is never acceptable.
//!
//! # Exchange
//!
//! ```text
//! client                                   server
//!   |-- VersionNegotiate [1,2] ------------->|   validate, choose highest common
//!   |<-- VersionAck { selected: 2 } ---------|   fail closed if none
//!   |-- Hello { name, version } ------------>|
//!   |<-- HelloAck { status } ----------------|   Accepted, or Rejected(reason)
//!   |-- Capabilities { .. } ---------------->|
//!   |<-- Capabilities { .. } ----------------|
//! ```
//!
//! Both sides validate. A server that accepts a malformed offer is as much a
//! bug as a client that sends one, so [`ServerHandshake`] validates as strictly
//! as [`ClientHandshake`].
//!
//! This type is transport-agnostic: it consumes and produces
//! [`VacuumDropMessage`] values, so it can be driven over TCP, a Wi-Fi Direct
//! link, or a test harness without change. That is the property ADR-0004
//! requires.

use vacuumdrop_core::{
    AuthenticationError, Limits, NegotiatedVersion, PeerDescription, ProtocolError, Violation,
};

use crate::message::{
    CapabilitySet, Goodbye, GoodbyeReason, Hello, HelloAck, HelloStatus, SupportedVersions,
    VacuumDropMessage, VersionSelection,
};

/// Errors raised while running a handshake.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    /// The peer sent something structurally invalid.
    #[error("invalid message during handshake: {0}")]
    Invalid(#[from] ProtocolError),

    /// No version is acceptable to both peers.
    #[error("version negotiation failed: {0}")]
    UnsupportedVersion(#[from] vacuumdrop_core::UnsupportedVersionError),

    /// The peer greeted us and we refused, or it refused us.
    #[error("peer rejected the greeting: {0}")]
    Rejected(&'static str),

    /// The greeting violated a limit or format rule.
    #[error("greeting invalid: {0}")]
    InvalidGreeting(#[from] Violation),

    /// The peer sent messages in the wrong order.
    #[error("unexpected message `{got}` during handshake (expected `{expected}`)")]
    OutOfOrder {
        /// What arrived.
        got: &'static str,
        /// What was required at this point.
        expected: &'static str,
    },

    /// The peer closed the connection mid-handshake.
    #[error("peer disconnected during handshake")]
    Disconnected,
}

impl From<HandshakeError> for ProtocolError {
    fn from(e: HandshakeError) -> Self {
        match e {
            HandshakeError::Invalid(p) => p,
            HandshakeError::UnsupportedVersion(_) => {
                ProtocolError::StateViolation("no common protocol version")
            }
            HandshakeError::Rejected(r) => ProtocolError::StateViolation(r),
            HandshakeError::OutOfOrder { got, expected } => ProtocolError::UnexpectedMessage {
                message: got,
                state: expected,
            },
            HandshakeError::InvalidGreeting(v) => ProtocolError::InvalidField {
                field: v.field,
                reason: v.reason,
            },
            HandshakeError::Disconnected => {
                ProtocolError::StateViolation("peer disconnected during handshake")
            }
        }
    }
}

impl From<HandshakeError> for AuthenticationError {
    fn from(e: HandshakeError) -> Self {
        Self::Handshake(match &e {
            HandshakeError::Rejected(r) => r,
            HandshakeError::UnsupportedVersion(_) => "no common protocol version",
            HandshakeError::Disconnected => "peer disconnected during handshake",
            _ => "handshake failed",
        })
    }
}

/// The outcome of a successful handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionParameters {
    /// The version both peers will use.
    pub version: NegotiatedVersion,
    /// The peer's validated description.
    pub peer: PeerDescription,
    /// The capabilities both peers advertised.
    pub local_capabilities: Vec<u8>,
    /// The chunk size both peers can accept.
    pub max_chunk_size: u32,
}

impl SessionParameters {
    /// The smaller of the two chunk sizes.
    ///
    /// Either side may send at most what both can receive, so the negotiated
    /// value is the minimum rather than a unilateral claim.
    pub fn chunk_size(&self) -> u32 {
        self.max_chunk_size.min(self.peer.max_chunk_size())
    }
}

/// Drives the handshake from the initiating side.
#[derive(Debug)]
pub struct ClientHandshake {
    device_name: String,
    client_version: String,
    limits: Limits,
    local_capabilities: Vec<u8>,
    local_max_chunk_size: u32,
    started: bool,
}

impl ClientHandshake {
    /// Builds a client handshake.
    pub fn new(
        device_name: impl Into<String>,
        client_version: impl Into<String>,
        limits: Limits,
        local_capabilities: Vec<u8>,
        local_max_chunk_size: u32,
    ) -> Self {
        Self {
            device_name: device_name.into(),
            client_version: client_version.into(),
            limits,
            local_capabilities,
            local_max_chunk_size,
            started: false,
        }
    }

    /// The first message to send: our advertised versions.
    pub fn opener(&mut self) -> VacuumDropMessage {
        self.started = true;
        VacuumDropMessage::VersionNegotiate(SupportedVersions::local())
    }

    /// Consumes the server's reply and produces our greeting.
    ///
    /// Validates the version choice against our own supported set. A server
    /// that names a version we never offered is rejected rather than accepted
    /// on trust.
    pub fn on_version_ack(
        &mut self,
        msg: VacuumDropMessage,
    ) -> Result<VacuumDropMessage, HandshakeError> {
        match msg {
            VacuumDropMessage::VersionAck(VersionSelection { selected }) => {
                if !NegotiatedVersion::SUPPORTED.contains(&selected) {
                    return Err(HandshakeError::UnsupportedVersion(
                        vacuumdrop_core::UnsupportedVersionError {
                            local: NegotiatedVersion::SUPPORTED.to_vec(),
                            remote: vec![selected],
                        },
                    ));
                }
                Ok(VacuumDropMessage::Hello(Hello {
                    device_name: self.device_name.clone(),
                    client_version: self.client_version.clone(),
                }))
            }
            other => Err(HandshakeError::OutOfOrder {
                got: other.name(),
                expected: "VersionAck",
            }),
        }
    }

    /// Consumes the server's greeting response.
    ///
    /// Returns the server's `HelloStatus`. A rejection is returned as `Ok` with
    /// `Rejected`, not as `Err`: the exchange completed correctly and the
    /// answer was no. The caller decides whether that is fatal.
    pub fn on_hello_ack(&mut self, msg: VacuumDropMessage) -> Result<HelloAck, HandshakeError> {
        match msg {
            VacuumDropMessage::HelloAck(ack) => {
                // A rejection carries no usable identity, so the name is not
                // validated here. Validating it would reject legitimate refusals
                // for an unrelated reason and hide the real one.
                if !matches!(ack.status, HelloStatus::Rejected(_)) {
                    PeerDescription::new(
                        ack.device_name.clone(),
                        NegotiatedVersion::SUPPORTED.to_vec(),
                        vec![],
                        self.local_max_chunk_size,
                        &self.limits,
                    )?;
                }
                Ok(ack)
            }
            other => Err(HandshakeError::OutOfOrder {
                got: other.name(),
                expected: "HelloAck",
            }),
        }
    }

    /// Our capability advertisement.
    pub fn capabilities(&self) -> VacuumDropMessage {
        VacuumDropMessage::Capabilities(CapabilitySet {
            features: self.local_capabilities.clone(),
            max_chunk_size: self.local_max_chunk_size,
        })
    }
}

/// Drives the handshake from the accepting side.
#[derive(Debug)]
pub struct ServerHandshake {
    device_name: String,
    limits: Limits,
    local_capabilities: Vec<u8>,
    local_max_chunk_size: u32,
    selected: Option<NegotiatedVersion>,
    client_name: Option<String>,
}

impl ServerHandshake {
    /// Builds a server handshake.
    pub fn new(
        device_name: impl Into<String>,
        limits: Limits,
        local_capabilities: Vec<u8>,
        local_max_chunk_size: u32,
    ) -> Self {
        Self {
            device_name: device_name.into(),
            limits,
            local_capabilities,
            local_max_chunk_size,
            selected: None,
            client_name: None,
        }
    }

    /// Consumes the client's version offer and produces our answer.
    ///
    /// Fails closed: no overlap is an error, never a silent downgrade.
    pub fn on_version_negotiate(
        &mut self,
        msg: VacuumDropMessage,
    ) -> Result<VacuumDropMessage, HandshakeError> {
        match msg {
            VacuumDropMessage::VersionNegotiate(offered) => {
                let selection = VersionSelection::negotiate(&offered)?;
                self.selected = Some(NegotiatedVersion(selection.selected));
                Ok(VacuumDropMessage::VersionAck(selection))
            }
            other => Err(HandshakeError::OutOfOrder {
                got: other.name(),
                expected: "VersionNegotiate",
            }),
        }
    }

    /// Consumes the client's greeting and produces our response.
    ///
    /// Validates the client's name before echoing a rejection, so a malformed
    /// greeting is reported as malformed rather than as a generic rejection.
    pub fn on_hello(
        &mut self,
        msg: VacuumDropMessage,
    ) -> Result<VacuumDropMessage, HandshakeError> {
        match msg {
            VacuumDropMessage::Hello(hello) => {
                PeerDescription::new(
                    hello.device_name.clone(),
                    NegotiatedVersion::SUPPORTED.to_vec(),
                    vec![],
                    self.local_max_chunk_size,
                    &self.limits,
                )?;

                self.client_name = Some(hello.device_name);
                Ok(VacuumDropMessage::HelloAck(HelloAck {
                    status: HelloStatus::Accepted,
                    device_name: self.device_name.clone(),
                }))
            }
            other => Err(HandshakeError::OutOfOrder {
                got: other.name(),
                expected: "Hello",
            }),
        }
    }

    /// Consumes the client's capabilities and completes the handshake.
    ///
    /// This is the last step, so the returned [`SessionParameters`] is only
    /// produced once every party has been validated.
    pub fn on_capabilities(
        &mut self,
        msg: VacuumDropMessage,
    ) -> Result<SessionParameters, HandshakeError> {
        let version = self.selected.ok_or(HandshakeError::OutOfOrder {
            got: "Capabilities",
            expected: "VersionAck",
        })?;

        let set = match msg {
            VacuumDropMessage::Capabilities(set) => set,
            other => {
                return Err(HandshakeError::OutOfOrder {
                    got: other.name(),
                    expected: "Capabilities",
                });
            }
        };

        set.validate(&self.limits)?;

        let name = self.client_name.clone().ok_or(HandshakeError::OutOfOrder {
            got: "Capabilities",
            expected: "Hello",
        })?;

        let peer = PeerDescription::new(
            name,
            vec![version.0],
            set.capabilities(),
            set.max_chunk_size,
            &self.limits,
        )?;

        Ok(SessionParameters {
            version,
            peer,
            local_capabilities: self.local_capabilities.clone(),
            max_chunk_size: self.local_max_chunk_size,
        })
    }
}

/// The message a side sends when it declines to proceed.
pub fn rejection(reason: crate::message::HelloRejection) -> VacuumDropMessage {
    VacuumDropMessage::HelloAck(HelloAck {
        status: HelloStatus::Rejected(reason),
        device_name: String::new(),
    })
}

/// The message a side sends when ending a session cleanly.
pub fn goodbye(reason: GoodbyeReason) -> VacuumDropMessage {
    VacuumDropMessage::Goodbye(Goodbye { reason })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ErrorCode, HelloRejection, ProtocolErrorReport};
    use vacuumdrop_core::Capability;

    const CLIENT: &str = "client-device";
    const SERVER: &str = "server-device";

    fn limits() -> Limits {
        Limits::new()
    }

    fn client() -> ClientHandshake {
        ClientHandshake::new(
            CLIENT,
            "0.1.0",
            limits(),
            vec![Capability::SupportsResume.discriminant()],
            8 * 1024 * 1024,
        )
    }

    fn server() -> ServerHandshake {
        ServerHandshake::new(
            SERVER,
            limits(),
            vec![Capability::SupportsResume.discriminant()],
            8 * 1024 * 1024,
        )
    }

    /// Runs a complete successful handshake between the two sides.
    fn handshake_pair(
        client: &mut ClientHandshake,
        server: &mut ServerHandshake,
    ) -> SessionParameters {
        let open = client.opener();
        let ack = server.on_version_negotiate(open).unwrap();
        let hello = client.on_version_ack(ack).unwrap();
        let hello_ack = server.on_hello(hello).unwrap();
        let caps = client.on_hello_ack(hello_ack).unwrap();
        assert_eq!(caps.status, HelloStatus::Accepted);
        let caps_msg = client.capabilities();
        server.on_capabilities(caps_msg).unwrap()
    }

    #[test]
    fn full_handshake_succeeds() {
        let mut c = client();
        let mut s = server();
        let params = handshake_pair(&mut c, &mut s);

        assert_eq!(params.version, NegotiatedVersion(1));
        assert_eq!(params.peer.display_name(), CLIENT);
        assert!(
            params
                .peer
                .capabilities()
                .contains(&Capability::SupportsResume)
        );
        assert_eq!(params.chunk_size(), 8 * 1024 * 1024);
    }

    #[test]
    fn opener_must_be_version_negotiate() {
        let mut c = client();
        assert!(c.opener().is_valid_opener());
    }

    #[test]
    fn out_of_order_messages_are_refused() {
        let mut c = client();
        // VersionAck before we sent an offer.
        assert!(matches!(
            c.on_version_ack(VacuumDropMessage::Ping(crate::message::Ping { nonce: 1 })),
            Err(HandshakeError::OutOfOrder { .. })
        ));

        let mut s = server();
        // Hello before version negotiation.
        assert!(matches!(
            s.on_version_negotiate(VacuumDropMessage::Hello(Hello {
                device_name: "x".into(),
                client_version: "1".into(),
            })),
            Err(HandshakeError::OutOfOrder { .. })
        ));
    }

    #[test]
    fn server_cannot_complete_without_version_negotiation() {
        let mut s = server();
        assert!(matches!(
            s.on_capabilities(VacuumDropMessage::Capabilities(CapabilitySet {
                features: vec![],
                max_chunk_size: 1024,
            })),
            Err(HandshakeError::OutOfOrder { .. })
        ));
    }

    #[test]
    fn server_cannot_complete_without_greeting() {
        let mut s = server();
        let _ = s.on_version_negotiate(VacuumDropMessage::VersionNegotiate(
            SupportedVersions::local(),
        ));
        assert!(matches!(
            s.on_capabilities(VacuumDropMessage::Capabilities(CapabilitySet {
                features: vec![],
                max_chunk_size: 1024,
            })),
            Err(HandshakeError::OutOfOrder { .. })
        ));
    }

    #[test]
    fn no_common_version_fails_closed() {
        // Charter 66: never silently downgrade.
        let mut s = server();
        let err = s
            .on_version_negotiate(VacuumDropMessage::VersionNegotiate(SupportedVersions {
                versions: vec![42, 43],
            }))
            .unwrap_err();
        assert!(matches!(err, HandshakeError::UnsupportedVersion(_)));
        assert!(s.selected.is_none());
    }

    #[test]
    fn server_rejects_a_version_we_never_offered() {
        // A lying server must not be able to steer us onto a version we do not
        // implement.
        let mut c = client();
        let _ = c.opener();
        let err = c
            .on_version_ack(VacuumDropMessage::VersionAck(VersionSelection {
                selected: 99,
            }))
            .unwrap_err();
        assert!(matches!(err, HandshakeError::UnsupportedVersion(_)));
    }

    #[test]
    fn server_rejects_malformed_version_list() {
        let mut s = server();
        assert!(
            s.on_version_negotiate(VacuumDropMessage::VersionNegotiate(SupportedVersions {
                versions: vec![],
            }))
            .is_err()
        );
        assert!(
            s.on_version_negotiate(VacuumDropMessage::VersionNegotiate(SupportedVersions {
                versions: vec![2, 1],
            }))
            .is_err()
        );
    }

    #[test]
    fn client_name_with_control_characters_is_refused() {
        let mut s = server();
        let _ = s.on_version_negotiate(VacuumDropMessage::VersionNegotiate(
            SupportedVersions::local(),
        ));
        let err = s
            .on_hello(VacuumDropMessage::Hello(Hello {
                device_name: "evil\u{1b}[31m".into(),
                client_version: "1".into(),
            }))
            .unwrap_err();
        assert!(matches!(err, HandshakeError::InvalidGreeting(_)));
    }

    #[test]
    fn overlong_client_name_is_refused() {
        let mut s = server();
        let _ = s.on_version_negotiate(VacuumDropMessage::VersionNegotiate(
            SupportedVersions::local(),
        ));
        assert!(
            s.on_hello(VacuumDropMessage::Hello(Hello {
                device_name: "x".repeat(1000),
                client_version: "1".into(),
            }))
            .is_err()
        );
    }

    #[test]
    fn server_name_is_validated_on_the_client_side_too() {
        let mut c = client();
        let _ = c.opener();
        assert!(
            c.on_hello_ack(VacuumDropMessage::HelloAck(HelloAck {
                status: HelloStatus::Accepted,
                device_name: "x".repeat(1000),
            }))
            .is_err()
        );
    }

    #[test]
    fn empty_server_name_is_refused() {
        let mut c = client();
        let _ = c.opener();
        assert!(
            c.on_hello_ack(VacuumDropMessage::HelloAck(HelloAck {
                status: HelloStatus::Accepted,
                device_name: String::new(),
            }))
            .is_err()
        );
    }

    #[test]
    fn rejection_is_reported_without_failing_the_exchange() {
        // The handshake completed; the answer was no. The caller decides.
        let mut c = client();
        let _ = c.opener();
        let ack = c
            .on_hello_ack(rejection(HelloRejection::ServerBusy))
            .unwrap();
        assert_eq!(
            ack.status,
            HelloStatus::Rejected(HelloRejection::ServerBusy)
        );
    }

    #[test]
    fn capabilities_with_oversized_chunk_size_are_refused() {
        let mut c = client();
        let mut s = server();
        let _ = s.on_version_negotiate(c.opener());
        let hello = c
            .on_version_ack(VacuumDropMessage::VersionAck(VersionSelection {
                selected: 1,
            }))
            .unwrap();
        let _ = s.on_hello(hello).unwrap();

        assert!(
            s.on_capabilities(VacuumDropMessage::Capabilities(CapabilitySet {
                features: vec![],
                max_chunk_size: limits().max_frame_length + 1,
            }))
            .is_err()
        );
    }

    #[test]
    fn capabilities_with_zero_chunk_size_are_refused() {
        let mut c = client();
        let mut s = server();
        let _ = s.on_version_negotiate(c.opener());
        let hello = c
            .on_version_ack(VacuumDropMessage::VersionAck(VersionSelection {
                selected: 1,
            }))
            .unwrap();
        let _ = s.on_hello(hello).unwrap();

        assert!(
            s.on_capabilities(VacuumDropMessage::Capabilities(CapabilitySet {
                features: vec![],
                max_chunk_size: 0,
            }))
            .is_err()
        );
    }

    #[test]
    fn negotiated_chunk_size_is_the_minimum_of_both_sides() {
        // A peer claiming a larger chunk than we accept must not widen ours.
        // We are the smaller side here.
        let mut c = ClientHandshake::new(CLIENT, "0.1.0", limits(), vec![], 4096);
        let mut s = ServerHandshake::new(SERVER, limits(), vec![], 1024);

        let _ = s.on_version_negotiate(c.opener()).unwrap();
        let hello = c
            .on_version_ack(VacuumDropMessage::VersionAck(VersionSelection {
                selected: 1,
            }))
            .unwrap();
        let _ = s.on_hello(hello).unwrap();

        let params = s
            .on_capabilities(VacuumDropMessage::Capabilities(CapabilitySet {
                features: vec![],
                max_chunk_size: 4096,
            }))
            .unwrap();
        assert_eq!(
            params.chunk_size(),
            1024,
            "chunk size widened past our own limit"
        );
    }

    #[test]
    fn negotiated_chunk_size_takes_the_smaller_of_two_generous_peers() {
        let mut c = ClientHandshake::new(CLIENT, "0.1.0", limits(), vec![], 1024);
        let mut s = ServerHandshake::new(SERVER, limits(), vec![], 8 * 1024 * 1024);

        let _ = s.on_version_negotiate(c.opener()).unwrap();
        let hello = c
            .on_version_ack(VacuumDropMessage::VersionAck(VersionSelection {
                selected: 1,
            }))
            .unwrap();
        let _ = s.on_hello(hello).unwrap();

        let params = s
            .on_capabilities(VacuumDropMessage::Capabilities(CapabilitySet {
                features: vec![],
                max_chunk_size: 4096,
            }))
            .unwrap();
        assert_eq!(params.chunk_size(), 4096);
    }

    #[test]
    fn unknown_capability_flags_do_not_break_the_handshake() {
        let mut c = client();
        let mut s = server();
        let _ = s.on_version_negotiate(c.opener()).unwrap();
        let hello = c
            .on_version_ack(VacuumDropMessage::VersionAck(VersionSelection {
                selected: 1,
            }))
            .unwrap();
        let _ = s.on_hello(hello).unwrap();

        let params = s
            .on_capabilities(VacuumDropMessage::Capabilities(CapabilitySet {
                features: vec![0, 200, 255], // 200/255 are from a hypothetical future
                max_chunk_size: 4096,
            }))
            .unwrap();
        assert_eq!(params.peer.capabilities(), vec![Capability::SupportsResume]);
    }

    #[test]
    fn error_message_can_be_carried_in_band() {
        let mut c = client();
        let _ = c.opener();
        // A peer may report an error instead of a HelloAck.
        let err = c
            .on_hello_ack(VacuumDropMessage::Error(ProtocolErrorReport::fatal(
                ErrorCode::RateLimited,
                "slow down",
            )))
            .unwrap_err();
        assert!(matches!(err, HandshakeError::OutOfOrder { .. }));
    }

    #[test]
    fn goodbye_helper_produces_a_clean_shutdown() {
        assert_eq!(
            goodbye(GoodbyeReason::Complete),
            VacuumDropMessage::Goodbye(Goodbye {
                reason: GoodbyeReason::Complete
            })
        );
    }

    #[test]
    fn handshake_errors_convert_into_core_errors() {
        let e = HandshakeError::OutOfOrder {
            got: "Ping",
            expected: "Hello",
        };
        let p: ProtocolError = e.into();
        assert!(p.to_string().contains("Ping"));

        let e2 = HandshakeError::Rejected("server busy");
        let a: AuthenticationError = e2.into();
        assert!(a.to_string().contains("server busy"));
    }
}
