# Roadmap & Milestones

Status: **Normative.** This document defines what gets built, in what order, and
what "done" means for each phase.

---

## 1. The Layered Progression

The project advances strictly downward through this ladder. **Do not skip
layers.**

```text
localhost
    ↓
LAN
    ↓
Wi-Fi P2P
    ↓
secure P2P
    ↓
reliable file transfer
    ↓
cross-platform
```

Each rung exists to isolate one class of bug from the next. Skipping a rung
means debugging protocol bugs and radio bugs simultaneously, which is exactly the
failure mode this ordering prevents.

---

## 2. MVP Definition

The MVP is **not** a polished consumer application.

```text
Windows device A
        │
    Wi-Fi Direct
        │
Windows device B
        ↓
Rust TCP communication
        ↓
Authenticated session
        ↓
Send file
        ↓
Chunked transfer
        ↓
Integrity verification
        ↓
Successful reconstruction
```

### 2.1 MVP Success Criteria

The MVP is successful when **all** of the following hold:

1. Two supported devices can discover and connect.
2. They can establish a Rust protocol session.
3. The session can authenticate peers.
4. A file can be transferred.
5. The file is streamed, never loaded entirely into memory.
6. Transfer progress is observable.
7. Integrity is verified.
8. Failed transfers are reported correctly.
9. Invalid protocol input cannot crash the application.
10. The system works with the Internet switched off.

Criteria 9 and 10 are as important as 1–8, and are the ones most likely to be
falsely claimed. Neither may be asserted without a test.

---

## 3. Phase Plan

| Phase | Name | Outcome |
| --- | --- | --- |
| **0** | Raw TCP baseline benchmark | Measured throughput ceiling |
| **1** | Foundation | Workspace + protocol + transport + CLI, over localhost |
| **2** | Real network | Same code, two machines on a Wi-Fi network |
| **3** | Wi-Fi Direct (Windows) | Adapter in place; protocol unchanged |
| **4** | Security | Identity, authentication, encryption, pairing |
| **5** | Reliable transfer | Chunking, integrity, resume, atomicity |
| **6** | Android | Android adapter + app |
| **7** | Linux | Linux adapter (experimental) |
| **8** | Desktop application | GUI, only after the engine is stable |

> **Note on numbering.** The source documents used "Phase 1" for two different
> things: the raw TCP benchmark (Doc 2) and the workspace foundation (Charter
> §75). Reconciled as **Phase 0** and **Phase 1** respectively — confirmed, see
> [`open-questions.md` Q-2](./open-questions.md).

---

## 4. Phase 0 — Raw TCP Baseline Benchmark

**Spec:** [`phase1-benchmark.md`](./phase1-benchmark.md)

**Purpose.** Prove the Tokio network engine is fast and memory-efficient before
investing in protocol or OS bindings. Establish the number every later phase is
measured against.

**Outcome.** Two binaries — `sender` and `receiver` — transferring a 10 GB file
over TCP with a hardcoded 8 MiB buffer, reporting MB/s and elapsed time.

**Exit criteria**

| # | Criterion | Status |
| --- | --- | --- |
| 1 | 10 GB transferred successfully over TCP | **Met** — 10 GiB at all three buffer sizes |
| 2 | Throughput and elapsed time reported by the receiver | **Met** — reported by both binaries |
| 3 | Peak RSS demonstrably flat w.r.t. file size | **Met** — sender flat at ~9 MiB from 10 MB to 10 GB |
| 4 | Results documented in [`performance.md`](./performance.md) | **Met** — §5.1-5.4, with caveats |
| 5 | 4 MiB and 16 MiB variants also measured | **Met** — 3-point curve recorded |

### Phase 0 findings

**Chunk size is not the throughput lever.** 4, 8, and 16 MiB all measured
76-79 MB/s over 10 GB — within noise of each other. The 8 MiB starting guess was
never shown to be better than 4 MiB. Larger buffers cost proportionally more
memory for nothing. Revisit in Phase 5 with a real link before fixing a default.

**Disk write throughput is the bottleneck on this configuration**, not the
network and not the protocol. The receiver's `sync_all()` is inside the timed
region, so these figures include a genuine flush to NVMe.

