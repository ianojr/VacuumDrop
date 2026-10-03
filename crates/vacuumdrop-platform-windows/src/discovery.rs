//! WinRT implementation. The only module that names `windows::*` types.
//!
//! Everything here converts between WinRT objects and the plain types in
//! [`crate::types`]. No WinRT type crosses this module's public boundary, which
//! is how [ADR-0004](../../docs/adr/0004-transport-abstraction.md) is held in
//! practice.
//!
//! # Feature gating
//!
//! `cfg(windows)` plus a `discovery` feature. Without the feature the crate
//! still compiles and reports honest unavailability, so a non-Windows or
//! feature-less build cannot accidentally claim Wi-Fi Direct support.

// `DeviceInformation` is only needed by the scan, which is behind the `discovery`
// feature. Importing it unconditionally would warn in the default build.
#[cfg(feature = "discovery")]
use windows::Devices::Enumeration::DeviceInformation;
use windows::Devices::WiFiDirect::{
    WiFiDirectAdvertisementListenStateDiscoverability, WiFiDirectAdvertisementPublisher,
    WiFiDirectAdvertisementPublisherStatus, WiFiDirectConnectionParameters, WiFiDirectDevice,
};
use windows::Networking::HostName;
use windows::core::HSTRING;

// `IntoFuture for IAsyncOperation<T>` lives in windows-future, which `windows`
// depends on but does not re-export at the crate root. Without this import the
// `.await` below fails to resolve, because the compiler looks for the impl in
// scope, not in the defining crate's dependency list.
use std::future::IntoFuture;

use crate::async_op::{AwaitError, await_op};
use crate::capability::Capability;
use crate::types::{AdapterError, ConnectionInfo, DiscoveredPeer};

/// Converts a WinRT error into our taxonomy.
///
/// The HRESULT code is preserved because it is the only thing that identifies
/// *which* Windows failure occurred, and the message is preserved because it is
/// the only thing a user can act on.
fn map_winrt(operation: &'static str, e: windows::core::Error) -> AdapterError {
    AdapterError::Windows {
        code: e.code().0,
        message: e.message().to_string(),
    }
    .with_context(operation)
}

impl AdapterError {
    /// Attaches an operation name to a `Windows` variant without rebuilding it.
    ///
    /// The operation name is what distinguishes "the API we called" from "what
    /// went wrong", and losing it produces errors like "Access denied" with no
    /// indication of which call was refused.
    fn with_context(self, operation: &'static str) -> AdapterError {
        match self {
            AdapterError::Windows { code, message } => AdapterError::Windows {
                code,
                message: format!("{operation}: {message}"),
            },
            other => other,
        }
    }
}

