; Windows installer for the Keyman desktop app (Inno Setup 6). Built by
; .github/workflows/release.yml:
;   ISCC.exe /DAppVersion=0.3.2 /DSourceExe=path\to\keyman-app.exe /O<out dir> installer\windows\keyman.iss
; Installs for the current user (no admin prompt) into %LOCALAPPDATA%\Programs\Keyman, with a
; Start menu entry, an optional desktop shortcut and an uninstaller. The folder is writable, so
; the app can update itself there. Uninstall leaves the settings and the master file alone.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\gui\target\release\keyman-app.exe"
#endif

[Setup]
; Fixed for the life of the app: Windows uses it to find upgrades and the uninstaller.
AppId={{9f0ee276-357d-4ea1-b8b9-020e62fd6ad0}
AppName=Keyman
AppVersion={#AppVersion}
AppVerName=Keyman {#AppVersion}
AppPublisher=Keyman
AppPublisherURL=https://github.com/moonmangit/keyman-cli
AppSupportURL=https://github.com/moonmangit/keyman-cli/issues
AppUpdatesURL=https://github.com/moonmangit/keyman-cli/releases
DefaultDirName={autopf}\Keyman
DefaultGroupName=Keyman
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=Keyman-setup-windows-x64
LicenseFile=..\..\LICENSE
SetupIconFile=..\..\gui\assets\keyman.ico
UninstallDisplayIcon={app}\Keyman.exe
UninstallDisplayName=Keyman
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Installing over a running copy: ask to close it first.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "Keyman.exe"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Keyman"; Filename: "{app}\Keyman.exe"
Name: "{autodesktop}\Keyman"; Filename: "{app}\Keyman.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Keyman.exe"; Description: "{cm:LaunchProgram,Keyman}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; The copy that an update set aside, if the app did not delete it yet.
Type: files; Name: "{app}\Keyman.old.exe"
