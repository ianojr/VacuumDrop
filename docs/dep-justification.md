# Dependency Justification

Status: **Normative.** Every dependency in the workspace is listed here with the
answers Charter 54 requires. Adding a dependency means adding a row.

---

## 1. Policy (Charter 54)

Before any crate is added, answer:

```text
What problem does it solve?
Is it maintained?
Is it necessary?
Does it introduce unsafe code?
Does it have known vulnerabilities?
Is the license compatible?
Does it increase binary size significantly?
Does the project actually need it?
```

Keep dependencies minimal. **Rule 3 of the engineering standards applies: a
dependency is never added silently.** It appears here or in an ADR, with the
reasoning visible.

---

## 2. Workspace Inventory

| Crate | Dependency | Purpose | Why not avoided |
| --- | --- | --- | --- |
| `vacuumdrop-core` | `thiserror` 2 | Derive `Display`/`Error`/`From` for ten error types | Hand-writing ~200 lines of boilerplate that a derive macro generates exactly. Proc-macro only; zero runtime dependency, so it adds nothing to the shipped binary. |
| `vacuumdrop-protocol` | `serde` 1 (derive) | Derives on protocol types | ADR-0007 rule 8 makes protocol types a reviewed public API. Derive keeps field order explicit in source, so an accidental reorder is visible in review rather than in production. |
| `vacuumdrop-protocol` | `postcard` 1 | Wire encoding | **Selected in ADR-0007** over `bincode` (breaking v1 rewrite, subtle schema evolution) and `rmp-serde` (self-describing bytes are wasted bandwidth on an untrusted wire). Tiny output, no configuration surface to misuse. |
| `vacuumdrop-protocol` | `tokio` 1 (io-util, sync, time, macros) | Async framing and session | ADR-0002. Features are listed individually: `full` is scoped to the benchmark crate only. |
| `vacuumdrop-protocol` | `tokio-util` 0.7 (codec) | `Encoder`/`Decoder` framing | Named by Charter 11 and ADR-0007. Reimplementing stream framing is exactly the kind of "sounds fast" complexity Charter 60 warns against. |
| `vacuumdrop-protocol` | `thiserror` 2 | Error derives | Consistent with core. |
| `vacuumdrop-transport` | `tokio` 1 (net, io-util, time) | Async sockets | ADR-0002. |
| `vacuumdrop-transport` | `thiserror` 2 | Error derives | Consistent. |
| `apps/cli` | `tokio` 1 (macros, rt-multi-thread, net, io-util, time, signal) | Runtime and sockets | ADR-0002. |
| `vacuumdrop-bench` | `tokio` 1 (**full**) | Phase 0 measurement harness | `full` is deliberate and scoped here: the benchmark measures a ceiling as fast as possible, and feature selection is noise in a throwaway crate. |
| `tests` | `proptest` 1 | Property tests for hostile input | Charter 52 requires property tests. The properties in `tests/tests/properties.rs` are the contract for the untrusted-input surface. |

### Dev-only

| Crate | Dependency | Purpose |
| --- | --- | --- |
| `vacuumdrop-protocol` | `proptest` 1 | Reserved for codec properties as the suite grows |

---

## 3. Deliberately Absent

Recorded so a future contributor knows these were considered, not overlooked.

| Crate | Why not |
| --- | --- |
| **`clap`** | The Phase 1 CLI is six subcommands with a handful of flags. Hand-rolled parsing is ~120 lines, fully tested, and gives better error text. Revisit the moment subcommands gain nested flags, aliases, and completions. |
| **`async-trait`** | The transport traits use `BoxFuture` directly. An `async fn` trait method is not object-safe, and the `BoxFuture` form costs one allocation per call — irrelevant beside a network round trip. Avoids a proc-macro dependency. |
| **`tracing` / `tracing-subscriber`** | Charter 59 requires structured logging, but Phase 1 emits too little to justify it yet. Deliberately deferred, not forgotten: it arrives with the transfer engine in Phase 5, where progress and correlation volume actually justify it. |
| **`bytes`** | `tokio_util::bytes` is already a transitive dependency of the codec work; depending on it directly would duplicate the tree for no benefit. |
| **`uuid`** | Session ids are per-process counters and transfer ids are Phase 5. Neither needs a UUID yet. |
| **`rand`** | Nothing in Phase 1 requires randomness. Session ids are monotonic counters. Cryptographic randomness arrives with Phase 4 identity, where a CSPRNG is mandatory — and it will come from the crypto library, not a general-purpose RNG crate. |
| **`anyhow`** | Charter 39 requires a structured error taxonomy. `anyhow` would erase exactly the distinctions the requirement exists to preserve. |
| **`futures`** | Only `std::future::Future` and `BoxFuture` are needed. |
| **`serde_json`** | A binary protocol does not need JSON. |

