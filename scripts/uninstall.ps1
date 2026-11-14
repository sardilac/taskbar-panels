<#
.SYNOPSIS
    Desinstala Taskbar Panels.
.DESCRIPTION
    Detiene el programa, elimina la tarea programada "TaskbarPanels" y la carpeta de instalación.
    La configuración del usuario (%APPDATA%\TaskbarPanels) se conserva salvo que se use -RemoveSettings.
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:ProgramFiles 'TaskbarPanels'),
    [switch]$RemoveSettings,
    [string]$SettingsDir = (Join-Path $env:APPDATA 'TaskbarPanels')
)

$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    $shell = (Get-Process -Id $PID).Path
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-InstallDir', "`"$InstallDir`"", '-SettingsDir', "`"$SettingsDir`"")
    if ($RemoveSettings) { $argList += '-RemoveSettings' }
    $p = Start-Process -FilePath $shell -Verb RunAs -ArgumentList $argList -Wait -PassThru
    exit $p.ExitCode
}

Stop-ScheduledTask -TaskName 'TaskbarPanels' -ErrorAction SilentlyContinue
Get-Process taskbar-panels -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Get-Process PresentMon* -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$InstallDir*" } | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
Unregister-ScheduledTask -TaskName 'TaskbarPanels' -Confirm:$false -ErrorAction SilentlyContinue

if (Test-Path $InstallDir) {
    # el script puede estar dentro de la carpeta: se borra todo lo demás y la carpeta al final si es posible
    Get-ChildItem $InstallDir -Recurse | Where-Object { $_.FullName -ne $PSCommandPath } | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
}
if ($RemoveSettings -and (Test-Path $SettingsDir)) { Remove-Item $SettingsDir -Recurse -Force }

Write-Host 'Taskbar Panels desinstalado.' -ForegroundColor Green
if (-not $RemoveSettings) { Write-Host "  La configuración se conservó en $SettingsDir (usa -RemoveSettings para borrarla)." }
