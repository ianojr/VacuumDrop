# ADR-0010 — Handshake and Record Protocol

**Status:** **Deferred — placeholder.** Must be decided before Phase 4 begins.
**Phase:** 4

---

## Context

Phase 4 must implement device identity, authentication, encrypted sessions, and
pairing.

The transport is TCP and we already own the framing layer
([ADR-0007](./0007-serialization-and-framing.md)). If we then run TLS inside our
own frames, we pay for two record layers. A purpose-built handshake such as
Noise is designed for exactly this shape.

Hard constraints from the Charter:

- **Do not invent cryptography.** No custom cipher, key exchange, MAC,
  signature, or password hashing.
- Use established cryptographic primitives and audited libraries.
- Must provide confidentiality, integrity, authenticity, and replay resistance.
- A connection does not imply trust.

## Decision

**Not yet decided.** No decision is recorded here.

This ADR exists to make the deferral explicit and to prevent a silent default
from becoming an implicit decision. Phase 4 must not begin without it.

## Candidates

| Option | Strengths | Costs |
| --- | --- | --- |
| **Noise Protocol Framework** (`snow`) | Purpose-built for this exact shape; handshake + record layer in one; no CA; small | Requires discipline on handshake message ordering and nonce handling; less tooling |
| **TLS 1.3** (`rustls`) | Battle-tested; ubiquitous tooling; strong AEAD; broad review | Record layer duplicates our framing; certificate/trust story needs design; heavier on the wire |
| **Application-layer AEAD only** | Simple and explicit | Still needs a key-agreement protocol — re-implementing the easy part of Noise badly is worse than using Noise |

## Questions to answer before deciding

1. Does the choice constrain the frame format (ADR-0007) or the transport
   (ADR-0003)? How much coupling is acceptable?
2. Forward secrecy — required, or acceptable without for resumed transfers?
3. Replay resistance window: per-session only, or across reconnects (matters for
   resume)?
4. How does the handshake interact with `Hello` / `VersionNegotiate`?
5. Does pairing bind the cryptographic identity to a physical-presence channel
   (SAS / QR / PIN)?
6. What is the cipher-suite negotiation policy, and how is downgrade prevented?

## Consequences of Deferring

- Phase 1–3 are unaffected: the protocol is designed so encryption slots in
  behind the transport abstraction without protocol rewrites.
- The cost of deferral is that the Phase-4 timeline is unbounded until this is
  decided.
- The risk of deciding early with incomplete information is worse: crypto
  choices are expensive to reverse.

## Related

[`../security.md` §3.1](../security.md),
[`../open-questions.md` Q-7](../open-questions.md),
[ADR-0005 — Cryptographic identity](./0005-cryptographic-identity.md),
[ADR-0007 — Serialization](./0007-serialization-and-framing.md)