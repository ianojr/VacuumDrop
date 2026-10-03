# Open Questions & Conflict Register

Status: **9 items resolved, 6 open.** Nothing here is resolved unilaterally
unless marked resolved.

Each item is a genuine ambiguity or contradiction in the source material.

---

## Resolved

| ID | Question | Resolution | Date |
| --- | --- | --- | --- |
| ✅ Q-1 / C-1 | Serialization crate | **`serde` + `postcard`** — [ADR-0007](./adr/0007-serialization-and-framing.md). `bincode` rejected: breaking rewrite of Doc 3's code, subtle schema evolution. Framing via `tokio-util` unaffected. | 2026-10-03 |
| ✅ Q-2 | Phase numbering collision | Benchmark is **Phase 0**; Charter's Phase 1–8 unchanged — [`roadmap.md` §3](./roadmap.md) | 2026-10-03 |
| ✅ Q-5 | Project name | **VacuumDrop.** Crate prefix `vacuumdrop-`, CLI binary `p2p`. (Directory remains `VaccumDrop` — see Q-14.) | 2026-10-03 |
| ✅ Q-13 | Workspace crate naming | `vacuumdrop-core`, `-protocol`, `-transport`, `-bench`, `-tests`; CLI `vacuumdrop-cli` — [`roadmap.md` §4](./roadmap.md) | 2026-10-03 |
| ✅ Q-16 | `clap` for the CLI | **Not adopted.** The Phase 1 surface is six subcommands with a handful of flags; hand-rolled parsing is ~120 lines, fully tested, and gives better error text. Recorded with a revisit trigger in `apps/cli/Cargo.toml` and [`dep-justification.md` §3](./dep-justification.md). | 2026-10-03 |
| ✅ Q-3 / C-3 | `Chunk { data: Vec<u8> }` unbounded allocation | **Resolved in code.** `FrameHeader::decode` validates `payload_len` against `max_frame_length` *before* the caller allocates; `FrameCodec::decode` additionally caps its own buffer at `MAX_READ_BUF`. Transfer messages are absent from Phase 1, so no `Chunk` decoder exists to carry the flaw forward. | 2026-10-03 |
| ✅ Q-4 / C-2 | `HelloAck { status: String }` | **Resolved in code.** `HelloStatus` is a closed enum (`Accepted` / `Rejected(HelloRejection)`) and `HelloRejection` is a closed enum. An unexpected value is a decode failure, not a string to interpret at runtime. | 2026-10-03 |
| ✅ Q-15 | Unvalidated message-type header field | **Found during Phase 1.** A property test showed bit flips in header bytes 8..10 decoded to the original message. The field was written but never validated, violating §32. Fixed: `ProtocolError::UnknownMessageType`, plus a test asserting the writer and validator cannot drift apart. | 2026-10-03 |

### Note on Q-3

Q-3 was originally recorded as blocking Phase 1 *and* Phase 5. Phase 1 is
genuinely closed. **Phase 5 remains constrained by it**: whoever writes the
`Chunk` decoder must validate `len == negotiated chunk_size` before decoding the
body, and should consider streaming the chunk body rather than materialising a
`Vec`. See [`file-transfer.md` §10](./file-transfer.md) and
[`threat-model.md` §4](./threat-model.md) T-01.

---

## Open — decide before the stated phase

### Q-6 — Windows binding: `windows` crate vs helper component

| | |
| --- | --- |
| **Affects** | Phase 3 |
| **Status** | **Resolved 2026-10-04 - `windows` crate, no helper component needed.** Ratified in [ADR-0009](./adr/0009-windows-wifi-direct-binding.md). |
| **Decision** | `windows` 0.62.2 + `windows-future` 0.3, quarantined in `vacuumdrop-platform-windows`. |
| **Evidence** | `IAsyncOperation` implements `IntoFuture`, so it drives on Tokio with no message pump. Zero `unsafe` in our code. `WiFiDirectAdvertisementPublisher` is bound. |
| **Correction** | The spike disproved two draft premises: `WiFiDirectDeviceListWatcher` is not in the binding (use `DeviceInformation` + `DeviceWatcher::Added`), and the approach does not require `unsafe`. |
| **Carried forward** | DHCP sufficiency is API-plausible but unverified on hardware, and capability detection still over-claims. Both are Phase 3 exit criteria. |

### Q-7 — Handshake / record protocol (Noise vs TLS 1.3)

| | |
| --- | --- |
| **Affects** | Phase 4 |
| **Status** | Deliberately deferred — [ADR-0010](./adr/0010-handshake-protocol.md) is a placeholder with the candidate list and the questions to answer. |
| **Risk** | Phase 4 cannot start without it. |

### Q-8 — Hash algorithm

