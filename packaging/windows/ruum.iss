[Setup]
AppId={{E8F4B9A2-6D1C-4B3A-9E7F-2C5A1D8E4B60}
AppName=ruum
AppVersion=@VERSION@
AppVerName=ruum @VERSION@
AppPublisher=JsonLee12138
AppPublisherURL=https://github.com/JsonLee12138/Ruum
AppSupportURL=https://github.com/JsonLee12138/Ruum/issues
DefaultDirName={autopf}\ruum
DefaultGroupName=ruum
AllowNoIcons=yes
LicenseFile=@LICENSE_PATH@
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=@INNO_ARCH_ALLOWED@
ArchitecturesInstallIn64BitMode=@INNO_ARCH_INSTALL64@
SetupIconFile=@ICON_PATH@
UninstallDisplayIcon={app}\ruum-gui.exe
OutputDir=@OUTPUT_DIR@
OutputBaseFilename=ruum-@VERSION@-@ARCH@-windows-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
CloseApplications=yes
CloseApplicationsFilter=ruum.exe,ruum-gui.exe
RestartApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Add ruum to user PATH"; GroupDescription: "Other tasks:"

[Files]
Source: "@EXE_PATH@"; DestDir: "{app}"; Flags: ignoreversion
Source: "@GUI_EXE_PATH@"; DestDir: "{app}"; Flags: ignoreversion
Source: "@README_PATH@"; DestDir: "{app}"; Flags: ignoreversion
Source: "@LICENSE_PATH@"; DestDir: "{app}"; Flags: ignoreversion
Source: "@NOTICES_PATH@"; DestDir: "{app}"; Flags: ignoreversion
@SCANNER_PACK_FILE_ENTRY@

[Icons]
Name: "{autoprograms}\ruum"; Filename: "{app}\ruum-gui.exe"; AppUserModelID: "io.github.tobi.ruum"
Name: "{autodesktop}\ruum"; Filename: "{app}\ruum-gui.exe"; Tasks: desktopicon; AppUserModelID: "io.github.tobi.ruum"

[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Code]
function NeedsAddPath(Param: string): boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;
