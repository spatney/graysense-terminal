<#
.SYNOPSIS
    Renders Graysense's icon, in-app logo and installer splash from the master logo.

.DESCRIPTION
    assets\logo.png is the master logo: a square PNG with a transparent background. From it:

      assets\graysense.ico          every size Windows uses (exe, installer, shortcuts)
      assets\logo-64.png            the window and taskbar icon
      assets\logo-128.png           the logo inside the app (title bar, start page, About)
      assets\installer-splash.png   shown while Setup.exe installs

    -Import makes a new master from a generated image first: it clears near-invisible background
    haze and crops to the icon's tile with an even margin.

    pwsh scripts\render-assets.ps1 [-Import generated.png]
#>
param([string]$Import)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$assets = Join-Path (Split-Path -Parent $PSScriptRoot) 'assets'
$master = Join-Path $assets 'logo.png'

function Get-Pixels([System.Drawing.Bitmap]$Bmp) {
    $rect = New-Object System.Drawing.Rectangle 0, 0, $Bmp.Width, $Bmp.Height
    $data = $Bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $buf = New-Object byte[] ($data.Stride * $Bmp.Height)
    [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $buf, 0, $buf.Length)
    $Bmp.UnlockBits($data)
    , $buf
}

function Set-Pixels([System.Drawing.Bitmap]$Bmp, [byte[]]$Buf) {
    $rect = New-Object System.Drawing.Rectangle 0, 0, $Bmp.Width, $Bmp.Height
    $data = $Bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::WriteOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    [System.Runtime.InteropServices.Marshal]::Copy($Buf, 0, $data.Scan0, $Buf.Length)
    $Bmp.UnlockBits($data)
}

function Resize-Image([System.Drawing.Image]$Src, [int]$Size) {
    $out = New-Object System.Drawing.Bitmap $Size, $Size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($out)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $attrs = New-Object System.Drawing.Imaging.ImageAttributes
    $attrs.SetWrapMode([System.Drawing.Drawing2D.WrapMode]::TileFlipXY)
    $g.DrawImage($Src, (New-Object System.Drawing.Rectangle 0, 0, $Size, $Size), 0, 0, $Src.Width, $Src.Height, [System.Drawing.GraphicsUnit]::Pixel, $attrs)
    $g.Dispose()
    $out
}

function Get-PngBytes([System.Drawing.Image]$Image) {
    $ms = New-Object System.IO.MemoryStream
    $Image.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    , $ms.ToArray()
}

# --- Import: clean and crop a generated logo into the master.
if ($Import) {
    $src = New-Object System.Drawing.Bitmap ([System.Drawing.Bitmap]::FromFile((Resolve-Path $Import)))
    $w = $src.Width; $h = $src.Height
    $px = Get-Pixels $src
    $stride = $w * 4
    $minX = $w; $minY = $h; $maxX = 0; $maxY = 0
    for ($y = 0; $y -lt $h; $y++) {
        for ($x = 0; $x -lt $w; $x++) {
            $i = $y * $stride + $x * 4 + 3
            if ($px[$i] -lt 8) { $px[$i] = 0; continue }
            if ($px[$i] -ge 128) {
                if ($x -lt $minX) { $minX = $x }; if ($x -gt $maxX) { $maxX = $x }
                if ($y -lt $minY) { $minY = $y }; if ($y -gt $maxY) { $maxY = $y }
            }
        }
    }
    Set-Pixels $src $px
    # A square around the tile, with a margin of 3% of its size on every side.
    $tile = [Math]::Max($maxX - $minX + 1, $maxY - $minY + 1)
    $side = [int]([Math]::Ceiling($tile * 1.06 / 16) * 16)
    $x0 = [int][Math]::Round(($minX + $maxX + 1 - $side) / 2)
    $y0 = [int][Math]::Round(($minY + $maxY + 1 - $side) / 2)
    $crop = New-Object System.Drawing.Bitmap $side, $side, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($crop)
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.DrawImage($src, (New-Object System.Drawing.Rectangle 0, 0, $side, $side), $x0, $y0, $side, $side, [System.Drawing.GraphicsUnit]::Pixel)
    $g.Dispose(); $src.Dispose()
    $crop.Save($master, [System.Drawing.Imaging.ImageFormat]::Png)
    $crop.Dispose()
    "imported $Import -> $master (tile ${tile}px, master ${side}px)"
}