| | |
| --- | --- |
| **Affects** | Phase 5; also the CLI output in [ADR-0008](./adr/0008-cli-first.md) |
| **Candidates** | **BLAKE3** — tree-structured, maps well onto chunked/resumable hashing, very fast. **SHA-256** — ubiquitous, hardware-accelerated on most CPUs. |
| **Proposed** | BLAKE3 for chunk + whole-file integrity |

### Q-9 — Persistence and trust-store backend

| | |
| --- | --- |
| **Affects** | Phase 5 (durable transfer journal), Phase 4 (identity + trust store) |
| **Candidates** | `rusqlite`/`sqlx` (SQLite) · plain files with atomic rename · embedded KV store |
| **Requirements** | Durable, crash-tolerant, atomic commit, no corruption on torn write (T-21) |
| **Note** | Plain files with atomic rename may be sufficient and much lighter. Do not add a database before the journal's actual needs are known. |

### Q-10 — Trust model after pairing

| | |
| --- | --- |
| **Affects** | Phase 4 |
| **Candidates** | TOFU with explicit user confirmation · user-managed allowlist · pre-provisioned pairing codes |
| **Proposed** | TOFU + explicit confirmation, with revocation. Pure TOFU lets an attacker present at first contact win permanently. |

### Q-11 — Licensing

| | |
| --- | --- |
| **Affects** | Dependency compatibility (§54 asks about license compatibility) |
| **Status** | Unspecified in all source documents. Every current dependency is MIT or Apache-2.0, both permissive and mutually compatible, so nothing is blocked today. `publish = false` on every crate prevents accidental publication meanwhile. Needed before distribution. |

### Q-12 — Toolchain and edition policy

| | |
| --- | --- |
| **Affects** | Everything |
| **Provisional** | Adopted during Phase 1, pending confirmation: Rust **edition 2024**, `rust-toolchain.toml` pinning `stable` with `rustfmt` + `clippy`. Verified building on 1.92.0 stable MSVC. MSRV is not yet declared — the `windows` crate requires 1.82, so 1.82 is the current floor by evidence, not by decision. |
| **Status** | Confirm or amend. If a lower MSRV matters for older Android toolchains (Phase 6), say so before Phase 4 locks dependencies. |

---

## Open questions that turned out to be non-questions

### Q-14 - Directory name typo

`VaccumDrop` (double `c`) on disk. Not a code problem - the crate and product
names are unaffected - but worth renaming for professionalism. Not urgent: renaming
would invalidate `Cargo.lock`, every recorded benchmark path, and this file's
relative links. **Recommend leaving it** until Phase 8, when there is no
downstream consumer to disturb.

---

## Recorded, no decision needed

| ID | Item | Resolution |
| --- | --- | --- |
| C-4 | Doc 3's enum has no version / session / request metadata | Resolved — [`protocol.md` §3.1](./protocol.md) is normative; Doc 3's enum is the Phase-1 subset |
| C-5 | Doc 3 omits pause / resume / verify / auth / cancel | Recorded as additions in the message catalogue; designed in their own phases. Transfer messages are **absent** from Phase 1 rather than stubbed |
| — | Phase-0 `Box<dyn Error>` vs Charter §39 structured errors | Explicit, documented Phase-0 exception. **Now closed**: `vacuumdrop-core` ships the full structured taxonomy and the benchmark crate is the only user of `Box<dyn Error>` |
| — | Phase-0 `tokio` `full` vs Charter §13 selectivity | `full` is scoped to the throwaway bench crate; the workspace selects features per crate |
| — | `sender`/`receiver` binaries vs `p2p` subcommands | Both kept, separated by purpose — [ADR-0008](./adr/0008-cli-first.md) |
| — | Mobile hotspot as a Wi-Fi Direct test rig | Documented as **infrastructure mode**, not P2P. Phase 2 baseline only — [`platform-windows.md` §2](./platform-windows.md) |
| — | `Box<dyn Transport>` vs generics for the transport trait | Object safety won: the engine must hold "whichever transport this platform provides" as one type. `BoxFuture` used instead of `async fn` in trait, which is not object-safe. Zero extra dependencies — [`dep-justification.md` §3](./dep-justification.md) |
| — | Codec error type | `FrameError` wraps `EngineError` plus the I/O variant `tokio-util` requires, rather than bolting a transport failure onto the protocol taxonomy where it does not belong |

---

## Also found during Phase 1

Beyond Q-3 and Q-4, two further gaps surfaced. Both are recorded so a future
reader does not rediscover them:

| ID | Item | Resolution |
| --- | --- | --- |
| Q-15 | Message-type header field written but never validated | **Fixed.** Violated Charter §32. `ProtocolError::UnknownMessageType` added, with a test asserting writer and validator cannot drift apart |
| — | Client handshake never established the session | **Fixed.** The client sent its capabilities and returned without waiting for the server's, so it sat in `AUTHENTICATING` and guessed at capabilities for the rest of the session — exactly what Charter §67 forbids. Both sides now exchange capabilities; the client validates before believing |