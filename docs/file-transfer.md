# File Transfer

Status: **Phase 5 design.** Chunk-level protocol messages are specified in
[`protocol.md`](./protocol.md).

---

## 1. Pipeline

Transfer is **streaming end to end**. The file is never fully resident in RAM.

```text
Disk
  │
  ▼
Chunk Reader
  │
  ▼
Integrity
  │
  ▼
Encryption
  │
  ▼
Network
  │
  ▼
Receiver
  │
  ▼
Verification
  │
  ▼
Disk
```

Each stage is a separate, testable unit with a bounded buffer between it and the
next.

---

## 2. Chunking

Large files are divided into chunks. Initial recommended **experimental** size:

```text
8 MiB
```

> 8 MiB is **not** a permanent requirement. Benchmark it.

| Candidate |
| --- |
| 256 KiB · 1 MiB · 4 MiB · **8 MiB** · 16 MiB · 32 MiB |

The chosen default is decided by measurement, per
[ADR-0006](./adr/0006-chunked-transfer.md), using the Phase-0 baseline in
[`phase1-benchmark.md` §8](./phase1-benchmark.md).

Chunk size is **negotiated per transfer** and carried in `FileMetadata`, then
validated on every `Chunk` (§5.1 below).

---

## 3. Transfer Metadata

```rust
// Sketch — shape only.
pub struct TransferMetadata {
    pub transfer_id: String,
    pub file_id: String,
    pub file_name: String,
    pub file_size: u64,
    pub chunk_size: u32,
    pub total_chunks: u32,
    pub content_hash: String,
    pub created_at: Timestamp,
    pub sender: DeviceId,
    pub receiver: DeviceId,
    pub state: TransferState,
}
```

The receiver must hold enough metadata to **safely resume** the transfer.

`file_size`, `file_name`, `content_hash` and `total_chunks` are all
attacker-controlled until validated — see
[`threat-model.md` §5](./threat-model.md).

---

## 4. Resume

> A transfer interrupted at 8.7 GB of 10 GB **must not** restart from zero.

```text
10 GB
|
|███████████████████░
|                   |
|                8.7 GB
|
Connection lost
|
Reconnect
|
Resume
|
Continue from missing chunks
```

Requirements:

- The receiver maintains **durable** transfer state.
- On reconnect, both sides reconcile which chunks are actually present.
- Resume uses the existing `Chunk` / `ChunkAck` messages plus
  `TransferResume` — it does **not** require a parallel message family.

### 4.1 Resume is a correctness feature, not a convenience

It is also a **security control**: without it, an attacker who can interrupt a
transfer repeatedly can force unbounded rework and disk churn. See T-21 in
[`threat-model.md` §4](./threat-model.md).

---

## 5. Integrity Verification

Cryptographic hashes, at minimum:

```text
per-chunk hash  +  whole-file hash
```

Algorithm selection is open ([`open-questions.md` Q-8](./open-questions.md)); the
property required is that it comes from a modern, established cryptographic
library.

### 5.1 Completion criteria

> The receiver must **never** mark a transfer complete merely because all bytes
> arrived.

```text
all chunks received
      +
all required chunks verified
      +
whole-file integrity verified
      +
metadata committed
```

All four. Missing any one means the transfer is **not** complete.

### 5.2 Chunk validation

On every `Chunk`, before the payload is used:

- `index < total_chunks`
- `len(data) == negotiated chunk_size` (last chunk excepted)
- hash matches the declared per-chunk hash

A duplicate or out-of-range index is a protocol error, not a warning.

---

## 6. Atomicity

Incomplete files must never be exposed as complete files.

```text
temporary file
      │
      ▼
  transfer
      │
      ▼
   verify
      │
      ▼
atomic finalize / rename
```

On Windows, the rename must handle the case where a scanner or indexer holds a
handle on the destination — expect `ERROR_SHARING_VIOLATION` and retry with
bounded backoff rather than failing the whole transfer.

---

## 7. Backpressure

The sender must **not** transmit infinitely faster than the receiver can
process.

```text
Producer
   │
   ▼
Bounded Queue
   │
   ▼
Network
   │
   ▼
Receiver
```

Rules:

