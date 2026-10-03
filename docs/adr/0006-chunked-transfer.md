# ADR-0006 — Chunked Streaming Transfer

**Status:** Accepted
**Phase:** 5 (measurement in Phase 0)

---

## Context

The primary workload is transferring files of 10 GB and larger, possibly with
insufficient RAM to hold them, over links where throughput matters.

Three properties are non-negotiable:

1. A 10 GB file must transfer without 10 GB of RAM.
2. An interrupted transfer must resume, not restart.
3. Integrity must be verified before a file is declared complete.

## Decision

Transfer is **chunked and streamed end to end**:

```text
Disk → Chunk Reader → Integrity → Encryption → Network
     → Receiver → Verification → Disk
```

- Chunk size starts at **8 MiB as an experiment**, not a permanent requirement.
- Chunk size is **negotiated per transfer** and validated on every chunk.
- Memory depends on **buffer count**, never on file size.
- Per-chunk hash **plus** whole-file hash.
- Incomplete files are never exposed as complete: write to a temp file, verify,
  then **atomic rename**.

## Chunk size is measured, not chosen

Candidates: 256 KiB · 1 MiB · 4 MiB · **8 MiB** · 16 MiB · 32 MiB.

The Phase-0 benchmark measures the curve across at least three points. The
default is then chosen from data. See
[`../performance.md` §5](../performance.md).

## Alternatives Considered

| Alternative | Why rejected |
| --- | --- |
| Single unbounded stream | No resume granularity, no per-chunk verification, no integrity localisation |
| Whole-file hash only | Cannot localise corruption or resume efficiently; requires a full re-read to identify damage |
| mmap the file | Looks zero-copy; on Windows it is unreliable for files being written, and it still risks large mappings on 32-bit or memory-constrained devices. Not obviously better than `read` into a bounded buffer |
| Memory-map + send slices | Same concerns; adds platform-specific failure modes |
| Fixed 8 MiB forever | Explicitly rejected — the Charter requires measurement. Larger chunks waste RAM on flaky links and delay resume granularity; smaller chunks add per-chunk overhead |

## Consequences

**Positive**

- Bounded, predictable memory regardless of file size.
- Resume granularity of one chunk.
- Corruption is localised and identifiable.
- Completion is a verifiable claim, not a byte count.
- Temp-file + atomic rename means a partial file can never be mistaken for a
  complete one.

**Negative**

- Per-chunk hashing costs CPU; whole-file hashing costs another pass.
- Windows atomic rename can fail with `ERROR_SHARING_VIOLATION` when a scanner
  holds the destination — needs bounded retry.
- Resume requires durable receiver-side state, which adds a persistence
  subsystem and its own failure modes (T-21).
- Overhead quantisation at chunk boundaries wastes up to one chunk per transfer.

## Validation

- Peak RSS for 10 GB ≈ peak RSS for 10 MB. Non-negotiable; see
  [`../testing.md` §7](../testing.md).
- Hash overhead attributed in
  [`../performance.md` §6](../performance.md) — measured, not guessed.

## Related

[`../file-transfer.md`](../file-transfer.md),
[`../protocol.md` §2](../protocol.md)