; Guardian installer for the CHILD's PC.
; Installs the protected service. Needs an admin account to install AND to
; uninstall, so his standard account can't remove it.

#define AppVersion GetEnv("GUARDIAN_VERSION")
#if AppVersion == ""
  #define AppVersion "0.1.0"
#endif

[Setup]
AppId={{6E0C3F1A-6D0B-4C59-9C1E-6A1F5E2B7C41}
AppName=Guardian
AppVersion={#AppVersion}
AppPublisher=Guardian
DefaultDirName={autopf}\Guardian
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Guardian-Child-PC-Setup-{#AppVersion}
SetupIconFile=..\assets\guardian.ico
UninstallDisplayIcon={app}\guardian-agent.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=Guardian
CloseApplications=no

[Messages]
WelcomeLabel2=This installs Guardian on the PC that will be looked after.%n%nAt the end you'll be shown a Device ID and pairing code. Write them down: you need them to connect from your own PC.%n%nMake sure the child's Windows account is a standard (not administrator) account.

[Files]
Source: "..\target\release\guardian-agent.exe"; DestDir: "{app}"; Flags: ignoreversion restartreplace
Source: "..\assets\guardian.ico"; DestDir: "{app}"; Flags: ignoreversion

[Run]
Filename: "{app}\guardian-agent.exe"; Parameters: "install"; Flags: runhidden waituntilterminated; StatusMsg: "Setting up Guardian..."

[UninstallRun]
Filename: "{app}\guardian-agent.exe"; Parameters: "uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "GuardianUninstall"

[Code]
function JsonValue(const Json, Key: String): String;
var P: Integer; S: String;
begin
  Result := '';
  P := Pos('"' + Key + '"', Json);
  if P = 0 then Exit;
  S := Copy(Json, P + Length(Key) + 2, 200);
  P := Pos('"', S); if P = 0 then Exit;
  S := Copy(S, P + 1, 200);
  P := Pos('"', S); if P = 0 then Exit;
  Result := Copy(S, 1, P - 1);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var Json: AnsiString; Id, Code: String; Tries: Integer;
begin
  if CurStep <> ssPostInstall then Exit;
  Tries := 0;
  while (not LoadStringFromFile(ExpandConstant('{commonappdata}\Guardian\agent.json'), Json)) and (Tries < 20) do
  begin Sleep(500); Tries := Tries + 1; end;
  Id := JsonValue(String(Json), 'id');
  Code := JsonValue(String(Json), 'code');
  if (Id <> '') and (Code <> '') then
    MsgBox('Guardian is installed and running.' + #13#10#13#10 +
           'Device ID:      ' + Id + #13#10 +
           'Pairing code:  ' + Code + #13#10#13#10 +
           'Write these down now and keep them private. Anyone with both can see and control this PC.',
           mbInformation, MB_OK)
  else
    MsgBox('Guardian is installed, but the pairing code could not be read. ' +
           'Run "guardian-agent.exe install" as administrator to see it.', mbError, MB_OK);
end;
