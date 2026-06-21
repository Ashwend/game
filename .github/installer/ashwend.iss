; Inno Setup script for the per-user Ashwend Windows installer.
;
; Compiled headless in CI by `.github/scripts/package-release.py` (Windows
; branch) with ISCC.exe. The script stages both shipped binaries beside this
; file and passes the staging dir + version + output name via /D and /F:
;
;   ISCC /Qp /O<out-dir> /F<base-name> ^
;        /DAppVersion=<x.y.z> /DStagingDir=<staging> ^
;        .github\installer\ashwend.iss
;
; Why per-user (`{localappdata}\Programs\Ashwend`, PrivilegesRequired=lowest):
; the in-app self-update (`ashwend-updater.exe`) swaps `ashwend.exe` in place
; with a non-elevated `std::fs::rename`. `C:\Program Files` is UAC-protected,
; so a per-machine install there would silently break auto-update. Per-user
; installs to a user-writable directory keep self-update working with no UAC
; prompt, ever (the Discord / VS Code-user-installer / Spotify convention).
; `DisableDirPage=yes` locks the location so a user can't relocate it into a
; protected directory and break updates.

#define AppName "Ashwend"
#define AppPublisher "Ashwend"
#define AppURL "https://ashwend.com"
#define GameExe "ashwend.exe"
#define UpdaterExe "ashwend-updater.exe"

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
; Directory holding the freshly built ashwend.exe + ashwend-updater.exe.
; Defaults to this script's directory so a manual `ISCC ashwend.iss` from a
; folder containing the two exes also works.
#ifndef StagingDir
  #define StagingDir "."
#endif

[Setup]
; A stable AppId is what lets a new installer recognise and upgrade an existing
; install (same registry uninstall key + install dir). Never change it.
AppId={{307311E9-ED7F-4193-B946-6449603776A0}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
DefaultDirName={localappdata}\Programs\Ashwend
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=.
OutputBaseFilename=ashwend-setup
SetupIconFile=..\assets\ashwend.ico
UninstallDisplayIcon={app}\{#GameExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StagingDir}\{#GameExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\{#UpdaterExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; AppUserModelID matches the macOS bundle identifier and keeps the taskbar
; grouping/pinning stable across self-updates.
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#GameExe}"; AppUserModelID: "com.Ashwend.Ashwend"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#GameExe}"; AppUserModelID: "com.Ashwend.Ashwend"; Tasks: desktopicon

[Run]
Filename: "{app}\{#GameExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
