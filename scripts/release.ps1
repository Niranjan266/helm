<#
.SYNOPSIS
    Builds a Helm release: bumps versions, runs every test, builds the desktop app (Helm.exe),
    the console agent, the service, the Android APK and the Windows installers, signs every
    asset for the in-app updaters and writes SHA256SUMS.

.DESCRIPTION
    0. Checks the signing keys first (see SIGNING below), so a release cannot fail at the end.
    1. Sets the version in host/agent/Cargo.toml, host/desktop/Cargo.toml,
       host/desktop/tauri.conf.json, host/crates/helm-service/Cargo.toml and
       android/app/build.gradle.kts (versionName = -Version, versionCode + 1).
    2. Runs scripts/test-all.ps1 (full, including clippy and the real-agent E2E) unless -SkipTests.
    3. cargo tauri build in host/desktop (Helm.exe + NSIS installer; installs tauri-cli if
       missing), then cargo build --release for host/agent (console agent, for developers and
       helm-service) and host/crates/helm-service.
    4. gradlew assembleRelease, signed by the Gradle config with the release keystore. The
       signature is checked with apksigner when it is available.
    5. Compiles host/installer/helm.iss with Inno Setup's ISCC if it is installed.
    6. Copies everything to dist/v<version>/ (Helm.exe as Helm-windows-x64.exe, the asset
       scripts/install.ps1 downloads; the NSIS installer as Helm-windows-x64-setup.exe, the
       one the desktop updater runs; the APK as Helm-android.apk), signs each file with
       helm-sign (<asset>.sig + <asset>.sha256), verifies them against the committed public
       key and writes dist/v<version>/SHA256SUMS.
    7. Only with -Publish: `gh release create v<version>` with those files.

    SIGNING. Private material lives outside the repository, in %USERPROFILE%\.helm:
      update-ed25519.key             Ed25519 update key (HELM_UPDATE_SIGNING_KEY overrides the path)
      android-release.p12            Android keystore (HELM_ANDROID_KEYSTORE)
      android-keystore-password.txt  its password (HELM_ANDROID_KEYSTORE_PASSWORD)
    The update key must match keys/update-ed25519.pub or the release stops. Without the keys
    the release stops too, unless -Unsigned is given (a test build: updaters will ignore it).

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
    [switch]$Unsigned,
    [string]$NotesFile
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
    throw "Version must look like 1.2.3 or 1.2.3-beta.1 (got '$Version')"
}

$RepoRoot   = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$AgentDir   = Join-Path $RepoRoot 'host\agent'
$DesktopDir = Join-Path $RepoRoot 'host\desktop'
$ServiceDir = Join-Path $RepoRoot 'host\crates\helm-service'
$AndroidDir = Join-Path $RepoRoot 'android'
$Iss        = Join-Path $RepoRoot 'host\installer\helm.iss'
$Dist       = Join-Path $RepoRoot "dist\v$Version"
$Tag        = "v$Version"
$Utf8NoBom  = New-Object System.Text.UTF8Encoding($false)
$KeyDir     = Join-Path $env:USERPROFILE '.helm'
$PubKeyFile = Join-Path $RepoRoot 'keys\update-ed25519.pub'
$SignKey    = if ($env:HELM_UPDATE_SIGNING_KEY) { $env:HELM_UPDATE_SIGNING_KEY } else { Join-Path $KeyDir 'update-ed25519.key' }
$Keystore   = if ($env:HELM_ANDROID_KEYSTORE) { $env:HELM_ANDROID_KEYSTORE } else { Join-Path $KeyDir 'android-release.p12' }
$KeystorePw = Join-Path $KeyDir 'android-keystore-password.txt'
$HelmSign   = Join-Path $RepoRoot 'host\crates\helm-update\target\release\examples\helm-sign.exe'

if ($Unsigned -and $Publish) { throw '-Unsigned builds cannot be published' }

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

