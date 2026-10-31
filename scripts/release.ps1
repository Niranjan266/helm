<#
.SYNOPSIS
    Builds a Helm release: bumps versions, runs every test, builds the agent, the service,
    the Android APK and the Windows installer, and writes SHA256SUMS.

.DESCRIPTION
    1. Sets the version in host/agent/Cargo.toml, host/crates/helm-service/Cargo.toml and
       android/app/build.gradle.kts (versionName = -Version, versionCode + 1).
    2. Runs scripts/test-all.ps1 (full, including clippy and the real-agent E2E) unless -SkipTests.
    3. cargo build --release for host/agent and host/crates/helm-service.
    4. gradlew assembleRelease. The APK is left unsigned unless a keystore is configured with
       HELM_ANDROID_KEYSTORE, HELM_ANDROID_KEYSTORE_PASSWORD and HELM_ANDROID_KEY_ALIAS
       (optional HELM_ANDROID_KEY_PASSWORD); passwords are read by apksigner from the
       environment, never put on the command line.
    5. Compiles host/installer/helm.iss with Inno Setup's ISCC if it is installed.
    6. Copies everything to dist/v<version>/ and writes dist/v<version>/SHA256SUMS.
    7. Only with -Publish: `gh release create v<version>` with those files.

    Nothing is committed or tagged locally; commit and push the version bump yourself
    before publishing so the release tag points at the right commit.

.EXAMPLE
    powershell -File scripts/release.ps1 -Version 0.2.0
    powershell -File scripts/release.ps1 -Version 0.2.0 -Publish -Draft
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$SkipTests,
    [switch]$Publish,
    [switch]$Draft,
    [string]$NotesFile
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
    throw "Version must look like 1.2.3 or 1.2.3-beta.1 (got '$Version')"
}

$RepoRoot   = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$AgentDir   = Join-Path $RepoRoot 'host\agent'
$ServiceDir = Join-Path $RepoRoot 'host\crates\helm-service'
$AndroidDir = Join-Path $RepoRoot 'android'
$Iss        = Join-Path $RepoRoot 'host\installer\helm.iss'
$Dist       = Join-Path $RepoRoot "dist\v$Version"
$Tag        = "v$Version"
$Utf8NoBom  = New-Object System.Text.UTF8Encoding($false)

if (-not $env:JAVA_HOME) {
    $studioJbr = 'C:\Program Files\Android\Android Studio\jbr'
    if (Test-Path $studioJbr) { $env:JAVA_HOME = $studioJbr }
}

function Write-Stage([string]$Text) {
    Write-Host ''
    Write-Host "==> $Text" -ForegroundColor Cyan
}

function Invoke-Native([string]$Dir, [string]$Exe, [string[]]$CmdArgs) {
    Push-Location $Dir
    try {
        $ErrorActionPreference = 'Continue'
        & $Exe @CmdArgs | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "$Exe $($CmdArgs -join ' ') failed (exit $LASTEXITCODE) in $Dir" }
    } finally {
        Pop-Location
    }
}

function Set-FileText([string]$Path, [string]$Text) {
    [System.IO.File]::WriteAllText($Path, $Text, $Utf8NoBom)
}

