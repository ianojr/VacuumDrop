# VacuumDrop Protocol Specification

Status: **Draft.** The Phase-2 message set is sketched; the wire format and
catalogue below are normative where marked **MUST**.

> **Scope note.** This document merges three sources:
> [Doc 3 — Protocol Design Spec](#), the message catalogue in Charter §9.2, and
> the framing/versioning/capability rules in Charter §10, §11, §66, §67.
> Where the sources disagree, the disagreement is recorded explicitly in
> [§9 Conflicts and required decisions](#9-conflicts-and-required-decisions)
> rather than silently resolved.

---

## 1. Overview

The system moves from raw byte streams to a **structured, stateful, framed,
versioned** protocol over TCP.

Two properties dominate the design:

1. **The peer is hostile.** Every field on the wire is attacker-controlled.
2. **TCP is a byte stream.** `write(n)` does not imply `read(n)`.

Both properties are non-negotiable and are treated as first-class design inputs,
not as afterthoughts.

---

## 2. Message Catalogue

The union of both source lists. `Phase` is the earliest phase in which the
message is required.

| # | Message | Direction | Phase | Status |
| --- | --- | --- | --- | --- |
| 1 | `Hello` | client → server | 1 | Specified (Doc 3) |
| 2 | `HelloAck` | server → client | 1 | Specified (Doc 3) — **needs revision**, see C-2 |
| 3 | `VersionNegotiate` | both | 1 | **Required** by Charter §66, missing from Doc 3 |
| 4 | `Capabilities` | both | 1 | **Required** by Charter §67, missing from Doc 3 |
| 5 | `AuthRequest` | client → server | 4 | Charter §9.2 |
| 6 | `AuthResponse` | server → client | 4 | Charter §9.2 |
| 7 | `SessionEstablished` | server → client | 4 | Charter §9.2 |
| 8 | `TransferRequest` | client → server | 5 | Charter §9.2 |
| 9 | `TransferAccept` | server → client | 5 | Charter §9.2 |
| 10 | `TransferReject` | server → client | 5 | Charter §9.2 |
| 11 | `FileMetadata` | client → server | 1/5 | Specified (Doc 3) |
| 12 | `ReadyToReceive` | server → client | 5 | Specified (Doc 3) |
| 13 | `Chunk` | client → server | 5 | Specified (Doc 3) — **needs revision**, see C-3 |
| 14 | `ChunkAck` | server → client | 5 | Specified (Doc 3) |
| 15 | `TransferPause` | both | 5 | Charter §9.2 — **missing from Doc 3** |
| 16 | `TransferResume` | both | 5 | Charter §9.2 — **missing from Doc 3** |
| 17 | `TransferCancel` | both | 5 | Derived from Charter §64 — **missing from Doc 3** |
| 18 | `TransferComplete` | client → server | 5 | Specified (Doc 3) |
| 19 | `TransferVerify` | both | 5 | Charter §9.2 — **missing from Doc 3** |
| 20 | `Ping` | both | 1 | Charter §9.2 |
| 21 | `Pong` | both | 1 | Charter §9.2 |
| 22 | `Error` | both | 1 | Charter §9.2 |
| 23 | `Goodbye` | both | 1 | Charter §9.2 |

**Observation.** Doc 3's `VacuumDropMessage` enum is a *subset* covering only the
transfer happy path. It is sufficient for a Phase-2 prototype but **not** for the
system described by the Charter: it has no version negotiation, no session or
request identifiers, no capability exchange, no authentication, no pause/resume,
and no verification step. Treat it as the **starting subset**, not the protocol.

---

## 3. Wire Format

### 3.1 Frame layout (normative)

```text
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        Magic  (u32)                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|          Protocol Version  (u16) |          Flags  (u16)     |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                       Message Type (u16)                     |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
~                    Session ID  (128-bit)                     ~
~                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
~                     Request ID  (u64)                        ~
~                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                     Payload Length  (u32)                    |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
~                          Payload                             ~
~                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

Header size: **38 bytes**, fixed.

| Field | Type | Notes |
| --- | --- | --- |
| Magic | `u32` | Reject non-matching immediately; cheap DoS filter |
| Protocol Version | `u16` | Offered version, see §6 |
| Flags | `u16` | Reserved bits **MUST** be rejected if unknown |
| Message Type | `u16` | Unknown types **MUST** produce `Error`, never a panic |
| Session ID | 128-bit | Correlates all frames in a session |
| Request ID | `u64` | Correlates request/response pairs |
| Payload Length | `u32` | **MUST** be validated *before* allocation — see §5 |

### 3.2 Framing rules (normative)

- The transport is a byte stream. The receiver **MUST** reconstruct frames
  incrementally and **MUST NOT** assume one write equals one read.
- Implement with `tokio_util::codec::Framed` +
  `tokio_util::codec::LengthDelimitedCodec`, with `max_frame_length` set from the
  configured limit table.
- Never depend on implicit message sizes.
- Never deserialize untrusted data without validating lengths first.

---

## 4. State Machines

### 4.1 Connection lifecycle (Charter §40)

```text
DISCONNECTED
     |
     v
DISCOVERING
     |
     v
PEER_FOUND
     |
     v
CONNECTING
     |
     v
CONNECTED
     |
     v
AUTHENTICATING
     |
     v
ESTABLISHED
     |
     v
TRANSFERRING
     |
     v
COMPLETED
     |
     v
DISCONNECTED
```

Invalid state transitions **MUST** be rejected. This is enforced by the Rust
type system where practical — see
[ADR-0001](./adr/0001-rust-core.md) and
[Charter §87](./engineering-standards.md): model states as an enum, not
`status: String`.

### 4.2 Message exchange order (Doc 3)

```text
Discovery:  Hello            -> HelloAck
Setup:      FileMetadata     -> ReadyToReceive
Stream:     Chunk(0)         -> ChunkAck(0)
            Chunk(1)         -> ChunkAck(1)
            ...
Finalize:   TransferComplete -> Close socket
```

### 4.3 Session lifecycle (Charter §41)

```text
create → authenticate → establish → use → pause → resume → close
                                                        ↘ fail
                                                        ↘ expire
```

Every operation that could otherwise wait forever **MUST** have a timeout
(Charter §65).

---

## 5. Resource Limits

Charter §35 and §36 require explicit limits on every endpoint. Proposed
defaults — **all configurable**, all enforced *before* allocation:

| Limit | Proposed default | Enforced at |
| --- | --- | --- |
| Max frame length | 32 MiB | `LengthDelimitedCodec` |
| Max metadata payload | 64 KiB | message decoder |
| Max filename length | 255 bytes | `FileMetadata` validation |
| Max simultaneous sessions | 8 | session manager |
| Max simultaneous transfers | 4 | transfer manager |
| Max in-flight chunks (window) | 8 | transfer window |
| Max auth attempts before cooldown | 5 | security manager |
| Max connections / minute | 30 | transport |
| Max queued chunks | bounded, no unbounded channels | transfer |

A peer **MUST NOT** be able to cause unbounded allocation. In particular, a
declared `file_size` of 18 exabytes **MUST NOT** trigger any allocation or
reservation (Charter §36).

---

## 6. Version Negotiation

Every message carries a version. Sessions negotiate explicitly (Charter §66).

```text
Peer A: supported versions [1, 2]
Peer B: supported versions [1, 2]
Negotiated: 2
```

On failure: `Error(UnsupportedProtocolVersion)`. **Never** silently downgrade to
an insecure protocol version.

---

## 7. Capability Negotiation

Peers advertise (Charter §67):

```text
supports_resume
supports_parallel_streams
supports_compression
supports_encryption
supports_chunk_verification
max_chunk_size
protocol_versions
```

**Capabilities are claims, not facts.** A remote device can assert anything. The
implementation **MUST** validate that a requested feature is genuinely supported
before acting on it.

---

## 8. Serialization

Doc 3 specifies `serde` + `bincode` with `tokio-util` framing. Charter §54/§55
require evaluating dependencies before adoption and forbid hand-rolling
primitives.

Selection is **deferred to [ADR-0007](./adr/0007-serialization-and-framing.md)**,
which compares `bincode` against `postcard`, `rmp-serde`, and manual
little-endian encoding. See conflict **C-1**.

The crate is **not** pinned here — Charter §89 forbids hard-coding obsolete
versions into the specification.

---

## 9. Conflicts and required decisions

These are genuine contradictions between the source documents. They are recorded
here, and in [`open-questions.md`](./open-questions.md), rather than being
resolved unilaterally.

### C-1 — Serialization format is both mandated and conditional

- **Doc 3** mandates `bincode`.
- **Charter §54** says do not add a crate merely because it is convenient;
  evaluate first.
- **Charter §89** says the AI should *evaluate* serialization libraries, not
  install them blindly.

`bincode` also has a breaking 2.x API change, so Doc 3's code is not portable as
written. **Decision needed:** ratify `bincode` in ADR-0007, or select an
alternative.

### C-2 — `HelloAck { status: String }` violates the Charter

```rust
HelloAck { status: String }
```

This is a free-form `String` on an untrusted boundary. It conflicts with:

- **§32** — never trust device-supplied strings.
- **§87** — prefer strong enums over `String` state.
- **§35** — no unbounded values from the wire.

**Proposed replacement:** a typed status enum plus a bounded, structured error
payload.

### C-3 — `Chunk { data: Vec<u8> }` is an unbounded-allocation vector

```rust
Chunk { index: u32, data: Vec<u8> }
```

Because the payload length is read from the wire, `bincode` will allocate a
`Vec` sized by an attacker-controlled length *before* any validation. This
directly violates Charter §33 (unbounded allocations), §35 (max message size),
and §36 (memory-exhaustion DoS).

**Proposed replacement:** enforce `max_frame_length` at the codec, validate
`len(data) == negotiated chunk_size` *before* decoding the body, and — for the
hot path — stream chunk bodies without materializing them, so peak memory is
governed by buffer count and not by chunk count.

### C-4 — Doc 3 has no version, session, or capability metadata

Charter §10 requires every message to carry version, type, length, session ID,
and request ID. Doc 3's bare enum has none of these. Resolved by adopting §3.1
as normative and treating Doc 3's enum as the Phase-2 subset only.

### C-5 — Doc 3 omits pause / resume / verify / auth

Charter §64 makes cancellation mandatory for every long-running operation, §24
requires resume, and §25 requires verification before completion. Doc 3 has no
messages for any of these. They are carried in the catalogue (§2) as additions to
be designed during their respective phases.

---

## 10. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §2, §4.2, C-1, C-2, C-3, C-5 | Doc 3 — Protocol Design Spec |
| §2, §4.1, §4.3, §5, §6, §7 | Charter §9.2, §10, §11, §35, §36, §40, §41, §66, §67 |