param(
    # Address the LISTENER binds on. Bind 0.0.0.0 to accept from the other device.
    [string]$Bind = "0.0.0.0",
    [int]$Port = 9000,
    [string]$DeviceName = "",
    # Seconds to wait for a peer before giving up.
    [int]$WaitSeconds = 60,
    # Skip the release build if the binaries are already present.
    [switch]$NoBuild
)

# Phase 2 test runner: proves the SAME binaries work across two machines.
#
# Spec: docs/roadmap.md section 6
#
# What this proves and what it does not:
#
#   PROVES    identical code completes a version-negotiated session across two
#             physical machines over a real network.
#   DOES NOT  exercise Wi-Fi Direct. A mobile hotspot is infrastructure-mode AP
#             plus DHCP: no peer discovery, no group formation, no group-owner
#             negotiation. Results are a Phase-2 baseline ONLY and must never be
#             recorded as Wi-Fi Direct evidence. See docs/platform-windows.md
#             section 2.
#
# Why it matters: Phase 3 claims "the protocol layer requires no changes". That
# claim is only meaningful if the protocol already worked on a real network. If a
# Phase-3 failure occurs without this baseline, it is ambiguous whether the
# adapter or the protocol is at fault.

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$releaseDir = Join-Path $root "target\release"
$p2p = Join-Path $releaseDir "p2p.exe"

if (-not $NoBuild) {
    Write-Host "Building release binaries..."
    Push-Location $root
    try {
        & cargo build --release --quiet
        if ($LASTEXITCODE -ne 0) { throw "build failed" }
    }
    finally { Pop-Location }
}

if (-not (Test-Path -LiteralPath $p2p)) {
    throw "p2p.exe not found at $p2p. Run without -NoBuild."
}

if ([string]::IsNullOrWhiteSpace($DeviceName)) {
    $DeviceName = [System.Environment]::MachineName
}

# ---- Preflight: the things that make this test lie if unchecked ---------------

Write-Host ""
Write-Host "=== preflight ==="
Write-Host "device name : $DeviceName"
Write-Host "bind        : $Bind`:$Port"
Write-Host "waiting up to: $WaitSeconds s"

# Firewall is the usual cause of "it works on loopback but not across machines".
# So prove the binary works at all before blaming the network: run `p2p version`,
# which must exit immediately and print its build. A listen-and-wait probe cannot
# distinguish "did not start" from "started and is correctly waiting", so it is
# useless as a preflight check.
$versionOut = & $p2p version 2>&1 | Out-String
if ($LASTEXITCODE -ne 0 -or $versionOut -notmatch "phase 1") {
    Write-Host "version output:"
    Write-Host $versionOut
    throw "p2p.exe is not runnable; preflight failed before touching the network"
}
Write-Host "local binary: OK"

# ---- Run the listener -------------------------------------------------------

Write-Host ""
Write-Host "=== listener ==="
Write-Host "Run this on the DEVICE THAT RECEIVES the connection."
Write-Host "Leave it running, then start the client on the other device."
Write-Host ""

$log = Join-Path $env:TEMP "vd-phase2-listener.log"
$err = Join-Path $env:TEMP "vd-phase2-listener.err"
Remove-Item $log, $err -Force -ErrorAction SilentlyContinue

$listener = Start-Process -FilePath $p2p `
    -ArgumentList @("listen", "--addr", "$Bind`:$Port", "--name", $DeviceName) `
    -PassThru -NoNewWindow -RedirectStandardOutput $log -RedirectStandardError $err

Write-Host "listener pid : $($listener.Id)"
Write-Host "waiting for a peer for up to $WaitSeconds s..."
Write-Host ""

$deadline = (Get-Date).AddSeconds($WaitSeconds)
$session = $null

while ((Get-Date) -lt $deadline -and -not $listener.HasExited) {
    Start-Sleep -Milliseconds 400
    if (Test-Path -LiteralPath $log) {
        $text = Get-Content -LiteralPath $log -Raw -ErrorAction SilentlyContinue
        if ($text -match "session established") {
            $session = $text
            break
        }
    }
}

Write-Host "=== listener transcript ==="
if (Test-Path -LiteralPath $log) {
    Get-Content -LiteralPath $log | ForEach-Object { "  $_" }
}
if ((Test-Path -LiteralPath $err) -and (Get-Item -LiteralPath $err).Length -gt 0) {
    Write-Host "  --- stderr ---"
    Get-Content -LiteralPath $err | ForEach-Object { "  $_" }
}

if ($null -eq $session) {
    Write-Host ""
    Write-Host "RESULT: no session was established."
    Write-Host ""
    Write-Host "Diagnose in this order:"
    Write-Host "  1. Is the client pointed at the right address? Use the listener's"
    Write-Host "     IPv4 on the hotspot (usually 192.168.137.1), NOT 127.0.0.1."
    Write-Host "  2. Windows Firewall - allow p2p.exe on private networks, or the"
    Write-Host "     inbound connection is silently dropped. This is the usual cause."
    Write-Host "  3. Is the client on the same subnet? Check with:"
    Write-Host "       ipconfig"
    Write-Host "  4. Is the port already in use? Try -Port 9001."
    if (-not $listener.HasExited) { $listener | Stop-Process -Force }
    exit 1
}

Write-Host ""
Write-Host "RESULT: PASS"
Write-Host ""
Write-Host "A session was negotiated across the network with no code change."
Write-Host "Record this in docs/performance.md section 5, alongside:"
Write-Host "  - the link type (hotspot / Wi-Fi LAN) and negotiated rate"
Write-Host "  - the client device and OS build"
Write-Host "  - the exact command line from each side"
Write-Host ""
Write-Host "Reminder: this is a Phase-2 baseline. It does NOT validate Wi-Fi"
Write-Host "Direct discovery, group formation, or group-owner negotiation."
Write-Host "Those are Phase 3 and remain untested."

exit 0