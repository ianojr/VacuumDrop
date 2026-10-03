# Architecture

Status: **Implemented for Phase 1.** Layers below are normative; the engine API
in section 8 remains an intent, not a contract.

Last updated: Phase 1 complete.

This document is the structural contract for VacuumDrop. It describes how the
system is decomposed, what each layer is allowed to know about, and which
invariants must hold for the engine to remain portable.

---

## 1. What VacuumDrop Is

VacuumDrop is a **cross-platform, offline-first, peer-to-peer communication
engine**. It is not a file-sharing application.

> File transfer is the *first* application built on top of the engine.

The engine must therefore never expose file-transfer concepts in its core
vocabulary. Concepts such as "file", "chunk size", or "directory" belong to the
transfer subsystem and, ultimately, to the application - never to the identity,
session, or transport layers.

### 1.1 Explicit non-dependencies

The engine must function with **all** of the following absent:

| Absent | Consequence |
| --- | --- |
| Internet access | No DNS, no remote endpoints |
| Cloud infrastructure | No accounts, no relay, no remote config |
| A central server | No coordination service |
| A conventional Wi-Fi router | Peer becomes the network |
| Mobile data | No cellular fallback |
| Bluetooth | Not a transport in any phase |

The first transport is **Wi-Fi Direct / Wi-Fi P2P**. Additional transports
(Ethernet, USB, Wi-Fi Aware, optical) must be addable **without modifying the
core protocol**.

---

## 2. Layer Model

```text
+---------------------------------------------------------+
|                    APPLICATION LAYER                    |
|                                                         |
| File Sharing | Messaging | Sync | Future Applications |
+-----------------------------+---------------------------+
                              |
                              v
+---------------------------------------------------------+
|                     RUST CORE                           |
|                                                         |
| Session Manager                                          |
| Peer Manager                                             |
| Protocol Engine                                          |
| Transfer Engine                                          |
| Security Manager                                         |
| Integrity Manager                                        |
| State Machine                                            |
| Persistence                                              |
| Telemetry                                                |
+-----------------------------+---------------------------+
                              |
                              v
+---------------------------------------------------------+
|                 TRANSPORT ABSTRACTION                   |
|                                                         |
| connect() | listen() | send() | receive() | shutdown() |
+-----------------------------+---------------------------+
                              |
              +---------------+---------------+
              |                               |
              v                               v
+------------------------+        +------------------------+
| Wi-Fi Direct Adapter   |        | Future Transport       |
|                        |        |                        |
| Android                |        | Wi-Fi Aware            |
| Windows                |        | Ethernet               |
| Linux                  |        | USB                    |
|                        |        | Other                  |
+------------------------+        +------------------------+
```

### 2.1 Layer rules (normative)

1. Dependencies point **downward only**.
2. The core **must not** reference Android, Windows, or Linux APIs. This is
   enforced structurally: core crates cannot depend on platform crates.
3. The transport abstraction **must not** contain Wi-Fi Direct concepts.
4. Platform-specific code stays as thin as possible; business logic is never
   duplicated across platforms.
5. A new application must be buildable by consuming the core API only - without
   touching transport or platform layers.

---

## 3. OS / Adapter Boundary Model

```text
                 +-----------------------+
                 |    UI / Application   |
                 +-----------+-----------+
                             |
                 +-----------+-----------+
                 |    Rust Core (Tokio)   |
                 | - VacuumDrop Protocol  |
                 | - Security/Encryption  |
                 | - Chunking & Hashing   |
                 +-----------+-----------+
                             |
          +------------------+------------------+
          v                  v                  v
      Android            Windows             Linux
      Adapter             Adapter             Adapter
 (WifiP2pManager)     (Wi-Fi Direct)      (Wi-Fi P2P)
```

**Division of responsibility**

1. The OS adapter performs peer discovery and establishes the Wi-Fi Direct P2P
   connection.
2. Once connected, the adapter resolves the resulting **IP address and port**
   and hands them to the Rust core.
3. The Rust core takes over completely: it opens a TCP connection and runs the
   VacuumDrop protocol over it.

The adapter is **not** a transport. It is a *connection facilitator*. Once the
core holds a socket, the Wi-Fi Direct radio is irrelevant to the protocol.

---

## 4. Cargo Workspace Layout

Prefer a Cargo workspace. **Do not create all modules immediately** - introduce
them as the project evolves. The table marks when each crate is introduced.