# tauri.conf.json carries the version the NSIS installer is named after.
function Set-TauriVersion([string]$Path) {
    $text = [System.IO.File]::ReadAllText($Path)
    $re = New-Object System.Text.RegularExpressions.Regex('"version"\s*:\s*"[^"]*"')
    if (-not $re.IsMatch($text)) { throw "no version in $Path" }
    Set-FileText $Path ($re.Replace($text, "`"version`": `"$Version`"", 1))
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

# ---------------------------------------------------------------- 0. signing keys
$haveKeys = $false
if (-not $Unsigned) {
    Write-Stage 'Checking the signing keys'
    if (-not (Test-Path $SignKey)) { throw "update signing key not found: $SignKey (or pass -Unsigned for a test build)" }
    if (-not (Test-Path $Keystore)) { throw "Android keystore not found: $Keystore (or pass -Unsigned for a test build)" }
    if (-not $env:HELM_ANDROID_KEYSTORE_PASSWORD -and -not (Test-Path $KeystorePw)) { throw "Android keystore password not found: $KeystorePw" }
    Invoke-Native $RepoRoot 'cargo' @('build', '--release', '--example', 'helm-sign', '--manifest-path', 'host\crates\helm-update\Cargo.toml')
    $expectedPub = (Get-Content $PubKeyFile -Raw).Trim()
    $actualPub = (& $HelmSign pubkey $SignKey | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'helm-sign could not read the update signing key' }
    if ($actualPub -ne $expectedPub) { throw "$SignKey does not match keys/update-ed25519.pub; apps would refuse this release" }
    $haveKeys = $true
    Write-Host "  update key matches keys/update-ed25519.pub; keystore: $Keystore"
}

# ---------------------------------------------------------------- 1. version bump
Write-Stage "Setting version $Version"
Set-CargoVersion (Join-Path $AgentDir 'Cargo.toml')
Set-CargoVersion (Join-Path $DesktopDir 'Cargo.toml')
Set-TauriVersion (Join-Path $DesktopDir 'tauri.conf.json')
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
Write-Stage 'Building the desktop app (cargo tauri build)'
$ErrorActionPreference = 'Continue'
& cargo tauri --version *> $null
$haveTauri = ($LASTEXITCODE -eq 0)
$ErrorActionPreference = 'Stop'
if (-not $haveTauri) {
    Invoke-Native $RepoRoot 'cargo' @('install', 'tauri-cli', '--version', '^2', '--locked')
}
Invoke-Native $DesktopDir 'cargo' @('tauri', 'build')
$desktopExe = Join-Path $DesktopDir 'target\release\Helm.exe'
if (-not (Test-Path $desktopExe)) { throw "missing $desktopExe" }
$nsisSetup = Get-ChildItem (Join-Path $DesktopDir 'target\release\bundle\nsis') -Filter "Helm_$($Version)_x64-setup.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 1

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

$signedApk = $builtApk.Name -notmatch 'unsigned'
if ($haveKeys -and -not $signedApk) { throw "Gradle built an unsigned APK ($($builtApk.Name)) although the keystore exists" }
if ($signedApk) {
    $apksigner = Find-BuildTool 'apksigner.bat'
    if ($apksigner) {
        Invoke-Native $Dist $apksigner @('verify', $builtApk.FullName)
        Write-Host '  APK signature verifies'
    } else {
        Write-Host '  apksigner not found: APK signature not double-checked' -ForegroundColor Yellow
    }
    Copy-Item $builtApk.FullName (Join-Path $Dist 'Helm-android.apk')
} else {
    Copy-Item $builtApk.FullName (Join-Path $Dist 'Helm-android-unsigned.apk')
    Write-Host '  No keystore: APK is unsigned (test build)' -ForegroundColor Yellow
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
Copy-Item $desktopExe (Join-Path $Dist 'Helm-windows-x64.exe')
if ($nsisSetup) {
    Copy-Item $nsisSetup.FullName (Join-Path $Dist 'Helm-windows-x64-setup.exe')
} else {
    Write-Host '  No NSIS installer from cargo tauri build: shipping Helm-windows-x64.exe only' -ForegroundColor Yellow
}
Copy-Item $agentExe   (Join-Path $Dist 'helm-agent.exe')
Copy-Item $serviceExe (Join-Path $Dist 'helm-service.exe')

$files = @(Get-ChildItem $Dist -File | Where-Object { $_.Name -ne 'SHA256SUMS' -and $_.Extension -notin '.sig', '.sha256' } | Sort-Object Name)
if ($haveKeys) {
    Write-Stage 'Signing every asset (.sig + .sha256)'
    foreach ($f in $files) {
        Invoke-Native $Dist $HelmSign @('sign', $SignKey, $f.FullName)
        Invoke-Native $Dist $HelmSign @('verify', $expectedPub, $f.FullName)
    }
}
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
