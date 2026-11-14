<#
.SYNOPSIS
    Compila en modo release y genera los paquetes para publicar: dist\TaskbarPanels-v<versión>-x64.zip
    y el instalador dist\TaskbarPanels-Setup-v<versión>-x64.exe (Inno Setup 6), cada uno con su .sha256
.DESCRIPTION
    El zip contiene el ejecutable (+ .pdb para pilas legibles en crash.log), los scripts de
    instalación y la documentación. Se usa localmente y en el workflow de GitHub Actions.
#>
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Push-Location $root
try {
    $version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
    Write-Host "→ Compilando taskbar-panels v$version" -ForegroundColor Cyan
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build falló' }

    $name = "TaskbarPanels-v$version-x64"
    $stage = Join-Path 'dist' $name
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item 'target\release\taskbar-panels.exe', 'scripts\install.ps1', 'scripts\uninstall.ps1', 'README.md', 'README.es.md', 'LICENSE', 'CHANGELOG.md', 'CHANGELOG.es.md' $stage
    # el .pdb conserva su nombre: el ejecutable lo busca por ese nombre para que crash.log tenga funciones legibles
    if (Test-Path 'target\release\taskbar_panels.pdb') { Copy-Item 'target\release\taskbar_panels.pdb' $stage }

    $zip = Join-Path 'dist' "$name.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    "$hash  $name.zip" | Set-Content "$zip.sha256" -Encoding ascii
    Write-Host "✓ $zip ($([math]::Round((Get-Item $zip).Length / 1MB, 2)) MB)" -ForegroundColor Green
    Write-Host "  SHA256 $hash"

    # ---- instalador (Inno Setup 6): ISCC en el PATH, en la variable ISCC o en las rutas habituales
    $iscc = @($env:ISCC, (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source,
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if ($iscc) {
        & $iscc /Q "/DAppVersion=$version" 'installer\TaskbarPanels.iss'
        if ($LASTEXITCODE -ne 0) { throw 'ISCC falló' }
        $setup = Join-Path 'dist' "TaskbarPanels-Setup-v$version-x64.exe"
        $sh = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLower()
        "$sh  $(Split-Path $setup -Leaf)" | Set-Content "$setup.sha256" -Encoding ascii
        Write-Host "✓ $setup ($([math]::Round((Get-Item $setup).Length / 1MB, 2)) MB)" -ForegroundColor Green
        Write-Host "  SHA256 $sh"
    } else {
        Write-Warning 'Inno Setup 6 no está instalado: se omite el instalador (winget install JRSoftware.InnoSetup)'
    }
} finally {
    Pop-Location
}
