<#
.SYNOPSIS
    Genera las imágenes del README (docs/images/*.png) con datos ficticios.
.DESCRIPTION
    Ejecuta `taskbar-panels.exe --render-demo` (no necesita la barra de tareas ni datos reales:
    IP de documentación, ISP y nombres genéricos) y convierte los BMP a PNG.
#>
param([string]$Exe = (Join-Path $PSScriptRoot '..\target\release\taskbar-panels.exe'))

$ErrorActionPreference = 'Stop'
$out = Join-Path $PSScriptRoot '..\docs\images'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("tp-demo-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $out, $tmp | Out-Null

Start-Process -FilePath $Exe -ArgumentList '--render-demo', "`"$tmp`"" -Wait  # app gráfica: hay que esperar a que termine

Add-Type -AssemblyName System.Drawing
Get-ChildItem $tmp -Filter *.bmp | ForEach-Object {
    $img = [System.Drawing.Image]::FromFile($_.FullName)
    $png = Join-Path $out ($_.BaseName + '.png')
    $img.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
    "{0,-24} {1}x{2}" -f ($_.BaseName + '.png'), $img.Width, $img.Height
    $img.Dispose()
}
Remove-Item $tmp -Recurse -Force
