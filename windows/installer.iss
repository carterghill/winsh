[Setup]
AppId=winsh.terminal
AppName=winsh
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\winsh
PrivilegesRequired=lowest
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=winsh-{#AppVersion}-setup
SetupIconFile=winsh.ico
UninstallDisplayIcon={app}\winsh-terminal.exe
Compression=lzma2
SolidCompression=yes

[Files]
Source: "..\target\release\winsh.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\winsh-terminal.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\crates\winsh\src\utilities\ls\LICENSE-eza.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\crates\winsh-terminal\assets\LICENSE-Nerd-Fonts.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\winsh"; Filename: "{app}\winsh-terminal.exe"; WorkingDir: "{%USERPROFILE}"