```text
vacuumdrop/
|
+-- Cargo.toml                    # workspace root
|
+-- crates/
|   +-- vacuumdrop-bench/           # Phase 0   sender / receiver
|   +-- vacuumdrop-core/            # Phase 1
|   +-- vacuumdrop-protocol/        # Phase 1
|   +-- vacuumdrop-transport/       # Phase 1
|   +-- vacuumdrop-security/        # Phase 4
|   +-- vacuumdrop-transfer/        # Phase 5
|   +-- vacuumdrop-discovery/       # Phase 5
|   +-- vacuumdrop-persistence/     # Phase 5
|   +-- vacuumdrop-platform/
|       +-- windows/                # Phase 3
|       +-- android/                # Phase 6
|       +-- linux/                  # Phase 7
|
+-- apps/
|   +-- cli/                        # Phase 1   binary name: p2p
|   +-- desktop/                    # Phase 8
|   +-- android/                    # Phase 6
|
+-- tests/                          # Phase 1
+-- benches/                        # Phase 1
+-- tests/                          # Phase 1   cross-crate integration + property
+-- fuzz/                           # Phase 1   cargo-fuzz targets
+-- docs/
+-- scripts/
```

> Crates carry the `vacuumdrop-` prefix. Confirmed in
> [`open-questions.md` Q-13](./open-questions.md).

### 4.1 Crate introduction schedule

Status is the state as of Phase 1.

| Crate | Phase | Status | Responsibility |
| --- | --- | --- | --- |
| `vacuumdrop-bench` | 0 | **Built** | Raw TCP throughput benchmark (`sender`, `receiver`) |
| `vacuumdrop-core` | 1 | **Built** | Domain types, peer identity, sessions, state machines, lifecycle, errors, events |
| `vacuumdrop-protocol` | 1 | **Built** | Message definitions, encoding, decoding, framing, version negotiation |
| `vacuumdrop-transport` | 1 | **Built** | Transport abstraction + TCP implementation |
| `apps/cli` (`p2p`) | 1 | **Built** | `p2p` command-line interface |
| `vacuumdrop-platform-windows` | 3 | Not started | Windows Wi-Fi Direct adapter |
| `vacuumdrop-security` | 4 | Not started | Device identity, key management, authentication, encryption, authorization |
| `vacuumdrop-transfer` | 5 | Not started | Chunking, integrity, backpressure, resume, atomic finalization |
| `vacuumdrop-discovery` | 5 | Not started | Application-layer discovery, capability negotiation |
| `vacuumdrop-persistence` | 5 | Not started | Durable transfer state, identity storage |
| `vacuumdrop-platform-android` | 6 | Not started | Android `WifiP2pManager` adapter |
| `vacuumdrop-platform-linux` | 7 | Not started | Linux Wi-Fi P2P adapter (experimental) |
| `apps/desktop` | 8 | Not started | GUI |

`vacuumdrop-security` is deliberately not started: Phase 4 is blocked on
Q-7 ([`open-questions.md`](./open-questions.md)), and choosing a handshake
protocol without that decision would be expensive to undo.

### 4.2 `vacuumdrop-core` must never know about

Android, Windows, or Linux APIs - and, per section 1, also not about files.
`vacuumdrop-core` owns *sessions and peers*, not *transfers of files*.

---

## 5. Module Responsibilities

| Module | Owns | Must not own |
| --- | --- | --- |
| `vacuumdrop-core` | Domain types, peer identity, sessions, state machines, lifecycle, errors, events | Platform APIs, transport specifics |
| `vacuumdrop-protocol` | Messages, versioning, encoding, framing | Sockets, files, crypto policy |
| `vacuumdrop-transport` | `connect` / `accept` / `send` / `receive` / `shutdown` | Any Wi-Fi Direct concept |
| `vacuumdrop-security` | Identity, authentication, encryption, authorization, key storage | Platform keychain APIs (behind a trait) |
| `vacuumdrop-transfer` | Chunking, integrity, resume, backpressure, atomicity | Sockets, protocol encoding |
| `vacuumdrop-discovery` | App-layer peer identity + capability exchange | Radio-level peer discovery |
| `vacuumdrop-persistence` | Durable state, atomic commits | Business rules |
| `vacuumdrop-platform::*` | Radio/connectivity, capability detection | Protocol, business logic |

---

## 6. Transport Abstraction Boundary

```text
Bad:   connect_wifi_direct_group_owner(peer)
Good:  connect(peer)
```

The Wi-Fi Direct adapter handles all platform-specific detail. The core calls
`connect(peer)` and receives a transport handle.

See [`transport.md`](./transport.md) for the trait shape and
[ADR-0004](./adr/0004-transport-abstraction.md) for the rationale.

