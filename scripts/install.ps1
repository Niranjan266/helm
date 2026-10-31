# Helm installer for Windows: downloads the newest release from GitHub, verifies it and starts it.
# Usage:  irm https://raw.githubusercontent.com/Niranjan266/helm/main/scripts/install.ps1 | iex
$ErrorActionPreference = 'Stop'
$repo = 'Niranjan266/helm'
$asset = 'Helm-agent-windows-x64.exe'
$dir = Join-Path $env:LOCALAPPDATA 'Programs\Helm'
$exe = Join-Path $dir 'helm-agent.exe'

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$headers = @{ 'User-Agent' = 'helm-installer' }

# Newest release, including previews.
$release = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases?per_page=10" -Headers $headers) |
    Where-Object { -not $_.draft } | Select-Object -First 1
if (-not $release) { throw "No Helm release found on GitHub." }
$exeUrl = ($release.assets | Where-Object name -eq $asset).browser_download_url
$sumUrl = ($release.assets | Where-Object name -eq 'SHA256SUMS.txt').browser_download_url
if (-not $exeUrl) { throw "Release $($release.tag_name) has no $asset." }
Write-Host "Installing Helm $($release.tag_name)..."

$tmp = Join-Path $env:TEMP "helm-$([guid]::NewGuid()).exe"
Invoke-WebRequest $exeUrl -OutFile $tmp -Headers $headers -UseBasicParsing
if ($sumUrl) {
    $sums = (Invoke-WebRequest $sumUrl -Headers $headers -UseBasicParsing).Content
    $expected = ($sums -split "`n" | Where-Object { $_ -match [regex]::Escape($asset) } | Select-Object -First 1) -replace '\s.*$', ''
    $actual = (Get-FileHash $tmp -Algorithm SHA256).Hash.ToLower()
    if ($expected -and $expected.Trim().ToLower() -ne $actual) {
        Remove-Item $tmp -Force
        throw "Checksum mismatch - download was not installed."
    }
    Write-Host "Checksum OK."
}

# Stop a running copy, then replace it. Paired phones and settings in %APPDATA%\Helm are kept.
Get-Process helm-agent -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Force $dir | Out-Null
Move-Item $tmp $exe -Force
Unblock-File $exe

$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Helm.lnk'
$link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
$link.TargetPath = $exe
$link.Arguments = '--background'
$link.Save()

Start-Process $exe -ArgumentList '--background'
Write-Host "Helm is running - look for its icon in the system tray (right-click > Show pairing QR)."
Write-Host "If Windows Firewall asks, allow Helm on Private networks."
