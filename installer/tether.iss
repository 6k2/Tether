; Inno Setup script for tether. Built by .github/workflows/release.yml:
;   iscc /DAppVersion=0.1.1 installer\tether.iss
; Output: installer\Output\Tether-Setup-<version>.exe
;
; Running a newer installer upgrades in place (same AppId). The in-app updater runs it
; with /SILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH=1 and tether starts again at the end.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define AppName "Tether"
#define AppExe "tether.exe"

[Setup]
; Never change the AppId: it is what makes new installers upgrade old installs.
AppId={{6F1C2B7E-4E0A-4C51-9C8B-1A7D3E2F9B40}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=izLoki
AppPublisherURL=https://github.com/izLoki/Tether
AppSupportURL=https://github.com/izLoki/Tether/issues
AppUpdatesURL=https://github.com/izLoki/Tether/releases
VersionInfoVersion={#AppVersion}
; Per-user install: no admin prompt, and the updater can replace files on its own.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
UsePreviousAppDir=yes
UsePreviousTasks=yes
LicenseFile=..\LICENSE
SetupIconFile=..\assets\tether.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir=Output
OutputBaseFilename=Tether-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Close a running tether before replacing it (the updater quits on its own first).
CloseApplications=force
CloseApplicationsFilter={#AppExe}
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "izLoki.Tether"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
; Interactive install: the usual "Launch Tether" checkbox on the last page.
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
; Silent update from inside the app: start it again.
Filename: "{app}\{#AppExe}"; Flags: nowait runasoriginaluser; Check: ShouldRelaunch

[Code]
function ShouldRelaunch(): Boolean;
begin
  Result := WizardSilent() and (ExpandConstant('{param:RELAUNCH|0}') = '1');
end;
