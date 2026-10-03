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

use crate::cli::{Cli, Command, WifiDirect, usage};

/// Exit codes, following the usual convention.
const EXIT_OK: u8 = 0;
const EXIT_FAILURE: u8 = 1;
const EXIT_TIMEOUT: u8 = 3;

/// Anything a subcommand can fail with.
///
/// The platform adapter's error type is a variant here rather than being folded
/// into [`SessionError`]. Folding it in would put a platform concept inside the
/// protocol crate's error surface, which is the coupling the layering exists to
/// prevent. An application may know about both platforms and protocols; a library
/// may not.
#[derive(Debug)]
enum CliError {
    Session(SessionError),
    #[cfg(all(windows, feature = "wifi-direct"))]
    Adapter(vacuumdrop_platform_windows::AdapterError),
    /// The command needs a build flag that was not set.
    ///
    /// Exists only when Wi-Fi Direct is compiled out. Gating the variant (not just
    /// its construction site) means a build with the feature on has no dead
    /// "not supported" path to reach.
    #[cfg(not(all(windows, feature = "wifi-direct")))]
    Unsupported(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Session(e) => write!(f, "{e}"),
            #[cfg(all(windows, feature = "wifi-direct"))]
            CliError::Adapter(e) => write!(f, "{e}"),
            #[cfg(not(all(windows, feature = "wifi-direct")))]
            CliError::Unsupported(m) => write!(f, "{m}"),
        }
    }
}

impl From<SessionError> for CliError {
    fn from(e: SessionError) -> Self {
        CliError::Session(e)
    }
}

impl From<vacuumdrop_core::TransportError> for CliError {
    fn from(e: vacuumdrop_core::TransportError) -> Self {
        // Routed through `SessionError::Transport`, whose `#[from]` already exists.
        // Recording it as a session error is accurate here: a transport failure
        // while connecting is what the user asked about.
        CliError::Session(SessionError::from(e))
    }
}