# Replaces the first `version = "..."` line (the [package] one) in a Cargo.toml.
function Set-CargoVersion([string]$Path) {
    $text = [System.IO.File]::ReadAllText($Path)
    $re = New-Object System.Text.RegularExpressions.Regex('(?m)^version\s*=\s*"[^"]*"')
    if (-not $re.IsMatch($text)) { throw "no version line in $Path" }
    Set-FileText $Path ($re.Replace($text, "version = `"$Version`"", 1))
    Write-Host "  $Path -> $Version"
}

function Set-AndroidVersion([string]$Path) {
    $text = [System.IO.File]::ReadAllText($Path)
    $codeRe = New-Object System.Text.RegularExpressions.Regex('versionCode\s*=\s*(\d+)')
    $m = $codeRe.Match($text)
    if (-not $m.Success) { throw "no versionCode in $Path" }
    $newCode = [int]$m.Groups[1].Value + 1
    $text = $codeRe.Replace($text, "versionCode = $newCode", 1)
    $nameRe = New-Object System.Text.RegularExpressions.Regex('versionName\s*=\s*"[^"]*"')
    if (-not $nameRe.IsMatch($text)) { throw "no versionName in $Path" }
    $text = $nameRe.Replace($text, "versionName = `"$Version`"", 1)
    Set-FileText $Path $text
    Write-Host "  $Path -> $Version (versionCode $newCode)"
}

function Find-Iscc {
    $cmd = Get-Command iscc -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    foreach ($base in @(${env:ProgramFiles(x86)}, $env:ProgramFiles, (Join-Path $env:LOCALAPPDATA 'Programs'))) {
        if (-not $base) { continue }
        $p = Join-Path $base 'Inno Setup 6\ISCC.exe'
        if (Test-Path $p) { return $p }
    }
    return $null
}

function Find-SdkDir {
    foreach ($v in @($env:ANDROID_HOME, $env:ANDROID_SDK_ROOT)) { if ($v -and (Test-Path $v)) { return $v } }
    $lp = Join-Path $AndroidDir 'local.properties'
    if (Test-Path $lp) {
        $line = Get-Content $lp | Where-Object { $_ -match '^sdk\.dir=' } | Select-Object -First 1
        if ($line) {
            $dir = ($line -replace '^sdk\.dir=', '') -replace '\\:', ':' -replace '\\\\', '\'
            if (Test-Path $dir) { return $dir }
        }
    }
    $def = Join-Path $env:LOCALAPPDATA 'Android\Sdk'
    if (Test-Path $def) { return $def }
    return $null
}

function Find-BuildTool([string]$Name) {
    $sdk = Find-SdkDir
    if (-not $sdk) { return $null }
    $bt = Join-Path $sdk 'build-tools'
    if (-not (Test-Path $bt)) { return $null }
    $latest = Get-ChildItem $bt -Directory | Sort-Object { try { [version]($_.Name -replace '-.*$', '') } catch { [version]'0.0' } } -Descending |
        Where-Object { Test-Path (Join-Path $_.FullName $Name) } | Select-Object -First 1
    if ($latest) { return (Join-Path $latest.FullName $Name) }
    return $null
}

# ---------------------------------------------------------------- 1. version bump
Write-Stage "Setting version $Version"
Set-CargoVersion (Join-Path $AgentDir 'Cargo.toml')
Set-CargoVersion (Join-Path $ServiceDir 'Cargo.toml')
Set-AndroidVersion (Join-Path $AndroidDir 'app\build.gradle.kts')

# ---------------------------------------------------------------- 2. tests
if ($SkipTests) {
    Write-Stage 'Skipping tests (-SkipTests)'
} else {
    Write-Stage 'Running scripts/test-all.ps1'
    $ErrorActionPreference = 'Continue'
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'test-all.ps1')
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($code -ne 0) { throw "test-all.ps1 failed (exit $code); not building a release" }
}

# ---------------------------------------------------------------- 3. Rust binaries
Write-Stage 'Building helm-agent and helm-service (release)'
Invoke-Native $AgentDir 'cargo' @('build', '--release')
Invoke-Native $ServiceDir 'cargo' @('build', '--release')
$agentExe   = Join-Path $AgentDir 'target\release\helm-agent.exe'
$serviceExe = Join-Path $ServiceDir 'target\release\helm-service.exe'
foreach ($f in @($agentExe, $serviceExe)) { if (-not (Test-Path $f)) { throw "missing $f" } }

# ---------------------------------------------------------------- 4. Android
Write-Stage 'Building Android release APK'
if (-not $env:JAVA_HOME) { throw 'JAVA_HOME is not set (Android Studio jbr not found)' }
Invoke-Native $AndroidDir (Join-Path $AndroidDir 'gradlew.bat') @('assembleRelease')
$apkDir = Join-Path $AndroidDir 'app\build\outputs\apk\release'
$builtApk = Get-ChildItem $apkDir -Filter '*.apk' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $builtApk) { throw "no APK in $apkDir" }

New-Item -ItemType Directory -Force -Path $Dist | Out-Null
Get-ChildItem $Dist -File | Remove-Item -Force

