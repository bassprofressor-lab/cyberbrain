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
;         add /DWEBVIEW2LOADER=<path to WebView2Loader.dll> for a GNU-toolchain build

Unicode true
!include "MUI2.nsh"
!include "x64.nsh"
!include "FileFunc.nsh"
!include "Sections.nsh"

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

; Windows locks a running program's file, so an upgrade over a Cyberbrain that is still
; open ends in NSIS's "Error opening file for writing", whose only offered way forward is
; the task manager. The launcher sits in the notification area and has no window, so
; `taskkill` without /F does not reach it either — hence `--quit`, which is a message it
; listens for: it stops its servers, sends a last delivery and goes.
;
; Written as a macro because the uninstaller needs the same thing and NSIS gives it a
; separate namespace; `${UN}` is empty for one and `un.` for the other.
!macro CloseCyberbrain UN
Function ${UN}CloseCyberbrain
  ; The installation being replaced, which is where the running programs are. Nothing to
  ; close on a first install.
  ReadRegStr $R2 HKLM "Software\${NAME}" "InstallDir"
  ${If} $R2 == ""
    Return
  ${EndIf}

  ; The collector's service holds cyberbrain.exe open just as firmly, and on that one
  ; machine an upgrade fails for the same reason. Stopped here and started again at the
  ; end, but only if it was running: `net start` on a machine that never had the hub would
  ; be an error message about something nobody asked for.
  StrCpy $R3 "0"
  nsExec::ExecToStack 'net stop "CyberbrainHub"'
  Pop $R0
  Pop $R1
  ${If} $R0 == 0
    StrCpy $R3 "1"
    DetailPrint "Stopped the ${NAME} hub service for the duration."
  ${EndIf}

  ${If} ${FileExists} "$R2\cyberbrain-desktop.exe"
    DetailPrint "Closing ${NAME}, if it is running..."
    ; Exit code 0 once nothing is left running, 1 if it is still there after ten seconds.
    nsExec::ExecToStack '"$R2\cyberbrain-desktop.exe" --quit'
    Pop $R0
    Pop $R1
    ${If} $R0 != 0
      MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION "${NAME} did not close when asked, and its files cannot be replaced while it is running.$\r$\n$\r$\nOK closes it now. Anything it had not yet saved to its hub stays in the local log and goes on the next start." IDOK +2
      Abort
      nsExec::ExecToStack 'taskkill /F /T /IM cyberbrain-desktop.exe'
      Pop $R0
      Pop $R1
      nsExec::ExecToStack 'taskkill /F /T /IM cyberbrain.exe'
      Pop $R0
      Pop $R1
      ; Windows lets go of the file handles a moment after the process is gone, and NSIS
      ; is quick enough to arrive before that.
      Sleep 1500
    ${EndIf}
  ${EndIf}
FunctionEnd
!macroend
!insertmacro CloseCyberbrain ""
!insertmacro CloseCyberbrain "un."

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


Section "Cyberbrain" SecMain
  SectionIn RO
  ; Before the first File: everything below writes into a directory whose files may still
  ; be open.
  Call CloseCyberbrain
  SetOutPath "$INSTDIR"
  File "${SOURCE}\cyberbrain.exe"
  File "${SOURCE}\cyberbrain-desktop.exe"
  File "/oname=cyberbrain.ico" "${ICON}"
  File "/oname=LICENSE.txt" "${LICENSE}"
  ; Only a GNU-toolchain build needs this beside the launcher: under MSVC the WebView2
  ; loader is linked in statically, under GNU it is an ordinary import and the window never
  ; opens without the DLL. Passed only by the cross build, so the release artefact that CI
  ; produces is unchanged by this line existing.
!ifdef WEBVIEW2LOADER
  File "${WEBVIEW2LOADER}"
!endif

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

  ; On the PATH, because the workstation half of this is a command-line program and
  ; "cyberbrain hub push" on a timer should not have to name a directory with a space in it.
  ; Through a script rather than from here: NSIS truncates strings at a fixed length and a
  ; machine PATH is regularly longer, so reading it here and writing it back would quietly
  ; cut it off. See path.ps1.
  InitPluginsDir
  File "/oname=$PLUGINSDIR\path.ps1" "path.ps1"
  DetailPrint "Putting $INSTDIR on the PATH..."
  nsExec::ExecToStack 'powershell -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\path.ps1" -Action add -Directory "$INSTDIR"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    ; Not a failure of the installation: everything works from the full path, and an
    ; installer that stops here over a convenience would be worse than the inconvenience.
    DetailPrint "Could not change the PATH ($0). Use the full path, or add $INSTDIR by hand."
  ${EndIf}

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

; After the sections on purpose: NSIS gives a section its number where the section stands,
; so `${SecHub}` above the definition is not an error but a warning, and the flag would have
; been accepted and quietly done nothing.
Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "Cyberbrain needs 64-bit Windows."
    Abort
  ${EndIf}

  ; A silent install picks its components from the command line. Without this, `/S` can only
  ; ever install the default set — which leaves out the hub, and an unattended rollout that
  ; cannot install the one machine that collects is not an unattended rollout. It is also
  ; what lets CI install this the way a customer would and then check what happened.
  ;
  ;   cyberbrain-setup.exe /S           workstation
  ;   cyberbrain-setup.exe /S /HUB      workstation and the collector service
  ${GetParameters} $R0
  ${GetOptions} $R0 "/HUB" $R1
  ${IfNot} ${Errors}
    !insertmacro SelectSection ${SecHub}
  ${EndIf}
  ClearErrors
FunctionEnd

; The collector's service, if it was stopped to free the file. After every section rather
; than at the end of the main one, so that a run which also (re)registered the hub has
; finished doing so first; `net start` on a service that is already running says so and is
; ignored, which is why this needs no second flag.
Function .onInstSuccess
  ${If} $R3 == "1"
    nsExec::ExecToStack 'net start "CyberbrainHub"'
    Pop $R0
    Pop $R1
  ${EndIf}
FunctionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} $(DESC_SecMain)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} $(DESC_SecDesktop)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecHub} $(DESC_SecHub)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  ; A running Cyberbrain would leave its own exe behind and the directory with it, which is
  ; an uninstall that says it worked and did not.
  Call un.CloseCyberbrain

  ; The service first, while the program that can remove it is still on disk. It is quiet
  ; about not finding one: most installations never had it.
  nsExec::ExecToStack '"$INSTDIR\cyberbrain.exe" hub service uninstall'
  Pop $0
  Pop $1

  ; Out of the PATH again. A directory that no longer exists, left in the PATH of every
  ; process on the machine, is litter that outlives the program by years.
  InitPluginsDir
  File "/oname=$PLUGINSDIR\path.ps1" "path.ps1"
  nsExec::ExecToStack 'powershell -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\path.ps1" -Action remove -Directory "$INSTDIR"'
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
  ; Unconditional: an installation made by a GNU build is uninstalled by the uninstaller it
  ; wrote, but leaving the name here costs nothing and a stray DLL would keep $INSTDIR alive.
  Delete "$INSTDIR\WebView2Loader.dll"
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
