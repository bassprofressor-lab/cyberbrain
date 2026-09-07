; Installer for Cyberbrain on Windows.
;
; Installs two programs into one directory: the command-line tool and the launcher that
; starts it without a terminal. The Start menu entry points at the launcher, because that is
; the way in this installer exists to provide.
;
; It can also register the hub as a Windows service. That is off by default — most machines
; are clients, not the collector — but when it is ticked, everything the hub needs is done
; here: the data directory, the service, the firewall rule, and a folder shortcut to drop
; the licence into. A customer who has to open a command prompt to finish an installation is
; a customer who stops.
;
; Deliberately NOT done here: putting the install directory on the PATH. NSIS truncates
; strings at 1024 characters in its default build, and a system PATH is often longer than
; that, so the "helpful" version of this feature silently eats the end of someone's PATH.
; The README says how to add it by hand, which is a worse experience and a better outcome.
;
; Build:  makensis /DVERSION=0.1.0 /DSOURCE=<dir with the exes> cyberbrain.nsi

Unicode true
!include "MUI2.nsh"
!include "x64.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef SOURCE
  !define SOURCE "."
!endif
; VIProductVersion takes four numbers and nothing else, so a version like 0.2.0-rc.1 has to
; arrive with its suffix already cut off. The build passes it; this default keeps a manual
; makensis run working.
!ifndef VIVERSION
  !define VIVERSION "0.0.0"
!endif

!define NAME "Cyberbrain"
!define PUBLISHER "Krynex Labs"
!define WEBSITE "https://www.krynexlabs.de/cyberbrain/"
!define REGKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${NAME}"

Name "${NAME} ${VERSION}"
OutFile "cyberbrain-setup-${VERSION}.exe"
InstallDir "$PROGRAMFILES64\${NAME}"
InstallDirRegKey HKLM "Software\${NAME}" "InstallDir"
; Program Files is not writable by a normal user, and a memory tool that installs itself
; somewhere surprising to avoid the prompt is not the trade to make.
RequestExecutionLevel admin
SetCompressor /SOLID lzma

VIProductVersion "${VIVERSION}.0"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "FileDescription" "${NAME} installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "Copyright 2026 ${PUBLISHER}"

; Both of these are copied into SOURCE by the build. Referring to them by a relative path
; out of this directory works with makensis on Linux and not with makensis on Windows,
; which is a difference nobody wants to rediscover at release time.
!define ICON "${SOURCE}\cyberbrain.ico"
!define LICENSE "${SOURCE}\LICENSE.txt"
!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING

!insertmacro MUI_PAGE_LICENSE "${LICENSE}"
; The optional desktop shortcut is a choice, so there is a page on which to make it.
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\cyberbrain-desktop.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open ${NAME}"
!define MUI_FINISHPAGE_LINK "Cyberbrain on the web"
!define MUI_FINISHPAGE_LINK_LOCATION "${WEBSITE}"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "Cyberbrain needs 64-bit Windows."
    Abort
  ${EndIf}
FunctionEnd

Section "Cyberbrain" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${SOURCE}\cyberbrain.exe"
  File "${SOURCE}\cyberbrain-desktop.exe"
  File "/oname=cyberbrain.ico" "${ICON}"
  File "/oname=LICENSE.txt" "${LICENSE}"

  CreateDirectory "$SMPROGRAMS\${NAME}"
  CreateShortcut "$SMPROGRAMS\${NAME}\${NAME}.lnk" "$INSTDIR\cyberbrain-desktop.exe" "" "$INSTDIR\cyberbrain.ico" 0
  CreateShortcut "$SMPROGRAMS\${NAME}\Uninstall ${NAME}.lnk" "$INSTDIR\uninstall.exe"

  WriteRegStr HKLM "Software\${NAME}" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "${REGKEY}" "DisplayName" "${NAME}"
  WriteRegStr HKLM "${REGKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${REGKEY}" "DisplayIcon" "$INSTDIR\cyberbrain.ico"
  WriteRegStr HKLM "${REGKEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${REGKEY}" "URLInfoAbout" "${WEBSITE}"
  WriteRegStr HKLM "${REGKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${REGKEY}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKLM "${REGKEY}" "NoModify" 1
  WriteRegDWORD HKLM "${REGKEY}" "NoRepair" 1
  ; Size in KB, so Programs and Features does not show a blank where every other entry
  ; shows a number.
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKLM "${REGKEY}" "EstimatedSize" "$0"

  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\${NAME}.lnk" "$INSTDIR\cyberbrain-desktop.exe" "" "$INSTDIR\cyberbrain.ico" 0
SectionEnd


; ---------------------------------------------------------------------------------------
; The hub, for the one machine that collects. Off by default: on a client it would open a
; port for nothing.

