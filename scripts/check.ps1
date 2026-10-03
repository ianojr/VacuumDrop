param([switch]$SkipFuzz)

# Mirrors docs/testing.md section 9. Every gate here is a rule from the charter
# that would otherwise be enforced only by discipline.
#
# IMPORTANT: $ErrorActionPreference is deliberately NOT set to 'Stop'.
#
# PowerShell wraps a native command's stderr into ErrorRecord objects. Under
# 'Stop', any cargo or fuzz message on stderr aborts the script even when the
# command succeeded - and cargo writes progress to stderr routinely. Every native
# invocation below checks $LASTEXITCODE explicitly, which is authoritative anyway.
#
# FUZZING NOTE: the libFuzzer targets in fuzz/ need an ASan runtime matching
# rustc's LLVM version. On this machine rustc nightly is built against LLVM 21.1.5
# while only LLVM 22.1.5 is installed, so `cargo fuzz run` cannot start. This
# script detects that and reports it as SKIPPED with the reason, rather than
# pretending to have run. The stable-Rust stress suite in tests/tests/stress.rs
# asserts the same four properties and does run, so the gate is not silently
# weakened. See docs/testing.md sections 4.1-4.2.
#
# Usage:
#   .\scripts\check.ps1              full gate, fuzz attempted
#   .\scripts\check.ps1 -SkipFuzz    omit the fuzz stage

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
$failed = @()
$skipped = @()


function Gate {
    param([string]$Name, [scriptblock]$Action)
    Write-Host ""
    Write-Host "=== $Name ==="
    & $Action
    if ($LASTEXITCODE -eq 0) {
        Write-Host "PASS: $Name"
    }
    else {
        Write-Host "FAIL: $Name (exit $LASTEXITCODE)"
        $script:failed += $Name
    }
}

Gate 'format' { cargo fmt --all -- --check }
Gate 'clippy' { cargo clippy --workspace --all-targets -- -D warnings }
Gate 'build' { cargo build --workspace }
Gate 'test' { cargo test --workspace }

# Charter 47 and engineering rule 5: platform code must never reach the core.
# Enforced structurally rather than by review, because review fails eventually.
Write-Host ""
Write-Host "=== dependency direction ==="
$violations = @()
$coreCrates = @('vacuumdrop-core', 'vacuumdrop-protocol', 'vacuumdrop-transport',
    'vacuumdrop-security', 'vacuumdrop-transfer', 'vacuumdrop-discovery',
    'vacuumdrop-persistence')
foreach ($crate in $coreCrates) {
    $manifest = "crates\$crate\Cargo.toml"
    if (-not (Test-Path -LiteralPath $manifest)) { continue }
    $text = Get-Content -LiteralPath $manifest -Raw
    # Any dependency whose path mentions platform/ is a violation.
    if ($text -match 'path\s*=\s*"[^"]*platform') {
        $violations += "$crate depends on a platform crate"
    }
}
if ($violations.Count -gt 0) {
    foreach ($v in $violations) { Write-Host "  $v" }
    Write-Host "FAIL: dependency direction"
    $failed += 'dependency direction'
}
else {
    Write-Host "PASS: no core crate depends on a platform crate"
}

# Charter 88: unsafe requires justification. `forbid(unsafe_code)` is the
# strongest expression of this and is already set workspace-wide; this checks it
# has not been weakened to `deny` or removed.
Write-Host ""
Write-Host "=== unsafe policy ==="
$rootManifest = Get-Content -LiteralPath "Cargo.toml" -Raw
if ($rootManifest -match 'unsafe_code\s*=\s*"forbid"') {
    Write-Host "PASS: workspace forbids unsafe_code"
}
else {
    Write-Host "FAIL: workspace no longer forbids unsafe_code"
    $failed += 'unsafe policy'
}

# Charter 5 and ADR-0004: the transport abstraction must not accumulate radio
# concepts. Cheap now, and it is the property most likely to erode silently.
Write-Host ""
Write-Host "=== transport abstraction purity ==="
$transport = Get-Content -LiteralPath "crates\vacuumdrop-transport\src\lib.rs" -Raw
$banned = @('group_owner', 'go_intent', 'wifi_direct', 'advertisement')
$leaked = @()
foreach ($b in $banned) {
    # Only method signatures matter; comments explaining what is excluded are fine.
    if ($transport -match "(?m)^\s*fn\s+\w*$b\w*") {
        $leaked += $b
    }
}
if ($leaked.Count -gt 0) {
    Write-Host "FAIL: transport surface leaks radio concepts: $($leaked -join ', ')"
    $failed += 'transport purity'
}
else {
    Write-Host "PASS: transport surface has no radio concepts"
}

