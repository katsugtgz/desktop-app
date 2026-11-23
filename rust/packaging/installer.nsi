; Station native shell — per-user Windows installer (NSIS 3.x).
;
; Contract: docs/rust-packaging-plan.md §5/§7. Everything is HKCU + $LOCALAPPDATA: no elevation,
; no admin, no SetShellVarContext all. The uninstaller removes only what this script installed and
; never touches the shell's user data (%APPDATA%\station, %APPDATA%\BrowserX).
;
; Build (from the repository root, after `cargo build --release -p station-shell`):
;   makensis /DVERSION=0.1.0 rust/packaging/installer.nsi
; Output: release/station-shell-setup-<VERSION>.exe
;
; All input/output paths below are relative to this script's directory (NSIS resolves them there),
; so the command works from any working directory. This script must not depend on the Electron
; tree: it uses rust/assets/station.ico, not packages/app/build/.

!ifndef VERSION
  !define VERSION "0.1.0"
!endif

!define APP_EXE   "..\target\release\station-shell.exe"
!define APP_ICON  "..\assets\station.ico"
!define WEBVIEW2_SETUP "dist\MicrosoftEdgeWebview2Setup.exe"

!define APP_NAME "Station"
!define PUBLISHER "Station"
!define WEBVIEW2_GUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Station"
!define PROTOCOL_KEY "Software\Classes\station"

Name "${APP_NAME}"
Caption "${APP_NAME} ${VERSION} Setup"
BrandingText "${APP_NAME} ${VERSION}"
OutFile "..\..\release\station-shell-setup-${VERSION}.exe"
InstallDir "$LOCALAPPDATA\Programs\Station"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"

; Per-user throughout: the process never asks for elevation.
RequestExecutionLevel user
Unicode true
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUninstDetails show

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "WordFunc.nsh"
!include "x64.nsh"

!define MUI_ABORTWARNING
!define MUI_ICON "${APP_ICON}"
!define MUI_UNICON "${APP_ICON}"
!define MUI_WELCOMEPAGE_TITLE "${APP_NAME} ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "This installs ${APP_NAME} for the current user only.$\r$\n$\r$\nNo administrator rights are needed, and your session and private apps are never touched by upgrades or uninstalls."

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; The shell is x64 (WebView2 child + Rune); refuse 32-bit Windows up front instead of installing
; an exe that cannot run.
Function .onInit
  SetShellVarContext current
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "${APP_NAME} requires 64-bit Windows (x64)."
    Abort
  ${EndIf}
FunctionEnd

Function un.onInit
  SetShellVarContext current
FunctionEnd

Section "${APP_NAME}" SecMain
  SetShellVarContext current
  SetOutPath "$INSTDIR"

  File "${APP_EXE}"
  File "${APP_ICON}"
  File "${WEBVIEW2_SETUP}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; WebView2 is a convenience install: the shell degrades (Rune chrome, no page rendering) rather
  ; than failing when the runtime is missing. A bootstrapper failure is logged and setup continues.
  Call InstallWebView2IfMissing

  ; station:// registration (the Electron contract's `protocols: [station]`).
  WriteRegStr HKCU "${PROTOCOL_KEY}" "" "URL:${APP_NAME}"
  WriteRegStr HKCU "${PROTOCOL_KEY}" "URL Protocol" ""
  WriteRegStr HKCU "${PROTOCOL_KEY}\shell\open\command" "" '"$INSTDIR\station-shell.exe" "%1"'

  ; Per-user Start Menu shortcut, carrying the installed icon file.
  CreateShortcut "$SMPROGRAMS\${APP_NAME}.lnk" "$INSTDIR\station-shell.exe" "" "$INSTDIR\station.ico" 0

  ; Add/Remove Programs entry (per-user; no Modify/Repair).
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\station.ico"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $0
SectionEnd

Section "Uninstall"
  SetShellVarContext current

  Delete "$SMPROGRAMS\${APP_NAME}.lnk"

  ; Only this script's files. RMDir (not /r) so anything the user put in $INSTDIR survives.
  Delete "$INSTDIR\station-shell.exe"
  Delete "$INSTDIR\station.ico"
  Delete "$INSTDIR\MicrosoftEdgeWebview2Setup.exe"
  Delete "$INSTDIR\webview2-install.log"
  Delete "$INSTDIR\Uninstall.exe"
  ; WebView2 creates its browser profile beside the exe (`station-shell.exe.WebView2`). It is
  ; app state in the install directory, not user data in %APPDATA%; left behind it keeps
  ; $INSTDIR alive after uninstall (~16 MB at this build).
  RMDir /r "$INSTDIR\station-shell.exe.WebView2"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${PROTOCOL_KEY}"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"

  ; Deliberately absent: any deletion of %APPDATA%\station or %APPDATA%\BrowserX. The shell's
  ; session (session.json) and private manifests (private-manifests.json) are user data.
SectionEnd

; WebView2 Evergreen runtime detection: the EdgeUpdate `pv` value under HKLM's WOW6432Node key,
; then the HKCU twin. Installed only when the value is absent or not newer than 0.0.0.0.
Function InstallWebView2IfMissing
  Push $0
  Push $1
  Push $2

  SetRegView 64
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  ${If} $0 == ""
    ReadRegStr $0 HKCU "Software\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  ${EndIf}
  SetRegView lastused

  ${If} $0 != ""
    ${VersionCompare} "$0" "0.0.0.0" $1
    ${If} $1 == "1"
      DetailPrint "WebView2 runtime $0 is already installed."
      Goto webview2_done
    ${EndIf}
    DetailPrint "WebView2 runtime $0 is not newer than 0.0.0.0; installing the Evergreen runtime."
  ${Else}
    DetailPrint "WebView2 runtime not found; installing the Evergreen runtime."
  ${EndIf}

  ExecWait '"$INSTDIR\MicrosoftEdgeWebview2Setup.exe" /silent /install' $2
  ${If} $2 != "0"
    DetailPrint "WebView2 bootstrapper exited with code $2; setup continues (the shell runs degraded until the runtime is installed)."
    FileOpen $1 "$INSTDIR\webview2-install.log" w
    FileWrite $1 "WebView2 bootstrapper exit code: $2$\r$\n"
    FileClose $1
    Goto webview2_done
  ${EndIf}
  DetailPrint "WebView2 runtime installed."

webview2_done:
  Pop $2
  Pop $1
  Pop $0
FunctionEnd