---

## 7. Rust <-> Platform FFI Boundary

Use a **narrow** FFI boundary.

```text
Bad:   Android API --> hundreds of platform-specific objects --> Rust logic
Good:  Android API --> Thin Adapter --> simple Rust values --> Rust Core
```

The boundary exchanges only simple, normalized concepts:

```text
Peer . ConnectionInfo . TransportHandle . Capability . Error . Event
```

Platform objects must never leak into the core. This is the single most
important portability invariant in the project.

### 7.1 Adapter capability surface

Every platform adapter must expose, conceptually:

```text
is_supported()
is_enabled()
discover_peers()
connect(peer)
disconnect(peer)
get_connection_info()
```

The core receives normalized information, never platform objects.

---

## 8. Engine Public API (Conceptual)

```text
Engine
 +-- discover()
 +-- connect(peer)
 +-- authenticate(peer)
 +-- send(file)
 +-- receive()
 +-- pause(transfer)
 +-- resume(transfer)
 +-- cancel(transfer)
 +-- disconnect(peer)
```

The exact API is expected to evolve through implementation. It is recorded here
as an intent, not a contract.

### 8.1 Event Catalogue

Applications receive events rather than polling:

```text
PeerDiscovered          PeerLost
ConnectionStarted       ConnectionEstablished
AuthenticationRequired  AuthenticationSucceeded   AuthenticationFailed
TransferStarted         TransferProgress
TransferPaused          TransferResumed
TransferCompleted       TransferFailed
PeerDisconnected
```

Every event carries correlation identifiers (`session_id`, `transfer_id`,
`request_id`) where meaningful.

---

## 9. Dependency Direction - Structural Enforcement

The layering in section 2 is only real if it is mechanically enforced. Minimum
bar:

- Core crates (`vacuumdrop-core`, `-protocol`, `-transport`, `-security`,
  `-transfer`) declare **no** platform dependencies.
- `vacuumdrop-platform::*` crates depend on core crates; core crates never
  depend on them.
- Any `unsafe` or `extern "C"` surface lives exclusively in
  `vacuumdrop-platform::*` and the FFI shims.
- CI must fail if a platform crate appears in a core crate's dependency tree.

### 9.1 Enforcement as implemented

Three of these four are enforced today by [`scripts/check.ps1`](../scripts/check.ps1),
run as the project gate:

| Rule | Enforced by | Automated? |
| --- | --- | --- |
| No core crate depends on a platform crate | Dependency scan of every `crates/*/Cargo.toml` | **Yes** |
| `unsafe` requires justification | `unsafe_code = "forbid"` workspace-wide | **Yes** |
| Transport abstraction carries no radio concepts | Trait-signature scan for `group_owner`, `go_intent`, `wifi_direct`, `advertisement` | **Yes** |
| Platform surface confined to `vacuumdrop-platform::*` | Not yet | **No** — no platform crate exists |

The fourth has nothing to enforce until Phase 3 creates
`vacuumdrop-platform-windows`. **Add the check in the same change that adds the
crate**, or the rule will be honoured by discipline until it is forgotten.

---

## 10. Long-Term Architectural Goal

```text
                         APPLICATIONS
                              |
          +-------------------+-------------------+
          |                   |                   |
      File Share          Messaging           Sync
          |                   |                   |
          +-------------------+-------------------+
                              |
                              v
                    +------------------+
                    |    RUST CORE     |
                    |                  |
                    | Identity         |
                    | Security         |
                    | Sessions         |
                    | Protocol         |
                    | Transfers        |
                    | Discovery        |
                    | Persistence      |
                    | Telemetry        |
                    +--------+---------+
                             |
                             v
                    +------------------+
                    | TRANSPORT API    |
                    +--------+---------+
                             |
            +----------------+----------------+
            |                |                |
            v                v                v
       Wi-Fi Direct      Wi-Fi Aware        Future
            |                                 |
     +------+------+                    +-----+-----+
     |      |      |                    |           |
 Android Windows Linux                USB       Optical
```

The long-term vision is not "a faster file sharing app". It is:

> **A general-purpose, secure, infrastructure-independent communication layer for
> nearby devices.**

---

## 11. Source Traceability

| Section | Origin |
| --- | --- |
| 1, 2, 3, 5 | Core Architecture & Context (Doc 1) |
| 1, 2.1, 3, 4, 7, 8, 10 | Offline P2P Engine Charter 1-8, 46-49, 71, 102 |
| 9 | Derived: enforcement of 33, 47, 83 Rule 5 |