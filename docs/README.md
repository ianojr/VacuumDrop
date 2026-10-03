# VacuumDrop — Documentation

**VacuumDrop** is a cross-platform, offline-first, peer-to-peer communication
engine. It lets nearby devices exchange large files and data streams directly —
**no internet, no router, no cloud, no mobile network**.

This directory is the project's design record. Read it before writing code.

---

## 1. Start Here

| If you want to… | Read |
| --- | --- |
| Understand what this is and why | [`architecture.md`](./architecture.md) §1–§2 |
| Know what to build first | [`roadmap.md`](./roadmap.md), [`phase1-benchmark.md`](./phase1-benchmark.md) |
| Understand the wire protocol | [`protocol.md`](./protocol.md) |
| Understand the rules you must follow | [`engineering-standards.md`](./engineering-standards.md) |
| See what is unresolved | [`open-questions.md`](./open-questions.md) |

---

## 2. Non-Negotiable Principles

These override convenience, schedule pressure, and code brevity — everywhere, in
every document.

1. **Offline first.** No internet, cloud, DNS, or central server in the core path.
2. **Never read a whole file into memory.** Chunked streaming, always.
3. **Treat all remote input as hostile.** Validate lengths before allocating.
4. **Do not invent cryptography.** Established libraries only.
5. **Rust core, thin platform adapters.** No business logic outside Rust.
6. **Measure before claiming.** No performance or compatibility claims without
   recorded evidence.
7. **Bounded resources.** No unbounded queues, buffers, or allocations.
8. **Tests are not negotiable.** Never delete a test or disable a security check
   to make something work.

---

## 3. Document Index

### Design

| Doc | Contents | Status |
| --- | --- | --- |
| [`architecture.md`](./architecture.md) | Layers, workspace layout, module boundaries, FFI boundary, engine API | **Implemented** — §9.1 records what is automated vs. not |
| [`protocol.md`](./protocol.md) | Message catalogue, wire format, framing, state machines, versioning, limits | **Phase 1 subset implemented** — transfer messages intentionally absent |
| [`dep-justification.md`](./dep-justification.md) | Every dependency with Charter 54's questions answered, plus what was deliberately left out | **Normative** |
| [`transport.md`](./transport.md) | Transport abstraction, Tokio usage, TCP-first, timeouts | **Implemented** |
| [`discovery.md`](./discovery.md) | Platform vs application discovery, adapter surface, capability handling | Design intent — Phase 5 |
| [`file-transfer.md`](./file-transfer.md) | Pipeline, chunking, resume, integrity, backpressure, atomicity | Design intent — Phase 5 |
| [`security.md`](./security.md) | Identity, authentication, encryption, authorization, pairing, secrets | Design intent — blocked on Q-7, Q-10 |
| [`threat-model.md`](./threat-model.md) | Adversary model, trust boundaries, 24-item threat register, validation matrix | **Substantive** — T-01, T-05 closed by Phase 1 |

### Process

| Doc | Contents | Status |
| --- | --- | --- |
| [`roadmap.md`](./roadmap.md) | Phases 0–8, MVP definition and success criteria, ordering constraints | **Phases 0–1 complete** |
| [`phase2-test.md`](./phase2-test.md) | Step-by-step two-machine test: prerequisites, commands, troubleshooting, recording template | **Ready to run** — needs two devices |
| [`phase1-benchmark.md`](./phase1-benchmark.md) | Phase 0 spec: raw Tokio TCP baseline | **Complete** — measured, see [`performance.md` §5](./performance.md) |
| [`engineering-standards.md`](./engineering-standards.md) | Agent rules, workflow, style, error handling, unsafe policy, dependencies, DoD | **Normative** |
| [`testing.md`](./testing.md) | Test layers, property tests, fuzz targets, security tests, CI gates | **Normative** |
| [`performance.md`](./performance.md) | Metrics, benchmark scenarios, **empty Phase-0 baseline**, checklist, compatibility matrix | **Substantive** — baseline not yet measured |
| [`troubleshooting.md`](./troubleshooting.md) | Failure philosophy, failure modes, symptom index | **Skeleton** — grows per phase |
| [`open-questions.md`](./open-questions.md) | 13 unresolved items + recorded resolutions | **Needs your input** |

### Platforms

| Doc | Contents | Status |
| --- | --- | --- |
| [`platform-windows.md`](./platform-windows.md) | WinRT Wi-Fi Direct, binding strategy, test procedure | **Design intent** — Phase 3 |
| [`platform-android.md`](./platform-android.md) | `WifiP2pManager`, permissions, JNI boundary | **Design intent** — Phase 6 |
| [`platform-linux.md`](./platform-linux.md) | wpa_supplicant P2P, driver variance | **Design intent** — Phase 7, experimental |

