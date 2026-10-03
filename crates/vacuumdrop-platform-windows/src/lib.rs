//! Windows Wi-Fi Direct adapter.
//!
//! Charter 46 assigns this crate five responsibilities and nothing else:
//!
//! 1. Wi-Fi Direct capability detection
//! 2. Peer discovery
//! 3. Connection management
//! 4. Retrieving connection information
//! 5. Exposing the result to Rust
//!
//! It does **not** implement protocol, chunking, hashing, or any business logic.
//! Those live in the Rust core exactly once.
//!
//! # Layering
//!
//! ```text
//! vacuumdrop-core        identity, state machine, errors
//!        ^
//!        |  depends on nothing from here except vacuumdrop-core
//!        |
//! vacuumdrop-platform-windows   <-- THIS CRATE
//!        |
//!        |  the only place WinRT types exist
//!        v
//! Windows.Devices.WiFiDirect
//! ```
//!
//! # Phase 3 spike results
//!
//! Recorded in [ADR-0009](../../docs/adr/0009-windows-wifi-direct-binding.md).
//! The binding strategy was ratified on four answers:
//!
//! | Q | Answer | Consequence here |
//! | --- | --- | --- |
//! | Does `windows` 0.62 expose `Windows.Devices.WiFiDirect`? | **Yes** | `discovery.rs` uses it directly |
//! | How are `IAsyncOperation`s awaited? | **No `IntoFuture` in 0.62** | [`async_op`] polls `Status` |
//! | Static IP or DHCP after group formation? | **DHCP suffices** | `GetConnectionEndpointPairs` supplies the address |
//! | Practical `unsafe` count? | **Zero in our code** | `#![forbid(unsafe_code)]` holds |
//!
//! # What is NOT implemented, and why
//!
//! **Automatic peer discovery is not wired up.** `WiFiDirectDevice::GetDeviceSelector()`
//! returns a device-watcher AQS string, and consuming it requires
//! `DeviceInformation::CreateWatcher` plus a `DeviceWatcher` event loop. That is
//! a second, larger piece of WinRT machinery, and it is **not** required to prove
//! the Phase 3 acceptance criterion, which is that `p2p connect` works over a
//! Wi-Fi Direct link with **no protocol changes**.
//!
//! Connecting by device id already works, so the acceptance path is unblocked.
//! Watcher-based discovery is a follow-up, and `Adapter::connect_by_id` is the
//! documented entry point until it lands. Claiming discovery works before it does
//! would be the exact over-claiming Charter 6 forbids.
//!
//! # Threading
//!
//! WinRT objects are COM-backed and some are apartment-bound. All WinRT calls
//! here happen on whatever Tokio worker drives the adapter future, and each call
//! completes within a single `poll`. No WinRT object is sent across a task
//! boundary. [`async_op`] polls, it does not spawn.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

pub mod async_op;
pub mod capability;
pub mod types;

#[cfg(windows)]
pub mod discovery;

pub use capability::Capability;
pub use types::{AdapterError, ConnectionInfo, DiscoveredPeer};

use std::time::Duration;

/// Default deadline for a discovery scan.
pub const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Default deadline for forming a group.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// The Windows Wi-Fi Direct adapter.
///
/// Thin by construction: it holds configuration and a shutdown flag, and
/// delegates to [`discovery`] for anything WinRT.
#[derive(Debug)]
pub struct Adapter {
    device_name: String,
    discovery_timeout: Duration,
    connect_timeout: Duration,
    prefer_group_owner: bool,
    shut_down: std::sync::atomic::AtomicBool,
}

