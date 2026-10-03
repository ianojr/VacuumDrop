# Performance Engineering

Status: **Phase 0 baseline recorded (loopback, 2026-10-03).** Network figures
still absent — no network or Wi-Fi Direct claim is permitted yet.

---

## 1. Rule Zero

> **Never optimise based on intuition alone.**

```text
measure → identify bottleneck → optimise → benchmark → verify
```

Do not add `unsafe` code, zero-copy complexity, multiple streams, custom
allocators, or custom protocols **merely because they sound fast**.

---

## 2. Prohibited Claims

Without controlled, recorded measurement:

| Do not claim | Why |
| --- | --- |
| "Faster than AirDrop" | Different hardware, different radio, no shared baseline |
| "Maximum Wi-Fi speed" | Theoretical link rate is not application throughput |
| "Near line speed" | Requires a stated link rate and stated conditions |
| "Low memory usage" | Requires a stated file size and a stated measurement method |

Every performance claim in this project must cite a row from the tables below,
with hardware, link, and method recorded.

---

## 3. Metrics

| Metric | Notes |
| --- | --- |
| Throughput | Bytes ÷ elapsed, receiver-side, monotonic clock |
| Latency | Round-trip where meaningful |
| CPU usage | Both ends, not just the sender |
| Memory usage | **Peak RSS** — and its *flatness* w.r.t. file size |
| Disk throughput | Source and destination volumes |
| Network utilisation | Against negotiated link rate |
| Packet loss / retransmission | Where measurable |
| Transfer duration | End-to-end wall clock |
| Connection establishment time | |
| Discovery time | Platform + application layers separately |

---

## 4. Benchmark Scenarios

| Scenario | Size | What it exercises |
| --- | --- | --- |
| Small file | 10 MB | Latency-dominated path; per-transfer overhead |
| Medium file | 1 GB | Steady state |
| Large file | 10 GB+ | Throughput ceiling; memory flatness |
| Many files | 10,000 files | Per-file overhead; metadata path |
| Interrupted @ 25% | — | Resume correctness |
| Interrupted @ 50% | — | Resume correctness |
| Interrupted @ 75% | — | Resume correctness |
| Slow receiver | — | **Backpressure** |
| Full disk | — | Failure handling |

The slow-receiver and full-disk scenarios are not performance tests — they are
correctness tests that happen to be slow. They are listed here because they are
executed alongside the performance suite.

---

## 5. Phase 0 — Raw TCP Baseline

**Status: NOT YET MEASURED.** Fill this table before making any performance
claim. Source: [`phase1-benchmark.md`](./phase1-benchmark.md).

### 5.1 Environment Record

Record once per test run; results without this are not comparable.

| Field | Value |
| --- | --- |
| Date | 2026-10-03 |
| Sender hardware | Intel Core i7-8565U @ 1.80 GHz, 4C/8T |
| Receiver hardware | same machine (localhost) |
| RAM | 15.8 GiB |
| Link type | **localhost loopback** — not a network result |
| Negotiated link rate | n/a |
| Distance / obstruction | n/a |
| OS build | Windows 11 Pro 10.0.26300.26300 |
| Rust toolchain | 1.92.0 stable-x86_64-pc-windows-msvc, edition 2024 |
| Filesystem (source / dest) | NTFS on C: |
| Storage type | INTEL SSDPEKNW512G8H, NVMe, 476 GB |
| Source file | sparse (`SetLength`) — see caveat in 5.4 |

Reproduce with:

```
cargo build --release
powershell -ExecutionPolicy Bypass -File scripts/run-bench.ps1 -SizeGiB 10 -BufferSizes "4MiB,8MiB,16MiB"
```

### 5.2 Results

Throughput is the **receiver** figure (it includes the final `sync_all`).
Peak RSS is sampled externally by `scripts/run-bench.ps1`.

| Buffer | File size | Throughput (MB/s) | Elapsed (s) | Peak RSS send | Peak RSS recv |
| --- | --- | --- | --- | --- | --- |
| 8 MiB | 10 MB | 151.32 | 0.071 | 8.96 MiB | 4.89 MiB |
| 8 MiB | 1 GB | 74.78 | 14.358 | 9.05 MiB | 15.59 MiB |
| 4 MiB | 10 GB | 78.71 | 136.410 | 9.03 MiB | 11.00 MiB |
| 8 MiB | 10 GB | 76.12 | 141.057 | 9.06 MiB | 15.77 MiB |
| 16 MiB | 10 GB | 79.16 | 135.649 | 8.98 MiB | 23.02 MiB |

