//! The `p2p` command-line interface.
//!
//! Phase 1 scope: listen, connect, ping, help, version. No UI, no file transfer,
//! no discovery. Charter 75 puts all of those later, and `docs/roadmap.md`
//! section 9 puts transfer in Phase 5.
//!
//! # What this binary does and does not do
//!
//! It establishes a version-negotiated session over TCP and reports what
//! happened. That is all.
//!
//! **Phase 1 sessions are unencrypted and unauthenticated.** Version
//! negotiation proves the peer speaks the same protocol; it establishes nothing
//! about who that peer is. The CLI states this on `help` and again after a
//! successful connect, because a user who reads "connected" as "trusted" is
//! exactly the failure mode Charter 6 warns about.
//!
//! Charter 6 also forbids claiming "works on every device": nothing here has
//! been tested outside this loopback configuration, so no such claim is made.

mod cli;
mod commands;

use std::process::ExitCode;

use cli::{ParseError, parse};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let parsed = match parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!();
            eprintln!("{}", cli::usage());
            return ExitCode::from(2);
        }
    };

    // A single-threaded runtime is enough: every operation here is I/O bound on a
    // socket, and a multi-threaded runtime would cost memory without buying
    // concurrency this binary does not have (ADR-0002 section 4.1).
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: could not start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(async move { commands::run(parsed).await })
}

/// Renders a parse failure for the test suite.
#[allow(dead_code)]
fn describe_error(e: &ParseError) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_errors_render_readable_text() {
        assert_eq!(
            describe_error(&ParseError::MissingValue("--addr")),
            "--addr requires a value"
        );
        assert!(
            describe_error(&ParseError::BadAddress("nope".into())).contains("expected host:port")
        );
    }
}
