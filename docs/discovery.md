# Discovery

Status: **Split across phases.** Platform-level discovery lands with each
platform adapter (Phase 3 / 6 / 7); application-level discovery lands in
Phase 5.

---

## 1. Two Distinct Layers

This is the single most important distinction in this subsystem. Conflating the
two is the most common architectural mistake in P2P systems.

| | Platform Discovery | Application Discovery |
| --- | --- | --- |
| **Performed by** | The operating system | The Rust application |
| **Answers** | "Which radios/devices are nearby?" | "Does that device speak VacuumDrop, and which version?" |
| **Phase** | 3 / 6 / 7 | 5 |
| **Trust** | Untrusted hint | Verified via authentication (Phase 4) |

```text
┌─────────────────────────────────────────────────────────┐
│ 1. Platform Discovery                                   │
│    OS scans for Wi-Fi Direct peers                     │
│    → returns: list of opaque platform handles          │
├─────────────────────────────────────────────────────────┤
│ 2. Connect + Application Discovery                     │
│    Core connects, negotiates protocol version,         │
│    exchanges identity and capabilities                 │
│    → returns: list of VacuumDrop peers, versioned,     │
│      authenticated, capability-described              │
└─────────────────────────────────────────────────────────┘
```

Platform discovery results are **hints**. Application discovery results are
**facts about the peer** — and even those facts are claims, validated per
[`protocol.md` §7](./protocol.md).

---

## 2. Capability Surface

Every platform adapter exposes, conceptually:

```text
is_supported()
is_enabled()
discover_peers()
connect(peer)
disconnect(peer)
get_connection_info()
```

The core receives **normalized values**, never platform objects.

```rust
// Sketch — shape only.
pub trait PeerDiscoveryAdapter {
    fn is_supported(&self) -> bool;
    fn is_enabled(&self) -> bool;
    async fn discover_peers(&self) -> Result<Vec<PlatformPeer>, DiscoveryError>;
    async fn connect(&self, peer: &PlatformPeer) -> Result<ConnectionInfo, DiscoveryError>;
    async fn disconnect(&self, peer: &PlatformPeer) -> Result<(), DiscoveryError>;
    fn get_connection_info(&self, peer: &PlatformPeer) -> Option<ConnectionInfo>;
}
```

---

## 3. The Compatibility Rule

> The system **MUST NOT** claim "works on every device."

Wi-Fi Direct support and behaviour are platform-, OS-, driver-, and
hardware-dependent. Support is verified per device, not assumed per platform.

The application must **detect capability** and expose it. A device that cannot do
Wi-Fi Direct must be able to say so clearly, at startup, rather than failing
mysteriously at connection time.

---

## 4. Normalized Types

The FFI boundary exchanges only simple, portable values:

```text
Peer · ConnectionInfo · TransportHandle · Capability · Error · Event
```

Platform objects must never leak into the core — see
[`architecture.md` §7](./architecture.md) and
[`threat-model.md` §3](./threat-model.md) Boundary 2.

```text
Good:  ConnectionInfo { ip: IpAddr, port: u16, interface: InterfaceKind }
Bad:   ConnectionInfo { winrt_WiFiDirectDevice: WiFiDirectDevice, ... }
```

---

## 5. Application-Layer Discovery

Once connected, peers determine:

- Whether the peer runs this software
- Protocol version
- Supported capabilities
- Device identity
- Supported features

Handled by the `Hello` / `VersionNegotiate` / `Capabilities` exchange in
[`protocol.md` §2](./protocol.md).

### 5.1 Untrusted claims

A peer can claim anything. Every advertised capability is validated before use:

| Claim | Validation |
| --- | --- |
| `supports_resume` | Observed behaviour during the transfer |
| `supports_chunk_verification` | Chunk hashes actually present and correct |
| `max_chunk_size` | Clamped to our own configured maximum |
| `supports_encryption` | Handshake actually completed |

---

## 6. Device Identity

Handled in [`security.md` §2](./security.md). Discovery-relevant summary:

- Persistent cryptographic identity, generated on first run.
- MAC address, IP address, and hostname are **network attributes**, used as
  display hints only — never as identity.

---

## 7. Offline-First Constraints

Discovery must work with **no** cloud service, **no** central registry, and
**no** internet.

```text
Internet = OFF
Wi-Fi Direct = ON
P2P = WORKING
```

This is a valid state and the target state. Discovery code must never infer
"no peers available" from failed internet reachability — those are unrelated
systems.

---

## 8. Platform-Specific Notes

| Platform | Mechanism | Detail |
| --- | --- | --- |
| Windows | `Windows.Devices.WiFiDirect` | [`platform-windows.md`](./platform-windows.md) |
| Android | `WifiP2pManager` | [`platform-android.md`](./platform-android.md) |
| Linux | `wpa_supplicant` P2P API | [`platform-linux.md`](./platform-linux.md) |

---

## 9. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §2, §3, §4 | Charter §15, §6, §47 |
| §5, §5.1 | Charter §15, §67 |
| §6 | Charter §16 |
| §7 | Charter §43, §44, §45 |
| §8 | Charter §46 |