# ADR-0009 - Windows Wi-Fi Direct Binding Strategy

**Status:** **Proposed - Phase 3 spike required before ratification**
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

**Proposed:** the `windows` crate, confined entirely to
`crates/vacuumdrop-platform-windows`.

```text
crates/vacuumdrop-platform-windows
    - uses `windows` crate, WinRT types, `unsafe`
    - exposes only: Peer . ConnectionInfo . Capability . Error . Event

crates/vacuumdrop-core, -protocol, -transport, -security, -transfer
    - zero knowledge of WinRT
```

**Escalation path:** if WinRT `IAsyncOperation` bridging proves to be a
persistent blocker, switch to a thin C#/WinRT helper component behind a stable
C ABI. Do **not** work around it by leaking WinRT types upward.

## Alternatives Considered

| Alternative | Pros | Cons |
| --- | --- | --- |
| **`windows` crate** | In-process; no IPC; generated; actively maintained by Microsoft | Large crate; WinRT async bridging is awkward; requires `unsafe` |
| C#/WinRT helper component | Idiomatic WinRT; async is natural; small Rust surface | Second project; IPC boundary; deployment complexity |
| `windows` crate + `wrl`/`com` hand-rolled | Full control | Highest risk; slowest to build; not justified for a hand-written WinRT wrapper |
| Third-party Wi-Fi Direct wrapper crate | Small surface | Unverified; likely thin or unmaintained; dependency risk per Charter 54 |
| PowerShell / script host | No build step | Not production-appropriate |

## Consequences

**Positive**

- No IPC hop in the data path.
- The heavy dependency is quarantined in one crate, so the blast radius is
  contained and auditable.
- Escalation to a helper component is possible without touching the core.

**Negative**

- The `windows` crate is large - a Charter 54 justification must be recorded
  (the justification is: there is no reasonable alternative for WinRT from
  Rust).
- WinRT async in Rust is the known risk. The Phase-3 spike exists to find out
  how bad it is **before** committing.
- Requires `unsafe`, all of it confined to this crate.

## Required Alongside This Decision

1. Zero WinRT types in any public signature crossing into `vacuumdrop-core`.
2. Dependency-direction check in CI
   ([`../testing.md` section 9](../testing.md)).
3. Capability detection must reflect real hardware, not merely a Wi-Fi radio's
   presence.
4. Every `unsafe` block carries the justification required by Charter 88.

## Phase-3 Spike (must happen first)

Before ratifying, answer:

- Can `WiFiDirectAdvertisementPublisher` and `WiFiDirectDeviceListWatcher` be
  driven from Rust, or do the event/callback models require a helper?
- How are `IAsyncOperation` completions marshalled without a message pump?
- Is static-IP configuration required after group formation, or is DHCP
  sufficient?
- What is the practical `unsafe` count?

**Revisit this ADR after the spike.** If the answer to the first question is
"no", switch to the helper component.

## Related

[`../platform-windows.md`](../platform-windows.md),
[ADR-0004 - Transport abstraction](./0004-transport-abstraction.md)