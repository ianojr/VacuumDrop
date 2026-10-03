# ADR-0009 - Windows Wi-Fi Direct Binding Strategy

**Status:** **Accepted** (ratified 2026-10-04, after the Phase-3 spike)
**Phase:** 3

---

## Context

Phase 3 requires a Windows Wi-Fi Direct adapter in Rust. The modern API is the
WinRT `Windows.Devices.WiFiDirect` namespace. Rust has no built-in WinRT support,
and the Charter forbids obsolete approaches (Hosted Network, IBSS ad-hoc) in
favour of modern APIs.

The binding choice must satisfy the portability rule: WinRT types must not leak
into the core.

## Decision

The `windows` crate, confined entirely to `crates/vacuumdrop-platform-windows`.

```text
crates/vacuumdrop-platform-windows
    - depends on `windows` 0.62.2 and `windows-future` 0.3
    - contains all WinRT and `unsafe` (of which we write none)
    - exposes only: Adapter . ConnectionInfo . DiscoveredPeer . Capability
                    . AdapterError

crates/vacuumdrop-core, -protocol, -transport, -security, -transfer
    - zero knowledge of WinRT
```

**Escalation path (not needed):** if WinRT async bridging had proven to be a
persistent blocker, the fallback is a thin C#/WinRT helper component behind a
stable C ABI. The spike showed this is unnecessary. Do **not** work around a
future problem by leaking WinRT types upward.

## Phase-3 Spike Results

The four questions the draft ADR posed. Two of them were **wrong questions**,
which is itself the most valuable output of a spike.

### 1. Can the publisher and list watcher be driven from Rust?

**Yes for the publisher. No watcher type exists — and none is needed.**

- `WiFiDirectAdvertisementPublisher` **is** bound, together with
  `WiFiDirectAdvertisementPublisherStatusChangedEventArgs` and
  `WiFiDirectConnectionRequestedEventArgs`. So group formation and the
  connection-request event model are reachable from Rust.
- **`WiFiDirectDeviceListWatcher` is NOT in `windows` 0.62.2**, and neither is
  `WiFiDirectAdvertisementListener`. The draft ADR named a type the binding does
  not provide, so it could never have been called from the direct binding.

  **Neither is required.** The draft ADR assumed a watcher was necessary for
  discovery. That was the single most over-pessimistic conclusion of the spike: a
  `DeviceInformation` watcher is only needed for *continuous* updates. A one-shot
  scan is `DeviceInformation::FindAllAsyncAqsFilter`, which returns an
  `IAsyncOperation` and awaits exactly like every other call here. Discovery is
  implemented and verified to run on real hardware with no events, no callbacks,
  and no message pump.

  Continuous watch would still need `DeviceInformation::CreateWatcher` plus
  `DeviceWatcher::Added`. That is deferred, not blocked.

### 1a. Making the device findable

`WiFiDirectAdvertisementPublisher::new()` + `SetListenStateDiscoverability(Normal)`
+ `Start()` makes a device discoverable, with no listener object. The publisher
must be kept alive, so it is held in an RAII guard that stops advertising on drop —
otherwise a program that made the machine discoverable could exit leaving it
discoverable.

`WiFiDirectAdvertisementPublisherStatus` has only `Created`, `Started`, `Stopped`,
and `Aborted` — there is no "Unsupported" value, which is why object *creation* is
the meaningful group-owner capability test.

### 2. How are `IAsyncOperation` completions marshalled?

**Cleanly, with no message pump and no helper.**

`windows-future` 0.3 implements `IntoFuture` for `IAsyncOperation<T>`
(`src/future.rs:96`), so the operation becomes a plain `Future` that a Tokio
executor drives.

One sharp edge, and it is an easy one to get wrong: `IAsyncOperation<T>`
implements **`IntoFuture`, not `Future`**. Calling `.await` directly does not
compile, and the fix is `.into_future().await`. A naive "the binding looks
missing" reading leads to abandoning a perfectly good API.

`tokio::time::timeout` composes over it directly, which is how
`async_op::await_op` imposes the connection deadline.

### 3. Is static-IP configuration required after group formation, or is DHCP sufficient?

**DHCP appears sufficient, per the API; not yet confirmed on hardware.**

After group formation, `WiFiDirectDevice::GetConnectionEndpointPairs()` yields
`EndpointPair` values whose `RemoteHostName()` carries the peer's DHCP-assigned
address. `ConnectByIdAsync2` even accepts an optional `HostName` to pin a
specific interface.

So no manual static-IP path appears necessary. **This is the one spike answer
that still needs real hardware behind it** before it is treated as settled.

### 4. What is the practical `unsafe` count?

