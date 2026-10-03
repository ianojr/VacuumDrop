# Troubleshooting & Failure Philosophy

Status: **Living document.** Extended as each phase lands.

---

## 1. Failure Philosophy

> Failures are normal.

The system must fail:

```text
predictably · safely · recoverably · observably
```

It must **never**:

```text
panic silently · corrupt files · lose transfer state
leak secrets · hang indefinitely
```

---

## 2. Expected Failure Modes

Every mode below has a **defined state transition** out of
[`transport.md` §7](./transport.md).

| # | Failure | Expected behaviour |
| --- | --- | --- |
| F-01 | Peer moves out of range | Transfer aborts to `PeerDisconnected`; state durable for resume |
| F-02 | Wi-Fi disabled | Disconnect event; resumable |
| F-03 | Device sleeps | Session timeout; resumable |
| F-04 | OS kills the process | On restart, transfer state recovered from durable journal |
| F-05 | Environment / interface changes | Connection invalidated; rediscovery required |
| F-06 | Radio interference | Throughput degrades or connection drops; resumable |
| F-07 | Network interface disappears | Transport error, mapped to `PeerDisconnected` |
| F-08 | Receiver crashes | Sender observes disconnect; resumable |
| F-09 | Sender crashes | Receiver retains partial state; resumable |
| F-10 | Disk becomes unavailable | `StorageError`; clean abort, temp file cleanup attempted |
| F-11 | Disk full | Rejected at admission if detectable; otherwise clean abort mid-transfer |
| F-12 | Receiver reboot mid-transfer | Receiver reloads journal; resume offered |

**None of these may produce a corrupted "completed" file.** See
[`file-transfer.md` §6](./file-transfer.md).

---

## 3. Symptom Index

| Symptom | Likely cause | Where to look |
| --- | --- | --- |
| **`10013` / `WSAEACCES` on any LAN connection** | **VPN filter driver. Check this first** | §3.1 |
| `10061` actively refused | Nothing listening; wrong port, or the listener died | [`phase2-test.md`](./phase2-test.md) |
| Timeout, no error | Firewall dropping inbound on the listener | [`phase2-test.md`](./phase2-test.md) |
| Window flashes and closes | No command given — `p2p` needs a subcommand | `p2p help` |
| Discovery always returns empty | Adapter lacks P2P support, or permission denied | [`discovery.md` §4](./discovery.md), [`platform-android.md` §3](./platform-android.md) |
| Discovery returns empty on Linux | Driver lacks P2P; wpa_supplicant without `CONFIG_P2P` | [`platform-linux.md` §4](./platform-linux.md) |
| Connect hangs | Missing timeout on connection establishment | [`transport.md` §6](./transport.md) |
| `UnsupportedProtocolVersion` | Version skew between builds | [`protocol.md` §6](./protocol.md) |
| Throughput far below Phase-0 baseline | Framing/encryption overhead, or disk is the bottleneck | [`performance.md` §6](./performance.md) |
| RSS grows with file size | Streaming property broken — **stop and fix** | [`file-transfer.md` §11](./file-transfer.md) |
| Resume restarts from zero | Transfer journal not durable, or not committed | [`file-transfer.md` §4](./file-transfer.md), T-21 |
| Received file has wrong name / wrong path | Filename sanitisation failure | T-03, T-04 |
| Transfer hangs near completion | Missing or too-long final ack / verify timeout | [`protocol.md` §2](./protocol.md) |
| Peer authenticates then is untrusted | Working as intended — auth ≠ trust | [`security.md` §3](./security.md) |
| Locked out after failed pairing attempts | Rate limiting; cooldown is time-based | [`security.md` §7](./security.md) |

### 3.1 `10013` / `WSAEACCES` on LAN connections — VPN interference

**Observed during Phase 2.** It reads like a permissions problem, which sends you
to firewall rules instead of the real cause.

```text
error: transport failed: i/o failed: An attempt was made to access a socket
in a way forbidden by its access permissions. (os error 10013)
```

Signature: **every** TCP port fails and `ping` fails too, while ARP resolves
normally. A firewall rule would not produce that combination.

Cause in the observed case: `ProtonVPNCallout`, a Windows Filtering Platform
callout driver in the network stack. It permitted DNS and web traffic but refused
the LAN connection.

```powershell
# Look for a callout driver
Get-CimInstance Win32_SystemDriver |
  Where-Object { $_.State -eq 'Running' -and $_.PathName -match 'proton|wireguard|tap|vpn' } |
  Select-Object Name, PathName

# Disable the tunnel on BOTH devices for the test
Disable-NetAdapter -Name "ProTUN" -Confirm:$false

# Restore afterwards
Enable-NetAdapter -Name "ProTUN" -Confirm:$false
```

**Confirm the OS is at fault before changing any code.** A plain .NET socket
reproducing the same error proves no code change would help:

```powershell
$c = New-Object System.Net.Sockets.TcpClient
try { $c.Connect("192.168.137.1", 9000) } catch { $_.Exception.Message }
```

Any VPN can do this, not only Proton. Ruled out along the way: client IP and
default route correct, ARP resolving, Windows Firewall block rules scoped to
loopback only, profiles enabled with no default overrides.

---

## 4. Diagnostic Principles

1. **Never delete a test to make a failure go away** (Rule 9).
2. **Never disable a security check to make a feature work** (Rule 10).
3. Correlate logs by `session_id`, `transfer_id`, `request_id`
   ([`security.md` §8](./security.md)).
4. Reproduce against the nearest lower rung of the ladder in
   [`roadmap.md` §1](./roadmap.md) before debugging a higher one. If it fails on
   localhost, the platform is not your problem.
5. Check the Phase-0 baseline before suspecting the protocol.
6. **Reproduce outside the project before blaming the project.** A one-line .NET
   or `curl` reproduction separates "our bug" from "the environment's". Phase 2
   cost real time because 10013 looks like an application permissions error and
   turned out to be a VPN driver; a .NET socket would have settled it in seconds.
7. **Check for VPN interference before firewall rules** on any network error that
   is not a clean refusal (§3.1).

---

## 5. Reporting a Bug

Include:

- VacuumDrop revision and toolchain version
- Platform, OS build, Wi-Fi adapter model
- Link type (Wi-Fi Direct / Wi-Fi LAN / localhost)
- Rung on the progression ladder where it reproduces
- Phase-0 baseline for comparison, if available
- Full logs for the session, **redacted per [`security.md` §8](./security.md)**
- Whether it is reproducible, and at what rate

---

## 6. Source Traceability

| Section | Origin |
| --- | --- |
| §1 | Charter §95 |
| §2 | Charter §42, §62, §41 |
| §3, §4 | Derived from Charter §83, §38, §98 |
| §5 | Charter §84 |