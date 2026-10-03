# Phase 0 - Raw Tokio TCP Benchmark

> **Also called "Phase 1" in Doc 2.** Renamed here to Phase 0 to resolve a
> numbering collision with Charter section 75. See
> [`roadmap.md` section 3](./roadmap.md) and
> [`open-questions.md` Q-2](./open-questions.md).

Status: **Ready to implement.** This is the first code that will be written.

---

## 1. Goal

Before building the custom protocol or OS-level Wi-Fi Direct bindings, prove
that the Rust asynchronous network engine is **fast and memory-efficient**.

This phase produces exactly one artifact of lasting value: a **measured
baseline** that every later performance claim is compared against. Everything
else here is scaffolding.

**Task.** Build two standalone Rust binaries (`sender` and `receiver`) that
transfer a 10 GB file over a local network using Tokio TCP streams.

---

## 2. Technical Requirements

| Requirement | Value |
| --- | --- |
| Framework | `tokio` 1.x, `full` feature |
| Disk I/O | `tokio::fs::File` |
| Network I/O | `tokio::net::TcpStream` / `TcpListener` |
| Buffer | Hardcoded `vec![0; 8 * 1024 * 1024]` (8 MiB) |
| Metrics | Receiver prints transfer speed (MB/s) and total elapsed time on completion |

### 2.1 Buffer sizing

8 MiB is a **starting experiment, not a permanent requirement**. Charter 22
requires benchmarking across:

```text
256 KiB . 1 MiB . 4 MiB . 8 MiB . 16 MiB . 32 MiB
```

and choosing based on measurement. Phase 0 hardcodes 8 MiB to keep the code
minimal; chunk size becomes configurable in
[ADR-0006](./adr/0006-chunked-transfer.md).

---

## 3. Implementation Rules

### 3.1 No framing yet

Do **not** implement length-delimited framing, `serde`, or custom headers in
Phase 0. Just pipe raw bytes from disk to socket, and socket to disk.

Rationale: framing adds allocation and bounds-checking to the hot loop. Measuring
the un-framed ceiling first tells you how much headroom the protocol costs.

### 3.2 Low-copy

Minimise allocations inside the `loop`. Reuse the same 8 MiB buffer for every
read/write cycle. A fresh allocation per iteration would measure the allocator,
not the network.

### 3.3 Error handling

Use `Result<(), Box<dyn std::error::Error>>` throughout, to keep the code clean
and focused on throughput.

> **This is explicitly a Phase-0 exception.** Charter 39 forbids this signature
> in production paths and requires the structured `TransportError` /
> `ProtocolError` / ... taxonomy. The structured error type arrives in Phase 1
> ([`engineering-standards.md`](./engineering-standards.md)). Do not let this
> pattern escape the benchmark crate.

### 3.4 Feature-flag scope

Doc 2 specifies `tokio` with `full`. That is acceptable for a throwaway
benchmark crate whose only job is to measure a ceiling.

Charter 13 requires deliberate runtime usage in the real engine ("do not
introduce Tokio everywhere without architectural reason"). The Phase-1 workspace
**MUST** select Tokio features per crate rather than defaulting to `full`
everywhere.

---

## 4. Where the Binaries Live

Doc 2 asks for two standalone binaries named `sender` and `receiver`. The
Charter (50, 101) specifies a single CLI, `p2p`, with subcommands.

**Both are honoured, without compromise:**

| | Location | Rationale |
| --- | --- | --- |
| `sender`, `receiver` | `crates/vacuumdrop-bench` (workspace member, two `[[bin]]` targets) | Benchmarks must stay dependency-minimal. Keeping them out of the CLI prevents benchmark shortcuts from leaking into shipping code, and keeps the measured path clean. |
| `p2p` | `apps/cli` | The real, debuggable interface |

The bench crate depends on `tokio` and nothing else from the workspace. It does
**not** share code with `transport` - a shared helper would contaminate the
measurement.

---

## 5. Memory Expectation

Charter 61: a 10 GB file must transfer **without requiring 10 GB of RAM**.

For Phase 0 the expected working set is approximately:

```text
8 MiB   read buffer
+ 8 MiB  write buffer (if not reused)
+ socket buffers
+ runtime overhead
= 16-24 MiB, flat
```

**The number to watch: RSS must not grow with file size.** A flat RSS across 10 MB
and 10 GB transfers is the actual proof of the streaming property. Record it.

---

## 6. Testing Environment

Windows, two devices:

1. Device A: enable **Mobile Hotspot**.
2. Device B: join the hotspot.
3. Establish the socket using the local gateway IP.

> A mobile hotspot is infrastructure-mode AP + DHCP. It exercises IP connectivity
> and throughput only. It does **not** exercise Wi-Fi Direct discovery, group
> formation, or group-owner negotiation. Hotspot numbers are a Phase-2 baseline,
> never Wi-Fi Direct evidence. See
> [`platform-windows.md`](./platform-windows.md).

---

## 7. Metrics to Record

Charter 29 requires more than throughput. Record at minimum:

| Metric | How |
| --- | --- |
| Throughput | Bytes / elapsed, reported by the receiver |
| Elapsed time | Monotonic clock, receiver-side |
| Peak RSS | Task Manager / `Get-Process` peak working set |
| CPU utilisation | `% Processor Time` for both processes |
| Disk throughput | Bytes/sec for the source and destination volumes |

Record results in [`performance.md`](./performance.md) under the Phase 0
baseline table. This is the reference for ADR-0003 (TCP-first) and for every
later "this is faster than X" claim.

---

## 8. Exit Criteria

Phase 0 is complete when:

1. 10 GB transferred successfully over TCP.
2. Throughput and elapsed time reported by the receiver.
3. Peak RSS recorded and demonstrably flat with respect to file size.
4. Results documented in [`performance.md`](./performance.md).
5. A **4 MiB and 16 MiB** variant also measured, so the chunk-size curve has at
   least three points before Phase 5 picks a default.

---

## 9. Explicitly Out of Scope

- Any VacuumDrop protocol message.
- Any framing, serialization, or handshake.
- Chunk hashing or integrity verification.
- Encryption or authentication.
- Discovery of any kind.
- Platform / Wi-Fi Direct code.

---

## 10. Source Traceability

| Section | Origin |
| --- | --- |
| 1, 2, 3, 6 | Doc 2 - Phase 1 Implementation Guide |
| 2.1, 5, 7 | Charter 22, 29, 61 |
| 3.3 | Charter 39 |
| 3.4 | Charter 13 |
| 4 | Charter 50, 101 |
| 8 | Charter 98 |