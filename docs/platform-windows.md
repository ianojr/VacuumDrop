# Windows Platform Adapter

Status: **Phase 3.** First platform implemented.

---

## 1. Responsibilities

The Windows adapter is responsible for:

- Wi-Fi Direct **capability detection**
- Peer / device discovery, as supported
- Pairing / invocation of platform APIs
- Connection establishment
- Exposing network connectivity to Rust

It is **not** responsible for the protocol, chunking, hashing, or any business
logic. It resolves a connection and returns an IP and port.

```text
Windows Wi-Fi Direct  -->  IP + port  -->  Rust core (TCP + protocol)
```

---

## 2. API Choice

> Use **modern Windows Wi-Fi Direct APIs** rather than obsolete ad-hoc networking
> approaches.

| Approach | Status | Use |
| --- | --- | --- |
| `Windows.Devices.WiFiDirect` (WinRT) | Modern | **Use this** |
| Hosted Network / SoftAP (`WlanSetHostedNetwork`, `WLAN_OPERATION_MODE_HOSTED`) | Legacy, driver-dependent, deprecated on modern drivers | Avoid |
| IBSS / ad-hoc (`DOT11_IBSS_*`) | Obsolete for this purpose | Avoid |
| `wifidirectsvc` / undocumented internals | Unsupported | Never |

The modern surface is the WinRT `Windows.Devices.WiFiDirect` namespace:
`WiFiDirectAdvertisementPublisher`, `WiFiDirectDevice`,
`WiFiDirectDeviceListWatcher`, `WiFiDirectInvitation*`.

> **Note.** "Mobile Hotspot" (used in the Phase 0/2 test rig) is
> Internet Connection Sharing over a SoftAP. It is **infrastructure mode**, not
> Wi-Fi Direct P2P. It validates IP connectivity and TCP throughput. It does
> **not** validate discovery, group formation, or GO negotiation.

---

## 3. Rust Binding Strategy

Three viable approaches:

| Option | Pros | Cons |
| --- | --- | --- |
| **`windows` crate** (`windows` with `WiFiDirect` features) | In-process, no IPC, generated bindings, no native build step | Large crate; WinRT `IAsyncOperation` bridging is awkward; some patterns need `unsafe` |
| Thin C#/WinRT helper component behind a C ABI | Idiomatic WinRT in C#; small Rust surface; async is natural | Extra project, IPC boundary, more moving parts |
| PowerShell / script host | No build complexity | Not production-appropriate |

**Proposed for Phase 3:** the `windows` crate, contained entirely inside
`crates/vacuumdrop-platform-windows`. If WinRT async bridging proves to be a
persistent blocker, escalate to the helper-component approach rather than
leaking WinRT types upward.

Recorded as [ADR-0009](./adr/0009-windows-wifi-direct-binding.md). The
`windows` crate must be justified under Charter 54 - it is large, and the
justification is "there is no reasonable alternative for WinRT from Rust".

### 3.1 Boundary discipline

All `windows`-crate types are confined to this crate. The crate exposes only
normalized values:

```text
ConnectionInfo { ip: IpAddr, port: u16, interface: InterfaceKind }
Peer          { id: String, display_name: String }
Capability    { ... }
```

No WinRT type appears in any public signature that crosses into
`vacuumdrop-core`.

---

## 4. Capability Detection

Per Charter 6, the adapter must expose:

```text
detect_capability()  # layered: available / no adapter / disabled / probe failed
discover_peers()
connect_by_id(device_id)
```

Detection must reflect **actual hardware/driver capability**, not merely the
presence of a Wi-Fi radio. A machine with an 802.11n adapter may support Wi-Fi
Direct in name only.

Support is a per-device, per-driver property. Never claim "works on every
device."

### 4.1 What is actually probed

Two independent questions, because a single check cannot answer both:

| # | Question | API | What a pass proves |
| --- | --- | --- | --- |
| 1 | Can we enumerate peers? | `WiFiDirectDevice::GetDeviceSelector()` | Windows resolved the P2P device query. |
| 2 | Can we be a group owner? | `WiFiDirectAdvertisementPublisher::new()` + `Status()` | The driver will actually run a group. |

**Question 1 alone is not sufficient.** A non-empty AQS selector is returned by
machines whose driver is corporate-locked, whose Wi-Fi Direct feature is disabled
by policy, or whose adapter lacks the group-owner role. Reporting "available"
from the selector alone is the over-claim ADR-0009 requirement 3 forbids, so the
publisher probe is mandatory.

`WiFiDirectAdvertisementPublisherStatus` has only `Created`, `Started`,
`Stopped`, and `Aborted` - there is **no `Unsupported` value**. That is exactly
why object creation is the meaningful test: if the driver cannot act as group
owner, `new()` itself fails.

### 4.2 The probe has no side effect

The capability probe calls `new()` and `Status()` but **never `Start()`**, so
running it never makes the machine discoverable to the room. A check that
broadcasts the device's presence as a side effect would be a surprising way to
answer "is this supported?"

### 4.3 Run it

From the CLI:

```powershell
cargo run --features wifi-direct --bin p2p -- wifi-direct probe
```

Or from the adapter directly:

```powershell
cargo run -p vacuumdrop-platform-windows --example probe
```

Both print the raw verdict for this host. Capability is the one part of Phase 3
that no unit test can validate, since it depends on the real radio, driver, and
policy of the machine. The commands exist so a hardware run records evidence
instead of asserting success.

### 4.4 The `wifi-direct` build flag

Wi-Fi Direct support is **off by default**:

```toml
[target.'cfg(windows)'.dependencies]
vacuumdrop-platform-windows = { ..., optional = true }

[features]
wifi-direct = ["dep:vacuumdrop-platform-windows"]
```

