# ADR-0002 — Tokio as the Async Runtime

**Status:** Accepted
**Phase:** 1

---

## Context

The engine needs concurrent sockets, timers, cancellation, bounded task
scheduling, and concurrent transfers. The protocol is fundamentally
I/O-bound and stateful.

The Charter requires asynchronous execution, but also warns:

> Do not introduce Tokio everywhere without architectural reason.

## Decision

Use **Tokio 1.x** as the async runtime for the engine. Select features **per
crate** rather than defaulting to `full` across the workspace.

Tokio provides async sockets, task scheduling, timers, cancellation,
concurrent transfers, and — where it genuinely helps — async filesystem access.

## Alternatives Considered

| Alternative | Why rejected |
| --- | --- |
| `async-std` | Smaller ecosystem for the networking primitives needed; weaker Windows support history |
| `smol` | Excellent, but Tokio's breadth and maturity fit a project expecting to grow |
| Hand-rolled executor | Not defensible for a security-sensitive network engine |
| Blocking I/O + threads | Adequate for the Phase-0 benchmark; does not scale to concurrent sessions, timers, and cancellation |
| `glommio` / io_uring | Interesting throughput ceiling, but far less portable — poor fit for a cross-platform mandate |

## Consequences

**Positive**

- Mature, widely deployed, excellent Windows support.
- Rich timer and cancellation primitives that the state machines need.
- Broad ecosystem for codecs and compatibility.

**Negative**

- `full` is a large compile-time and binary-size cost; per-crate feature
  discipline is required or this decision quietly becomes "Tokio everywhere".
- Blocking calls on a runtime thread are a throughput hazard; each one must be
  identified and either made async or moved to `spawn_blocking`.
- Async call stacks are harder to reason about when debugging.

## Validation

- Async is used **only** where it provides value; blocking filesystem calls on hot
  paths must be justified.
- No blocking I/O in transfer hot loops (see
  [`../phase1-benchmark.md` §3.2](../phase1-benchmark.md)).
- Workspace feature selection reviewed whenever a crate is added.

## Related

[ADR-0003 — TCP first](./0003-tcp-first.md)