**Measured on loopback only.** This establishes a ceiling on one machine's
disk-to-disk path. It says nothing about Wi-Fi Direct or real network behaviour.
Phase 2 remains required before any network claim is permitted.

---

## 5. Phase 1 — Foundation

Build:

```text
Cargo workspace
Rust core
protocol crate
transport crate
CLI
Tokio runtime
TCP transport
logging
error system
```

**Explicitly excluded:** UI, Android, and any encryption complexity beyond the
minimum interfaces needed to keep the architecture honest.

**Goal:** localhost sender → localhost receiver.

```text
$ p2p listen

Listening...
```

```text
$ p2p connect <address>

Connected.
Protocol negotiated.
Session established.
```

**Exit criteria**

| # | Criterion | Status | Evidence |
| --- | --- | --- | --- |
| 1 | `protocol` encodes/decodes every Phase-1 message with round-trip tests | **Met** | 96 unit tests; property tests assert lossless round-trip, deterministic encoding, and byte-identical re-encoding |
| 2 | Invalid frames rejected without panic | **Met** | Property tests over arbitrary bytes; reserved flags, unknown message types, and bad magic all refused |
| 3 | `core` state machine rejects invalid transitions | **Met** | 55 unit tests; authentication cannot be skipped, terminal states cannot rejoin |
| 4 | Integration test drives both ends in process | **Met** | `tests/tests/handshake.rs`, 21 tests over real loopback sockets |
| 5 | Zero platform dependencies in any core crate | **Met** | Enforced by `scripts/check.ps1`, not by review |

**Phase 1 as built**

```text
crates/vacuumdrop-core        domain, errors, limits, state machine
crates/vacuumdrop-protocol    header, codec, messages, handshake, session
crates/vacuumdrop-transport   Transport trait + TCP
apps/cli                      the p2p binary
tests                         cross-crate integration + property tests
```

`p2p listen` and `p2p connect` complete a version-negotiated session over TCP.
Negotiation fails closed: no overlap is an error, never a silent downgrade.

**Not built, deliberately.** Transfer messages are absent rather than stubbed, so
nothing can be used before it works. `tracing` is deferred to Phase 5, where log
volume justifies it. Encryption and authentication are Phase 4 — the CLI states
the unencrypted status in `help` and again after every successful connect, because
a user who reads "connected" as "trusted" is the failure mode Charter 6 warns about.

---

## 6. Phase 2 — Real Network

```text
Device A
    ↓
Wi-Fi network
    ↓
Device B
```

Same binary, same protocol, different address. This isolates protocol bugs from
platform connectivity bugs **before** Wi-Fi Direct enters the picture.

**Test rig.** Windows Mobile Hotspot on Device A, Device B joins, connect to the
local gateway IP.

> **Caveat.** A mobile hotspot is infrastructure-mode AP plus DHCP. It validates
> *IP connectivity and throughput* — it does **not** exercise Wi-Fi Direct
> discovery, group formation, or GO negotiation. Treat hotspot results as a
> Phase-2 baseline only, never as Wi-Fi Direct evidence. See
> [`platform-windows.md`](./platform-windows.md).

---

## 7. Phase 3 — Wi-Fi Direct (Windows)

Implement the Windows Wi-Fi Direct adapter.

```text
Windows A
   ↕
Wi-Fi Direct
   ↕
Windows B
```

Then run the existing Rust protocol over the resulting connection.

**The protocol layer must require minimal or no changes.** If it does, the
transport abstraction leaked radio concepts — that is a defect in
[ADR-0004](./adr/0004-transport-abstraction.md), not a reason to patch the
protocol.

---

## 8. Phase 4 — Security

Implement:

- Device identity
- Key management
- Authentication
- Encrypted sessions
- Pairing
- Authorization
- Secure storage

Security is integrated **into** the protocol, not bolted onto file transfer after
the fact.

Design constraints:

- Do not invent cryptography. Use established libraries and protocols.
- A connection does **not** imply trust.
- Authentication ("who are you?") and authorization ("what may you do?") remain
  separate concerns.

