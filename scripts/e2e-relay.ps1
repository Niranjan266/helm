<#
.SYNOPSIS
    Real end-to-end test through a live relay: agent <-> wss:// relay <-> phone code.

.DESCRIPTION
    Starts host/agent with a throwaway --data-dir (never %APPDATA%\Helm), relay on, on a free port
    and without mDNS. Reads the helm://pair link it prints and runs the opt-in JVM test
    E2eRealAgentTest.pairPingStreamAndRevokeThroughTheRealRelay, which uses the link's r= address
    only (no LAN address). Then quits the agent and deletes the data dir.

    The test streams this PC's screen for a few seconds over the relay (end-to-end encrypted, frames
    are dropped on arrival) and uses a little of the relay's Cloudflare quota.

.PARAMETER Relay
    Relay address (any form the agent accepts). Defaults to the owner's deployment.

.EXAMPLE
    powershell -File scripts/e2e-relay.ps1
#>
[CmdletBinding()]
param(
    [string]$Relay = 'wss://helm-relay.supportdangle-ae0.workers.dev',
    [string]$AgentExe = '',
    # Optional soak: keep the session idle this long before streaming.
    [int]$IdleSeconds = 0
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $AgentExe) { $AgentExe = Join-Path $repo 'host\agent\target\debug\helm-agent.exe' }
if (-not (Test-Path $AgentExe)) { throw "Build the agent first: cargo build --bin helm-agent (in host\agent). Not found: $AgentExe" }

$dataDir = Join-Path ([IO.Path]::GetTempPath()) ("helm-e2e-relay-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $dataDir | Out-Null
$relayJson = @{ enabled = $true; url = $Relay } | ConvertTo-Json -Compress
[IO.File]::WriteAllText((Join-Path $dataDir 'relay.json'), $relayJson)

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $AgentExe
$psi.Arguments = "--data-dir `"$dataDir`" --port 0 --no-mdns --no-tray"
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.StandardOutputEncoding = [Text.Encoding]::UTF8
$psi.CreateNoWindow = $true
$proc = [System.Diagnostics.Process]::Start($psi)
$errors = $proc.StandardError.ReadToEndAsync()
$exit = 1
try {
    $uri = $null
    while (-not $uri) {
        $line = $proc.StandardOutput.ReadLine()
        if ($null -eq $line) { throw "agent exited before printing a pairing link:`n$($errors.Result)" }
        if ($line -match 'Pairing link:\s*(helm://\S+)') { $uri = $Matches[1] }
    }
    $rest = $proc.StandardOutput.ReadToEndAsync()
    if ($uri -notmatch '[?&]r=') { throw "the pairing link has no r= parameter" }
    Write-Host "[e2e-relay] agent running, data dir $dataDir; link carries r="

    $env:HELM_E2E_RELAY = '1'
    $env:HELM_E2E_URI = $uri
    if ($IdleSeconds -gt 0) { $env:HELM_E2E_IDLE_S = "$IdleSeconds" }
    $gradlew = Join-Path $repo 'android\gradlew.bat'
    & $gradlew -p (Join-Path $repo 'android') --offline ':app:testDebugUnitTest' '--tests' 'com.niranjan.helm.core.E2eRealAgentTest.pairPingStreamAndRevokeThroughTheRealRelay' 2>&1 |
        Where-Object { $_ -match '\[helm-e2e\]|PASSED|FAILED|BUILD|AssertionError|Exception|SKIPPED' } | ForEach-Object { Write-Host $_ }
    $exit = $LASTEXITCODE
} finally {
    Remove-Item Env:HELM_E2E_URI, Env:HELM_E2E_RELAY, Env:HELM_E2E_IDLE_S -ErrorAction SilentlyContinue
    if (-not $proc.HasExited) {
        $proc.StandardInput.WriteLine('q')
        if (-not $proc.WaitForExit(15000)) { $proc.Kill() }
    }
    Write-Host "[e2e-relay] agent stopped"
    $errors.Result -split "`n" | Where-Object { $_ -match 'relay' } | ForEach-Object { Write-Host "[agent] $_" }
    $audit = Join-Path $dataDir 'audit.log'
    if (Test-Path $audit) {
        Get-Content $audit | Where-Object { $_ -match 'relay' } | ForEach-Object { Write-Host "[audit] $_" }
    }
    Remove-Item -Recurse -Force $dataDir -ErrorAction SilentlyContinue
}
exit $exit
