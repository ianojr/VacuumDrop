# Threat Model

Status: **Living document.** Reviewed every phase; re-reviewed whenever the
protocol or platform surface changes.

---

## 1. Adversary Model (Charter §31)

Assume attackers may:

| Capability | Consequence for the design |
| --- | --- |
| Be physically nearby | Discovery is hostile; pairing must be explicit |
| Join the same local network | Network presence proves nothing |
| Impersonate another device | Identity must be cryptographic, not attribute-based |
| Send malformed packets | Parser is a security boundary — fuzz it |
| Send huge metadata values | Every field needs a bound, enforced pre-allocation |
| Attempt denial of service | Limits, timeouts, and rate limits are mandatory |
| Attempt replay attacks | Nonces and monotonic counters required |
| Interrupt transfers | Resume is a correctness requirement |
| Manipulate filenames | Path traversal defence is mandatory |
| Exploit decompression / parsing bugs | No bespoke parsing of received structures |
| Attempt memory exhaustion | No unbounded allocation, ever |
| Send unexpected protocol versions | Version negotiation with explicit failure |

> **All remote input is hostile.** Not "most". Not "trusted after
> authentication". A peer that authenticates can still send a hostile frame.

---

## 2. Assets

| Asset | Impact if compromised |
| --- | --- |
| Device private keys | Full identity impersonation, permanent |
| Session secrets | Traffic decryption |
| Transferred file contents | Data disclosure |
| File integrity | Tampered content accepted as authentic |
| Peer trust store | Silent trust escalation to an attacker |
| Transfer resume state | Corruption, or write to unintended paths |
| Local disk | Storage exhaustion, overwrite of existing data |

---

## 3. Trust Boundaries

```text
┌──────────────────────────────────────────────────────────────┐
│  APPLICATION            trusted: our own code                │
└───────────────────────────┬──────────────────────────────────┘
                            │  API calls
┌───────────────────────────┴──────────────────────────────────┐
│  RUST CORE             trusted, but processes hostile input  │
└───────────────────────────┬──────────────────────────────────┘
                            │  ▲
┌───────────────────────────┴──────────────────────────────────┐
│  WIRE / PROTOCOL      ╳══ UNTRUSTED BOUNDARY ═══           │
└───────────────────────────┬──────────────────────────────────┘
                            │
┌───────────────────────────┴──────────────────────────────────┐
│  PLATFORM ADAPTER    semi-trusted: OS, but may fail or lie   │
└──────────────────────────────────────────────────────────────┘
```

**Boundary 1 — the wire.** The single most important boundary in the project.
Everything crossing it is attacker-controlled.

**Boundary 2 — the platform adapter.** The OS is generally trusted, but it may:
report capabilities incorrectly, hand back a connection that silently dies, or
expose platform-specific objects that must never leak inward.

---

## 4. Threat Register

Severity: **H** high, **M** medium, **L** low. Phase = when it must be closed.

