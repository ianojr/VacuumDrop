# Engineering Standards

Status: **Normative.** Binding on all code and all AI agents working on this
repository.

---

## 1. Agent Operating Mode

Act as a:

> **Senior Rust systems engineer + networking engineer + security engineer.**

Do:

- Challenge unsafe architectural assumptions
- Identify hidden failure modes
- Explain important decisions
- Prioritise correctness, then security
- Benchmark before claiming performance
- Avoid premature optimisation
- Maintain clean boundaries
- Preserve portability
- Write tests
- Maintain documentation

Do **not** act as:

> "Generate as much code as possible."

---

## 2. The Ten Rules

| # | Rule |
| --- | --- |
| 1 | Do not implement large amounts of code without understanding the architecture |
| 2 | Before changing architecture, explain why |
| 3 | Do not silently introduce dependencies |
| 4 | Do not rewrite working components unnecessarily |
| 5 | Do not mix platform-specific code into core crates |
| 6 | Do not invent cryptography |
| 7 | Do not claim compatibility without testing |
| 8 | Do not claim performance without benchmarking |
| 9 | Do not remove tests to make code compile |
| 10 | Do not disable security checks merely to make a feature work |

Rule 9 deserves emphasis: a test that blocks a release is doing its job. Deleting
it converts a known defect into an unknown one.

---

## 3. Feature Workflow

For every feature:

```text
 1. Understand requirements
 2. Inspect existing architecture
 3. Identify affected modules
 4. Design solution
 5. Explain important tradeoffs
 6. Implement
 7. Format
 8. Compile
 9. Run tests
10. Run relevant integration tests
11. Review for security
12. Review for performance
13. Document changes
```

**Do not jump directly from request to massive code generation.**

---

## 4. Repository Exploration Rule

Before modifying existing code, inspect:

```text
Cargo.toml · workspace structure · module boundaries · tests · configuration
existing traits · existing errors · existing protocol definitions
```

Never assume a file exists. Never invent an API that has not been inspected.

---

## 5. Coding Style

Prefer:

```text
small modules · small functions · explicit types · clear names
strong enums · typed errors · documented invariants
```

Avoid:

```text
giant functions · global mutable state · magic numbers
hidden background tasks · implicit state
unnecessary macros · unnecessary unsafe
```

### 5.1 Magic numbers

Buffer sizes, limits, and timeouts are named constants with a documented source
(phase, measurement, or threat). `8 * 1024 * 1024` inline in a loop is a magic
number; `CHUNK_SIZE` with a comment explaining why 8 MiB is not a magic number is
not.

---

## 6. Rust-Specific Rules

Use the type system to model invalid states where practical.

```rust
// Good — the invalid states are unrepresentable.
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Authenticating,
    Established,
    Closing,
}
```

```rust
// Bad — every invariant is now a runtime check someone will forget.
struct Session {
    status: String,
}
```

Prefer structured types over generic maps. A `HashMap<String, Value>` in the
protocol path is a validation debt collector.

---

## 7. Error Handling

Never use:

```text
unwrap() · expect() · panic!()
```

in production paths — unless there is a documented invariant that makes failure
impossible **and** the decision is justified in a comment.

Use structured errors that distinguish:

```text
TransportError · ProtocolError · AuthenticationError · AuthorizationError
IntegrityError · StorageError · TimeoutError · PeerDisconnected
UnsupportedVersion · InvalidMessage
```

Every input-derived failure path is a security boundary — see
[`threat-model.md` §5](./threat-model.md).

> The Phase-0 benchmark crate uses `Box<dyn Error>` as an explicit, documented
> exception. Do not let that pattern escape into the workspace.

---

## 8. Unsafe Rust

`unsafe` is permitted only when necessary. Every `unsafe` block must explain:

```text
Why unsafe is required
What invariant makes it safe
What assumptions are made
```

If safe Rust can reasonably accomplish the same task, prefer safe Rust.

Expected `unsafe` budget: near zero in core crates; concentrated in
`platform::*` FFI.

---

## 9. Dependency Policy

Do not add a crate merely because it is convenient. Before adding a dependency,
answer:

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

Keep dependencies minimal. Rule 3 applies: **do not add a dependency silently** —
surface it in the discussion or an ADR.

### 9.1 Cryptographic dependencies

Primitives **must** come from established libraries. Never implement:

```text
AES · ChaCha · SHA · Ed25519 · X25519 · HKDF · TLS · Noise
```

from scratch. This is a networking system, not a cryptography research project.

### 9.2 Library evaluation

For any library category, **evaluate** before adopting:

| Category | Candidate shortlist |
| --- | --- |
| Async runtime | Tokio (ratified — [ADR-0002](./adr/0002-tokio.md)) |
| Serialization | evaluate — [ADR-0007](./adr/0007-serialization-and-framing.md) |
| Cryptography | evaluate — [ADR-0010](./adr/0010-handshake-protocol.md) *(pending)* |
| Logging / tracing | `tracing` + `tracing-subscriber` |
| ID generation | `uuid` v4/v7, or content-derived IDs |
| Hashing | evaluate — [`open-questions.md` Q-8](./open-questions.md) |
| Testing | built-in + `proptest` |
| Fuzzing | `cargo-fuzz` |

Exact versions are determined from the ecosystem **when implementation begins**.
Do not hard-code obsolete versions into the specification.

---

## 10. Secrets

Private keys must be protected. Never:

```text
commit private keys · log private keys · send private keys
store keys in source code · store secrets in Git
```

---

## 11. Definition of Done

A feature is **not** done because the code compiles.

```text
requirements satisfied
    +
implementation complete
    +
tests pass
    +
security reviewed
    +
failure paths handled
    +
performance measured where relevant
    +
documentation updated
```

Rules 7 and 9 of §92 reinforce this: no test may be deleted to reach "done", and
no security check may be disabled to reach "done".

---

## 12. Documentation Obligations

Every major subsystem requires documentation, per
[`docs/README.md`](./README.md). Major decisions become ADRs with **Context,
Decision, Alternatives, Consequences, Status**.

---

## 13. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §2, §3, §4 | Charter §99, §83, §84, §85 |
| §5, §6, §7, §8 | Charter §86, §87, §39, §88 |
| §9 | Charter §54, §55, §89 |
| §10 | Charter §56 |
| §11 | Charter §98, §92 |