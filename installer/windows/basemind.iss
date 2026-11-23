; ---------------------------------------------------------------------------
; Basemind per-user Windows installer (Inno Setup 6).
;
; Built by installer/windows/build-installer.ps1, which supplies the /D defines:
;   Variant       full | hacienda-legal
;   Version       release version (e.g. 0.39.0)
;   Triple        release target triple (x86_64-pc-windows-msvc)
;   SourceDir     staging directory holding basemind.exe (+ optional DLLs) and basemind.toml
;   SetupName     exact output filename, e.g. basemind-x86_64-pc-windows-msvc-setup.exe
;   AppId         stable per-variant GUID (keeps the two variants separately upgradeable)
;
; The product this installs is CLI + stdio MCP only. Basemind has no GUI: no GUI
; binary is shipped, no Start Menu shortcut is created and nothing is auto-launched.
; The installer wizard itself is normal (and fully scriptable with /VERYSILENT).
;
; The existing release .zip is untouched: this installer is an ADDITIONAL artifact
; built from the same staged payload, not a replacement for package-release.sh.
; ---------------------------------------------------------------------------

#ifndef Variant
  #define Variant "full"
#endif
#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Triple
  #define Triple "x86_64-pc-windows-msvc"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\payload"
#endif
#ifndef SetupName
  #define SetupName "basemind-setup.exe"
#endif
; The build driver passes /DLegalVariant=1 for hacienda-legal and 0 for full.
; Compare as an integer (#ifdef would be true for either value).
#ifndef LegalVariant
  #define LegalVariant 0
#endif
#if LegalVariant == 1
  #define IsLegal 1
#else
  #define IsLegal 0
#endif

#if IsLegal == 1
  #define ProductName "Basemind (hacienda-legal)"
  #define InstallSubdir "Basemind-Hacienda-Legal"
  #define ProfileLabel "hacienda-legal"
  #define ProfileDescription "Legal focus release: documents/RAG, NER, vault, memory, GraphRAG, stdio MCP"
  #ifndef AppId
    #define AppId "{{6F0D4A8E-2B71-4C35-9A08-1D73E5B4C902}}"
  #endif
#else
  #define ProductName "Basemind"
  #define InstallSubdir "Basemind"
  #define ProfileLabel "full"
  #define ProfileDescription "Full release: code map, git, documents/RAG, memory, comms, stdio MCP"
  #ifndef AppId
    #define AppId "{{2C9B81F6-5D40-4E2A-8F17-6B0A9C3D5E41}}"
  #endif
#endif

[Setup]
; Per-user install: no elevation, no machine-wide state.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\{#InstallSubdir}
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableFinishedPage=no
WizardStyle=modern
ShowLanguageDialog=no
MinVersion=10.0
ArchitecturesInstallIn64BitMode=x64

AppId={#AppId}
AppName={#ProductName}
AppVersion={#Version}
AppPublisher=Basemind
AppComments={#ProfileDescription} (CLI + MCP, no GUI)

OutputBaseFilename={#SetupName}
Compression=lzma2/max
SolidCompression=yes
UninstallDisplayName={#ProductName} {#Version} (user install)
UninstallDisplayIcon={app}\basemind.exe
; Deliberately no [Icons] section: Basemind ships no GUI to pin to the Start Menu.

[Files]
; The executable and any vendored DLLs from the SAME staged payload the .zip ships.
Source: "{#SourceDir}\basemind.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
; Config template is per-profile so both variants can coexist under one user account.
; onlyifdoesntexist keeps a user-edited copy across upgrades.
Source: "{#SourceDir}\basemind.toml"; DestDir: "{appdata}\Basemind\{#ProfileLabel}"; DestName: "basemind.toml"; Flags: ignoreversion onlyifdoesntexist

[Code]
{ Add the install directory to the USER Path (HKCU\Environment\Path), idempotently,
  and broadcast WM_SETTINGCHANGE so new shells pick it up without a logoff. }

const
  HWND_BROADCAST = LongWord($ffff);
  WM_SETTINGCHANGE = LongWord($001A);
  SMTO_ABORTIFHUNG = LongWord($0002);
  EnvironmentKey = 'Environment';
  PathValueName = 'Path';

function SendMessageTimeout(hWnd: LongWord; Msg: LongWord; wParam: LongWord;
  lParam: string; fuFlags: LongWord; uTimeout: LongWord;
  var lpdwResult: LongWord): LongInt;
  external 'SendMessageTimeoutW@user32.dll stdcall delayload';

procedure BroadcastEnvironmentChange;
var
  ResultCode: LongWord;
begin
  SendMessageTimeout(HWND_BROADCAST, WM_SETTINGCHANGE, 0, 'Environment',
    SMTO_ABORTIFHUNG, 2000, ResultCode);
end;

function PathContainsDir(const PathValue, Dir: string): Boolean;
var
  Rest, Segment: string;
  P: Integer;
begin
  Result := False;
  Rest := PathValue;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P > 0 then
    begin
      Segment := Trim(Copy(Rest, 1, P - 1));
      Rest := Copy(Rest, P + 1, Length(Rest) - P);
    end
    else
    begin
      Segment := Trim(Rest);
      Rest := '';
    end;
    if (Segment <> '') and (CompareText(Segment, Dir) = 0) then
    begin
      Result := True;
      Exit;
    end;
  end;
end;

procedure AddDirToUserPath(const Dir: string);
var
  CurrentPath, NewPath: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, PathValueName, CurrentPath) then
    CurrentPath := '';
  if PathContainsDir(CurrentPath, Dir) then
  begin
    Log('Basemind: user PATH already contains ' + Dir);
    Exit;
  end;
  if CurrentPath = '' then
    NewPath := Dir
  else if Copy(CurrentPath, Length(CurrentPath), 1) = ';' then
    NewPath := CurrentPath + Dir
  else
    NewPath := CurrentPath + ';' + Dir;
  if not RegWriteStringValue(HKEY_CURRENT_USER, EnvironmentKey, PathValueName, NewPath) then
    RaiseException('Basemind: could not add ' + Dir + ' to the user PATH');
  Log('Basemind: added to user PATH: ' + Dir);
  BroadcastEnvironmentChange;
end;

procedure RemoveDirFromUserPath(const Dir: string);
var
  CurrentPath, NewPath, Rest, Segment: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, PathValueName, CurrentPath) then
    Exit;
  if not PathContainsDir(CurrentPath, Dir) then
    Exit;
  NewPath := '';
  Rest := CurrentPath;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P > 0 then
    begin
      Segment := Trim(Copy(Rest, 1, P - 1));
      Rest := Copy(Rest, P + 1, Length(Rest) - P);
    end
    else
    begin
      Segment := Trim(Rest);
      Rest := '';
    end;
    if (Segment <> '') and (CompareText(Segment, Dir) <> 0) then
    begin
      if NewPath = '' then
        NewPath := Segment
      else
        NewPath := NewPath + ';' + Segment;
    end;
  end;
  RegWriteStringValue(HKEY_CURRENT_USER, EnvironmentKey, PathValueName, NewPath);
  Log('Basemind: removed from user PATH: ' + Dir);
  BroadcastEnvironmentChange;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    AddDirToUserPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveDirFromUserPath(ExpandConstant('{app}'));
end;