/// Probes whether Wi-Fi Direct can be used.
///
/// Layered rather than a single boolean, because "no hardware", "hardware
/// disabled", and "driver error" have different fixes and a user needs to know
/// which they have (Charter 6).
///
/// # Two independent questions, deliberately asked separately
///
/// 1. **Can we enumerate peers?** `WiFiDirectDevice::GetDeviceSelector2`
///    returns an AQS selector string. A non-empty selector means *the API
///    resolved a query*, which is not the same as "you have a usable adapter".
/// 2. **Can we be a group owner?** Only `WiFiDirectAdvertisementPublisher`
///    creation proves the driver will actually run a group. A laptop can have a
///    Wi-Fi Direct-capable chipset whose driver is corporate-locked, has the
///    feature disabled by policy, or lacks the GO role entirely - all of which
///    still return a perfectly non-empty selector.
///
/// Reporting "available" from the selector alone would therefore over-claim, and
/// ADR-0009 requirement 3 forbids exactly that.
pub fn detect_capability() -> Capability {
    const ADAPTER: &str = "wifi-direct";

    // Question 1: enumeration.
    let selector = match WiFiDirectDevice::GetDeviceSelector() {
        Ok(s) => s,
        Err(e) => {
            return Capability::ProbeFailed {
                adapter: "unknown".to_owned(),
                error: format!("GetDeviceSelector failed: {}", e.message()),
            };
        }
    };

    if selector.is_empty() {
        return Capability::NoAdapter;
    }

    // Question 2: group-owner capability.
    //
    // `new()` allocates the publisher but does **not** call `Start()`, so this
    // probe never makes the machine discoverable. We check `Status()` rather than
    // publishing, because a capability check must not have the side effect of
    // broadcasting this device's presence to the room.
    //
    // `WiFiDirectAdvertisementPublisherStatus` has only Created/Started/Stopped/
    // Aborted - there is no "Unsupported" value. That is precisely why creating
    // the object is the meaningful test: if the driver cannot act as group owner,
    // `new()` itself fails.
    let publisher = match WiFiDirectAdvertisementPublisher::new() {
        Ok(p) => p,
        Err(e) => {
            let code = e.code().0;
            // HRESULT is i32, so `0x80070005u32 as i32` is the correct spelling.
            // Writing the hex literal directly overflows and silently becomes a
            // negative number, which would never match.
            const E_ACCESSDENIED: i32 = 0x8007_0005u32 as i32;
            // Access denied means the API exists but policy forbids it - a
            // different user-facing fix from "no hardware present".
            if code == E_ACCESSDENIED {
                return Capability::Disabled {
                    adapter: ADAPTER.to_owned(),
                    reason: e.message().to_string(),
                };
            }
            return Capability::ProbeFailed {
                adapter: ADAPTER.to_owned(),
                error: format!(
                    "advertisement publisher rejected: 0x{code:08X}: {}",
                    e.message()
                ),
            };
        }
    };

    match publisher.Status() {
        Ok(status) if status == WiFiDirectAdvertisementPublisherStatus::Created => {
            Capability::Available {
                adapter: ADAPTER.to_owned(),
            }
        }
        Ok(status) => Capability::ProbeFailed {
            adapter: ADAPTER.to_owned(),
            // `status` is a newtype over i32 and has no Debug impl, so it is
            // rendered numerically rather than pretending it is a known state.
            error: format!("unexpected publisher status {}", status.0),
        },
        Err(e) => Capability::ProbeFailed {
            adapter: ADAPTER.to_owned(),
            error: format!(
                "publisher status unreadable: 0x{:08X}: {}",
                e.code().0,
                e.message()
            ),
        },
    }
}

/// Connects to a peer by device id and returns the address it was assigned.
///
/// This is the Phase 3 acceptance path. The group owner assigns an address via
/// DHCP (ADR-0009 Q3), and `GetConnectionEndpointPairs` reports it - so no
/// static IP configuration is required, and no Windows Firewall exemption is
/// needed for the address itself.
pub async fn connect_by_id(
    device_id: &str,
    prefer_group_owner: bool,
    timeout: std::time::Duration,
) -> Result<ConnectionInfo, AdapterError> {
    // A blank id would produce a confusing WinRT error much later. Reject it here,
    // where the message can be specific.
    if device_id.trim().is_empty() {
        return Err(AdapterError::ConnectFailed {
            peer: String::new(),
            reason: "device id must not be empty".to_owned(),
        });
    }

    let id = HSTRING::from(device_id);

    // Connection parameters: the group-owner intent is the one setting that
    // matters. Preferring owner gives us the stable address.
    let params =
        WiFiDirectConnectionParameters::new().map_err(|e| map_winrt("connection parameters", e))?;
    if prefer_group_owner {
        params
            .SetGroupOwnerIntent(15) // 0..15; 15 is the strongest preference
            .map_err(|e| map_winrt("set group owner intent", e))?;
    }

    // `FromIdAsync2` resolves the device and forms the group in one call.
    let op =
        WiFiDirectDevice::FromIdAsync2(&id, &params).map_err(|e| map_winrt("FromIdAsync2", e))?;

    // `IAsyncOperation<T>` implements `IntoFuture`, not `Future`, so `.await`
    // alone does not resolve - `AsyncFuture` is the real Future, and it is what we
    // hand to `await_op`. That wrapper adds the deadline Charter 65 requires.
    // (ADR-0009 Q2.)
    let device = await_op(op.into_future(), "FromIdAsync2", timeout)
        .await
        .map_err(|e| map_async("FromIdAsync2", e))?;

    // Ask what address the group assigned us. A formed group with no address is
    // a distinct failure from a failed group, and conflating them would send a
    // caller retrying the radio link, which cannot help.
    let pairs = device
        .GetConnectionEndpointPairs()
        .map_err(|e| map_winrt("GetConnectionEndpointPairs", e))?;

    // `EndpointPair` has LocalHostName / RemoteHostName, not NetworkAdapter. The
    // *remote* name is the peer's address on the formed group - which is what we
    // want to connect to, since the core opens an outbound TCP connection.
    let mut found: Option<IpAndPort> = None;
    for pair in pairs {
        // `RemoteHostName` is the correct side. Taking `LocalHostName` here would
        // give us our own address on the group, which the core would then try to
        // connect to instead of the peer.
        if let Ok(host) = pair.RemoteHostName()
            && let Some(addr) = resolve_host(&host)?
        {
            found = Some(addr);
            break;
        }
    }

    match found {
        Some(ip) => Ok(ConnectionInfo::new(ip.0, ip.1)),
        None => Err(AdapterError::NoAddressAssigned),
    }
}

