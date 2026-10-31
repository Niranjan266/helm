; Helm installer (Inno Setup 6). MIT licensed.
;
; Build the binaries first (release, x64):
;   cargo build --release --manifest-path host\agent\Cargo.toml
;   cargo build --release --manifest-path host\crates\helm-service\Cargo.toml
; then compile this script:
;   iscc host\installer\helm.iss                       (version from #define below)
;   iscc /DAppVersion=0.2.0 host\installer\helm.iss    (override the version)
;
; Output: host\installer\Output\Helm-Setup-<version>-x64.exe
; Sign the result for the in-app updater with:
;   cargo run --example helm-sign --manifest-path host\crates\helm-update\Cargo.toml -- sign <key> <setup.exe>
;
; What it does
;   * installs helm-agent.exe and helm-service.exe to Program Files\Helm
;   * registers HelmService (LocalSystem), which starts the agent in the
;     signed-in user's session; "Start with Windows" makes it start at boot,
;     otherwise it is demand-start
;   * adds an inbound firewall rule for the agent's TCP port on the Private
;     profile only (never Public or Domain)
;   * Start Menu: Helm Agent (runs the agent directly, for when the service
;     is stopped) and Uninstall
; Uninstall removes the service and the firewall rule. The agent's data
; (pairing identity, trusted phones, settings) in %APPDATA%\Helm is left
; alone on purpose so a reinstall keeps existing pairings.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppName      "Helm"
#define AgentPort    "47820"
#define FirewallRule "Helm Agent"
#define RepoRoot     "..\.."
#define AgentExe     RepoRoot + "\host\agent\target\release\helm-agent.exe"
#define ServiceExe   RepoRoot + "\host\crates\helm-service\target\release\helm-service.exe"

[Setup]
; AppId identifies the install for upgrades; never change it.
AppId={{8C4292D3-CEFE-4820-825A-024F0FCD2AED}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Helm contributors
DefaultDirName={autopf}\Helm
DefaultGroupName=Helm
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile={#RepoRoot}\LICENSE
OutputDir=Output
OutputBaseFilename=Helm-Setup-{#AppVersion}-x64
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
UninstallDisplayIcon={app}\helm-agent.exe
UninstallDisplayName={#AppName}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "startwithwindows"; Description: "Start Helm automatically when Windows starts"; GroupDescription: "Startup:"

[Files]
Source: "{#AgentExe}";   DestDir: "{app}"; Flags: ignoreversion
Source: "{#ServiceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"

[Icons]
Name: "{group}\Helm Agent"; Filename: "{app}\helm-agent.exe"; WorkingDir: "{app}"; Comment: "Run the Helm agent directly (when the service is stopped)"
Name: "{group}\Uninstall Helm"; Filename: "{uninstallexe}"

[Run]
; Firewall: drop any old rule first so upgrades do not duplicate it.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""{#FirewallRule}"""; \
  Flags: runhidden waituntilterminated; StatusMsg: "Configuring Windows Firewall..."
Filename: "{sys}\netsh.exe"; \
  Parameters: "advfirewall firewall add rule name=""{#FirewallRule}"" dir=in action=allow protocol=TCP localport={#AgentPort} profile=private program=""{app}\helm-agent.exe"" enable=yes"; \
  Flags: runhidden waituntilterminated; StatusMsg: "Configuring Windows Firewall..."

; Service registration (the old one, if any, was removed in PrepareToInstall).
Filename: "{app}\helm-service.exe"; Parameters: "install --start auto -- --background"; Tasks: startwithwindows; \
  Flags: runhidden waituntilterminated; StatusMsg: "Registering the Helm service..."
Filename: "{app}\helm-service.exe"; Parameters: "install --start demand -- --background"; Tasks: not startwithwindows; \
  Flags: runhidden waituntilterminated; StatusMsg: "Registering the Helm service..."

; Optional "start now" checkbox on the final page.
Filename: "{app}\helm-service.exe"; Parameters: "start"; Description: "Start Helm now"; \
  Flags: postinstall runhidden waituntilterminated runascurrentuser

[UninstallRun]
Filename: "{app}\helm-service.exe"; Parameters: "uninstall"; \
  Flags: runhidden waituntilterminated; RunOnceId: "RemoveHelmService"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""{#FirewallRule}"""; \
  Flags: runhidden waituntilterminated; RunOnceId: "RemoveHelmFirewallRule"

[UninstallDelete]
; Only the service's own log in ProgramData. %APPDATA%\Helm is kept.
Type: files;      Name: "{commonappdata}\Helm\service.log"
Type: files;      Name: "{commonappdata}\Helm\service.log.1"
Type: dirifempty; Name: "{commonappdata}\Helm"

[Code]
// On upgrade, stop and remove the existing service before files are
// replaced; stopping it also ends the agent it supervises.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  OldService: String;
  ResultCode: Integer;
begin
  Result := '';
  OldService := ExpandConstant('{app}\helm-service.exe');
  if FileExists(OldService) then
    Exec(OldService, 'uninstall', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;
