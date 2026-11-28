; VisorGuBIMGal Windows installer (E9). Built by `npm run desktop:build` (scripts/desktop.mjs):
;   makensis /DVERSION=0.1.0 /DSTAGE=<staged folder> /DOUTFILE=<setup.exe> installer.nsi
; Per-user install (no administrator rights), as the Tauri installer did. Unsigned: code signing
; is the owner's decision (E6).
Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !error "VERSION is required"
!endif
!ifndef STAGE
  !error "STAGE is required"
!endif
!ifndef OUTFILE
  !error "OUTFILE is required"
!endif

!define APP "VisorGuBIMGal"
!define EXE "VisorGuBIMGal.exe"
!define PROGID "VisorGuBIMGal.ifc"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"
!define WEBVIEW2_GUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

Name "${APP}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\${APP}"
InstallDirRegKey HKCU "Software\${APP}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=1034 "ProductName" "${APP}"
VIAddVersionKey /LANG=1034 "FileDescription" "Instalador de ${APP}"
VIAddVersionKey /LANG=1034 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1034 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=1034 "LegalCopyright" "VisorGuBIMGal"

!define MUI_ICON "..\icons\icon.ico"
!define MUI_UNICON "..\icons\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Spanish"

Function .onInit
  ; WebView2 Runtime ships with Windows 11; warn on systems without it, the app needs it.
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  ${If} $0 == ""
    ReadRegStr $0 HKCU "Software\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  ${EndIf}
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    MessageBox MB_ICONEXCLAMATION|MB_OK "No se encontró Microsoft Edge WebView2 Runtime, que ${APP} necesita.$\r$\nInstálalo desde https://developer.microsoft.com/microsoft-edge/webview2/"
  ${EndIf}
FunctionEnd

Section "Install"
  SetOutPath "$INSTDIR"
  ; A previous version's web files must not linger next to the new ones.
  RMDir /r "$INSTDIR\app"
  File "${STAGE}\${EXE}"
  File "${STAGE}\NOTICE.md"
  SetOutPath "$INSTDIR\app"
  File /r "${STAGE}\app\*.*"
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Double-clicking an .ifc opens it here (E1).
  WriteRegStr HKCU "Software\Classes\.ifc" "" "${PROGID}"
  WriteRegStr HKCU "Software\Classes\.ifc\OpenWithProgids" "${PROGID}" ""
  WriteRegStr HKCU "Software\Classes\${PROGID}" "" "Modelo BIM IFC"
  WriteRegStr HKCU "Software\Classes\${PROGID}\DefaultIcon" "" "$INSTDIR\${EXE},0"
  WriteRegStr HKCU "Software\Classes\${PROGID}\shell\open\command" "" '"$INSTDIR\${EXE}" "%1"'
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'

  CreateShortCut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\${EXE}"
  WriteRegStr HKCU "Software\${APP}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "VisorGuBIMGal"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${EXE},0"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\${APP}.lnk"
  RMDir /r "$INSTDIR\app"
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\NOTICE.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  ; Give .ifc back only if it still points here.
  ReadRegStr $0 HKCU "Software\Classes\.ifc" ""
  ${If} $0 == "${PROGID}"
    DeleteRegValue HKCU "Software\Classes\.ifc" ""
  ${EndIf}
  DeleteRegValue HKCU "Software\Classes\.ifc\OpenWithProgids" "${PROGID}"
  DeleteRegKey /ifempty HKCU "Software\Classes\.ifc\OpenWithProgids"
  DeleteRegKey /ifempty HKCU "Software\Classes\.ifc"
  DeleteRegKey HKCU "Software\Classes\${PROGID}"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\${APP}"
SectionEnd