- **No unbounded channels.** Anywhere.
- **No unbounded buffering.** Anywhere.
- Memory usage depends on buffer count, never on file size (Charter §61).
- Backpressure must propagate all the way back to the disk reader — a bounded
  channel that the reader ignores is not backpressure.

This is the direct countermeasure to T-13 in
[`threat-model.md` §4](./threat-model.md).

---

## 8. Concurrency

Candidate independent tasks:

```text
Discovery · Session · Control Messages · File Reader · Network Writer
Network Reader · Integrity · Persistence · Telemetry
```

Concurrency must be **measurable and justified**. Do not create thousands of
tasks; each task is memory, scheduler time, and a failure path.

---

## 9. Parallel Transfer — Deferred

> **Do NOT** immediately open many simultaneous TCP streams.

Build first:

```text
single connection · single transfer stream
```

Benchmark it. Introduce parallel streams **only if measurements show the
transport would benefit**.

Possible future shape:

```text
Connection
 ├── Control Stream
 ├── Data Stream 1
 ├── Data Stream 2
 └── Data Stream N
```

This is an **optimisation, not a default assumption**.

---

## 10. Flow Control & Acknowledgement

Doc 3 requires that the sender **not** block on `ChunkAck` before sending the
next chunk — a strict stop-and-wait kills throughput.

Implement a **sliding window** with asynchronous acknowledgement:

```text
sender:  [0][1][2][3][4][5][6][7]  ← in-flight window
acks:         ✓   ✓   ✓   ✓   ✓
```

Window size is a bounded, negotiated value — not unbounded pipelining. It is
listed among the resource limits in
[`protocol.md` §5](./protocol.md) (proposed default: 8 chunks).

The window is also the natural place to express resume: on reconnect, the
receiver's durable chunk set defines where the window restarts.

---

## 11. Memory Requirement

> A 10 GB file must be transferable without requiring 10 GB of RAM.

Memory usage depends primarily on:

```text
buffer configuration + number of concurrent operations
```

**not** on file size.

Verification: RSS must remain flat across a 10 MB and a 10 GB transfer. Measured in
Phase 0 ([`phase1-benchmark.md` §5](./phase1-benchmark.md)) and re-verified in
Phase 5.

---

## 12. Storage Preconditions

Never assume the receiver has enough disk space. Perform practical checks before
starting.

```text
Insufficient space  →  reject transfer
```

See [`threat-model.md` §9](./threat-model.md).

---

## 13. Cancellation

Every long-running operation supports cancellation:

```text
cancel discovery · cancel connection · cancel transfer · cancel verification
shutdown engine
```

Cancellation **must release**:

- sockets · file handles · tasks · temporary files · locks · resources

Requires the `TransferCancel` message — currently missing from Doc 3's enum,
recorded as conflict C-5 in [`protocol.md` §9](./protocol.md).

---

## 14. Compression — Deferred

Compression is **not** applied blindly.

| Already-compressed | Likely benefit |
| --- | --- |
| MP4, ZIP, JPEG, PNG, 7z | Near zero, real CPU cost |

Future versions may inspect file characteristics or allow user/application policy.
If ever enabled, cap decompressed size to prevent zip bombs (T-23).

---

## 15. Directories — Future

```text
folder/
├── file1
├── file2
├── photo.jpg
└── videos/
    └── movie.mp4
```

Directory metadata must be validated. **Never trust received paths.** Same rules
as single files: sanitise, resolve inside a receiver-controlled root, reject
traversal.

---

## 16. Multiple Transfers — Future

```text
Device A
 ├── Transfer 1
 ├── Transfer 2
 └── Transfer 3
```

Concurrency limits **must** prevent resource exhaustion. Max simultaneous
transfers is a configured limit (proposed: 4).

---

## 17. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §2, §3 | Charter §21, §22, §23 |
| §4 | Charter §24 |
| §5, §5.1 | Charter §25 |
| §6 | Charter §63 |
| §7, §11 | Charter §26, §61 |
| §8 | Charter §27 |
| §9 | Charter §28 |
| §10 | Doc 3 §4 (sliding-window guideline) |
| §12 | Charter §62 |
| §13 | Charter §64 |
| §14 | Charter §68 |
| §15, §16 | Charter §69, §70 |