---

## 4. Security-Relevant Dependencies

Treated as remote-code-execution-adjacent, because a vulnerability in any of
these is a vulnerability in the protocol path.

| Crate | Exposure | Review cadence |
| --- | --- | --- |
| `postcard` | Parses untrusted input | Every dependency bump; fuzzed via `fuzz/` |
| `serde` | Derives only; no runtime parsing of its own | Every bump |
| `tokio-util` (codec) | Frames untrusted input | Every bump |
| `tokio` | Socket I/O | Every bump |
| `thiserror` | Proc-macro only, absent from the binary | Every bump |

### Fuzzing requirement

Charter 53 requires fuzzing the frame parser, message decoder, metadata parser,
filename handling, and transfer metadata. The parser **must not** panic, hang,
allocate unboundedly, or crash the process.

#### Targets built

| Target | Surface | Status |
| --- | --- | --- |
| `fuzz/frame_parser` | `FrameCodec::decode` over raw bytes | Built, compiles |
| `fuzz/message_decoder` | `postcard` payload -> `VacuumDropMessage` -> validators | Built, compiles |
| `fuzz/peer_validation` | `PeerDescription`, version lists, capability flags | Built, compiles |
| Metadata / filename / transfer-metadata | Chunk index and size bounds | Phase 5 — parsers do not exist yet |

The Phase-5 rows are listed because `testing.md` §4 requires them. They are **not**
implemented: a fuzz target for absent code would claim coverage that does not
exist.

#### Toolchain limitation, recorded

**The libFuzzer targets cannot execute on this machine.** Verified, not assumed:

```text
cargo fuzz build frame_parser   : OK
cargo fuzz run  frame_parser    : 0xc0000135 STATUS_DLL_NOT_FOUND
   with LLVM 22 DLL on PATH     : 0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND
```

`rustc` nightly is built against **LLVM 21.1.5**; the only ASan runtime installed
is **LLVM 22.1.5**, which is ABI-incompatible. `clang_rt.asan_dynamic-x86_64.dll`
is linked at build time and must match. Fixing this needs LLVM 21, or running the
targets on Linux/CI.

#### What runs today instead

`tests/tests/stress.rs` asserts the same four properties on stable Rust with a
deterministic PRNG. It is **not** equivalent to coverage-guided fuzzing — libFuzzer
uses instrumentation feedback to reach states random generation will not. It is
stronger in one respect: deterministic, and it runs in ordinary CI.

`scripts/check.ps1` attempts the real fuzzers, detects this specific failure mode,
and reports `SKIPPED` with the reason rather than silently passing.

#### What it found

`name_length_boundary_is_exact` caught a **real defect**: device names accepted
RIGHT-TO-LEFT OVERRIDE (`U+202E`), ZERO WIDTH SPACE (`U+200B`), and BOM
(`U+FEFF`), because `char::is_control()` covers only Unicode `Cc` and these are
`Cf`. A malicious peer could display as `"topakS na'l nI"`. Fixed; recorded as
threat **T-25**.

---

## 5. Licence Compatibility

**Q-11 is unresolved.** The source charter never specified a project licence, and
every dependency above is `MIT` or `Apache-2.0`, both permissive and mutually
compatible.

A licence decision is needed before distribution. It does not block development,
and `publish = false` is set on every crate so nothing is accidentally published
in the meantime.

---

## 6. Adding a Dependency

The checklist, in order:

1. Confirm the problem is real and the code cannot be written clearly here.
2. Check maintenance: recent releases, open issue responsiveness.
3. Check `cargo audit` for known advisories.
4. Count `unsafe` lines. A networking project wants this near zero.
5. Confirm the licence is compatible.
6. Measure the compile-time and binary-size cost.
7. Record it in §2 with the reasoning, and add an ADR if it affects the
   architecture.
8. If it is a cryptographic primitive, it **must** come from an established,
   audited library (Charter 55). Never implement one.

---

## 7. Source Traceability

| Section | Origin |
| --- | --- |
| 1 | Charter 54 |
| 2, 3 | Charter 54, 55, 89; ADR-0002, 0007 |
| 4 | Charter 31, 33, 53; threat-model T-01, T-24 |
| 5 | Charter 54 (licence compatibility), 56 |
| 6 | Charter 54, 55; engineering-standards rules 3 and 6 |