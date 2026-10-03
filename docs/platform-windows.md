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
is_supported()     # does this machine have a Direct-capable adapter?
is_enabled()       # is Wi-Fi Direct currently enabled/policy-allowed?
discover_peers()
connect(peer)
disconnect(peer)
get_connection_info()
```

Detection must reflect **actual hardware/driver capability**, not merely the
presence of a Wi-Fi radio. A machine with an 802.11n adapter may support Wi-Fi
Direct in name only.

Support is a per-device, per-driver property. Never claim "works on every
device."

---

## 5. Known Environment Variables

| Failure | Symptom | Handling |
| --- | --- | --- |
| Adapter lacks P2P | `discover_peers()` returns empty, always | Report unsupported, not "no peers" |
| Policy blocks P2P | Discovery starts, no results | Distinguish from unsupported |
| Driver/VIF creation fails | Connect error | Surface driver-specific error text |
| Interface disappears mid-transfer | Transfer aborts | Must produce a defined state transition ([`troubleshooting.md`](./troubleshooting.md)) |

---

## 6. Test Procedure

Two Windows devices, both with Direct-capable adapters.

```text
1. Confirm is_supported() == true on both
2. Device A: publish advertisements
3. Device B: run discovery, confirm Device A appears
4. Device B: initiate connect, wait for group formed
5. Read ConnectionInfo -> IP + port
6. Hand to Rust core: p2p connect <ip:port>
7. Run the Phase-1 protocol over it, unchanged
```

Step 7 is the acceptance test for
[ADR-0004](./adr/0004-transport-abstraction.md): **the protocol must require no
changes.** If it does, the abstraction leaked.

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

| Question | Reference |
| --- | --- |
| `windows` crate vs helper component | [ADR-0009](./adr/0009-windows-wifi-direct-binding.md) |
| Static IP vs DHCP-allocated IP after group formation | Phase 3 spike |
| IPv6 support | Deferred |
| Whether to ship a GUI settings page for adapter diagnostics | Phase 8 |

---

## 9. Source Traceability

| Section | Origin |
| --- | --- |
| 1, 2, 4 | Charter 46 (Windows), 6 |
| 3 | Charter 54, 47 |
| 5 | Charter 6, 42 |
| 6, 7 | Charter 77, 76 |
| 2 (hotspot caveat) | Doc 2 section 4 |