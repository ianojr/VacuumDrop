# ADR-0005 — Cryptographic Device Identity

**Status:** Accepted
**Phase:** 4 (design ratified now)

---

## Context

The engine must let two nearby devices establish **who they are**. Without a
durable identity, an attacker can trivially impersonate a device, and the
attacker needs only to be nearby.

The Charter identifies non-identity and forbids them:

| Attribute | Why invalid |
| --- | --- |
| MAC address | Spoofable; randomised per-network on modern OSes |
| IP address | Reassigned on every network change |
| Hostname | User-controlled; trivially duplicated |

## Decision

Every installation holds a **persistent cryptographic identity** generated on
first run:

```text
Device
 ├── Device ID          stable, derived from the public key
 ├── Public Key
 ├── Private Key        never transmitted, never logged, never committed
 ├── Display Name       user-controlled, untrusted
 └── Capabilities       declared, untrusted
```

A **connection does not imply trust**. Trust is established by explicit
authentication and user-confirmed pairing.

Key agreement, signatures, and encryption come from **established cryptographic
libraries**. No custom primitives. See
[`../security.md` §3.1](../security.md) for the open protocol-selection question
([`../open-questions.md` Q-7](../open-questions.md)).

## Alternatives Considered

| Alternative | Why rejected |
| --- | --- |
| MAC address as identity | Spoofable and frequently randomised; non-durable |
| Fingerprint + human comparison only | Not an identity — cannot authenticate, cannot sign, and does not scale past a few devices |
| Pre-shared pairing code | Acceptable as a *bootstrap* mechanism, not as a durable identity |
| X.509 certificates | Requires a CA or a bespoke trust story; disproportionate for local P2P |
| TOFU (trust on first use) | Reasonable default for automation, but insufficient alone — an attacker present at first contact wins permanently. Must be paired with explicit user confirmation. |

## Consequences

**Positive**

- Impersonation requires stealing a private key, not being nearby.
- Identity survives network changes, reboots, and interface changes.
- Enables cryptographic authentication and non-repudiation of transfer
  provenance.
- Stable across all platforms by construction.

**Negative**

- Requires secure key storage on every platform (DPAPI / Android Keystore /
  libsecret) behind a trait, so the core stays portable.
- Key loss means re-pairing — an operational cost users will notice; the UX must
  handle it explicitly.
- Trust management becomes a real product surface (pairing, revocation,
  re-pairing) rather than a boolean.

## Security Requirements

- Private keys are never committed, logged, sent, or hard-coded.
- Private keys live in platform secure storage when productionised.
- Display names and capabilities are **untrusted claims**, length-limited and
  stripped of control characters before display.
- Loss or corruption of the identity must produce a clear, recoverable state —
  never a silently re-identified device.

## Related

[`../security.md`](../security.md), [`../threat-model.md` §2](../threat-model.md),
[ADR-0001 — Rust core](./0001-rust-core.md)