/// Resolves a WinRT `HostName` to an address and our protocol port.
///
/// `HostName::DisplayName` is the literal address for link-local endpoints, which
/// is what a Wi-Fi Direct group owner hands out via DHCP (ADR-0009 Q3). It can
/// also be a DNS name, so parsing is tried first and resolution is the fallback.
fn resolve_host(host: &HostName) -> Result<Option<IpAndPort>, AdapterError> {
    // `DisplayName` returns `Result<HSTRING>` in windows 0.62, not a bare HSTRING.
    let raw = host
        .DisplayName()
        .map_err(|e| map_winrt("HostName::DisplayName", e))?
        .to_string();

    // Fast path: a literal address, which is the normal case and costs no DNS.
    if let Ok(ip) = raw.parse::<std::net::IpAddr>() {
        return Ok(Some(IpAndPort(ip, DEFAULT_PORT)));
    }

    // Slow path: a name. Only reached if the OS reports a hostname, which for
    // Wi-Fi Direct groups is rare.
    use std::net::ToSocketAddrs;
    let mut resolved = raw.to_socket_addrs().map_err(|e| AdapterError::Windows {
        code: 0,
        message: format!("could not resolve `{raw}`: {e}"),
    })?;

    match resolved.next() {
        Some(sa) => Ok(Some(IpAndPort(sa.ip(), DEFAULT_PORT))),
        None => Ok(None),
    }
}

/// The port VacuumDrop listens on.
///
/// Not a Windows concept: Windows assigns us an address, and the port belongs to
/// our own protocol ([`crate::types::ConnectionInfo::port`]).
const DEFAULT_PORT: u16 = 9000;

/// A resolved address and port.
#[derive(Debug, Clone, Copy)]
struct IpAndPort(std::net::IpAddr, u16);

/// Converts an async bridge error into the adapter taxonomy.
///
/// The WinRT error arrives as a rendered `Debug` string rather than an HRESULT,
/// because `await_op` is generic over the future's error type. `windows::core::Error`
/// renders as `Error { code: ..., message: ..., info: ... }`, so the HRESULT is
/// still present in the text and a diagnostic grep for it still works. That is a
/// deliberate trade: preserving the typed HRESULT would require `await_op` to be
/// generic over the error too, which buys nothing at this layer.
fn map_async(operation: &'static str, e: AwaitError) -> AdapterError {
    match e {
        AwaitError::Timeout { limit, .. } => AdapterError::ConnectFailed {
            peer: String::new(),
            reason: format!("{operation} exceeded {limit:?}"),
        },
        AwaitError::Failed { message, .. } => AdapterError::Windows {
            code: 0,
            message: format!("{operation}: {message}"),
        },
    }
}

/// Scans for peers.
///
/// Unavailable in this build; requires the `discovery` feature.
#[cfg(not(feature = "discovery"))]
pub async fn discover(_timeout: std::time::Duration) -> Result<Vec<DiscoveredPeer>, AdapterError> {
    Err(AdapterError::Unavailable(
        "peer discovery requires the `discovery` feature; connect by device id".to_owned(),
    ))
}

