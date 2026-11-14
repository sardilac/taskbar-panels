<#
.SYNOPSIS
    Pasos del instalador (Inno Setup) que no son copiar archivos. Se ejecuta ya elevado.
.DESCRIPTION
    -Action Stop        detiene la tarea y el proceso (antes de copiar o desinstalar)
    -Action PresentMon  descarga PresentMon (Intel) del release oficial y verifica su firma
    -Action Register    registra la tarea programada "TaskbarPanels" para -User y la inicia
    -Action Unregister  detiene y elimina la tarea programada
    Compatible con Windows PowerShell 5.1. Los errores no detienen la instalación (código 0),
    salvo en Register, que devuelve 1 si no se pudo registrar la tarea.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Stop', 'PresentMon', 'Register', 'Unregister')][string]$Action,
    [string]$InstallDir = (Split-Path -Parent $PSScriptRoot),
    [string]$User = "$env:USERDOMAIN\$env:USERNAME"
)

$ErrorActionPreference = 'Stop'
$TaskName = 'TaskbarPanels'

function Stop-Panels {
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    $old = Get-Process taskbar-panels -ErrorAction SilentlyContinue
    $old | Stop-Process -Force -ErrorAction SilentlyContinue
    $old | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
    Get-Process PresentMon* -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$InstallDir*" } | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
}

switch ($Action) {
    'Stop' {
        Stop-Panels
    }
    'PresentMon' {
        $pm = Join-Path $InstallDir 'tools\PresentMon.exe'
        if (Test-Path $pm) { break }
        try {
            New-Item -ItemType Directory -Force (Split-Path $pm) | Out-Null
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
            $rel = Invoke-RestMethod 'https://api.github.com/repos/GameTechDev/PresentMon/releases/latest' -Headers @{ 'User-Agent' = 'taskbar-panels-installer' } -TimeoutSec 30
            $asset = $rel.assets | Where-Object { $_.name -match '^PresentMon-[\d\.]+-x64\.exe$' } | Select-Object -First 1
            if (-not $asset) { throw "el release $($rel.tag_name) no tiene un ejecutable x64" }
            Invoke-WebRequest $asset.browser_download_url -OutFile $pm -UseBasicParsing -TimeoutSec 120
            $sig = Get-AuthenticodeSignature $pm
            if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'Intel Corporation') {
                Remove-Item $pm -Force
                throw "firma no válida ($($sig.Status)); se descartó el archivo"
            }
        } catch {
            Write-Warning "PresentMon: $($_.Exception.Message)"
        }
    }
    'Register' {
        try {
            Stop-Panels
            # limpieza de la antigua versión PowerShell (si existía)
            $startup = [Environment]::GetFolderPath('Startup')
            'PublicIpTray.lnk', 'UsagePanel.lnk', 'MediaPanel.lnk' | ForEach-Object { Remove-Item (Join-Path $startup $_) -ErrorAction SilentlyContinue }
            Unregister-ScheduledTask -TaskName 'SystemPanel' -Confirm:$false -ErrorAction SilentlyContinue

            $exe = Join-Path $InstallDir 'taskbar-panels.exe'
            $act = New-ScheduledTaskAction -Execute $exe -WorkingDirectory $InstallDir
            $trg = New-ScheduledTaskTrigger -AtLogOn -User $User
            $prn = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Highest
            $set = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
                -MultipleInstances IgnoreNew -Priority 5 -RestartCount 5 -RestartInterval (New-TimeSpan -Minutes 1)
            Register-ScheduledTask -TaskName $TaskName -Action $act -Trigger $trg -Principal $prn -Settings $set -Force | Out-Null
            # arrancar ahora solo si el usuario tiene sesión interactiva (no en instalaciones como SYSTEM)
            Start-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        } catch {
            Write-Warning "Tarea programada: $($_.Exception.Message)"
            exit 1
        }
    }
    'Unregister' {
        Stop-Panels
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    }
}
exit 0
