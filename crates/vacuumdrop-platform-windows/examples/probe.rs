//! Diagnostic: report what the Wi-Fi Direct adapter detects on THIS machine.
//!
//! Run with:
//!     cargo run -p vacuumdrop-platform-windows --example probe
//!
//! Exists because capability detection is the one thing in Phase 3 that cannot be
//! validated by a unit test: it depends on the actual radio, driver, and policy of
//! the host. Charter 6 requires the answer be honest, and an honest answer needs to
//! be observable. Printing the raw verdict is how a Phase 3 hardware run records
//! evidence instead of asserting success.
//!
//! This probes only. It deliberately does NOT call `Start()` on the advertisement
//! publisher, so running it never makes the machine discoverable to the room.

use vacuumdrop_platform_windows::Capability;
use vacuumdrop_platform_windows::discovery::detect_capability;

fn main() {
    println!("VacuumDrop Wi-Fi Direct capability probe");
    println!("crate version: {}", env!("CARGO_PKG_VERSION"));
    println!();

    match detect_capability() {
        Capability::Available { adapter } => {
            println!("VERDICT: available ({adapter})");
            println!("A group owner could be created. Discovery of peers is a");
            println!("separate feature and is not wired up yet.");
        }
        Capability::NoAdapter => {
            println!("VERDICT: no adapter");
            println!("Windows returned an empty Wi-Fi Direct selector.");
            println!("Fix: enable Wi-Fi, then check the adapter is not blocked by policy.");
        }
        Capability::Disabled { adapter, reason } => {
            println!("VERDICT: disabled ({adapter})");
            println!("reason: {reason}");
            println!("Fix: this is usually policy or a permission, not missing hardware.");
        }
        Capability::ProbeFailed { adapter, error } => {
            println!("VERDICT: probe failed ({adapter})");
            println!("error: {error}");
            println!("Fix: the API exists but errored. Report the HRESULT verbatim.");
        }
    }
}