| ID | Threat | Sev | Mitigation | Phase |
| --- | --- | --- | --- | --- |
| T-01 | Unbounded allocation from declared payload length | **H** | Validate length **before** decode; codec `max_frame_length`; bound every numeric field | 1 |
| T-02 | Memory exhaustion via declared `file_size` (e.g. 18 EB) | **H** | Never allocate or reserve from a peer-declared size; check free disk space first; reject early | 5 |
| T-03 | Path traversal via received filename (`../../secret.txt`, `C:\Windows\...`) | **H** | Sanitise to a bare filename; resolve against a receiver-controlled directory; reject absolute paths, drive letters, UNC, `..`, NUL, reserved names | 5 |
| T-04 | Overwrite of existing files without consent | **H** | Explicit collision policy; never blind-overwrite | 5 |
| T-05 | Parser panic on malformed frame | **H** | No `unwrap`/`expect`/`panic!` on input paths; fuzz the parser; total functions returning `Result` | 1 |
| T-06 | Unauthenticated peer gains trust | **H** | Explicit authentication + user-confirmed pairing; connection ≠ trust | 4 |
| T-07 | Silent trust escalation | **H** | Pairing requires deliberate user action; no auto-trust on repeat connection | 4 |
| T-08 | Traffic eavesdropping on the radio | **H** | Application-layer encryption; never rely on Wi-Fi Direct security alone | 4 |
| T-09 | Replay of captured frames | **M** | Monotonic nonces/counters per session; reject replayed sequence numbers | 4 |
| T-10 | Resource exhaustion via connection flood | **M** | Max simultaneous sessions; connection-rate limit | 1 |
| T-11 | Authentication lockout weaponised against a legitimate user | **M** | Bounded, time-based cooldown; per-peer; not persisted by default | 4 |
| T-12 | Unbounded task creation | **M** | Bounded concurrency; no task spawned per untrusted event without a cap | 1 |
| T-13 | Unbounded queue growth in the transfer pipeline | **M** | Bounded channels everywhere; backpressure propagates to the disk reader | 5 |
| T-14 | Disk exhaustion via many concurrent transfers | **M** | Max simultaneous transfers; pre-flight free-space check | 5 |
| T-15 | Secrets leaked into logs | **M** | Redacting `Display` by default; log review in release checklist | 4 |
| T-16 | Malicious device name / metadata in the UI | **M** | Length limits; reject control and invisible-format characters before rendering; never `exec` or auto-open | 5 |
| T-17 | Protocol downgrade to an insecure version | **M** | Explicit version negotiation; fail closed with `UnsupportedProtocolVersion` | 1 |
| T-18 | Capability claims trusted blindly | **M** | Treat all advertised capabilities as unverified claims; validate before use | 1 |
| T-19 | Stuck connection (no timeout) | **M** | Explicit timeouts on connect, handshake, auth, metadata, idle | 1 |
| T-20 | Partial file exposed as complete | **M** | Write to temp file; atomic rename only after full verification | 5 |
| T-21 | Resume state corruption after unclean shutdown | **M** | Durable, atomically-committed transfer journal; tolerate torn writes | 5 |
| T-22 | Temp file left behind on failure/cancel | **L** | Cancellation releases temp files and handles | 5 |
| T-23 | Compression side-channel / CPU exhaustion via zip bomb | **L** | Compression off by default; cap decompressed size if enabled | future |
| T-24 | Third-party dependency supply-chain compromise | **M** | Minimal pinned deps; audit before adding; review licenses | 1 |
| T-25 | **Device-name spoofing via invisible or reordering characters** | **H** | `is_disallowed_in_name` rejects Unicode `Cf` format chars (RLO/LRO, zero-width, BOM, isolates), private-use and noncharacter code points. `char::is_control` alone is insufficient — it covers `Cc` only | 1 |

---

## 5. Input Validation Matrix (Charter §32)

Never trust:

```text
file names · file sizes · chunk numbers · chunk sizes · message lengths
protocol versions · device names · metadata · IDs · timestamps
capability lists
```

| Input | Validation | Where |
| --- | --- | --- |
| Magic | Exact match; reject immediately | Frame reader |
| Protocol version | In supported set; otherwise `UnsupportedProtocolVersion` | Handshake |
| Flags | Unknown reserved bits → reject | Frame reader |
| Message type | Known type; otherwise `Error` (never panic) | Frame reader |
| Session ID | Matches current session, or is a legitimate `Hello` | Session manager |
| Request ID | Echoed within the current session | Session manager |
| Payload length | ≤ configured max frame length | Frame reader, **pre-allocation** |
| File name | Bare filename only; length ≤ 255 bytes; no separators, `..`, drive letters, UNC, NUL, reserved device names | `FileMetadata` |
| File size | Plausibility + free-space check; never used to pre-allocate | `FileMetadata` |
| Chunk size | Must equal the negotiated size | `Chunk` |
| Chunk index | `< total_chunks`; no duplicates within a window | `Chunk` |
| Total chunks | Consistent with `file_size / chunk_size` | `FileMetadata` |
| Device name | Length limit; reject control **and** invisible-format characters | `Hello` |