### Architecture Decision Records

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](./adr/0001-rust-core.md) | Rust as the portable core | Accepted |
| [0002](./adr/0002-tokio.md) | Tokio as the async runtime | Accepted |
| [0003](./adr/0003-tcp-first.md) | TCP before QUIC | Accepted |
| [0004](./adr/0004-transport-abstraction.md) | Pluggable transport abstraction | Accepted |
| [0005](./adr/0005-cryptographic-identity.md) | Cryptographic device identity | Accepted (Phase 4) |
| [0006](./adr/0006-chunked-transfer.md) | Chunked streaming transfer | Accepted (Phase 5) |
| [0007](./adr/0007-serialization-and-framing.md) | Serialization + framing — **`serde` + `postcard`** | **Accepted** |
| [0008](./adr/0008-cli-first.md) | CLI first | Accepted |
| [0009](./adr/0009-windows-wifi-direct-binding.md) | Windows binding strategy | **Proposed — needs Phase-3 spike** |
| [0010](./adr/0010-handshake-protocol.md) | Handshake / record protocol | **Deferred — blocks Phase 4** |

Each ADR contains **Context, Decision, Alternatives, Consequences, Status**.

---

## 4. Current State

```text
Design          ██████████  complete
Phase 0         ██████████  DONE - 10 GiB, RSS flat at ~9 MiB
Phase 1         ██████████  DONE - 5/5 exit criteria, 262 tests
Phase 2         ██████████  DONE - two machines, session negotiated at v1
Phase 3         ░░░░░░░░░░  NEXT - Windows Wi-Fi Direct adapter
Phase 4-8       ░░░░░░░░░░  security, transfer, android, linux, GUI
Decisions       ████████░░  9 resolved, 6 open
```

**Settled:** project name **VacuumDrop** · crates `vacuumdrop-*` · CLI `p2p` ·
serialization **`serde` + `postcard`** · benchmark is **Phase 0**.

**Phase 0 result:** 10 GiB transferred at 76-79 MB/s. Sender peak RSS flat at
~9 MiB from 10 MB to 10 GB — the streaming property is proven. Chunk size (4/8/16
MiB) made no throughput difference. Disk write throughput is the bottleneck, not
the network. Full numbers and caveats in
[`performance.md` §5](./performance.md).

**Phase 1 result:** workspace of five crates, `p2p listen|connect|ping`, version
negotiation that fails closed, framed codec with pre-allocation length bounds,
structured error taxonomy, and the full connection state machine. `scripts/check.ps1`
gates format, clippy, build, tests, dependency direction, the
`unsafe_code = "forbid"` policy, transport-abstraction purity, and fuzzing.

**Phase 2 result:** two physical machines, identical binary, session negotiated at
`v1` over a 229 Mbps hotspot link. **This is not a Wi-Fi Direct result** — a
hotspot is infrastructure-mode AP plus DHCP, so discovery, group formation, and
group-owner negotiation remain untested.

```text
$ p2p connect 192.168.137.1:9000
connected: tcp://192.168.137.1:9000
session established
  version     : v1
  peer        : Device-A
```

**Next:** Phase 3 — the Windows Wi-Fi Direct adapter, after the ADR-0009 spike.
**Disable any VPN on both devices first**: a Proton VPN callout driver was
observed returning `10013` on LAN TCP.

---

## 5. Source Documents

This directory was assembled from four inputs. Where they disagreed, the
disagreement is recorded rather than silently resolved.

| Source | Contents |
| --- | --- |
| **Doc 1** — Core Architecture & Context | Project overview, strict AI constraints, adapter abstraction diagram, workflow |
| **Doc 2** — Phase 1 Implementation Guide | Raw Tokio TCP benchmark: `sender`/`receiver`, 8 MiB buffer, no framing, metrics |
| **Doc 3** — Protocol Design Spec | `VacuumDropMessage` enum, `bincode` + `LengthDelimitedCodec`, state machine, sliding-window guideline |
| **Charter** — Offline P2P Communication Engine | 102 numbered sections: philosophy, architecture, security, transfer, platforms, testing, phases, agent rules |

### Precedence

1. **Charter** wins on security, resource limits, and process rules — it is the
   most specific and most security-conscious source.
2. **Doc 3** wins on nothing by default; it is the earliest draft and conflicts
   with the Charter in five places, all recorded in
   [`protocol.md` §9](./protocol.md).
3. **Doc 2** is authoritative for Phase 0 only, except where it conflicts with
   the Charter — in which case the Charter wins and the exception is documented
   in [`phase1-benchmark.md` §3](./phase1-benchmark.md).

---

## 6. Maintenance

- Every major subsystem requires documentation. If a subsystem is undocumented,
  it is not finished — see the Definition of Done in
  [`engineering-standards.md` §11](./engineering-standards.md).
- Update [`open-questions.md`](./open-questions.md) as items resolve; move the
  decision into the relevant ADR.
- Update the Phase-0 baseline in [`performance.md`](./performance.md) as soon as
  it is measured. Until then, **no performance claim is permitted**.
- Update the compatibility matrix per platform as each is *tested* — never
  merely as it compiles.