**Zero in our code.** The draft ADR claimed the approach "requires `unsafe`". It
does not, as long as only the hand-written layer is counted: the crate compiles
under `#![forbid(unsafe_code)]` and contains no `unsafe` block. The generated
bindings do use `unsafe` internally, which is upstream's problem and is not
counted against this boundary.

## Corrections to the Draft ADR

Recorded because a ratified ADR that silently keeps a false claim is worse than
no ADR:

| Draft claim | Reality |
| --- | --- |
| "`WiFiDirectDeviceListWatcher` must be driven from Rust" | Type absent from the binding. Not needed either: a one-shot `FindAllAsyncAqsFilter` scan covers discovery without any watcher |
| "Requires `unsafe`" | Zero `unsafe` in our code; `#![forbid(unsafe_code)]` holds |
| "WinRT async bridging is awkward" | `IntoFuture` + `tokio::time::timeout`; this was the least risky part |
| Public surface `Peer . Event` | Shipped as `Adapter . DiscoveredPeer`; no event type exists yet |
| Discovery "needs the watcher machinery" | Implemented with a one-shot enumeration; the watcher is only for continuous updates |

## A Platform Limitation Found

`WiFiDirectDevice` exposes `DeviceId`, `ConnectionStatus`, and
`GetConnectionEndpointPairs` — and **no friendly-name accessor at all**. The
Wi-Fi Direct protocol carries no device name.

So `DiscoveredPeer` reports the device id as its label. That is the only
identifier the platform provides, and a user-facing name belongs to the Phase 4
VacuumDrop handshake anyway: any name the OS handed us would be peer-controlled
text with no way to validate it.

## Alternatives Considered

| Alternative | Pros | Cons |
| --- | --- | --- |
| **`windows` crate (chosen)** | In-process; no IPC; generated and maintained by Microsoft; no `unsafe` in our code | Large crate; one named type missing from the binding |
| C#/WinRT helper component | Idiomatic WinRT; async is natural | Second project; IPC boundary; deployment complexity. Spike showed it is unnecessary |
| `windows` crate + `wrl`/`com` hand-rolled | Full control | Highest risk; slowest to build; not justified |
| Third-party Wi-Fi Direct wrapper crate | Small surface | Unverified; likely thin or unmaintained; dependency risk per Charter 54 |
| PowerShell / script host | No build step | Not production-appropriate |

## Consequences

**Positive**

- No IPC hop in the data path.
- The heavy dependency is quarantined in one crate, so the blast radius is
  contained and auditable.
- Async is ordinary Rust async, so deadlines and cancellation are free.

**Negative**

- The `windows` crate is large; the Charter 54 justification is that there is no
  reasonable alternative for WinRT from Rust.
- Continuous peer discovery needs the `DeviceInformation` watcher. One-shot
  scanning is implemented; watching is **not**.
- No device name from the platform, by protocol limitation.
- Binding only the P2P interface on the host side needs
  `Windows.Networking.Connectivity`, which is not yet wired up. The host half of
  the test rig uses `0.0.0.0`, which also accepts connections from the normal
  network. That is acceptable for an isolated test rig and is **not** the shipping
  configuration.

## Required Alongside This Decision

1. Zero WinRT types in any public signature crossing into `vacuumdrop-core`.
   **Enforced** by the `WinRT boundary discipline` gate in
   [`../../scripts/check.ps1`](../../scripts/check.ps1), which scans every
   `cfg` variant and whitelists exactly one `pub(crate)` function.
2. Dependency-direction check, same script. **Enforced.**
3. Capability detection must reflect real hardware, not merely a Wi-Fi radio's
   presence. **Not yet satisfied** - see the open item below.
4. Platform quarantine: only `vacuumdrop-platform-*` crates may depend on
   `windows`. **Enforced** by the `platform quarantine` gate.

## Open Item Carried Forward

Capability detection asks two separate questions — can we enumerate peers, and can
we act as group owner — because a non-empty AQS selector does **not** prove usable
hardware. Both are now answered by real API calls rather than assumptions.

Still open, and honestly recorded:

- Whether DHCP assignment behaves on a real formed group (API suggests yes).
- Whether a scan matches a genuinely advertising peer (the scan runs; a match has
  not been observed).

## Superseded Requirement

Requirement 3 originally read "capability detection must reflect real hardware,
not merely a Wi-Fi radio's presence." That is now implemented: the probe requires
`WiFiDirectAdvertisementPublisher::new()` to succeed and report `Created`, which
is a driver-level group-owner test, not a radio-presence test.

## Related

[`../platform-windows.md`](../platform-windows.md),
[ADR-0004 - Transport abstraction](./0004-transport-abstraction.md)
