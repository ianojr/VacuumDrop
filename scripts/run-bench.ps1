param(
    [string]$Address = "127.0.0.1",
    [int]$Port = 9000,
    [string]$SourceFile = ".\target\bench-source.bin",
    [string]$DestFile = ".\target\bench-dest.bin",
    [double]$SizeGiB = 1,
    # Comma-separated, e.g. -BufferSizes "4MiB,8MiB,16MiB".
    # A plain string rather than [string[]] because PowerShell collapses array
    # arguments into one token when a script is invoked via -File, which would
    # pass the literal "4MiB,8MiB,16MiB" through as a single buffer size.
    [string]$BufferSizes = "8MiB",
    [switch]$KeepFiles
)

# Phase 0 benchmark runner.
# Spec: docs/phase1-benchmark.md
#
# Orchestrates a sender/receiver pair and samples PEAK WORKING SET for both
# processes. The binaries deliberately report only throughput and elapsed time;
# measuring RSS from outside keeps the crate dependency-minimal (tokio only, no
# unsafe FFI for GetProcessMemoryInfo) and keeps the measured path clean.
#
# The number this script exists to produce: peak RSS for a large transfer versus
# peak RSS for a small one. If the large one is materially higher, the streaming
# property is broken.

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$release = Join-Path $root "target\release"
$senderBin = Join-Path $release "sender.exe"
$receiverBin = Join-Path $release "receiver.exe"

if (-not (Test-Path -LiteralPath $senderBin) -or -not (Test-Path -LiteralPath $receiverBin)) {
    Write-Error "Binaries not found. Run: cargo build --release"
    exit 1
}

$sourceBytes = [uint64]($SizeGiB * 1GB)
if ($sourceBytes -le 0) { Write-Error "SizeGiB must be > 0"; exit 1 }

function New-SourceFile {
    param([string]$Path, [uint64]$Bytes)

    $parent = Split-Path -Parent $Path
    if ($parent -and -not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }

    if ((Test-Path -LiteralPath $Path) -and ((Get-Item -LiteralPath $Path).Length -eq $Bytes)) {
        Write-Host "source: reusing $Path ($Bytes bytes)"
        return
    }

    # SetLength on a new file creates a sparse file instantly. That is fine for
    # a throughput ceiling: the bytes still cross the socket and still land on
    # disk. It does mean the SOURCE read path is served largely from cache, so
    # treat these numbers as a network ceiling, not a disk-to-disk result.
    $fs = [System.IO.File]::Create($Path)
    try {
        $fs.SetLength($Bytes)
    }
    finally {
        $fs.Dispose()
    }
    Write-Host "source: created $Path ($Bytes bytes, sparse)"
}