/// Makes this device discoverable to Wi-Fi Direct peers.
///
/// `intensive` selects the listen state. This is not cosmetic:
///
/// - `Normal` answers *directed* probes. A peer that already knows this device can
///   connect, which is why connect-by-id works while enumeration finds nothing.
/// - `Intensive` additionally answers *active/broadcast* scans, at a cost in radio
///   power and airtime.
///
/// `DeviceInformation` enumeration behaves like an active scan, so a `Normal`
/// advertiser can be invisible to it. That asymmetry is the most likely reason a
/// scan finds nothing while the device is genuinely advertising, which is why the
/// intensity is selectable rather than fixed.
///
/// The publisher must stay **alive** for the device to remain discoverable, so it
/// is returned rather than dropped here. See [`AdvertisementGuard`].
#[cfg(windows)]
pub fn advertise(intensive: bool) -> Result<AdvertisementGuard, AdapterError> {
    let publisher =
        WiFiDirectAdvertisementPublisher::new().map_err(|e| map_winrt("publisher", e))?;

    let state = if intensive {
        WiFiDirectAdvertisementListenStateDiscoverability::Intensive
    } else {
        WiFiDirectAdvertisementListenStateDiscoverability::Normal
    };
    publisher
        .Advertisement()
        .map_err(|e| map_winrt("advertisement", e))?
        .SetListenStateDiscoverability(state)
        .map_err(|e| map_winrt("set listen state", e))?;

    publisher
        .Start()
        .map_err(|e| map_winrt("start publisher", e))?;

    // Verify the publisher actually reached `Started`.
    //
    // `Start()` returning Ok only means the request was accepted, not that the
    // device is now discoverable. Without this check `advertise` would cheerfully
    // claim to be visible while being invisible - the worst kind of bug in a
    // tool whose entire job is being found. A driver can accept `Start()` and
    // still fail to publish.
    let status = publisher
        .Status()
        .map_err(|e| map_winrt("publisher status after start", e))?;
    if status != WiFiDirectAdvertisementPublisherStatus::Started {
        return Err(AdapterError::Unavailable(format!(
            "advertisement publisher did not start (status {}); \
             this machine is NOT discoverable",
            status.0
        )));
    }

    Ok(AdvertisementGuard {
        publisher: Some(publisher),
    })
}

/// Keeps a Wi-Fi Direct advertisement alive.
///
/// Dropping this stops advertising. An explicit [`stop`](Self::stop) exists because
/// relying on drop order during shutdown is how a machine stays discoverable after
/// the program that made it discoverable has exited.
#[cfg(windows)]
#[derive(Debug)]
pub struct AdvertisementGuard {
    publisher: Option<WiFiDirectAdvertisementPublisher>,
}

#[cfg(windows)]
impl AdvertisementGuard {
    /// Stops advertising. Idempotent.
    pub fn stop(&mut self) {
        if let Some(p) = self.publisher.take() {
            // Failure here means the publisher is already torn down, which is the
            // state the caller wanted anyway. Deliberately not surfaced: there is
            // no action a caller could take about it.
            let _ = p.Stop();
        }
    }

    /// Whether advertising is still active.
    pub fn is_advertising(&self) -> bool {
        self.publisher.is_some()
    }
}

#[cfg(windows)]
impl Drop for AdvertisementGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What a scan observed.
///
/// # Why this is not just a `Vec`
///
/// "No peers found" and "this machine reports no Wi-Fi Direct devices at all" are
/// very different claims, and a bare `Vec` renders them identically. The second
/// means the radio or driver is not offering P2P, which is a hardware or policy
/// problem; the first is an ordinary empty room. Conflating them would send someone
/// to the wrong fix, which is exactly what Charter 6 forbids.
#[cfg(feature = "discovery")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// Peers observed, merged across every selector. May legitimately be empty.
    pub peers: Vec<DiscoveredPeer>,
    /// True when no selector could be obtained at all, meaning Windows is offering
    /// no Wi-Fi Direct devices to enumerate. `peers` is then empty for a reason that
    /// has nothing to do with nearby devices.
    pub selector_empty: bool,
    /// Per-selector counts, so a caller can see *which* selector matched. That is
    /// what turns "discovery is broken" into a specific, actionable finding.
    pub per_selector: Vec<SelectorCount>,
}

/// How many devices one AQS selector matched.
#[cfg(feature = "discovery")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorCount {
    /// Human-readable name of the selector, for the report.
    pub label: &'static str,
    /// Devices that selector matched.
    pub count: usize,
    /// Why this selector contributed nothing, when it contributed nothing.
    ///
    /// Distinguishing "matched no devices" from "the selector itself failed" is the
    /// whole point: an empty count with an empty error means the platform is not
    /// offering P2P devices through this selector, which is a real finding.
    pub error: Option<String>,
}

