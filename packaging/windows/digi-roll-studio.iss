; Inno Setup script for the Digi-Roll Studio Windows installer.
;
; Build it with packaging/windows/build-installer.ps1, which fills in the
; version and the source directory. Compiling this file by hand works too:
;
;   ISCC.exe /DAppVersion=0.1.0 packaging\windows\digi-roll-studio.iss
;
; Needs Inno Setup 6.3 or newer, for `x64compatible`. Paths are relative to this
; file.
;
; The output name matters: the download page picks the Windows asset out of the
; GitHub release by matching /\.(exe|msi)$/ on the asset name, so the installer
; must be one of those (it is an .exe) for the button to point at the file
; rather than at the releases page.

#ifndef AppVersion
  #define AppVersion "0.0.0-dev"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\x86_64-pc-windows-msvc\release"
#endif
; VersionInfoVersion only accepts a numeric x.y.z[.w], so a version with a
; prerelease suffix — "0.2.0-beta", which this being beta software makes likely —
; would fail to compile here. build-installer.ps1 passes the stripped form; the
; fallback covers compiling this by hand with a plain version.
#ifndef VersionInfo
  #define VersionInfo AppVersion
#endif

#define AppName        "Digi-Roll Studio"
#define AppPublisher   "zooloo303"
#define AppExe         "Digi-Roll Studio.exe"
#define CargoExe       "digi_roll_studio.exe"
#define SiteUrl        "https://zooloo303.github.io/digi-roll/studio/"
#define RepoUrl        "https://github.com/zooloo303/digi-roll-studio"

[Setup]
; Generated once and never changed: this is the identity Windows matches an
; upgrade or an uninstall against. A new GUID here would make the next release
; install alongside this one instead of over it.
AppId={{FC78B52D-54BC-46CB-8ABF-E2B310EBE52B}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#SiteUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
VersionInfoVersion={#VersionInfo}

; Per-user install, and deliberately so. `lowest` means no UAC prompt at all —
; and a UAC prompt raised by an unsigned installer is the red "unknown
; publisher" one, which would be a second scary dialog stacked on top of the
; SmartScreen warning the install page already walks people through. Nothing
; here needs machine-wide access: it is one exe in one folder.
; {autopf} follows the privilege level, so this lands in
; %LOCALAPPDATA%\Programs\Digi-Roll Studio.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
DefaultGroupName={#AppName}

; x64 only, matching what the page advertises. `x64compatible` rather than the
; older `x64` so this also installs on Windows 11 on Arm, where the binary runs
; under emulation.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0

OutputDir=..\..\dist
OutputBaseFilename=Digi-Roll-Studio-{#AppVersion}-Windows-x64-Setup
SetupIconFile=..\..\icons\windows\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; Nothing in the wizard needs a decision from the user except the desktop icon,
; so do not make them read a directory page to get to it.
DisableDirPage=auto
DisableReadyPage=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
; Renamed on the way in: cargo builds `digi_roll_studio.exe`, and the underscores
; would show up in the Start menu, in Task Manager and in every SmartScreen
; dialog. build.rs stamps the matching name into the exe's version resource.
Source: "{#SourceDir}\{#CargoExe}"; DestDir: "{app}"; DestName: "{#AppExe}"; Flags: ignoreversion
; The GPL requires the licence travel with the binary.
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\..\CREDITS.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; \
  Flags: nowait postinstall skipifsilent
