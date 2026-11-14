<#
.SYNOPSIS
    Instala Taskbar Panels en este equipo.
.DESCRIPTION
    - Copia taskbar-panels.exe a "C:\Program Files\TaskbarPanels" (o -InstallDir).
    - Descarga PresentMon (Intel, para medir FPS) desde su release oficial en GitHub y verifica
      que la firma digital sea válida y de Intel Corporation. Se puede omitir con -SkipPresentMon.
    - Registra la tarea programada "TaskbarPanels" (al iniciar sesión, privilegios elevados:
      necesarios para los sensores de temperatura y para PresentMon) y la inicia.
    Se eleva solo con UAC si no se ejecuta como administrador.
    Compatible con Windows PowerShell 5.1 y PowerShell 7.
.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:ProgramFiles 'TaskbarPanels'),
    [string]$User = "$env:USERDOMAIN\$env:USERNAME",
    [switch]$SkipPresentMon
)

$ErrorActionPreference = 'Stop'
$TaskName = 'TaskbarPanels'

# ---------------------------------------------------------------- elevación
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    Write-Host 'Se necesitan permisos de administrador; se abrirá el aviso de UAC...'
    $shell = (Get-Process -Id $PID).Path
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-InstallDir', "`"$InstallDir`"", '-User', "`"$User`"")
    if ($SkipPresentMon) { $argList += '-SkipPresentMon' }
    $p = Start-Process -FilePath $shell -Verb RunAs -ArgumentList $argList -Wait -PassThru
    exit $p.ExitCode
}

function Step($msg) { Write-Host "→ $msg" -ForegroundColor Cyan }

# ---------------------------------------------------------------- ejecutable a instalar
$candidates = @(
    (Join-Path $PSScriptRoot 'taskbar-panels.exe'),                      # junto al script (zip del release)
    (Join-Path $PSScriptRoot '..\target\release\taskbar-panels.exe')     # repositorio compilado
)
$exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) { throw 'No se encontró taskbar-panels.exe junto al script ni en target\release (compila con "cargo build --release").' }
$exe = (Resolve-Path $exe).Path

# ---------------------------------------------------------------- detener versión anterior
Step 'Deteniendo instancias anteriores'
Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
$old = Get-Process taskbar-panels -ErrorAction SilentlyContinue
$old | Stop-Process -Force -ErrorAction SilentlyContinue
$old | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue   # el .exe debe quedar libre antes de copiarlo
Get-Process PresentMon* -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$InstallDir*" } | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

# ---------------------------------------------------------------- copiar archivos
Step "Copiando a $InstallDir"
New-Item -ItemType Directory -Force (Join-Path $InstallDir 'tools') | Out-Null
# reintentos: el archivo puede seguir bloqueado unos instantes tras cerrar el proceso
for ($i = 1; ; $i++) {
    try { Copy-Item $exe (Join-Path $InstallDir 'taskbar-panels.exe') -Force; break }
    catch { if ($i -ge 10) { throw }; Start-Sleep -Milliseconds 500 }
}
$pdb = Join-Path (Split-Path $exe) 'taskbar_panels.pdb'
if (Test-Path $pdb) { Copy-Item $pdb $InstallDir -Force }   # pilas legibles en crash.log
Copy-Item (Join-Path $PSScriptRoot 'uninstall.ps1') $InstallDir -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------- PresentMon (FPS)
$pm = Join-Path $InstallDir 'tools\PresentMon.exe'
if (-not $SkipPresentMon -and -not (Test-Path $pm)) {
    Step 'Descargando PresentMon (GameTechDev/PresentMon, release oficial)'
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        $rel = Invoke-RestMethod 'https://api.github.com/repos/GameTechDev/PresentMon/releases/latest' -Headers @{ 'User-Agent' = 'taskbar-panels-installer' }
        $asset = $rel.assets | Where-Object { $_.name -match '^PresentMon-[\d\.]+-x64\.exe$' } | Select-Object -First 1
        if (-not $asset) { throw "el release $($rel.tag_name) no tiene un ejecutable x64" }
        Invoke-WebRequest $asset.browser_download_url -OutFile $pm -UseBasicParsing
        $sig = Get-AuthenticodeSignature $pm
        if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'Intel Corporation') {
            Remove-Item $pm -Force
            throw "firma no válida ($($sig.Status)); se descartó el archivo"
        }
        Write-Host "  PresentMon $($rel.tag_name) verificado (firmado por Intel Corporation)"
    } catch {
        Write-Warning "No se pudo instalar PresentMon: $($_.Exception.Message). El panel funcionará sin FPS."
    }
}

# ---------------------------------------------------------------- limpieza de la versión PowerShell (si existía)
$startup = [Environment]::GetFolderPath('Startup')
'PublicIpTray.lnk', 'UsagePanel.lnk', 'MediaPanel.lnk' | ForEach-Object { Remove-Item (Join-Path $startup $_) -ErrorAction SilentlyContinue }
Unregister-ScheduledTask -TaskName 'SystemPanel' -Confirm:$false -ErrorAction SilentlyContinue

# ---------------------------------------------------------------- tarea programada
Step "Registrando la tarea '$TaskName' para $User"
$installed = Join-Path $InstallDir 'taskbar-panels.exe'
$act = New-ScheduledTaskAction -Execute $installed -WorkingDirectory $InstallDir
$trg = New-ScheduledTaskTrigger -AtLogOn -User $User
$prn = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Highest
$set = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -MultipleInstances IgnoreNew -Priority 5 -RestartCount 5 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName $TaskName -Action $act -Trigger $trg -Principal $prn -Settings $set -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName

Start-Sleep -Seconds 2
if (Get-Process taskbar-panels -ErrorAction SilentlyContinue) {
    Write-Host "`nTaskbar Panels instalado y en ejecución." -ForegroundColor Green
    Write-Host '  Clic derecho sobre cualquier panel para configurarlo.'
    Write-Host "  Configuración: $env:APPDATA\TaskbarPanels\config.json"
} else {
    Write-Warning "La tarea se registró pero el proceso no está en ejecución. Revisa $env:APPDATA\TaskbarPanels\crash.log"
}