/// Scans for peers using **every** known AQS selector and reports each one's count.
///
/// # Why more than one selector
///
/// `windows` 0.62.2 offers two device selectors:
/// `GetDeviceSelector()` and `GetDeviceSelector2(WiFiDirectDeviceSelectorType)`.
/// Which one a given driver populates with discoverable peers is not documented and
/// evidently not uniform - an earlier build of this adapter used the classic
/// selector and found nothing on real hardware.
///
/// Rather than guess, both are enumerated and the per-selector counts are reported.
/// That turns a hypothesis into a measurement in a single run, which is the
/// difference between diagnosing and guessing.
///
/// Results are merged and de-duplicated by device id, because the two selectors can
/// overlap.
#[cfg(feature = "discovery")]
pub async fn scan(timeout: std::time::Duration) -> Result<Scan, AdapterError> {
    let mut per_selector: Vec<SelectorCount> = Vec::new();
    let mut peers: Vec<DiscoveredPeer> = Vec::new();
    let mut any_selector_usable = false;

    // (label, result of obtaining the AQS string)
    let selectors: Vec<(
        &'static str,
        Result<windows::core::HSTRING, windows::core::Error>,
    )> = vec![
        ("classic", WiFiDirectDevice::GetDeviceSelector()),
        (
            "assoc-endpoint",
            WiFiDirectDevice::GetDeviceSelector2(
                windows::Devices::WiFiDirect::WiFiDirectDeviceSelectorType::AssociationEndpoint,
            ),
        ),
    ];

    for (label, selector) in selectors {
        let selector = match selector {
            Ok(s) => s,
            Err(e) => {
                per_selector.push(SelectorCount {
                    label,
                    count: 0,
                    error: Some(format!("0x{:08X}: {}", e.code().0, e.message())),
                });
                continue;
            }
        };

        if selector.is_empty() {
            per_selector.push(SelectorCount {
                label,
                count: 0,
                error: Some("selector was empty".to_owned()),
            });
            continue;
        }
        any_selector_usable = true;

        match enumerate(&selector, timeout).await {
            Ok(found) => {
                per_selector.push(SelectorCount {
                    label,
                    count: found.len(),
                    error: None,
                });
                for peer in found {
                    if !peers.iter().any(|p| p.device_id == peer.device_id) {
                        peers.push(peer);
                    }
                }
            }
            Err(e) => per_selector.push(SelectorCount {
                label,
                count: 0,
                error: Some(e.to_string()),
            }),
        }
    }

    Ok(Scan {
        peers,
        selector_empty: !any_selector_usable,
        per_selector,
    })
}

/// Enumerates the devices matching one AQS selector.
#[cfg(feature = "discovery")]
async fn enumerate(
    selector: &windows::core::HSTRING,
    timeout: std::time::Duration,
) -> Result<Vec<DiscoveredPeer>, AdapterError> {
    let op = DeviceInformation::FindAllAsyncAqsFilter(selector)
        .map_err(|e| map_winrt("FindAllAsyncAqsFilter", e))?;
    let collection = await_op(op.into_future(), "FindAllAsyncAqsFilter", timeout)
        .await
        .map_err(|e| map_async("FindAllAsyncAqsFilter", e))?;

    // `DeviceInformationCollection` is a projected IMapView exposing `Size` and
    // `GetAt`, so indexed iteration is used rather than an `IntoIterator` impl
    // that does not exist for this projection.
    let count = collection
        .Size()
        .map_err(|e| map_winrt("collection size", e))?;

    let mut peers = Vec::with_capacity(count as usize);
    for index in 0..count {
        let info = match collection.GetAt(index) {
            Ok(i) => i,
            // A peer that disappears between enumeration and lookup is ordinary in
            // P2P, where devices roam. Skipping it is correct; failing the whole scan
            // because one peer left would make discovery useless in a busy room.
            Err(_) => continue,
        };

        // `DeviceInformation::Id()` is the id `FromIdAsync2` accepts, so this string
        // is directly usable as a connect target.
        let device_id = info
            .Id()
            .map_err(|e| map_winrt("DeviceInformation::Id", e))?
            .to_string();

        // `DeviceInformation::Name()` can be empty for a P2P device - the protocol
        // carries no friendly name (ADR-0009). Fall back to the id rather than
        // printing an empty label.
        let display_name = match info.Name() {
            Ok(n) if !n.is_empty() => n.to_string(),
            _ => device_id.clone(),
        };

        peers.push(DiscoveredPeer::new(device_id, display_name));
    }

    Ok(peers)
}
/// Scans for peers, discarding the reason an empty result occurred.
#[cfg(feature = "discovery")]
pub async fn discover(timeout: std::time::Duration) -> Result<Vec<DiscoveredPeer>, AdapterError> {
    Ok(scan(timeout).await?.peers)
}