See [`security.md`](./security.md). Library selection is an open decision
([`open-questions.md` Q-7](./open-questions.md)).

---

## 9. Phase 5 — Reliable Transfer

Implement:

- Chunking
- Checksums (per chunk + whole file)
- Resume
- Cancellation
- Temporary files
- Atomic finalization
- Failure recovery

The headline requirement: **a transfer interrupted at 8.7 GB of 10 GB resumes
from 8.7 GB, not from zero.**

```text
10 GB
|
|███████████████████░
|                   |
|                8.7 GB
|
Connection lost
|
Reconnect → Resume → Continue from missing chunks
```

See [`file-transfer.md`](./file-transfer.md).

---

## 10. Phase 6 — Android

```text
Android UI
     │
Android Wi-Fi Direct Adapter
     │
Rust Core
```

Android-specific logic stays out of the Rust domain layer. The Kotlin/Java layer
is a thin adapter over `WifiP2pManager`.

See [`platform-android.md`](./platform-android.md).

---

## 11. Phase 7 — Linux

Implement the Linux adapter; initially **experimental**.

Document supported distributions, drivers, limitations, and required system
components. Do not assume every distribution exposes identical Wi-Fi P2P
behaviour — detect capabilities, record findings.

See [`platform-linux.md`](./platform-linux.md).

---

## 12. Phase 8 — Desktop Application

Only after the engine is stable. Nearest-devices list and a live transfer view:

```text
Sending movie.mp4

██████████████████░░ 91%

4.82 GB / 5.20 GB

486 MB/s

32 seconds remaining
```

---

## 13. First Task

**Build the smallest possible Rust networking prototype.**

```text
Machine A
    │
    │ TCP
    │
Machine B
```

Implement: Rust sender, Rust receiver, protocol framing, message validation,
basic logging, clean errors, integration tests.

**Use localhost first. Then two machines on the same network. Then Wi-Fi
Direct.**

---

## 14. First Milestone

```text
$ p2p listen

Listening...
```

```text
$ p2p connect <address>

Connected.
Protocol negotiated.
Session established.
```

```text
$ p2p send test.bin

Sending...

100%

Transfer complete.

Integrity verified.
```

Only after this works reliably does Wi-Fi Direct integration begin.

---

## 15. What NOT to Build Yet

These are distractions from the core engineering problem:

mesh networking · blockchain · custom encryption · custom wireless protocol ·
custom Wi-Fi driver · custom congestion control · massive GUI · cloud backend ·
accounts · social features · payments · analytics · AI features · distributed
database · custom filesystem

---

## 16. Deliberate Ordering Constraints

These are hard sequencing rules, not preferences.

1. **Point-to-point → reliable sessions → discovery → security → transfer →
   multi-peer → routing → mesh.** Mesh networking is last, not first.
2. **TCP before QUIC.** Any move to QUIC must be justified by measured
   requirements, not by reputation.
3. **Single stream before parallel streams.** Do not open many simultaneous TCP
   streams until measurements show the transport would benefit.
4. **One platform at a time.** Build for multiple; implement one.
5. **Security before "reliable transfer complete".** Retrofitting encryption onto
   a finished transfer path produces a worse protocol than designing it in.

---

## 17. Learning Objectives

The project exists partly to build depth in:

| Domain | Areas |
| --- | --- |
| Rust | ownership, borrowing, lifetimes, traits, enums, async, concurrency, error handling, FFI, performance |
| Networking | TCP, UDP, sockets, connection lifecycle, framing, protocols, backpressure, congestion, reliability, discovery, routing |
| Security | public-key crypto, authentication, authorization, key management, threat modelling, secure protocols, input validation |
| Systems | OS APIs, platform abstraction, capability detection, resource management, persistence, fault tolerance |
| Distributed systems | peer discovery, replication, routing, synchronization, consistency, multi-hop, distributed identity |

---

## 18. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §13, §14 | Doc 2 §1; Charter §100, §101 |
| §2 | Charter §73, §74 |
| §3–§12 | Charter §75–§82 |
| §15, §16, §17 | Charter §96, §14, §28, §72, §97 |