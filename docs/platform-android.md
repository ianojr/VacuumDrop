# Android Platform Adapter

Status: **Phase 6 design.** Not yet implemented.

---

## 1. Responsibilities

The Android adapter is responsible for:

- Checking Wi-Fi P2P support
- Requesting required permissions
- Peer discovery
- Connection management
- Retrieving connection information
- Exposing the resulting connection to the Rust layer

**Android's platform APIs remain outside the Rust core.** The Rust layer must not
attempt to recreate Android's Wi-Fi P2P implementation.

---

## 2. Platform API

`WifiP2pManager` and its peers:

| Type | Role |
| --- | --- |
| `WifiP2pManager` | Entry point; obtained from `Context.WIFI_P2P_SERVICE` |
| `WifiP2pManager.Channel` | `prefetch` + `register` + `discoverPeers` → `Intent` |
| `WifiP2pDeviceListWatcher` | Peer discovery results |
| `WifiP2pDevice` | Peer: name, address, `wifiP2pDeviceList` capabilities |
| `WifiP2pConfig` | Connection intent: groupOwnerIntent, `wps` (nfc/pushbutton/pin), owner address |
| `WifiP2pInfo` | Post-connect info: **group owner address, isGroupOwner, groupOwner** |
| `BroadcastReceiver` | Receives discovery / connection / IP-assignment broadcasts |

### 2.1 Getting the address

`WifiP2pInfo.groupOwnerAddress` yields the group-owner IP. This is the value the
adapter hands to the Rust core as `ConnectionInfo.ip`.

Do not attempt to parse network-interface state to discover it — the framework
already tells you.

---

## 3. Permissions

| Permission | Why |
| --- | --- |
| `INTERNET` | Sockets |
| `ACCESS_WIFI_STATE` | Query Wi-Fi/P2P state |
| `CHANGE_WIFI_STATE` | Toggle / enable P2P |
| `ACCESS_NETWORK_STATE` | Interface state |
| `ACCESS_FINE_LOCATION` | **Required** for peer discovery up to API 32 |
| `NEARBY_WIFI_DEVICES` | **Required** from API 33 for nearby-device operations |

> The location-permission requirement is the most common cause of "discovery
> silently returns nothing" on Android. It is a privacy-visible permission, so
> request it with clear justification — and never quietly degrade when denied.

---

## 4. Architecture

```text
Android UI (Kotlin)
       │
       ▼
Android Wi-Fi Direct Adapter  ──►  normalized values  ──►  Rust Core
   (WifiP2pManager)
```

**Kotlin/Java stays as thin as possible.** It must not contain business logic,
and that logic must not be duplicated across platforms. The Kotlin layer:

1. Owns the `BroadcastReceiver` lifecycle.
2. Translates Android intents/callbacks into normalized Rust values.
3. Exposes a small, stable surface to Rust over JNI.

All protocol, chunking, hashing, and session state lives in Rust.

### 4.1 Rust integration

Options:

| Option | Notes |
| --- | --- |
| Rust `cdylib` + Kotlin façade (`uniffi`/`diem`) | Most ergonomic; adds codegen |
| `jni` crate, hand-written bindings | Fewer moving parts; more boilerplate |
| Rust `cdylib` loaded via `System.loadLibrary` | Minimal; no binding layer, only primitives |

Recommended: a `cdylib` exposing **primitives and normalized structs only** —
never JNI objects that mirror Android classes. Same boundary discipline as
Windows; see [`architecture.md` §7](./architecture.md).

---

## 5. Capability Detection

Expose, per Charter §6:

```text
is_supported()   # WifiP2pManager present + feature available
is_enabled()     # Wi-Fi + P2P enabled, not airplane mode, not in a restricted state
```

Android-specific failure modes to detect explicitly:

- Device manifest missing a P2P feature (`uses-feature`).
- Airplane mode.
- Battery saver / Doze restrictions on scanning.
- Permission denied.
- Location services disabled at the OS level (separate from permission).

---

## 6. Android ↔ Wi-Fi Aware

> Android increasingly favours **Wi-Fi Aware / NAN** over classic Wi-Fi Direct.

Wi-Fi Aware is a **different transport** with different discovery semantics. It
is explicitly a *future adapter* per
[`architecture.md` §2](./architecture.md), not a Phase-6 requirement.

Phase 6 targets classic `WifiP2pManager`. Confirm that the abstraction introduced
in [`discovery.md` §2](./discovery.md) genuinely permits adding Wi-Fi Aware
later without changing the core; if it does not, fix it in Phase 3.

---

## 7. Scope Boundary

Per Charter §80, Phase 6 delivers:

```text
Android UI  →  Android Wi-Fi Direct Adapter  →  Rust Core
```

It does **not** include: a polished UI, background transfers, or a Wi-Fi Aware
transport. Android-specific logic stays out of the Rust domain layer.

---

## 8. Source Traceability

| Section | Origin |
| --- | --- |
| §1, §2, §7 | Charter §46 (Android), §80 |
| §4 | Charter §3.1, §47 |
| §5 | Charter §6 |
| §6 | Charter §8 (future adapters), §102 |