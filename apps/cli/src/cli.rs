//! Command-line parsing.
//!
//! Hand-rolled rather than using clap. The justification is in `Cargo.toml`; the
//! short version is that the Phase 1 surface is small enough that a dependency
//! would cost more than it saves, and that this module is fully tested.
//!
//! Parsing never panics and never exits. Every failure is a value the caller
//! renders, so `--help` and error output stay consistent and the whole surface is
//! testable without spawning a process.

use std::net::SocketAddr;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Print usage.
    Help,
    /// Print the build version.
    Version,
    /// Accept incoming sessions.
    Listen {
        /// Address to bind.
        bind: SocketAddr,
    },
    /// Connect to a peer and complete a handshake.
    Connect {
        /// Peer address.
        addr: SocketAddr,
    },
    /// Send a keepalive probe.
    Ping {
        /// Peer address.
        addr: SocketAddr,
    },
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    /// The subcommand.
    pub command: Command,
    /// This device's declared name.
    pub device_name: String,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            command: Command::Help,
            device_name: default_device_name(),
        }
    }
}

/// The name reported when none is given.
pub fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
        .unwrap_or_else(|| "vacuumdrop".to_owned())
}

/// Why a command line could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The subcommand is not one this build supports.
    ///
    /// No "missing command" case exists: bare `p2p` shows help rather than
    /// failing, which is the conventional and less hostile behaviour.
    UnknownCommand(String),
    /// A flag was given without its value.
    MissingValue(&'static str),
    /// The value was not a valid socket address.
    BadAddress(String),
    /// A flag was not recognised for this subcommand.
    UnknownFlag {
        /// The subcommand in effect.
        command: &'static str,
        /// The offending flag.
        flag: String,
    },
    /// The same flag was given twice.
    DuplicateFlag(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnknownCommand(c) => write!(f, "unknown command `{c}`"),
            ParseError::MissingValue(v) => write!(f, "{v} requires a value"),
            ParseError::BadAddress(a) => write!(
                f,
                "`{a}` is not a valid address (expected host:port, e.g. 127.0.0.1:9000)"
            ),
            ParseError::UnknownFlag { command, flag } => {
                write!(f, "`{flag}` is not valid for `{command}`")
            }
            ParseError::DuplicateFlag(v) => write!(f, "{v} given more than once"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses arguments, excluding the program name.
pub fn parse<I, S>(args: I) -> Result<Cli, ParseError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    let mut iter = args.into_iter();

    let Some(first) = iter.next() else {
        return Ok(Cli::default());
    };

    // A leading flag with no subcommand is a user mistake worth reporting
    // rather than silently showing help for.
    if first.starts_with('-') {
        if first == "-h" || first == "--help" {
            return Ok(Cli {
                command: Command::Help,
                device_name: default_device_name(),
            });
        }
        if first == "-V" || first == "--version" {
            return Ok(Cli {
                command: Command::Version,
                device_name: default_device_name(),
            });
        }
        return Err(ParseError::UnknownCommand(first));
    }

    let name = default_device_name();
    let mut device_name = name.clone();
    let mut addr: Option<SocketAddr> = None;

    // Collect global flags first, then the subcommand's positional argument.
    let command = match first.as_str() {
        "help" => Command::Help,
        "version" => Command::Version,
        "listen" | "connect" | "ping" => {
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--name" => {
                        if device_name != name {
                            return Err(ParseError::DuplicateFlag("--name"));
                        }
                        device_name = iter.next().ok_or(ParseError::MissingValue("--name"))?;
                    }
                    "--addr" => {
                        if addr.is_some() {
                            return Err(ParseError::DuplicateFlag("--addr"));
                        }
                        let raw = iter.next().ok_or(ParseError::MissingValue("--addr"))?;
                        addr = Some(raw.parse().map_err(|_| ParseError::BadAddress(raw))?);
                    }
                    other => {
                        let command: &'static str = match first.as_str() {
                            "listen" => "listen",
                            "connect" => "connect",
                            _ => "ping",
                        };
                        if other.starts_with('-') {
                            return Err(ParseError::UnknownFlag {
                                command,
                                flag: other.to_owned(),
                            });
                        }
                        addr = Some(
                            other
                                .parse()
                                .map_err(|_| ParseError::BadAddress(other.to_owned()))?,
                        );
                    }
                }
            }
            match first.as_str() {
                "listen" => Command::Listen {
                    bind: addr.unwrap_or_else(default_bind),
                },
                "connect" => Command::Connect {
                    addr: addr.ok_or(ParseError::MissingValue("--addr"))?,
                },
                _ => Command::Ping {
                    addr: addr.ok_or(ParseError::MissingValue("--addr"))?,
                },
            }
        }
        other => return Err(ParseError::UnknownCommand(other.to_owned())),
    };

    // `--name` for help/version is still accepted but has no effect.
    if matches!(command, Command::Help | Command::Version) {
        device_name = name;
    }

    Ok(Cli {
        command,
        device_name,
    })
}

