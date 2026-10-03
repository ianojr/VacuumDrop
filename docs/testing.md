# Testing Strategy

Status: **Normative.** Testing is mandatory, not optional.

---

## 1. Test Layers

| Layer | Scope |
| --- | --- |
| **Unit** | Protocol encoding/decoding, state machines, chunk calculations, hashes, validation, path handling, transfer state, error conversion |
| **Integration** | `sender ↔ receiver`, real sockets, real files |
| **Property** | Malformed frames, random metadata, chunk boundaries, serialisation round-trips |
| **Security** | Malformed packets, oversized messages, path traversal, replay, invalid auth, invalid chunk numbers, memory exhaustion |
| **Performance** | Throughput, latency, CPU, memory, disk |

---

## 2. Unit Tests — Priority Targets

Ordered by risk, highest first.

| Target | Property under test |
| --- | --- |
| Frame parser | Rejects every malformed input without panicking |
| Length validation | Rejects oversized payload **before** allocation |
| Chunk arithmetic | `total_chunks` consistent with `file_size / chunk_size`; handles 0, 1, exact-multiple, and remainder cases |
| State machine | Rejects every invalid transition |
| Filename sanitisation | Strips traversal, absolute paths, drive letters, UNC, NUL, reserved names |
| Hash verification | Detects single-bit flips, truncation, reordering, duplication |
| Serialisation round-trip | Encode → decode is identity for all valid messages |
| Error conversion | Every internal error maps to a protocol-level error without losing information |

---

## 3. Property Tests

Use `proptest` for:

- Random byte strings into the frame parser → must never panic, never hang,
  never allocate unboundedly.
- Random metadata → validation must reject out-of-range values.
- Chunk boundary conditions → sizes 0, 1, `chunk_size - 1`, `chunk_size`,
  `chunk_size + 1`, `file_size`.
- Serialisation round-trips for generated valid messages.

---

## 4. Fuzzing

The protocol parser is **security-sensitive** and is treated as such.

| Target | Phase |
| --- | --- |
| Frame parser | 1 |
| Message decoder | 1 |
| Metadata parser | 5 |
| Filename handling | 5 |
| Transfer metadata | 5 |

The parser must **not**:

- panic
- hang
- allocate unbounded memory
- crash the process

Run `cargo-fuzz` in CI, at minimum as a short bounded run per change.

### 4.1 Toolchain reality on this machine (recorded, not aspirational)

**The libFuzzer targets compile but cannot execute here.** Verified:

```text
rustc nightly        : 1.94.0-nightly, built against LLVM 21.1.5
LLVM runtime present : 22.1.5  (C:\Program Files\LLVM)
cargo fuzz build     : OK
cargo fuzz run       : 0xc0000135 STATUS_DLL_NOT_FOUND
with DLL on PATH     : 0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND
```

The ASan runtime Rust links against (`clang_rt.asan_dynamic-x86_64.dll`) must
match `rustc`'s LLVM version. LLVM 22's copy is present and is ABI-incompatible
with a 21.1.5-compiled binary, which is why adding it to `PATH` changes the error
rather than fixing it. This needs LLVM 21 installed, or running the fuzz targets
on Linux/CI.

### 4.2 The fallback that runs today

`tests/tests/stress.rs` asserts the same four properties on stable Rust with a
deterministic PRNG:

| Property | Test |
| --- | --- |
| No panic on any input | `frame_decoder_survives_random_input` (50,000 iterations) |
| Bounded memory | `declared_length_never_drives_allocation` (10,000 iterations, near-`u32::MAX` declared lengths) |
| No unbounded allocation | same |
| No input consumed on failure | both above |
| Validation matches its contract | `validators_agree_with_their_contract_on_random_input` |
| Exact name-length boundary | `name_length_boundary_is_exact` |
| Version-list contract everywhere | `version_list_contract_holds_everywhere` (6,400 combinations) |
| Unknown flags non-fatal | `unknown_capability_flags_are_discarded_not_fatal` (all 256 byte values) |

**This is not equivalent to coverage-guided fuzzing.** libFuzzer uses
instrumentation feedback to reach states random generation will not; this does
not. It is stronger in one respect: deterministic and on stable Rust, so the
properties are checked every commit rather than only where the toolchain works.

`fuzz/` remains authoritative. Run both when its toolchain requirement is met.

### 4.3 What the fallback found

