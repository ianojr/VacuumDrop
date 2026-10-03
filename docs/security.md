# Security Model

Status: **Design intent.** Phase 4. Threat analysis lives in
[`threat-model.md`](./threat-model.md); this document covers identity,
authentication, encryption, authorization, and secret handling.

---

## 1. Governing Principle

> **A connection does not imply trust.**

TCP succeeding, a socket opening, a device appearing in a list — none of these
establish identity. Trust is established by an explicit, authenticated,
user-visible process.

The one hard rule from the Charter that overrides convenience everywhere:

> **Do not invent cryptography.**

No custom ciphers. No custom key exchange. No custom MACs. No custom signatures.
No custom password hashing. This is a networking project, not a cryptography
research project. Primitives come from established, audited libraries.

---

## 2. Device Identity (Charter §16)

Every installation holds a persistent cryptographic identity:

```text
Device
 ├── Device ID          (stable identifier derived from the public key)
 ├── Public Key
 ├── Private Key        (never leaves the device, never logged)
 ├── Display Name       (user-controlled, untrusted)
 └── Capabilities       (declared, untrusted)
```

### 2.1 Never use as identity

| Attribute | Why it is invalid |
| --- | --- |
| MAC address | Spoofable, frequently randomised per-network |
| IP address | Reassigned on every network change |
| Hostname | User-controlled, trivially duplicated |

These are **network attributes**, not durable cryptographic identities. They may
be used as *discovery hints*; they must never be used as *identity*.

### 2.2 Identity storage

Private keys are protected:

- Never committed to Git.
- Never logged.
- Never sent over the wire.
- Never hard-coded.
- Productionized: platform secure storage (DPAPI / Android Keystore / libsecret)
  behind a trait, so the core stays portable.

See [`open-questions.md` Q-9](./open-questions.md) for the persistence backend
decision.

---

## 3. Authentication (Charter §17)

Authentication is a distinct protocol step, not a side effect of connecting.

```text
Device A
   │  connection
   ▼
Device B
   │  exchange identity information
   ▼
Authentication
   │
   ▼
Trusted  /  Untrusted
```

The cryptographic protocol **MUST** use established primitives and audited
libraries.

### 3.1 Selection is deferred — and deliberately

Candidate approaches, none yet ratified:

| Option | Strengths | Costs |
| --- | --- | --- |
| **Noise Protocol Framework** (`snow`) | Purpose-built for exactly this; handshake + record layer; no certificate authority; ~small crate | Requires discipline on message ordering and nonce handling |
| **TLS 1.3** (`rustls`) | Battle-tested, ubiquitous tooling, strong AEAD | Double framing — TLS needs its own record layer *inside* our frames; certificate story needs a design |
| **Application-layer AEAD** | Simple, explicit | Key agreement still needs a handshake protocol; re-implementing the easy part of Noise badly is worse than using Noise |

**Not yet decided.** Recorded as [`open-questions.md` Q-7](./open-questions.md);
will become an ADR before Phase 4 begins. This is not an oversight — the
Charter's whole posture is that premature commitment here is expensive to undo.

### 3.2 Authentication is not authorization

See §5 below. Keeping these separate is a hard architectural requirement.

---

## 4. Encryption (Charter §18)

All application-level sensitive data is encrypted in transit.

**Wi-Fi Direct's own security is explicitly not assumed sufficient.** It is a
radio-level mechanism with a poor history of implementation quality, and it
protects link frames — not a compromised endpoint.

The implementation must protect:

- **Confidentiality**
- **Integrity**
- **Authenticity**
- **Replay resistance** where relevant

### 4.1 Never implement

```text
my_own_encryption_algorithm()
```

No custom cipher, key exchange, MAC, signature, or password hashing. See
[ADR-0005](./adr/0005-cryptographic-identity.md).

---

## 5. Authorization (Charter §20)

| Question | Answered by |
| --- | --- |
| Who are you? | Authentication |
| What are you allowed to do? | Authorization |

These are separate mechanisms. A successfully authenticated peer is still
**untrusted by default** and holds only the permissions granted to it.

Proposed permission set:

```text
DISCOVER · CONNECT · SEND · RECEIVE
REQUEST_FILE · ACCEPT_FILE · MESSAGE · STREAM · ADMIN
```

A future application may define more granular permissions. The set is
data, not hard-coded control flow — model it as an explicit permission type so
that authorization decisions are inspectable and testable.

