# ADR-0001 — Rust as the Portable Core

**Status:** Accepted
**Phase:** 1

---

## Context

VacuumDrop must run on Windows, Android, and Linux, and must keep working as
additional transports and platforms are added. Business logic duplicated across
platform languages is a correctness and security liability.

Wi-Fi Direct cannot be driven portably from any single language.

## Decision

Implement the engine in Rust. Platform-specific code — Android
(`WifiP2pManager`), Windows (Wi-Fi Direct), Linux (wpa_supplicant P2P), and
eventually Swift/Objective-C on Apple platforms — is restricted to **thin
adapters**.

All protocol, session, framing, transfer, integrity, crypto, and state-machine
logic lives in Rust exactly once.

## Alternatives Considered

| Alternative | Why rejected |
| --- | --- |
| Kotlin Multiplatform | Rejected the Rust-first requirement; still needs a native core for performance-sensitive paths |
| C++ core | Correct, but no memory safety and much larger unsafe surface for the protocol parser |
| Rust + per-platform business logic | Rejected — this is precisely the duplication the architecture forbids |
| Go | Good concurrency, but weak fit for the explicit `unsafe`-minimisation and type-state modelling the project depends on |

## Consequences

**Positive**

- One implementation of the security- and correctness-critical logic.
- Memory safety in the protocol parser, which is the primary attack surface.
- The type system can model protocol states so invalid states are unrepresentable
  ([`../engineering-standards.md` §6](../engineering-standards.md)).
- Adding a platform is an adapter, not a rewrite.

**Negative**

- FFI to each platform's Wi-Fi API is unavoidable, and WinRT-style async APIs are
  awkward from Rust (see [ADR-0009](./0009-windows-wifi-direct-binding.md)).
- Platform-specific concurrency models must be mapped into Rust's.
- Team must be proficient in async Rust and unsafe-code review.

## Validation

Rule 5 of the engineering standards. CI enforces that no core crate depends on a
platform crate — see [`../testing.md` §9](../testing.md).