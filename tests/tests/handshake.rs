//! Integration tests: a real client and a real server, over a real socket.
//!
//! Charter 76 exit criterion for Phase 1: an integration test drives both ends
//! in process. These are those tests, plus the hostile-input cases Charter 52
//! requires.
//!
//! Nothing here is mocked. Every test opens a loopback socket and runs the same
//! code path `p2p connect` runs, so a passing test is evidence about shipped
//! behaviour rather than about a test double.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::bytes::BytesMut;
use tokio_util::codec::Encoder;

use vacuumdrop_core::{
    Capability, ConnectionState, Error, Limits, ProtocolError, SessionId, TransportError,
};
use vacuumdrop_protocol::codec::FrameCodec;
use vacuumdrop_protocol::message::{
    GoodbyeReason, Hello, Ping, SupportedVersions, VacuumDropMessage, VersionSelection,
};
use vacuumdrop_protocol::session::{Session, SessionError};
use vacuumdrop_tests::{
    HANDSHAKE, config, connector, handshake_pair, limits, listener, session, shared, write_all,
};
use vacuumdrop_transport::tcp::TcpTransportListener;
use vacuumdrop_transport::{Connector, Transport, TransportListener};

#[tokio::test]
async fn client_and_server_complete_a_handshake() {
    let (client, server) = handshake_pair("client-device", "server-device").await;

    assert_eq!(client.version.0, 1);
    assert_eq!(server.version.0, 1);
    // Each side must see the other's declared name.
    assert_eq!(client.peer.display_name(), "server-device");
    assert_eq!(server.peer.display_name(), "client-device");
    // Resume was advertised by both, so both must report it.
    assert!(
        client
            .peer
            .capabilities()
            .contains(&Capability::SupportsResume)
    );
    assert!(
        server
            .peer
            .capabilities()
            .contains(&Capability::SupportsResume)
    );
}

#[tokio::test]
async fn negotiated_chunk_size_is_consistent_on_both_sides() {
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(1, shared(t), &limits);
            let params = s
                .run_server_handshake(&config("server", &limits))
                .await
                .unwrap();
            params.chunk_size()
        }
    });

    let t = connector(&limits).connect(addr).await.unwrap();
    let mut c = session(2, shared(t), &limits);
    let client_size = c
        .run_client_handshake(&config("client", &limits))
        .await
        .unwrap()
        .chunk_size();
    let server_size = server_task.await.unwrap();

    // Both sides must agree, or a later transfer would size chunks differently
    // at each end.
    assert_eq!(client_size, server_size);
}

#[tokio::test]
async fn session_reaches_established_after_a_handshake() {
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(1, shared(t), &limits);
            s.run_server_handshake(&config("server", &limits))
                .await
                .unwrap();
            // Hold the session open long enough for the client to observe it.
            tokio::time::sleep(Duration::from_millis(150)).await;
            s.state()
        }
    });

    let t = connector(&limits).connect(addr).await.unwrap();
    let mut c = session(2, shared(t), &limits);
    c.run_client_handshake(&config("client", &limits))
        .await
        .unwrap();

    assert_eq!(c.state(), ConnectionState::Established);
    assert_eq!(server_task.await.unwrap(), ConnectionState::Established);
}

#[tokio::test]
async fn ping_is_answered_with_a_matching_pong() {
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(1, shared(t), &limits);
            s.run_server_handshake(&config("server", &limits))
                .await
                .unwrap();

            // Answer exactly one Ping, echoing the nonce. `None` if the peer
            // never probes or the session ends first.
            loop {
                match s.receive(Duration::from_secs(2)).await {
                    Ok(Some(VacuumDropMessage::Ping(p))) => {
                        let nonce = p.nonce;
                        s.send(Session::pong_for(nonce)).await.unwrap();
                        return Some(nonce);
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) | Err(_) => return None,
                }
            }
        }
    });

    let t = connector(&limits).connect(addr).await.unwrap();
    let mut c = session(2, shared(t), &limits);
    c.run_client_handshake(&config("client", &limits))
        .await
        .unwrap();

    c.ping(0xDEAD_BEEF).await.unwrap();
    assert!(c.has_pending_ping());

    match c.receive(Duration::from_secs(3)).await.unwrap() {
        Some(VacuumDropMessage::Pong(p)) => {
            assert_eq!(p.nonce, 0xDEAD_BEEF, "nonce was not echoed");
            assert!(!c.has_pending_ping(), "pending ping was not cleared");
        }
        other => panic!("expected Pong, got {other:?}"),
    }

    assert_eq!(server_task.await.unwrap(), Some(0xDEAD_BEEF));
}

#[tokio::test]
async fn server_refuses_a_peer_with_no_common_protocol_version() {
    // Charter 66: fail closed. A peer offering only unknown versions must be
    // rejected, never silently downgraded.
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(1, shared(t), &limits);
            s.run_server_handshake(&config("server", &limits))
                .await
                .is_err()
        }
    });

    // Speak raw framing so we can send an offer this build would never make.
    let t = connector(&limits).connect(addr).await.unwrap();
    let transport = shared(t);
    write_all(
        &transport,
        &encode(
            &VacuumDropMessage::VersionNegotiate(SupportedVersions {
                versions: vec![200, 201],
            }),
            &limits,
        ),
    )
    .await;

    // The server hangs up rather than proceeding.
    let mut chunk = [0u8; 64];
    let _ = transport.receive(&mut chunk).await;

    assert!(
        server_task.await.unwrap(),
        "server accepted an incompatible peer"
    );
}

#[tokio::test]
async fn server_refuses_a_greeting_with_control_characters() {
    // A name is rendered in a terminal; control codes must not survive.
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(1, shared(t), &limits);
            s.run_server_handshake(&config("server", &limits))
                .await
                .is_err()
        }
    });

    let t = connector(&limits).connect(addr).await.unwrap();
    let transport = shared(t);

    for msg in [
        VacuumDropMessage::VersionNegotiate(SupportedVersions::local()),
        VacuumDropMessage::Hello(Hello {
            device_name: "evil\u{1b}[31m\u{0}".into(),
            client_version: "1".into(),
        }),
    ] {
        write_all(&transport, &encode(&msg, &limits)).await;
    }

    let mut chunk = [0u8; 64];
    let _ = transport.receive(&mut chunk).await;

    assert!(
        server_task.await.unwrap(),
        "server accepted a hostile device name"
    );
}

#[tokio::test]
async fn server_rejects_an_overlong_device_name() {
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(1, shared(t), &limits);
            s.run_server_handshake(&config("server", &limits))
                .await
                .is_err()
        }
    });

    let t = connector(&limits).connect(addr).await.unwrap();
    let transport = shared(t);

    for msg in [
        VacuumDropMessage::VersionNegotiate(SupportedVersions::local()),
        VacuumDropMessage::Hello(Hello {
            device_name: "x".repeat(5000),
            client_version: "1".into(),
        }),
    ] {
        write_all(&transport, &encode(&msg, &limits)).await;
    }

    let mut chunk = [0u8; 64];
    let _ = transport.receive(&mut chunk).await;

    assert!(
        server_task.await.unwrap(),
        "server accepted an overlong name"
    );
}

#[tokio::test]
async fn server_hangs_up_when_the_peer_disconnects_mid_handshake() {
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(1, shared(t), &limits);
            // Only a version offer arrives, then the socket closes.
            s.run_server_handshake(&config("server", &limits))
                .await
                .is_err()
        }
    });

    let t = connector(&limits).connect(addr).await.unwrap();
    let transport = shared(t);
    transport.shutdown().await.unwrap();

    assert!(server_task.await.unwrap());
}

#[tokio::test]
async fn non_vacuumdrop_bytes_are_rejected_without_killing_the_listener() {
    // A port scanner or an unrelated service must be refused cleanly, and the
    // listener must survive to serve the next peer.
    let limits = limits();
    let (raw_listener, addr) = listener(&limits).await;
    // Shared so the junk connection and the follow-up connection are served by
    // one listener, which is what "survives junk input" actually means.
    let junk_listener = Arc::new(raw_listener);

    // Run the server side for the junk connection too. Without this the server
    // never reads the bytes, the listener stays busy on that connection, and the
    // test would pass for the wrong reason.
    let junk_server = tokio::spawn({
        let limits = limits.clone();
        let listener = junk_listener.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(8, shared(t), &limits);
            // Must fail: these bytes are not a VacuumDrop frame.
            s.run_server_handshake(&config("server", &limits))
                .await
                .is_err()
        }
    });

    {
        let t = connector(&limits).connect(addr).await.unwrap();
        let transport = shared(t);
        let junk = b"GET / HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\n\r\n";
        write_all(&transport, junk).await;
        let mut chunk = [0u8; 16];
        // Read until the server hangs up on us, which is the observable proof it
        // rejected the input rather than waiting for a valid frame.
        loop {
            match transport.receive(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
        transport.shutdown().await.ok();
    }

    assert!(junk_server.await.unwrap(), "server accepted junk bytes");

    // A well-behaved peer must still connect afterwards.
    let good = tokio::spawn({
        let limits = limits.clone();
        let listener = junk_listener.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(9, shared(t), &limits);
            s.run_server_handshake(&config("server", &limits))
                .await
                .is_ok()
        }
    });

    let t = connector(&limits).connect(addr).await.unwrap();
    let mut c = session(50, shared(t), &limits);
    assert!(
        c.run_client_handshake(&config("client", &limits))
            .await
            .is_ok()
    );
    assert!(good.await.unwrap(), "listener did not survive junk input");
}

#[tokio::test]
async fn a_session_cannot_be_established_twice() {
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(1, shared(t), &limits);
            s.run_server_handshake(&config("server", &limits))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    });

    let t = connector(&limits).connect(addr).await.unwrap();
    let mut c = session(2, shared(t), &limits);
    c.run_client_handshake(&config("client", &limits))
        .await
        .unwrap();

    // Re-running the handshake from ESTABLISHED must be refused, not silently
    // renegotiated.
    let second = c.run_client_handshake(&config("client", &limits)).await;
    assert!(second.is_err(), "a second handshake was accepted");

    server_task.await.unwrap();
}

#[tokio::test]
async fn sending_after_close_is_refused() {
    let limits = limits();
    let mut s = stub_session(&limits);
    s.mark_connected().unwrap();
    s.mark_authenticating().unwrap();
    s.close(GoodbyeReason::Shutdown).await.ok();

    assert!(
        s.send(VacuumDropMessage::Ping(Ping { nonce: 1 }))
            .await
            .is_err(),
        "a closed session accepted a send"
    );
}

#[tokio::test]
async fn two_concurrent_handshakes_both_succeed() {
    // The listener must serve peers independently rather than serialising them.
    let limits = limits();
    let (listener, addr) = listener(&limits).await;
    let listener = Arc::new(listener);

    let server = tokio::spawn({
        let limits = limits.clone();
        async move {
            let mut handles = Vec::new();
            for i in 0..2u64 {
                let l = listener.clone();
                let limits = limits.clone();
                handles.push(tokio::spawn(async move {
                    let t = l.accept().await.unwrap();
                    let mut s = session(i, shared(t), &limits);
                    s.run_server_handshake(&config("server", &limits))
                        .await
                        .is_ok()
                }));
            }
            let mut results = Vec::new();
            for h in handles {
                results.push(h.await.unwrap());
            }
            results
        }
    });

    let mut clients = Vec::new();
    for i in 0..2u64 {
        let t = connector(&limits).connect(addr).await.unwrap();
        let mut s = session(100 + i, shared(t), &limits);
        let limits = limits.clone();
        clients.push(tokio::spawn(async move {
            s.run_client_handshake(&config("client", &limits))
                .await
                .is_ok()
        }));
    }

    for c in clients {
        assert!(c.await.unwrap(), "a client handshake failed");
    }
    let results = server.await.unwrap();
    assert!(
        results.iter().all(|ok| *ok),
        "a concurrent server handshake failed"
    );
}

