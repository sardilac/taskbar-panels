<#
.SYNOPSIS
    Compila en modo release y genera el paquete para publicar: dist\TaskbarPanels-v<versión>-x64.zip
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
    Copy-Item 'target\release\taskbar-panels.exe', 'scripts\install.ps1', 'scripts\uninstall.ps1', 'README.md', 'LICENSE', 'CHANGELOG.md' $stage
    # el .pdb conserva su nombre: el ejecutable lo busca por ese nombre para que crash.log tenga funciones legibles
    if (Test-Path 'target\release\taskbar_panels.pdb') { Copy-Item 'target\release\taskbar_panels.pdb' $stage }

    $zip = Join-Path 'dist' "$name.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    "$hash  $name.zip" | Set-Content "$zip.sha256" -Encoding ascii
    Write-Host "✓ $zip ($([math]::Round((Get-Item $zip).Length / 1MB, 2)) MB)" -ForegroundColor Green
    Write-Host "  SHA256 $hash"
} finally {
    Pop-Location
}