> **The `Cf` class matters.** `char::is_control()` returns `false` for
> RIGHT-TO-LEFT OVERRIDE, ZERO WIDTH SPACE, and BOM. A name validator that checks
> only `is_control` accepts text that renders reversed or invisible - which is
> exactly how T-25 happened. The check is `is_disallowed_in_name`, not
> `is_control`.
| Capabilities | Validate claimed support before acting | Discovery |
| Timestamps | Never trusted for timeouts; monotonic clocks only | Timers |

---

## 6. Memory Safety (Charter §33)

Rust gives strong memory-safety guarantees. It does **not** make application
logic secure.

Avoid:

- Unnecessary `unsafe`
- Unchecked indexing
- Unbounded allocations
- Unbounded queues
- Unbounded recursion
- Uncontrolled parsing

Every `unsafe` block requires:

1. **Justification** — why safe Rust cannot do it.
2. **Documented invariant** — what makes it sound.
3. **Minimal scope.**
4. **Tests.**

`unsafe` is expected to be near-zero in the core. Platform FFI is the main
justifiable use.

---

## 7. Filesystem Security (Charter §34)

Filenames received from another device are **untrusted input**.

Never allow:

```text
../../secret.txt
C:\Windows\...
\\server\share\...
```

Required behaviour:

- Resolve the destination **inside a receiver-controlled directory**. The
  receiver chooses or controls the destination — never the sender.
- Sanitise to a bare filename, then verify the resolved path is still inside the
  destination root.
- Never blindly execute a received file.
- Never automatically open a received file.
- Never overwrite an existing file without an explicit policy.

---

## 8. Resource Limits (Charter §35, §36)

Every protocol endpoint has limits. Proposed defaults, all configurable — see
[`protocol.md` §5](./protocol.md) for the protocol-level table and
[`discovery.md`](./discovery.md) for the adapter surface.

Defence against remote-triggered resource exhaustion:

| Resource | Defence |
| --- | --- |
| Memory | Bounded buffers; bounded frame length; bounded queues; no pre-allocation from declared size |
| Disk | Pre-flight free-space check; max simultaneous transfers; reject if insufficient |
| CPU | Bounded concurrency; optional compression off by default |
| Tasks | Task-creation caps; no task per untrusted event without a bound |
| Connections | Max simultaneous sessions; connection-rate limiting |

A peer must never be able to force unbounded consumption of any of the above.

---

## 9. Storage Exhaustion (Charter §62)

Never assume the receiver has enough disk space.

```text
Insufficient space  →  reject transfer
```

Not: start the transfer and let it fail 90% of the way through. A predictable
failure detected at admission costs the user 5 seconds; the same failure detected
at 90% costs them an hour and a partial file.

---

## 10. Logging Security

Cross-referenced with [`security.md` §8](./security.md). The threat-model-specific
requirement:

> Logs should support debugging without becoming a secret database.

Log review is an explicit release-gate item.

---

## 11. Dependency Supply Chain (Charter §54, §55)

Before adding a dependency, answer:

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

Keep dependencies minimal. This is especially load-bearing for the core
protocol path: a vulnerable serialization or crypto crate is a remote
vulnerability.

---

## 12. Open Security Questions

| Question | Why it is open | Reference |
| --- | --- | --- |
| Handshake / record protocol | Noise vs TLS 1.3 — affects the whole framing design | [`open-questions.md` Q-7](./open-questions.md) |
| Hash algorithm | BLAKE3 (tree-structured, suits resume) vs SHA-256 (ubiquitous) | [`open-questions.md` Q-8](./open-questions.md) |
| Pairing verification UX | Determines what the user must actually read and compare | [`security.md` §6](./security.md) |
| Trust store model | TOFU, POUF, or user-managed allowlist | [`open-questions.md` Q-9](./open-questions.md) |

---

## 13. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §2, §6, §7, §8, §9, §10 | Charter §31, §16, §33, §34, §35, §36, §62, §38 |
| §4 | Charter §31–§38, §62, §64, §24, §25, §40, §65, §66, §67 |
| §5 | Charter §32, §35, §41 |
| §11 | Charter §54, §55 |