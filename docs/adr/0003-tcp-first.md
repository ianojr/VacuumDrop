# ADR-0003 — TCP Before QUIC

**Status:** Accepted
**Phase:** 1

---

## Context

The engine could use QUIC from the start, offering TLS 1.3, multiplexed streams,
and connection migration.

The Charter is explicit:

> Do not immediately implement QUIC. First prove that the protocol works
> reliably over TCP. After the TCP implementation is stable, evaluate QUIC
> experimentally. Any change from TCP to QUIC must be based on measured
> requirements, not hype.

A realistic motivation for QUIC exists: connection migration when a peer moves
between radios or access points is listed among expected failure modes.

## Decision

Implement the protocol over **TCP** first. Evaluate QUIC experimentally **only
after** the TCP implementation is stable, and adopt it only if measurement
demonstrates a requirement it satisfies.

## Alternatives Considered

| Alternative | Why rejected *now* |
| --- | --- |
| QUIC from day one | Two hard problems (transport reliability + protocol correctness) at once. Bugs become unattributable. QUIC also duplicates our own framing and handshake design (see [ADR-0007](./0007-serialization-and-framing.md), [ADR-0010](./0010-handshake-protocol.md)). |
| WebTransport | Requires an HTTP/3 stack; inappropriate for a pure P2P engine |
| QUIC for data, TCP for control | Two transports, two failure models — complexity without measured justification |

## Consequences

**Positive**

- Bugs are attributable: protocol bugs vs transport bugs.
- Framing, backpressure, and resume are validated on the simplest possible
  substrate.
- Wi-Fi Direct hands us IP; TCP is universally available over it.
- The transport abstraction means a later QUIC adapter is additive.

**Negative**

- Multiplexed streams unavailable, so multiple concurrent transfers share one
  connection initially — mitigated by bounded concurrency.
- Head-of-line blocking exists between streams on a shared connection.
- Connection migration is not handled initially; treated as a disconnect and
  resumed instead.

## Revisit Trigger

Re-evaluate when **any** of these become true:

1. Measured resume cost after peer movement is unacceptable.
2. Concurrent-transfer throughput degrades due to head-of-line blocking.
3. The Wi-Fi Direct layer cannot keep a TCP connection alive reliably.

## Related

[ADR-0002 — Tokio](./0002-tokio.md), [ADR-0004 — Transport abstraction](./0004-transport-abstraction.md)