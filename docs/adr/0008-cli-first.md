# ADR-0008 - CLI First

**Status:** Accepted
**Phase:** 1

---

## Context

The project needs fast iteration, debuggability, and CI-testable behaviour before
it has a stable feature set. A GUI built now would be rewritten repeatedly and
would obscure what the engine is actually doing.

Doc 2 also asks for two standalone `sender` / `receiver` binaries, while the
Charter (50, 101) specifies a single `p2p` CLI with subcommands.

## Decision

Build the **CLI first**. It is primarily a development and debugging tool. The
GUI comes in Phase 8, only after the engine is stable.

Command surface:

```text
p2p discover
p2p peers
p2p connect <peer>
p2p send ./movie.mkv <peer>
p2p transfers
p2p resume <transfer-id>
```

Separately, Phase 0 ships a minimal `sender` / `receiver` benchmark pair in
`crates/vacuumdrop-bench`, kept deliberately free of workspace dependencies so it
measures the raw TCP ceiling without contamination.

### Intended session

```text
$ p2p discover

Searching for nearby devices...

1. Ian-Laptop
   Device ID: 82f4...
   Status: Available

2. Pixel-Device
   Device ID: 91ac...
   Status: Available
```

```text
$ p2p connect 82f4

Connecting...
Peer authenticated.

Connection established.
```

```text
$ p2p send ./video.mkv 82f4

File: video.mkv
Size: 10.42 GB

Preparing transfer...

[##################..] 91.3%

Speed: 486 MB/s
Transferred: 9.51 GB
Remaining: 0.91 GB

Verifying...

Transfer complete.

SHA-256: VERIFIED
```

> The final hash algorithm shown in output is **provisional** - see
> [`../open-questions.md` Q-8](../open-questions.md).

## Alternatives Considered

| Alternative | Why rejected |
| --- | --- |
| GUI first | Rewritten repeatedly; hides engine behaviour; hard to test in CI |
| Daemon/service first | Premature - the process model is not yet settled, and a daemon makes crash diagnosis harder during development |
| Library only, no interface | No way to exercise the engine manually during development |
| TUI (ratatui) | Attractive, but premature. Deferred until the engine is stable |
| Only `sender`/`receiver` binaries | Sufficient for the benchmark; insufficient as a long-term interface - no subcommand structure, no transfer listing, no resume UX |

## Consequences

**Positive**

- Every feature is manually exercisable and scriptable.
- Integration tests can drive the same code path as production.
- Fast feedback during protocol development.
- Benchmarks stay clean and isolated.

**Negative**

- Output formatting work is thrown away when the GUI lands (accepted - it is
  small).
- Rich progress display is limited by terminal capabilities.
- Long-running transfers need care so the CLI does not become the reason a
  transfer dies.

## Related

[`../roadmap.md` section 14](../roadmap.md),
[`../phase1-benchmark.md` section 4](../phase1-benchmark.md)