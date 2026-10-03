# Phase 2 Test Procedure — Two Machines

Status: **Ready to run.** Requires two Windows devices and ~15 minutes.

Spec: [`docs/roadmap.md` §6](roadmap.md)

---

## What this test proves

The **identical binaries** complete a version-negotiated session across two
physical machines over a real network, with no code change.

## What it does NOT prove

**This is not a Wi-Fi Direct test.** A Windows Mobile Hotspot is
infrastructure-mode access point plus DHCP. It exercises:

| Exercised | Not exercised |
| --- | --- |
| IP connectivity between two devices | Wi-Fi Direct peer **discovery** |
| TCP across a real network link | **Group formation** |
| Protocol framing over a real link | **Group-owner negotiation** |
| Latency on a real link | Direct-mode link characteristics |
| Loopback-only code paths | Anything the radio layer does |

Recording hotspot results as Wi-Fi Direct evidence would be a false claim. See
[`docs/platform-windows.md` §2](platform-windows.md).

## Why it is a gate

Phase 3 asserts *"the protocol layer must require no changes"*. That assertion is
only meaningful if the protocol already worked on a real network. Skip this and a
Phase 3 failure becomes ambiguous: was it the new adapter, or was the protocol
already broken across machines?

---

## Prerequisites

- [ ] **Two Windows devices.** Laptops, desktops, whatever.
- [ ] Both can reach the internet or have a copy of the repo — you will copy the
      built binaries across manually.
- [ ] On **both**: Windows Firewall allows `p2p.exe` on **private networks**.
      *(This is the single most common cause of failure. See Troubleshooting.)*
- [ ] Built binaries available on both (below).

---

## Step 1 — Build on your dev machine

```powershell
cd "C:\Software Engineering\Rust Programming\VaccumDrop"
cargo build --release
```

Copy to the second device:

```
target\release\p2p.exe
```

> Copy the `.exe`, not the target directory. It is statically self-contained apart
> from the MSVC runtime.

**Verify on BOTH devices:**

```powershell
p2p.exe version
```

Expected:

```text
p2p 0.1.0
protocol 1
phase 1 - handshake only, unencrypted and unauthenticated
```

If either device shows anything else, stop and fix it. A version mismatch here
will surface as a confusing handshake failure later.

---

## Step 2 — Set up the hotspot (Device A = receiver)

On **Device A** (the one running `listen`):

1. Settings → Network & internet → **Mobile hotspot** → **ON**
2. Note the **network name** and **password**
3. Note the gateway IP — usually `192.168.137.1`

> If Windows says the hotspot is unsupported, that device's Wi-Fi adapter lacks
> SoftAP capability. Use ordinary Wi-Fi instead (both devices on the same router)
> and set `Bind 0.0.0.0`. The test is identical; only the transport setup differs.

---

## Step 3 — Join from Device B

On **Device B**:

1. Settings → Network & internet → Wi-Fi → find the hotspot from Step 2
2. Join with the password
3. Run `ipconfig`
4. Note Device B's IPv4 — typically `192.168.137.x`

---

## Step 4 — Start the listener (Device A)

```powershell
.\p2p.exe listen --addr 0.0.0.0:9000 --name "Device-A"
```

Expected:

```text
listening on 0.0.0.0:9000
device name : Device-A
waiting for a peer...
```

> **Leave this window open.** The script form below does the same thing and adds a
> timeout plus diagnostics.

---

## Step 5 — Connect (Device B)

```powershell
.\p2p.exe connect 192.168.137.1:9000 --name "Device-B"
```

> Use **Device A's gateway IP**, not `127.0.0.1`. `127.0.0.1` means "this
> machine", which is the single most common mistake.

Expected on **both**:

```text
session established
  version     : v1
  peer        : Device-B        (on A)  /  Device-A  (on B)
  chunk size  : 8388608 bytes
```

**Phase 2 passes when both sides print `session established` with `v1`.**

---

## Scripted alternative

Instead of Steps 4–5 by hand, run the listener as a script. It adds a timeout and
prints diagnostics on failure:

**On Device A:**

```powershell
.\scripts\phase2-test.ps1 -Bind 0.0.0.0 -Port 9000 -DeviceName "Device-A"
```

**On Device B:**

```powershell
.\p2p.exe connect 192.168.137.1:9000 --name "Device-B"
```

The script prints `RESULT: PASS` on success and a diagnosis on failure.

---

## Step 6 — Record the result

Copy this into [`docs/performance.md` §5](performance.md) and send it back
to me so I can update the compatibility matrix and mark the gate.

```text
Date              : <date>
Link type         : <hotspot / Wi-Fi LAN>
Negotiated rate   : <from ipconfig, or netsh wlan show interfaces>
Distance          : <same room / next room / through a wall>
Device A          : <model>, <OS build>
Device B          : <model>, <OS build>
Command (A)       : p2p listen --addr 0.0.0.0:9000 --name "Device-A"
Command (B)       : p2p connect 192.168.137.1:9000 --name "Device-B"
Result            : <both sides reached v1, or the exact failure>
```

---

## Troubleshooting

Work through these in order. The first three cause the large majority of failures.

### 1. Windows Firewall drops the inbound connection *(most common)*

Symptom: client reports `actively refused` (os error 10061) or a timeout.

The listener is running; the *packet* is being discarded. On Device A, allow it:

- Windows Security → Firewall & network protection → **Allow an app through
  firewall** → *Change settings* → tick **Private** for `p2p`
- Or from an **administrator** PowerShell on Device A:

```powershell
New-NetFirewallRule -DisplayName "p2p Phase2" -Direction Inbound `
  -Program "$PWD\p2p.exe" -Action Allow -Profile Private
```

### 2. Wrong address

`127.0.0.1` on Device B points at Device B. Use Device A's IP.

```powershell
ipconfig
```

### 3. Different subnets

Both devices must be on the same subnet. If `ipconfig` shows different
`192.168.137.x` / `192.168.x.x` ranges, they are not on the same network.

### 4. Port already in use

```powershell
Get-NetTCPConnection -LocalPort 9000 -ErrorAction SilentlyContinue
```

Try another: `-Port 9001` on A and `:9001` on B.

### 5. `bind` defaulted to loopback

`p2p listen` with no `--addr` binds `127.0.0.1` **on purpose**, so an
unauthenticated protocol is never exposed to the network by accident. For a
cross-machine test you must pass `--addr 0.0.0.0` explicitly.

### 6. Version mismatch

If one side is stale, negotiation fails closed with
`UnsupportedProtocolVersion` — by design, never a silent downgrade. Confirm both
report the same `protocol 1` in `p2p version`.

---

## What Phase 3 still has to prove

Passing this test is necessary and not sufficient. Phase 3 must additionally show:

1. A Wi-Fi Direct link is established **without** a router or hotspot.
2. Group formation succeeds.
3. `p2p connect` works over the P2P link **unchanged** — zero protocol edits.
4. If (3) fails, the transport abstraction leaked radio concepts, and that is a
   defect in [ADR-0004](adr/0004-transport-abstraction.md) to fix — not a
   protocol to patch.