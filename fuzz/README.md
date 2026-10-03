# Fuzz Targets

Status: **Active for the Phase-1 parsing surface.** Metadata and filename
targets arrive with Phase 5.

Charter 53 requires the protocol parser to be treated as security-sensitive. The
parser **must not**:

- panic
- hang
- allocate unbounded memory
- crash the process

This directory holds the `cargo-fuzz` targets that enforce those properties
against input no property test would think to generate.

---

## What is fuzzed, and what is not

| Target | Surface | Status |
| --- | --- | --- |
| `frame_parser` | `FrameCodec::decode` over raw bytes: tag, header, length, payload | **Active** |
| `message_decoder` | `postcard` payload into `VacuumDropMessage`, then validation | **Active** |
| Metadata parser | `FileMetadata` field bounds | Phase 5 — parser does not exist yet |
| Filename handling | Path-traversal rejection | Phase 5 — parser does not exist yet |
| Transfer metadata | Chunk index and size bounds | Phase 5 — parser does not exist yet |

The Phase 5 rows are listed because [`testing.md`](../docs/testing.md) section 4
requires them. They are **not** implemented, because a fuzz target for absent code
would be a claim of coverage that does not exist.

---

## Why both a fuzzer and property tests

They overlap, and that is deliberate:

| | Property tests | Fuzzing |
| --- | --- | --- |
| Input | Structured, from generators | Raw bytes, mutation-driven |
| Finds shallow bugs | Yes | Yes |
| Finds deep parser bugs | Rarely | Often |
| Reproducible by inspection | Yes | Only from a saved corpus |
| Runs in CI | Every commit | Bounded run per commit |

The property suite in [`tests/tests/properties.rs`](../tests/tests/properties.rs)
asserts the *contract*. These targets search for a *violation* of it. Property
tests alone would not have found the unvalidated message-type field that
`validated_header_fields_reject_bit_flips` exposed.

---

## Running

```bash
# One target, one minute. This is what CI runs.
cargo +nightly fuzz run frame_parser -- -max_total_time=60

# The other target.
cargo +nightly fuzz run message_decoder -- -max_total_time=60

# Longer soak. Use when actively hunting.
cargo +nightly fuzz run frame_parser -- -max_total_time=3600

# Replay a saved failure.
cargo +nightly fuzz run frame_parser fuzz/artifacts/frame_parser/crash-<hash>

# Triage the corpus.
cargo +nightly fuzz tmin frame_parser
```

`+nightly` is mandatory. libFuzzer needs the nightly sanitiser and coverage
instrumentation that stable Rust does not provide.

---

## Corpus

`corpus/<target>/` holds inputs that reached interesting parser states. It is
committed deliberately: it is the regression corpus, and a fuzzer that forgets
what it already learned will rediscover the same crashes on every machine.

`cargo fuzz cmin` shrinks it. Review any change with `git diff`.

---

## A failing target

1. The artifact lands in `fuzz/artifacts/<target>/`.
2. **Do not delete it.** Rule 9: never remove a test to make something pass.
3. `cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<file>` reproduces.
4. Fix the bug, then add a unit test to the crate that reproduces the *cause*,
   not the input. A regression test carrying a 200 KB byte blob protects nothing;
   a test asserting the invariant protects everything.
5. Keep the artifact in the corpus directory so the fuzzer keeps exercising the
   path.

---

## Requirements

- Nightly toolchain (`rustup toolchain install nightly`)
- `cargo-fuzz` (`cargo install cargo-fuzz`)
- Windows: libFuzzer builds under MSVC. If it fails, see the fallback note below.

### Fallback if libFuzzer will not build

Run the property suite at a high iteration count instead:

```bash
cargo test -p vacuumdrop-tests --test properties -- --ignored
```

This is **not** equivalent coverage and must not be described as such. If this
fallback is used, `testing.md` section 4 records the deviation rather than
claiming fuzz coverage that does not exist.

---

## Adding a target

1. Create `fuzz/targets/<name>.rs`.
2. Name it after the surface, not the technique: `frame_parser`, not `fuzz_bytes`.
3. Assert the four properties from the top of this file. A target that only
   checks for panics misses the allocation bound.
4. Add it to the gate in [`../scripts/check.ps1`](../scripts/check.ps1).
5. Record it in [`../docs/dep-justification.md`](../docs/dep-justification.md)
   section 4.

---

## Related

[`../docs/testing.md`](../docs/testing.md) section 4 ·
[`../docs/threat-model.md`](../docs/threat-model.md) T-01, T-05 ·
[`../docs/adr/0007-serialization-and-framing.md`](../docs/adr/0007-serialization-and-framing.md)