#[cfg(all(windows, feature = "wifi-direct"))]
impl From<vacuumdrop_platform_windows::AdapterError> for CliError {
    fn from(e: vacuumdrop_platform_windows::AdapterError) -> Self {
        CliError::Adapter(e)
    }
}

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
        Command::Listen { bind } => listen(*bind, &cli).await.map_err(CliError::from),
        Command::Connect { addr } => connect(*addr, &cli).await.map_err(CliError::from),
        Command::Ping { addr } => ping(*addr, &cli).await.map_err(CliError::from),
        Command::WifiDirect(action) => match action {
            WifiDirect::Probe => wifi_direct_probe(&cli).await,
            WifiDirect::Discover => wifi_direct_discover(&cli).await,
            WifiDirect::Advertise { seconds, intensive } => {
                wifi_direct_advertise(*seconds, *intensive, &cli).await
            }
            WifiDirect::Connect { device_id } => wifi_direct_connect(device_id, &cli).await,
        },
    };

    match result {
        Ok(()) => ok(),
        Err(e) => {
            eprintln!("error: {e}");
            match &e {
                CliError::Session(SessionError::Timeout { .. })
                | CliError::Session(SessionError::DeadlineExceeded { .. }) => {
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

/// Becomes discoverable and waits to be connected to.
///
/// This is the *host* half of the two-device test. Without it the initiator has
/// nothing to connect to, which is why `connect` alone could never have been a
/// complete acceptance procedure.
///
/// Accepting the incoming connection uses **autonomous group owner** mode, which
/// exists precisely so a device can join a group without hosting an event handler.
/// That keeps the host side free of a COM callback, and therefore free of a message
/// pump, and is the same reason the direct `windows` binding was viable at all.
/// Makes this device discoverable and holds the advertisement.
///
/// This is the *host* half of the two-device test. Without it the initiator has
/// nothing to connect to, which is why `connect` alone was never a complete
/// procedure.
///
/// # What this deliberately does NOT do
///
/// It does not serve a session itself. Doing that needs this side to learn the P2P
/// interface's own address, which means enumerating `NetworkInformation` adapters -
/// a second platform subsystem, added with no way to test it here. Instead it
/// prints the `p2p listen` command to run alongside, which is a real, already-tested
/// code path.
///
/// That is the honest trade: every step of the procedure this prints has been run at
/// least once, rather than a smoother command that would be the first thing ever
/// executed on real hardware.
async fn wifi_direct_advertise(seconds: u64, intensive: bool, cli: &Cli) -> Result<(), CliError> {
    #[cfg(all(windows, feature = "wifi-direct"))]
    {
        use vacuumdrop_platform_windows::discovery;

        let config = config(&cli.device_name)?;
        let port = crate::cli::default_bind().port();

        let mut guard = discovery::advertise(intensive)?;
        println!("discoverable as `{}`", config.device_name);
        println!(
            "  for {seconds}s, {} discoverability",
            if intensive { "INTENSIVE" } else { "normal" }
        );
        println!();
        println!("In a SECOND terminal on this device, run:");
        println!("  p2p listen --addr 0.0.0.0:{port}");
        println!();
        println!("Then on the OTHER device:");
        println!("  p2p wifi-direct discover");
        println!("  p2p wifi-direct connect <device-id>");
        println!();
        println!(
            "note: `listen --addr 0.0.0.0` also accepts connections from your normal\n\
             network. That is acceptable for a test rig and is NOT the shipping\n\
             configuration, which will bind only the P2P interface."
        );

        // Sleep rather than poll: there is nothing to poll for, because the session
        // is served by the `p2p listen` in the other terminal. The user's window is
        // the deadline, so this always terminates on its own.
        tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;

        guard.stop();
        println!();
        println!("advertisement stopped.");
        Ok(())
    }
    #[cfg(not(all(windows, feature = "wifi-direct")))]
    {
        let _ = (seconds, intensive, cli);
        Err(wifi_direct_needs_feature())
    }
}

/// Explains that Wi-Fi Direct was not compiled into this binary.
///
/// A dedicated path rather than falling through to `UnknownCommand`, because the
/// user typed something we recognise. Naming the exact build flag is the
/// difference between a five-second fix and a confused user.
#[cfg(not(all(windows, feature = "wifi-direct")))]
fn wifi_direct_needs_feature() -> CliError {
    CliError::Unsupported(format!(
        "this build has no Wi-Fi Direct support.\n\
         Rebuild on Windows with:\n    \
         cargo run --features wifi-direct --bin p2p -- wifi-direct <action>\n\
         ({}).",
        crate::cli::wifi_direct_flag()
    ))
}

/// Reports what this machine's Wi-Fi Direct stack supports.
async fn wifi_direct_probe(cli: &Cli) -> Result<(), CliError> {
    #[cfg(all(windows, feature = "wifi-direct"))]
    {
        use vacuumdrop_platform_windows::Capability;

        let adapter = vacuumdrop_platform_windows::Adapter::new(&cli.device_name);
        println!("probing Wi-Fi Direct...");
        match adapter.detect_capability() {
            Capability::Available { adapter } => {
                println!("VERDICT: available ({adapter})");
                println!("  a Wi-Fi Direct group could be formed as group owner");
            }
            Capability::NoAdapter => {
                println!("VERDICT: no adapter");
                println!("  Windows returned an empty Wi-Fi Direct selector.");
                println!("  check that Wi-Fi is enabled and not blocked by policy");
            }
            Capability::Disabled { adapter, reason } => {
                println!("VERDICT: disabled ({adapter})");
                println!("  reason: {reason}");
                println!("  this is usually policy, not missing hardware");
            }
            Capability::ProbeFailed { adapter, error } => {
                println!("VERDICT: probe failed ({adapter})");
                println!("  error: {error}");
            }
        }
        println!();
        println!(
            "NOTE: availability is a per-adapter, per-driver property.\n\
             It is not a claim that any specific peer will connect."
        );
        Ok(())
    }
    #[cfg(not(all(windows, feature = "wifi-direct")))]
    {
        let _ = cli;
        Err(wifi_direct_needs_feature())
    }
}

/// How long Wi-Fi Direct group formation is allowed to take.
///
/// A fixed constant rather than a `--timeout` flag for now: the Phase 3 acceptance
/// run is a manual two-device procedure, and a knob nobody has calibrated yet is
/// worse than an honest fixed budget. Revisit once real formation timings exist.
///
/// It is still bounded. Charter 65 forbids waiting forever, and an interactive
/// command that can hang indefinitely is worse than one that gives up and says so.
///
/// Gated to the builds that use it: the Wi-Fi Direct path, and the test module -
/// which asserts the budget stays sane even in a build where the path is compiled
/// out. Without the `test` arm this is dead code in a default build.
#[cfg(any(all(windows, feature = "wifi-direct"), test))]
const GROUP_FORMATION_BUDGET: std::time::Duration = std::time::Duration::from_secs(45);

/// Lists nearby peers.
async fn wifi_direct_discover(_cli: &Cli) -> Result<(), CliError> {
    #[cfg(all(windows, feature = "wifi-direct"))]
    {
        println!("scanning for Wi-Fi Direct peers...");
        let report = vacuumdrop_platform_windows::discovery::scan(
            vacuumdrop_platform_windows::DEFAULT_DISCOVERY_TIMEOUT,
        )
        .await?;

        // Report every selector, including the ones that matched nothing. Which
        // selector sees a peer is the single most useful diagnostic when discovery
        // fails, and hiding the zeroes would discard exactly the evidence needed.
        if !report.per_selector.is_empty() {
            println!("selectors:");
            for s in &report.per_selector {
                match &s.error {
                    None => println!("  {:<16} {} device(s)", s.label, s.count),
                    Some(reason) => println!("  {:<16} unavailable ({reason})", s.label),
                }
            }
            println!();
        }

        if report.selector_empty {
            // Distinguish "no peers nearby" from "this machine offers no Wi-Fi
            // Direct devices at all". They look identical in a bare list and lead
            // to completely different fixes.
            println!("this machine reports NO Wi-Fi Direct devices to enumerate.");
            println!("  that is a radio / driver / policy condition, not an empty room.");
            println!("  check `p2p wifi-direct probe` for the specific verdict.");
            return Ok(());
        }

        if report.peers.is_empty() {
            println!("scan completed: no peers nearby.");
            println!();
            println!("A peer only appears if it is ACTIVELY ADVERTISING, and a");
            println!("device never discovers itself. So this is the expected result");
            println!("when the other machine is not running `advertise`.");
            println!();
            println!("On the OTHER device, run:");
            println!("  p2p wifi-direct advertise 300");
            println!();
            println!("then run this command again. Allow ~5s after `advertise`");
            println!("starts before scanning.");
        } else {
            println!("scan completed: {} peer(s) found", report.peers.len());
        }
        for peer in &report.peers {
            // Wi-Fi Direct carries no friendly name, so the device id is the label.
            println!("  {}", peer.device_id);
            if peer.display_name != peer.device_id {
                println!("    name: {}", peer.display_name);
            }
        }
        if !report.peers.is_empty() {
            println!();
            println!("Connect with:");
            println!("  p2p wifi-direct connect <device-id>");
        }
        Ok(())
    }
    #[cfg(not(all(windows, feature = "wifi-direct")))]
    {
        let _ = _cli;
        Err(wifi_direct_needs_feature())
    }
}

/// Forms a group with a peer, then negotiates a session across it.
///
/// # The point of this command
///
/// Wi-Fi Direct supplies **an address**. Everything after that is the existing
/// TCP transport and the existing session code, unchanged. That is deliberate:
/// the roadmap requires the protocol layer to need no changes, and this is the
/// demonstration. If a future edit makes this function reach past
/// [`vacuumdrop_transport`] into radio concepts, the layering has eroded.
async fn wifi_direct_connect(device_id: &str, cli: &Cli) -> Result<(), CliError> {
    #[cfg(all(windows, feature = "wifi-direct"))]
    {
        let config = config(&cli.device_name)?;
        let limits = config.limits.clone();

        // Group formation gets its own deadline, deliberately NOT the protocol's
        // `connect_timeout`.
        //
        // `limits.connect_timeout` is 10s, sized for a TCP connect on a LAN where
        // the peer is one hop away. Wi-Fi Direct group formation is peer discovery,
        // group-owner negotiation, and DHCP - routinely longer than that. Reusing
        // the TCP value meant the adapter's own 30s default was being overridden
        // down to a value that cannot reliably cover formation, and the resulting
        // failure reads like "Wi-Fi Direct is broken" rather than "the deadline was
        // too tight".
        //
        // These are genuinely different operations with different costs, so they get
        // different budgets.
        let formation_timeout = GROUP_FORMATION_BUDGET;

        let adapter = vacuumdrop_platform_windows::Adapter::new(&cli.device_name)
            .connect_timeout(formation_timeout);

        println!("forming Wi-Fi Direct group with {device_id}...");
        println!("  (group formation budget: {formation_timeout:?})");

        let info = adapter.connect_by_id(device_id).await?;
        let peer_addr = info.socket_addr();
        println!("group formed");
        println!("  peer address : {peer_addr}");

        // From here on this is the ordinary Phase 1 path. The radio has done its
        // only job: producing an address a socket can be opened to.
        let connector = TcpConnector::new(
            limits.connect_timeout,
            limits.idle_timeout,
            limits.idle_timeout,
        );
        println!("connecting over TCP...");
        let transport = connector.connect(peer_addr).await?;
        let transport: Arc<dyn Transport> = Arc::from(transport);
        println!("connected: {}", transport.describe());

        let mut session = Session::new(next_session_id(), transport, limits.clone())?;
        println!("negotiating protocol version...");
        let params = session.run_client_handshake(&config).await?;

        println!();
        println!("session established over Wi-Fi Direct");
        println!("  session id  : {}", session.id());
        println!("  version     : {}", params.version);
        println!("  peer        : {}", params.peer.display_name());
        println!("  chunk size  : {} bytes", params.chunk_size());
        println!("  link        : Wi-Fi Direct (direct P2P, no access point)");
        println!();
        println!(
            "NOTE: the radio link is direct, but this session is still\n\
             UNENCRYPTED and UNAUTHENTICATED. Being on the same P2P link\n\
             proves proximity, not identity."
        );

        session.close(GoodbyeReason::Shutdown).await?;
        adapter.shutdown();
        Ok(())
    }
    #[cfg(not(all(windows, feature = "wifi-direct")))]
    {
        let _ = (device_id, cli);
        Err(wifi_direct_needs_feature())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_formation_budget_exceeds_the_protocol_connect_timeout() {
        // Regression guard. Group formation was originally given
        // `limits.connect_timeout` (10s, sized for a one-hop TCP connect), which
        // silently overrode the adapter's 30s default down to a value that cannot
        // reliably cover discovery + GO negotiation + DHCP. The failure mode looked
        // like "Wi-Fi Direct is broken" rather than "the deadline was too tight".
        let protocol_connect = config("ok").unwrap().limits.connect_timeout;
        assert!(
            GROUP_FORMATION_BUDGET > protocol_connect,
            "formation budget {GROUP_FORMATION_BUDGET:?} must exceed the protocol's \
             TCP connect timeout {protocol_connect:?}"
        );
    }

    #[test]
    fn group_formation_budget_is_bounded() {
        // Charter 65: never wait forever. An interactive command that hangs
        // indefinitely is worse than one that fails with a message.
        assert!(GROUP_FORMATION_BUDGET <= std::time::Duration::from_secs(120));
        assert!(GROUP_FORMATION_BUDGET >= std::time::Duration::from_secs(20));
    }

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