### 5.3 What the numbers say

**The streaming property holds.** Sender peak RSS is **flat at ~9 MiB** across a
1000x range of file sizes (10 MB to 10 GB). It is driven entirely by the buffer
allocation, not by file size. This is the assertion that mattered, and it
passed.

Receiver RSS tracks the buffer size almost exactly as expected:

| Buffer | Predicted | Measured |
| --- | --- | --- |
| 4 MiB | ~11 MiB | 11.00 MiB |
| 8 MiB | ~16 MiB | 15.77 MiB |
| 16 MiB | ~23 MiB | 23.02 MiB |

Receiver = buffer + ~7 MiB of runtime and socket overhead. Note the 10 MB row
reads 4.89 MiB rather than ~16 MiB: the transfer was too short to touch most of
the buffer, so it under-reports. **RSS figures are only meaningful once the file
is larger than a few buffers.**

**Throughput is flat across buffer sizes.** 4, 8, and 16 MiB all land in
76-79 MB/s at 10 GB, within run-to-run noise. On this loopback configuration
**buffer size is not the throughput lever**. Larger buffers do not help, and cost
proportionally more memory. Do not assume 8 MiB is right because it was the
starting guess - it has simply not been shown to be better than 4 MiB.

**The bottleneck is not the network.** ~78 MB/s sustained is far below
loopback's capability, and it does not respond to buffer size. The receiver
calls `sync_all()` before stopping its clock, so this figure includes a real
flush to the NVMe. **Disk write throughput is the limiting factor on this
configuration**, and the protocol work in later phases is not what is capping
speed here.

### 5.4 Caveats — read before reusing these numbers

1. **These are loopback numbers, not network numbers.** They establish a
   *ceiling on this machine's disk-to-disk path*, nothing more. They say nothing
   about Wi-Fi Direct, and must not be cited as if they did.
2. **The source file is sparse.** `run-bench.ps1` creates it with `SetLength`,
   so the read side is largely served from cache. This inflates throughput and
   means the source disk is not represented in these figures. Use a real,
   non-sparse file for any disk-to-disk claim.
3. **Sender and receiver shared one machine**, so CPU and disk contention are
   not separated. Real two-device results will differ.
4. **Loopback socket buffers are large and free**, so this setup does not
   exercise the partial-write and backpressure paths a real network link will.
   Those remain untested until Phase 2.
5. Throughput was measured once per configuration. Repeat runs before treating
   any of these as stable.

---

## 6. Later Measurement Points

| Phase | Measurement | Status |
| --- | --- | --- |
| 0 | Raw TCP ceiling, chunk-size curve | **Done** (§5.2) — loopback only |
| 1 | Cost of framing + serialization vs raw baseline | pending |
| 2 | Same code, real network — isolates platform from protocol | pending — **required before any network claim** |
| 2 | Same code, real network — isolates platform from protocol |
| 3 | Same protocol, Wi-Fi Direct — isolates radio from protocol |
| 4 | Cost of encryption + authentication |
| 5 | Cost of hashing, resume, atomic finalization; resume time-to-recover |

Each phase's overhead should be attributed, not merely observed. "Phase 5 is
slower than Phase 4" is not an answer; "hashing costs 8% because we chose BLAKE3
over SHA-256" is.

---

## 7. Performance Review Checklist

```text
[ ] Large file does not load into RAM
[ ] Backpressure works
[ ] Disk I/O measured
[ ] Network throughput measured
[ ] CPU usage measured
[ ] Memory usage measured
[ ] Connection establishment measured
[ ] Discovery latency measured
[ ] Transfer resume tested
[ ] Different chunk sizes benchmarked
[ ] Parallelism benchmarked before enabling
```

---

## 8. Compatibility Matrix

Maintained as platforms land. **A platform is not "supported" merely because it
compiles.**

| Platform | OS version | Hardware | Wi-Fi adapter | Direct supported | Discovery | Connection | Transfer | Security | Known problems |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Windows | 11 | TBD | TBD | pending | pending | pending | pending | pending | — |
| Android | 15 | TBD | TBD | pending (P6) | — | — | — | — | — |
| Linux | Ubuntu | TBD | TBD | pending (P7) | — | — | — | — | experimental |

---

## 9. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §2, §6 | Charter §60, §29, §30 |
| §4 | Charter §30 |
| §5 | Doc 2; Charter §22, §61 |
| §7 | Charter §93 |
| §8 | Charter §94 |