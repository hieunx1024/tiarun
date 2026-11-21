; Inno Setup Script for TiaRun
#define MyAppName "TiaRun"
#define MyAppVersion "1.0.0"
#define MyAppPublisher "Hieu Nguyen"
#define MyAppURL "https://github.com/hieunx1024/tiarun"
#define MyAppExeName "tiarun.exe"

[Setup]
AppId={{D37F7D28-854B-4E38-9FE7-99E8B62B23F1}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\TiaRun
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
OutputDir=Output
OutputBaseFilename=tiarun-setup
SetupIconFile=..\..\assets\tiarun.ico
UninstallDisplayIcon={app}\tiarun.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=force
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "startupicon"; Description: "Automatically start with Windows and register Ctrl+Alt+Space"; GroupDescription: "Startup Options"; Flags: checkedonce

[Files]
Source: "..\..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\assets\tiarun.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\assets\tiarun.png"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Start Menu
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\tiarun.ico"; WorkingDir: "{app}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"

; Desktop
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\tiarun.ico"; WorkingDir: "{app}"; Tasks: desktopicon; HotKey: "Ctrl+Alt+Space"

; Startup
Name: "{userstartup}\TiaRun"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\tiarun.ico"; WorkingDir: "{app}"; Tasks: startupicon

[Registry]
; Add install folder to User PATH
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch TiaRun now"; Flags: postinstall nowait skipifsilent

[Code]
function InitializeSetup(): Boolean;
var
  ResultCode: Integer;
begin
  Exec('taskkill.exe', '/F /IM tiarun.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec('taskkill.exe', '/F /IM view-launcher.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Sleep(250);
  Result := True;
end;

function InitializeUninstall(): Boolean;
var
  ResultCode: Integer;
begin
  Exec('taskkill.exe', '/F /IM tiarun.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec('taskkill.exe', '/F /IM view-launcher.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Sleep(250);
  Result := True;
end;

function NeedsAddPath(Param: string): boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath)
  then begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + UpperCase(Param) + ';', ';' + UpperCase(OrigPath) + ';') = 0;
end;