$apkOut = $null
$haveKeystore = $env:HELM_ANDROID_KEYSTORE -and $env:HELM_ANDROID_KEYSTORE_PASSWORD -and $env:HELM_ANDROID_KEY_ALIAS
if ($builtApk.Name -notmatch 'unsigned') {
    $apkOut = Join-Path $Dist "Helm-$Version.apk"
    Copy-Item $builtApk.FullName $apkOut
    Write-Host "  APK signed by the Gradle config: $($builtApk.Name)"
} elseif ($haveKeystore) {
    $zipalign  = Find-BuildTool 'zipalign.exe'
    $apksigner = Find-BuildTool 'apksigner.bat'
    if (-not $zipalign -or -not $apksigner) { throw 'keystore configured but zipalign/apksigner not found in the Android SDK build-tools' }
    $aligned = Join-Path $Dist 'aligned.apk'
    $apkOut  = Join-Path $Dist "Helm-$Version.apk"
    Invoke-Native $Dist $zipalign @('-p', '-f', '4', $builtApk.FullName, $aligned)
    $signArgs = @('sign', '--ks', $env:HELM_ANDROID_KEYSTORE, '--ks-pass', 'env:HELM_ANDROID_KEYSTORE_PASSWORD',
        '--ks-key-alias', $env:HELM_ANDROID_KEY_ALIAS, '--out', $apkOut)
    if ($env:HELM_ANDROID_KEY_PASSWORD) { $signArgs += @('--key-pass', 'env:HELM_ANDROID_KEY_PASSWORD') }
    Invoke-Native $Dist $apksigner ($signArgs + @($aligned))
    Remove-Item $aligned -Force
    Remove-Item (Join-Path $Dist '*.idsig') -Force -ErrorAction SilentlyContinue
    Write-Host '  APK signed with HELM_ANDROID_KEYSTORE'
} else {
    $apkOut = Join-Path $Dist "Helm-$Version-unsigned.apk"
    Copy-Item $builtApk.FullName $apkOut
    Write-Host '  No keystore configured: APK is unsigned' -ForegroundColor Yellow
}

# ---------------------------------------------------------------- 5. installer
Write-Stage 'Building Windows installer'
$installerOut = $null
$iscc = Find-Iscc
if ($iscc) {
    Invoke-Native (Split-Path $Iss) $iscc @("/DAppVersion=$Version", $Iss)
    $setup = Join-Path (Split-Path $Iss) "Output\Helm-Setup-$Version-x64.exe"
    if (-not (Test-Path $setup)) { throw "ISCC did not produce $setup" }
    $installerOut = Join-Path $Dist (Split-Path $setup -Leaf)
    Copy-Item $setup $installerOut
} else {
    Write-Host '  Inno Setup (ISCC.exe) not found: skipping the installer' -ForegroundColor Yellow
}

# ---------------------------------------------------------------- 6. dist + SHA256SUMS
Write-Stage "Collecting artifacts in $Dist"
Copy-Item $agentExe   (Join-Path $Dist 'helm-agent.exe')
Copy-Item $serviceExe (Join-Path $Dist 'helm-service.exe')

$files = @(Get-ChildItem $Dist -File | Where-Object { $_.Name -ne 'SHA256SUMS' } | Sort-Object Name)
$sums = foreach ($f in $files) {
    $h = (Get-FileHash -Algorithm SHA256 -Path $f.FullName).Hash.ToLowerInvariant()
    "$h  $($f.Name)"
}
Set-FileText (Join-Path $Dist 'SHA256SUMS') (($sums -join "`n") + "`n")
$files | ForEach-Object { Write-Host ("  {0,-34} {1,12:N0} bytes" -f $_.Name, $_.Length) }
Write-Host '  SHA256SUMS'

# ---------------------------------------------------------------- 7. publish (opt-in only)
if (-not $Publish) {
    Write-Host ''
    Write-Host "Release $Version built in $Dist (not published; pass -Publish to create the GitHub release)." -ForegroundColor Green
    exit 0
}

Write-Stage "Publishing GitHub release $Tag"
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { throw 'gh (GitHub CLI) not found' }
$assets = @(Get-ChildItem $Dist -File | ForEach-Object { $_.FullName })
$ghArgs = @('release', 'create', $Tag) + $assets + @('--title', "Helm $Version")
if ($NotesFile) { $ghArgs += @('--notes-file', $NotesFile) } else { $ghArgs += '--generate-notes' }
if ($Draft) { $ghArgs += '--draft' }
if ($Version -match '-') { $ghArgs += '--prerelease' }
Invoke-Native $RepoRoot 'gh' $ghArgs
Write-Host "Published $Tag." -ForegroundColor Green