`name_length_boundary_is_exact` immediately caught a **real defect**: peer device
names accepted RIGHT-TO-LEFT OVERRIDE (`U+202E`), ZERO WIDTH SPACE (`U+200B`), and
BOM (`U+FEFF`).

`char::is_control()` returns `false` for all of these — they are Unicode `Cf`
*format* characters, not `Cc` *control* characters. So the name validator let
through text that renders reversed or invisible.

**Impact:** a malicious peer could display as `"topakS na'l nI"` while the
underlying string is `"Ian's Laptop"`. That is a device-impersonation primitive
against the exact UI where a user decides whether to trust something.

**Fixed** in `vacuumdrop-core` with an explicit character-class check, plus tests
confirming legitimate non-ASCII names (`Büro`, `ノートブック`, `Ноутбук`, emoji) are
still accepted. Recorded as threat T-25.

---

## 5. Security Tests

Each maps to a threat in [`threat-model.md` §4](./threat-model.md).

| Test | Threat | Expected result |
| --- | --- | --- |
| Truncated frame | T-05 | `InvalidMessage`, no panic |
| Declared length > max | T-01 | Rejected pre-allocation |
| `file_size` = 18 EB | T-02 | Rejected at admission, no allocation |
| `file_name` = `../../secret.txt` | T-03 | Sanitised or rejected |
| `file_name` = `C:\Windows\...` | T-03 | Sanitised or rejected |
| `chunk_index` ≥ `total_chunks` | T-01 | `InvalidMessage` |
| Wrong `chunk_size` for negotiated size | T-01 | `InvalidMessage` |
| Unknown message type | T-05 | `Error` message, no panic |
| Unknown protocol version | T-17 | `UnsupportedProtocolVersion`, fail closed |
| False capability claims | T-18 | Ignored / validated |
| No handshake response | T-19 | Timeout, not indefinite hang |
| Replayed frame | T-09 | Rejected by nonce/counter |
| Unauthenticated session requesting data | T-06 | `AuthenticationError` |
| Connection flood | T-10 | Rate limited |
| Disk full mid-transfer | T-14 | Clean failure, temp file cleaned up |
| Corrupted transfer journal | T-21 | Recoverable, no corruption |

---

## 6. Integration Tests

```text
sender <-> receiver
```

Real sockets. Real files. `localhost` by default.

| Test | Verifies |
| --- | --- |
| Happy path, small file | Correct round-trip |
| Happy path, multi-chunk | Chunk sequencing |
| Interrupted + resumed | Resume correctness |
| Receiver disconnects mid-transfer | Clean sender-side error |
| Sender disconnects mid-transfer | Temp file preserved for resume |
| Concurrent transfers | Bounded concurrency holds |
| Slow reader | Backpressure propagates; memory stays flat |
| Cancel mid-transfer | Resources released, temp file removed |

---

## 7. Performance Tests

Recorded in [`performance.md`](./performance.md). Automated where practical,
with **tolerance bands** rather than fixed thresholds — a benchmark that fails CI
on a 5% variance is a benchmark that gets ignored.

The one non-negotiable assertion:

> Peak RSS for a 10 GB transfer must not materially exceed peak RSS for a 10 MB
> transfer.

That single check enforces the streaming property. See
[`file-transfer.md` §11](./file-transfer.md).

---

## 8. Repository Layout

```text
tests/                 # cross-crate integration tests
crates/*/tests/        # crate-level integration tests
crates/*/src/**        # #[cfg(test)] unit tests
benches/               # performance harnesses
fuzz/                  # cargo-fuzz targets
```

---

## 9. CI Gates

Proposed, enforced on every change:

```text
[ ] cargo fmt --check
[ ] cargo clippy -- -D warnings
[ ] cargo build
[ ] cargo test
[ ] cargo test --doc
[ ] cargo fuzz run <targets> -- -max_total_time=60
[ ] cargo tree  # no core crate depends on a platform crate
[ ] cargo audit (if adopted)
[ ] core crates contain no `unsafe` without a documented justification
```

The dependency-direction check is the mechanical enforcement of
[`architecture.md` §9](./architecture.md). If it is not automated, it will be
violated.

---

## 10. Source Traceability

| Section | Origin |
| --- | --- |
| §1 | Charter §52 |
| §2, §3 | Charter §52 (unit, property) |
| §4 | Charter §53 |
| §5 | Charter §52 (security tests), §31 |
| §6, §7 | Charter §52, §30, §61 |
| §8 | Charter §8 |
| §9 | Derived from Charter §33, §47, §98 |