// Note on a removed function, kept because the finding is easy to rediscover the
// hard way.
//
// `peer_from_device` used to convert a `WiFiDirectDevice` into a `DiscoveredPeer`.
// It was removed rather than kept behind `#[allow(dead_code)]` because nothing
// called it while discovery was unimplemented. Dead code that no test can reach is a
// liability, not documentation -
// the `--all-features` clippy gate in `scripts/check.ps1` is what surfaced it.
//
// The finding worth keeping: `WiFiDirectDevice` in windows 0.62.2 exposes
// `DeviceId`, `ConnectionStatus`, and `GetConnectionEndpointPairs`, and **no `Name`
// accessor at all**. The Wi-Fi Direct protocol carries no friendly device name, so
// `DiscoveredPeer` reports the device id instead.
//
// When the watcher lands, write that function again alongside the code that calls
// it. It must be `pub(crate)` and take the WinRT type: `scripts/check.ps1` fails the
// build if any WinRT type appears in a `pub` signature, so the conversion has to
// happen inside this module.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_error_context_is_preserved() {
        let base = AdapterError::Windows {
            code: 0x8007_0005u32 as i32,
            message: "Access is denied.".to_owned(),
        };
        let ctx = base.with_context("FromIdAsync2");
        match ctx {
            AdapterError::Windows { message, .. } => {
                assert!(message.contains("FromIdAsync2"));
                assert!(message.contains("Access is denied"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn with_context_leaves_other_variants_untouched() {
        let e = AdapterError::NoAddressAssigned.with_context("some op");
        assert_eq!(e, AdapterError::NoAddressAssigned);
    }

    #[test]
    fn empty_device_id_is_refused_before_touching_winrt() {
        // Must fail fast with a clear message rather than deep inside WinRT.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            for bad in ["", "   ", "\t"] {
                let e = connect_by_id(bad, true, std::time::Duration::from_millis(50))
                    .await
                    .unwrap_err();
                let s = e.to_string();
                assert!(
                    s.contains("must not be empty"),
                    "unclear message for {bad:?}: {s}"
                );
            }
        });
    }

    #[test]
    fn default_port_is_our_protocol_not_a_windows_one() {
        // Guards against someone "fixing" this to a Windows-supplied port.
        assert_eq!(DEFAULT_PORT, 9000);
    }

    #[test]
    fn async_timeout_maps_to_a_connect_failure() {
        let e = map_async(
            "op",
            AwaitError::Timeout {
                operation: "op",
                limit: std::time::Duration::from_secs(3),
            },
        );
        let s = e.to_string();
        assert!(s.contains("op"));
        assert!(s.contains("3s"));
    }

    #[test]
    fn async_failure_maps_to_a_windows_error() {
        let e = map_async(
            "op",
            AwaitError::Failed {
                operation: "op",
                message: "denied".into(),
            },
        );
        match e {
            AdapterError::Windows { message, .. } => assert!(message.contains("denied")),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn async_errors_preserve_the_operation_name() {
        // Without the operation name, "exceeded 3s" is unactionable.
        let e = map_async(
            "WiFiDirectDevice::FromIdAsync2",
            AwaitError::Timeout {
                operation: "op",
                limit: std::time::Duration::from_secs(3),
            },
        );
        assert!(e.to_string().contains("FromIdAsync2"));
    }

    #[test]
    fn capability_probe_returns_a_variant_never_a_panic() {
        // Runs on the real machine. Whatever it finds, it must not panic and must
        // produce a non-empty label - a panic here would be a crash on startup.
        let c = detect_capability();
        assert!(!c.label().is_empty());
        assert!(!c.to_string().is_empty());
    }
}
