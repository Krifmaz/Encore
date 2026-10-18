; Encore desktop: the Windows installer (NSIS 3, Unicode).
;
; Built by scripts/desktop-package.sh, which passes:
;   -DVERSION=<the release version, e.g. 0.27.0 or dev.123>   shown in Apps & features
;   -DNUMVER=<X.Y.Z>                                           the file version (numbers only)
;   -DAPP_EXE=<path to encore-desktop.exe>  -DICON=<path to encore.ico>  -DOUTFILE=<installer path>
;
; Per user, no administrator rights: Encore goes to %LOCALAPPDATA%\Encore or the folder chosen (the
; in-app updater starts it again from InstallLocation below, else from that default folder) with a
; Start menu shortcut and an entry in Apps & features. /S installs
; silently (the in-app updater runs it that way, after Encore quit, and starts Encore again).
; The classic app (Tauri) installs into the same folder as Encore.exe with its own uninstall.exe
; and its own Apps & features entry; every file and key here has a different name, so either one
; can be installed or removed without touching the other. Only the Start menu shortcut (Encore.lnk)
; is shared: it opens whichever app was installed last, and each uninstaller removes it only while
; it points at its own app.
; The settings and offline songs (%APPDATA%\encore-desktop, %LOCALAPPDATA%\encore-desktop) are
; kept when Encore is uninstalled.

Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef NUMVER
  !define NUMVER "0.0.0"
!endif
!ifndef APP_EXE
  !error "pass -DAPP_EXE=<path to encore-desktop.exe>"
!endif
!ifndef ICON
  !error "pass -DICON=<path to encore.ico>"
!endif
!ifndef OUTFILE
  !define OUTFILE "Encore-${VERSION}-windows-x86_64.exe"
!endif

!define NAME "Encore"
!define EXE "encore-desktop.exe"
!define UNINSTALLER "uninstall-encore-desktop.exe"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\EncoreDesktop"
!define RUNKEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define APPROVEDKEY "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
; the name Settings → This computer → "Start with the computer" registers (src/desktop/autostart.rs ENTRY)
!define AUTOSTART "Encore Desktop"
!define SHORTCUT "$SMPROGRAMS\Encore.lnk"

Name "${NAME}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Encore"
; an earlier install in another folder is updated where it is
InstallDirRegKey HKCU "${UNINSTKEY}" "InstallLocation"
BrandingText "Encore ${VERSION}"

VIProductVersion "${NUMVER}.0"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "Encore installer"
VIAddVersionKey "CompanyName" "Encore"
VIAddVersionKey "LegalCopyright" "Encore, MIT licence"

!include "MUI2.nsh"
!include "FileFunc.nsh"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Start Encore"

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; Wait until no running Encore holds the program file (up to 10 s), then ask to quit it; a silent
; install (the updater, after Encore quit) closes a copy that is still running instead.
!macro WAIT_FOR_ENCORE UN
Function ${UN}WaitForEncore
  StrCpy $0 0
  retry:
    IfFileExists "$INSTDIR\${EXE}" 0 done
    ClearErrors
    Delete "$INSTDIR\${EXE}"
    IfErrors 0 done
    IntOp $0 $0 + 1
    IntCmp $0 20 still 0 still
    Sleep 500
    Goto retry
  still:
    IfSilent kill
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Encore is still running. Quit Encore (tray icon → Quit Encore), then press Retry." IDRETRY again
    Abort
  again:
    StrCpy $0 0
    Goto retry
  kill:
    nsExec::Exec 'taskkill /F /IM ${EXE}'
    Pop $1
    Sleep 1000
  done:
FunctionEnd
!macroend
!insertmacro WAIT_FOR_ENCORE ""
!insertmacro WAIT_FOR_ENCORE "un."

Section "Encore" SecMain
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  Call WaitForEncore
  File "/oname=${EXE}" "${APP_EXE}"
  File "/oname=encore.ico" "${ICON}"
  WriteUninstaller "$INSTDIR\${UNINSTALLER}"

  CreateShortcut "${SHORTCUT}" "$INSTDIR\${EXE}" "" "$INSTDIR\encore.ico" 0

  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\encore.ico"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "Encore"
  WriteRegStr HKCU "${UNINSTKEY}" "URLInfoAbout" "https://github.com/Krifmaz/Encore"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\${UNINSTALLER}"'
  WriteRegStr HKCU "${UNINSTKEY}" "QuietUninstallString" '"$INSTDIR\${UNINSTALLER}" /S'
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/M=${EXE} /S=0K /G=0" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINSTKEY}" "EstimatedSize" "$0"
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  Call un.WaitForEncore
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\encore.ico"
  Delete "$INSTDIR\${UNINSTALLER}"
  ; only when empty: the classic app may live in the same folder
  RMDir "$INSTDIR"

  ; the shortcut, unless it has become the classic app's since (same name, other target)
  IfFileExists "${SHORTCUT}" 0 shortcut_done
    nsExec::ExecToStack `powershell.exe -NoProfile -NonInteractive -Command "[Console]::Write((New-Object -ComObject WScript.Shell).CreateShortcut('${SHORTCUT}').TargetPath)"`
    Pop $0
    Pop $1
    StrCmp $1 "$INSTDIR\${EXE}" 0 shortcut_done
    Delete "${SHORTCUT}"
  shortcut_done:

  DeleteRegValue HKCU "${RUNKEY}" "${AUTOSTART}"
  DeleteRegValue HKCU "${APPROVEDKEY}" "${AUTOSTART}"
  DeleteRegKey HKCU "${UNINSTKEY}"
SectionEnd
