<#
.SYNOPSIS
    Runs every Helm test suite locally and prints a pass/fail summary.

.DESCRIPTION
    Steps (in order):
      agent      cargo test + cargo clippy --all-targets -- -D warnings  (host/agent)
      <crate>    the same for every folder in host/crates (each has its own Cargo.toml/target)
      relay      npm ci + npm test                                      (relay)
      android    gradlew testDebugUnitTest assembleDebug --offline       (android)
      e2e        builds host/agent --release, starts it with a temp --data-dir
                 (--no-mdns --no-tray --port 0), reads the helm://pair link it prints,
                 runs E2eRealAgentTest against it with HELM_E2E_URI, parses the JUnit XML,
                 then stops the agent and deletes the temp data dir.

    #[ignore] Rust tests are never run (no --ignored / --include-ignored is ever passed).

.PARAMETER Quick
    Skip clippy and the real-agent E2E step.

.PARAMETER Only
    Run only the named step group(s): agent, crates, <crate folder name> (e.g. helm-win-audio),
    relay, android, e2e. Accepts several, e.g. -Only agent,relay

.EXAMPLE
    powershell -File scripts/test-all.ps1
    powershell -File scripts/test-all.ps1 -Quick
    powershell -File scripts/test-all.ps1 -Only helm-win-input,relay
#>
[CmdletBinding()]
param(
    [switch]$Quick,
    [string[]]$Only
)

$ErrorActionPreference = 'Continue'
Set-StrictMode -Version 2

$RepoRoot   = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$AgentDir   = Join-Path $RepoRoot 'host\agent'
$CratesDir  = Join-Path $RepoRoot 'host\crates'
$RelayDir   = Join-Path $RepoRoot 'relay'
$AndroidDir = Join-Path $RepoRoot 'android'
$Gradlew    = Join-Path $AndroidDir 'gradlew.bat'
$E2eClass   = 'com.niranjan.helm.core.E2eRealAgentTest'

if (-not $env:JAVA_HOME) {
    $studioJbr = 'C:\Program Files\Android\Android Studio\jbr'
    if (Test-Path $studioJbr) { $env:JAVA_HOME = $studioJbr }
}

$CrateNames = @()
if (Test-Path $CratesDir) {
    $CrateNames = @(Get-ChildItem $CratesDir -Directory |
        Where-Object { Test-Path (Join-Path $_.FullName 'Cargo.toml') } |
        Sort-Object Name | ForEach-Object { $_.Name })
}

$Results = New-Object System.Collections.Generic.List[object]

function Test-Wanted([string]$Group, [bool]$IsCrate = $false) {
    if (-not $Only -or $Only.Count -eq 0) { return $true }
    foreach ($o in $Only) {
        foreach ($name in ($o -split ',')) {
            $n = $name.Trim()
            if ($n -ieq $Group) { return $true }
            if ($IsCrate -and $n -ieq 'crates') { return $true }
        }
    }
    return $false
}

function Add-Result([string]$Step, [string]$Status, [double]$Seconds, [string]$Note = '') {
    $Results.Add([pscustomobject]@{
        Step    = $Step
        Result  = $Status
        Seconds = [math]::Round($Seconds, 1)
        Note    = $Note
    })
}

# Runs a native command in a directory; PASS when the exit code is 0.
function Invoke-Step([string]$Step, [string]$Dir, [string]$Exe, [string[]]$CmdArgs) {
    Write-Host ''
    Write-Host ("==> {0}   ({1} {2})" -f $Step, $Exe, ($CmdArgs -join ' ')) -ForegroundColor Cyan
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $code = 1
    $note = ''
    Push-Location $Dir
    try {
        # Out-Host keeps tool output on screen and out of this function's return value.
        & $Exe @CmdArgs | Out-Host
        $code = $LASTEXITCODE
        if ($null -eq $code) { $code = 0 }
    } catch {
        $note = $_.Exception.Message
        $code = 1
    } finally {
        Pop-Location
    }
    $sw.Stop()
    if ($code -eq 0) { $status = 'PASS' } else { $status = 'FAIL'; if (-not $note) { $note = "exit code $code" } }
    Add-Result $Step $status $sw.Elapsed.TotalSeconds $note
    return ($code -eq 0)
}

function Test-Tool([string]$Step, [string]$Tool) {
    if (Get-Command $Tool -ErrorAction SilentlyContinue) { return $true }
    Add-Result $Step 'FAIL' 0 "$Tool not found on PATH"
    return $false
}

function Invoke-RustSteps([string]$Name, [string]$Dir) {
    if (-not (Test-Tool "${Name}: test" 'cargo')) { return }
    # Plain `cargo test` never runs #[ignore] tests.
    [void](Invoke-Step "${Name}: test" $Dir 'cargo' @('test'))
    if (-not $Quick) {
        [void](Invoke-Step "${Name}: clippy" $Dir 'cargo' @('clippy', '--all-targets', '--', '-D', 'warnings'))
    }
}