Section /o "Hub service (collector)" SecHub
  CreateDirectory "$COMMONPROGRAMDATA\${NAME}"

  ; Written before the service is registered, so the folder is never empty and confusing.
  FileOpen $0 "$COMMONPROGRAMDATA\${NAME}\HOW-TO-LICENCE.txt" w
  FileWrite $0 "This folder is the hub's record.$\r$\n$\r$\n"
  FileWrite $0 "To license the hub, save the licence file you were sent here, next to$\r$\n"
  FileWrite $0 "this note, under the name:  licence.txt$\r$\n$\r$\n"
  FileWrite $0 "Windows hides file extensions, so a file shown as licence.txt may really$\r$\n"
  FileWrite $0 "be licence.txt.txt. That is taken as well, and so is license.txt. The log$\r$\n"
  FileWrite $0 "says which one was used, so you do not have to guess.$\r$\n$\r$\n"
  FileWrite $0 "Then restart the service (Services > ${NAME} Hub > Restart).$\r$\n"
  FileWrite $0 "Without a licence the hub runs but collects nothing.$\r$\n$\r$\n"
  FileWrite $0 "hub.db is the record itself. Back this folder up; it is the evidence.$\r$\n"
  FileWrite $0 "hub-service.log says what the service did on each start.$\r$\n"
  FileClose $0

  DetailPrint "Registering the ${NAME} hub service..."
  nsExec::ExecToStack '"$INSTDIR\cyberbrain.exe" hub service install --data "$COMMONPROGRAMDATA\${NAME}\hub.db" --addr 0.0.0.0:7788'
  Pop $0
  Pop $1
  ${If} $0 != 0
    ; Not fatal: the rest of the installation is fine and the command can be run again.
    ; Saying what happened beats a silent half-installed collector.
    MessageBox MB_ICONEXCLAMATION "The hub service could not be registered:$\r$\n$\r$\n$1$\r$\nEverything else was installed, and the workstation side works.$\r$\n$\r$\nTo try again, in a prompt opened with Run as administrator:$\r$\n  cd $\"$INSTDIR$\"$\r$\n  cyberbrain.exe hub service install$\r$\n$\r$\nIf it says the service is being removed, restart Windows first."
  ${Else}
    DetailPrint $1
    ; Without this the service listens and nothing ever reaches it, which looks exactly
    ; like a broken client. The rule is removed again on uninstall.
    nsExec::ExecToStack 'netsh advfirewall firewall add rule name="${NAME} Hub" dir=in action=allow protocol=TCP localport=7788'
    Pop $0
    Pop $1
  ${EndIf}

  CreateShortcut "$SMPROGRAMS\${NAME}\Hub data folder.lnk" "$COMMONPROGRAMDATA\${NAME}"
SectionEnd

LangString DESC_SecMain ${LANG_ENGLISH} "The command-line tool and the launcher that opens it without a terminal."
LangString DESC_SecDesktop ${LANG_ENGLISH} "An icon on the desktop as well as in the Start menu."
LangString DESC_SecHub ${LANG_ENGLISH} "Only for the one machine that collects the audit trail of the others. Registers a Windows service on port 7788, opens that port in the firewall, and creates a folder to put the licence in. Not needed on a normal workstation."

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} $(DESC_SecMain)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} $(DESC_SecDesktop)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecHub} $(DESC_SecHub)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  ; The service first, while the program that can remove it is still on disk. It is quiet
  ; about not finding one: most installations never had it.
  nsExec::ExecToStack '"$INSTDIR\cyberbrain.exe" hub service uninstall'
  Pop $0
  Pop $1
  nsExec::ExecToStack 'netsh advfirewall firewall delete rule name="${NAME} Hub"'
  Pop $0
  Pop $1

  ; The notes are the user's and live in their projects; nothing under the install
  ; directory is theirs, so this removes what it put there and stops.
  Delete "$INSTDIR\cyberbrain.exe"
  Delete "$INSTDIR\cyberbrain-desktop.exe"
  Delete "$INSTDIR\cyberbrain.ico"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${NAME}\${NAME}.lnk"
  Delete "$SMPROGRAMS\${NAME}\Uninstall ${NAME}.lnk"
  Delete "$SMPROGRAMS\${NAME}\Hub data folder.lnk"
  RMDir "$SMPROGRAMS\${NAME}"
  Delete "$DESKTOP\${NAME}.lnk"

  DeleteRegKey HKLM "${REGKEY}"
  DeleteRegKey HKLM "Software\${NAME}"
  ; The hub's record under %PROGRAMDATA%\${NAME} is left alone, deliberately and without
  ; asking: it is the company's evidence, it is what a works agreement promises to keep,
  ; and removing the software must never remove it.
  ; %APPDATA%\cyberbrain\desktop.toml is left alone: it is one line saying which folder was
  ; open, and a reinstall that remembers is friendlier than one that forgets.
SectionEnd
