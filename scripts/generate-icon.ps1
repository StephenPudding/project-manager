# Generates our original vector mark at Windows icon sizes. No project screenshots are used.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$assetDirectory = Join-Path (Split-Path -Parent $PSScriptRoot) 'assets'
function Add-RoundedRectangle($path, [single]$x, [single]$y, [single]$w, [single]$h, [single]$r) {
    $diameter = 2 * $r
    $path.AddArc($x, $y, $diameter, $diameter, 180, 90)
    $path.AddArc(($x+$w-$diameter), $y, $diameter, $diameter, 270, 90)
    $path.AddArc(($x+$w-$diameter), ($y+$h-$diameter), $diameter, $diameter, 0, 90)
    $path.AddArc($x, ($y+$h-$diameter), $diameter, $diameter, 90, 90)
    $path.CloseFigure()
}
$frames = [System.Collections.Generic.List[byte[]]]::new()
$sizes = @(16,20,24,32,40,48,64,128,256)
foreach ($size in $sizes) {
    $bitmap = [System.Drawing.Bitmap]::new($size*4, $size*4)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.ScaleTransform($size/64.0, $size/64.0)
    $dark = [System.Drawing.SolidBrush]::new([System.Drawing.ColorTranslator]::FromHtml('#27352c'))
    $light = [System.Drawing.SolidBrush]::new([System.Drawing.ColorTranslator]::FromHtml('#c3dc9d'))
    $pen = [System.Drawing.Pen]::new($light,13)
    $shape = [System.Drawing.Drawing2D.GraphicsPath]::new()
    Add-RoundedRectangle $shape 8 8 240 240 58
    $graphics.FillPath($dark,$shape)
    $shape.Dispose()
    foreach ($box in @(@(61,61,50,50),@(145,61,50,92),@(61,145,50,50))) {
        $shape = [System.Drawing.Drawing2D.GraphicsPath]::new()
        Add-RoundedRectangle $shape $box[0] $box[1] $box[2] $box[3] 7
        $graphics.DrawPath($pen,$shape)
        $shape.Dispose()
    }
    $shape = [System.Drawing.Drawing2D.GraphicsPath]::new()
    Add-RoundedRectangle $shape 139 176 62 26 9
    $graphics.FillPath($light,$shape)
    $shape.Dispose()
    $small = [System.Drawing.Bitmap]::new($size,$size)
    $scaled = [System.Drawing.Graphics]::FromImage($small)
    $scaled.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $scaled.DrawImage($bitmap,0,0,$size,$size)
    $stream = [System.IO.MemoryStream]::new()
    $small.Save($stream,[System.Drawing.Imaging.ImageFormat]::Png)
    $frames.Add($stream.ToArray())
    if ($size -eq 256) { [System.IO.File]::WriteAllBytes((Join-Path $assetDirectory 'app-icon.png'),$stream.ToArray()) }
    $stream.Dispose(); $scaled.Dispose(); $small.Dispose(); $graphics.Dispose(); $bitmap.Dispose(); $dark.Dispose(); $light.Dispose(); $pen.Dispose()
}
$file = [System.IO.File]::Create((Join-Path $assetDirectory 'app-icon.ico'))
$writer = [System.IO.BinaryWriter]::new($file)
try {
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$sizes.Count)
    $offset = 6 + 16*$sizes.Count
    for ($index=0; $index -lt $sizes.Count; $index++) {
        $dimension = if ($sizes[$index] -eq 256) { 0 } else { $sizes[$index] }
        $writer.Write([byte]$dimension); $writer.Write([byte]$dimension); $writer.Write([uint16]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32)
        $writer.Write([uint32]$frames[$index].Length); $writer.Write([uint32]$offset)
        $offset += $frames[$index].Length
    }
    foreach ($frame in $frames) { $writer.Write($frame) }
} finally { $writer.Dispose(); $file.Dispose() }
