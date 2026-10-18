# Renders the app's icons: a pulse line with one tall spike on a rounded square,
# one colour per tray state. Each size is drawn on its own rather than scaled down
# from a big image, so the 16 px version stays crisp.
#
# Usage: pwsh -File assets/make-icons.ps1   (writes idle.ico, alert.ico, paused.ico next to this script)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$sizes = 16, 20, 24, 32, 40, 48, 64, 256
$states = [ordered]@{
    idle   = '#0E7C66'  # deep teal: all clear
    alert  = '#D9480F'  # orange-red: anomalies found
    paused = '#6B7280'  # grey: monitoring paused
}

# The pulse, in fractions of the icon size: flat, tall spike, deep dip, small rebound, flat.
$pulse = @(
    @(0.14, 0.58), @(0.36, 0.58), @(0.44, 0.24), @(0.54, 0.80), @(0.61, 0.50), @(0.66, 0.58), @(0.86, 0.58)
)
# Small sizes get a simpler, wider zigzag: at 16 px the full pulse's up and down
# strokes are under two pixels apart and merge into a plus sign.
$smallBaseline = 0.56
$pulseSmall = @(
    @(0.10, $smallBaseline), @(0.30, $smallBaseline), @(0.40, 0.18), @(0.60, 0.84), @(0.70, $smallBaseline), @(0.90, $smallBaseline)
)

function New-IconPng([int]$size, [string]$color) {
    $bitmap = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bitmap)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)

    # Rounded square, inset half a pixel so the edge antialiases inside the canvas.
    $inset = 0.5
    $side = $size - 2 * $inset
    $radius = [Math]::Max(2.0, $size * 0.22)
    $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $path.AddArc($inset, $inset, 2 * $radius, 2 * $radius, 180, 90)
    $path.AddArc($inset + $side - 2 * $radius, $inset, 2 * $radius, 2 * $radius, 270, 90)
    $path.AddArc($inset + $side - 2 * $radius, $inset + $side - 2 * $radius, 2 * $radius, 2 * $radius, 0, 90)
    $path.AddArc($inset, $inset + $side - 2 * $radius, 2 * $radius, 2 * $radius, 90, 90)
    $path.CloseFigure()
    $brush = [System.Drawing.SolidBrush]::new([System.Drawing.ColorTranslator]::FromHtml($color))
    $g.FillPath($brush, $path)

    # The pulse: thick enough to read at 16 px, proportionally thinner when large.
    # Small sizes use an exactly 2 px line whose flat parts sit on a pixel boundary,
    # so they cover two full pixel rows instead of smearing over three.
    $small = $size -le 24
    $width = if ($small) { 2.0 } else { $size * 0.095 }
    $pen = [System.Drawing.Pen]::new([System.Drawing.Color]::White, $width)
    $pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $shape = if ($small) { $pulseSmall } else { $pulse }
    $points = $shape | ForEach-Object {
        $y = if ($small -and $_[1] -eq $smallBaseline) { [Math]::Round($smallBaseline * $size) } else { $_[1] * $size }
        [System.Drawing.PointF]::new($_[0] * $size, $y)
    }
    $g.DrawLines($pen, [System.Drawing.PointF[]]$points)

    $stream = [System.IO.MemoryStream]::new()
    $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bitmap.Dispose(); $brush.Dispose(); $pen.Dispose(); $path.Dispose()
    , $stream.ToArray()
}

# ICO container with PNG-compressed frames (supported since Windows Vista).
function Write-Ico([string]$file, [byte[][]]$frames, [int[]]$frameSizes) {
    $out = [System.IO.BinaryWriter]::new([System.IO.File]::Create($file))
    $out.Write([uint16]0); $out.Write([uint16]1); $out.Write([uint16]$frames.Count)
    $offset = 6 + 16 * $frames.Count
    for ($i = 0; $i -lt $frames.Count; $i++) {
        $dim = if ($frameSizes[$i] -ge 256) { 0 } else { $frameSizes[$i] }  # 0 means 256
        $out.Write([byte]$dim); $out.Write([byte]$dim); $out.Write([byte]0); $out.Write([byte]0)
        $out.Write([uint16]1); $out.Write([uint16]32)
        $out.Write([uint32]$frames[$i].Length); $out.Write([uint32]$offset)
        $offset += $frames[$i].Length
    }
    foreach ($frame in $frames) { $out.Write($frame) }
    $out.Dispose()
}

foreach ($state in $states.Keys) {
    $frames = [byte[][]]($sizes | ForEach-Object { , (New-IconPng $_ $states[$state]) })
    $file = Join-Path $PSScriptRoot "$state.ico"
    Write-Ico $file $frames $sizes
    "{0,-8} {1,6:N0} bytes" -f "$state.ico", (Get-Item $file).Length
}
