# Helm installer for Windows: downloads the newest release from GitHub, verifies it and starts it.
# Usage:  irm https://raw.githubusercontent.com/Niranjan266/helm/main/scripts/install.ps1 | iex
#
# Installs Helm.exe (the desktop app: window + tray, runs the agent inside) to
# %LOCALAPPDATA%\Programs\Helm. Paired phones and settings in %APPDATA%\Helm are kept.
$ErrorActionPreference = 'Stop'
$repo = 'Niranjan266/helm'
$asset = 'Helm-windows-x64.exe'
$legacyAsset = 'Helm-agent-windows-x64.exe'   # console agent, releases before the desktop app
$dir = Join-Path $env:LOCALAPPDATA 'Programs\Helm'
$exe = Join-Path $dir 'Helm.exe'
$legacyExe = Join-Path $dir 'helm-agent.exe'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$headers = @{ 'User-Agent' = 'helm-installer' }

# Newest release, including previews.
$release = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases?per_page=10" -Headers $headers) |
    Where-Object { -not $_.draft } | Select-Object -First 1
if (-not $release) { throw "No Helm release found on GitHub." }
$pick = $asset
$exeUrl = ($release.assets | Where-Object name -eq $asset).browser_download_url
if (-not $exeUrl) {
    # Older release without the desktop app: fall back to the console agent.
    $pick = $legacyAsset
    $exeUrl = ($release.assets | Where-Object name -eq $legacyAsset).browser_download_url
    $exe = $legacyExe
}
if (-not $exeUrl) { throw "Release $($release.tag_name) has no $asset." }
$sumUrl = ($release.assets | Where-Object { $_.name -eq 'SHA256SUMS' -or $_.name -eq 'SHA256SUMS.txt' } |
    Select-Object -First 1).browser_download_url
Write-Host "Installing Helm $($release.tag_name)..."

$tmp = Join-Path $env:TEMP "helm-$([guid]::NewGuid()).exe"
Invoke-WebRequest $exeUrl -OutFile $tmp -Headers $headers -UseBasicParsing
if ($sumUrl) {
    $sums = (Invoke-WebRequest $sumUrl -Headers $headers -UseBasicParsing).Content
    if ($sums -is [byte[]]) { $sums = [Text.Encoding]::UTF8.GetString($sums) }
    $line = $sums -split "`n" | Where-Object { $_ -match "\s\*?$([regex]::Escape($pick))\s*$" } | Select-Object -First 1
    $expected = if ($line) { ($line -replace '\s.*$', '').Trim().ToLower() } else { '' }
    $actual = (Get-FileHash $tmp -Algorithm SHA256).Hash.ToLower()
    if (-not $expected) {
        Remove-Item $tmp -Force
        throw "SHA256SUMS has no entry for $pick - download was not installed."
    }
    if ($expected -ne $actual) {
        Remove-Item $tmp -Force
        throw "Checksum mismatch - download was not installed."
    }
    Write-Host "Checksum OK."
}

# Stop running copies (desktop app and the old console agent), then replace.
Get-Process Helm, helm-agent -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500
New-Item -ItemType Directory -Force $dir | Out-Null
Move-Item $tmp $exe -Force
Unblock-File $exe

$desktop = $pick -eq $asset
if ($desktop) {
    # The desktop app replaces the console agent installed by earlier versions of this script.
    if (Test-Path $legacyExe) { Remove-Item $legacyExe -Force -ErrorAction SilentlyContinue }
    # Keep "Start with Windows" on if it was on for the old agent, now pointing at Helm.exe.
    $old = (Get-ItemProperty $runKey -Name Helm -ErrorAction SilentlyContinue).Helm
    if ($old) { Set-ItemProperty $runKey -Name Helm -Value "`"$exe`" --hidden" }
}

$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Helm.lnk'
$link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
$link.TargetPath = $exe
$link.Arguments = if ($desktop) { '' } else { '--background' }
$link.IconLocation = "$exe,0"
$link.Save()

if ($desktop) {
    Start-Process $exe
    Write-Host "Helm is running. Its window shows the pairing QR; closing the window keeps Helm in the tray."
} else {
    Start-Process $exe -ArgumentList '--background'
    Write-Host "Helm is running - look for its icon in the system tray (right-click > Show pairing QR)."
}
Write-Host "If Windows Firewall asks, allow Helm on Private networks."
