# Renders the Helm app icon (icons/helm-app.svg drawn with GDI+) to icons/app-icon.png (1024 px).
# Then:  cargo tauri icon icons/app-icon.png   (writes icon.ico, icon.png and the sized PNGs)
# Usage: powershell -NoProfile -File host/desktop/icons/render-icon.ps1
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$size = 1024
$out = Join-Path $PSScriptRoot 'app-icon.png'
$bmp = New-Object System.Drawing.Bitmap($size, $size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
$g.Clear([System.Drawing.Color]::Transparent)

$paper = [System.Drawing.ColorTranslator]::FromHtml('#0d3a86')
$pencil = [System.Drawing.ColorTranslator]::FromHtml('#ffd84d')

function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

# Blue tile (same as helm-app.svg: 48-unit viewBox, tile 1..47, radius 10).
$u = $size / 48.0
$g.FillPath((New-Object System.Drawing.SolidBrush($paper)), (RoundRect (1 * $u) (1 * $u) (46 * $u) (46 * $u) (10 * $u)))

# The mark (website Logo.jsx geometry), scaled to 78% around the centre.
$s = $u * 0.78
$o = $size / 2 - 24 * $s
function X([float]$v) { return $o + $v * $s }
$brush = New-Object System.Drawing.SolidBrush($pencil)
$g.FillPath($brush, (RoundRect (X 6) (X 6) (7 * $s) (36 * $s) (3.5 * $s)))
$g.FillPath($brush, (RoundRect (X 35) (X 6) (7 * $s) (36 * $s) (3.5 * $s)))
$pen = New-Object System.Drawing.Pen($pencil, (3 * $s))
$pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
$g.DrawEllipse($pen, (X 16.5), (X 16.5), (15 * $s), (15 * $s))
$g.DrawLine($pen, (X 13), (X 24), (X 16.5), (X 24))
$g.DrawLine($pen, (X 31.5), (X 24), (X 35), (X 24))
$g.DrawLine($pen, (X 24), (X 13), (X 24), (X 16.5))
$g.DrawLine($pen, (X 24), (X 31.5), (X 24), (X 35))
foreach ($cy in @(24, 11, 37)) {
    $g.FillEllipse($brush, (X (24 - 2.2)), (X ($cy - 2.2)), (4.4 * $s), (4.4 * $s))
}

$g.Dispose()
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host "wrote $out"
