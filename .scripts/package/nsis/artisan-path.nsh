!include "LogicLib.nsh"
!include "WinMessages.nsh"

!macro customInstall
  nsExec::ExecToStack 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\resources\artisan-forge\update-user-path.ps1" -Action Add -BinPath "$INSTDIR\resources\artisan-forge"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    DetailPrint "Could not expose the ae command: $1"
    Abort "Artisan Editor could not install the ae command for the current user."
  ${EndIf}
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend

!macro customUnInstall
  nsExec::ExecToStack 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\resources\artisan-forge\update-user-path.ps1" -Action Remove -BinPath "$INSTDIR\resources\artisan-forge"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    DetailPrint "Could not remove the ae command from PATH: $1"
  ${EndIf}
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend
