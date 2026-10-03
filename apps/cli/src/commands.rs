//! Command implementations.
//!
//! Each subcommand drives a real [`Session`] over a real [`Transport`], so the
//! code exercised by `p2p connect` is the same code the integration tests drive.
//! No shortcuts, and no mock path that could pass while the real one is broken.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use vacuumdrop_core::SessionId;
use vacuumdrop_protocol::VacuumDropMessage;
use vacuumdrop_protocol::message::GoodbyeReason;
use vacuumdrop_protocol::session::{Session, SessionConfig, SessionError};
use vacuumdrop_transport::tcp::{TcpConnector, TcpTransportListener};
use vacuumdrop_transport::{Connector, Transport, TransportListener};

use crate::cli::{Cli, Command, usage};

/// Exit codes, following the usual convention.
const EXIT_OK: u8 = 0;
const EXIT_FAILURE: u8 = 1;
const EXIT_TIMEOUT: u8 = 3;

/// Runs the parsed command.
pub async fn run(cli: Cli) -> std::process::ExitCode {
    let result = match &cli.command {
        Command::Help => {
            println!("{}", usage());
            return ok();
        }
        Command::Version => {
            println!("p2p {}", env!("CARGO_PKG_VERSION"));
            println!("protocol {}", vacuumdrop_protocol::VERSION);
            println!("phase 1 - handshake only, unencrypted and unauthenticated");
            return ok();
        }
        Command::Listen { bind } => listen(*bind, &cli).await,
        Command::Connect { addr } => connect(*addr, &cli).await,
        Command::Ping { addr } => ping(*addr, &cli).await,
    };

    match result {
        Ok(()) => ok(),
        Err(e) => {
            eprintln!("error: {e}");
            match e {
                SessionError::Timeout { .. } | SessionError::DeadlineExceeded { .. } => {
                    std::process::ExitCode::from(EXIT_TIMEOUT)
                }
                _ => std::process::ExitCode::from(EXIT_FAILURE),
            }
        }
    }
}

fn ok() -> std::process::ExitCode {
    std::process::ExitCode::from(EXIT_OK)
}

fn config(device_name: &str) -> Result<SessionConfig, SessionError> {
    let c = SessionConfig::new(device_name);
    c.validate()?;
    Ok(c)
}

/// Session ids are per-process sequential.
///
/// A counter rather than a random value: ids exist to correlate log lines within
/// one run, and a predictable counter is easier to read in a capture.
fn next_session_id() -> SessionId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    SessionId::new(COUNTER.fetch_add(1, Ordering::Relaxed))
}

async fn listen(bind: SocketAddr, cli: &Cli) -> Result<(), SessionError> {
    let config = config(&cli.device_name)?;
    let limits = config.limits.clone();

    let listener = TcpTransportListener::bind(
        bind,
        limits.idle_timeout,
        limits.idle_timeout,
        limits.idle_timeout,
    )
    .await?;

    // Report the address actually bound. With port 0 the OS chose the port, and
    // printing the requested address would be a lie.
    let actual = listener.local_addr()?;
    println!("listening on {actual}");
    println!("device name : {}", config.device_name);
    println!("waiting for a peer...");
    println!();

    let transport = listener.accept().await?;
    let transport: Arc<dyn Transport> = Arc::from(transport);
    println!("peer connected: {}", transport.describe());
    println!("negotiating protocol version...");

    // Capture the description before moving the handle into the session: the
    // session owns the transport, so the caller keeps only what it printed.
    let transport_desc = transport.describe();
    let mut session = Session::new(next_session_id(), transport, limits.clone())?;
    let params = session.run_server_handshake(&config).await?;

    println!();
    println!("session established");
    println!("  session id  : {}", session.id());
    println!("  version     : {}", params.version);
    println!("  peer        : {}", params.peer.display_name());
    println!("  chunk size  : {} bytes", params.chunk_size());
    println!("  transport   : {transport_desc}");
    println!();
    println!(
        "NOTE: this session is UNENCRYPTED and UNAUTHENTICATED.\n\
         Version negotiation is not trust. Any device that reached this\n\
         socket was accepted without proof of identity."
    );

    session.close(GoodbyeReason::Shutdown).await?;
    Ok(())
}

async fn connect(addr: SocketAddr, cli: &Cli) -> Result<(), SessionError> {
    let config = config(&cli.device_name)?;
    let limits = config.limits.clone();

    let connector = TcpConnector::new(
        limits.connect_timeout,
        limits.idle_timeout,
        limits.idle_timeout,
    );
    println!("connecting to {addr}...");
    let transport = connector.connect(addr).await?;
    let transport: Arc<dyn Transport> = Arc::from(transport);
    println!("connected: {}", transport.describe());

    let mut session = Session::new(next_session_id(), transport, limits.clone())?;
    println!("negotiating protocol version...");

    let params = session.run_client_handshake(&config).await?;

    println!();
    println!("session established");
    println!("  session id  : {}", session.id());
    println!("  version     : {}", params.version);
    println!("  peer        : {}", params.peer.display_name());
    println!("  chunk size  : {} bytes", params.chunk_size());
    println!();
    println!(
        "NOTE: this session is UNENCRYPTED and UNAUTHENTICATED.\n\
         Version negotiation is not trust."
    );

    session.close(GoodbyeReason::Shutdown).await?;
    Ok(())
}

async fn ping(addr: SocketAddr, cli: &Cli) -> Result<(), SessionError> {
    let config = config(&cli.device_name)?;
    let limits = config.limits.clone();

    let connector = TcpConnector::new(
        limits.connect_timeout,
        limits.idle_timeout,
        limits.idle_timeout,
    );
    let transport = connector.connect(addr).await?;
    let mut session = Session::new(
        next_session_id(),
        Arc::from(transport) as Arc<dyn Transport>,
        limits.clone(),
    )?;

    let params = session.run_client_handshake(&config).await?;
    println!("session established with {}", params.peer.display_name());

    // A nonce derived from the session id keeps the probe correlatable without
    // needing a random source for a value that carries no security weight.
    let nonce = session.id().get() ^ 0x504E_4700;
    let timeout = limits.idle_timeout.min(Duration::from_secs(10));

    session.ping(nonce).await?;

    match session.receive(timeout).await? {
        Some(VacuumDropMessage::Pong(p)) if p.nonce == nonce => {
            println!(
                "pong from {} (nonce {:#x})",
                params.peer.display_name(),
                p.nonce
            );
            session.close(GoodbyeReason::Complete).await?;
            Ok(())
        }
        Some(VacuumDropMessage::Pong(p)) => {
            // A Pong answering a different probe. The peer is alive but not
            // answering us, which is worth saying plainly rather than reporting
            // as a generic protocol fault.
            let _ = session.close(GoodbyeReason::Idle).await;
            Err(SessionError::Protocol(
                vacuumdrop_core::ProtocolError::UnexpectedMessage {
                    message: "Pong with a nonce we did not send",
                    state: "Pong",
                },
            ))
            .inspect(|_| {
                eprintln!("  expected nonce {nonce:#x}, received {:#x}", p.nonce);
            })
        }
        Some(other) => {
            // `listen` in Phase 1 completes one handshake and closes, so a
            // Goodbye here is the expected outcome, not a fault. Saying so is
            // the difference between a confusing error and an accurate one.
            let _ = session.close(GoodbyeReason::Idle).await;
            if let VacuumDropMessage::Goodbye(g) = other {
                eprintln!(
                    "peer said goodbye ({:?}) before answering the probe.",
                    g.reason
                );
                Err(SessionError::Refused("peer closed the session"))
            } else {
                Err(SessionError::Protocol(
                    vacuumdrop_core::ProtocolError::UnexpectedMessage {
                        message: other.name(),
                        state: "Pong",
                    },
                ))
            }
        }
        None => {
            let _ = session.close(GoodbyeReason::Idle).await;
            Err(SessionError::Timeout {
                operation: "Pong",
                limit: timeout,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_unique_per_process() {
        let a = next_session_id();
        let b = next_session_id();
        let c = next_session_id();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert!(a.get() < b.get() && b.get() < c.get());
    }

    #[test]
    fn config_rejects_a_bad_device_name() {
        assert!(config("").is_err());
        assert!(config("ok").is_ok());
    }

    #[test]
    fn connecting_to_a_dead_port_fails_quickly() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            // Bind then drop to obtain a port that is almost certainly free.
            let l = TcpTransportListener::bind(
                crate::cli::default_bind(),
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_millis(200),
            )
            .await
            .unwrap();
            let addr = l.local_addr().unwrap();
            drop(l);

            let cli = Cli {
                command: Command::Connect { addr },
                device_name: "test".into(),
            };
            let result = run(cli).await;
            assert_eq!(result, std::process::ExitCode::from(EXIT_FAILURE));
        });
    }

    #[test]
    fn help_and_version_succeed() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            for cmd in [Command::Help, Command::Version] {
                let cli = Cli {
                    command: cmd,
                    device_name: "test".into(),
                };
                assert_eq!(run(cli).await, std::process::ExitCode::from(EXIT_OK));
            }
        });
    }
}
