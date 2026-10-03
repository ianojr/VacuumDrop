# Transport Layer

Status: **Phase 1.** Rationale in
[ADR-0004](./adr/0004-transport-abstraction.md).

---

## 1. Purpose

The transport layer is the boundary between "how bytes move" and "what the bytes
mean". The core protocol must be completely unaware of Wi-Fi Direct, and the
Wi-Fi Direct adapter must be completely unaware of the VacuumDrop protocol.

```text
Protocol  ──needs──►  Transport  ◄──provides──  Socket
                                              ▲
                                     Adapter supplies the
                                     socket's address
```

---

## 2. Abstraction

```text
Transport
    connect()
    accept()
    send()
    receive()
    shutdown()
```

The exact Rust trait is determined during implementation. Sketch, for shape
only:

```rust
// Sketch — not a contract. See ADR-0004.
pub trait Transport: Send + Sync {
    async fn send(&self, frame: Frame) -> Result<(), TransportError>;
    async fn receive(&mut self) -> Result<Frame, ProtocolError>;
    async fn shutdown(&self) -> Result<(), TransportError>;
    fn connection_info(&self) -> ConnectionInfo;
}
```

### 2.1 Naming rule (normative)

The abstraction must not contain Wi-Fi Direct-specific concepts.

| Bad | Good |
| --- | --- |
| `connect_wifi_direct_group_owner()` | `connect(peer)` |
| `wifi_p2p_negotiate_channel()` | `connect(peer)` |
| `set_go_intent()` | *(does not belong in the core at all)* |

**Test for leakage:** if a name would be meaningless on Ethernet or USB, it does
not belong in this trait.

---

## 3. Where the Socket Comes From

The adapter establishes the peer-to-peer connection and resolves an
**IP address and port**. The core then opens its own TCP connection to it.

```text
1. Adapter discovers peers          (OS-level)
2. Adapter establishes P2P link     (OS-level)
3. Adapter returns IP + port        (normalized values)
4. Core opens TCP connection        (Rust)
5. Core runs VacuumDrop protocol    (Rust)
```

The radio is irrelevant from step 4 onward. This is what makes the protocol
portable, and it is the property to preserve when adding transports.

---

## 4. Runtime

Tokio is the preferred initial runtime (see [ADR-0002](./adr/0002-tokio.md)).

| Tokio provides | Used for |
| --- | --- |
| Async sockets | `TcpStream` / `TcpListener` |
| Task scheduling | Session and transfer tasks |
| Timers | Timeouts and retry backoff |
| Cancellation | `select!` / cancellation tokens on shutdown |
| Concurrent transfers | One task per transfer, bounded |
| Async filesystem | Only where it genuinely helps |

### 4.1 Do not sprinkle Tokio without reason

> Use asynchronous execution where it provides meaningful value.

`std::fs` for a single metadata query does not need an async wrapper. Async pays
for itself when work genuinely waits on I/O that would otherwise block a
runtime thread. Every blocking call on a runtime thread is a throughput bug
waiting to happen.

Note: the Phase-0 benchmark crate uses `tokio` with `full` for expedience. The
Phase-1 workspace selects features per crate.

---

## 5. TCP First

```text
Wi-Fi Direct
     │
     ▼
    TCP
     │
     ▼
Rust protocol
```

**Do not implement QUIC immediately.** First prove the protocol works reliably
over TCP.

QUIC is evaluated **experimentally, after** the TCP implementation is stable, and
only if measurement shows a concrete requirement it satisfies (for example:
connection migration when a peer moves between radios — a scenario explicitly
listed as an expected failure mode).

> Any change from TCP to QUIC must be based on **measured requirements, not
> hype.**

See [ADR-0003](./adr/0003-tcp-first.md).

---

## 6. Timeouts (Charter §65)

Network operations must never wait forever.

| Operation | Timeout |
| --- |
| Connection establishment | Required |
| Handshake / version negotiation | Required |
| Authentication | Required |
| Metadata exchange | Required |
| Chunk acknowledgement | Required where applicable |
| Idle session | Required |
| Graceful shutdown | Bounded |

Values are configurable and documented. Every one of these maps to a threat in
[`threat-model.md` §4](./threat-model.md) — T-19 in particular.

---

## 7. Connection Lifecycle

```text
DISCONNECTED → CONNECTING → CONNECTED → AUTHENTICATING
             → ESTABLISHED → TRANSFERRING → COMPLETED → DISCONNECTED
```

Invalid transitions are rejected by construction where the type system allows,
and explicitly otherwise. See [`protocol.md` §4.1](./protocol.md).

Every failure listed in [`troubleshooting.md`](./troubleshooting.md) has a
defined transition out of this lifecycle.

---

## 8. Failure Assumption

The system assumes connections **will** fail. Expected failure modes:

```text
peer moves away · Wi-Fi disabled · device sleeps · OS kills process
environment changes · radio interference · interface disappears
receiver crashes · sender crashes · disk becomes unavailable
```

Design consequences:

- No operation assumes it will complete.
- No state is held only in memory if losing it corrupts a transfer
  (see [`file-transfer.md` §6](./file-transfer.md)).
- Errors are typed and mapped to state transitions, never ignored.

---

## 9. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §2, §3 | Doc 1 §2–§3; Charter §12 |
| §2.1 | Charter §12 (bad/good naming) |
| §4, §4.1 | Charter §13 |
| §5 | Charter §14; Doc 1 |
| §6 | Charter §65 |
| §7, §8 | Charter §40, §42 |