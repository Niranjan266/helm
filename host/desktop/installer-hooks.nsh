; Tauri NSIS hooks for the optional "Stay reachable after restart" service (HelmSignIn).
;
; The service runs a copy of Helm.exe from %ProgramFiles%\Helm\SignIn (see src/reach.rs).
; This per-user installer never installs it; it only keeps an existing one in step:
;   * after install / update: refresh the service's copy of Helm.exe (asks for UAC once);
;   * before uninstall: remove the service and its copy (asks for UAC once), and delete the
;     machine-sealed identity copy from the user's data folder.
; Nothing happens when the service is not installed (sc query fails with 1060).

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToStack '"$SYSDIR\sc.exe" query HelmSignIn'
  Pop $0
  Pop $1
  ${If} $0 == 0
    ExecShellWait "runas" "$INSTDIR\Helm.exe" "--signin-refresh" SW_HIDE
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToStack '"$SYSDIR\sc.exe" query HelmSignIn'
  Pop $0
  Pop $1
  ${If} $0 == 0
    ExecShellWait "runas" "$INSTDIR\Helm.exe" "--signin-remove" SW_HIDE
  ${EndIf}
  Delete "$APPDATA\Helm\service-identity.json"
!macroend
