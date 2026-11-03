; Guardian installer for YOUR PC (and your mum's, if she has one).
; Runs the home relay in the background and adds a "Guardian" shortcut
; that opens the viewer.

#define AppVersion GetEnv("GUARDIAN_VERSION")
#if AppVersion == ""
  #define AppVersion "0.1.0"
#endif

[Setup]
AppId={{B5A2D7E4-3F61-4E2A-8D35-2C9A7F0E4B18}
AppName=Guardian Parent
AppVersion={#AppVersion}
AppPublisher=Guardian
DefaultDirName={autopf}\Guardian Parent
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Guardian-Parent-PC-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Messages]
WelcomeLabel2=This installs the Guardian viewer on a parent's PC.%n%nIn Home mode this PC also connects to the child's PC over your Wi-Fi, so leave it switched on when you want to check in or get approval requests.

[Files]
Source: "..\target\release\guardian-relay.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\relay\web\*"; DestDir: "{app}\web"; Flags: ignoreversion recursesubdirs

[INI]
Filename: "{app}\Guardian.url"; Section: "InternetShortcut"; Key: "URL"; String: "http://localhost:8080"

[Icons]
Name: "{autodesktop}\Guardian"; Filename: "{app}\Guardian.url"
Name: "{autoprograms}\Guardian"; Filename: "{app}\Guardian.url"
; Start the relay quietly whenever anyone signs in to this PC.
Name: "{commonstartup}\Guardian relay"; Filename: "{app}\guardian-relay.exe"; WorkingDir: "{app}"

[Run]
; Allow the child's PC to reach this one, on private (home) networks only.
Filename: "netsh.exe"; Parameters: "advfirewall firewall delete rule name=""Guardian relay"""; Flags: runhidden
Filename: "netsh.exe"; Parameters: "advfirewall firewall add rule name=""Guardian relay"" dir=in action=allow protocol=TCP localport=8080 program=""{app}\guardian-relay.exe"" profile=private"; Flags: runhidden
Filename: "{app}\guardian-relay.exe"; WorkingDir: "{app}"; Flags: nowait runasoriginaluser
Filename: "{app}\Guardian.url"; Description: "Open Guardian now"; Flags: postinstall shellexec nowait skipifsilent

[UninstallRun]
Filename: "taskkill.exe"; Parameters: "/F /IM guardian-relay.exe"; Flags: runhidden; RunOnceId: "StopRelay"
Filename: "netsh.exe"; Parameters: "advfirewall firewall delete rule name=""Guardian relay"""; Flags: runhidden; RunOnceId: "FirewallRule"

[UninstallDelete]
Type: files; Name: "{app}\Guardian.url"
