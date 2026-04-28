; Patina — Inno Setup 6.7+ installer script
; Place this file in the project root alongside Cargo.toml.
; Build with: iscc patina_setup.iss
; Output: Output\Patina_Setup.exe

#define AppName      "Patina"
#define AppVersion   "0.1.0"
#define AppPublisher "Fatih Cihan Hizlikan"
#define AppExeName   "Patina.exe"
#define AppId        "{{7A3E9F14-2C88-4D71-B5AC-8F3D92E74C01}"
#define ProgId       "Patina.ImageFile"

[Setup]
AppId={#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL=https://github.com/yourusername/patina
AppSupportURL=https://github.com/yourusername/patina
AppUpdatesURL=https://github.com/yourusername/patina

; Per-user by default, no UAC. User can choose "Install for all users"
; on the scope page which re-launches elevated and switches to per-machine.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

; Default install paths — Inno picks the right one based on scope.
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes

; Output
OutputDir=Output
OutputBaseFilename=Patina_Setup
SetupIconFile=patina.ico
UninstallDisplayIcon={app}\{#AppExeName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern

; Force 64-bit install path on 64-bit Windows.
ArchitecturesInstallIn64BitMode=x64compatible arm64

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "openwith";    Description: "Add to &Open With menu for all supported image formats";  GroupDescription: "File Associations:"; Flags: checkedonce
Name: "setdefault";  Description: "&Set as default image viewer for all supported formats";  GroupDescription: "File Associations:"; Flags: checkedonce
Name: "desktopicon"; Description: "Create a &desktop shortcut";                              GroupDescription: "Additional icons:"; Flags: checkedonce

[Files]
Source: "target\release\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Dirs]
Name: "{app}"; Flags: uninsalwaysuninstall

[Icons]
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; \
    IconFilename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Registry]
; ── App Paths ────────────────────────────────────────────────────────────────
; Lets Windows and shell extensions find and launch the application.
Root: HKLM; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#AppExeName}"; \
    ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExeName}"; \
    Flags: uninsdeletekey; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#AppExeName}"; \
    ValueType: string; ValueName: "Path"; ValueData: "{app}"; \
    Check: IsAdminInstallMode
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#AppExeName}"; \
    ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExeName}"; \
    Flags: uninsdeletekey; Check: not IsAdminInstallMode
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#AppExeName}"; \
    ValueType: string; ValueName: "Path"; ValueData: "{app}"; \
    Check: not IsAdminInstallMode

; ── ProgId ───────────────────────────────────────────────────────────────────
; HKA = HKLM on per-machine installs, HKCU on per-user installs.
Root: HKA; Subkey: "Software\Classes\{#ProgId}"; \
    ValueType: string; ValueName: ""; ValueData: "Image File"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\{#ProgId}\DefaultIcon"; \
    ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExeName},0"
Root: HKA; Subkey: "Software\Classes\{#ProgId}\shell\open\command"; \
    ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""

; ── Open With ────────────────────────────────────────────────────────────────
; Standard raster formats
Root: HKA; Subkey: "Software\Classes\.jpg\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.jpeg\OpenWithProgIds"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.png\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.bmp\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.gif\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.webp\OpenWithProgIds"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.tiff\OpenWithProgIds"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.tif\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.ico\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.jxl\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Canon
Root: HKA; Subkey: "Software\Classes\.cr2\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.cr3\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Nikon
Root: HKA; Subkey: "Software\Classes\.nef\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.nrw\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Sony
Root: HKA; Subkey: "Software\Classes\.arw\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.srf\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.sr2\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Fujifilm
Root: HKA; Subkey: "Software\Classes\.raf\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Olympus
Root: HKA; Subkey: "Software\Classes\.orf\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — DNG (Adobe/universal)
Root: HKA; Subkey: "Software\Classes\.dng\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Pentax
Root: HKA; Subkey: "Software\Classes\.pef\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.ptx\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Panasonic
Root: HKA; Subkey: "Software\Classes\.rw2\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Hasselblad
Root: HKA; Subkey: "Software\Classes\.3fr\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.fff\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Leaf/Phase One
Root: HKA; Subkey: "Software\Classes\.mos\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Sigma
Root: HKA; Subkey: "Software\Classes\.x3f\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Samsung
Root: HKA; Subkey: "Software\Classes\.srw\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Mamiya
Root: HKA; Subkey: "Software\Classes\.mef\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
; RAW formats — Leica
Root: HKA; Subkey: "Software\Classes\.rwl\OpenWithProgIds";  ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith

; ── Set as Default ───────────────────────────────────────────────────────────
; Only HKLM (requires per-machine / admin install). Avoids HKCU UserChoice
; entries which survive uninstall and leave a broken stale association.
; Standard raster
Root: HKLM; Subkey: "Software\Classes\.jpg";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.jpeg"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.png";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.bmp";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.gif";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.webp"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.tiff"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.tif";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.ico";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.jxl";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
; RAW formats — set as default
Root: HKLM; Subkey: "Software\Classes\.cr2";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.cr3";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.nef";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.nrw";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.arw";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.srf";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.sr2";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.raf";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.orf";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.dng";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.pef";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.ptx";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.rw2";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.3fr";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.fff";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.mos";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.x3f";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.srw";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.mef";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.rwl";  ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: setdefault; Check: IsAdminInstallMode

; ── UserChoice cleanup on uninstall ─────────────────────────────────────────
; Windows writes UserChoice entries when HKCU\Software\Classes\.ext is set.
; Clean them up on uninstall so no stale entries remain.
; Standard raster
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.jpg\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.jpeg\UserChoice"; Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.png\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.bmp\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.gif\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.webp\UserChoice"; Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.tiff\UserChoice"; Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.tif\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.ico\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.jxl\UserChoice";  Flags: deletekey uninsdeletekey
; RAW formats
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.cr2\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.cr3\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.nef\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.nrw\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.arw\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.srf\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.sr2\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.raf\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.orf\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.dng\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pef\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.ptx\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.rw2\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.3fr\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.fff\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.mos\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.x3f\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.srw\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.mef\UserChoice";  Flags: deletekey uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.rwl\UserChoice";  Flags: deletekey uninsdeletekey

[Run]
Filename: "{sys}\ie4uinit.exe"; Parameters: "-ClearIconCache"; \
    Flags: runhidden nowait; StatusMsg: "Refreshing file type icons..."

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im {#AppExeName}"; \
    RunOnceId: "KillPatina"; Flags: runhidden skipifdoesntexist
Filename: "{sys}\ie4uinit.exe"; Parameters: "-ClearIconCache"; \
    RunOnceId: "RefreshIcons"; Flags: runhidden nowait
