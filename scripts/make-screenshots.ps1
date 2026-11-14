<#
.SYNOPSIS
    Genera las imágenes de los README con datos ficticios: docs/images/*.png en inglés (README.md)
    y docs/images/es/*.png en español (README.es.md).
.DESCRIPTION
    Ejecuta `taskbar-panels.exe --render-demo <carpeta> <idioma>` (no necesita la barra de tareas ni
    datos reales: IP de documentación, ISP y nombres genéricos) y convierte los BMP a PNG.
#>
param([string]$Exe = (Join-Path $PSScriptRoot '..\target\release\taskbar-panels.exe'))

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$base = Join-Path $PSScriptRoot '..\docs\images'

foreach ($lang in 'en', 'es') {
    $out = if ($lang -eq 'en') { $base } else { Join-Path $base $lang }
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("tp-demo-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Force $out, $tmp | Out-Null
    Start-Process -FilePath $Exe -ArgumentList '--render-demo', "`"$tmp`"", $lang -Wait  # app gráfica: hay que esperar a que termine
    Get-ChildItem $tmp -Filter *.bmp | ForEach-Object {
        $img = [System.Drawing.Image]::FromFile($_.FullName)
        $png = Join-Path $out ($_.BaseName + '.png')
        $img.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
        "[{0}] {1,-24} {2}x{3}" -f $lang, ($_.BaseName + '.png'), $img.Width, $img.Height
        $img.Dispose()
    }
    Remove-Item $tmp -Recurse -Force
}