function Invoke-Transfer {
    param(
        [string]$BufferSize,
        [string]$Addr,
        [int]$TcpPort,
        [string]$Src,
        [string]$Dst
    )

    $destDir = Split-Path -Parent $Dst
    if ($destDir -and -not (Test-Path -LiteralPath $destDir)) {
        New-Item -ItemType Directory -Path $destDir -Force | Out-Null
    }
    if (Test-Path -LiteralPath $Dst) { Remove-Item -LiteralPath $Dst -Force }

    Write-Host ""
    Write-Host "=== buffer $BufferSize -> $Addr`:$TcpPort ==="

    $recvOut = Join-Path $env:TEMP "vd-recv-$([guid]::NewGuid().ToString('N')).log"
    $sendOut = Join-Path $env:TEMP "vd-send-$([guid]::NewGuid().ToString('N')).log"

    $receiver = Start-Process -FilePath $receiverBin `
        -ArgumentList @("--addr", "0.0.0.0:$TcpPort", "--output", $Dst, "--buffer-size", $BufferSize) `
        -PassThru -RedirectStandardOutput $recvOut -RedirectStandardError "$recvOut.err" -NoNewWindow

    Start-Sleep -Milliseconds 700

    $sender = Start-Process -FilePath $senderBin `
        -ArgumentList @("--input", $Src, "--addr", "$Addr`:$TcpPort", "--buffer-size", $BufferSize) `
        -PassThru -RedirectStandardOutput $sendOut -RedirectStandardError "$sendOut.err" -NoNewWindow

    $peakRecv = 0L
    $peakSend = 0L
    while (-not $sender.HasExited -or -not $receiver.HasExited) {
        foreach ($pair in @(@($receiver, [ref]$peakRecv), @($sender, [ref]$peakSend))) {
            $p = $pair[0]
            if ($p -and -not $p.HasExited) {
                try {
                    $p.Refresh()
                    $ws = [int64]$p.WorkingSet64
                    $target = $pair[1]
                    if ($ws -gt $target.Value) { $target.Value = $ws }
                }
                catch { }
            }
        }
        Start-Sleep -Milliseconds 150
    }

    # NOTE: every log line below is piped through ForEach-Object { Write-Host }
    # rather than bare ForEach-Object { "..." }. A bare string expression emits
    # to the pipeline, which would append log text to this function's return
    # value and corrupt the summary row for that transfer.
    Write-Host "--- receiver ---"
    Get-Content -LiteralPath $recvOut -ErrorAction SilentlyContinue |
        ForEach-Object { Write-Host "  $_" }
    Write-Host "--- sender ---"
    Get-Content -LiteralPath $sendOut -ErrorAction SilentlyContinue |
        ForEach-Object { Write-Host "  $_" }

    foreach ($e in @("$recvOut.err", "$sendOut.err")) {
        if ((Test-Path -LiteralPath $e) -and (Get-Item -LiteralPath $e).Length -gt 0) {
            Write-Host "--- stderr ---"
            Get-Content -LiteralPath $e | ForEach-Object { Write-Host "  $_" }
        }
    }

    # Single-element array wrapper: without it, any stray pipeline output from
    # this function would merge into the object and produce a malformed row.
    , [PSCustomObject]@{
        BufferSize = $BufferSize
        SourceBytes = (Get-Item -LiteralPath $Src).Length
        DestBytes = if (Test-Path -LiteralPath $Dst) { (Get-Item -LiteralPath $Dst).Length } else { 0 }
        PeakRssSenderMiB = [math]::Round($peakSend / 1MB, 2)
        PeakRssReceiverMiB = [math]::Round($peakRecv / 1MB, 2)
        SenderExit = $sender.ExitCode
        ReceiverExit = $receiver.ExitCode
    }
}

New-SourceFile -Path $SourceFile -Bytes $sourceBytes

$results = @()
$port = $Port
foreach ($bs in ($BufferSizes -split "," | ForEach-Object { $_.Trim() } | Where-Object { $_ })) {
    $results += Invoke-Transfer -BufferSize $bs -Addr $Address -TcpPort $port -Src $SourceFile -Dst $DestFile
    $port++
}

Write-Host ""
Write-Host "=== SUMMARY ($SizeGiB GiB) ==="
# Plain rows rather than Format-Table: Format-Table writes to the success
# stream while Write-Host writes to the host, so the two interleave and the
# table ends up printed before the logs it is summarising.
Write-Host ("{0,-9} {1,14} {2,14} {3,11} {4,11}" -f "Buffer", "SourceBytes", "DestBytes", "RssSend", "RssRecv")
foreach ($r in $results) {
    $line = "{0,-9} {1,14} {2,14} {3,11} {4,11}" -f $r.BufferSize, $r.SourceBytes, $r.DestBytes, $r.PeakRssSenderMiB, $r.PeakRssReceiverMiB
    Write-Host ("$line MiB")
}

Write-Host "Paste into docs/performance.md section 5.2, and record the environment"
Write-Host "fields from section 5.1 alongside it."
Write-Host "Reminder: a single size proves nothing. Repeat at 10 MB and 10 GB and"
Write-Host "compare PeakRss columns -- they must be flat."

foreach ($f in @($SourceFile, $DestFile)) {
    if ((Test-Path -LiteralPath $f) -and -not $KeepFiles) {
        Remove-Item -LiteralPath $f -Force
    }
}
foreach ($e in (Get-ChildItem -Path $env:TEMP -Filter "vd-*.*.log*" -ErrorAction SilentlyContinue)) {
    Remove-Item -LiteralPath $e.FullName -Force -ErrorAction SilentlyContinue
}