function Invoke-E2e {
    $step = 'e2e: real agent'
    Write-Host ''
    Write-Host "==> $step" -ForegroundColor Cyan
    if (-not (Test-Tool $step 'cargo')) { return }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()

    if (-not (Invoke-Step 'e2e: build agent (release)' $AgentDir 'cargo' @('build', '--release'))) {
        Add-Result $step 'SKIP' 0 'agent release build failed'
        return
    }
    $exe = Join-Path $AgentDir 'target\release\helm-agent.exe'
    if (-not (Test-Path $exe)) {
        Add-Result $step 'FAIL' $sw.Elapsed.TotalSeconds "missing $exe"
        return
    }

    $tmp     = Join-Path ([System.IO.Path]::GetTempPath()) ("helm-e2e-" + [guid]::NewGuid().ToString('N'))
    $dataDir = Join-Path $tmp 'data'
    $outLog  = Join-Path $tmp 'agent.out.log'
    $errLog  = Join-Path $tmp 'agent.err.log'
    $stdin   = Join-Path $tmp 'stdin.txt'
    New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
    Set-Content -Path $stdin -Value '' -Encoding ascii

    $proc = $null
    $status = 'FAIL'
    $note = ''
    try {
        $argLine = "--data-dir `"$dataDir`" --no-mdns --no-tray --port 0"
        $proc = Start-Process -FilePath $exe -ArgumentList $argLine -PassThru -NoNewWindow `
            -RedirectStandardOutput $outLog -RedirectStandardError $errLog -RedirectStandardInput $stdin

        $uri = $null
        $deadline = (Get-Date).AddSeconds(30)
        while (-not $uri -and (Get-Date) -lt $deadline) {
            Start-Sleep -Milliseconds 250
            if (Test-Path $outLog) {
                $text = Get-Content -Raw -Path $outLog -ErrorAction SilentlyContinue
                if ($text -and $text -match '(helm://pair\?\S+)') { $uri = $Matches[1] }
            }
            if ($proc.HasExited) { break }
        }
        if (-not $uri) {
            $note = 'no helm://pair link from agent within 30 s'
            if ($proc.HasExited) { $note = "agent exited early (code $($proc.ExitCode))" }
            Write-Host '--- agent stdout ---'; if (Test-Path $outLog) { Get-Content $outLog | Write-Host }
            Write-Host '--- agent stderr ---'; if (Test-Path $errLog) { Get-Content $errLog | Write-Host }
            return
        }
        Write-Host "Agent up (pid $($proc.Id)); running $E2eClass"

        $env:HELM_E2E_URI = $uri
        try {
            $ok = Invoke-Step 'e2e: gradle E2eRealAgentTest' $AndroidDir $Gradlew `
                @('testDebugUnitTest', '--tests', $E2eClass, '--rerun', '--offline')
        } finally {
            Remove-Item Env:\HELM_E2E_URI -ErrorAction SilentlyContinue
        }

        $xml = Join-Path $AndroidDir "app\build\test-results\testDebugUnitTest\TEST-$E2eClass.xml"
        if (-not (Test-Path $xml)) {
            $note = 'no JUnit XML produced'
            return
        }
        [xml]$doc = Get-Content -Raw -Path $xml
        $suite = $doc.testsuite
        $tests = [int]$suite.tests; $fail = [int]$suite.failures
        $errs  = [int]$suite.errors; $skip = [int]$suite.skipped
        $note  = "tests=$tests failures=$fail errors=$errs skipped=$skip"
        if ($ok -and $tests -gt 0 -and $fail -eq 0 -and $errs -eq 0 -and $skip -eq 0) { $status = 'PASS' }
        elseif ($skip -gt 0 -and $fail -eq 0 -and $errs -eq 0) { $note += ' (HELM_E2E_URI not seen by the test JVM?)' }
    } finally {
        if ($proc -and -not $proc.HasExited) {
            Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
            $null = $proc.WaitForExit(10000)
        }
        Remove-Item -Recurse -Force -Path $tmp -ErrorAction SilentlyContinue
        $sw.Stop()
        Add-Result $step $status $sw.Elapsed.TotalSeconds $note
    }
}

# ---------------------------------------------------------------- run
$total = [System.Diagnostics.Stopwatch]::StartNew()

if (Test-Wanted 'agent') { Invoke-RustSteps 'agent' $AgentDir }

foreach ($c in $CrateNames) {
    if (Test-Wanted $c $true) { Invoke-RustSteps $c (Join-Path $CratesDir $c) }
}

if (Test-Wanted 'relay') {
    if (Test-Tool 'relay: npm ci' 'npm') {
        if (Invoke-Step 'relay: npm ci' $RelayDir 'npm' @('ci', '--no-audit', '--no-fund')) {
            [void](Invoke-Step 'relay: npm test' $RelayDir 'npm' @('test'))
        } else {
            Add-Result 'relay: npm test' 'SKIP' 0 'npm ci failed'
        }
    }
}

if (Test-Wanted 'android') {
    if (-not $env:JAVA_HOME) {
        Add-Result 'android: unit tests + assembleDebug' 'FAIL' 0 'JAVA_HOME not set'
    } else {
        [void](Invoke-Step 'android: unit tests + assembleDebug' $AndroidDir $Gradlew `
            @('testDebugUnitTest', 'assembleDebug', '--offline'))
    }
}

if (-not $Quick -and (Test-Wanted 'e2e')) {
    if (-not $env:JAVA_HOME) { Add-Result 'e2e: real agent' 'FAIL' 0 'JAVA_HOME not set' }
    else { Invoke-E2e }
}

$total.Stop()

# ---------------------------------------------------------------- summary
Write-Host ''
Write-Host '================ Helm test summary ================' -ForegroundColor White
if ($Results.Count -eq 0) {
    Write-Host 'No steps ran (check -Only / -Quick).' -ForegroundColor Yellow
    exit 2
}
$Results | Format-Table -AutoSize Step, Result, Seconds, Note | Out-String -Width 200 | Write-Host
$failed = @($Results | Where-Object { $_.Result -eq 'FAIL' })
$passed = @($Results | Where-Object { $_.Result -eq 'PASS' })
$line = "{0} passed, {1} failed, {2} skipped in {3:N0} s" -f $passed.Count, $failed.Count,
    ($Results.Count - $passed.Count - $failed.Count), $total.Elapsed.TotalSeconds
if ($failed.Count -gt 0) {
    Write-Host "FAILED: $line" -ForegroundColor Red
    exit 1
}
Write-Host "OK: $line" -ForegroundColor Green
exit 0