/// The default bind address.
///
/// Loopback, deliberately. Binding to all interfaces by default would expose an
/// unauthenticated, unencrypted Phase 1 protocol to the network without the user
/// asking for it (Charter 43, and the absence of authentication in Phase 1).
pub fn default_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 9000))
}

/// The usage text.
pub fn usage() -> String {
    format!(
        "p2p {version} - offline peer-to-peer engine (Phase 1: handshake only)

USAGE:
    p2p <command> [options]

COMMANDS:
    listen [--addr <host:port>]   Accept incoming sessions
    connect <host:port>          Connect to a peer and negotiate a session
    ping <host:port>             Send a keepalive probe
    help                         Show this help
    version                      Show the build version

OPTIONS:
    --name <name>                Device name reported during the handshake
    -h, --help                   Show this help
    -V, --version                Show the version

DEFAULT:
    listen binds {bind}

NOTES:
    Phase 1 sessions are UNENCRYPTED and UNAUTHENTICATED. Version negotiation
    proves both peers speak the same protocol; it does not establish trust.
    Encryption and authentication arrive in Phase 4.

EXAMPLES:
    p2p listen --addr 127.0.0.1:9000
    p2p connect 127.0.0.1:9000
    p2p ping 127.0.0.1:9000 --name \"Ian's Laptop\"
",
        version = env!("CARGO_PKG_VERSION"),
        bind = default_bind()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cli, ParseError> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_arguments_shows_help() {
        assert_eq!(p(&[]).unwrap().command, Command::Help);
    }

    #[test]
    fn help_flag_before_any_command() {
        assert_eq!(p(&["--help"]).unwrap().command, Command::Help);
        assert_eq!(p(&["-h"]).unwrap().command, Command::Help);
    }

    #[test]
    fn version_flag_and_subcommand() {
        assert_eq!(p(&["--version"]).unwrap().command, Command::Version);
        assert_eq!(p(&["-V"]).unwrap().command, Command::Version);
        assert_eq!(p(&["version"]).unwrap().command, Command::Version);
    }

    #[test]
    fn help_subcommand() {
        assert_eq!(p(&["help"]).unwrap().command, Command::Help);
    }

    #[test]
    fn unknown_command_is_reported() {
        assert_eq!(
            p(&["frobnicate"]).unwrap_err(),
            ParseError::UnknownCommand("frobnicate".into())
        );
    }

    #[test]
    fn unknown_leading_flag_is_reported() {
        assert_eq!(
            p(&["--wat"]).unwrap_err(),
            ParseError::UnknownCommand("--wat".into())
        );
    }

    #[test]
    fn listen_defaults_to_loopback() {
        // Defaulting to loopback avoids exposing an unauthenticated protocol to
        // the network by accident.
        match p(&["listen"]).unwrap().command {
            Command::Listen { bind } => assert!(bind.ip().is_loopback()),
            other => panic!("expected Listen, got {other:?}"),
        }
    }

    #[test]
    fn listen_accepts_explicit_address() {
        match p(&["listen", "--addr", "0.0.0.0:1234"]).unwrap().command {
            Command::Listen { bind } => assert_eq!(bind.port(), 1234),
            other => panic!("expected Listen, got {other:?}"),
        }
    }

    #[test]
    fn connect_accepts_positional_address() {
        match p(&["connect", "127.0.0.1:9000"]).unwrap().command {
            Command::Connect { addr } => assert_eq!(addr.port(), 9000),
            other => panic!("expected Connect, got {other:?}"),
        }
    }

    #[test]
    fn ping_accepts_positional_address() {
        match p(&["ping", "127.0.0.1:9000"]).unwrap().command {
            Command::Ping { addr } => assert_eq!(addr.port(), 9000),
            other => panic!("expected Ping, got {other:?}"),
        }
    }

    #[test]
    fn connect_requires_an_address() {
        assert_eq!(
            p(&["connect"]).unwrap_err(),
            ParseError::MissingValue("--addr")
        );
    }

    #[test]
    fn ping_requires_an_address() {
        assert_eq!(
            p(&["ping"]).unwrap_err(),
            ParseError::MissingValue("--addr")
        );
    }

    #[test]
    fn missing_flag_value_is_reported() {
        assert_eq!(
            p(&["connect", "--addr"]).unwrap_err(),
            ParseError::MissingValue("--addr")
        );
        assert_eq!(
            p(&["connect", "--addr", "1.2.3.4:1", "--name"]).unwrap_err(),
            ParseError::MissingValue("--name")
        );
    }

    #[test]
    fn malformed_address_is_reported() {
        for bad in [
            "not-an-address",
            "127.0.0.1",
            "999.1.1.1:80",
            ":",
            "host:port",
        ] {
            assert!(
                matches!(p(&["connect", bad]).unwrap_err(), ParseError::BadAddress(_)),
                "accepted malformed address {bad:?}"
            );
        }
    }

    #[test]
    fn unknown_flag_for_command_is_reported() {
        assert_eq!(
            p(&["listen", "--wat"]).unwrap_err(),
            ParseError::UnknownFlag {
                command: "listen",
                flag: "--wat".into()
            }
        );
    }

    #[test]
    fn duplicate_flags_are_rejected() {
        assert_eq!(
            p(&["connect", "--addr", "1.2.3.4:1", "--addr", "5.6.7.8:2"]).unwrap_err(),
            ParseError::DuplicateFlag("--addr")
        );
        assert_eq!(
            p(&[
                "connect",
                "--addr",
                "1.2.3.4:1",
                "--name",
                "a",
                "--name",
                "b"
            ])
            .unwrap_err(),
            ParseError::DuplicateFlag("--name")
        );
    }

    #[test]
    fn name_flag_is_recorded() {
        let cli = p(&["connect", "127.0.0.1:9000", "--name", "Test Device"]).unwrap();
        assert_eq!(cli.device_name, "Test Device");
    }

    #[test]
    fn name_flag_works_with_eq_style_not_supported() {
        // `--name=value` is deliberately not supported; documenting the failure
        // is better than a half-implemented form.
        assert!(p(&["connect", "--name=Device", "1.2.3.4:1"]).is_err());
    }

    #[test]
    fn ipv6_addresses_parse() {
        match p(&["connect", "[::1]:9000"]).unwrap().command {
            Command::Connect { addr } => assert!(addr.ip().is_loopback()),
            other => panic!("expected Connect, got {other:?}"),
        }
    }

    #[test]
    fn parsing_never_panics_on_odd_input() {
        let cases: Vec<Vec<String>> = vec![
            vec![],
            vec!["".into()],
            vec!["--".into()],
            vec!["-".into()],
            vec!["connect".into()],
            vec!["connect".into(), "".into()],
            vec!["\u{1b}[31m".into()],
            vec!["connect".into(), "1.2.3.4:99999".into()],
            vec!["listen".into(), "--addr".into(), "x".into()],
            vec!["a".repeat(10_000)],
        ];
        for case in cases {
            let _ = parse(case); // must not panic
        }
    }

    #[test]
    fn usage_mentions_the_security_posture() {
        // The CLI must not let a user believe a Phase 1 session is secure.
        let text = usage();
        assert!(text.contains("UNENCRYPTED"));
        assert!(text.contains("UNAUTHENTICATED"));
    }

    #[test]
    fn usage_documents_every_command() {
        let text = usage();
        for cmd in ["listen", "connect", "ping", "help", "version"] {
            assert!(text.contains(cmd), "usage does not mention `{cmd}`");
        }
    }

    #[test]
    fn default_device_name_is_never_empty_or_has_control_chars() {
        let name = default_device_name();
        assert!(!name.is_empty());
        assert!(!name.chars().any(char::is_control));
    }
}