$logo = [System.Drawing.Bitmap]::FromFile($master)

# --- Icon: PNG-compressed entries (Windows Vista and later), every size Windows asks for.
$sizes = 16, 20, 24, 32, 40, 48, 64, 96, 128, 256
$images = foreach ($size in $sizes) {
    $img = Resize-Image $logo $size
    [pscustomobject]@{ Size = $size; Png = (Get-PngBytes $img) }
    $img.Dispose()
}
$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter $ms
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($im in $images) {
    $dim = if ($im.Size -ge 256) { 0 } else { $im.Size }
    $bw.Write([byte]$dim); $bw.Write([byte]$dim); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32)
    $bw.Write([uint32]$im.Png.Length); $bw.Write([uint32]$offset)
    $offset += $im.Png.Length
}
foreach ($im in $images) { $bw.Write([byte[]]$im.Png) }
$bw.Flush()
$ico = Join-Path $assets 'graysense.ico'
[System.IO.File]::WriteAllBytes($ico, $ms.ToArray())
"wrote $ico ($([int]($ms.Length / 1KB)) KB, sizes $($sizes -join ', '))"

foreach ($size in 64, 128) {
    $img = Resize-Image $logo $size
    $path = Join-Path $assets "logo-$size.png"
    $img.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $img.Dispose()
    "wrote $path"
}

# --- Installer splash: the logo, softly lit, and the wordmark on the Graphite background.
# Velopack draws its progress bar along the bottom edge, so that strip stays empty.
$W = 560; $H = 320
$splash = New-Object System.Drawing.Bitmap $W, $H
$g = [System.Drawing.Graphics]::FromImage($splash)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.Clear([System.Drawing.Color]::FromArgb(0x0F, 0x10, 0x12))

$logoSize = 104
$top = 50
$cx = $W / 2
$glow = New-Object System.Drawing.Drawing2D.GraphicsPath
$glow.AddEllipse($cx - 150, $top + $logoSize / 2 - 110, 300, 220)
$brush = New-Object System.Drawing.Drawing2D.PathGradientBrush $glow
$brush.CenterColor = [System.Drawing.Color]::FromArgb(70, 0x6A, 0x6E, 0xFF)
$brush.SurroundColors = @([System.Drawing.Color]::FromArgb(0, 0x0F, 0x10, 0x12))
$g.FillPath($brush, $glow)
$small = Resize-Image $logo $logoSize
$g.DrawImage($small, [int]($cx - $logoSize / 2), $top, $logoSize, $logoSize)
$small.Dispose()

$families = (New-Object System.Drawing.Text.InstalledFontCollection).Families.Name
$semibold = @('Segoe UI Variable Display Semib', 'Segoe UI Semibold', 'Segoe UI') | Where-Object { $families -contains $_ } | Select-Object -First 1
$regular = @('Segoe UI Variable Text', 'Segoe UI') | Where-Object { $families -contains $_ } | Select-Object -First 1
$center = New-Object System.Drawing.StringFormat
$center.Alignment = [System.Drawing.StringAlignment]::Center
$title = New-Object System.Drawing.Font $semibold, 26, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$sub = New-Object System.Drawing.Font $regular, 13.5, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$g.DrawString('Graysense', $title, (New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0xE6, 0xE8, 0xEB))), (New-Object System.Drawing.RectangleF 0, ($top + $logoSize + 18), $W, 40), $center)
$g.DrawString('A fast, flat terminal for Copilot agents', $sub, (New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0x9B, 0xA1, 0xAA))), (New-Object System.Drawing.RectangleF 0, ($top + $logoSize + 58), $W, 24), $center)
$border = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(0x26, 0x29, 0x2E)), 1
$g.DrawRectangle($border, 0, 0, $W - 1, $H - 1)
$g.Dispose()
$png = Join-Path $assets 'installer-splash.png'
$splash.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
$splash.Dispose()
$logo.Dispose()
"wrote $png (${W}x${H})"
