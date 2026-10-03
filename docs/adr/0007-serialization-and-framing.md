# ADR-0007 — Serialization and Framing

**Status:** **Accepted**
**Decided:** 2026-10-03, superseding the Doc 3 `bincode` proposal
**Affects:** Phase 1

---

## Context

**Doc 3 mandates `serde` + `bincode` with `tokio-util`'s
`LengthDelimitedCodec`.**

The Charter pulls the other way:

- §54 — do not add a crate merely because it is convenient; evaluate first.
- §89 — the AI should **evaluate**, rather than blindly install, serialization
  libraries.
- §32 / §33 / §35 / §36 — every value from the wire is untrusted and bounded.

There is also a practical problem: `bincode` 2.x is a breaking API change from
1.x, so Doc 3's code is not portable as written.

Framing itself is **not** in question — `tokio-util` framing is ratified by
Charter §11 and by Doc 3. The open question is the payload codec.

## Decision

**`serde` + `postcard`**, framed with `tokio_util::codec::LengthDelimitedCodec`.

Framing via `tokio-util` was never in question — Charter §11 and Doc 3 agree on
it. The open question was the payload codec, and it is now settled.

> **Resolves** [Q-1 / C-1](../open-questions.md). Doc 3's `bincode` proposal is
> superseded.

## Alternatives Considered

| Option | Size | Notes |
| --- | --- | --- |
| **`postcard`** | Very small | No `varint`-vs-fixed-width config surface to get wrong. serde-native. Downside: less ergonomic debugging, weaker ecosystem. **SELECTED** |
| `bincode` 2.x | Small | Fast; serde-native. **Rejected:** breaking rewrite of Doc 3's code; schema evolution and per-field config are subtle; known footgun class around `serde` compatibility |
| `rmp-serde` (MessagePack) | Medium | Self-describing — debuggable, but self-describing bytes on an untrusted wire is wasted bandwidth and unnecessary attack surface |
| Manual `from_le_bytes` / `to_le_bytes` | Minimal | Maximum control; **highest bug risk** in the parser, which is the primary attack surface. Explicitly disfavoured by §33 |
| CBOR / JSON | Large | Undersized for a throughput-oriented path |

## Consequences

**Positive**

- serde keeps the domain types clean and testable.
- `postcard` is compact and has no serialisation-config surface to misuse.
- `LengthDelimitedCodec` solves TCP stream framing (§11) and enforces
  `max_frame_length` — the primary defence against T-01.
- Both serde and postcard are far safer than hand-rolled parsing.

**Negative / shared risk**

- `postcard` is schema-sensitive: field order and types are part of the wire
  format. Mitigation: explicit numeric discriminants, `#[serde(rename)]` on
  every variant and field, and golden-byte tests that fail on accidental format
  change. **Mandatory, not optional.**
- Renaming or reordering a field is a **breaking protocol change**. Treat the
  message types as a public API and review them with the care that implies.
- `postcard` is not self-describing, so debugging captured wire bytes requires
  the decoder — acceptable for a measured-throughput path, and a real cost when
  diagnosing a field-level bug. `tracing` at the message-type level is the
  mitigation.
- Neither codec enforces semantic bounds. `max_frame_length` and per-field
  validation are still mandatory (see T-01).

## Required Alongside This Decision

1. `LengthDelimitedCodec` with an explicit `max_frame_length`
   ([`../protocol.md` §3.2](../protocol.md)).
2. Validate payload length **before** allocation (T-01).
3. Explicit numeric discriminants on the message enum.
4. `#[serde(rename)]` on every variant and field, so Rust-side renames cannot
   silently alter the wire format.
5. Golden-byte tests asserting exact encoding for every message variant — these
   are the regression net for schema drift.
6. `#[serde(deny_unknown_fields)]` where forward compatibility is not required,
   so unknown fields are rejected rather than ignored.
7. A version field in every frame, so a codec change cannot be mistaken for a
   protocol version change.
8. Protocol-message type definitions reviewed as a **public API** — any change
   is a protocol change and goes through this ADR process.

## Related

[`../protocol.md` §3](../protocol.md), [`../protocol.md` §9](../protocol.md) (C-1),
[`../threat-model.md` §4](../threat-model.md) (T-01, T-24)