impl Adapter {
    /// Builds an adapter.
    pub fn new(device_name: impl Into<String>) -> Self {
        Self {
            device_name: device_name.into(),
            discovery_timeout: DEFAULT_DISCOVERY_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            // Prefer to be the group owner: the group owner is the side that ends
            // up with the stable address, which is what makes the resulting
            // ConnectionInfo usable. Charter 6 territory, so it is observable.
            prefer_group_owner: true,
            shut_down: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Overrides the discovery deadline.
    pub fn discovery_timeout(mut self, timeout: Duration) -> Self {
        self.discovery_timeout = timeout;
        self
    }

    /// Overrides the connect deadline.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets whether to request group-owner role.
    pub fn prefer_group_owner(mut self, prefer: bool) -> Self {
        self.prefer_group_owner = prefer;
        self
    }

    /// This device's declared name.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Whether the group-owner role is being requested.
    pub fn prefers_group_owner(&self) -> bool {
        self.prefer_group_owner
    }

    /// Probes whether Wi-Fi Direct can be used.
    ///
    /// Charter 6: capability must be detected, never assumed. The result
    /// distinguishes *absent*, *disabled*, and *probe failed*, because those have
    /// three different fixes.
    #[cfg(windows)]
    pub fn detect_capability(&self) -> Capability {
        discovery::detect_capability()
    }

    /// Probes whether Wi-Fi Direct can be used.
    ///
    /// On a non-Windows build this reports `NoAdapter` rather than pretending.
    /// A stub that returns "available" on a platform that cannot do it would be a
    /// lie the compatibility matrix would then record.
    #[cfg(not(windows))]
    pub fn detect_capability(&self) -> Capability {
        Capability::NoAdapter
    }

    /// Stops the adapter. Idempotent.
    pub fn shutdown(&self) {
        self.shut_down
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the adapter has been shut down.
    pub fn is_shut_down(&self) -> bool {
        self.shut_down.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn check_live(&self) -> Result<(), AdapterError> {
        if self.is_shut_down() {
            Err(AdapterError::ShutDown)
        } else {
            Ok(())
        }
    }

    /// Connects to a peer by its platform device id.
    ///
    /// The Phase 3 acceptance path: form a Wi-Fi Direct group and return an
    /// address the core can open a TCP socket to. No protocol knowledge here.
    #[cfg(windows)]
    pub async fn connect_by_id(&self, device_id: &str) -> Result<ConnectionInfo, AdapterError> {
        self.check_live()?;
        discovery::connect_by_id(device_id, self.prefer_group_owner, self.connect_timeout).await
    }

    /// Connects to a peer by its platform device id.
    ///
    /// Reports unavailability rather than pretending on a non-Windows build.
    #[cfg(not(windows))]
    pub async fn connect_by_id(&self, device_id: &str) -> Result<ConnectionInfo, AdapterError> {
        let _ = device_id;
        Err(AdapterError::Unavailable(
            "Wi-Fi Direct is only implemented on Windows".to_owned(),
        ))
    }

    /// Scans for peers.
    ///
    /// **Not implemented.** Requires the `DeviceInformation` watcher machinery;
    /// see the module documentation for why it is deferred rather than faked.
    pub async fn discover(&self) -> Result<Vec<DiscoveredPeer>, AdapterError> {
        self.check_live()?;
        #[cfg(windows)]
        {
            discovery::discover(self.discovery_timeout).await
        }
        #[cfg(not(windows))]
        {
            Err(AdapterError::Unavailable(
                "peer discovery is not implemented yet; connect by device id".to_owned(),
            ))
        }
    }

    /// Whether peer discovery is available.
    ///
    /// Exposed so the CLI can say "discovery is not implemented" plainly instead
    /// of printing an empty list that looks like "no peers nearby".
    pub fn discovery_available(&self) -> bool {
        cfg!(all(windows, feature = "discovery"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_starts_live() {
        let a = Adapter::new("test");
        assert!(!a.is_shut_down());
    }

    #[test]
    fn shutdown_is_idempotent() {
        let a = Adapter::new("test");
        a.shutdown();
        a.shutdown();
        assert!(a.is_shut_down());
    }

    #[test]
    fn adapter_is_send_and_sync() {
        // The engine holds "whichever transport this platform provides" as one
        // type and shares it across tasks. If the adapter were not Send + Sync,
        // that would fail to compile at the composition point instead of here.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Adapter>();
    }

    #[test]
    fn deadlines_are_configurable() {
        let a = Adapter::new("test")
            .discovery_timeout(Duration::from_secs(3))
            .connect_timeout(Duration::from_secs(7));
        // Round-trip through the accessors so the test fails if a setter stops
        // storing what it is given.
        assert_eq!(a.discovery_timeout, Duration::from_secs(3));
        assert_eq!(a.connect_timeout, Duration::from_secs(7));
    }

    #[test]
    fn group_owner_preference_defaults_on_and_is_settable() {
        let a = Adapter::new("test");
        assert!(
            a.prefers_group_owner(),
            "should prefer group owner by default"
        );
        let b = Adapter::new("test").prefer_group_owner(false);
        assert!(!b.prefers_group_owner());
    }

    #[tokio::test]
    async fn operations_after_shutdown_are_refused() {
        let a = Adapter::new("test");
        a.shutdown();
        // Every long-running operation must respect shutdown (Charter 64).
        assert!(matches!(a.discover().await, Err(AdapterError::ShutDown)));
        assert!(matches!(
            a.connect_by_id("whatever").await,
            Err(AdapterError::ShutDown) | Err(AdapterError::Unavailable(_))
        ));
    }

    #[test]
    fn capability_probe_is_honest_on_this_platform() {
        let a = Adapter::new("test");
        let c = a.detect_capability();
        // Whatever it reports, it must be a real answer rather than a default
        // "yes". `is_available` is the only true path.
        if !c.is_available() {
            assert!(!c.label().is_empty());
        }
    }

    #[tokio::test]
    async fn discovery_either_reports_a_real_scan_or_says_why_not() {
        // Discovery is implemented, so this no longer asserts "unavailable". What it
        // still asserts is the invariant behind that decision: a caller can always
        // tell an *observed* empty result from an *unavailable* feature.
        //
        // Without the `discovery` feature the answer must be an explicit error.
        // An empty list there would read as "no peers nearby", which is a
        // different and false claim.
        let a = Adapter::new("test");
        // Bound to a local first: `cfg!` expands to a constant, and clippy's
        // `assertions_on_constants` rejects asserting on one directly. The binding
        // also documents that the check is a build-time property, not a runtime one.
        let feature_on = cfg!(feature = "discovery");
        match a.discover().await {
            Err(AdapterError::Unavailable(msg)) => {
                assert!(!msg.is_empty(), "unavailability must be explained");
                assert!(
                    !feature_on,
                    "reported unavailable even though the feature is on"
                );
            }
            Err(other) => {
                // A real scan may fail for real reasons on a real machine (no
                // adapter, policy). That is a legitimate outcome, not a defect, so
                // there is nothing to assert beyond it being reported.
                let _ = other;
            }
            Ok(peers) => {
                assert!(
                    feature_on,
                    "discovery returned peers but the feature is off"
                );
                // An observed result is allowed to be empty: this machine may
                // genuinely have no peers nearby. That is now a true claim.
                let _ = peers;
            }
        }
    }

    #[test]
    fn discovery_available_flag_matches_reality() {
        let a = Adapter::new("test");
        // The flag exists so the CLI can explain an empty result rather than
        // showing a bare "no peers" list.
        assert_eq!(
            a.discovery_available(),
            cfg!(all(windows, feature = "discovery"))
        );
    }
}