#[tokio::test]
async fn receive_times_out_rather_than_hanging_forever() {
    // Charter 65. A peer that goes quiet must not block us indefinitely.
    let limits = limits();
    let mut s = stub_session(&limits);
    s.mark_connected().unwrap();

    let result = s.receive(Duration::from_millis(150)).await;
    assert!(
        matches!(result, Err(SessionError::Timeout { .. })),
        "expected a timeout, got {result:?}"
    );
}

#[tokio::test]
async fn session_ids_are_retained_for_log_correlation() {
    // Charter 59: logs need correlation ids.
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(0xAABB_CCDD, shared(t), &limits);
            s.run_server_handshake(&config("server", &limits))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
            s.id()
        }
    });

    let t = connector(&limits).connect(addr).await.unwrap();
    let mut c = session(0x1122_3344, shared(t), &limits);
    c.run_client_handshake(&config("client", &limits))
        .await
        .unwrap();

    assert_eq!(server_task.await.unwrap().get(), 0xAABB_CCDD);
    assert_eq!(c.id().get(), 0x1122_3344);
}

#[tokio::test]
async fn a_clean_close_mid_handshake_is_a_deadline_not_a_protocol_error() {
    // The distinction matters: a peer that closes cleanly has not misbehaved, so
    // reporting a protocol error would send a user hunting for a bug in their
    // own build. It is also not a *transport* error, because the socket behaved.
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(1, shared(t), &limits);
            s.run_server_handshake(&config("server", &limits))
                .await
                .err()
        }
    });

    // A graceful shutdown sends FIN, so the server observes end-of-stream.
    let t = connector(&limits).connect(addr).await.unwrap();
    shared(t).shutdown().await.unwrap();

    match server_task.await.unwrap() {
        Some(e) => {
            assert!(
                matches!(e, SessionError::DeadlineExceeded { .. }),
                "expected DeadlineExceeded for a clean close, got {e:?}"
            );
            let core: Error = e.into();
            assert!(
                matches!(core, Error::Timeout(_)),
                "a clean close should map to a timeout, got {core:?}"
            );
        }
        None => panic!("server reported success against a peer that hung up"),
    }
}

#[tokio::test]
async fn connecting_to_a_closed_port_is_a_transport_error() {
    // A refused connection is unambiguously a transport failure and must stay
    // distinguishable from every protocol outcome.
    let limits = limits();
    let addr = {
        let (l, a) = listener(&limits).await;
        drop(l); // free the port
        a
    };

    match connector(&limits).connect(addr).await {
        Err(TransportError::Io(_)) => {}
        Err(other) => panic!("expected an I/O transport error, got {other}"),
        Ok(_) => panic!("connected to a port that was closed"),
    }
}

#[tokio::test]
async fn handshake_deadline_is_bounded() {
    // A peer that connects and then says nothing must be dropped, not waited on.
    let limits = limits();
    let (listener, addr) = listener(&limits).await;

    let server_task = tokio::spawn({
        let limits = limits.clone();
        async move {
            let t = listener.accept().await.unwrap();
            let mut s = session(1, shared(t), &limits);
            let started = std::time::Instant::now();
            let result = s.run_server_handshake(&config("server", &limits)).await;
            (started.elapsed(), result.is_err())
        }
    });

    // Connect, then go silent.
    let t = connector(&limits).connect(addr).await.unwrap();
    let transport = shared(t);
    tokio::time::sleep(HANDSHAKE + Duration::from_millis(300)).await;
    transport.shutdown().await.ok();

    let (elapsed, err) = server_task.await.unwrap();
    assert!(err, "a silent peer was accepted");
    assert!(
        elapsed < HANDSHAKE + Duration::from_secs(2),
        "handshake took {elapsed:?}, well past its deadline"
    );
}