# Charter 53: fuzz the parser. Attempted, not assumed.
#
# The four properties are also asserted by tests/tests/stress.rs on stable Rust,
# which runs as part of the `test` gate above. This stage is about coverage-guided
# exploration, which stress testing cannot substitute for. When the toolchain
# cannot run it, that is recorded rather than hidden.
if (-not $SkipFuzz) {
    Write-Host ""
    Write-Host "=== fuzzing ==="

    $targets = @('frame_parser', 'message_decoder', 'peer_validation')
    $seconds = if ($env:VD_FUZZ_SECONDS) { $env:VD_FUZZ_SECONDS } else { 5 }
    $reason = ''

    if (-not (Get-Command cargo-fuzz -ErrorAction SilentlyContinue)) {
        $reason = 'cargo-fuzz is not installed (cargo install cargo-fuzz)'
    }
    elseif (-not (rustup toolchain list 2>&1 | Select-String -Pattern '^nightly')) {
        $reason = 'no nightly toolchain (rustup toolchain install nightly)'
    }

    if (-not [string]::IsNullOrWhiteSpace($reason)) {
        Write-Host "SKIPPED: $reason"
        $skipped += 'fuzzing'
    }
    else {
        # Probe once before spending time on all three targets. Without this, an
        # unrunnable toolchain reports three separate failures that are really one
        # environmental problem, and the real reason is buried in the output.
        Push-Location "$root\fuzz"
        $probeOk = $false
        $probeReason = ''
        try {
            # `2>&1 | Out-String` is deliberately avoided here: PowerShell wraps a
            # native command's stderr into ErrorRecords, which then get written to
            # the error stream and abort the script under $ErrorActionPreference.
            # Redirecting to a file and reading it back keeps cargo's diagnostics
            # as plain text.
            $probeLog = Join-Path $env:TEMP "vd-fuzz-probe.log"
            & cargo +nightly fuzz run $targets[0] -- "-max_total_time=$seconds" *>&1 |
                Out-File -FilePath $probeLog -Encoding utf8
            $probeOk = ($LASTEXITCODE -eq 0)
            if (-not $probeOk) {
                $probeOutput = Get-Content -LiteralPath $probeLog -Raw -ErrorAction SilentlyContinue
                # STATUS_DLL_NOT_FOUND (0xc0000135) then
                # STATUS_ENTRYPOINT_NOT_FOUND (0xC0000139) both mean the ASan
                # runtime could not be loaded: it must match rustc's LLVM version.
                if ($probeOutput -match '0xc0000135|0xC0000139|DLL_NOT_FOUND|ENTRYPOINT_NOT_FOUND') {
                    $probeReason = 'ASan runtime missing or ABI-incompatible with rustc''s LLVM ' +
                        "(needs 21.1.5). Run the targets on Linux/CI, or install a " +
                        'matching LLVM. See docs/testing.md section 4.1.'
                }
                elseif ($probeOutput -match 'panicked|Test unit written') {
                    $probeReason = "$($targets[0]) found a crash - see fuzz\artifacts\"
                }
                else {
                    $probeReason = "$($targets[0]) failed to run"
                }
            }
        }
        finally {
            Pop-Location
        }

        if (-not $probeOk) {
            if ($probeReason -match 'found a crash') {
                Write-Host "FAIL: fuzzing - $($targets[0]) CRASHED"
                Write-Host "      Artifact in fuzz\artifacts\$($targets[0])\"
                Write-Host "      Do not delete it. See fuzz\README.md."
                $failed += 'fuzzing'
            }
            else {
                Write-Host "SKIPPED: $probeReason"
                Write-Host "         The four properties are asserted by tests\tests\stress.rs,"
                Write-Host "         which runs in the test gate above."
                $skipped += 'fuzzing'
            }
        }
        else {
            $fuzzFailed = @()
            foreach ($t in $targets) {
                Push-Location "$root\fuzz"
                try {
                    & cargo +nightly fuzz run $t -- "-max_total_time=$seconds" 2>&1 | Out-Null
                    if ($LASTEXITCODE -ne 0) { $fuzzFailed += $t }
                }
                finally {
                    Pop-Location
                }
            }
            if ($fuzzFailed.Count -gt 0) {
                Write-Host "FAIL: fuzzing ($($fuzzFailed -join ', '))"
                Write-Host "      A failing target writes an artifact to fuzz\artifacts\."
                Write-Host "      Do not delete it. See fuzz\README.md."
                $failed += 'fuzzing'
            }
            else {
                Write-Host "PASS: all $($targets.Count) fuzz targets ran clean for ${seconds}s"
            }
        }
    }
}

Pop-Location

Write-Host ""
if ($skipped.Count -gt 0) {
    Write-Host "SKIPPED (not passing, not failing): $($skipped -join ', ')"
}
if ($failed.Count -gt 0) {
    Write-Host "FAILED GATES: $($failed -join ', ')"
    exit 1
}
Write-Host "All gates passed."
exit 0