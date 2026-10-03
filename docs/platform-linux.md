# Linux Platform Adapter

Status: **Phase 7 design — EXPERIMENTAL.** Not yet implemented.

---

## 1. Status

The Linux adapter is **initially experimental**.

> Do not assume every Linux distribution exposes identical Wi-Fi P2P behaviour.

Detect capabilities. Document distribution and driver dependencies. A Linux
platform entry is not added to the compatibility matrix in
[`performance.md` §8](./performance.md) until it has been tested on a named
distribution with a named driver.

---

## 2. Platform API

Classic Wi-Fi P2P on Linux is provided by **wpa_supplicant** with `CONFIG_P2P`,
programmatically via its D-Bus control interface `fi.w1.p2p`:

| Operation | Mechanism |
| --- | --- |
| Capability check | Presence of the P2P D-Bus interface; `p2p_capability` |
| Peer discovery | `p2p_find_peers`, async via `p2p_peer_found` signals |
| Connect | `p2p_connect` with a peer address and `p2p_connect intent` |
| Disconnect | `p2p_disconnect`, `p2p_group_remove` |
| Result | `p2p_group_formed` signal |
| Address | `P2PGetInterfaceAddr` on the resulting group interface |

Alternative: NetworkManager's `wifi.p2p` support, which wraps wpa_supplicant.
Choosing between "direct D-Bus to wpa_supplicant" and "go through NetworkManager"
is an open question (below).

---

## 3. Responsibilities

Per Charter §46 and §6:

- Detect Wi-Fi P2P support and report it honestly.
- Perform discovery, connection management.
- Retrieve connection information and return normalized values.
- **Never leak wpa_supplicant D-Bus or NetworkManager objects into the core.**

---

## 4. Known Difficulties

These are expected, and should be treated as known limitations rather than
surprises during Phase 7.

| Issue | Detail |
| --- | --- |
| Driver support | P2P support is highly driver- and firmware-dependent. Atheros `ath9k`/`ath10k` are generally good; Broadcom and Realtek support is frequently absent or broken. |
| Dual-interface requirement | Many drivers need a dedicated P2P-capable interface |
| Hostapd vs wpa_supplicant | P2P group formation interacts with the AP daemon; concurrent operation often needs configuration |
| Privilege | Requires elevated privileges (`CAP_NET_ADMIN` / root) for interface manipulation |
| Distribution variance | Daemon names, service layout, D-Bus policy, and default configs differ across Debian/Ubuntu, Fedora, Arch |
| Persistence mode | wpa_supplicant may be managed by NetworkManager, which does not expose raw P2P control |
| Regulatory domain | P2P channel availability depends on the configured regulatory domain |

Consequence: the adapter must **probe** rather than assume, and report precise
reasons for failure. "Wi-Fi Direct unavailable" is not an acceptable error when
the real cause is "driver lacks P2P support".

---

## 5. Architecture

```text
Rust core
    │
    ▼
Transport abstraction (unchanged)
    │
    ▼
Linux adapter  ──D-Bus──►  wpa_supplicant / NetworkManager
    │
    ▼
Normalized: IpAddr + port
```

Identical boundary discipline to every other platform; see
[`architecture.md` §7](./architecture.md).

---

## 6. Scope Boundary

Per Charter §81, Phase 7 delivers:

- A Linux adapter for Wi-Fi P2P.
- Documentation of supported distributions, drivers, limitations, and required
  system components.

It does not deliver: distribution-agnostic support, parity with Windows, or a
polished UX. Experimental means experimental.

---

## 7. Open Questions

| Question | Why it matters |
| --- | --- |
| Direct wpa_supplicant D-Bus vs NetworkManager | NetworkManager is the default on desktop Linux but hides P2P control; direct D-Bus is precise but bypasses system conventions |
| Privilege model | Whether the daemon is expected to run privileged, or an unprivileged helper is needed |
| Discovery strategy | `p2p_find_peers` vs persistent `p2p_listen` vs social-formation channels |
| Target distributions | Which two or three to validate against first |

---

## 8. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §6 | Charter §46 (Linux), §81 |
| §3 | Charter §46 |
| §5 | Charter §47 |