Two reasons, both deliberate:

1. **Portability.** `p2p` is the portable front end; platform crates are the
   replaceable back end. A hard dependency would make the binary Windows-only,
   which is the coupling Charter 47 exists to prevent.
2. **Honesty.** Real two-device P2P is unvalidated. Default-on would ship a path
   that has never worked.

Without the flag, `p2p wifi-direct ...` fails with the exact command needed to
rebuild. It does **not** silently do nothing, and it is not reported as an
adapter fault. `p2p help` states which build you are running.

### 4.5 Verified on hardware

On the development host the probe reports `available (wifi-direct)`, confirming
group-owner creation works on a real adapter with a real driver.

Not yet verified: a real group formation between two devices, and whether DHCP
assignment behaves as the API suggests.

---

## 5. Known Environment Variables

| Failure | Symptom | Handling |
| --- | --- | --- |
| Adapter lacks P2P | `detect_capability()` reports `no adapter` or `probe failed` | Report unsupported, not "no peers" |
| Policy blocks P2P | Publisher creation or status fails | Reported as `disabled`, distinct from unsupported |
| Driver/VIF creation fails | Connect error | Surface driver-specific error text |
| VPN callout blocks sockets | `WSAEACCES` / os error `10013` on connect | Disable the VPN; see [`troubleshooting.md`](./troubleshooting.md) |
| Interface disappears mid-transfer | Transfer aborts | Must produce a defined state transition ([`troubleshooting.md`](./troubleshooting.md)) |

---

## 6. Test Procedure

Two Windows devices, both with Direct-capable adapters.

### 6.1 The procedure

Every command below has been executed at least once. On the device that will
**host** the session:

```powershell
# terminal 1 - become discoverable
p2p wifi-direct advertise 300

# terminal 2 - accept the session
p2p listen --addr 0.0.0.0:9000
```

On the device that will **connect**:

```powershell
p2p wifi-direct probe                  # confirm this machine supports it
p2p wifi-direct discover               # list peers; note the device-id
p2p wifi-direct connect <device-id>    # form the group, then handshake
```

`connect` prints the group address it was assigned, connects over TCP, and runs
the unchanged Phase 1 handshake. Both sides then report a session id, protocol
version, peer name, and chunk size.

### 6.2 Why `listen --addr 0.0.0.0`

The host must be reachable on the P2P interface. Binding to `0.0.0.0` also
accepts connections from the normal network, which is **not** the shipping
configuration — that will bind only the P2P interface, once this side learns its
own address (see 6.4). For a two-device test rig on an isolated network it is
acceptable, and `advertise` says so in its own output rather than leaving it
implicit.

### 6.3 Why the host half is a separate command

`p2p wifi-direct advertise` deliberately does **not** serve the session itself.
Doing so requires this side to discover the P2P interface's own address, via
`Windows.Networking.Connectivity` — a second platform subsystem, which cannot be
tested without a second device. Splitting it out means every step of the
procedure runs code that has already been executed.

### 6.4 What is still unverified

| Claim | Status |
| --- | --- |
| Group forms between two devices | **Untested.** Needs two devices. |
| DHCP assigns a usable address | **Untested.** API suggests yes (ADR-0009 Q3). |
| Discovery finds an advertising peer | **Untested.** The scan itself is verified to run; that it finds a *real* peer is not. |
| Binding only the P2P interface | Not implemented. |

### 6.5 Step 7 is the acceptance test

Running the Phase 1 protocol unchanged over the P2P link is the acceptance test
for [ADR-0004](./adr/0004-transport-abstraction.md): **the protocol must require
no changes.** If it does, the abstraction leaked.

This is structurally enforced. `wifi_direct_connect` calls the adapter for
exactly one thing — an address — and then uses the ordinary `TcpConnector` and
`Session`. No radio concept appears below that line.

---

## 7. Phase 2 Test Rig (Distinct from Phase 3)

```text
Device A: Windows Mobile Hotspot ON
Device B: joins the hotspot
Both: connect to the local gateway IP
```

Purpose: validate IP connectivity and TCP throughput across two real machines
**before** introducing P2P negotiation. Isolates protocol bugs from radio bugs.

Do not record these results as Wi-Fi Direct results.

---

## 8. Open Questions

| Question | Status | Reference |
| --- | --- | --- |
| `windows` crate vs helper component | **Resolved** - direct `windows` crate, no helper | [ADR-0009](./adr/0009-windows-wifi-direct-binding.md) |
| Static IP vs DHCP-allocated IP after group formation | API says DHCP sufficient; **unverified on hardware** | ADR-0009 spike Q3 |
| `DeviceInformation` watcher-based peer discovery | Not wired up; `discovery` feature is a declared stub | `src/discovery.rs` |
| Wi-Fi Direct device friendly name | **Unavailable from the platform.** `WiFiDirectDevice` has no `Name` accessor; the protocol carries no name. Reports the device id. | ADR-0009 |
| IPv6 support | Deferred, but link-local is the expected P2P case and `ConnectionInfo` renders/brackets it correctly | - |
| Whether to ship a GUI settings page for adapter diagnostics | Phase 8 | - |

---

## 9. Source Traceability

| Section | Origin |
| --- | --- |
| 1, 2, 4 | Charter 46 (Windows), 6 |
| 3 | Charter 54, 47 |
| 4.1, 4.2 | ADR-0009 requirement 3, spike Q1 |
| 4.3, 4.4 | Charter 6 (honest reporting), 76 |
| 5 | Charter 6, 42 |
| 6, 7 | Charter 77, 76 |
| 2 (hotspot caveat) | Doc 2 section 4 |