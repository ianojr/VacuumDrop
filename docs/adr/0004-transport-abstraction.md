# ADR-0004 — Pluggable Transport Abstraction

**Status:** Accepted
**Phase:** 1

---

## Context

The engine's first transport is Wi-Fi Direct, which cannot be driven portably
from Rust. Each OS provides its own peer discovery and connection establishment,
and hands back an IP address.

Additional transports are planned: Wi-Fi Aware, Ethernet, USB.

If Wi-Fi Direct concepts leak into the core, every future transport becomes a
core rewrite.

## Decision

Define a narrow transport abstraction over the concepts common to all transports:

```text
Transport
    connect()
    accept()
    send()
    receive()
    shutdown()
```

Wi-Fi Direct specifics live **entirely** in the platform adapter. The adapter
resolves a connection and returns an IP and port; the core opens TCP and runs
the protocol.

The abstraction contains **no** Wi-Fi Direct concepts.

## The naming test

| Bad | Good |
| --- | --- |
| `connect_wifi_direct_group_owner()` | `connect(peer)` |
| `set_go_intent()` | *(not in the core at all)* |
| `wait_for_p2p_group_formation()` | *(not in the core at all)* |

> If a name would be meaningless over Ethernet or USB, it does not belong in this
> trait.

## Alternatives Considered

| Alternative | Why rejected |
| --- | --- |
| Hard-code Wi-Fi Direct | Directly contradicts the pluggable-transport requirement; every future transport is a rewrite |
| Abstract at the socket level only (`AsyncRead + AsyncWrite`) | Too thin — loses connection lifecycle, timeouts, and shutdown semantics the core needs |
| Adapter implements the whole protocol | Duplicates protocol logic per platform; breaks Rule 5 |
| gRPC-like RPC abstraction | Misfit — the protocol is a stateful stream, not request/response |
| `trait Transport { type Stream }` with GATs | Considered; may become necessary for parallel streams in Phase 5. Deferred until measured. |

## Consequences

**Positive**

- Phase 3 (Wi-Fi Direct) requires **no** protocol changes. This is the
  acceptance test for this ADR.
- New transports are additive adapters.
- Core protocol code is testable entirely over localhost.
- Platform bugs and protocol bugs stay separable.

**Negative**

- The abstraction can be too thin in practice — connection establishment, retry,
  and radio-resume handling are awkward to express generically.
- Some platform-specific operations cannot be abstracted without becoming
  meaningless. Those must be pushed into the adapter and surfaced as normalised
  values, not leaked as optional trait methods.

## Validation

**Primary test:** implement the Windows Wi-Fi Direct adapter (Phase 3) and confirm
the protocol layer requires no changes. If it does, this ADR has failed.

Secondary: the Phase-0/1/2 progression runs `localhost → LAN` on identical
protocol code — see [`../roadmap.md` §1](../roadmap.md).

## Related

[ADR-0003 — TCP first](./0003-tcp-first.md), [`../transport.md`](../transport.md), [`../discovery.md`](../discovery.md)