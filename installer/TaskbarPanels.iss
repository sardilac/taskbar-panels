; Instalador de Taskbar Panels (Inno Setup 6). Se compila con scripts\package.ps1:
;   ISCC.exe /DAppVersion=1.3.0 installer\TaskbarPanels.iss
; Instalación silenciosa: TaskbarPanels-Setup-v1.3.0-x64.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
; (añadir /TASKS="" para no descargar PresentMon)

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppName "Taskbar Panels"
#define AppExe "taskbar-panels.exe"
#define Helper "installer-helper.ps1"
#define PS "{sys}\WindowsPowerShell\v1.0\powershell.exe"

[Setup]
; AppId fijo: identifica el producto para actualizar/desinstalar (y para winget: ProductCode "<AppId>_is1")
AppId={{7E1C9F3A-5B2D-4C8E-9A61-3F0D2B7C4E15}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=sardilac
AppPublisherURL=https://github.com/sardilac/taskbar-panels
AppSupportURL=https://github.com/sardilac/taskbar-panels/issues
AppUpdatesURL=https://github.com/sardilac/taskbar-panels/releases
AppCopyright=MIT License
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\TaskbarPanels
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir=..\dist
OutputBaseFilename=TaskbarPanels-Setup-v{#AppVersion}-x64
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
LicenseFile=..\LICENSE
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=no
RestartApplications=no
ShowLanguageDialog=no
LanguageDetectionMethod=uilanguage

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"

[CustomMessages]
en.PresentMonTask=Download Intel PresentMon to measure FPS (verified digital signature)
es.PresentMonTask=Descargar PresentMon de Intel para medir FPS (firma digital verificada)
en.Optional=Optional:
es.Optional=Opcional:
en.StatusPresentMon=Downloading PresentMon...
es.StatusPresentMon=Descargando PresentMon...
en.StatusTask=Registering the scheduled task and starting the panels...
es.StatusTask=Registrando la tarea programada e iniciando los paneles...

[Tasks]
Name: "presentmon"; Description: "{cm:PresentMonTask}"; GroupDescription: "{cm:Optional}"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\taskbar_panels.pdb"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\scripts\{#Helper}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.es.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; restos de la instalación por script (install.ps1) que el instalador no gestiona
Type: files; Name: "{app}\uninstall.ps1"

[Run]
Filename: "{#PS}"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\{#Helper}"" -Action PresentMon -InstallDir ""{app}"""; Flags: runhidden waituntilterminated; Tasks: presentmon; StatusMsg: "{cm:StatusPresentMon}"
Filename: "{#PS}"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\{#Helper}"" -Action Register -InstallDir ""{app}"""; Flags: runhidden waituntilterminated; StatusMsg: "{cm:StatusTask}"

[UninstallRun]
Filename: "{#PS}"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\{#Helper}"" -Action Unregister -InstallDir ""{app}"""; Flags: runhidden waituntilterminated; RunOnceId: "UnregisterTask"

[UninstallDelete]
; PresentMon lo descarga el instalador (no está en [Files])
Type: filesandordirs; Name: "{app}\tools"

[Code]
// Antes de copiar: detener la tarea y el proceso para que el .exe no esté bloqueado
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\schtasks.exe'), '/End /TN TaskbarPanels', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM taskbar-panels.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(800);
  Result := '';
end;
