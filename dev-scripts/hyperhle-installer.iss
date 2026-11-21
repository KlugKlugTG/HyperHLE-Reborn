; Inno Setup script for the HyperHLE Windows installer.
;
; Packages the output of make-windows-bundle.sh plus the documentation that
; the release zip ships. Build it with Inno Setup 6.3 or newer, e.g.:
;
;   cd dev-scripts
;   ./make-windows-bundle.sh ../target/release/touchHLE.exe
;   ISCC.exe /DAppVersion=v1.2.3 /DAppNumericVersion=1.2.3 hyperhle-installer.iss
;
; touchHLE reads its resources from, and writes user data (sandbox, options,
; picker settings) to, the current directory. Shortcuts therefore start it
; from the install directory, which has to be writable: by default HyperHLE is
; installed per-user, and an all-users install grants users write access.

#ifndef AppVersion
  #define AppVersion "dev"
#endif
#ifndef AppNumericVersion
  #define AppNumericVersion "0.0.0"
#endif
; Relative paths are resolved against this script's directory.
#ifndef BundleDir
  #define BundleDir "touchHLE_windows_bundle"
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

#define AppName "HyperHLE"
#define AppExeName "touchHLE.exe"

[Setup]
; Never change AppId: it is how upgrades find an existing install.
AppId={{240E0CC4-02F5-4DEB-A0F8-902393AEB609}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=HyperHLE Reborn contributors
AppPublisherURL=https://github.com/KlugKlugTG/HyperHLE-Reborn
AppSupportURL=https://github.com/KlugKlugTG/HyperHLE-Reborn/issues
AppUpdatesURL=https://github.com/KlugKlugTG/HyperHLE-Reborn/releases
VersionInfoVersion={#AppNumericVersion}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\res\icon.ico
UninstallDisplayIcon={app}\{#AppExeName}
OutputDir={#OutputDir}
OutputBaseFilename=HyperHLE_Windows_x86_64_Setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Dirs]
Name: "{app}"; Permissions: users-modify; Check: IsAdminInstallMode
Name: "{app}\HyperHLE_apps"

[Files]
Source: "{#BundleDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BundleDir}\touchHLE_dylibs\*"; DestDir: "{app}\touchHLE_dylibs"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#BundleDir}\touchHLE_fonts\*"; DestDir: "{app}\touchHLE_fonts"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#BundleDir}\touchHLE_default_options.txt"; DestDir: "{app}"; Flags: ignoreversion
; The user is expected to edit or replace these, so keep their copies.
Source: "{#BundleDir}\touchHLE_wallpaper.png"; DestDir: "{app}"; Flags: onlyifdoesntexist uninsneveruninstall
Source: "..\HyperHLE_options.txt"; DestDir: "{app}"; Flags: onlyifdoesntexist uninsneveruninstall
Source: "..\HyperHLE_apps\README.txt"; DestDir: "{app}\HyperHLE_apps"; Flags: ignoreversion
Source: "..\OPTIONS_HELP.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "gpl-3.0.txt"; DestDir: "{app}"; DestName: "COPYING.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"
Name: "{group}\{#AppName} apps folder"; Filename: "{app}\HyperHLE_apps"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