---

## 6. Pairing UX (Charter §19)

The first user experience is **explicit pairing**. An unknown nearby device must
not be able to silently become trusted.

```text
Nearby Device

Ian's Laptop

Fingerprint:

A8:42:91:...

        [Pair]
```

Candidate verification methods:

| Method | Notes |
| --- | --- |
| Numeric comparison | Familiar from Bluetooth; short PINs are brute-forceable, so pair with rate limiting |
| Short authentication string (SAS) | Strong; requires both humans; best fit for later phases |
| QR code | Strong and fast; needs a camera or manual transfer |
| Explicit trust confirmation | Weakest; acceptable only for low-risk actions |

Whatever is chosen, the security design must make silent trust escalation
**difficult**, not merely unlikely.

---

## 7. Rate Limiting Authentication (Charter §37)

Repeated authentication failures are controlled:

```text
failure 1
failure 2
failure 3
   ...
temporary cooldown
```

**Balance matters.** Aggressive lockouts are themselves a denial-of-service
vector: an attacker nearby can lock a legitimate user out by failing
authentication on their behalf.

Therefore:

- Cooldown is **time-based and bounded**, not permanent.
- Cooldown applies **per peer identity**, and is not a global lockout.
- Lockout state is not persisted across restarts by default, to avoid
  weaponisable persistent denial.

---

## 8. Log Hygiene (Charter §38)

### Never log

- Private keys
- Session secrets
- Encryption keys
- Passwords
- Raw authentication tokens

### Treat with care

- Filenames
- IP addresses
- Device identifiers
- User metadata

Logs must support debugging without becoming a secret database.

Implementation rule: derive `Display` for secret-holding types that
**redacts by default**, so accidental logging leaks nothing. Make the safe path
the default path.

---

## 9. Data at Rest (Charter §57)

The MVP may store received files normally. Future versions should consider:

- Encrypted transfer metadata
- Encrypted local databases
- Protected private keys
- Secure deletion policies

> **Do not promise secure deletion** unless the platform's storage semantics
> actually deliver it. On SSDs with wear levelling and TRIM — and on Android
> with `fscrypt` — overwriting a block is not a guarantee. Document the real
> behaviour per platform, or do not claim the capability.

---

## 10. Privacy (Charter §58)

The application is privacy-first. **No analytics by default.**

Never sent to remote servers:

- Device information · transfer information · files · identifiers · usage
  statistics

If telemetry is ever added it must be **opt-in**, **documented**, **minimal**,
and **privacy-conscious**.

This is reinforced by the offline-first requirement: there is no server to send
anything to, which makes the privacy property structural rather than a policy
promise. See §11.

---

## 11. Offline-First Security Implications

Core functionality must never require a cloud server for:

```text
discovery · pairing · authentication · file transfer · local messaging
```

No cloud authentication server. No cloud database. No cloud relay. No cloud file
storage.

### 11.1 The Internet is not the network

Do not equate:

```text
Internet unavailable
```

with

```text
network unavailable
```

`Internet = OFF, Wi-Fi Direct = ON, P2P = WORKING` is a **valid and normal
state** — arguably the target state. Any code that infers "no network" from
failed internet reachability is broken by design.

The application must also not accidentally depend on DNS, cloud APIs, external
authentication, remote telemetry, or external time services for core P2P
communication. Time in particular: protocol timeouts must use monotonic clocks,
not wall-clock or NTP-derived time.

---

## 12. Security Review Checklist

Run before considering any release.

```text
[ ] Authentication implemented
[ ] Encryption implemented
[ ] Private keys protected
[ ] Input validation reviewed
[ ] Protocol parser fuzzed
[ ] File paths sanitized
[ ] Resource limits implemented
[ ] Rate limiting considered
[ ] Timeouts implemented
[ ] Unbounded allocations reviewed
[ ] Unbounded queues reviewed
[ ] Unsafe Rust reviewed
[ ] Dependencies reviewed
[ ] Logs reviewed for secrets
[ ] Temporary files handled securely
[ ] Transfer integrity verified
```

---

## 13. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §2, §3, §4, §5, §6, §7 | Charter §17, §18, §19, §20, §37 |
| §2 | Charter §16 |
| §2.2, §8, §9, §10 | Charter §38, §56, §57, §58 |
| §11, §11.1 | Charter §43, §44, §45 |
| §12 | Charter §92 |