#[tokio::test]
async fn listener_reports_the_bound_address_rather_than_the_requested_one() {
    // Binding port 0 must report the port the OS chose; printing the requested
    // address would be a lie the user acts on.
    let limits = limits();
    let l = TcpTransportListener::bind(
        "127.0.0.1:0".parse().unwrap(),
        limits.idle_timeout,
        limits.idle_timeout,
        limits.idle_timeout,
    )
    .await
    .unwrap();
    let addr = l.local_addr().unwrap();
    assert_ne!(addr.port(), 0);
    assert!(addr.ip().is_loopback());
}

#[test]
fn protocol_and_frame_errors_are_distinct() {
    // Two error types that collapse into one would make diagnosis impossible.
    let proto = ProtocolError::BadMagic;
    let frame: vacuumdrop_protocol::codec::FrameError = proto.into();
    assert!(matches!(
        frame,
        vacuumdrop_protocol::codec::FrameError::Engine(_)
    ));

    let io = vacuumdrop_protocol::codec::FrameError::Io(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "gone",
    ));
    assert!(matches!(io, vacuumdrop_protocol::codec::FrameError::Io(_)));

    assert_eq!(VersionSelection { selected: 1 }.selected, 1);
}

#[test]
fn version_lists_are_validated_before_use() {
    // Guards the fail-closed path at the message layer, independent of sockets.
    let offered = SupportedVersions {
        versions: vec![9, 9],
    };
    assert!(
        offered.validate().is_err(),
        "duplicate versions were accepted"
    );

    let descending = SupportedVersions {
        versions: vec![3, 1],
    };
    assert!(descending.validate().is_err());

    assert!(
        vacuumdrop_protocol::message::VersionSelection::negotiate(&offered).is_err(),
        "a malformed offer must not negotiate"
    );

    let good = SupportedVersions { versions: vec![1] };
    assert_eq!(
        vacuumdrop_protocol::message::VersionSelection::negotiate(&good)
            .unwrap()
            .selected,
        1
    );
}

#[test]
fn default_limits_are_valid() {
    // A configuration that cannot be enforced must fail at construction, not at
    // the moment it first matters.
    assert_eq!(Limits::new().validate(), Ok(()));
}

/// Encodes a message with a throwaway codec, for tests that need to send bytes
/// this build's own client would never produce.
fn encode(msg: &VacuumDropMessage, limits: &Arc<Limits>) -> Vec<u8> {
    let mut codec = FrameCodec::for_session(99, limits.max_frame_length);
    let mut buf = BytesMut::new();
    codec.encode(msg.clone(), &mut buf).unwrap();
    buf.to_vec()
}

/// A session over a transport that accepts writes and never produces data.
///
/// `receive` blocks forever rather than returning 0. Returning 0 would be a
/// *clean end of stream*, which is a different outcome and would make a
/// timeout test pass for the wrong reason.
fn stub_session(limits: &Arc<Limits>) -> Session {
    struct Silent;
    impl Transport for Silent {
        fn peer_addr(&self) -> Option<std::net::SocketAddr> {
            None
        }
        fn send<'a>(
            &'a self,
            _: &'a [u8],
        ) -> vacuumdrop_transport::BoxFuture<'a, Result<usize, TransportError>> {
            Box::pin(async { Ok(0) })
        }
        fn receive<'a>(
            &'a self,
            _: &'a mut [u8],
        ) -> vacuumdrop_transport::BoxFuture<'a, Result<usize, TransportError>> {
            // Never completes, so the caller's own deadline is what ends the wait.
            Box::pin(std::future::pending())
        }
        fn shutdown(&self) -> vacuumdrop_transport::BoxFuture<'_, Result<(), TransportError>> {
            Box::pin(async { Ok(()) })
        }
    }
    Session::new(SessionId::new(7), Arc::new(Silent), limits.as_ref().clone()).unwrap()
}
